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
use rozi::session::discovery::InstanceLookup;
use rozi::session::headless::run_session_control;
use rozi::session::protocol::ServerMessage;
use rozi::session::server::ServerSettings;

use crate::common::{attach_client, io_timeout, spawn_listener};

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
        credential: None,
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
            size: None,
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
            size: None,
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
                size: None,
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
                size: None,
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
                size: None,
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
            size: None,
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
                size: None,
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

/// Requests a session server refuses on purpose reach a script as a refusal that says why, over the
/// real endpoint. Which requests are refused, and the full reasons, are pinned by the server's own
/// unit tests (`every_ui_only_command_is_refused_with_a_reason_rather_than_silently_accepted`,
/// `a_request_carrying_extension_provenance_is_refused_rather_than_trusted`,
/// `an_inherited_pane_id_never_addresses_a_pane_in_the_session_being_targeted`); this proves the
/// refusal survives the trip and that nothing behind it ran.
#[test]
fn refusals_reach_a_script_with_their_reason_and_nothing_runs() {
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
            size: None,
        },
    )["id"]
        .as_u64()
        .expect("spawn reported a pane id") as u32;

    // A second pane, so a targetless send is ambiguous and only the inherited id could resolve it.
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
            size: None,
        },
    );

    let untyped = "this must not be typed anywhere";
    let ui_only =
        |command: ControlCommand| (request(command), &["session server", "client-local"][..]);
    let cases: Vec<(ControlRequest, &[&str])> = vec![
        ui_only(ControlCommand::Focus { target: 1 }),
        ui_only(ControlCommand::SwitchWorkspace { index: 2 }),
        ui_only(ControlCommand::RunAction {
            action: "toggle-float".to_string(),
        }),
        ui_only(ControlCommand::Notify {
            message: "hi".to_string(),
            title: None,
            level: rozi::control::NotifyLevel::Info,
        }),
        ui_only(ControlCommand::Subscribe { events: Vec::new() }),
        // The CLI attaches extension provenance from the environment. A session server cannot check
        // the fencing token, so it declines to act on the extension's behalf at all.
        (
            ControlRequest {
                extension: Some(ExtensionProvenance {
                    id: "git-tools".to_string(),
                    generation: "a-token-only-a-client-could-mint".to_string(),
                }),
                ..request(ControlCommand::ListPanes)
            },
            &["git-tools"][..],
        ),
        // A script inside pane `first` of another session addressing this one: its inherited
        // `ROZI_PANE` says nothing about this session, which happens to have a pane with that id.
        (
            ControlRequest {
                source_pane: Some(first),
                ..request(ControlCommand::SendText {
                    target: None,
                    text: format!("{untyped}\n"),
                    wait: None,
                    capture: None,
                    scale: None,
                })
            },
            &["--target"][..],
        ),
    ];
    for (request, reasons) in cases {
        let command = request.command.clone();
        let response = run_session_control(&session, request).expect("the session answered");
        assert!(!response.ok, "{command:?} must be refused");
        let error = response.error.unwrap_or_default();
        assert!(
            reasons.iter().any(|reason| error.contains(reason)),
            "{command:?} was refused without saying why ({reasons:?}): {error}"
        );
    }

    // The same command without provenance is ordinary and works.
    expect_ok(&session, ControlCommand::ListPanes);
    // And the refused send typed nothing.
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
        !text.contains(untyped),
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
            size: None,
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
            size: None,
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
/// request, and only with output that came after its input. The wait semantics themselves are
/// unit-tested in `capture_waits`; this proves they hold over the real endpoint and PTY.
#[test]
fn a_send_that_waits_answers_with_output_that_came_after_its_input() {
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
            size: None,
        },
    );
    let pane = spawned["id"].as_u64().expect("spawn reported a pane id") as u32;
    let timeout_ms = u64::try_from(io_timeout().as_millis()).unwrap();

    // The marker only exists once the shell has evaluated it: the echoed command line reads
    // `$((40+2))`, never `42`, so only output produced after the delay can satisfy the wait.
    let waited = control(
        &session,
        send_keys_waiting(
            pane,
            &["sleep 0.15; echo waited-$((40+2))", "Enter"],
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
            pane_wait(Some("waited-42"), None, 150),
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
            wait: pane_wait(None, Some(100), timeout_ms),
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

/// Open a pane in `session` and read back the pane id and server instance it was told, exactly as
/// a hook running inside it would see them.
///
/// Unix-only: the pane prints its environment with a POSIX shell line, which the cmd or PowerShell
/// launch shell on Windows does not run. Which instance a pane is told is covered on every platform
/// by the server's unit tests.
#[cfg(unix)]
fn pane_identity(session: &str) -> (u32, rozi::session::protocol::SessionInstanceId) {
    let pane = expect_ok(
        session,
        ControlCommand::NewPane {
            command: Some(
                "printf 'env:%s:%s:%s:\\n' \"$ROZI\" \"$ROZI_PANE\" \"$ROZI_SESSION_INSTANCE\""
                    .to_string(),
            ),
            argv: None,
            cwd: None,
            title: None,
            keep_open: true,
            focus: false,
            workspace: None,
            size: None,
        },
    )["id"]
        .as_u64()
        .expect("spawn reported a pane id") as u32;
    let text = capture_until(session, pane, |text| {
        text.lines().any(|line| line.starts_with("env:1:"))
    });
    let line = text
        .lines()
        .find(|line| line.starts_with("env:1:"))
        .expect("the pane printed its environment");
    let fields: Vec<&str> = line.split(':').collect();
    assert_eq!(fields[2], pane.to_string(), "{line}");
    let instance = rozi::session::protocol::SessionInstanceId::from_env_value(fields[3])
        .expect("a shared pane is told its server instance");
    (pane, instance)
}

/// A pane with no UI to name - remote, restored, or spawned headlessly - reports its agent to the
/// server it runs in. It knows that server only by the `ROZI_SESSION_INSTANCE` it was given, so
/// this proves the whole chain: the pane is told its instance and id, discovery finds the server
/// that answers to that instance, and the server judges the report only for its own pane.
#[cfg(unix)]
#[test]
fn a_pane_with_no_ui_finds_its_own_server_and_reports_its_agent_there() {
    let server = spawn_listener(headless_settings());
    let session = server.session().to_string();
    let (pane, instance) = pane_identity(&session);

    assert_eq!(
        rozi::session::discovery::session_with_instance(&instance).expect("discovery ran"),
        InstanceLookup::Found(session.clone())
    );

    let report = |source| ControlRequest {
        source_session: Some(source),
        ..request(ControlCommand::AgentReport {
            target: Some(pane),
            agent: "claude".into(),
            integration: "hook-e2e".into(),
            state: rozi::session::protocol::AgentState::Blocked,
            reason: Some("Permission required".into()),
            native_session: Some("native-e2e".into()),
            seq: 1,
        })
    };
    let stranger = rozi::session::protocol::SessionInstanceId::from_env_value("not-this-server")
        .expect("non-empty instance");
    let refused = run_session_control(&session, report(stranger))
        .expect_err("a report from another server's pane is refused");
    assert!(
        refused.to_string().starts_with("instance-mismatch"),
        "{refused}"
    );

    // The pane runs `printf`, not Claude, so its own server judges the report against that pane
    // and declines it: an integration may only speak for an agent detection has seen there. That
    // answer can only come from the right server looking at the right pane.
    let judged = run_session_control(&session, report(instance)).expect("server answered");
    assert!(!judged.ok);
    let reason = judged.error.unwrap_or_default();
    assert!(
        reason.contains(&format!("not the currently detected agent in pane {pane}")),
        "{reason}"
    );
}

/// The instance a server names in its discovery reply: what a pane of it is told.
fn server_instance(
    server: &crate::common::ListenerGuard,
) -> rozi::session::protocol::SessionInstanceId {
    let mut stream =
        rozi::platform::ipc::IpcConnection::connect(server.endpoint()).expect("connect");
    stream
        .set_read_timeout(Some(io_timeout()))
        .expect("read timeout");
    rozi::session::protocol::write_frame(
        &mut stream,
        &rozi::session::protocol::ClientMessage::Query {
            capabilities: None,
            session: server.session().to_string(),
            protocol_version: rozi::session::protocol::PROTOCOL_VERSION,
            min_protocol_version: rozi::session::protocol::MIN_SUPPORTED_PROTOCOL,
        },
    )
    .expect("send query");
    match rozi::session::protocol::read_frame::<_, ServerMessage>(&mut stream).expect("answer") {
        ServerMessage::SessionInfo {
            instance: Some(instance),
            ..
        } => instance,
        other => panic!("expected a session info naming its instance, got {other:?}"),
    }
}

/// Several sessions run at once. Each instance must lead to its own server, never to whichever
/// answered first - a pane id alone is the same in every one of them.
#[test]
fn each_instance_finds_its_own_server_among_several() {
    let servers: Vec<_> = (0..3)
        .map(|_| spawn_listener(headless_settings()))
        .collect();
    for server in &servers {
        assert_eq!(
            rozi::session::discovery::session_with_instance(&server_instance(server))
                .expect("discovery ran"),
            InstanceLookup::Found(server.session().to_string())
        );
    }
}

/// A server from before instances were reported answers discovery without one. It may be the
/// pane's own server, so it is named rather than silently skipped - and never picked.
#[test]
fn a_server_that_does_not_name_its_instance_is_reported_and_never_chosen() {
    let session = crate::common::unique_session_name();
    let (listener, endpoint) =
        rozi::session::server::bind_session_socket(&session).expect("bind stand-in server");
    listener.set_nonblocking(true).expect("non-blocking accept");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let answering = {
        let session = session.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok(mut stream) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                if rozi::session::protocol::read_frame::<_, rozi::session::protocol::ClientMessage>(
                    &mut stream,
                )
                .is_ok()
                {
                    let _ = rozi::session::protocol::write_frame(
                        &mut stream,
                        &ServerMessage::SessionInfo {
                            capabilities: None,
                            agents: Vec::new(),
                            session: session.clone(),
                            panes: 1,
                            clients: 0,
                            has_layout: false,
                            effective_protocol: rozi::session::protocol::PROTOCOL_VERSION,
                            origin: Default::default(),
                            instance: None,
                        },
                    );
                }
            }
        })
    };

    let wanted =
        rozi::session::protocol::SessionInstanceId::from_env_value("pane-of-an-old-server")
            .expect("non-empty instance");
    let lookup = rozi::session::discovery::session_with_instance(&wanted).expect("discovery ran");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    answering.join().expect("stand-in server thread");
    endpoint.remove_stale();

    let InstanceLookup::NotFound { unidentified } = lookup else {
        panic!("an unidentified server must never be chosen: {lookup:?}");
    };
    assert!(unidentified.contains(&session), "{unidentified:?}");
}
