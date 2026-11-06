//! Session resurrection across a server restart on Windows, driven through a real subprocess and
//! ConPTY. The Unix counterpart is `session_resurrect_e2e.rs`.
//!
//! The pane runs an interactive PowerShell, launched directly so the test does not depend on which
//! shell the runner defaults to. The replay marker is assembled by PowerShell from two literals, so
//! the line the terminal echoes back never contains it, and a restored PowerShell starts with no
//! command, so the marker can only come back through the saved replay.
//!
//! The server is spawned with null and piped standard handles and waited on with deadlines, never
//! with `Command::output()`: a detached server inherits those handles, and an output collector then
//! waits on a pipe the server keeps open.
#![cfg(windows)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rozi::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use rozi::pane::TerminalPane;
use rozi::pane::launch::PaneLaunch;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{ClientMessage, Frame, ServerMessage, WirePalette};
use tui_lipan::prelude::TerminalColorPalette;

use common::{
    ServerGuard, TestConnection, attach_message, connect_when_ready, contains, private_temp_dir,
    read_until, subprocess_endpoint, unique_session_name,
};

const PANE_ID: u32 = 91;
const PANE_GENERATION: u64 = 1;
const REPLAY_MARKER: &str = "rozi-resurrect-replay-marker";

/// Where the restarted server finds the session's snapshot.
#[derive(Clone, Copy)]
enum SnapshotState {
    /// Published, as a completed write leaves it.
    Published,
    /// Moved to its backup name, as a write that died between its two renames leaves it.
    InterruptedReplacement,
}

struct Paths {
    test_root: PathBuf,
    local_app_data: PathBuf,
    roaming_app_data: PathBuf,
    config_path: PathBuf,
}

impl Paths {
    fn new() -> Self {
        let test_root = private_temp_dir();
        let local_app_data = test_root.join("AppData").join("Local");
        let roaming_app_data = test_root.join("AppData").join("Roaming");
        for dir in [&local_app_data, &roaming_app_data] {
            fs::create_dir_all(dir).expect("create isolated app data");
        }
        let config_path = test_root.join("config.toml");
        fs::write(
            &config_path,
            "scrollback = 100\n\n[session]\nresurrect = true\n",
        )
        .expect("write test config");
        Self {
            test_root,
            local_app_data,
            roaming_app_data,
            config_path,
        }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.local_app_data
            .join("rozi")
            .join("state")
            .join("sessions")
    }

    fn spawn_server(&self, session: &str) -> std::process::Child {
        Command::new(env!("CARGO_BIN_EXE_rozi"))
            .args(["--server", session])
            .env("ROZI_CONFIG", &self.config_path)
            .env("USERPROFILE", &self.test_root)
            .env("LOCALAPPDATA", &self.local_app_data)
            .env("APPDATA", &self.roaming_app_data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch real session server")
    }
}

#[test]
fn subprocess_restart_restores_layout_and_pane_replay() {
    restart_restores_the_session(SnapshotState::Published);
}

#[test]
fn subprocess_restart_recovers_a_snapshot_interrupted_mid_replacement() {
    restart_restores_the_session(SnapshotState::InterruptedReplacement);
}

fn restart_restores_the_session(state: SnapshotState) {
    let session = unique_session_name();
    let paths = Paths::new();
    let endpoint = subprocess_endpoint(&paths.test_root, &session);

    let mut server = ServerGuard::new(paths.spawn_server(&session), paths.test_root.clone());
    let mut client = connect_when_ready(&endpoint, server.child_mut());
    client.write_control(&attach_message(&session, "snapshot-writer"));
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    spawn_powershell(&mut client);

    // Wait for the prompt before typing, so the line is not read by a host still starting up.
    let mut live = TerminalPane::new(100);
    live.apply_server_resize(80, 24);
    live.bind_server_backend(PANE_ID, PANE_GENERATION);
    read_pane_until(&mut client, &mut live, PANE_GENERATION, |text| {
        text.contains("PS ")
    });
    client.write_pane_input(
        PANE_ID,
        PANE_GENERATION,
        b"1..40 | ForEach-Object { 'resurrect-line-{0:d3}' -f $_ }; 'rozi-resurrect-' + 'replay-marker'\r",
    );
    read_pane_until(&mut client, &mut live, PANE_GENERATION, |text| {
        text.lines().any(|line| line.trim() == REPLAY_MARKER)
    });

    let layout = pane_layout();
    client.write_control(&ClientMessage::CommitLayout {
        base_rev: 1,
        layout: layout.clone(),
        live: false,
    });
    read_until(&mut client, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted { rev: 2, .. })
        )
    });
    // The last client leaving a changed session is what triggers the snapshot.
    client.write_control(&ClientMessage::Detach);
    drop(client);

    let snapshot = paths.sessions_dir().join(&session);
    wait_for_snapshot(&snapshot);
    let replay_file = snapshot.join("panes").join(format!("{PANE_ID}.replay"));
    assert!(
        contains(
            &fs::read(&replay_file).expect("read replay snapshot"),
            REPLAY_MARKER.as_bytes()
        ),
        "snapshot replay omitted the marker"
    );

    server.kill_for_restart();
    let backup = paths.sessions_dir().join(format!(".{session}.old-4242.1"));
    if let SnapshotState::InterruptedReplacement = state {
        fs::rename(&snapshot, &backup).expect("move the snapshot to its backup name");
    }
    server.replace_child(paths.spawn_server(&session));

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
        ..
    } = attached.expect("restored attach response")
    else {
        unreachable!()
    };
    assert_eq!(layout_rev, 1);
    let restored_layout = restored_layout.expect("restored shared layout");
    assert_eq!(restored_layout.workspaces.len(), 1);
    assert_eq!(
        restored_layout.workspaces[0].name.as_deref(),
        Some("resurrected")
    );
    let meta = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("restored pane metadata");
    assert_eq!(meta.scrollback, 100);

    let mut replay = TerminalPane::new(meta.scrollback);
    replay.apply_server_resize(80, 24);
    replay.bind_server_backend(PANE_ID, meta.generation);
    read_pane_until(&mut restored, &mut replay, meta.generation, |text| {
        text.lines().any(|line| line.trim() == REPLAY_MARKER)
    });
    assert!(
        replay
            .capture_scrollback_text(None)
            .contains("resurrect-line-001"),
        "the replay carries the history above the marker"
    );
    assert!(snapshot.join("meta.json").is_file(), "snapshot in place");
    assert!(!backup.exists(), "no backup left behind");

    restored.write_control(&ClientMessage::Shutdown);
    drop(restored);
    server.wait_for_exit();
}

fn spawn_powershell(client: &mut TestConnection) {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PANE_ID, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: Some(PaneLaunch::Direct {
            argv: ["powershell", "-NoLogo", "-NoProfile"]
                .map(str::to_string)
                .to_vec(),
        }),
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
    read_until(client, |frame| match frame {
        Frame::Control(ServerMessage::SpawnResult {
            pane_id: PANE_ID,
            ok,
            error,
            ..
        }) => {
            assert!(*ok, "spawn failed: {error:?}");
            true
        }
        _ => false,
    });
}

/// Feed the pane's output into `pane` until its text satisfies `done`.
#[track_caller]
fn read_pane_until(
    client: &mut TestConnection,
    pane: &mut TerminalPane,
    generation: u64,
    mut done: impl FnMut(&str) -> bool,
) {
    read_until(client, |frame| {
        if let Frame::PaneOutput {
            pane_id: PANE_ID,
            generation: frame_generation,
            bytes,
            ..
        } = frame
            && *frame_generation == generation
        {
            pane.process_server_output(bytes);
        }
        done(&pane.capture_scrollback_text(None))
    });
}

fn wait_for_snapshot(snapshot: &Path) {
    let deadline = Instant::now() + common::io_timeout();
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
        pane_geometry: Default::default(),
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
                launch: None,
                replay: false,
                keep_open: false,
                floating: false,
                pinned: false,
                fullscreen: false,
                rect: None,
                scrollable_width: rozi::layout::model::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    }
}
