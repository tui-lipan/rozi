//! Guards the `app_render` bench setup. The bench measures whole-app view/layout cost, which is
//! only meaningful if every pane is actually tiled, visible, and carrying terminal content — an
//! empty or half-built workspace would quietly benchmark almost nothing.
//!
//! Two traps this pins down, both of which silently produced a near-empty frame while looking
//! correct in state:
//! - `Pane::new` sets `opening: true`, and an opening pane animates in from nothing.
//! - Reusing pane id 1 collides with the pane `State::new` seeds, whose stale per-pane transition
//!   entries keep the reused pane off its target rect.

#[path = "../../../benches/support/mod.rs"]
mod bench_support;

const PANES: usize = 4;

#[test]
fn bench_style_setup_renders_populated_tiled_panes() {
    rozi::test_support::on_deeper_stack(|| {
        let corpus: String = (0..80)
            .map(|line| format!("row{line:03}-BENCH\r\n"))
            .collect();
        let mut backend = bench_support::backend_with_panes(PANES, corpus.as_bytes());
        for pane in &mut backend.state_mut().current_mut().workspaces[0].panes {
            pane.set_custom_title("benchmark-pane");
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let rendered = lines.join("\n");

        // Every pane is tiled and titled.
        let titles: usize = lines
            .iter()
            .map(|line| line.matches("benchmark-pane").count())
            .sum();
        assert_eq!(
            titles, PANES,
            "expected one title bar per tiled pane:\n{rendered}"
        );

        // Check every terminal's own rectangle so an empty tile cannot hide behind a busy one.
        let snapshot =
            backend.capture_ui_snapshot_with_options(&tui_lipan::UiSnapshotOptions::diagnostic());
        let frame = backend.capture_frame();
        for pane in &backend.state().current().workspaces[0].panes {
            let key = format!("rozi-terminal-{}", pane.id);
            let rect = snapshot
                .widgets
                .iter()
                .find(|widget| {
                    widget
                        .key
                        .as_ref()
                        .is_some_and(|value| value.as_ref() == key)
                })
                .expect("benchmark terminal is mounted")
                .rect;
            let content: String = (rect.y as u16..rect.y as u16 + rect.h)
                .flat_map(|y| (rect.x as u16..rect.x as u16 + rect.w).map(move |x| (x, y)))
                .map(|(x, y)| frame.cell(x, y).symbol.as_str())
                .collect();
            assert!(
                content.contains("-BENCH"),
                "pane {} has no benchmark content:\n{rendered}",
                pane.id
            );
        }
    });
}

// The two fixture checks below guard workload validity only: benchmark dimensions are free to
// change, but a corpus that stops producing matches or history would measure nothing. Replay
// correctness itself is covered by tui-lipan's `TerminalScreen` tests.

#[test]
fn scrollback_search_fixture_produces_sparse_dense_and_empty_workloads() {
    let mut pane = rozi::pane::TerminalPane::new(bench_support::SEARCH_SCROLLBACK);
    pane.apply_server_resize(bench_support::SEARCH_COLS, bench_support::SEARCH_ROWS);
    pane.process_server_output(&bench_support::scrollback_search_corpus());

    let sparse = pane
        .search_scrollback(bench_support::SEARCH_SPARSE_QUERY)
        .len();
    let dense = pane
        .search_scrollback(bench_support::SEARCH_DENSE_QUERY)
        .len();
    assert!(
        0 < sparse && sparse < dense,
        "sparse={sparse} dense={dense}"
    );
    assert!(
        pane.search_scrollback(bench_support::SEARCH_NO_MATCH_QUERY)
            .is_empty()
    );
}

#[test]
fn resurrection_snapshot_fixture_fills_requested_history() {
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
    }
}
