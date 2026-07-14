mod support;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hyprmux::pane::TerminalPane;
use std::hint::black_box;

fn snapshot_rebuild(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_rebuild");
    for (cols, rows) in [(80, 24), (200, 60), (320, 90)] {
        group.throughput(Throughput::Bytes(u64::from(cols) * u64::from(rows)));
        group.bench_function(
            BenchmarkId::from_parameter(format_args!("{cols}x{rows}")),
            |b| {
                b.iter_batched(
                    || support::dirty_screen(cols, rows),
                    |mut terminal| black_box(terminal.render_snapshot()),
                    BatchSize::PerIteration,
                );
            },
        );
    }
    group.finish();

    let mut group = c.benchmark_group("terminal_pane_process_server_output");
    for size in [64, 1024, 64 * 1024] {
        let bytes = support::bytes_of_len(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &bytes, |b, bytes| {
            b.iter_batched(
                || TerminalPane::new(5_000),
                |mut pane| black_box(pane.process_server_output(black_box(bytes))),
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, snapshot_rebuild);
criterion_main!(benches);
