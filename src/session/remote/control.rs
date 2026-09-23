//! `rozi --remote <HOST> --session <NAME> <command>`: one control request, forwarded over SSH.
//!
//! The local CLI parses the command exactly as it does for a local session, then hands the
//! resulting [`ControlRequest`] to a rozi on the far host to run against *its* session server. The
//! answer comes back as the same [`ControlResponse`] the local endpoint would have produced, so
//! the caller's output, exit code, and error wording do not depend on which machine served it.
//!
//! What is forwarded is the request, not the command line. The far side never re-parses argv, so
//! a flag spelled differently by an older or newer rozi cannot change what runs; and the reply is
//! rendered here, so a remote command in a terminal prints the human table rather than the JSON a
//! pipe would have got on the far host.
//!
//! No new transport. This is the SSH path `--remote` attach already uses: saved host config,
//! connection multiplexing, the discovered remote binary, and the same askpass rules.

use std::io::{Read, Write};
use std::process::Stdio;

use crate::config::RemoteConfig;
use crate::control::{ControlRequest, ControlResponse};

use super::target::RemoteTarget;
use super::{
    ResolvedRemote, append_ssh_destination, quote_remote_executable, ssh_base_command,
    validate_remote_executable_token, validate_remote_target,
};

/// Exit status the far side uses for "the session could not be reached at all", as distinct from a
/// command the session answered by refusing. Mirrors what the local CLI exits with in that case.
pub const UNREACHABLE_EXIT: i32 = 2;

/// Run one control request against a session on `target` and return what it answered.
///
/// `Err` means the request never reached a session server - ssh failed, the host has no usable
/// rozi, or the named session is not running there. A command the session *refused* comes back as
/// `Ok` with a response carrying `ok: false`, exactly as a local session's refusal does.
pub fn forward_control(
    target: &RemoteTarget,
    session: &str,
    request: &ControlRequest,
    config: &RemoteConfig,
) -> Result<ControlResponse, String> {
    if !crate::session::discovery::valid_attach_target(session) {
        return Err("invalid session name".to_string());
    }
    validate_remote_target(target)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let remote_bin = super::binary::resolve(target, config)?;
    validate_remote_executable_token(&remote_bin)?;

    let payload = serde_json::to_vec(request)
        .map_err(|err| format!("could not encode the control request: {err}"))?;

    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    command.arg(quote_remote_executable(&remote_bin));
    command.arg("--remote-control");
    command.arg(session);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("remote control ssh failed: {err}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "remote control ssh gave no stdin".to_string())?;
        stdin
            .write_all(&payload)
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|err| format!("could not send the control request: {err}"))?;
        // Dropped here: the far side reads one request and then waits for end of input, and a
        // wait holds the connection open for as long as it takes.
    }
    // Deliberately unbounded. `agents wait` and `agents prompt --wait` are supposed to take as
    // long as the thing they are waiting for; the deadline belongs to the request, and the far
    // side's session server enforces it.
    let output = child
        .wait_with_output()
        .map_err(|err| format!("remote control ssh failed: {err}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        super::binary::invalidate(target, config);
        return Err(control_forward_failure(
            output.status.code(),
            &stderr,
            session,
            &target.display_label(),
        ));
    }
    parse_response(&output.stdout)
}

/// Find the answer in whatever else the remote login put on stdout.
///
/// A shell profile that prints a message of the day, a banner, or a `logout` line shares this
/// stdout with the answer, and neither end controls what a host's `/etc/profile` does. Rozi's own
/// host probe is already deliberately tolerant of that; this has to be too, or a forwarded command
/// fails on a perfectly working machine for a reason that has nothing to do with the command.
///
/// Searched from the end, because the answer is the last thing this process writes and a banner is
/// among the first. Being a parseable [`ControlResponse`] is the test: every line is tried, so
/// output on either side of the answer is skipped rather than mistaken for it.
fn parse_response(stdout: &[u8]) -> Result<ControlResponse, String> {
    let mut lines = stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .peekable();
    if lines.peek().is_none() {
        return Err("the remote rozi answered with nothing".to_string());
    }
    lines
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|line| serde_json::from_slice(line).ok())
        .ok_or_else(|| {
            "could not read the remote rozi's answer: no control response in its output".to_string()
        })
}

/// Turn a failed forwarding attempt into one line worth reading.
///
/// Two things need saying that the raw stderr does not say. A remote rozi old enough not to know
/// `--remote-control` rejects it in its argument parser, which reads as a typo in the caller's
/// command rather than as two versions disagreeing. And the far side's own advice was written for
/// someone standing on that machine, so the commands it names have to be retargeted at the host the
/// caller is actually addressing.
fn control_forward_failure(code: Option<i32>, stderr: &str, session: &str, host: &str) -> String {
    let stderr = stderr.trim();
    if stderr.contains("unknown flag `--remote-control`") {
        return format!(
            "remote control failed: the rozi on `{host}` is too old to run control commands (update rozi there)"
        );
    }
    if stderr.is_empty() {
        return match code {
            Some(UNREACHABLE_EXIT) => {
                format!("no running session named `{session}` on `{host}`")
            }
            Some(code) => format!("remote control failed: the rozi on `{host}` exited {code}"),
            None => "remote control failed: ssh was terminated by a signal".to_string(),
        };
    }
    let stderr = stderr.replace(
        "run `rozi sessions list`",
        &format!("run `rozi sessions list --remote {host}`"),
    );
    format!("remote control failed: {stderr}")
}

/// `--remote-control <NAME>`: the far side of [`forward_control`].
///
/// Reads one JSON [`ControlRequest`] from stdin, runs it against the named local session, and
/// writes the [`ControlResponse`] as one JSON line. Whether the command succeeded is carried in
/// that response, not in this process's exit status: a non-zero exit means the session could not
/// be reached at all, which is a different thing the caller reports differently.
pub fn run_remote_control(session: &str) -> Result<ControlResponse, String> {
    let mut payload = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut payload)
        .map_err(|err| format!("could not read the control request: {err}"))?;
    let request: ControlRequest = serde_json::from_str(payload.trim())
        .map_err(|err| format!("could not parse the control request: {err}"))?;
    crate::session::headless::run_session_control(session, request).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_session_name_never_reaches_ssh() {
        let target = RemoteTarget::Alias("workbox".to_string());
        let request = ControlRequest {
            command: crate::control::ControlCommand::ListPanes,
            source_pane: None,
            extension: None,
        };
        for name in ["../escape", "dev;rm -rf /", "", "dev\npanes"] {
            assert_eq!(
                forward_control(&target, name, &request, &RemoteConfig::default()),
                Err("invalid session name".to_string()),
                "accepted {name:?}"
            );
        }
    }

    /// The answer is a `ControlResponse` either way; only the transport can fail here. A refusal
    /// must survive the trip intact, because the caller's exit code is read off it.
    #[test]
    fn a_refusal_comes_back_as_an_answer_rather_than_a_transport_failure() {
        let refusal = ControlResponse::error_with(
            crate::control::ControlErrorCode::PaneNotFound,
            "pane 3 not found",
        );
        let encoded = serde_json::to_vec(&refusal).expect("response encodes");

        let parsed = parse_response(&encoded).expect("a refusal is still an answer");

        assert!(!parsed.ok);
        assert_eq!(
            parsed.code,
            Some(crate::control::ControlErrorCode::PaneNotFound)
        );
    }

    /// A host whose `/etc/profile` prints a message of the day shares this stdout with the answer.
    /// Neither end gets to decide that, so reading only the first line made every forwarded command
    /// fail on an otherwise working machine.
    #[test]
    fn the_answer_is_found_past_whatever_the_login_printed() {
        let response = ControlResponse::ok(serde_json::json!({"panes": []}));
        let encoded = serde_json::to_vec(&response).expect("response encodes");

        let mut stdout =
            b"Welcome to workbox!\n  \n* 3 packages can be updated.\nLast login: Tue\n".to_vec();
        stdout.extend_from_slice(&encoded);
        // Some logins have something to say on the way out, too.
        stdout.extend_from_slice(b"\nlogout\n");

        assert!(
            parse_response(&stdout)
                .expect("answer found past the banner")
                .ok,
            "the answer was lost among the login's own output"
        );
    }

    #[test]
    fn output_with_no_answer_in_it_says_so() {
        assert_eq!(
            parse_response(b"   \n\n"),
            Err("the remote rozi answered with nothing".to_string())
        );
        assert!(
            parse_response(b"Welcome to workbox!\nLast login: Tue\n")
                .expect_err("a banner alone is not an answer")
                .contains("no control response")
        );
    }

    /// Version skew has to be named. `unknown flag` alone reads like a typo in the user's command
    /// rather than a rozi on the far host that predates the feature.
    #[test]
    fn an_old_remote_rozi_is_named_as_the_problem() {
        let skew = control_forward_failure(
            Some(1),
            "unknown flag `--remote-control`\nRun `rozi --help` for usage.",
            "dev",
            "workbox",
        );
        assert!(skew.contains("too old"), "{skew}");
        assert!(skew.contains("workbox"), "{skew}");

        assert!(
            control_forward_failure(Some(UNREACHABLE_EXIT), "", "dev", "workbox")
                .contains("no running session named `dev` on `workbox`")
        );
        assert!(
            control_forward_failure(
                Some(255),
                "ssh: connect to host workbox port 22: Connection refused",
                "dev",
                "workbox",
            )
            .contains("Connection refused")
        );
    }

    /// The far side's advice was written for someone standing on that machine. Repeating it here
    /// would send the caller to look for the session on the wrong host.
    #[test]
    fn the_remote_rozis_own_advice_is_retargeted_at_the_host_being_addressed() {
        let message = control_forward_failure(
            Some(UNREACHABLE_EXIT),
            "no running session named `dev` (run `rozi sessions list`)",
            "dev",
            "workbox",
        );

        assert!(
            message.contains("rozi sessions list --remote workbox"),
            "{message}"
        );
    }
}
