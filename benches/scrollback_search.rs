mod support;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hyprmux::pane::TerminalPane;
use std::hint::black_box;

fn populated_pane(corpus: &[u8]) -> TerminalPane {
    let mut pane = TerminalPane::new(support::SEARCH_SCROLLBACK);
    pane.apply_server_resize(support::SEARCH_COLS, support::SEARCH_ROWS);
    pane.process_server_output(corpus);
    pane
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
        let mut terminals: Vec<_> = (0..panes).map(|_| populated_pane(&corpus)).collect();
        for (case, query) in queries {
            group.bench_with_input(BenchmarkId::new(case, panes), &query, |b, query| {
                b.iter(|| {
                    let matches: usize = terminals
                        .iter_mut()
                        .map(|pane| pane.search_scrollback(black_box(query)).len())
                        .sum();
                    black_box(matches)
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, scrollback_search);
criterion_main!(benches);
