mod support;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rozi::pane::TerminalPane;
use rozi::session::protocol::{Frame, FrameDecoder, ServerMessage, write_pane_output_frame};
use std::hint::black_box;
use std::io::{Cursor, Read};

fn session_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("session_pipeline_memory");
    for size in [64, 4 * 1024, 64 * 1024] {
        let payload = support::bytes_of_len(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &payload, |b, payload| {
            b.iter_batched(
                || TerminalPane::new(5_000),
                |mut pane| {
                    let mut encoded = Vec::with_capacity(payload.len() + 18);
                    write_pane_output_frame(&mut encoded, 41, 9, false, black_box(payload))
                        .unwrap();
                    let frame = decode_one(Cursor::new(encoded));
                    process_frame(&mut pane, frame);
                    black_box(pane)
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();

    #[cfg(unix)]
    unix_socket_pipeline(c);
}

#[cfg(unix)]
fn unix_socket_pipeline(c: &mut Criterion) {
    use std::os::unix::net::UnixStream;

    let payload = support::bytes_of_len(4 * 1024);
    let mut group = c.benchmark_group("session_pipeline_unix_socketpair");
    group.throughput(Throughput::Bytes(payload.len() as u64));
    group.bench_function("4096", |b| {
        b.iter_batched(
            || {
                let (writer, reader) = UnixStream::pair().unwrap();
                (TerminalPane::new(5_000), writer, reader)
            },
            |(mut pane, mut writer, reader)| {
                write_pane_output_frame(&mut writer, 41, 9, false, black_box(&payload)).unwrap();
                let frame = decode_one(reader);
                process_frame(&mut pane, frame);
                black_box(pane)
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn decode_one<R: Read>(mut reader: R) -> Frame<ServerMessage> {
    let mut decoder = FrameDecoder::default();
    loop {
        if let Some(frame) = decoder.next_frame().unwrap() {
            return frame;
        }
        decoder.read_from_status(&mut reader).unwrap();
    }
}

fn process_frame(pane: &mut TerminalPane, frame: Frame<ServerMessage>) {
    let Frame::PaneBytes {
        pane_id,
        generation,
        bytes,
        ..
    } = frame
    else {
        panic!("expected pane output frame");
    };
    black_box((pane_id, generation));
    black_box(pane.process_server_output(black_box(&bytes)));
}

criterion_group!(benches, session_pipeline);
criterion_main!(benches);
