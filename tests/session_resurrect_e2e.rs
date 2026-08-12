//! Session resurrection across a server restart, driven through a real subprocess and PTY.
//!
//! Unix-only, at file scope. The pane input below is a POSIX shell program: a `while` loop feeding
//! `printf`, with the replay marker deliberately assembled from a format string and an argument so
//! the literal never appears in the line the terminal echoes back. On Windows the launch shell is
//! cmd or PowerShell, which runs none of that, so the marker never arrives and the read waits out
//! its deadline. There is no portable spelling worth having - `echo` exists on every shell but
//! would put the marker into the echoed command line, and the assertion could then pass without the
//! pane ever producing it.
//!
//! The gate is on the file rather than the test function because every helper and import here
//! exists only to serve it, and `-D warnings` turns each one into an error once the test is
//! compiled out. Windows resurrection deserves its own test with a shell it can actually run.
#![cfg(unix)]

mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rozi::pane::TerminalPane;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, Frame, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use rozi::shared_layout::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use tui_lipan::prelude::TerminalColorPalette;

use common::{
    ServerGuard, attach_message, connect_when_ready, contains, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

const PANE_ID: u32 = 81;
const PANE_GENERATION: u64 = 1;
const REPLAY_MARKER: &[u8] = b"hyprmux-resurrect-replay-marker";

#[test]
fn subprocess_restart_restores_layout_and_pane_replay() {
    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let state_base = test_root.join("state");
    let config_base = test_root.join("config");
    fs::create_dir_all(&runtime_base).expect("create runtime base");
    fs::create_dir_all(&state_base).expect("create state base");
    fs::create_dir_all(&config_base).expect("create config base");
    let config_path = config_base.join("hyprmux.toml");
    fs::write(
        &config_path,
        "scrollback = 100\n\n[session]\nresurrect = true\n",
    )
    .expect("write test config");
    let endpoint = subprocess_endpoint(&runtime_base, &session);

    let child = spawn_server(
        &session,
        &runtime_base,
        &state_base,
        &config_base,
        &config_path,
    );
    let mut server = ServerGuard::new(child, test_root.clone());
    let mut client = connect_when_ready(&endpoint, server.child_mut());
    client.write_control(&ClientMessage::Attach {
        session: session.clone(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "snapshot-writer".into(),
        read_only: false,
    });
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        command: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: Some("resurrected pane".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(&mut client, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SpawnResult {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                ok: true,
                ..
            })
        )
    });
    client.write_control(&ClientMessage::SetSessionOrigin {
        profile: "work".into(),
    });
    client.write_pane_input(PANE_ID, PANE_GENERATION, b"i=0; while [ $i -lt 40 ]; do printf 'resurrect-line-%03d\\n' $i; i=$((i+1)); done; printf 'hyprmux-resurrect-%s\\n' 'replay-marker'\r");
    let mut live_output = Vec::new();
    read_until(&mut client, |frame| {
        if let Frame::PaneBytes { bytes, .. } = frame {
            live_output.extend_from_slice(bytes);
        }
        contains(&live_output, REPLAY_MARKER)
    });

    let layout = pane_layout();
    client.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: layout.clone(),
    });
    read_until(&mut client, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted { rev: 1, .. })
        )
    });
    client.write_control(&ClientMessage::Detach);
    drop(client);

    let snapshot = state_base.join("hyprmux").join("sessions").join(&session);
    wait_for_snapshot(&snapshot);
    let snapshot_replay = fs::read(snapshot.join("panes").join(format!("{PANE_ID}.replay")))
        .expect("read replay snapshot");
    assert!(
        contains(&snapshot_replay, REPLAY_MARKER),
        "snapshot replay omitted marker"
    );
    let mut before_restart = TerminalPane::new(100);
    before_restart.apply_server_resize(80, 24);
    before_restart.bind_server_backend(PANE_ID, PANE_GENERATION);
    before_restart.process_server_output(&snapshot_replay);
    assert!(before_restart.total_scrollback_rows() > 3);

    server.kill_for_restart();
    fs::write(
        &config_path,
        "scrollback = 3\n\n[session]\nresurrect = true\n",
    )
    .expect("lower scrollback before restart");
    let restarted = spawn_server(
        &session,
        &runtime_base,
        &state_base,
        &config_base,
        &config_path,
    );
    server.replace_child(restarted);

    let mut restored = connect_when_ready(&endpoint, server.child_mut());
    restored.write_control(&attach_message(&session, "snapshot-reader"));
    let mut attached = None;
    read_until(&mut restored, |frame| {
        if let Frame::Control(message @ ServerMessage::Attached { .. }) = frame {
            attached = Some(message.clone());
            true
        } else {
            false
        }
    });
    let ServerMessage::Attached {
        panes,
        layout_rev,
        layout: restored_layout,
        created_from_profile,
        ..
    } = attached.expect("restored attach response")
    else {
        unreachable!()
    };
    assert_eq!(layout_rev, 1);
    assert_eq!(created_from_profile.as_deref(), Some("work"));
    let restored_layout = restored_layout.expect("restored shared layout");
    assert_eq!(restored_layout.canvas_cols, layout.canvas_cols);
    assert_eq!(restored_layout.workspaces.len(), 1);
    assert!(panes.iter().any(|pane| pane.pane_id == PANE_ID));

    let generation = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("restored pane metadata")
        .generation;
    let mut replay = TerminalPane::new(100);
    replay.apply_server_resize(80, 24);
    replay.bind_server_backend(PANE_ID, generation);
    read_until(&mut restored, |frame| {
        if let Frame::PaneBytes {
            pane_id: PANE_ID,
            bytes,
            ..
        } = frame
        {
            replay.process_server_output(bytes);
        }
        replay
            .capture_scrollback_text(None)
            .contains("hyprmux-resurrect-replay-marker")
    });
    assert!(replay.total_scrollback_rows() <= 3);
    assert!(
        !replay
            .capture_scrollback_text(None)
            .contains("resurrect-line-000")
    );

    restored.write_control(&ClientMessage::Shutdown);
    drop(restored);
    server.wait_for_exit();
}

fn spawn_server(
    session: &str,
    runtime_base: &std::path::Path,
    state_base: &std::path::Path,
    config_base: &std::path::Path,
    config_path: &std::path::Path,
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(["--server", session])
        .env("HYPRMUX_CONFIG", config_path)
        .env("XDG_RUNTIME_DIR", runtime_base)
        .env("XDG_STATE_HOME", state_base)
        .env("XDG_CONFIG_HOME", config_base)
        .env("LOCALAPPDATA", state_base)
        .env("APPDATA", config_base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch real session server")
}

fn wait_for_snapshot(snapshot: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if snapshot.join("meta.json").is_file()
            && snapshot.join("layout.json").is_file()
            && snapshot
                .join("panes")
                .join(format!("{PANE_ID}.replay"))
                .is_file()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "session snapshot was not completed at {}",
        snapshot.display()
    );
}

fn pane_layout() -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 23,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: Some("resurrected".to_string()),
            synchronized: true,
            layout: SharedLayoutKind::Dwindle,
            start_axis: SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: Some(SharedTree::Leaf { pane: PANE_ID }),
            panes: vec![SharedPane {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                title: Some("resurrected pane".to_string()),
                profile_name: None,
                cwd: None,
                command: None,
                replay: false,
                keep_open: false,
                floating: false,
                fullscreen: false,
                rect: None,
                scrollable_width: rozi::state::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    }
}
