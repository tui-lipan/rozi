//! End-to-end coverage for controlling a session that has no UI attached.
//!
//! Everything here goes through the production path a script uses:
//! [`rozi::session::headless::run_session_control`] over the real IPC endpoint, answered by the
//! real session server. The point being proven is the absence of a client — no
//! [`ClientMessage::Attach`](rozi::session::protocol::ClientMessage) is sent until the test
//! deliberately checks that a UI attaching later sees what the script did.

use std::time::{Duration, Instant};

use rozi::config::ExtensionProvenance;
use rozi::control::{
    CaptureRender, ControlCommand, ControlErrorCode, ControlRequest, ControlResponse, PaneWait,
};
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::headless::run_session_control;
use rozi::session::protocol::ServerMessage;
use rozi::session::server::ServerSettings;

use crate::common::{attach_client, io_timeout, spawn_listener};

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
    let deadline = Instant::now() + io_timeout();
    loop {
        let data = expect_ok(
            session,
            ControlCommand::CapturePane {
                target: Some(pane),
                scrollback: None,
                render: CaptureRender::Text,
                scale: None,
                wait: None,
                image_pixels: false,
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
            wait: None,
            capture: None,
            scale: None,
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
            render: CaptureRender::Text,
            scale: None,
            wait: None,
            image_pixels: false,
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

/// Styled captures come from the server's own screen, so a script sees colors and gets an image
/// without any UI attached.
#[test]
fn a_detached_session_captures_its_screen_as_ansi_png_and_spans() {
    use base64::Engine as _;

    // The program is launched directly rather than typed into the default shell, which differs by
    // platform in quoting, and in which key submits a line.
    #[cfg(windows)]
    let argv = [
        "powershell",
        "-NoProfile",
        "-Command",
        "Write-Host \"$([char]27)[31mstyled-marker$([char]27)[0m\"",
    ];
    #[cfg(not(windows))]
    let argv = ["printf", "\u{1b}[31mstyled-marker\u{1b}[0m\\n"];

    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();
    let spawned = expect_ok(
        &session,
        ControlCommand::NewPane {
            command: None,
            argv: Some(argv.map(str::to_string).to_vec()),
            cwd: None,
            title: None,
            // The screen must outlive the program that drew it.
            keep_open: true,
            focus: false,
            workspace: None,
        },
    );
    let pane = spawned["id"].as_u64().expect("spawn reported a pane id") as u32;
    capture_until(&session, pane, |text| {
        text.lines().any(|line| line.trim() == "styled-marker")
    });

    let capture = |render| {
        expect_ok(
            &session,
            ControlCommand::CapturePane {
                target: Some(pane),
                scrollback: None,
                render,
                scale: None,
                wait: None,
                image_pixels: false,
            },
        )
    };
    let ansi = capture(CaptureRender::Ansi);
    assert_eq!(ansi["render"], serde_json::json!("ansi"));
    let text = ansi["text"].as_str().expect("ansi capture carries text");
    assert!(text.contains("\u{1b}[31mstyled-marker"), "{text:?}");

    let png = capture(CaptureRender::Png);
    assert_eq!(png["render"], serde_json::json!("png"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            png["png_base64"]
                .as_str()
                .expect("png capture carries data"),
        )
        .expect("png capture is base64");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

    let spans = capture(CaptureRender::Spans);
    assert_eq!(spans["render"], serde_json::json!("spans"));
    let frame = &spans["frame"];
    assert_eq!(frame["format"], serde_json::json!("rozi-spans"));
    assert_eq!(frame["version"], serde_json::json!(1));
    let red = frame["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .flat_map(|row| row.as_array().expect("a row of runs"))
        .find(|run| run["text"] == "styled-marker")
        .expect("the marker is one run of its own");
    assert_eq!(red["fg"], serde_json::json!("red"));
    assert!(frame["cursor"]["visible"].is_boolean());

    let refused = control(
        &session,
        ControlCommand::CapturePane {
            target: Some(pane),
            scrollback: Some(rozi::control::CaptureScrollback::Named(
                rozi::control::CaptureScrollbackNamed::Full,
            )),
            render: CaptureRender::Png,
            scale: None,
            wait: None,
            image_pixels: false,
        },
    );
    assert!(!refused.ok);
    assert_eq!(
        refused.code,
        Some(rozi::control::ControlErrorCode::InvalidArgument)
    );
}

/// `layout get` answers from the document the server owns, so a script can see how a session it
/// grew is arranged before anyone attaches to draw it.
#[test]
fn a_detached_session_reports_its_arrangement_without_any_client() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    let before = expect_ok(&session, ControlCommand::LayoutGet { workspace: None });
    assert_eq!(before["revision"], serde_json::Value::Null);
    assert_eq!(before["workspaces"], serde_json::json!([]));

    let mut spawned = Vec::new();
    for title in ["left", "right"] {
        let data = expect_ok(
            &session,
            ControlCommand::NewPane {
                command: None,
                argv: None,
                cwd: None,
                title: Some(title.to_string()),
                keep_open: false,
                focus: false,
                workspace: Some(3),
            },
        );
        spawned.push(data["id"].as_u64().expect("spawn reported a pane id"));
    }

    let report = expect_ok(&session, ControlCommand::LayoutGet { workspace: Some(3) });
    assert!(report["revision"].as_u64().is_some_and(|rev| rev > 0));
    assert!(report.get("client").is_none(), "no UI answered");
    let cols = report["canvas"]["cols"].as_u64().expect("canvas cols");
    let rows = report["canvas"]["rows"].as_u64().expect("canvas rows");
    let workspaces = report["workspaces"].as_array().expect("workspaces");
    assert_eq!(workspaces.len(), 1, "--workspace narrows the report");
    assert_eq!(workspaces[0]["index"], serde_json::json!(3));
    let panes = workspaces[0]["panes"].as_array().expect("panes");
    let ids: Vec<u64> = panes
        .iter()
        .filter_map(|pane| pane["id"].as_u64())
        .collect();
    assert_eq!(ids, spawned, "tiled panes come back in tiling order");
    // Two tiled panes share the canvas between them without overlapping or leaving a gap.
    assert!(panes.iter().all(|pane| pane["floating"] == false));
    let spans: Vec<(u64, u64)> = panes
        .iter()
        .map(|pane| {
            let rect = &pane["rect"];
            assert_eq!(rect["height"].as_u64(), Some(rows));
            (
                rect["x"].as_u64().expect("x"),
                rect["width"].as_u64().expect("width"),
            )
        })
        .collect();
    assert_eq!(spans[0].0, 0);
    assert_eq!(spans[0].0 + spans[0].1, spans[1].0);
    assert_eq!(spans[1].0 + spans[1].1, cols);
}

/// A script can rearrange a session nobody is attached to, and what it wrote is what the next
/// `layout get` - and the next client to attach - sees.
#[test]
fn a_detached_session_can_be_rearranged_without_any_client() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();
    let mut panes = Vec::new();
    for _ in 0..2 {
        let data = expect_ok(
            &session,
            ControlCommand::NewPane {
                command: None,
                argv: None,
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: Some(1),
            },
        );
        panes.push(data["id"].as_u64().expect("spawn reported a pane id") as u32);
    }
    let revision =
        expect_ok(&session, ControlCommand::LayoutGet { workspace: Some(1) })["revision"]
            .as_u64()
            .expect("a revision");

    let floated = expect_ok(
        &session,
        ControlCommand::PaneSet {
            target: panes[1],
            floating: Some(true),
            fullscreen: None,
            rect: Some(rozi::control::CellRect {
                x: 4,
                y: 2,
                width: 30,
                height: 10,
            }),
            rect_fraction: None,
            split_ratio: None,
            width_ratio: None,
            if_revision: Some(revision),
        },
    );
    assert_eq!(floated["changed"], serde_json::json!(true));
    assert_eq!(floated["revision"].as_u64(), Some(revision + 1));

    // A script holding the old revision is told the layout moved on, and changes nothing.
    let stale = control(
        &session,
        ControlCommand::LayoutSet {
            workspace: 1,
            layout: Some(rozi::control::ControlLayoutKind::Grid),
            master_ratio: None,
            if_revision: Some(revision),
        },
    );
    assert!(!stale.ok);
    assert_eq!(stale.code, Some(rozi::control::ControlErrorCode::Conflict));

    let report = expect_ok(&session, ControlCommand::LayoutGet { workspace: Some(1) });
    assert_eq!(report["revision"].as_u64(), Some(revision + 1));
    let workspace = &report["workspaces"][0];
    assert_eq!(workspace["layout"], serde_json::json!("dwindle"));
    let float = workspace["panes"]
        .as_array()
        .and_then(|panes| panes.iter().find(|pane| pane["floating"] == true))
        .expect("the floated pane");
    assert_eq!(float["id"].as_u64(), Some(u64::from(panes[1])));
    assert_eq!(
        float["rect"],
        serde_json::json!({"x": 4, "y": 2, "width": 30, "height": 10})
    );

    let (_client, attached) = attach_client(server.endpoint(), &session, "late client");
    let ServerMessage::Attached { layout, .. } = attached else {
        panic!("expected an attach response");
    };
    let layout = layout.expect("a layout");
    assert!(
        layout.workspaces[0]
            .panes
            .iter()
            .any(|pane| pane.pane_id == panes[1] && pane.floating),
        "a client attaching later finds the pane floating"
    );
}

/// `pane close` ends the pane's process and removes it from the layout in one step, with nobody
/// attached to do the layout half.
#[test]
fn a_detached_session_can_close_a_pane_without_any_client() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();
    let mut panes = Vec::new();
    for _ in 0..2 {
        let data = expect_ok(
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
        );
        panes.push(data["id"].as_u64().expect("spawn reported a pane id") as u32);
    }

    let closed = expect_ok(
        &session,
        ControlCommand::PaneClose {
            target: panes[0],
            if_revision: None,
        },
    );
    assert_eq!(closed["id"].as_u64(), Some(u64::from(panes[0])));

    let listed = expect_ok(&session, ControlCommand::ListPanes);
    let ids: Vec<u64> = listed
        .as_array()
        .expect("panes")
        .iter()
        .filter_map(|pane| pane["id"].as_u64())
        .collect();
    assert_eq!(
        ids,
        vec![u64::from(panes[1])],
        "the closed pane is gone, not exited"
    );

    let (_client, attached) = attach_client(server.endpoint(), &session, "late client");
    let ServerMessage::Attached {
        layout,
        panes: metas,
        ..
    } = attached
    else {
        panic!("expected an attach response");
    };
    assert!(metas.iter().all(|meta| meta.pane_id != panes[0]));
    let layout = layout.expect("a layout");
    assert!(
        layout
            .workspaces
            .iter()
            .all(|workspace| workspace.panes.iter().all(|pane| pane.pane_id != panes[0]))
    );
    layout.validate().expect("valid");
}

/// The whole feature is for sessions nobody is driving. When somebody *is* driving one, opening a
/// pane means committing a layout revision over their arrangement, and that is the controller's
/// call - the same rule the protocol already applies to a non-controller's `SpawnPane`.
#[test]
fn a_client_holding_layout_control_keeps_a_script_from_reshaping_the_session() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    let (controller, attached) = attach_client(server.endpoint(), &session, "controller");
    let ServerMessage::Attached {
        client_id,
        controller: holder,
        ..
    } = attached
    else {
        panic!("expected an attach response");
    };
    assert_eq!(
        holder,
        Some(client_id),
        "the first attacher takes the lease; this test needs it to"
    );

    let refused = control(
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
    );
    assert!(!refused.ok, "a controller is driving this session");
    assert_eq!(
        refused.code,
        Some(rozi::control::ControlErrorCode::NotController)
    );
    let error = refused.error.unwrap_or_default();
    assert!(error.contains("layout control"), "{error}");
    assert_eq!(
        expect_ok(&session, ControlCommand::ListPanes)
            .as_array()
            .map(Vec::len),
        Some(0),
        "the refused spawn left no pane behind"
    );

    // Reading and typing never needed the lease; a follower client may already do both.
    expect_ok(&session, ControlCommand::ListPanes);

    // Once the client lets go, the same request is simply allowed.
    drop(controller);
    let deadline = Instant::now() + io_timeout();
    let allowed = loop {
        let response = control(
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
        );
        if response.ok || Instant::now() >= deadline {
            break response;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        allowed.ok,
        "the lease released with the client: {:?}",
        allowed.error
    );
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

/// The CLI attaches extension provenance automatically from the environment, so this is the shape
/// a real extension's request arrives in. A session server cannot check the fencing token, so it
/// declines to act on the extension's behalf at all rather than becoming the way around it.
#[test]
fn an_extension_cannot_use_a_session_endpoint_to_escape_its_own_generation_fence() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

    let response = run_session_control(
        &session,
        ControlRequest {
            command: ControlCommand::ListPanes,
            source_pane: None,
            extension: Some(ExtensionProvenance {
                id: "git-tools".to_string(),
                generation: "a-token-only-a-client-could-mint".to_string(),
            }),
        },
    )
    .expect("the session answered");
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(error.contains("git-tools"), "{error}");

    // The same command without provenance is ordinary and works.
    expect_ok(&session, ControlCommand::ListPanes);
}

/// A script running inside pane 3 of one session, addressing another with `--session`, must not
/// have its inherited `ROZI_PANE` treated as a target. The id says nothing about which session it
/// belongs to, and the session being addressed may well have a pane 3 of its own.
#[test]
fn an_inherited_pane_id_does_not_leak_across_the_session_boundary() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();

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
        ControlCommand::NewPane {
            command: None,
            argv: None,
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
        },
    );

    // The caller is sitting in a pane whose id this session also happens to use.
    let response = run_session_control(
        &session,
        ControlRequest {
            command: ControlCommand::SendText {
                target: None,
                text: "this must not be typed anywhere\n".to_string(),
                wait: None,
                capture: None,
                scale: None,
            },
            source_pane: Some(first),
            extension: None,
        },
    )
    .expect("the session answered");
    assert!(
        !response.ok,
        "an inherited pane id must not silently become the target"
    );
    let error = response.error.unwrap_or_default();
    assert!(error.contains("--target"), "{error}");

    // Nothing was typed: the pane's screen is still whatever its shell drew.
    let text = expect_ok(
        &session,
        ControlCommand::CapturePane {
            target: Some(first),
            scrollback: Some(rozi::control::CaptureScrollback::Named(
                rozi::control::CaptureScrollbackNamed::Full,
            )),
            render: CaptureRender::Text,
            scale: None,
            wait: None,
            image_pixels: false,
        },
    )["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !text.contains("this must not be typed anywhere"),
        "the refused command still reached a pane:\n{text}"
    );
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
            wait: None,
            capture: None,
            scale: None,
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
            render: CaptureRender::Text,
            scale: None,
            wait: None,
            image_pixels: false,
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
            render: CaptureRender::Text,
            scale: None,
            wait: None,
            image_pixels: false,
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

fn pane_wait(text: Option<&str>, settle_ms: Option<u64>, timeout_ms: u64) -> Option<PaneWait> {
    Some(PaneWait {
        text: text.map(str::to_string),
        settle_ms,
        timeout_ms,
    })
}

fn send_keys_waiting(pane: u32, keys: &[&str], wait: Option<PaneWait>) -> ControlCommand {
    ControlCommand::SendKeys {
        target: Some(pane),
        keys: keys.iter().map(|key| key.to_string()).collect(),
        literal: false,
        wait,
        capture: Some(CaptureRender::Text),
        scale: None,
    }
}

fn captured_text(response: &ControlResponse) -> String {
    response
        .data
        .as_ref()
        .and_then(|data| data["text"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The pattern the waits replace is send, sleep, capture - which reads the screen before a slow
/// program has answered. A send that waits answers with the program's output instead, in one
/// request, and only with output that came after its input.
#[test]
fn a_send_that_waits_answers_with_the_output_a_naive_capture_misses() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();
    let spawned = expect_ok(
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
    );
    let pane = spawned["id"].as_u64().expect("spawn reported a pane id") as u32;
    let timeout_ms = u64::try_from(io_timeout().as_millis()).unwrap();

    // The markers only exist once the shell has evaluated them: the echoed command line reads
    // `$((40+2))`, never `42`.
    expect_ok(
        &session,
        ControlCommand::SendText {
            target: Some(pane),
            text: "sleep 1; echo naive-$((40+2))\n".to_string(),
            wait: None,
            capture: None,
            scale: None,
        },
    );
    let naive = capture_until(&session, pane, |text| text.contains("naive-$((40+2))"));
    assert!(
        !naive.contains("naive-42"),
        "a capture right after sending cannot have the delayed output yet:\n{naive}"
    );

    let waited = control(
        &session,
        send_keys_waiting(
            pane,
            &["sleep 0.5; echo waited-$((40+2))", "Enter"],
            pane_wait(Some("waited-42"), None, timeout_ms),
        ),
    );
    assert!(waited.ok, "{waited:?}");
    assert!(captured_text(&waited).contains("waited-42"), "{waited:?}");

    // What was already on screen does not satisfy a send's wait, so this one runs out its clock
    // and says so, carrying the screen it gave up on.
    let stale = control(
        &session,
        send_keys_waiting(
            pane,
            &["true", "Enter"],
            pane_wait(Some("waited-42"), None, 700),
        ),
    );
    assert_eq!(stale.code, Some(ControlErrorCode::Timeout), "{stale:?}");
    assert!(captured_text(&stale).contains("waited-42"), "{stale:?}");

    // A capture's wait takes the screen as it is, so the same text answers it at once.
    let started = Instant::now();
    let present = control(
        &session,
        ControlCommand::CapturePane {
            target: Some(pane),
            scrollback: None,
            render: CaptureRender::Text,
            scale: None,
            image_pixels: false,
            wait: pane_wait(Some("waited-42"), None, timeout_ms),
        },
    );
    assert!(present.ok, "{present:?}");
    assert!(started.elapsed() < Duration::from_secs(1));

    let settled = control(
        &session,
        ControlCommand::CapturePane {
            target: Some(pane),
            scrollback: None,
            render: CaptureRender::Text,
            scale: None,
            image_pixels: false,
            wait: pane_wait(None, Some(300), timeout_ms),
        },
    );
    assert!(settled.ok, "{settled:?}");

    // A program that exits ends the wait rather than leaving it to its deadline.
    let started = Instant::now();
    let exited = control(
        &session,
        send_keys_waiting(
            pane,
            &["exit", "Enter"],
            pane_wait(Some("never-printed"), None, timeout_ms),
        ),
    );
    assert_eq!(
        exited.code,
        Some(ControlErrorCode::PaneNotRunning),
        "{exited:?}"
    );
    assert!(started.elapsed() < io_timeout());
}
