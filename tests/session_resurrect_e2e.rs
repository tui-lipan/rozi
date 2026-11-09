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
//! compiled out. Windows resurrection has its own test, with a shell it can actually run, in
//! `session_resurrect_e2e_windows.rs`.
#![cfg(unix)]

mod common;

use std::fs;
#[cfg(target_os = "linux")]
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rozi::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use rozi::pane::TerminalPane;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, Frame, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use tui_lipan::prelude::TerminalColorPalette;

use common::{
    ServerGuard, attach_message, connect_when_ready, contains, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

const PANE_ID: u32 = 81;
const PANE_GENERATION: u64 = 1;
const REPLAY_MARKER: &[u8] = b"rozi-resurrect-replay-marker";

#[test]
fn subprocess_restart_restores_layout_and_pane_replay() {
    restart_restores_layout_and_pane_replay(false);
}

#[test]
fn subprocess_restart_migrates_v2_layout_and_pane_replay() {
    restart_restores_layout_and_pane_replay(true);
}

fn restart_restores_layout_and_pane_replay(migrate_v2: bool) {
    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let state_base = test_root.join("state");
    let config_base = test_root.join("config");
    fs::create_dir_all(&runtime_base).expect("create runtime base");
    fs::create_dir_all(&state_base).expect("create state base");
    fs::create_dir_all(&config_base).expect("create config base");
    let config_path = config_base.join("config.toml");
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
        capabilities: None,
        session: session.clone(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "snapshot-writer".into(),
        read_only: false,
        shares_filesystem: true,
        expected_server_nonce: None,
        handoff_token: None,
    });
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PANE_ID, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: None,
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
        origin: rozi::session::origin::SessionOrigin {
            profile: Some("work".into()),
            ..Default::default()
        },
    });
    client.write_pane_input(PANE_ID, PANE_GENERATION, b"i=0; while [ $i -lt 40 ]; do printf 'resurrect-line-%03d\\n' $i; i=$((i+1)); done; printf 'rozi-resurrect-%s\\n' 'replay-marker'\r");
    let mut live_output = Vec::new();
    read_until(&mut client, |frame| {
        if let Frame::PaneOutput { bytes, .. } = frame {
            live_output.extend_from_slice(bytes);
        }
        contains(&live_output, REPLAY_MARKER)
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
    client.write_control(&ClientMessage::Detach);
    drop(client);

    let snapshot = state_base.join("rozi").join("sessions").join(&session);
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
    if migrate_v2 {
        let path = snapshot.join("meta.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        document["version"] = 2.into();
        for pane in document["panes"].as_array_mut().unwrap() {
            pane.as_object_mut().unwrap().remove("foreground");
        }
        fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    }
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
        origin,
        ..
    } = attached.expect("restored attach response")
    else {
        unreachable!()
    };
    assert_eq!(layout_rev, 1);
    assert_eq!(origin.profile.as_deref(), Some("work"));
    let restored_layout = restored_layout.expect("restored shared layout");
    assert_eq!(restored_layout.canvas_cols, layout.canvas_cols);
    assert_eq!(restored_layout.workspaces.len(), 1);
    assert!(panes.iter().any(|pane| pane.pane_id == PANE_ID));

    let meta = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("restored pane metadata");
    let generation = meta.generation;
    assert_eq!(meta.scrollback, 100);
    let mut replay = TerminalPane::new(meta.scrollback);
    replay.apply_server_resize(80, 24);
    replay.bind_server_backend(PANE_ID, generation);
    read_until(&mut restored, |frame| {
        if let Frame::PaneOutput {
            pane_id: PANE_ID,
            bytes,
            ..
        } = frame
        {
            replay.process_server_output(bytes);
        }
        replay
            .capture_scrollback_text(None)
            .contains("rozi-resurrect-replay-marker")
    });
    assert!(replay.total_scrollback_rows() > 3);
    assert!(
        replay
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
        .env("ROZI_CONFIG", config_path)
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

/// A shell pane restored as a shell must come back running whatever was in it. This is the whole
/// difference between restoring a session and restoring a transcript of one.
///
/// Linux-only, unlike the rest of this file. Replaying a command means replaying its arguments,
/// and reading another process's `argv` is what macOS does not permit; `tail` without the file it
/// was following is a pane waiting on stdin, so a macOS run would assert on output that could
/// never arrive. The capture rules themselves are covered by unit tests on every platform.
#[cfg(target_os = "linux")]
#[test]
fn a_restored_shell_reruns_the_command_it_was_holding() {
    const PROBE_PANE: u32 = 82;
    const MARKER: &str = "rozi-foreground-rerun-marker";
    const RESTORED_MARKER: &str = "rozi-foreground-live-after-restore";

    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let state_base = test_root.join("state");
    let config_base = test_root.join("config");
    for base in [&runtime_base, &state_base, &config_base] {
        fs::create_dir_all(base).expect("create test base");
    }
    let config_path = config_base.join("config.toml");
    fs::write(
        &config_path,
        "scrollback = 100\n\n[session]\nresurrect = true\nresurrect_foreground = \"auto\"\n",
    )
    .expect("write test config");
    // The marker lives in the file `tail` follows, never in the command line. A restored shell
    // echoes what is typed into it, and a marker inside that echo would let this test pass
    // without the command ever having run again.
    //
    // The file sits in a directory the shell only enters on the same line that starts `tail`, and
    // `tail` names it relatively. The shell's own directory report still names where the line
    // started, so the restored pane finds the file only if the snapshot kept the directory `tail`
    // was actually running in.
    let workdir = test_root.join("work");
    fs::create_dir_all(&workdir).expect("create work directory");
    let workdir = fs::canonicalize(&workdir).expect("resolve work directory");
    let probe = workdir.join("probe.txt");
    fs::write(&probe, format!("{MARKER}\n")).expect("write probe file");
    let command = "tail -f probe.txt";

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
    client.write_control(&attach_message(&session, "foreground-writer"));
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });

    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PROBE_PANE, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PROBE_PANE,
        generation: PANE_GENERATION,
        // No launch intent at all: this is the pane the old snapshot format had nothing to say
        // about beyond "it was a shell".
        launch: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: None,
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
                pane_id: PROBE_PANE,
                ok: true,
                ..
            })
        )
    });

    // The `printf` is the report a shell with integration makes at its prompt, sent explicitly so
    // the test does not depend on the host shell's integration being active.
    client.write_pane_input(
        PROBE_PANE,
        PANE_GENERATION,
        format!(
            "printf '\\033]7;file://localhost%s\\033\\\\' \"$PWD\"; cd '{}' && {command}\r",
            workdir.display()
        )
        .as_bytes(),
    );
    // A snapshot records what the last runtime poll saw, so detaching before the server has
    // noticed `tail` holding the terminal would capture a bare shell and prove nothing. Its name
    // can arrive a poll before its arguments, and a detach between the two would save `tail` alone.
    let expected_cwd = workdir.to_string_lossy().into_owned();
    read_until(&mut client, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::PaneRuntimeChanged { pane_id: PROBE_PANE, state, .. })
                if state.foreground_program.as_deref() == Some("tail")
                    && state.foreground_arguments == ["-f", "probe.txt"]
                    && state.cwd.as_deref() == Some(expected_cwd.as_str())
        )
    });

    client.write_control(&ClientMessage::Detach);
    drop(client);

    let snapshot = state_base.join("rozi").join("sessions").join(&session);
    wait_for_meta(&snapshot, command);

    server.kill_for_restart();
    let restarted = spawn_server(
        &session,
        &runtime_base,
        &state_base,
        &config_base,
        &config_path,
    );
    server.replace_child(restarted);

    let mut restored = connect_when_ready(&endpoint, server.child_mut());
    restored.write_control(&attach_message(&session, "foreground-reader"));
    read_until(&mut restored, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });

    // This marker did not exist when the replay was captured. Seeing it proves the restored
    // `tail -f` is live, rather than merely seeing the old marker in restored scrollback.
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&probe)
            .expect("open probe for append"),
        "{RESTORED_MARKER}"
    )
    .expect("append restored marker");
    let mut output = Vec::new();
    read_until(&mut restored, |frame| {
        if let Frame::PaneOutput {
            pane_id: PROBE_PANE,
            bytes,
            ..
        } = frame
        {
            output.extend_from_slice(bytes);
        }
        contains(&output, RESTORED_MARKER.as_bytes())
    });

    restored.write_control(&ClientMessage::Shutdown);
    drop(restored);
    server.wait_for_exit();
}

#[cfg(target_os = "linux")]
fn wait_for_meta(snapshot: &std::path::Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(meta) = fs::read_to_string(snapshot.join("meta.json"))
            && meta.contains(expected)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "session snapshot at {} did not record {expected:?}; meta.json: {}",
        snapshot.display(),
        fs::read_to_string(snapshot.join("meta.json")).unwrap_or_else(|err| err.to_string())
    );
}

/// A server must not start over a snapshot it cannot read: running empty, its first save would
/// replace the snapshot, and with it the session, for the build that can read it.
#[test]
fn a_server_refuses_to_start_over_a_snapshot_it_cannot_read() {
    // Left by another build's interrupted save, in a format this build does not read.
    refuses_to_start_over(
        |sessions, session| sessions.join(format!(".{session}.old-4242.1")),
        |session| {
            Some(format!(
                r#"{{"version":99,"session":"{session}","future":true}}"#
            ))
        },
        "its snapshot is in format 99",
        true,
    );
}

/// A published snapshot whose metadata is gone is damaged, not absent: what is left of it may
/// still be salvaged, so it is not started over either.
#[test]
fn a_server_refuses_to_start_over_a_snapshot_missing_its_metadata() {
    refuses_to_start_over(
        |sessions, session| sessions.join(session),
        |_| None,
        "its snapshot is damaged",
        false,
    );
}

/// Nor over a backup that lost its metadata, which recovery publishes for the server to refuse.
#[test]
fn a_server_refuses_to_start_over_a_backup_missing_its_metadata() {
    refuses_to_start_over(
        |sessions, session| sessions.join(format!(".{session}.old-4242.1")),
        |_| None,
        "its snapshot is damaged",
        false,
    );
}

fn refuses_to_start_over(
    snapshot_dir: impl Fn(&std::path::Path, &str) -> std::path::PathBuf,
    meta: impl Fn(&str) -> Option<String>,
    reason: &str,
    preserve_in_place: bool,
) {
    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let state_base = test_root.join("state");
    let config_base = test_root.join("config");
    for dir in [&runtime_base, &state_base, &config_base] {
        fs::create_dir_all(dir).expect("create isolated base");
    }
    let config_path = config_base.join("config.toml");
    fs::write(&config_path, "[session]\nresurrect = true\n").expect("write test config");
    let sessions = state_base.join("rozi").join("sessions");
    let saved = snapshot_dir(&sessions, &session);
    fs::create_dir_all(saved.join("panes")).expect("create snapshot");
    let meta = meta(&session);
    if let Some(meta) = &meta {
        fs::write(saved.join("meta.json"), meta).expect("write snapshot meta");
    }
    fs::write(saved.join("panes").join("1.replay"), b"kept").expect("write snapshot replay");

    let mut child = spawn_server(
        &session,
        &runtime_base,
        &state_base,
        &config_base,
        &config_path,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll server") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the server started over a snapshot it cannot read");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().expect("stderr"), &mut stderr)
        .expect("read server stderr");
    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains(reason), "the refusal says why: {stderr}");
    let published = sessions.join(&session);
    let preserved = if preserve_in_place {
        assert!(!published.exists(), "a newer backup must not be promoted");
        saved
    } else {
        published
    };
    assert_eq!(
        fs::read_to_string(preserved.join("meta.json")).ok(),
        meta,
        "the snapshot metadata is unchanged"
    );
    assert_eq!(
        fs::read(preserved.join("panes").join("1.replay")).expect("preserved replay"),
        b"kept"
    );
    assert!(
        !subprocess_endpoint(&runtime_base, &session).path().exists(),
        "no endpoint is left behind"
    );
    let _ = fs::remove_dir_all(test_root);
}
