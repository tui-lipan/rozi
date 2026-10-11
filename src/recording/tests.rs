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
            assert!(replay.frame().unwrap().images[0].fill_cell_box);
            let id = replay.frame().unwrap().images[0].id.clone().unwrap();
            assert_eq!(id, stored[0].id);
            let decoded = &replay.frame_images().unwrap()[&id];
            assert_eq!((decoded.width, decoded.height), (20, 20));
            assert_eq!(&decoded.rgba[..4], &[255, 0, 0, 255]);
            let recorded = replay.frame().unwrap().clone();
            let captured = frame::captured_frame(&recorded, replay.frame_images().unwrap());
            assert!(captured.images[0].fill_cell_box);
        }
    }
    assert_eq!(frames, 5);
}

#[test]
fn negative_image_planes_round_trip_beneath_recorded_glyphs() {
    image_planes_round_trip(-1, 255);
}

#[test]
fn translucent_image_planes_round_trip_over_recorded_glyphs() {
    image_planes_round_trip(1, 160);
}

#[test]
fn opaque_image_planes_round_trip_without_disclosing_occluded_text() {
    image_planes_round_trip(1, 255);
}

fn image_planes_round_trip(z_index: i32, alpha: u8) {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut expected = Vec::new();
    let opaque = z_index >= 0 && alpha == 255;
    let texts = if opaque {
        ["SECR", "HIDE"]
    } else {
        ["A界", "B界"]
    };
    for (t, text) in [(0, texts[0]), (10, texts[1])] {
        let mut screen = TerminalScreen::new(4, 20, 100);
        screen.set_cell_size(TerminalCellSize {
            width: 8,
            height: 16,
        });
        screen.process_bytes(format!("\x1b[?25l\x1b[31;44;1;4m{text}\x1b[1;1H").as_bytes());
        if opaque {
            screen.process_bytes(format!("\x1b[2;1H{t}\x1b[1;1H").as_bytes());
        }
        let pixels = [20, 100, 220, alpha].repeat(32 * 16);
        screen.process_bytes(
            format!(
                "\x1b_Ga=T,f=32,s=32,v=16,t=d,i=1,c=4,r=1,z={z_index},C=1;{}\x1b\\",
                base64::engine::general_purpose::STANDARD.encode(pixels)
            )
            .as_bytes(),
        );
        let captured = screen.capture_frame();
        assert_eq!(captured.images[0].z_index, z_index);
        assert!(
            captured.images[0]
                .underlying_cells
                .iter()
                .flatten()
                .any(|cell| cell.symbol.starts_with(text.chars().next().unwrap()))
        );
        if opaque {
            let spans = crate::pane::spans::span_frame(&captured, screen.palette(), true).unwrap();
            assert_eq!(spans.version, 2);
            assert!(spans.images[0].underlying_cells.is_empty());
            assert!(!serde_json::to_string(&spans).unwrap().contains(text));
            assert!(spans.rows[0].iter().all(|run| !run.text.contains(text)));
        }
        assert!(recorder.push_frame(t, captured.clone(), screen.palette()));
        expected.push((captured, screen.palette()));
    }
    let outcome = finish(recorder, 20, EndReason::Stopped);
    assert_eq!(
        outcome.totals.images, 1,
        "only metadata changes between frames"
    );
    assert_eq!(
        outcome.totals.deltas, 1,
        "layer metadata survives deltas too"
    );
    let bytes = std::fs::read(&path).unwrap();
    if opaque {
        let stored = String::from_utf8(bytes.clone()).unwrap();
        let header: RecordingHeader = serde_json::from_str(stored.lines().next().unwrap()).unwrap();
        assert_eq!(
            header.spans_version, 2,
            "old players must refuse these frames"
        );
        for text in texts {
            assert!(
                !stored.contains(text),
                "occluded text leaked into the recording"
            );
        }
    }
    let mut replay = Replay::new(BufReader::new(bytes.as_slice())).unwrap();
    let mut frames = 0;
    while let Some(step) = replay.step().unwrap() {
        if !matches!(step, ReplayStep::Frame { .. }) {
            continue;
        }
        let recorded = replay.frame().unwrap().clone();
        let reconstructed = frame::captured_frame(&recorded, replay.frame_images().unwrap());
        let (original, palette) = &expected[frames];
        assert_same_image_png(original, &reconstructed, *palette);
        frames += 1;
    }
    assert_eq!(frames, expected.len());
}

fn assert_same_image_png(
    original: &tui_lipan::CapturedFrame,
    reconstructed: &tui_lipan::CapturedFrame,
    palette: TerminalColorPalette,
) {
    for text_renderer in [
        tui_lipan::PngTextRenderer::Bitmap,
        tui_lipan::PngTextRenderer::Font,
    ] {
        let options = tui_lipan::PngOptions {
            default_fg: palette.foreground.unwrap_or(Color::White),
            default_bg: palette.background.unwrap_or(Color::Black),
            ansi_palette: palette.ansi,
            text_renderer,
            font_family: Some("JetBrainsMono Nerd Font".into()),
            ..Default::default()
        };
        assert!(
            original.to_png(&options).unwrap() == reconstructed.to_png(&options).unwrap(),
            "PNG pixels changed through capture/spans/recording/replay for renderer={text_renderer:?}"
        );
    }
}

#[test]
fn opaque_base_and_translucent_patch_hide_original_text() {
    stacked_images_round_trip(0, 6, "SECRET", true, false);
}

#[test]
fn image_stacks_hide_both_halves_of_a_fully_occluded_wide_glyph() {
    stacked_images_round_trip(0, 2, "界", true, false);
}

#[test]
fn image_stacks_preserve_the_visible_half_of_a_wide_glyph() {
    stacked_images_round_trip(0, 1, "界", false, false);
}

#[test]
fn opaque_negative_base_preserves_text_beneath_a_translucent_patch() {
    stacked_images_round_trip(-1, 6, "SECRET", false, false);
}

#[test]
fn image_stacks_preserve_wide_glyphs_across_tile_boundaries() {
    stacked_images_round_trip(0, 1, "界", false, true);
}

#[test]
fn an_image_over_a_wide_leader_keeps_the_image_free_continuation() {
    image_underlay_edge_round_trip("界", None, true);
}

#[test]
fn an_image_keeps_the_glyph_redrawn_by_a_visible_block_cursor() {
    image_underlay_edge_round_trip("S", Some(tui_lipan::CursorShape::Block), true);
}

#[test]
fn an_image_keeps_a_private_use_icon_spilling_into_an_image_free_blank() {
    image_underlay_edge_round_trip("\u{f05b2}", None, true);
}

#[test]
fn non_block_cursors_keep_styles_without_disclosing_occluded_text() {
    for shape in [
        tui_lipan::CursorShape::HollowBlock,
        tui_lipan::CursorShape::Underline,
        tui_lipan::CursorShape::Bar,
    ] {
        image_underlay_edge_round_trip("S", Some(shape), false);
    }
}

fn image_underlay_edge_round_trip(
    text: &str,
    cursor: Option<tui_lipan::CursorShape>,
    text_visible: bool,
) {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut expected = Vec::new();
    for t in [0, 10] {
        let mut screen = TerminalScreen::new(4, 20, 100);
        screen.set_cell_size(TerminalCellSize {
            width: 8,
            height: 16,
        });
        screen
            .process_bytes(format!("\x1b[?25l\x1b[31;44m{text} \x1b[2;1H{t}\x1b[1;1H").as_bytes());
        screen.process_bytes(
            format!(
                "\x1b_Ga=T,f=32,s=8,v=16,t=d,i=1,c=1,r=1,z=1,C=1;{}\x1b\\",
                base64::engine::general_purpose::STANDARD
                    .encode([20, 100, 220, 255].repeat(8 * 16)),
            )
            .as_bytes(),
        );
        let mut captured = screen.capture_frame();
        captured.cursor = cursor.map(|shape| tui_lipan::CursorState::new(0, 0).shape(shape));
        assert_eq!(
            captured.images[0].area.w, 1,
            "the next cell has no image metadata"
        );
        let spans = crate::pane::spans::span_frame(&captured, screen.palette(), true).unwrap();
        assert_stack_text_visibility(&serde_json::to_string(&spans).unwrap(), text, !text_visible);
        assert!(recorder.push_frame(t, captured.clone(), screen.palette()));
        expected.push((captured, screen.palette()));
    }
    finish(recorder, 20, EndReason::Stopped);
    let bytes = std::fs::read(path).unwrap();
    assert_stack_text_visibility(std::str::from_utf8(&bytes).unwrap(), text, !text_visible);
    let mut replay = Replay::new(BufReader::new(bytes.as_slice())).unwrap();
    let mut frames = 0;
    while let Some(step) = replay.step().unwrap() {
        if !matches!(step, ReplayStep::Frame { .. }) {
            continue;
        }
        let recorded = replay.frame().unwrap().clone();
        let reconstructed = frame::captured_frame(&recorded, replay.frame_images().unwrap());
        let (original, palette) = &expected[frames];
        assert_same_image_png(original, &reconstructed, *palette);
        frames += 1;
    }
    assert_eq!(frames, expected.len());
}

fn stacked_images_round_trip(
    base_z: i32,
    opaque_columns: usize,
    text: &str,
    hidden: bool,
    split_wide: bool,
) {
    let (_dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut expected = Vec::new();
    for t in [0, 10] {
        let mut screen = TerminalScreen::new(4, 20, 100);
        screen.set_cell_size(TerminalCellSize {
            width: 8,
            height: 16,
        });
        screen.process_bytes(
            format!("\x1b[?25l\x1b[31;44;1;4m{text}\x1b[2;1H{t}\x1b[1;1H").as_bytes(),
        );
        let width = if split_wide { 8 } else { 48 };
        let columns = width / 8;
        let base: Vec<u8> = (0..width * 16)
            .flat_map(|pixel| {
                [
                    20,
                    100,
                    220,
                    if pixel % width < opaque_columns * 8 {
                        255
                    } else {
                        0
                    },
                ]
            })
            .collect();
        for (id, z, pixels) in [
            (1, base_z, base),
            (2, 1, [240, 40, 80, 160].repeat(width * 16)),
        ] {
            if split_wide && id == 2 {
                screen.process_bytes(b"\x1b[1;2H");
            }
            screen.process_bytes(
                format!(
                    "\x1b_Ga=T,f=32,s={width},v=16,t=d,i={id},c={columns},r=1,z={z},C=1;{}\x1b\\",
                    base64::engine::general_purpose::STANDARD.encode(pixels),
                )
                .as_bytes(),
            );
        }
        let captured = screen.capture_frame();
        assert_eq!(captured.images.len(), 2, "exercise separate z-planes");
        let spans = crate::pane::spans::span_frame(&captured, screen.palette(), true).unwrap();
        assert_stack_text_visibility(&serde_json::to_string(&spans).unwrap(), text, hidden);
        assert!(recorder.push_frame(t, captured.clone(), screen.palette()));
        expected.push((captured, screen.palette()));
    }
    let outcome = finish(recorder, 20, EndReason::Stopped);
    assert_eq!(outcome.totals.images, 2);
    assert_eq!(outcome.totals.deltas, 1);
    let bytes = std::fs::read(path).unwrap();
    assert_stack_text_visibility(std::str::from_utf8(&bytes).unwrap(), text, hidden);
    let mut replay = Replay::new(BufReader::new(bytes.as_slice())).unwrap();
    let mut frames = 0;
    while let Some(step) = replay.step().unwrap() {
        if !matches!(step, ReplayStep::Frame { .. }) {
            continue;
        }
        let recorded = replay.frame().unwrap().clone();
        let reconstructed = frame::captured_frame(&recorded, replay.frame_images().unwrap());
        let (original, palette) = &expected[frames];
        assert_same_image_png(original, &reconstructed, *palette);
        frames += 1;
    }
    assert_eq!(frames, expected.len());
}

fn assert_stack_text_visibility(serialized: &str, text: &str, hidden: bool) {
    let captured_text: String = serialized
        .lines()
        .map(|line| serialized_cell_text(&serde_json::from_str(line).unwrap()))
        .collect();
    assert_eq!(
        captured_text
            .chars()
            .any(|character| text.contains(character)),
        !hidden,
        "original characters have the wrong visibility in serialized cell text"
    );
}

fn serialized_cell_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                if key == "text" {
                    value.as_str().unwrap_or_default().to_string()
                } else {
                    serialized_cell_text(value)
                }
            })
            .collect(),
        serde_json::Value::Array(values) => values.iter().map(serialized_cell_text).collect(),
        _ => String::new(),
    }
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
         file 'frame-000001.png'\noption framerate 1000\nduration 0.250\n\
         file 'frame-000002.png'\noption framerate 1000\nduration 0.750\n\
         file 'frame-000003.png'\noption framerate 1000\nduration 0.500\n\
         file 'frame-000003.png'\noption framerate 1000\n"
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

/// Step through `events` after a valid header, and say why reading stopped.
fn refusal(events: &[serde_json::Value]) -> String {
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    file.push(b'\n');
    for event in events {
        file.extend_from_slice(&serde_json::to_vec(event).unwrap());
        file.push(b'\n');
    }
    let mut replay = Replay::new(BufReader::new(file.as_slice())).unwrap();
    loop {
        match replay.step() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("read to the end"),
            Err(error) => return error,
        }
    }
}

#[test]
fn a_reader_refuses_image_metadata_outside_its_cell_area() {
    let mut screen = TerminalScreen::new(1, 2, 0);
    screen.process_bytes(b"\x1b_Ga=T,f=32,s=1,v=1,t=d,i=1,C=1;AAAA/w==\x1b\\");
    let mut frame = span(&mut screen);
    let image = &mut frame.images[0];
    assert!(!image.underlying_cells.is_empty());
    image.underlying_cells.push(None);
    let error = refusal(&[serde_json::to_value(RecordingEvent::Keyframe { t: 0, frame }).unwrap()]);
    assert!(error.contains("underlying cells"), "{error}");
}

#[test]
fn a_reader_refuses_a_line_frame_or_image_too_large_to_hold() {
    // A line past the ceiling is refused before it is parsed, or even held whole.
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    file.push(b'\n');
    let endless = std::io::Read::chain(
        file.as_slice(),
        std::io::Read::take(std::io::repeat(b' '), read::MAX_EVENT_LINE as u64 + 2),
    );
    let mut reader = RecordingReader::new(BufReader::new(endless)).unwrap();
    let refused = reader.next_event().unwrap_err();
    assert!(refused.contains("longer than"), "{refused}");

    let mut frame = span(&mut TerminalScreen::new(1, 2, 0));
    (frame.width, frame.height) = (2048, 1024);
    let refused =
        refusal(&[serde_json::to_value(RecordingEvent::Keyframe { t: 0, frame }).unwrap()]);
    assert!(refused.contains("2048x1024 cells"), "{refused}");

    let refused = refusal(&[serde_json::json!({
        "kind": "image",
        "t": 0,
        "id": "huge",
        "pixel_width": 16384,
        "pixel_height": 16384,
        "png_base64": "",
    })]);
    assert!(refused.contains("16384x16384 pixels"), "{refused}");

    let mut file = serde_json::to_vec(&header(4096, 4096)).unwrap();
    file.push(b'\n');
    assert!(Replay::new(BufReader::new(file.as_slice())).is_err());
}

#[test]
fn images_with_no_pixels_still_count_against_the_image_budget() {
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    file.push(b'\n');
    for n in 0..5_000 {
        let image = RecordingEvent::Image(format::RecordedImage {
            t: 0,
            id: format!("unique-id-{n}"),
            pixel_width: 1,
            pixel_height: 1,
            png_base64: String::new(),
        });
        file.extend_from_slice(&serde_json::to_vec(&image).unwrap());
        file.push(b'\n');
    }
    let mut replay = Replay::new(BufReader::new(file.as_slice()))
        .unwrap()
        .with_image_budget(16 * 1024);
    assert_eq!(replay.step(), Ok(None));
    let retained = replay.retained_images();
    assert!((1..100).contains(&retained), "{retained} images kept");

    let image = |id: String, side: u32| {
        serde_json::json!({
            "kind": "image",
            "t": 0,
            "id": id,
            "pixel_width": side,
            "pixel_height": side,
            "png_base64": "",
        })
    };
    let refused = refusal(&[image("x".repeat(65), 1)]);
    assert!(refused.contains("image id"), "{refused}");
    // Wide enough that the product wraps a u64.
    let refused = refusal(&[image("wraps".into(), u32::MAX)]);
    assert!(refused.contains("larger than rozi reads"), "{refused}");
}

/// A `side`×`side` PNG of one color with `channels` bytes a pixel: tiny compressed.
fn flat_png_bytes(side: u32, color: png::ColorType, channels: u32) -> Vec<u8> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, side, side);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![7; (side * side * channels) as usize])
        .unwrap();
    writer.finish().unwrap();
    png
}

/// A `side`×`side` RGBA PNG of one color, as base64: tiny compressed, `side² × 4` bytes decoded.
fn flat_png(side: u32) -> String {
    base64::engine::general_purpose::STANDARD.encode(flat_png_bytes(side, png::ColorType::Rgba, 4))
}

#[test]
fn an_image_is_measured_as_the_rgba_it_becomes() {
    // 12 KiB as RGB, 16 KiB once it gains alpha.
    let rgb = flat_png_bytes(64, png::ColorType::Rgb, 3);
    let refused = frame::DecodedImage::from_png(&rgb, 14 * 1024).unwrap_err();
    assert!(refused.contains("64x64"), "{refused}");
    let decoded = frame::DecodedImage::from_png(&rgb, 16 * 1024).unwrap();
    assert_eq!(decoded.rgba.len(), 16 * 1024);
}

#[test]
fn the_images_of_one_frame_decode_within_one_budget() {
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    file.push(b'\n');
    let ids = ["a", "b", "c", "d"];
    let pixels = flat_png(64);
    for id in ids {
        let image = RecordingEvent::Image(format::RecordedImage {
            t: 0,
            id: id.to_string(),
            pixel_width: 64,
            pixel_height: 64,
            png_base64: pixels.clone(),
        });
        file.extend_from_slice(&serde_json::to_vec(&image).unwrap());
        file.push(b'\n');
    }
    // Every image on the same cell.
    let mut frame = span(&mut TerminalScreen::new(1, 2, 0));
    frame.images = ids
        .map(|id| crate::control::SpanImage {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            pixel_width: 64,
            pixel_height: 64,
            fill_cell_box: false,
            z_index: 0,
            underlying_cells: Vec::new(),
            visible: None,
            png_base64: None,
            id: Some(id.to_string()),
        })
        .to_vec();
    file.extend_from_slice(&serde_json::to_vec(&RecordingEvent::Keyframe { t: 0, frame }).unwrap());
    file.push(b'\n');

    // Each decodes to 16 KiB; the budget holds two and a half.
    let mut replay = Replay::new(BufReader::new(file.as_slice()))
        .unwrap()
        .with_decoded_budget(40 * 1024);
    assert!(matches!(replay.step(), Ok(Some(ReplayStep::Frame { .. }))));
    let decoded = replay.frame_images().unwrap();
    assert_eq!(decoded.len(), 2, "the rest are left to their half-blocks");
    assert!(
        decoded
            .values()
            .map(|image| image.rgba.len())
            .sum::<usize>()
            <= 40 * 1024
    );
}

#[test]
fn a_replay_past_its_image_budget_forgets_the_oldest_image() {
    let mut file = serde_json::to_vec(&header(2, 1)).unwrap();
    file.push(b'\n');
    for id in ["old", "new"] {
        let image = RecordingEvent::Image(format::RecordedImage {
            t: 0,
            id: id.to_string(),
            pixel_width: 1,
            pixel_height: 1,
            png_base64: "A".repeat(600),
        });
        file.extend_from_slice(&serde_json::to_vec(&image).unwrap());
        file.push(b'\n');
    }
    let mut frame = span(&mut TerminalScreen::new(1, 2, 0));
    frame.images = ["old", "new"]
        .map(|id| crate::control::SpanImage {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            pixel_width: 1,
            pixel_height: 1,
            fill_cell_box: false,
            z_index: 0,
            underlying_cells: Vec::new(),
            visible: None,
            png_base64: None,
            id: Some(id.to_string()),
        })
        .to_vec();
    file.extend_from_slice(&serde_json::to_vec(&RecordingEvent::Keyframe { t: 0, frame }).unwrap());
    file.push(b'\n');

    let mut replay = Replay::new(BufReader::new(file.as_slice()))
        .unwrap()
        .with_image_budget(1000);
    assert!(matches!(replay.step(), Ok(Some(ReplayStep::Frame { .. }))));
    // The forgotten image is skipped; the kept one is decoded, and its garbage refused.
    let refused = replay.frame_images().unwrap_err();
    assert!(refused.starts_with("image new:"), "{refused}");
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

#[test]
fn video_export_keeps_resized_frames_on_one_canvas_without_changing_timing() {
    let (dir, path) = scratch();
    let recorder = Recorder::start(options(&path, u64::MAX)).unwrap();
    let mut small = TerminalScreen::new(2, 4, 0);
    small.process_bytes("\x1b[41mA\x1b[0m\x1b[1;4H\u{e0b0}\x1b[2;4H\u{f17c}".as_bytes());
    push(&recorder, 0, &small);
    let mut wide = TerminalScreen::new(1, 8, 0);
    wide.process_bytes(b"B");
    push(&recorder, 250, &wide);
    let mut tall = TerminalScreen::new(4, 2, 0);
    tall.process_bytes(b"C");
    push(&recorder, 900, &tall);
    finish(recorder, 1_500, EndReason::Stopped);
    let native = dir.path().join("native");
    let fixed = dir.path().join("fixed");
    export::png_frames(export::open(&path).unwrap(), &native, 1, false).unwrap();
    let summary = export::video_frames(&path, &fixed, 1, false).unwrap();
    assert_eq!(summary.frames, 3);
    assert_eq!(summary.duration_ms, 1_500);
    assert_eq!(
        std::fs::read(native.join(export::CONCAT_LISTING)).unwrap(),
        std::fs::read(fixed.join(export::CONCAT_LISTING)).unwrap()
    );
    let mut sizes = Vec::new();
    for index in 1..=3 {
        let bytes = std::fs::read(fixed.join(format!("frame-{index:06}.png"))).unwrap();
        let reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        sizes.push(reader.info().size());
    }
    assert!(sizes.iter().all(|size| *size == sizes[0]));
    let native_bytes = std::fs::read(native.join("frame-000001.png")).unwrap();
    let fixed_bytes = std::fs::read(fixed.join("frame-000001.png")).unwrap();
    let native_image = frame::DecodedImage::from_png(&native_bytes, 1 << 20).unwrap();
    let fixed_image = frame::DecodedImage::from_png(&fixed_bytes, 1 << 20).unwrap();
    assert!(fixed_image.width > native_image.width);
    assert!(fixed_image.height > native_image.height);
    for y in 0..native_image.height as usize {
        let source = y * native_image.width as usize * 4;
        let target = y * fixed_image.width as usize * 4;
        assert_eq!(
            &native_image.rgba[source..source + native_image.width as usize * 4],
            &fixed_image.rgba[target..target + native_image.width as usize * 4]
        );
    }
    // Right-edge private-use glyphs must remain clipped. Every pixel outside the original
    // frame is the default background, including the neighbor an expanded cell grid would add.
    let background = frame::palette(&span(&mut small).palette)
        .background
        .unwrap();
    let Color::Rgb(r, g, b) = background else {
        panic!("RGB palette");
    };
    for y in 0..fixed_image.height as usize {
        for x in 0..fixed_image.width as usize {
            if x >= native_image.width as usize || y >= native_image.height as usize {
                let at = (y * fixed_image.width as usize + x) * 4;
                assert_eq!(&fixed_image.rgba[at..at + 4], &[r, g, b, 255]);
            }
        }
    }
    assert!(export::video_frames(&path, &fixed, 1, false).is_err());
    assert!(export::video_frames(&path, &fixed, 1, true).is_ok());
}

#[test]
fn video_export_rejects_an_oversized_combined_canvas_before_creating_output() {
    let (dir, path) = scratch();
    let mut bytes = serde_json::to_vec(&header(2_000, 1)).unwrap();
    bytes.push(b'\n');
    for (t, width, height) in [(0, 2_000, 1), (100, 1, 2_000)] {
        let mut frame = span(&mut TerminalScreen::new(1, 1, 0));
        frame.width = width;
        frame.height = height;
        frame.rows = vec![Vec::new(); usize::from(height)];
        bytes.extend(serde_json::to_vec(&RecordingEvent::Keyframe { t, frame }).unwrap());
        bytes.push(b'\n');
    }
    std::fs::write(&path, bytes).unwrap();
    let output = dir.path().join("frames");
    assert!(
        export::video_frames(&path, &output, 1, false)
            .unwrap_err()
            .contains("canvas")
    );
    assert!(!output.exists());
}
