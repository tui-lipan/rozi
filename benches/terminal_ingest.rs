mod support;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

fn terminal_ingest(c: &mut Criterion) {
    let corpora = [
        ("plain_lines", support::plain_lines()),
        ("sgr_heavy", support::sgr_heavy()),
        ("scroll_regions", support::scroll_regions()),
        ("wide_unicode", support::wide_unicode()),
        ("cat_large", support::cat_large()),
    ];
    let mut group = c.benchmark_group("terminal_ingest");

    for (name, corpus) in &corpora {
        for (cols, rows) in [(80, 24), (200, 60), (320, 90)] {
            group.throughput(Throughput::Bytes(corpus.len() as u64));
            group.bench_with_input(
                BenchmarkId::new(*name, format_args!("{cols}x{rows}")),
                corpus,
                |b, corpus| {
                    b.iter_batched(
                        || support::screen(cols, rows),
                        |mut terminal| {
                            terminal.process_bytes(black_box(corpus));
                            black_box(terminal);
                        },
                        BatchSize::PerIteration,
                    );
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, terminal_ingest);
criterion_main!(benches);
