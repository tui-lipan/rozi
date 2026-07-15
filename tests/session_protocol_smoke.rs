//! End-to-end smoke coverage for the named-session wire protocol.
//!
//! The test launches the real `--server` entry point and communicates through the same typed
//! protocol and platform-neutral IPC abstraction used by production clients.

mod common;

use std::process::{Command, Stdio};

use hyprmux::platform::command::{ShellEnv, resolve_launch_argv};
use hyprmux::session::protocol::{
    ClientMessage, Frame, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use hyprmux::shared_layout::{SHARED_LAYOUT_VERSION, SharedLayout};
use tui_lipan::prelude::TerminalColorPalette;

use common::{
    ServerGuard, TestConnection, connect_when_ready, contains, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

const PANE_ID: u32 = 41;
const PANE_GENERATION: u64 = 1;
const OUTPUT_MARKER: &[u8] = b"hyprmux-session-smoke-output";

#[test]
fn real_server_replays_pane_backlog_and_layout_after_reattach() {
    let session = unique_session_name();
    let runtime_base = private_temp_dir();
    let endpoint = subprocess_endpoint(&runtime_base, &session);
    let child = Command::new(env!("CARGO_BIN_EXE_hyprmux"))
        .args(["--server", &session])
        .env("XDG_RUNTIME_DIR", &runtime_base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch real session server");
    let mut server = ServerGuard::new(child, runtime_base);

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
    assert_eq!(controller, Some(first_client_id));
    assert_eq!(created_from_profile, None);

    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    first.write_control(&ClientMessage::SpawnPane {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        command: Some(format!("echo {}", String::from_utf8_lossy(OUTPUT_MARKER))),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: true,
        env: Vec::new(),
        title: Some("protocol smoke".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
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
        workspaces: Vec::new(),
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
}

fn attach_message(session: &str, label: &str) -> ClientMessage {
    ClientMessage::Attach {
        session: session.to_string(),
        protocol_version: PROTOCOL_VERSION,
        label: label.to_string(),
        read_only: false,
    }
}
