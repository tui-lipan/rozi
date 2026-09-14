//! History benchmarks: steady ingestion starts full and keeps the same parser alive.
//! Boundary measurements exclude fixture construction and destruction from their timers.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::ops::ControlFlow;
use tui_lipan::prelude::TerminalScreen;
use tui_lipan::utils::{GridPos, SelectionEnd};

const HISTORY: usize = 5_000;

fn corpus(kind: &str, cols: u16, lines: usize) -> Vec<u8> {
    let line = match kind {
        "unicode" => "東京 e\u{301} 🦀 log record\r\n".to_owned(),
        "dense" => {
            let mut text: String = (0..cols)
                .map(|col| char::from(b'a' + (col % 26) as u8))
                .collect();
            text.push_str("\r\n");
            text
        }
        _ => "short log record\r\n".to_owned(),
    };
    line.repeat(lines).into_bytes()
}

fn populated(cols: u16, rows: u16, kind: &str) -> TerminalScreen {
    let mut screen = TerminalScreen::new(rows, cols, HISTORY);
    screen.process_bytes(&corpus(kind, cols, HISTORY + usize::from(rows) + 1));
    assert_eq!(screen.total_scrollback_rows(), HISTORY);
    screen
}

fn sustained(c: &mut Criterion) {
    let mut group = c.benchmark_group("terminal_history/sustained");
    for (cols, rows) in [(80, 24), (253, 64)] {
        for kind in ["plain", "unicode", "dense"] {
            for lines in [10_000, 100_000] {
                let input = corpus(kind, cols, lines);
                group.throughput(Throughput::Bytes(input.len() as u64));
                group.bench_function(
                    BenchmarkId::new(kind, format!("{cols}x{rows}/{lines}")),
                    |b| {
                        let mut screen = populated(cols, rows, kind);
                        b.iter(|| screen.process_bytes(black_box(&input)));
                        assert_eq!(screen.total_scrollback_rows(), HISTORY);
                    },
                );
            }
        }
    }
    group.finish();
}

fn boundaries(c: &mut Criterion) {
    let mut group = c.benchmark_group("terminal_history/boundary");
    for kind in ["plain", "unicode", "dense"] {
        for (name, offset) in [
            ("render_active", 0),
            ("render_mix", 32),
            ("render_deep", HISTORY),
        ] {
            group.bench_function(BenchmarkId::new(name, kind), |b| {
                b.iter_batched_ref(
                    || {
                        let mut screen = populated(253, 64, kind);
                        screen.set_scrollback(offset);
                        screen
                    },
                    |screen| black_box(screen.render_snapshot()),
                    BatchSize::PerIteration,
                );
            });
        }
        let mut screen = populated(253, 64, kind);
        group.bench_function(BenchmarkId::new("search_no_match", kind), |b| {
            b.iter(|| {
                let mut matches = 0;
                let _ = screen.try_for_each_text_line(0, HISTORY, |_, text| {
                    matches += usize::from(text.contains(black_box("absent-query")));
                    ControlFlow::Continue(())
                });
                black_box(matches)
            });
        });
        group.bench_function(BenchmarkId::new("copy", kind), |b| {
            b.iter(|| {
                screen.export_selection_text(
                    GridPos { row: 0, col: 2 },
                    GridPos {
                        row: HISTORY - 1,
                        col: 8,
                    },
                    SelectionEnd::Inclusive,
                )
            });
        });
        group.bench_function(BenchmarkId::new("replay", kind), |b| {
            b.iter(|| screen.write_replay_bytes(&mut std::io::sink()).unwrap());
        });
        for (name, cols, rows) in [
            ("resize_height", 253, 80),
            ("resize_narrow", 80, 64),
            ("resize_wide", 320, 64),
        ] {
            group.bench_function(BenchmarkId::new(name, kind), |b| {
                b.iter_batched_ref(
                    || populated(253, 64, kind),
                    |screen| screen.resize(rows, cols),
                    BatchSize::PerIteration,
                );
            });
        }
    }
    group.finish();
}

criterion_group!(benches, sustained, boundaries);
criterion_main!(benches);
