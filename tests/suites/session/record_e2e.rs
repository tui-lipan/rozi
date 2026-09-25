//! End-to-end coverage for recording a pane of a session nobody is attached to.
//!
//! Every request goes through [`run_session_control`] over the real endpoint, answered by the real
//! session server, and the recording is read back and exported with the same code `rozi record
//! export` runs.

use std::path::Path;
use std::time::{Duration, Instant};

use rozi::control::{
    ControlCommand, ControlRequest, ControlResponse, RecordingInfo, RecordingStopped,
};
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::recording::export;
use rozi::recording::{EndReason, RecordingMeta, ReplayStep};
use rozi::session::headless::run_session_control;
use rozi::session::protocol::{
    Capabilities, ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION,
};
use rozi::session::server::ServerSettings;

use crate::common::{TestConnection, io_timeout, spawn_listener};

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        extension: None,
    }
}

#[track_caller]
fn expect_ok(session: &str, command: ControlCommand) -> serde_json::Value {
    let response: ControlResponse =
        run_session_control(session, request(command)).expect("the session answered");
    assert!(response.ok, "command failed: {:?}", response.error);
    response.data.unwrap_or(serde_json::Value::Null)
}

fn settings() -> ServerSettings {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    ServerSettings {
        shell,
        command_shell,
        ..ServerSettings::default()
    }
}

fn spawn(session: &str, script: &str) -> u32 {
    let spawned = expect_ok(
        session,
        ControlCommand::NewPane {
            command: None,
            argv: Some(vec!["sh".into(), "-c".into(), script.into()]),
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
        },
    );
    spawned["id"].as_u64().expect("a pane id") as u32
}

fn start(pane: u32, path: &Path) -> ControlCommand {
    ControlCommand::RecordStart {
        target: Some(pane),
        output: path.display().to_string(),
        max_fps: None,
        duration_ms: None,
        max_bytes: None,
        force: false,
        follow: false,
    }
}

fn recordings(session: &str) -> Vec<RecordingInfo> {
    serde_json::from_value(expect_ok(session, ControlCommand::RecordList)).unwrap()
}

fn pane_recording(session: &str, pane: u32) -> bool {
    expect_ok(session, ControlCommand::ListPanes)
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == pane)
        .is_some_and(|row| row["recording"] == true)
}

#[track_caller]
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + io_timeout();
    while !condition() {
        assert!(Instant::now() < deadline, "never saw: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Each frame's time and visible text, the marks, the meta events, and how it ended.
struct Played {
    frames: Vec<(u64, String)>,
    marks: Vec<String>,
    meta: Vec<RecordingMeta>,
    end: EndReason,
}

fn play(path: &Path) -> Played {
    let mut replay = export::open(path).unwrap();
    let mut played = Played {
        frames: Vec::new(),
        marks: Vec::new(),
        meta: Vec::new(),
        end: EndReason::Unknown,
    };
    while let Some(step) = replay.step().unwrap() {
        match step {
            ReplayStep::Frame { t } => {
                let text = replay
                    .frame()
                    .unwrap()
                    .rows
                    .iter()
                    .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n");
                played.frames.push((t, text));
            }
            ReplayStep::Mark { label, .. } => played.marks.push(label),
            ReplayStep::Meta { meta, .. } => played.meta.push(meta),
            ReplayStep::End(end) => played.end = end.reason,
        }
    }
    assert!(!replay.truncated());
    played
}

/// When the recording first shows `text`.
fn first_seen(played: &Played, text: &str) -> u64 {
    played
        .frames
        .iter()
        .find(|(_, frame)| frame.contains(text))
        .unwrap_or_else(|| panic!("no frame shows {text:?}"))
        .0
}

#[test]
fn a_detached_pane_is_recorded_after_its_caller_exits_and_exports_with_real_timing() {
    let server = spawn_listener(settings());
    let session = server.session().to_string();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ticks.rozirec");
    let pane = spawn(
        &session,
        "sleep 0.3; for i in 1 2 3 4 5 6; do echo tick $i; sleep 0.3; done; sleep 60",
    );

    // The request's connection closes once it is answered, as the CLI's would: the recording
    // carries on in the server.
    let started: RecordingInfo =
        serde_json::from_value(expect_ok(&session, start(pane, &path))).unwrap();
    assert_eq!(started.pane, pane);
    assert!(pane_recording(&session, pane));
    wait_until("the last tick recorded", || {
        recordings(&session)[0].totals.frames >= 7
            && std::fs::read_to_string(&path).is_ok_and(|text| text.contains("tick 6"))
    });
    expect_ok(
        &session,
        ControlCommand::RecordMark {
            label: "ticked".into(),
            id: None,
        },
    );
    let stopped: RecordingStopped =
        serde_json::from_value(expect_ok(&session, ControlCommand::RecordStop { id: None }))
            .unwrap();
    assert_eq!(stopped.reason, EndReason::Stopped);
    assert_eq!(stopped.totals.dropped, 0);
    assert!(recordings(&session).is_empty());
    assert!(!pane_recording(&session, pane));

    let played = play(&path);
    assert_eq!(played.end, EndReason::Stopped);
    assert_eq!(played.marks, ["ticked"]);
    // Each tick is a change of its own, written when it happened: about 300ms apart.
    let seen: Vec<u64> = (1..=6)
        .map(|i| first_seen(&played, &format!("tick {i}")))
        .collect();
    for pair in seen.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            (150..=1_000).contains(&gap),
            "ticks {gap}ms apart: {seen:?}"
        );
    }
    // Only changes are written: nothing between one tick and the next.
    assert!(
        (7..=14).contains(&played.frames.len()),
        "{} frames for six ticks",
        played.frames.len()
    );

    let frames_dir = dir.path().join("frames");
    let summary = export::png_frames(export::open(&path).unwrap(), &frames_dir, 1, false).unwrap();
    assert_eq!(summary.frames, played.frames.len() as u64);
    let listing = std::fs::read_to_string(frames_dir.join(export::CONCAT_LISTING)).unwrap();
    let durations: Vec<f64> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("duration "))
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(durations.len(), played.frames.len());
    let total: f64 = durations.iter().sum();
    assert!(
        (total * 1000.0 - summary.duration_ms as f64).abs() < 5.0,
        "listing covers {total}s of {}ms",
        summary.duration_ms
    );
    assert!(
        frames_dir
            .join(format!("frame-{:06}.png", summary.frames))
            .is_file()
    );
}

#[test]
fn a_pane_exiting_mid_recording_ends_it_on_the_last_screen() {
    let server = spawn_listener(settings());
    let session = server.session().to_string();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exit.rozirec");
    let pane = spawn(&session, "sleep 0.5; echo last words; exit 3");
    expect_ok(&session, start(pane, &path));
    wait_until("the recording to end with the pane", || {
        recordings(&session).is_empty()
    });

    let played = play(&path);
    assert_eq!(played.end, EndReason::PaneExited);
    assert!(played.meta.contains(&RecordingMeta::Exited { status: 3 }));
    assert!(played.frames.last().unwrap().1.contains("last words"));
}

#[test]
fn a_foreground_recording_stops_when_its_caller_goes_away() {
    let server = spawn_listener(settings());
    let session = server.session().to_string();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("follow.rozirec");
    let pane = spawn(&session, "while :; do date +%N; sleep 0.1; done");

    let mut caller = TestConnection::connect(server.endpoint());
    let ControlCommand::RecordStart { target, output, .. } = start(pane, &path) else {
        unreachable!()
    };
    caller.write_control(&ClientMessage::SessionControl {
        capabilities: Some(Capabilities::current()),
        session: session.clone(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        request: request(ControlCommand::RecordStart {
            target,
            output,
            max_fps: None,
            duration_ms: None,
            max_bytes: None,
            force: false,
            follow: true,
        }),
    });
    wait_until("the followed recording to write frames", || {
        recordings(&session)
            .first()
            .is_some_and(|recording| recording.follow && recording.totals.frames >= 3)
    });
    // Ctrl-C on `rozi record pane` closes this connection.
    drop(caller);
    wait_until("the recording to stop with its caller", || {
        recordings(&session).is_empty()
    });
    assert_eq!(play(&path).end, EndReason::Stopped);
}
