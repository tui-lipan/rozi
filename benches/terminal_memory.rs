//! Retained-allocation probe for one populated production client terminal.
//!
//! This is not a timing benchmark. It brackets construction, server-backend binding, and terminal
//! ingest with a counting allocator, then reports allocations still owned by the pane. Adjacent
//! history capacities around Alacritty's 1,000-row growth boundary make accidental spare blocks
//! visible without relying on process RSS or allocator arena release.
//!
//! Run with `cargo bench --bench terminal_memory`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufWriter, Seek, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rozi::state::{Pane, PaneId};
use tui_lipan::prelude::{FloatRect, TerminalScreen};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

fn note_alloc(size: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

fn note_free(size: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    FREES.fetch_add(1, Ordering::Relaxed);
    LIVE.fetch_sub(size, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_free(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy)]
struct Counts {
    allocs: usize,
    frees: usize,
    bytes: usize,
    live: usize,
    peak: usize,
}

fn measure<T>(body: impl FnOnce() -> T) -> (T, Counts) {
    ALLOCS.store(0, Ordering::Relaxed);
    FREES.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::SeqCst);
    let value = body();
    ENABLED.store(false, Ordering::SeqCst);
    let counts = Counts {
        allocs: ALLOCS.load(Ordering::Relaxed),
        frees: FREES.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        live: LIVE.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
    };
    (value, counts)
}

fn corpus(rows: u16, history: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((history + usize::from(rows) + 2) * 32);
    for line in 0..history + usize::from(rows) + 2 {
        bytes.extend_from_slice(
            format!("\x1b[3{}mline-{line:06}\x1b[0m\r\n", line % 7 + 1).as_bytes(),
        );
    }
    bytes
}

fn production_probe(rows: u16, cols: u16, history: usize) -> Counts {
    let input = corpus(rows, history);
    let rect = FloatRect {
        x: 0.0,
        y: 0.0,
        w: f32::from(cols),
        h: f32::from(rows),
    };
    let (pane, counts) = measure(|| {
        let id: PaneId = 1;
        let mut pane = Pane::new(id, history, rect);
        pane.terminal.bind_server_backend(id, 1);
        // Match the real lifecycle: backend binding happens at the fallback size, then layout
        // supplies the authoritative geometry before output starts.
        pane.terminal.apply_server_resize(cols, rows);
        pane.terminal.process_server_output(&input);
        pane
    });
    std::hint::black_box(&pane);
    drop(pane);
    counts
}

fn direct_probe(rows: u16, cols: u16, history: usize) -> Counts {
    let input = corpus(rows, history);
    let (screen, counts) = measure(|| {
        let mut screen = TerminalScreen::new(rows, cols, history);
        screen.process_bytes(&input);
        screen
    });
    std::hint::black_box(&screen);
    drop(screen);
    counts
}

fn replay_screen(rows: u16, cols: u16, history: usize) -> TerminalScreen {
    let mut input = Vec::new();
    for line in 0..history + usize::from(rows) + 2 {
        for col in 0..cols {
            write!(
                &mut input,
                "\x1b[38;5;{}mX",
                (line + usize::from(col)) % 256
            )
            .unwrap();
        }
        input.extend_from_slice(b"\x1b[0m\r\n");
    }
    let mut screen = TerminalScreen::new(rows, cols, history);
    screen.process_bytes(&input);
    screen
}

fn replay_export_probe(rows: u16, cols: u16, history: usize) {
    let mut screen = replay_screen(rows, cols, history);
    let (replay, collected) = measure(|| screen.export_replay_bytes());
    let replay_len = replay.len();
    drop(replay);

    let mut spool = tempfile::tempfile().expect("temporary replay spool");
    let (result, streamed) = measure(|| {
        let mut writer = BufWriter::with_capacity(256 * 1024, &mut spool);
        screen.write_replay_bytes(&mut writer)?;
        writer.flush()
    });
    result.expect("stream replay");
    let streamed_len = spool.stream_position().expect("spool position") as usize;

    println!();
    println!(
        "{:>10}  {:>9}  {:>7}  {:>12}  {:>10}  {:>10}  {:>12}  {:>12}",
        "export", "viewport", "history", "replay_bytes", "allocs", "frees", "live", "peak_live"
    );
    for (export, len, counts) in [
        ("collected", replay_len, collected),
        ("spooled", streamed_len, streamed),
    ] {
        println!(
            "{export:>10}  {cols:>4}x{rows:<4}  {history:>7}  {len:>12}  {:>10}  {:>10}  {:>12}  {:>12}",
            counts.allocs, counts.frees, counts.live, counts.peak
        );
    }
}

fn main() {
    println!(
        "{:>10}  {:>9}  {:>7}  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}",
        "constructor", "viewport", "history", "allocs", "frees", "bytes", "live", "peak_live"
    );
    for (rows, cols) in [(24, 80), (64, 253)] {
        for history in [0, 999, 1_000, 1_001, 4_999, 5_000, 5_001] {
            for (constructor, counts) in [
                ("direct", direct_probe(rows, cols, history)),
                ("production", production_probe(rows, cols, history)),
            ] {
                println!(
                    "{constructor:>10}  {cols:>4}x{rows:<4}  {history:>7}  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}",
                    counts.allocs, counts.frees, counts.bytes, counts.live, counts.peak
                );
            }
        }
    }
    replay_export_probe(64, 253, 5_000);
}
