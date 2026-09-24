//! Client side of headless session control: sending one [`ControlRequest`] straight to a named
//! session server and reading its answer.
//!
//! The UI control endpoint is discovered by pid and belongs to a running rozi (see
//! [`crate::control`]). This one is addressed by *session name*, which is the whole difference: a
//! script reaches `dev` because `dev` is what it means, whether or not anybody is looking at it.
//!
//! Nothing here attaches. The connection sends a single
//! [`ClientMessage::SessionControl`] frame, reads one
//! reply, and closes — the same shape as the discovery probe next door, for the same reason: a
//! session must not look occupied because a shell script asked it a question.

use std::time::Duration;

use crate::control::ControlRequest;
use crate::control::ControlResponse;
use crate::platform::ipc::IpcConnection;
use crate::session::protocol::{self, ClientMessage, ServerMessage};

/// How long a headless request waits on the session pump.
///
/// Generous next to discovery's 60 ms probe budget, and for a different reason: a probe is one of
/// many in a sweep and a slow answer just costs a row, while this is a single deliberate command
/// whose failure is the script's failure. A server busy draining a large burst of PTY output can
/// take a few pump iterations to reach the request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a wait's answer may lag the deadline the caller set.
///
/// The server resolves an expired wait on a pump iteration and sends `timeout` itself, so this is
/// only there to let that answer win the race against the socket giving up on it.
const WAIT_REPLY_SLACK: Duration = REQUEST_TIMEOUT;

/// How long `record stop` may take to answer: the writer finishes what it has queued and syncs
/// the file first, which a slow disk can stretch past an ordinary request's budget.
const RECORDING_STOP_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a headless control request did not produce an answer.
#[derive(Debug)]
pub enum SessionControlError {
    /// No live server owns this name.
    NoSuchSession(String),
    /// The endpoint was reached, but the exchange failed.
    Transport(String),
    /// The server answered with a protocol-level refusal (`protocol-mismatch`,
    /// `session-mismatch`), which is a wrong or mismatched peer rather than a rejected command.
    Refused { code: String, message: String },
}

impl std::fmt::Display for SessionControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSession(name) => write!(
                f,
                "no running session named `{name}` (run `rozi sessions list`)"
            ),
            Self::Transport(message) => write!(f, "{message}"),
            Self::Refused { code, message } => write!(f, "{code}: {message}"),
        }
    }
}

impl std::error::Error for SessionControlError {}

/// Run one control command against the named session server and return its response.
///
/// A successful return means the server answered, not that the command succeeded — a refused
/// command comes back as a [`ControlResponse`] with `ok: false`, exactly as it does from a UI
/// endpoint, so callers report both the same way.
pub fn run_session_control(
    name: &str,
    request: ControlRequest,
) -> std::result::Result<ControlResponse, SessionControlError> {
    if !crate::session::discovery::valid_attach_target(name) {
        return Err(SessionControlError::Transport(
            "invalid session name".to_string(),
        ));
    }
    let endpoint = crate::session::server::session_endpoint(name)
        .map_err(|err| SessionControlError::Transport(err.to_string()))?;
    if !endpoint.is_live() {
        return Err(SessionControlError::NoSuchSession(name.to_string()));
    }
    let mut stream = endpoint.connect().map_err(|err| {
        // An endpoint file whose server is gone answers here rather than at `is_live`, and the
        // useful thing to say is that the session is not running, not that a connect failed.
        if matches!(
            err.kind(),
            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
        ) {
            SessionControlError::NoSuchSession(name.to_string())
        } else {
            SessionControlError::Transport(format!("could not reach session `{name}`: {err}"))
        }
    })?;
    exchange(name, &mut stream, request)
}

/// How long to wait for the server's answer, which is a different question per command.
///
/// An ordinary command is served from the server's next pump iteration, so anything slower than
/// [`REQUEST_TIMEOUT`] is a wedged server rather than a busy one.
///
/// A wait is the opposite: taking a long time is the whole point of it. The server holds the
/// request open until its condition resolves or its own deadline expires, so the socket has to
/// outlast that deadline - and a wait given no deadline waits as long as the caller does. Holding
/// every wait to the short budget instead turned each one longer than five seconds into a
/// transport failure rather than the answer it was about to get.
fn reply_timeout(command: &crate::control::ControlCommand) -> Option<Duration> {
    use crate::control::ControlCommand;

    let deadline_ms = match command {
        // A foreground recording answers when it ends, which is whenever the caller stops it.
        ControlCommand::RecordStart { follow: true, .. } => None,
        // Answered once the file is finished: queued frames written and the file synced.
        ControlCommand::RecordStop { .. } => return Some(RECORDING_STOP_TIMEOUT),
        ControlCommand::AgentWait { timeout_ms, .. } => *timeout_ms,
        ControlCommand::AgentPrompt {
            wait: Some(_),
            timeout_ms,
            ..
        } => *timeout_ms,
        command => match command.pane_wait() {
            Some(wait) => Some(wait.timeout_ms),
            None => return Some(REQUEST_TIMEOUT),
        },
    };
    // `None` here, and on overflow, means "no read timeout": an unbounded wait, as asked for.
    deadline_ms.and_then(|ms| Duration::from_millis(ms).checked_add(WAIT_REPLY_SLACK))
}

fn exchange(
    name: &str,
    stream: &mut IpcConnection,
    request: ControlRequest,
) -> std::result::Result<ControlResponse, SessionControlError> {
    let _ = stream.set_read_timeout(reply_timeout(&request.command));
    let _ = stream.set_write_timeout(Some(REQUEST_TIMEOUT));
    protocol::write_frame(
        stream,
        &ClientMessage::SessionControl {
            capabilities: Some(protocol::Capabilities::current()),
            session: name.to_string(),
            protocol_version: protocol::PROTOCOL_VERSION,
            min_protocol_version: protocol::MIN_SUPPORTED_PROTOCOL,
            request,
        },
    )
    .map_err(|err| SessionControlError::Transport(format!("could not send request: {err}")))?;
    match protocol::read_frame::<_, ServerMessage>(stream)
        .map_err(|err| SessionControlError::Transport(format!("no answer from `{name}`: {err}")))?
    {
        ServerMessage::SessionControlResult { response, .. } => Ok(response),
        ServerMessage::Error { code, message } => {
            Err(SessionControlError::Refused { code, message })
        }
        other => Err(SessionControlError::Transport(format!(
            "session `{name}` answered with an unexpected {} message",
            message_kind(&other)
        ))),
    }
}

/// The wire tag of a message, for an error that has to name what arrived without printing it.
fn message_kind(message: &ServerMessage) -> String {
    serde_json::to_value(message)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wait that outlives the ordinary request budget must still be waited for. Holding it to
    /// [`REQUEST_TIMEOUT`] reported `Resource temporarily unavailable` a few seconds in, while the
    /// server was still perfectly willing to answer.
    #[test]
    fn a_wait_outlasts_its_own_deadline_and_an_ordinary_command_does_not() {
        use crate::control::{AgentTarget, AgentWaitCondition, ControlCommand};

        let target = || AgentTarget::Pane(3);
        let wait = |timeout_ms| ControlCommand::AgentWait {
            target: target(),
            until: AgentWaitCondition::Idle,
            timeout_ms,
        };
        let prompt = |wait, timeout_ms| ControlCommand::AgentPrompt {
            target: target(),
            prompt: "go".to_string(),
            wait,
            timeout_ms,
            allow_working: false,
        };

        assert_eq!(
            reply_timeout(&wait(Some(300_000))),
            Some(Duration::from_secs(300) + WAIT_REPLY_SLACK)
        );
        assert_eq!(
            reply_timeout(&prompt(Some(AgentWaitCondition::Idle), Some(300_000))),
            Some(Duration::from_secs(300) + WAIT_REPLY_SLACK)
        );
        assert_eq!(
            reply_timeout(&wait(None)),
            None,
            "a wait with no deadline waits as long as the caller does"
        );
        assert_eq!(
            reply_timeout(&prompt(None, None)),
            Some(REQUEST_TIMEOUT),
            "a prompt that does not wait is an ordinary command"
        );
        assert_eq!(
            reply_timeout(&ControlCommand::ListPanes),
            Some(REQUEST_TIMEOUT)
        );

        let pane_wait = Some(crate::control::PaneWait {
            text: Some("done".to_string()),
            settle_ms: None,
            timeout_ms: 60_000,
        });
        for command in [
            ControlCommand::CapturePane {
                target: None,
                scrollback: None,
                render: crate::control::CaptureRender::Text,
                scale: None,
                image_pixels: false,
                wait: pane_wait.clone(),
            },
            ControlCommand::SendKeys {
                target: None,
                keys: vec!["Enter".to_string()],
                literal: false,
                wait: pane_wait.clone(),
                capture: None,
                scale: None,
            },
        ] {
            assert_eq!(
                reply_timeout(&command),
                Some(Duration::from_secs(60) + WAIT_REPLY_SLACK)
            );
        }
    }

    #[test]
    fn a_hostile_session_name_is_refused_before_any_endpoint_is_touched() {
        for name in ["../escape", "dev;rm -rf /", "", "dev\npanes"] {
            let error = run_session_control(
                name,
                ControlRequest {
                    command: crate::control::ControlCommand::ListPanes,
                    source_pane: None,
                    extension: None,
                },
            )
            .expect_err("hostile name must be refused");
            assert!(
                matches!(&error, SessionControlError::Transport(message) if message == "invalid session name"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_missing_session_says_so_rather_than_reporting_a_transport_failure() {
        let error = run_session_control(
            "no-such-session-for-headless-control",
            ControlRequest {
                command: crate::control::ControlCommand::ListPanes,
                source_pane: None,
                extension: None,
            },
        )
        .expect_err("a session that does not exist cannot answer");
        assert!(
            matches!(&error, SessionControlError::NoSuchSession(name) if name
                == "no-such-session-for-headless-control"),
            "{error}"
        );
        assert!(error.to_string().contains("rozi sessions list"), "{error}");
    }
}
