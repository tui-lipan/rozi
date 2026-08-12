//! Guards the `app_render` bench setup. The bench measures whole-app view/layout cost, which is
//! only meaningful if every pane is actually tiled, visible, and carrying terminal content — an
//! empty or half-built workspace would quietly benchmark almost nothing.
//!
//! Two traps this pins down, both of which silently produced a near-empty frame while looking
//! correct in state:
//! - `Pane::new` sets `opening: true`, and an opening pane animates in from nothing.
//! - Reusing pane id 1 collides with the pane `State::new` seeds, whose stale per-pane transition
//!   entries keep the reused pane off its target rect.

use hyprmux::AppRoot;
use hyprmux::state::{Pane, PaneId};
use hyprmux::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

#[path = "../benches/support/mod.rs"]
mod bench_support;

const PANES: usize = 4;

#[test]
fn bench_style_setup_renders_populated_tiled_panes() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let viewport = Rect {
                x: 0,
                y: 0,
                w: 200,
                h: 60,
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            {
                let state = backend.state_mut();
                state.current_mut().workspaces[0].panes.clear();
                state.current_mut().workspaces[0].tile_tree = None;
                let rect = FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 200.0,
                    h: 60.0,
                };
                let mut ids = Vec::new();
                for index in 0..PANES {
                    // Ids start past the seeded pane 1 (see module docs).
                    let id = index as PaneId + 10;
                    let mut pane = Pane::new(id, 5_000, rect);
                    pane.opening = false;
                    pane.terminal_active = true;
                    // Every row filled, so content is visible regardless of how short the tile is.
                    for line in 0..80 {
                        pane.terminal
                            .process_server_output(format!("row{line:03}-BENCH\r\n").as_bytes());
                    }
                    pane.terminal.title = Some("nvim src/main.rs".to_string());
                    pane.terminal.original_user = Some("benchmark".to_string());
                    pane.terminal.cwd = Some("/workspace/hyprmux/src".to_string());
                    pane.terminal.display_path = Some("hyprmux/src".to_string());
                    state.current_mut().workspaces[0].panes.push(pane);
                    ids.push(id);
                }
                let start_axis = state.current().workspaces[0].start_axis;
                let ratios = state.current().workspaces[0].split_ratios.clone();
                state.current_mut().workspaces[0].tile_tree =
                    build_dwindle_tree(&ids, start_axis, &ratios);
                state.current_mut().next_pane_id = 20;
                state.current_mut().focused_pane = Some(10);
                state.current_mut().workspaces[0].focused_pane = Some(10);
            }
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();
            eprintln!("{}", lines.join("\n"));

            // Every pane is tiled and titled.
            let titles: usize = lines
                .iter()
                .map(|line| line.matches("󰖲  nvim src/main.rs · hyprmux/src").count())
                .sum();
            assert_eq!(titles, PANES, "expected one title bar per tiled pane");

            // Every pane is drawing terminal content, not just chrome. Each tile is its own
            // column band, so a content row appears once per pane sharing that band.
            let content_rows = lines.iter().filter(|line| line.contains("-BENCH")).count();
            assert!(
                content_rows > 20,
                "expected panes to be full of terminal content, got {content_rows} rows"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn scrollback_search_fixture_pins_dimensions_corpus_and_matches() {
    assert_eq!(bench_support::SEARCH_PANE_COUNTS, [1, 8, 16]);
    assert_eq!(
        (
            bench_support::SEARCH_COLS,
            bench_support::SEARCH_ROWS,
            bench_support::SEARCH_SCROLLBACK,
            bench_support::SEARCH_LINES,
        ),
        (250, 60, 5_000, 5_000)
    );

    let corpus = bench_support::scrollback_search_corpus();
    assert_eq!(
        corpus.windows(2).filter(|bytes| *bytes == b"\r\n").count(),
        bench_support::SEARCH_LINES
    );

    let mut pane = hyprmux::pane::TerminalPane::new(bench_support::SEARCH_SCROLLBACK);
    pane.apply_server_resize(bench_support::SEARCH_COLS, bench_support::SEARCH_ROWS);
    pane.process_server_output(&corpus);
    assert_eq!(
        pane.search_scrollback(bench_support::SEARCH_SPARSE_QUERY)
            .len(),
        bench_support::SEARCH_SPARSE_MATCHES_PER_PANE
    );
    assert_eq!(
        pane.search_scrollback(bench_support::SEARCH_DENSE_QUERY)
            .len(),
        bench_support::SEARCH_DENSE_MATCHES_PER_PANE
    );
    assert!(
        pane.search_scrollback(bench_support::SEARCH_NO_MATCH_QUERY)
            .is_empty()
    );
}

#[test]
fn resurrection_snapshot_fixture_pins_matrix_dimensions_and_history() {
    assert_eq!(bench_support::SNAPSHOT_PANE_COUNTS, [1, 8, 16]);
    assert_eq!(bench_support::SNAPSHOT_HISTORY_ROWS, [0, 1_000, 5_000]);
    assert_eq!(
        (bench_support::SNAPSHOT_COLS, bench_support::SNAPSHOT_ROWS),
        (250, 60)
    );

    for history_rows in bench_support::SNAPSHOT_HISTORY_ROWS {
        let mut screen = tui_lipan::prelude::TerminalScreen::new(
            bench_support::SNAPSHOT_ROWS,
            bench_support::SNAPSHOT_COLS,
            history_rows,
        );
        screen.process_bytes(&bench_support::resurrection_snapshot_corpus(
            1,
            history_rows,
        ));
        assert_eq!(screen.total_scrollback_rows(), history_rows);

        let replay = screen.export_replay_bytes();
        let mut restored = tui_lipan::prelude::TerminalScreen::new(
            bench_support::SNAPSHOT_ROWS,
            bench_support::SNAPSHOT_COLS,
            history_rows,
        );
        restored.process_bytes(&replay);
        assert_eq!(restored.total_scrollback_rows(), history_rows);
    }
}
