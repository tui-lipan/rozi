//! Full-app view/expand/layout cost, the work every `Update::full()` pays for.
//!
//! `HyprmuxApp` is the only `Component` in the crate, so there is no subtree smaller than "the
//! whole app": one pane's output redraws every pane, the workbar, and the overlays. This target
//! measures that per-frame cost against pane count and terminal content, so the value of scoping
//! renders (child components with `memo_key`, in-view `Memo`) can be judged against a number
//! rather than a hunch.
//!
//! `TestBackend::render()` runs view + expand + layout without painting a backend buffer, which is
//! exactly the work `Update::full()` adds over `Update::paint()` — so this measures the avoidable
//! part of a full frame, not the whole frame.
//!
//! Draw is deliberately not benchmarked here. `TestBackend::capture_frame()` builds a
//! `CapturedFrame` holding a heap `String` per cell (12k allocations at this viewport), so it
//! measures the test harness rather than the real ratatui buffer write and frame diff. Use the
//! devtools metrics panel (`Draw` row) for real draw numbers.

mod support;

use criterion::{BenchmarkId, Criterion};
use hyprmux::HyprmuxApp;
use hyprmux::state::{Pane, PaneId};
use hyprmux::tiling::build_dwindle_tree;
use std::hint::black_box;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 200,
    h: 60,
};

/// Layout recursion on a deep dwindle tree overflows the default 2MB test stack.
const STACK_SIZE: usize = 16 * 1024 * 1024;

/// A backend with `panes` tiled panes, each pre-filled with `corpus` so every pane carries a
/// realistic styled snapshot rather than an empty screen.
///
/// The setup details here are load-bearing and are pinned by `tests/bench_setup_sanity.rs`:
/// `opening` must be cleared (an opening pane animates in from nothing) and ids must start past
/// the pane `State::new` seeds (a reused id inherits stale transition entries). Getting either
/// wrong renders an almost empty frame while the state still looks correct.
fn backend_with_panes(panes: usize, corpus: &[u8]) -> TestBackend<HyprmuxApp> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        state.workspaces[0].panes.clear();
        state.workspaces[0].tile_tree = None;
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
            state.workspaces[0].panes.push(pane);
            ids.push(id);
        }
        let start_axis = state.workspaces[0].start_axis;
        let ratios = state.workspaces[0].split_ratios.clone();
        state.workspaces[0].tile_tree = build_dwindle_tree(&ids, start_axis, &ratios);
        state.next_pane_id = panes as PaneId + 10;
        state.focused_pane = Some(10);
        state.workspaces[0].focused_pane = Some(10);
    }
    backend.render();
    backend
}

fn app_render(c: &mut Criterion) {
    // A screenful of styled output per pane: the realistic steady state for a working mux, and the
    // case where snapshot size actually matters.
    let corpus = support::sgr_heavy();
    let filled: Vec<u8> = corpus.iter().copied().take(64 * 1024).collect();

    let mut group = c.benchmark_group("app_render");
    for panes in [1usize, 2, 4, 8, 16] {
        // The whole cost of `Update::full()` over `Update::paint()`: view + expand + layout for
        // every pane, workbar, and overlay. This is exactly what scoping renders could avoid.
        group.bench_with_input(
            BenchmarkId::new("view_layout", panes),
            &panes,
            |b, &panes| {
                let mut backend = backend_with_panes(panes, &filled);
                b.iter(|| {
                    backend.render();
                    black_box(backend.element());
                });
            },
        );

        // Same structure with empty screens. The difference against `view_layout` is the share of
        // the frame that comes from terminal content rather than pane chrome and tiling.
        group.bench_with_input(
            BenchmarkId::new("view_layout_empty", panes),
            &panes,
            |b, &panes| {
                let mut backend = backend_with_panes(panes, b"");
                b.iter(|| {
                    backend.render();
                    black_box(backend.element());
                });
            },
        );
    }
    group.finish();
}

fn main() {
    // Criterion's harness runs on the main thread; re-host it on a deep stack so the dwindle
    // layout recursion at 16 panes does not overflow.
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let mut criterion = Criterion::default().configure_from_args();
            app_render(&mut criterion);
            criterion.final_summary();
        })
        .expect("spawn bench thread")
        .join()
        .expect("bench thread panicked");
}
