mod support;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use rozi::benchmark_advance_search_scan;
use rozi::config::Config;
use rozi::pane::TerminalPane;
use rozi::state::{ScrollbackSearchScan, ScrollbackSearchState, State};
use std::cell::RefCell;
use std::hint::black_box;
use std::sync::Arc;
use tui_lipan::prelude::Theme;

struct FullSliceFixture {
    state: RefCell<State>,
    epoch: u64,
    seed_matches: Vec<rozi::state::ScrollbackMatch>,
    seed_items: Arc<[tui_lipan::prelude::SearchItem<usize>]>,
    seed_scan: ScrollbackSearchScan,
}

fn populated_pane(corpus: &[u8]) -> TerminalPane {
    let mut pane = TerminalPane::new(support::SEARCH_SCROLLBACK);
    pane.apply_server_resize(support::SEARCH_COLS, support::SEARCH_ROWS);
    pane.process_server_output(corpus);
    pane
}

fn populated_search_state(corpus: &[u8], query: &'static str) -> FullSliceFixture {
    let mut state = State::new(Config::default(), Theme::default());
    let target = state.current().focused_pane.expect("benchmark target");
    let pane = state.current_mut().workspaces[0]
        .panes
        .iter_mut()
        .find(|pane| pane.id == target)
        .expect("benchmark pane");
    pane.terminal
        .apply_server_resize(support::SEARCH_COLS, support::SEARCH_ROWS);
    pane.terminal.process_server_output(corpus);
    let pane_end = pane.terminal.search_line_count();
    let epoch = 1;
    let mut search = ScrollbackSearchState::new(target);
    search.scan = Some(ScrollbackSearchScan {
        epoch,
        query: Arc::from(query),
        panes: Arc::from([target]),
        pane_ends: Arc::from([pane_end]),
        pane_index: 0,
        line_cursor: 0,
        first_jump_done: false,
    });
    state.search_scan_epoch = epoch;
    state.search = Some(search);
    let _ = benchmark_advance_search_scan(&mut state, epoch, 1_488);
    let (seed_matches, seed_items, seed_scan) = {
        let search = state.search.as_ref().expect("seeded benchmark search");
        (
            search.matches.clone(),
            Arc::clone(&search.items),
            search.scan.clone().expect("remaining benchmark scan"),
        )
    };
    FullSliceFixture {
        state: RefCell::new(state),
        epoch,
        seed_matches,
        seed_items,
        seed_scan,
    }
}

fn scrollback_search(c: &mut Criterion) {
    let corpus = support::scrollback_search_corpus();
    let queries = [
        ("sparse", support::SEARCH_SPARSE_QUERY),
        ("dense", support::SEARCH_DENSE_QUERY),
        ("no_match", support::SEARCH_NO_MATCH_QUERY),
    ];
    let mut group = c.benchmark_group("scrollback_search");

    for panes in support::SEARCH_PANE_COUNTS {
        let terminals: Vec<_> = (0..panes).map(|_| populated_pane(&corpus)).collect();
        for (case, query) in queries {
            group.bench_with_input(BenchmarkId::new(case, panes), &query, |b, query| {
                b.iter(|| {
                    let matches: usize = terminals
                        .iter()
                        .map(|pane| pane.search_scrollback(black_box(query)).len())
                        .sum();
                    black_box(matches)
                });
            });
        }
    }

    let terminal = populated_pane(&corpus);
    let slice_end = 512.min(terminal.search_line_count());
    assert_eq!(slice_end, 512, "search corpus must retain a full slice");
    for (case, query) in queries {
        group.bench_with_input(
            BenchmarkId::new(format!("slice_{case}"), slice_end),
            &query,
            |b, query| {
                b.iter(|| {
                    black_box(
                        terminal
                            .search_scrollback_range(black_box(query), 0, slice_end, usize::MAX)
                            .matches
                            .len(),
                    )
                });
            },
        );
    }

    for (case, query) in queries {
        let fixture = populated_search_state(&corpus, query);
        group.bench_function(format!("full_slice_{case}/512"), |b| {
            b.iter_batched(
                || {
                    let mut state = fixture.state.borrow_mut();
                    let search = state.search.as_mut().expect("benchmark search");
                    search.matches.clone_from(&fixture.seed_matches);
                    search.items = Arc::clone(&fixture.seed_items);
                    search.current = 0;
                    search.truncated = false;
                    search.scan = Some(fixture.seed_scan.clone());
                },
                |()| {
                    black_box(benchmark_advance_search_scan(
                        &mut fixture.state.borrow_mut(),
                        fixture.epoch,
                        512,
                    ))
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, scrollback_search);
criterion_main!(benches);
