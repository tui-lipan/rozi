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

fn program_status_ingest(c: &mut Criterion) {
    let mut group = c.benchmark_group("program_status_ingest");
    let progress = (0..1000)
        .map(|i| {
            format!(
                "\x1b]7501;state=working:app=cargo:progress={}:msg=QnVpbGRpbmc=\x1b\\",
                i % 101
            )
        })
        .collect::<String>()
        .into_bytes();
    for (name, corpus) in [("plain", support::plain_lines()), ("progress", progress)] {
        for enabled in [false, true] {
            group.throughput(Throughput::Bytes(corpus.len() as u64));
            group.bench_with_input(BenchmarkId::new(name, enabled), &corpus, |b, corpus| {
                b.iter_batched(
                    || {
                        let mut terminal = support::screen(200, 60);
                        terminal.set_program_status_enabled(enabled);
                        terminal
                    },
                    |mut terminal| {
                        terminal.process_bytes(black_box(corpus));
                        terminal.drain_program_status_events();
                        black_box(terminal);
                    },
                    BatchSize::PerIteration,
                );
            });
        }
    }
    group.finish();
}

criterion_group!(benches, terminal_ingest, program_status_ingest);
criterion_main!(benches);
