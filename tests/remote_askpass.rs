//! The askpass helper contract: what `ssh` executes, and what it gets back.
//!
//! `ssh` reads a password from `/dev/tty`, so a prompt raised while the TUI owns the terminal
//! paints over the UI and eats the keystrokes meant for it. Rozi points `SSH_ASKPASS` at its own
//! binary instead; these drive that re-executed binary for real, because the two halves of the
//! contract — how ssh invokes the helper, and how the helper answers it — live in different
//! processes and no in-process test can hold both.
//!
//! The broker side is played by hand here (the real one needs a mounted `AppRoot`), speaking the
//! same framed JSON over the same platform IPC endpoint the client binds.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rozi::platform::ipc::{IpcEndpoint, IpcListener};
use rozi::session::protocol::{read_frame, write_frame};

// Not `tempfile::tempdir`: the platform layer requires an endpoint's parent to be private to the
// current user, and on Windows a temp directory carries its container's inheritable ACEs rather
// than a protected DACL of its own, so `bind` refuses it with `PermissionDenied`. Creating the
// directory the way the runtime does is what lets a test endpoint bind at all.
use common::private_temp_dir;

/// One prompt, answered by `reply`. Returns the request the helper sent, so a test can assert the
/// prompt and kind reached the UI intact.
fn serve_one(listener: IpcListener, reply: serde_json::Value) -> serde_json::Value {
    let mut conn = listener.accept().expect("helper connects");
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let request: serde_json::Value = read_frame(&mut conn).expect("helper sends a request");
    write_frame(&mut conn, &reply).expect("reply reaches the helper");
    request
}

struct HelperRun {
    request: serde_json::Value,
    stdout: String,
    success: bool,
}

/// A socket name no other endpoint in this binary shares.
///
/// A private directory per test is not enough on Windows: the pipe name is derived from the path's
/// file *stem* alone (see `IpcEndpoint::pipe_name`), so two endpoints under different directories
/// but with the same file name are one pipe, and `FILE_FLAG_FIRST_PIPE_INSTANCE` fails whichever
/// binds second with `Access is denied`. These tests run in parallel in one binary, so the name has
/// to carry the uniqueness the directory cannot.
fn unique_socket(prefix: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Run the real binary the way `ssh` runs an askpass program: prompt as the argument, endpoint and
/// token in the environment.
fn run_helper(dir: &Path, prompt: &str, token: &str, reply: serde_json::Value) -> HelperRun {
    run_helper_in_session(dir, prompt, token, "session-1", reply)
}

fn run_helper_in_session(
    dir: &Path,
    prompt: &str,
    token: &str,
    session: &str,
    reply: serde_json::Value,
) -> HelperRun {
    let endpoint = IpcEndpoint::at_path(dir.join(unique_socket("askpass-test")));
    let listener = endpoint
        .bind()
        .expect("bind broker endpoint")
        .into_listener();
    let broker = std::thread::spawn(move || serve_one(listener, reply));

    let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .arg(prompt)
        .env("ROZI_ASKPASS_ENDPOINT", endpoint.path())
        .env("ROZI_ASKPASS_TOKEN", token)
        .env("ROZI_ASKPASS_SESSION", session)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run rozi as the askpass helper");

    HelperRun {
        request: broker.join().expect("broker thread"),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        success: output.status.success(),
    }
}

/// The whole point: a password typed into the UI reaches ssh on the helper's stdout, and the
/// terminal is never involved.
#[test]
fn the_helper_prints_the_answer_the_ui_gave_it() {
    let dir = private_temp_dir();
    let run = run_helper(
        &dir,
        "dev@workbox's password: ",
        "0123456789abcdef",
        serde_json::json!({ "answer": { "text": "hunter2" } }),
    );

    assert_eq!(run.request["token"], "0123456789abcdef");
    // Which ssh is asking: the UI reads a repeat from this same value as "that answer was
    // rejected", and a refusal as covering everything else this connection asks.
    assert_eq!(run.request["session"], "session-1");
    assert_eq!(run.request["kind"], "secret");
    assert_eq!(run.request["prompt"], "dev@workbox's password: ");
    // Trailing newline included: ssh reads the answer as one line.
    assert_eq!(run.stdout, "hunter2\n");
    assert!(run.success, "the helper reports success to ssh");
}

/// Host-key verification is the prompt worth *not* masking, and ssh sets no prompt-kind hint for
/// it — the text is all the helper has to go on.
#[test]
fn a_host_key_question_is_classified_as_a_confirmation() {
    let dir = private_temp_dir();
    let prompt = "The authenticity of host 'workbox (192.0.2.7)' can't be established.\n\
         ED25519 key fingerprint is SHA256:qJv1zH.\n\
         Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
    let run = run_helper(
        &dir,
        prompt,
        "token",
        serde_json::json!({ "answer": { "text": "yes" } }),
    );

    assert_eq!(run.request["kind"], "confirm");
    assert_eq!(run.request["prompt"], prompt);
    assert_eq!(run.stdout, "yes\n");
}

/// Dismissing the modal has to fail the connection. Anything else — printing an empty answer,
/// falling back to the terminal — would either loop ssh on a wrong password or reintroduce the
/// prompt this whole path exists to keep off the screen.
#[test]
fn cancelling_fails_the_connection_instead_of_answering_it() {
    let dir = private_temp_dir();
    let run = run_helper(
        &dir,
        "dev@workbox's password: ",
        "token",
        serde_json::json!("cancel"),
    );

    assert!(run.stdout.is_empty(), "nothing is offered to ssh");
    assert!(!run.success, "a non-zero exit is what stops ssh retrying");
}

/// The environment decides, not the argv. `ssh` passes the prompt as an ordinary argument, and a
/// prompt can look like anything — so helper mode has to be settled before the CLI parser ever
/// sees it, or a prompt shaped like a flag would run that flag instead of being asked.
#[test]
fn the_environment_puts_the_binary_in_helper_mode_before_cli_parsing() {
    let dir = private_temp_dir();
    let run = run_helper(
        &dir,
        "--version",
        "token",
        serde_json::json!({ "answer": { "text": "answered" } }),
    );

    assert_eq!(run.request["prompt"], "--version");
    assert_eq!(run.stdout, "answered\n");
}

/// The same argv without the environment is a plain launch: `--version` prints a version, not an
/// answer to a prompt nobody asked for.
#[test]
fn without_the_endpoint_variable_the_binary_is_not_a_helper() {
    let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .arg("--version")
        .env_remove("ROZI_ASKPASS_ENDPOINT")
        .env_remove("ROZI_ASKPASS_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run rozi");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "plain --version succeeds");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected a version, got: {stdout}"
    );
}

/// A broker that will not answer — a rejected token, a UI that went away — leaves the helper with
/// a closed connection. It must fail its ssh rather than print something ssh would try to use.
#[test]
fn a_broker_that_answers_nothing_fails_the_connection() {
    let dir = private_temp_dir();
    let endpoint = IpcEndpoint::at_path(dir.join(unique_socket("askpass-reject")));
    let listener = endpoint.bind().expect("bind").into_listener();
    // What the real broker does with a token that is not its own: read the request, say nothing,
    // drop the connection.
    let broker = std::thread::spawn(move || {
        let mut conn = listener.accept().expect("helper connects");
        conn.set_read_timeout(Some(Duration::from_secs(30)))
            .expect("read timeout");
        let _: serde_json::Value = read_frame(&mut conn).expect("helper sends a request");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .arg("dev@workbox's password: ")
        .env("ROZI_ASKPASS_ENDPOINT", endpoint.path())
        .env("ROZI_ASKPASS_TOKEN", "not-the-clients-token")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run rozi as the askpass helper");
    broker.join().expect("broker thread");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// Every helper one ssh invocation runs inherits that invocation's session, so its three retries
/// arrive as one conversation. Losing this would put the UI back to guessing from elapsed time.
#[test]
fn every_helper_from_one_invocation_reports_the_same_session() {
    let dir = private_temp_dir();
    let answer = serde_json::json!({ "answer": { "text": "hunter2" } });
    let first = run_helper_in_session(&dir, "p: ", "t", "ssh-7", answer.clone());
    let retry = run_helper_in_session(&dir, "p: ", "t", "ssh-7", answer.clone());
    let next = run_helper_in_session(&dir, "p: ", "t", "ssh-8", answer);

    assert_eq!(first.request["session"], retry.request["session"]);
    assert_ne!(first.request["session"], next.request["session"]);
}
