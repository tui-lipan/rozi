//! Full-app view/expand/layout cost, the work every `Update::full()` pays for.
//!
//! `AppRoot` is the only `Component` in the crate, so there is no subtree smaller than "the
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

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use rozi::AppRoot;
use rozi::session::protocol::Frame;
use rozi::state::{Pane, PaneId};
use rozi::tiling::build_dwindle_tree;
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

/// What the sidebar adds to every full frame.
///
/// The sidebar rebuilds its whole body on each render: the agents tab walks every pane in every
/// workspace and allocates per-row strings, and the file tree tabs re-expand a directory listing.
/// A mux renders on every batch of terminal output, so this is the cost of having the sidebar open
/// at all — measured per tab against the same frame with it hidden.
fn sidebar_render(c: &mut Criterion) {
    use rozi::config::{SidebarTab, SidebarTreeConfig, SidebarTreeView};

    let corpus = support::sgr_heavy();
    let filled: Vec<u8> = corpus.iter().copied().take(64 * 1024).collect();
    const PANES: usize = 8;

    let tree_tab = |view: SidebarTreeView| SidebarTab::Tree {
        view,
        config: SidebarTreeConfig::for_view(view),
    };
    let cases: Vec<(&str, Option<SidebarTab>)> = vec![
        ("hidden", None),
        ("panes", Some(SidebarTab::Panes)),
        ("activity", Some(SidebarTab::Activity)),
        ("files", Some(tree_tab(SidebarTreeView::Files))),
        ("git", Some(tree_tab(SidebarTreeView::Changes))),
    ];

    let mut group = c.benchmark_group("sidebar_render");
    for (name, tab) in cases {
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            let mut backend = backend_with_panes(PANES, &filled);
            if let Some(tab) = tab.clone() {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
                // The file tree roots at the focused pane's directory; point it at this repo so the
                // benchmark reads a real listing rather than an empty one.
                state.current_mut().workspaces[0].panes[0].terminal.cwd =
                    Some(env!("CARGO_MANIFEST_DIR").to_string());
                state.sidebar.tree_cwd = Some(env!("CARGO_MANIFEST_DIR").to_string());
                state.sidebar.tree_repo = Some(env!("CARGO_MANIFEST_DIR").to_string());
                // Agent rows only exist for panes the server detected an agent in.
                for pane in &mut state.current_mut().workspaces[0].panes {
                    pane.terminal.detected_agent = Some(rozi::session::protocol::DetectedAgent {
                        agent: rozi::session::protocol::AgentIdentity::new("claude", "Claude Code")
                            .into(),
                        state: rozi::session::protocol::DetectedAgentState::Working,
                    });
                }
            }
            // Warm the tree's background directory load before timing.
            for _ in 0..20 {
                backend.render();
                let _ = backend.pump();
            }
            b.iter(|| {
                backend.render();
                black_box(backend.element());
            });
        });
    }
    group.finish();
}

/// Per-message overhead of the post-update chokepoints in `update::handle_msg`.
///
/// `SessionOutput` is the highest-frequency message in the app — one per batch of PTY bytes — and
/// every one of them pays for the sidebar's root sync and the focus chokepoint regardless of
/// whether the sidebar is even visible. This measures that fixed cost against the message itself.
fn message_overhead(c: &mut Criterion) {
    let corpus = support::sgr_heavy();
    let filled: Vec<u8> = corpus.iter().copied().take(64 * 1024).collect();
    let chunk: Vec<u8> = corpus.iter().copied().take(512).collect();

    let mut group = c.benchmark_group("message_overhead");
    for cwd in ["none", "deep"] {
        group.bench_function(BenchmarkId::from_parameter(cwd), |b| {
            let mut backend = backend_with_panes(8, &filled);
            let (pane_id, generation) = {
                let state = backend.state_mut();
                if cwd == "deep" {
                    // A realistic reported directory: the sync compares this on every message.
                    state.current_mut().workspaces[0].panes[0].terminal.cwd =
                        Some("/home/user/work/projects/rozi/src/view/sidebar".to_string());
                }
                let pane = &state.current().workspaces[0].panes[0];
                (pane.id, pane.pty_generation)
            };
            let epoch = backend.state().runtime_epoch;
            b.iter(|| {
                let _ = backend.dispatch(rozi::Msg::SessionOutput {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    bytes: chunk.clone(),
                });
            });
        });
    }
    group.finish();
}

/// Client mailbox throughput when several panes produce interleaved output.
///
/// Adjacent output for one pane is already coalesced on insertion. Round-robin pane order pins the
/// multi-writer case where that optimization cannot collapse the mailbox, and therefore exposes
/// how much dispatcher and post-update work the drain policy adds around terminal processing.
fn inbound_drain(c: &mut Criterion) {
    const TOTAL_BYTES: usize = 256 * 1024;

    let mut group = c.benchmark_group("inbound_drain");
    group.throughput(Throughput::Bytes(TOTAL_BYTES as u64));
    for panes in [1usize, 2, 4, 8] {
        for chunk_size in [64usize, 1024] {
            group.bench_function(
                BenchmarkId::new(format!("{panes}_panes"), chunk_size),
                |b| {
                    let mut backend = backend_with_panes(panes, b"");
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    while backend.state().command_link.is_none() {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the benchmark backend never delivered its command link"
                        );
                        backend.pump().expect("settle benchmark mount");
                        std::thread::yield_now();
                    }
                    let link = backend
                        .state()
                        .command_link
                        .clone()
                        .expect("command link settled above");
                    let epoch = backend.state().runtime_epoch;
                    let pane_keys: Vec<_> = backend.state().current().workspaces[0]
                        .panes
                        .iter()
                        .map(|pane| (pane.id, pane.pty_generation))
                        .collect();
                    let chunk = support::bytes_of_len(chunk_size);
                    let frames: Vec<_> = (0..TOTAL_BYTES / chunk_size)
                        .map(|index| {
                            let (pane_id, generation) = pane_keys[index % pane_keys.len()];
                            Frame::PaneBytes {
                                pane_id,
                                local: false,
                                generation,
                                bytes: chunk.clone(),
                            }
                        })
                        .collect();

                    b.iter_batched(
                        || {
                            rozi::test_support::inbound_mailbox_fixture(
                                link.clone(),
                                epoch,
                                frames.clone(),
                            )
                        },
                        |mailbox| {
                            let mut dispatcher_messages = 0usize;
                            while !mailbox.is_empty() {
                                let level = backend
                                    .update_level(mailbox.drain_message())
                                    .expect("drain benchmark update");
                                black_box(level);
                                dispatcher_messages += 1;
                            }
                            black_box(dispatcher_messages);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
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
            sidebar_render(&mut criterion);
            message_overhead(&mut criterion);
            inbound_drain(&mut criterion);
            criterion.final_summary();
        })
        .expect("spawn bench thread")
        .join()
        .expect("bench thread panicked");
}
