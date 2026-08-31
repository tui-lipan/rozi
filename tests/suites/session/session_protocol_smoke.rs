//! End-to-end smoke coverage for the named-session wire protocol.
//!
//! The test launches the real `--server` entry point and communicates through the same typed
//! protocol and platform-neutral IPC abstraction used by production clients.

use std::process::{Command, Stdio};

use rozi::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis,
    SharedWorkspace,
};
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, Frame, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{
    ServerGuard, TestConnection, connect_when_ready, contains, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

const PANE_ID: u32 = 41;
const PANE_GENERATION: u64 = 1;
const OUTPUT_MARKER: &[u8] = b"rozi-session-smoke-output";

#[test]
fn real_server_replays_pane_backlog_and_layout_after_reattach() {
    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let config_path = test_root.join("config.toml");
    std::fs::write(&config_path, "[session]\nresurrect = false\n")
        .expect("write isolated server config");
    let endpoint = subprocess_endpoint(&runtime_base, &session);
    let child = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(["--server", &session])
        // A subprocess does not inherit the integration test's in-process path override. Redirect
        // every persisted-data root explicitly, and disable resurrection because this test does
        // not exercise it. Otherwise an interrupted run can publish this protocol-test session
        // into the developer's real resurrection directory.
        .env("HOME", &test_root)
        .env("XDG_CONFIG_HOME", test_root.join("config"))
        .env("XDG_STATE_HOME", test_root.join("state"))
        .env("XDG_CACHE_HOME", test_root.join("cache"))
        .env("XDG_DATA_HOME", test_root.join("data"))
        .env("XDG_RUNTIME_DIR", &runtime_base)
        .env("APPDATA", test_root.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", test_root.join("AppData").join("Local"))
        .env("ROZI_CONFIG", config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch real session server");
    let mut server = ServerGuard::new(child, test_root.clone());

    let mut first = connect_when_ready(&endpoint, server.child_mut());
    first.write_control(&attach_message(&session, "first"));
    let mut attached = None;
    read_until(&mut first, |frame| {
        if let Frame::Control(message @ ServerMessage::Attached { .. }) = frame {
            attached = Some(message.clone());
            true
        } else {
            false
        }
    });
    let ServerMessage::Attached {
        protocol_version,
        effective_protocol,
        session: attached_session,
        client_id: first_client_id,
        controller,
        created_from_profile,
        ..
    } = attached.expect("attached control frame")
    else {
        unreachable!()
    };
    assert_eq!(attached_session, session);
    assert_eq!(protocol_version, PROTOCOL_VERSION);
    assert_eq!(effective_protocol, PROTOCOL_VERSION);
    assert_eq!(controller, Some(first_client_id));
    assert_eq!(created_from_profile, None);

    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    first.write_control(&ClientMessage::SpawnPane {
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: Some(rozi::pane::launch::PaneLaunch::shell(format!(
            "echo {}",
            String::from_utf8_lossy(OUTPUT_MARKER)
        ))),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: true,
        env: Vec::new(),
        title: Some("protocol smoke".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });

    let mut saw_spawn = false;
    let mut live_output = Vec::new();
    read_until(&mut first, |frame| {
        match frame {
            Frame::Control(ServerMessage::SpawnResult {
                pane_id,
                generation,
                ok,
                error,
                ..
            }) if *pane_id == PANE_ID && *generation == PANE_GENERATION => {
                assert!(*ok, "spawn failed: {error:?}");
                saw_spawn = true;
            }
            Frame::PaneBytes {
                pane_id,
                generation,
                bytes,
                ..
            } if *pane_id == PANE_ID && *generation == PANE_GENERATION => {
                live_output.extend_from_slice(bytes);
            }
            _ => {}
        }
        saw_spawn && contains(&live_output, OUTPUT_MARKER)
    });
    first.write_control(&ClientMessage::SetSessionOrigin {
        profile: "work".to_string(),
    });

    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 23,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: None,
            synchronized: false,
            layout: SharedLayoutKind::Dwindle,
            start_axis: SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: None,
            panes: vec![SharedPane {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                title: Some("protocol smoke".to_string()),
                profile_name: None,
                cwd: None,
                launch: Some(rozi::pane::launch::PaneLaunch::shell(format!(
                    "echo {}",
                    String::from_utf8_lossy(OUTPUT_MARKER)
                ))),
                replay: false,
                keep_open: true,
                floating: false,
                fullscreen: false,
                rect: None,
                scrollable_width: rozi::state::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    };
    first.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: layout.clone(),
    });
    let mut committed = None;
    read_until(&mut first, |frame| {
        if let Frame::Control(message @ ServerMessage::LayoutCommitted { .. }) = frame {
            committed = Some(message.clone());
            true
        } else {
            false
        }
    });
    assert_eq!(
        committed,
        Some(ServerMessage::LayoutCommitted {
            rev: 1,
            author: first_client_id,
            layout: layout.clone(),
        })
    );

    first.write_control(&ClientMessage::Detach);
    drop(first);

    let mut second = TestConnection::connect(&endpoint);
    second.write_control(&attach_message(&session, "second"));
    let mut reattached = None;
    read_until(&mut second, |frame| {
        if let Frame::Control(message @ ServerMessage::Attached { .. }) = frame {
            reattached = Some(message.clone());
            true
        } else {
            false
        }
    });
    let ServerMessage::Attached {
        panes,
        layout_rev,
        layout: reattached_layout,
        created_from_profile,
        ..
    } = reattached.expect("reattached control frame")
    else {
        unreachable!()
    };
    assert_eq!(layout_rev, 1);
    assert_eq!(reattached_layout, Some(layout));
    assert_eq!(created_from_profile.as_deref(), Some("work"));
    assert!(
        panes
            .iter()
            .any(|pane| pane.pane_id == PANE_ID && pane.generation == PANE_GENERATION),
        "reattach omitted the live pane: {panes:?}"
    );

    let mut replay = Vec::new();
    read_until(&mut second, |frame| {
        if let Frame::PaneBytes {
            pane_id,
            generation,
            bytes,
            ..
        } = frame
            && *pane_id == PANE_ID
            && *generation == PANE_GENERATION
        {
            replay.extend_from_slice(bytes);
        }
        contains(&replay, OUTPUT_MARKER)
    });

    second.write_control(&ClientMessage::Shutdown);
    drop(second);
    server.wait_for_exit();

    let state_base = if cfg!(windows) {
        test_root.join("AppData").join("Local").join("rozi/state")
    } else {
        test_root.join("state/rozi")
    };
    assert!(
        !state_base
            .join("sessions")
            .join(&session)
            .join("meta.json")
            .is_file(),
        "the protocol test must not publish a resurrection snapshot"
    );
}

fn attach_message(session: &str, label: &str) -> ClientMessage {
    ClientMessage::Attach {
        session: session.to_string(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: label.to_string(),
        read_only: false,
        shares_filesystem: true,
    }
}
