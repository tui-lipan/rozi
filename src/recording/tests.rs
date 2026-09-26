use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use tui_lipan::prelude::*;

use super::format::{FrameDelta, RecordingEvent, RecordingMeta};
use super::*;
use crate::control::{SPAN_FRAME_VERSION, SpanFrame};

fn header(width: u16, height: u16) -> RecordingHeader {
    RecordingHeader {
        format: RECORDING_FORMAT.to_string(),
        version: RECORDING_VERSION,
        rozi: env!("CARGO_PKG_VERSION").to_string(),
        target: RecordingTarget::Pane {
            session: "dev".to_string(),
            pane: 3,
        },
        width,
        height,
        started_at_unix_ms: 0,
        max_fps: 30,
        keyframe_interval_ms: format::KEYFRAME_INTERVAL_MS,
        spans_version: SPAN_FRAME_VERSION,
        palette: span(&mut TerminalScreen::new(1, 1, 0)).palette,
        compression: None,
    }
}

fn options(path: &Path, max_bytes: u64) -> RecorderOptions {
    RecorderOptions {
        path: path.to_path_buf(),
        overwrite: false,
        header: header(20, 4),
        max_bytes,
    }
}

fn span(screen: &mut TerminalScreen) -> SpanFrame {
    crate::pane::spans::span_frame(&screen.capture_frame(), screen.palette(), false).unwrap()
}

fn push(recorder: &Recorder, t: u64, screen: &TerminalScreen) -> bool {
    recorder.push_frame(t, screen.capture_frame(), screen.palette())
}

fn finish(recorder: Recorder, t: u64, reason: EndReason) -> RecorderOutcome {
    recorder.finish(t, reason);
    recorder
        .join(Duration::from_secs(10))
        .expect("the writer finished")
}

/// Every frame, mark, meta, and end a recording replays, with each frame whole.
#[derive(Debug, Default)]
struct Replayed {
    frames: Vec<(u64, SpanFrame)>,
    marks: Vec<(u64, String)>,
    meta: Vec<RecordingMeta>,
    end: Option<RecordingEnd>,
    truncated: bool,
}

fn replay(path: &Path) -> Replayed {
    replay_bytes(&std::fs::read(path).unwrap())
}

fn replay_bytes(bytes: &[u8]) -> Replayed {
    let mut replay = Replay::new(BufReader::new(bytes)).expect("a recording");
    let mut out = Replayed::default();
    while let Some(step) = replay.step().expect("a readable recording") {
        match step {
            ReplayStep::Frame { t } => out.frames.push((t, replay.frame().unwrap().clone())),
            ReplayStep::Mark { t, label } => out.marks.push((t, label)),
            ReplayStep::Meta { meta, .. } => out.meta.push(meta),
            ReplayStep::End(end) => out.end = Some(end),
        }
    }
    out.truncated = replay.truncated();
    out
}

fn events(path: &Path) -> Vec<RecordingEvent> {
    let bytes = std::fs::read(path).unwrap();
    let mut reader = RecordingReader::new(BufReader::new(bytes.as_slice())).unwrap();
    let mut events = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        events.push(event);
    }
    events
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pane.rozirec");
    (dir, path)
}

#[test]
fn keyframes_and_deltas_reproduce_every_frame_exactly() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    let mut expected = Vec::new();
    let steps: [(u64, &[u8]); 6] = [
        (0, b"$ "),
        (40, b"\x1b[31mcargo test\x1b[0m\r\n"),
        (80, b"running 3 tests\r\n"),
        // Long enough after the first keyframe that a periodic one is due.
        (11_000, b"\x1b[1mok\x1b[0m"),
        (11_050, b"\x1b[2J\x1b[H\x1b[44m  \x1b[0m"),
        (11_100, "wide 中文".as_bytes()),
    ];
    for (t, bytes) in steps {
        screen.process_bytes(bytes);
        push(&recorder, t, &screen);
        expected.push((t, span(&mut screen)));
    }
    screen.resize(6, 30);
    screen.process_bytes(b"\r\nresized");
    push(&recorder, 12_000, &screen);
    expected.push((12_000, span(&mut screen)));
    let outcome = finish(recorder, 12_500, EndReason::Stopped);

    let replayed = replay(&path);
    assert_eq!(replayed.frames, expected);
    assert_eq!(outcome.reason, EndReason::Stopped);
    assert_eq!(replayed.end.as_ref().unwrap().reason, EndReason::Stopped);
    assert_eq!(replayed.end.as_ref().unwrap().t, 12_500);
    assert!(!replayed.truncated);

    let kinds: Vec<&str> = events(&path)
        .iter()
        .map(|event| match event {
            RecordingEvent::Keyframe { .. } => "keyframe",
            RecordingEvent::Delta(_) => "delta",
            RecordingEvent::Resize { .. } => "resize",
            RecordingEvent::End(_) => "end",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "keyframe", "delta", "delta", "keyframe", "delta", "delta", "resize", "keyframe", "end"
        ],
        "a keyframe starts, recurs after the interval, and follows a resize"
    );
    assert_eq!(
        (
            outcome.totals.frames,
            outcome.totals.keyframes,
            outcome.totals.deltas
        ),
        (7, 3, 4)
    );
}

#[test]
fn a_delta_carries_only_the_rows_that_changed() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    screen.process_bytes(b"one\r\ntwo\r\nthree");
    push(&recorder, 0, &screen);
    screen.process_bytes(b"\x1b[2;1HTWO");
    push(&recorder, 10, &screen);
    finish(recorder, 20, EndReason::Stopped);

    let delta = events(&path)
        .into_iter()
        .find_map(|event| match event {
            RecordingEvent::Delta(delta) => Some(delta),
            _ => None,
        })
        .expect("a delta");
    assert_eq!(delta.rows.len(), 1);
    assert_eq!(delta.rows[0].y, 1);
    assert_eq!(delta.rows[0].runs[0].text, "TWO");
    assert!(delta.images.is_none() && delta.palette.is_none());
    assert!(delta.cursor.is_some(), "the cursor moved with the write");
}

#[test]
fn an_unchanged_screen_writes_nothing() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    screen.process_bytes(b"still");
    for t in 0..10 {
        push(&recorder, t * 100, &screen);
    }
    let outcome = finish(recorder, 1_000, EndReason::Stopped);
    assert_eq!(outcome.totals.frames, 1);
    assert_eq!(replay(&path).frames.len(), 1);
}

#[test]
fn a_truncated_recording_reads_up_to_its_last_complete_event() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for t in 0..5 {
        screen.process_bytes(format!("line {t}\r\n").as_bytes());
        push(&recorder, t * 10, &screen);
    }
    finish(recorder, 100, EndReason::Stopped);
    let bytes = std::fs::read(&path).unwrap();
    let whole = replay_bytes(&bytes);

    // Cut at every byte after the header: each prefix still reads, and holds the frames of its
    // complete lines.
    let header_end = bytes.iter().position(|&b| b == b'\n').unwrap() + 1;
    for cut in header_end..bytes.len() {
        let prefix = &bytes[..cut];
        let read = replay_bytes(prefix);
        let complete_lines = prefix.iter().filter(|&&b| b == b'\n').count() - 1;
        assert!(read.frames.len() <= complete_lines);
        assert_eq!(read.frames[..], whole.frames[..read.frames.len()]);
        assert_eq!(read.truncated, !prefix.ends_with(b"\n"));
    }
    let short = replay_bytes(&bytes[..bytes.len() - 5]);
    assert!(short.truncated && short.end.is_none());
    assert_eq!(short.frames, whole.frames);
}

#[test]
fn an_image_is_stored_once_however_many_frames_show_it() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    screen.set_cell_size(TerminalCellSize {
        width: 10,
        height: 20,
    });
    let pixels = [255u8, 0, 0].repeat(20 * 20);
    screen.process_bytes(
        format!(
            "\x1b_Ga=T,f=24,s=20,v=20,t=d,i=1;{}\x1b\\",
            base64::engine::general_purpose::STANDARD.encode(pixels)
        )
        .as_bytes(),
    );
    for t in 0..5 {
        screen.process_bytes(format!("\x1b[4;1Htick {t}").as_bytes());
        push(&recorder, t * 10, &screen);
    }
    let outcome = finish(recorder, 100, EndReason::Stopped);
    assert_eq!(outcome.totals.images, 1);
    assert_eq!(outcome.totals.frames, 5);

    let stored: Vec<_> = events(&path)
        .into_iter()
        .filter_map(|event| match event {
            RecordingEvent::Image(image) => Some(image),
            _ => None,
        })
        .collect();
    assert_eq!(stored.len(), 1);

    // Every frame names it, and a player gets the pixels back.
    let bytes = std::fs::read(&path).unwrap();
    let mut replay = Replay::new(BufReader::new(bytes.as_slice())).unwrap();
    let mut frames = 0;
    while let Some(step) = replay.step().unwrap() {
        if let ReplayStep::Frame { .. } = step {
            frames += 1;
            let id = replay.frame().unwrap().images[0].id.clone().unwrap();
            assert_eq!(id, stored[0].id);
            let decoded = &replay.frame_images().unwrap()[&id];
            assert_eq!((decoded.width, decoded.height), (20, 20));
            assert_eq!(&decoded.rgba[..4], &[255, 0, 0, 255]);
        }
    }
    assert_eq!(frames, 5);
}

#[test]
fn a_stalled_writer_keeps_the_newest_state_and_counts_what_it_dropped() {
    let (_dir, path) = scratch();
    let mut recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    let total = writer::QUEUE_FRAMES as u64 + 12;
    for t in 0..total {
        screen.process_bytes(format!("\x1b[1;1Hframe {t}").as_bytes());
        // Never waits, however far behind the writer is.
        push(&recorder, t * 10, &screen);
    }
    assert!(recorder.mark(500, "kept".to_string()));
    assert_eq!(recorder.totals().dropped, 12);
    let last = span(&mut screen);

    recorder.resume();
    let outcome = finish(recorder, 1_000, EndReason::Stopped);
    assert_eq!(outcome.totals.dropped, 12);
    assert_eq!(outcome.totals.frames, writer::QUEUE_FRAMES as u64);

    let replayed = replay(&path);
    assert_eq!(replayed.frames.last().unwrap(), &((total - 1) * 10, last));
    assert_eq!(replayed.marks, [(500, "kept".to_string())]);
    assert_eq!(
        replayed.end.unwrap().totals.dropped,
        12,
        "the file records it"
    );
}

#[test]
fn a_recording_stops_at_its_byte_cap_and_stays_readable() {
    let (_dir, path) = scratch();
    let max_bytes = writer::MIN_MAX_BYTES;
    let recorder = Recorder::start(options(&path, max_bytes)).unwrap();
    let mut screen = TerminalScreen::new(40, 120, 100);
    for t in 0..2_000u64 {
        screen.process_bytes(format!("\x1b[{};1H{}", t % 40 + 1, "x".repeat(100)).as_bytes());
        screen.process_bytes(format!("\x1b[1;1H{t}").as_bytes());
        push(&recorder, t, &screen);
        if recorder.closed() {
            break;
        }
    }
    let outcome = recorder
        .join(Duration::from_secs(10))
        .expect("the cap ended it");
    assert_eq!(outcome.reason, EndReason::MaxBytes);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size <= max_bytes, "{size} > {max_bytes}");
    let replayed = replay(&path);
    assert_eq!(replayed.end.unwrap().reason, EndReason::MaxBytes);
    assert!(!replayed.frames.is_empty());
}

#[test]
fn every_end_reason_is_written_as_the_last_event() {
    for reason in [
        EndReason::Stopped,
        EndReason::Duration,
        EndReason::PaneExited,
        EndReason::PaneClosed,
        EndReason::SessionEnded,
        EndReason::ServerShutdown,
    ] {
        let (_dir, path) = scratch();
        let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
        let mut screen = TerminalScreen::new(4, 20, 100);
        screen.process_bytes(b"x");
        push(&recorder, 0, &screen);
        recorder.meta(5, RecordingMeta::Exited { status: 3 });
        let outcome = finish(recorder, 10, reason);
        assert_eq!(outcome.reason, reason);
        let events = events(&path);
        let Some(RecordingEvent::End(end)) = events.last() else {
            panic!("{reason:?}: the last event is not an end: {events:?}");
        };
        assert_eq!(end.reason, reason);
        assert_eq!(end.totals.frames, 1);
    }
}

#[test]
fn a_recording_is_private_and_never_replaces_a_file_unless_forced() {
    let (_dir, path) = scratch();
    std::fs::write(&path, b"precious").unwrap();
    let refused = Recorder::start(options(&path, u64::MAX))
        .err()
        .expect("refused");
    assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&path).unwrap(), b"precious");

    let mut forced = options(&path, u64::MAX);
    forced.overwrite = true;
    finish(Recorder::start(forced).unwrap(), 0, EndReason::Stopped);
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(b"{\"format\":\"rozi-recording\"")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // Forcing never writes through a link.
        let link = path.with_extension("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let mut through = options(&link, u64::MAX);
        through.overwrite = true;
        assert!(Recorder::start(through).is_err());
    }
}

#[test]
fn the_wire_shape_of_events() {
    let delta = RecordingEvent::Delta(FrameDelta {
        t: 7,
        cursor: Some(None),
        ..FrameDelta::default()
    });
    assert_eq!(
        serde_json::to_value(&delta).unwrap(),
        serde_json::json!({"kind": "delta", "t": 7, "cursor": null})
    );
    let parsed: RecordingEvent =
        serde_json::from_value(serde_json::json!({"kind": "delta", "t": 7, "cursor": null}))
            .unwrap();
    assert_eq!(
        parsed, delta,
        "a null cursor is a change, not an absent one"
    );
    let unchanged: RecordingEvent =
        serde_json::from_value(serde_json::json!({"kind": "delta", "t": 7})).unwrap();
    assert!(matches!(
        unchanged,
        RecordingEvent::Delta(FrameDelta { cursor: None, .. })
    ));

    let meta = RecordingEvent::Meta {
        t: 3,
        meta: RecordingMeta::CommandFinished { status: Some(1) },
    };
    assert_eq!(
        serde_json::to_value(&meta).unwrap(),
        serde_json::json!({"kind": "meta", "t": 3, "event": "command-finished", "status": 1})
    );
    assert_eq!(
        serde_json::from_value::<RecordingEvent>(serde_json::to_value(&meta).unwrap()).unwrap(),
        meta
    );

    let end = RecordingEvent::End(RecordingEnd {
        t: 9,
        reason: EndReason::MaxBytes,
        totals: RecordingTotals {
            dropped: 2,
            ..RecordingTotals::default()
        },
    });
    let value = serde_json::to_value(&end).unwrap();
    assert_eq!(value["reason"], "max-bytes");
    assert_eq!(value["dropped"], 2);
}

#[test]
fn a_reader_skips_what_it_does_not_know() {
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&file).unwrap();
    value["from_the_future"] = serde_json::json!(true);
    file = serde_json::to_vec(&value).unwrap();
    file.extend_from_slice(b"\n{\"kind\":\"hologram\",\"t\":1}\n");
    let frame = span(&mut TerminalScreen::new(1, 2, 0));
    let mut keyframe = serde_json::to_value(RecordingEvent::Keyframe { t: 2, frame }).unwrap();
    keyframe["new_field"] = serde_json::json!(1);
    file.extend_from_slice(&serde_json::to_vec(&keyframe).unwrap());
    file.extend_from_slice(b"\n{\"kind\":\"meta\",\"t\":3,\"event\":\"weather\"}\n");
    let replayed = replay_bytes(&file);
    assert_eq!(replayed.frames.len(), 1);
    assert_eq!(replayed.meta, [RecordingMeta::Unknown]);

    let mut newer = header(2, 1);
    newer.version = RECORDING_VERSION + 1;
    let mut file = serde_json::to_vec(&newer).unwrap();
    file.push(b'\n');
    assert!(Replay::new(BufReader::new(file.as_slice())).is_err());
    assert!(Replay::new(BufReader::new(&b"{\"format\":\"other\"}\n"[..])).is_err());
}

#[test]
fn export_writes_frames_with_their_real_durations_and_a_cast() {
    let (dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for (t, text) in [(0, "a"), (250, "b"), (1_000, "c")] {
        screen.process_bytes(text.as_bytes());
        push(&recorder, t, &screen);
    }
    finish(recorder, 1_500, EndReason::Stopped);

    let frames = dir.path().join("frames");
    let summary = export::png_frames(export::open(&path).unwrap(), &frames, 1, false).unwrap();
    assert_eq!(summary.frames, 3);
    assert_eq!(summary.duration_ms, 1_500);
    let listing = std::fs::read_to_string(frames.join(export::CONCAT_LISTING)).unwrap();
    assert_eq!(
        listing,
        "ffconcat version 1.0\n\
         file 'frame-000001.png'\nduration 0.250\n\
         file 'frame-000002.png'\nduration 0.750\n\
         file 'frame-000003.png'\nduration 0.500\n\
         file 'frame-000003.png'\n"
    );
    let png = std::fs::read(frames.join("frame-000003.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // Frames are never silently replaced.
    assert!(export::png_frames(export::open(&path).unwrap(), &frames, 1, false).is_err());
    assert!(export::png_frames(export::open(&path).unwrap(), &frames, 1, true).is_ok());

    let cast_path = dir.path().join("out.cast");
    let summary = export::cast(export::open(&path).unwrap(), &cast_path, false).unwrap();
    assert_eq!(summary.frames, 3);
    let cast = std::fs::read_to_string(&cast_path).unwrap();
    let mut lines = cast.lines();
    let head: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(
        (head["version"].as_u64(), head["width"].as_u64()),
        (Some(2), Some(20))
    );
    let times: Vec<f64> = lines
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()[0]
                .as_f64()
                .unwrap()
        })
        .collect();
    assert_eq!(times, [0.0, 0.25, 1.0, 1.5]);
}

#[test]
fn a_recorded_frame_draws_back_into_the_same_cells() {
    let mut screen = TerminalScreen::new(3, 12, 100);
    screen.process_bytes(
        "\x1b[1;31mred\x1b[0m 中 \x1b[4:3mx\x1b[0m\r\n\x1b[44m   \x1b[0m".as_bytes(),
    );
    let captured = screen.capture_frame();
    let span = crate::pane::spans::span_frame(&captured, screen.palette(), false).unwrap();
    let rebuilt = frame::captured_frame(&span, &Default::default());
    assert_eq!(rebuilt.plain_text(), captured.plain_text());
    assert_eq!(rebuilt.to_ansi_text(), captured.to_ansi_text());
}

#[test]
fn a_small_change_inside_a_row_writes_only_the_columns_that_changed() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 60, 100);
    screen.process_bytes(
        "CPU \x1b[32m######\x1b[0m   5% 中 load \x1b[1m0.25\x1b[0m trailing text".as_bytes(),
    );
    push(&recorder, 0, &screen);
    screen.process_bytes(b"\x1b[1;15H7");
    push(&recorder, 10, &screen);
    finish(recorder, 20, EndReason::Stopped);

    let delta = events(&path)
        .into_iter()
        .find_map(|event| match event {
            RecordingEvent::Delta(delta) => Some(delta),
            _ => None,
        })
        .expect("a delta");
    assert_eq!(delta.rows.len(), 1);
    let change = &delta.rows[0];
    assert!(change.partial, "{change:?}");
    let covered: u16 = change.runs.iter().map(|run| run.width).sum();
    assert!(
        covered <= 2,
        "a one-digit change covers {covered} columns: {change:?}"
    );
    assert_eq!(replay(&path).frames.last().unwrap().1, span(&mut screen));
}

#[test]
fn random_screens_replay_exactly() {
    // Straight through the encoder rather than a writer thread, whose queue would rightly coalesce
    // a burst this fast on a slow machine.
    let mut encoder = encode::FrameEncoder::default();
    let mut file = serde_json::to_vec(&header(40, 12)).unwrap();
    file.push(b'\n');
    let mut screen = TerminalScreen::new(12, 40, 100);
    let mut expected: Vec<(u64, SpanFrame)> = Vec::new();
    let mut seed: u64 = 0x5eed;
    let mut next = |bound: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % bound
    };
    let pieces = ["ab", "中文", "é", "  ", "│", "x", "日本語", "▀▄", "ok"];
    for t in 0..400u64 {
        let mut chunk = String::new();
        for _ in 0..next(6) + 1 {
            match next(7) {
                0 => chunk.push_str(&format!("\x1b[{};{}H", next(12) + 1, next(40) + 1)),
                1 => chunk.push_str(&format!(
                    "\x1b[{}m",
                    [0, 1, 4, 7, 31, 42, 93, 2][next(8) as usize]
                )),
                2 => chunk.push_str(&format!(
                    "\x1b[38;2;{};{};{}m",
                    next(256),
                    next(256),
                    next(256)
                )),
                3 => chunk.push_str("\x1b[K"),
                4 => chunk.push_str("\r\n"),
                _ => chunk.push_str(pieces[next(pieces.len() as u64) as usize]),
            }
        }
        screen.process_bytes(chunk.as_bytes());
        let frame = span(&mut screen);
        for event in encoder.encode(t * 7, frame.clone()) {
            serde_json::to_writer(&mut file, &event).unwrap();
            file.push(b'\n');
        }
        if expected.last().is_none_or(|(_, last)| *last != frame) {
            expected.push((t * 7, frame));
        }
    }
    assert_eq!(replay_bytes(&file).frames, expected);
    let events: Vec<RecordingEvent> = file
        .split(|&byte| byte == b'\n')
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    let partial = events
        .iter()
        .filter_map(|event| match event {
            RecordingEvent::Delta(delta) => {
                Some(delta.rows.iter().filter(|row| row.partial).count())
            }
            _ => None,
        })
        .sum::<usize>();
    assert!(partial > 0, "no partial rows in {} frames", expected.len());
}

/// Every event's time, in file order.
fn times(path: &Path) -> Vec<(u64, &'static str)> {
    events(path)
        .iter()
        .filter_map(|event| {
            let kind = match event {
                RecordingEvent::Keyframe { .. } | RecordingEvent::Delta(_) => "frame",
                RecordingEvent::Mark { .. } => "mark",
                RecordingEvent::Meta { .. } => "meta",
                RecordingEvent::End(_) => "end",
                _ => return None,
            };
            Some((event.time()?, kind))
        })
        .collect()
}

#[test]
fn a_frame_that_arrives_behind_a_mark_never_moves_ahead_of_it() {
    let (_dir, path) = scratch();
    let mut recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for n in 0..writer::QUEUE_FRAMES as u64 {
        screen.process_bytes(format!("\x1b[1;1Hframe {n}").as_bytes());
        assert!(push(&recorder, n * 10, &screen));
    }
    let at_mark = span(&mut screen);
    assert!(recorder.mark(85, "here".to_string()));
    screen.process_bytes(b"\x1b[1;1Hlater");
    assert!(push(&recorder, 90, &screen));
    assert_eq!(recorder.totals().dropped, 1);
    recorder.resume();
    finish(recorder, 100, EndReason::Stopped);

    let times = times(&path);
    assert!(
        times.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "time runs backwards: {times:?}"
    );
    let mark = times.iter().position(|(_, kind)| *kind == "mark").unwrap();
    assert_eq!(
        times[mark - 1],
        (70, "frame"),
        "the screen the mark was made on"
    );
    assert_eq!(times[mark + 1], (90, "frame"));
    let replayed = replay(&path);
    assert_eq!(
        replayed.frames.iter().find(|(t, _)| *t == 70).unwrap().1,
        at_mark
    );
}

#[test]
fn the_last_frame_is_queued_past_a_full_queue_once_and_never_again() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for n in 0..writer::QUEUE_FRAMES as u64 {
        screen.process_bytes(format!("\x1b[1;1Hframe {n}").as_bytes());
        assert!(push(&recorder, n * 10, &screen));
        assert!(recorder.mark(n * 10 + 5, format!("after {n}")));
    }
    screen.process_bytes(b"\x1b[1;1Hlast");
    assert!(!push(&recorder, 100, &screen));
    assert!(recorder.push_last_frame(100, screen.capture_frame(), screen.palette()));
    assert_eq!(recorder.totals().dropped, 0);
    // A second one takes the first's place at the end rather than growing the queue again.
    screen.process_bytes(b"\x1b[1;1Hlater");
    assert!(recorder.push_last_frame(110, screen.capture_frame(), screen.palette()));
    assert_eq!(recorder.totals().dropped, 1);
    let last = span(&mut screen);
    let mut recorder = recorder;
    recorder.resume();
    finish(recorder, 200, EndReason::Stopped);
    let replayed = replay(&path);
    assert_eq!(replayed.frames.len(), writer::QUEUE_FRAMES + 1);
    assert_eq!(replayed.frames.last().unwrap(), &(110, last));
}

#[test]
fn a_frame_and_the_events_on_it_are_queued_together_or_not_at_all() {
    let (_dir, path) = scratch();
    let mut recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for n in 0..writer::QUEUE_FRAMES as u64 {
        screen.process_bytes(format!("\x1b[1;1Hframe {n}").as_bytes());
        assert!(push(&recorder, n * 10, &screen));
        assert!(recorder.mark(n * 10 + 5, format!("after {n}")));
    }
    screen.process_bytes(b"\x1b[1;1Hrefused");
    let mark = |t| {
        vec![RecordingEvent::Mark {
            t,
            label: "on it".to_string(),
        }]
    };
    assert!(!recorder.push_frame_with(
        100,
        screen.capture_frame(),
        screen.palette(),
        mark(100),
        false
    ));
    assert!(recorder.push_frame_with(
        110,
        screen.capture_frame(),
        screen.palette(),
        mark(110),
        true
    ));
    recorder.resume();
    finish(recorder, 200, EndReason::Stopped);

    let times = times(&path);
    assert_eq!(
        times[times.len() - 4..],
        [(75, "mark"), (110, "frame"), (110, "mark"), (200, "end")],
        "the refused frame left no event behind: {times:?}"
    );
}

#[test]
fn the_last_frame_keeps_its_events_past_a_full_event_queue() {
    let (_dir, path) = scratch();
    let mut recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    assert!(push(&recorder, 0, &screen));
    for n in 0..writer::QUEUE_EVENTS as u64 {
        assert!(recorder.mark(n / 100, format!("mark {n}")));
    }
    screen.process_bytes(b"\x1b[1;1Hsettings");
    let meta = || {
        vec![RecordingEvent::Meta {
            t: 50,
            meta: RecordingMeta::Overlay {
                overlay: Some("settings".to_string()),
            },
        }]
    };
    assert!(
        !recorder.push_frame_with(50, screen.capture_frame(), screen.palette(), meta(), false),
        "a frame whose events do not fit waits"
    );
    assert!(
        recorder.push_frame_with(50, screen.capture_frame(), screen.palette(), meta(), true),
        "the last frame takes its events with it"
    );
    recorder.resume();
    finish(recorder, 60, EndReason::Stopped);

    let times = times(&path);
    assert_eq!(
        times[times.len() - 3..],
        [(50, "frame"), (50, "meta"), (60, "end")],
        "{:?}",
        &times[times.len() - 5..]
    );
}

#[test]
fn a_queue_of_frames_each_before_an_event_refuses_another_rather_than_growing() {
    let (_dir, path) = scratch();
    let mut recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    for n in 0..writer::QUEUE_FRAMES as u64 {
        screen.process_bytes(format!("\x1b[1;1Hframe {n}").as_bytes());
        assert!(push(&recorder, n * 10, &screen));
        assert!(recorder.mark(n * 10 + 5, format!("after {n}")));
    }
    screen.process_bytes(b"\x1b[1;1Hrefused");
    assert!(!push(&recorder, 100, &screen), "no frame can give way");
    assert_eq!(recorder.totals().dropped, 0, "nothing was lost");
    recorder.resume();
    // Once the writer catches up the same change is taken.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !push(&recorder, 110, &screen) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    finish(recorder, 200, EndReason::Stopped);
    let replayed = replay(&path);
    assert_eq!(replayed.frames.len(), writer::QUEUE_FRAMES + 1);
    assert_eq!(replayed.marks.len(), writer::QUEUE_FRAMES);
}

#[test]
fn a_recording_that_is_ending_takes_no_mark() {
    let (_dir, path) = scratch();
    let recorder = Recorder::start_paused(options(&path, u64::MAX)).unwrap();
    recorder.finish(10, EndReason::Stopped);
    assert!(!recorder.mark(20, "late".to_string()));
}

#[test]
fn a_reader_refuses_frames_newer_than_it_reads() {
    let mut newer = header(2, 1);
    newer.spans_version = SPAN_FRAME_VERSION + 1;
    let mut file = serde_json::to_vec(&newer).unwrap();
    file.push(b'\n');
    let refused = Replay::new(BufReader::new(file.as_slice()))
        .err()
        .expect("refused");
    assert!(refused.contains("rozi-spans"), "{refused}");
}

#[test]
fn a_cast_resizes_its_terminal_with_the_pane_and_keeps_marks() {
    let (dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut screen = TerminalScreen::new(4, 20, 100);
    screen.process_bytes(b"small");
    assert!(push(&recorder, 0, &screen));
    assert!(recorder.mark(50, "before growing".to_string()));
    screen.resize(6, 30);
    screen.process_bytes(b"\x1b[6;25Hcorner");
    assert!(push(&recorder, 100, &screen));
    screen.resize(3, 10);
    assert!(push(&recorder, 200, &screen));
    finish(recorder, 300, EndReason::Stopped);

    let cast_path = dir.path().join("resized.cast");
    export::cast(export::open(&path).unwrap(), &cast_path, false).unwrap();
    let cast = std::fs::read_to_string(&cast_path).unwrap();
    let mut lines = cast.lines();
    let head: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(
        (head["width"].as_u64(), head["height"].as_u64()),
        (Some(20), Some(4)),
        "the size the recording started at"
    );
    let events: Vec<(f64, String, String)> = lines
        .map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            (
                event[0].as_f64().unwrap(),
                event[1].as_str().unwrap().to_string(),
                event[2].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let kinds: Vec<(f64, &str)> = events
        .iter()
        .map(|(t, kind, _)| (*t, kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (0.0, "o"),
            (0.05, "m"),
            (0.1, "r"),
            (0.1, "o"),
            (0.2, "r"),
            (0.2, "o"),
            (0.3, "o")
        ]
    );
    assert_eq!(events[1].2, "before growing");
    assert_eq!(events[2].2, "30x6");
    assert!(events[3].2.contains("corner"), "{:?}", events[3].2);
    assert_eq!(events[4].2, "10x3");
}
