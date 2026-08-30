//! Capture OpenSSH's interactive prompts instead of letting them reach the terminal.
//!
//! `ssh` reads a password, a key passphrase, or a host-key confirmation from `/dev/tty` — not from
//! the stdin rozi handed it. A prompt raised while the TUI owns the terminal therefore paints over
//! the running UI *and* swallows the keystrokes meant for it, which neither side recovers from.
//!
//! `SSH_ASKPASS_REQUIRE=force` (OpenSSH 8.4+) redirects every one of those prompts to a helper
//! program, and this module makes rozi its own helper. The client binds a private endpoint,
//! [`configure`] points each ssh/scp child at it, and the re-executed `rozi` relays the prompt to
//! the running UI and prints back whatever the user typed. Nothing touches the terminal.
//!
//! The broker exists only while the TUI is up, and [`configure`] is a no-op without it. Every
//! pre-TUI path — the `--remote` startup install prompt, `rozi list-sessions --remote` — therefore
//! keeps ssh's ordinary terminal prompt, which is the right answer when there is no UI to cover.
//!
//! A bind failure is not fatal and is not reported: it leaves prompts on the terminal exactly as
//! they were before this module existed, and a runtime directory rozi cannot bind in has already
//! failed the session endpoints the user will hear about first.

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::CommandLink;

use crate::Msg;
use crate::platform::ipc::{IpcConnection, IpcEndpoint, IpcListener};
use crate::session::protocol::{read_frame_with_limit, write_frame};

/// Endpoint the helper connects back to. Set by [`configure`] on the ssh/scp child alone, so it
/// never reaches a pane's shell or the remote end of the connection.
const ENDPOINT_ENV: &str = "ROZI_ASKPASS_ENDPOINT";
/// Proof the helper is one rozi launched rather than an unrelated process that guessed the
/// endpoint name. The endpoint already lives in the private runtime directory; this is the second
/// lock on a door that only opens for prompts.
const TOKEN_ENV: &str = "ROZI_ASKPASS_TOKEN";
/// Which ssh is asking. Minted per [`configure`] call — that is, once per ssh or scp invocation —
/// and inherited by every helper that invocation runs, so all three of its retries carry one value
/// and the next invocation carries a different one.
///
/// This is what lets the UI tell "ssh asking again about the answer you just gave" from "a new
/// connection asking for the first time", which decides whether a refusal still stands and whether
/// a repeated question means the last answer was rejected. A pid would say the same thing, but
/// there is no cross-platform way to read one, and a nonce set before the spawn needs no OS.
const SESSION_ENV: &str = "ROZI_ASKPASS_SESSION";
/// OpenSSH sets this to `confirm` for a yes/no question and `none` for an informational message.
/// It has existed since 8.4 but is empty for several prompts that still want a yes/no answer
/// (host-key verification among them), so [`kind_for`] reads the prompt text as well.
const SSH_PROMPT_KIND_ENV: &str = "SSH_ASKPASS_PROMPT";

/// A prompt and its answer are each a line of text. Anything larger is a peer we should not be
/// talking to, so the frame limit stays far below the session protocol's.
const MAX_FRAME: usize = 64 * 1024;

/// How long the helper waits before giving up and failing its ssh. Long enough to go find a
/// password manager, short enough that a modal nobody answers cannot pin an ssh process forever.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// What the prompt is asking for, which decides whether the answer is masked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskpassKind {
    /// A password or key passphrase. Masked, and never echoed anywhere.
    Secret,
    /// A yes/no question — host-key verification, agent key confirmation. Shown in clear: the
    /// fingerprint the user is checking is the whole point of the question.
    Confirm,
}

impl AskpassKind {
    pub fn is_secret(self) -> bool {
        matches!(self, Self::Secret)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AskpassRequest {
    token: String,
    session: String,
    kind: AskpassKind,
    prompt: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AskpassReply {
    Answer { text: String },
    Cancel,
}

struct Broker {
    endpoint: IpcEndpoint,
    token: String,
}

static BROKER: OnceLock<Broker> = OnceLock::new();
/// Claimed once so a second [`start`] cannot bind a second endpoint while the first is still
/// coming up.
static STARTED: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Prompt workers waiting on the UI, by request id. A `Vec` rather than a map: at most a couple of
/// ssh processes ever prompt at once, and `HashMap::new` is not const.
static PENDING: Mutex<Vec<(u64, SyncSender<AskpassReply>)>> = Mutex::new(Vec::new());

/// Bind the endpoint and start answering prompts. Idempotent; safe to call before any remote host
/// is configured, since nothing connects until an ssh child actually prompts.
pub(crate) fn start(link: CommandLink<Msg>) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok((broker, listener)) = bind() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("rozi-askpass".to_string())
        .spawn(move || accept_loop(listener, link));
    if spawned.is_ok() {
        let _ = BROKER.set(broker);
    }
}

fn bind() -> io::Result<(Broker, IpcListener)> {
    let dir = crate::control::runtime_dir()?;
    // Named by pid like the control endpoint, so a crashed predecessor's leftover is reclaimed by
    // `bind` (which replaces only an endpoint nothing answers) rather than accumulating.
    let endpoint = IpcEndpoint::at_path(dir.join(format!("askpass-{}.sock", std::process::id())));
    let bound = endpoint.bind()?;
    let broker = Broker {
        endpoint: bound.endpoint().clone(),
        token: fresh_token(),
    };
    Ok((broker, bound.into_listener()))
}

/// Whether an ssh spawned now could raise a prompt in the UI — that is, whether the broker is up.
/// Callers use it to decide how long a wait might legitimately include a person typing.
pub(crate) fn may_prompt() -> bool {
    BROKER.get().is_some()
}

/// Retire the endpoint on the way out, so a quit does not leave a socket behind in the runtime
/// directory. The accept thread dies with the process; only the on-disk trace needs retiring.
pub(crate) fn shutdown() {
    if let Some(broker) = BROKER.get() {
        broker.endpoint.remove_stale();
    }
}

fn accept_loop(listener: IpcListener, link: CommandLink<Msg>) {
    loop {
        match listener.accept() {
            Ok(conn) => {
                let link = link.clone();
                // One thread per prompt: the worker blocks until the user answers, and a second
                // ssh must not queue behind it at the accept.
                let _ = std::thread::Builder::new()
                    .name("rozi-askpass-prompt".to_string())
                    .spawn(move || serve(conn, link));
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn serve(mut conn: IpcConnection, link: CommandLink<Msg>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(request) = read_frame_with_limit::<_, AskpassRequest>(&mut conn, MAX_FRAME) else {
        return;
    };
    let Some(broker) = BROKER.get() else {
        return;
    };
    if !token_matches(&request.token, &broker.token) {
        return;
    }

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = sync_channel(1);
    if let Ok(mut pending) = PENDING.lock() {
        pending.push((id, tx));
    } else {
        return;
    }
    link.send(Msg::RemoteAskpassPrompt {
        id,
        session: request.session,
        kind: request.kind,
        prompt: request.prompt,
    });

    let reply = match rx.recv_timeout(ANSWER_TIMEOUT) {
        Ok(reply) => reply,
        Err(RecvTimeoutError::Timeout) => {
            // The modal is still up in front of a user who walked away; take it down with the ssh
            // it belonged to rather than leaving a dialog that answers nothing.
            link.send(Msg::RemoteAskpassExpired { id });
            AskpassReply::Cancel
        }
        Err(RecvTimeoutError::Disconnected) => AskpassReply::Cancel,
    };
    forget(id);

    let _ = conn.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = write_frame(&mut conn, &reply);
}

fn forget(id: u64) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.retain(|(pending_id, _)| *pending_id != id);
    }
}

fn take(id: u64) -> Option<SyncSender<AskpassReply>> {
    let mut pending = PENDING.lock().ok()?;
    let index = pending
        .iter()
        .position(|(pending_id, _)| *pending_id == id)?;
    Some(pending.swap_remove(index).1)
}

/// Hand the user's answer to the waiting ssh. Answering an id twice, or answering one whose helper
/// already gave up, is a no-op.
pub(crate) fn answer(id: u64, text: String) {
    if let Some(tx) = take(id) {
        let _ = tx.send(AskpassReply::Answer { text });
    }
}

/// Refuse the prompt. `ssh` treats a helper that declines as an authentication failure and stops,
/// which is what makes Esc a way out of a host that keeps asking.
pub(crate) fn cancel(id: u64) {
    if let Some(tx) = take(id) {
        let _ = tx.send(AskpassReply::Cancel);
    }
}

/// Point one ssh or scp child at this client's broker.
///
/// Overrides any `SSH_ASKPASS` the user's environment already carries: a desktop askpass would pop
/// a window for a terminal the multiplexer is drawing, and the whole point here is that the answer
/// is collected in the UI the user is looking at.
pub(crate) fn configure(command: &mut Command) {
    let Some(broker) = BROKER.get() else {
        return;
    };
    let Some(exe) = crate::platform::paths::current_binary() else {
        return;
    };
    command
        .env("SSH_ASKPASS", exe)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(ENDPOINT_ENV, broker.endpoint.path())
        .env(TOKEN_ENV, &broker.token)
        .env(SESSION_ENV, fresh_token());
}

/// This binary re-executed by `ssh` as its askpass helper: the prompt arrives as the argument, the
/// endpoint and token in the environment.
///
/// Recognized before CLI parsing, because the prompt text is not an argument `rozi` accepts and
/// there is no argv shape that could be mistaken for one. Both environment variables are required,
/// and [`configure`] is the only thing that sets either.
pub struct Helper {
    endpoint: IpcEndpoint,
    token: String,
    session: String,
    kind: AskpassKind,
    prompt: String,
}

pub fn helper_invocation() -> Option<Helper> {
    let endpoint = std::env::var_os(ENDPOINT_ENV).filter(|path| !path.is_empty())?;
    let token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty())?;
    // OpenSSH passes the whole prompt as a single argument, but joining is free insurance against
    // a build that splits it.
    let prompt = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    Some(Helper {
        endpoint: IpcEndpoint::at_path(PathBuf::from(endpoint)),
        token,
        session: std::env::var(SESSION_ENV).unwrap_or_default(),
        kind: kind_for(&prompt),
        prompt,
    })
}

impl Helper {
    /// Ask the running UI, print the answer the way `ssh` reads it, and exit.
    ///
    /// A failure here exits non-zero rather than falling back to the terminal. Falling back would
    /// reintroduce the exact prompt this module exists to keep off a terminal the TUI is drawing
    /// on; failing the connection is recoverable, a scrambled UI is not.
    pub fn run(self) -> ! {
        match self.request() {
            Ok(AskpassReply::Answer { text }) => {
                use std::io::Write as _;
                let mut stdout = io::stdout().lock();
                if stdout
                    .write_all(text.as_bytes())
                    .and_then(|()| stdout.write_all(b"\n"))
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    std::process::exit(1);
                }
                std::process::exit(0)
            }
            Ok(AskpassReply::Cancel) | Err(_) => std::process::exit(1),
        }
    }

    fn request(&self) -> io::Result<AskpassReply> {
        let mut conn = self.endpoint.connect()?;
        conn.set_write_timeout(Some(Duration::from_secs(5)))?;
        write_frame(
            &mut conn,
            &AskpassRequest {
                token: self.token.clone(),
                session: self.session.clone(),
                kind: self.kind,
                prompt: self.prompt.clone(),
            },
        )?;
        // No read timeout: the answer arrives when the user finishes typing, and the broker's own
        // `ANSWER_TIMEOUT` is what bounds the wait.
        conn.set_read_timeout(None)?;
        read_frame_with_limit(&mut conn, MAX_FRAME)
    }
}

/// Whether the prompt wants a secret or a yes/no answer.
///
/// `SSH_ASKPASS_PROMPT` is authoritative when OpenSSH sets it, but it is empty for host-key
/// verification — the prompt most worth showing in clear — so the text decides when it is not.
fn kind_for(prompt: &str) -> AskpassKind {
    match std::env::var(SSH_PROMPT_KIND_ENV).as_deref() {
        Ok("confirm") | Ok("none") => return AskpassKind::Confirm,
        Ok("password") => return AskpassKind::Secret,
        _ => {}
    }
    let lowered = prompt.to_ascii_lowercase();
    if lowered.contains("(yes/no") || lowered.contains("type 'yes'") {
        AskpassKind::Confirm
    } else {
        AskpassKind::Secret
    }
}

/// Length-checked, branch-free-ish comparison. The endpoint's directory permissions are the real
/// boundary; this only keeps a wrong token from being told it was close.
fn token_matches(candidate: &str, expected: &str) -> bool {
    if candidate.len() != expected.len() {
        return false;
    }
    candidate
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn fresh_token() -> String {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_verification_is_a_confirmation_even_though_ssh_sets_no_prompt_kind() {
        let prompt = "The authenticity of host 'localhost (::1)' can't be established.\n\
             ED25519 key fingerprint is: SHA256:abc\n\
             Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(kind_for(prompt), AskpassKind::Confirm);
        assert_eq!(
            kind_for("Please type 'yes', 'no' or the fingerprint: "),
            AskpassKind::Confirm
        );
    }

    #[test]
    fn a_password_prompt_is_masked() {
        assert_eq!(kind_for("dev@workbox's password: "), AskpassKind::Secret);
        assert_eq!(
            kind_for("Enter passphrase for key '/home/dev/.ssh/id_ed25519': "),
            AskpassKind::Secret
        );
    }

    #[test]
    fn a_token_of_the_wrong_length_or_the_wrong_bytes_is_refused() {
        let expected = fresh_token();
        assert!(token_matches(&expected, &expected));
        assert!(!token_matches("", &expected));
        assert!(!token_matches(&expected[..expected.len() - 1], &expected));
        let mut wrong = expected.clone();
        wrong.replace_range(0..1, if expected.starts_with('a') { "b" } else { "a" });
        assert!(!token_matches(&wrong, &expected));
    }

    #[test]
    fn fresh_tokens_do_not_repeat() {
        assert_ne!(fresh_token(), fresh_token());
    }
}
