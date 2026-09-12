//! End-to-end coverage for controlling a session that has no UI attached.
//!
//! Everything here goes through the production path a script uses:
//! [`rozi::session::headless::run_session_control`] over the real IPC endpoint, answered by the
//! real session server. The point being proven is the absence of a client — no
//! [`ClientMessage::Attach`](rozi::session::protocol::ClientMessage) is sent until the test
//! deliberately checks that a UI attaching later sees what the script did.

use std::time::{Duration, Instant};

use rozi::control::{ControlCommand, ControlRequest, ControlResponse};
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::headless::run_session_control;
use rozi::session::protocol::ServerMessage;
use rozi::session::server::ServerSettings;

use crate::common::{IO_TIMEOUT, attach_client, spawn_listener};

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        extension: None,
    }
}

/// Run one headless command and insist the server answered at all.
///
/// A refused *command* is still an answer and comes back here as `ok: false`; only an unreachable
/// or mismatched session panics, because that is a broken test rather than a tested outcome.
#[track_caller]
fn control(session: &str, command: ControlCommand) -> ControlResponse {
    run_session_control(session, request(command)).expect("session answered the headless request")
}

#[track_caller]
fn expect_ok(session: &str, command: ControlCommand) -> serde_json::Value {
    let response = control(session, command);
    assert!(response.ok, "command failed: {:?}", response.error);
    response.data.unwrap_or(serde_json::Value::Null)
}

/// A session server that resolves launches the way a real one started from config does.
fn headless_settings() -> ServerSettings {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    ServerSettings {
        shell,
        command_shell,
        ..ServerSettings::default()
    }
}

/// Poll a pane's visible text until `predicate` holds, so the test waits on the PTY rather than
/// on a sleep long enough to be flaky on a loaded runner.
#[track_caller]
fn capture_until(session: &str, pane: u32, predicate: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let data = expect_ok(
            session,
            ControlCommand::CapturePane {
                target: Some(pane),
                scrollback: None,
            },
        );
        let text = data["text"].as_str().unwrap_or_default().to_string();
        if predicate(&text) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "pane {pane} never showed the expected text; last capture was:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn a_detached_session_can_be_grown_typed_into_and_read_without_any_client() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    // Nothing has attached, so there is nothing to list yet.
    let empty = expect_ok(&session, ControlCommand::ListPanes);
    assert_eq!(empty.as_array().map(Vec::len), Some(0));

    let spawned = expect_ok(
        &session,
        ControlCommand::NewPane {
            command: None,
            argv: None,
            cwd: None,
            title: Some("headless".to_string()),
            keep_open: false,
            focus: false,
            workspace: Some(2),
        },
    );
    assert_eq!(spawned["accepted"], serde_json::json!(true));
    assert_eq!(spawned["pty_ready"], serde_json::json!(true));
    let pane = spawned["id"].as_u64().expect("spawn reported a pane id") as u32;

    let listed = expect_ok(&session, ControlCommand::ListPanes);
    let row = listed
        .as_array()
        .and_then(|rows| rows.first())
        .expect("the spawned pane is listed");
    assert_eq!(row["id"], serde_json::json!(pane));
    assert_eq!(row["session"], serde_json::json!(session));
    // The workspace comes from the layout the server committed for the spawn, one-based like the
    // tabs a client draws.
    assert_eq!(row["workspace"], serde_json::json!(2));
    assert_eq!(row["status"], serde_json::json!("ready"));

    // Typing into a session nobody is watching, and reading back what the program drew.
    expect_ok(
        &session,
        ControlCommand::SendText {
            target: Some(pane),
            text: "printf 'headless-marker\\n'\n".to_string(),
        },
    );
    let text = capture_until(&session, pane, |text| text.contains("headless-marker"));
    assert!(text.contains("headless-marker"), "{text}");

    // Scrollback export reads the same screen through the retained history path.
    let full = expect_ok(
        &session,
        ControlCommand::CapturePane {
            target: Some(pane),
            scrollback: Some(rozi::control::CaptureScrollback::Named(
                rozi::control::CaptureScrollbackNamed::Full,
            )),
        },
    );
    assert!(
        full["text"]
            .as_str()
            .is_some_and(|text| text.contains("headless-marker")),
        "scrollback export lost the pane's output"
    );

    // A status a script reports headlessly is the same server-owned status a client would see.
    expect_ok(
        &session,
        ControlCommand::SetStatus {
            target: Some(pane),
            status: Some("working".to_string()),
            reason: Some("headless".to_string()),
        },
    );

    // Only now does a UI appear: it must find the pane the script created, placed where the
    // script put it, with the status the script set.
    let (_client, attached) = attach_client(server.endpoint(), &session, "late client");
    let ServerMessage::Attached {
        panes,
        layout,
        layout_rev,
        ..
    } = attached
    else {
        panic!("expected an attach response");
    };
    let meta = panes
        .iter()
        .find(|meta| meta.pane_id == pane)
        .expect("the attaching client is told about the headless pane");
    assert_eq!(
        meta.runtime
            .status
            .as_ref()
            .map(|status| status.value.as_str()),
        Some("working")
    );
    assert!(
        layout_rev > 0,
        "a headless spawn must commit a layout revision"
    );
    let layout = layout.expect("the server committed a layout for the headless pane");
    let workspace = layout
        .workspaces
        .iter()
        .find(|workspace| workspace.index == 1)
        .expect("the pane was placed in workspace 2 (index 1)");
    assert!(
        workspace.panes.iter().any(|entry| entry.pane_id == pane
            && entry.generation == meta.generation
            && !entry.floating),
        "the committed layout does not carry the headless pane"
    );
    layout
        .validate()
        .expect("a server-committed layout must satisfy the same rules a client's does");
}

#[test]
fn commands_that_need_a_screen_say_so_instead_of_failing_obscurely() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    for command in [
        ControlCommand::Focus { target: 1 },
        ControlCommand::SwitchWorkspace { index: 2 },
        ControlCommand::RunAction {
            action: "toggle-float".to_string(),
        },
        ControlCommand::Notify {
            message: "hi".to_string(),
            title: None,
            level: rozi::control::NotifyLevel::Info,
        },
        ControlCommand::Subscribe { events: Vec::new() },
    ] {
        let response = control(&session, command.clone());
        assert!(!response.ok, "{command:?} must be refused by a server");
        let error = response.error.unwrap_or_default();
        assert!(
            error.contains("session server") || error.contains("client-local"),
            "{command:?} was refused without saying a UI is what it needs: {error}"
        );
    }
}

#[test]
fn a_command_with_no_target_names_the_panes_it_could_have_meant() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    // No panes at all: the caller is told the session is empty, not that a pane is missing.
    let empty = control(
        &session,
        ControlCommand::SendText {
            target: None,
            text: "x".to_string(),
        },
    );
    assert!(!empty.ok);
    assert!(
        empty.error.unwrap_or_default().contains("has no panes"),
        "an empty session must say it is empty"
    );

    // One pane is unambiguous, so no target is needed.
    let first = expect_ok(
        &session,
        ControlCommand::NewPane {
            command: None,
            argv: None,
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
        },
    )["id"]
        .as_u64()
        .expect("first spawn reported a pane id") as u32;
    expect_ok(
        &session,
        ControlCommand::CapturePane {
            target: None,
            scrollback: None,
        },
    );

    // A second pane makes it ambiguous, and there is no focus to break the tie.
    let second = expect_ok(
        &session,
        ControlCommand::NewPane {
            command: None,
            argv: None,
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
        },
    )["id"]
        .as_u64()
        .expect("second spawn reported a pane id") as u32;
    assert_ne!(first, second, "a headless spawn must mint a fresh pane id");

    let ambiguous = control(
        &session,
        ControlCommand::CapturePane {
            target: None,
            scrollback: None,
        },
    );
    assert!(!ambiguous.ok);
    let error = ambiguous.error.unwrap_or_default();
    assert!(error.contains("--target"), "{error}");
    assert!(
        error.contains(&first.to_string()) && error.contains(&second.to_string()),
        "the error must list the ids to choose from: {error}"
    );
}
