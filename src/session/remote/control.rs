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

use std::io::Read;

use crate::config::RemoteConfig;
use crate::control::{ControlRequest, ControlResponse};

use super::json_transport::{self, TransportError};
use super::target::RemoteTarget;

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
    json_transport::forward(target, config, &["--remote-control", session], request).map_err(
        |error| match error {
            TransportError::Preflight(message) => message,
            TransportError::Encode(error) => {
                format!("could not encode the control request: {error}")
            }
            TransportError::Ssh(error) => format!("remote control ssh failed: {error}"),
            TransportError::Send(error) => format!("could not send the control request: {error}"),
            TransportError::Failed(output) => control_forward_failure(
                output.status.code(),
                &String::from_utf8_lossy(&output.stderr),
                session,
                &target.display_label(),
            ),
            TransportError::Answer(json_transport::AnswerError::Empty) => {
                "the remote rozi answered with nothing".into()
            }
            TransportError::Answer(json_transport::AnswerError::Malformed) => {
                "could not read the remote rozi's answer: no control response in its output".into()
            }
        },
    )
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
    let mut request: ControlRequest = serde_json::from_str(payload.trim())
        .map_err(|err| format!("could not parse the control request: {err}"))?;
    // A relative recording path means the directory this runner starts in: the remote login's.
    if let Ok(cwd) = std::env::current_dir() {
        request.command.resolve_output_against(&cwd);
    }
    if matches!(
        request.command,
        crate::control::ControlCommand::Subscribe { .. }
    ) {
        let (mut stream, response) =
            crate::session::headless::run_session_subscribe(session, request)
                .map_err(|err| err.to_string())?;
        println!("{}", serde_json::to_string(&response).unwrap());
        use std::io::Write;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        if !response.ok {
            return Ok(response);
        }
        let mut stdout = std::io::stdout().lock();
        loop {
            match crate::session::protocol::read_frame::<_, crate::session::protocol::ServerMessage>(
                &mut stream,
            ) {
                Ok(crate::session::protocol::ServerMessage::SessionSubscribeRecord { record }) => {
                    let is_lagged = matches!(
                        record,
                        crate::session::server::events::SessionSubscribeRecord::Lagged
                    );
                    writeln!(stdout, "{}", serde_json::to_string(&record).unwrap())
                        .map_err(|e| e.to_string())?;
                    stdout.flush().map_err(|e| e.to_string())?;
                    if is_lagged {
                        break;
                    }
                }
                Ok(crate::session::protocol::ServerMessage::Error { code, message }) => {
                    eprintln!("{code}: {message}");
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(err) => {
                    return Err(format!("remote subscribe connection error: {err}"));
                }
                _ => {}
            }
        }
        return Ok(ControlResponse::empty());
    }
    crate::session::headless::run_session_control(session, request).map_err(|err| err.to_string())
}

pub fn forward_control_stream(
    target: &RemoteTarget,
    session: &str,
    request: &ControlRequest,
    config: &RemoteConfig,
) -> Result<(), String> {
    if !crate::session::discovery::valid_attach_target(session) {
        return Err("invalid session name".to_string());
    }
    json_transport::stream_forward(
        target,
        config,
        &["--remote-control", session],
        request,
        |first_line, lines| {
            let response: ControlResponse = serde_json::from_str(first_line)
                .map_err(|e| format!("could not parse initial remote response: {e}"))?;
            if !response.ok {
                if let Some(err) = response.error {
                    eprintln!("{err}");
                }
                std::process::exit(1);
            }
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            for line in lines {
                let line = line.map_err(|e| format!("stream read error: {e}"))?;
                writeln!(stdout, "{line}").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            Ok(())
        },
    )
    .map_err(|error| match error {
        TransportError::Preflight(message) => message,
        TransportError::Encode(error) => format!("could not encode request: {error}"),
        TransportError::Ssh(error) => format!("remote control ssh failed: {error}"),
        TransportError::Send(error) => format!("could not send request: {error}"),
        TransportError::Failed(output) => control_forward_failure(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
            session,
            &target.display_label(),
        ),
        TransportError::Answer(_) => "no response from remote rozi".to_string(),
    })
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
            source_session: None,
            extension: None,
            caller_context: None,
            credential: None,
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

        let parsed: ControlResponse =
            json_transport::parse_answer(&encoded).expect("a refusal is still an answer");

        assert!(!parsed.ok);
        assert_eq!(
            parsed.code,
            Some(crate::control::ControlErrorCode::PaneNotFound)
        );
    }

    /// Old runners must report version skew rather than a typo in the caller's command.
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
