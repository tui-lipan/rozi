//! Allocation accounting for the view + expand + layout pass.
//!
//! Not a timing benchmark. It runs the same `AppRoot` setup `app_render/view_layout` uses and
//! reports what one `TestBackend::render()` costs in allocations, so a candidate optimization can
//! be judged by what it removes rather than by a noisy microsecond delta. Counts are exact and
//! repeat run to run, which makes this the cheaper regression signal of the two.
//!
//! Run with `cargo bench --bench alloc_probe`.
//!
//! Attribution - which phase of the pass made an allocation, and what the expanded tree is made
//! of - is not wired up here, because it needs a hook inside `tui-lipan`. To get it back, add
//! `alloc-probe = []` to the framework's features behind the probe module described in
//! `docs/performance/audits/2026-09-04.md`, point `.cargo/config.toml` at the local checkout, and
//! read `tui_lipan::alloc_probe::bucket()` from `note_alloc` below.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{Pane, PaneId};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 200,
    h: 60,
};

/// Layout recursion on a deep dwindle tree overflows the default 2MB stack.
const STACK_SIZE: usize = 16 * 1024 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Counts every allocation while `ENABLED`, so a probe can bracket exactly one render.
///
/// `realloc` is left to the default `GlobalAlloc` shim, which routes through `alloc` + `dealloc`
/// here, so a growing container reads as one allocation per growth step - which is the number a
/// pre-sized or recycled container would remove.
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

struct Counts {
    allocs: usize,
    frees: usize,
    bytes: usize,
    peak: usize,
}

/// Run `body` with counting on, reporting what it allocated.
///
/// `LIVE` is reset per measurement, so `peak` is peak growth over the state the frame started in
/// rather than process-wide peak RSS.
fn measure(body: impl FnOnce()) -> Counts {
    ALLOCS.store(0, Ordering::Relaxed);
    FREES.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::SeqCst);
    body();
    ENABLED.store(false, Ordering::SeqCst);
    Counts {
        allocs: ALLOCS.load(Ordering::Relaxed),
        frees: FREES.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
    }
}

/// Mirrors `app_render`'s setup; see that bench for why `opening` and the id base matter.
fn backend_with_panes(panes: usize, corpus: &[u8]) -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: f32::from(VIEWPORT.w),
            h: f32::from(VIEWPORT.h),
        };
        let mut ids = Vec::with_capacity(panes);
        for index in 0..panes {
            let id = index as PaneId + 10;
            let mut pane = Pane::new(id, 5_000, rect);
            pane.opening = false;
            pane.terminal_active = true;
            pane.terminal.process_server_output(corpus);
            state.current_mut().workspaces[0].panes.push(pane);
            ids.push(id);
        }
        let start_axis = state.current().workspaces[0].start_axis;
        let ratios = state.current().workspaces[0].split_ratios.clone();
        state.current_mut().workspaces[0].tile_tree = build_dwindle_tree(&ids, start_axis, &ratios);
        state.current_mut().next_pane_id = panes as PaneId + 10;
        state.current_mut().focused_pane = Some(10);
        state.current_mut().workspaces[0].focused_pane = Some(10);
    }
    backend.render();
    backend
}

fn run() {
    let corpus = support::sgr_heavy();
    let filled: Vec<u8> = corpus.iter().copied().take(64 * 1024).collect();

    println!(
        "{:>6}  {:>10}  {:>10}  {:>12}  {:>12}",
        "panes", "allocs", "frees", "bytes", "peak_live"
    );
    for panes in [1usize, 2, 4, 8, 16] {
        let mut backend = backend_with_panes(panes, &filled);
        // One warm render outside the window: the first render after setup mounts state that
        // steady-state frames reuse, and it is not what `view_layout` measures.
        backend.render();
        let counts = measure(|| {
            backend.render();
        });
        println!(
            "{panes:>6}  {:>10}  {:>10}  {:>12}  {:>12}",
            counts.allocs, counts.frees, counts.bytes, counts.peak
        );
    }
}

fn main() {
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(run)
        .expect("spawn probe thread")
        .join()
        .expect("probe thread panicked");
}
