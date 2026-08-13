mod support;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rozi::session::protocol::{
    Frame, FrameDecoder, ServerMessage, write_control_frame, write_pane_output_frame,
};
use std::hint::black_box;
use std::io::{Cursor, Read};

fn protocol_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("pane_output_frame_roundtrip");
    for size in [64, 4 * 1024, 1024 * 1024] {
        let payload = support::bytes_of_len(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &payload, |b, payload| {
            b.iter(|| {
                let mut encoded = Vec::with_capacity(payload.len() + 18);
                write_pane_output_frame(&mut encoded, 41, 9, false, black_box(payload)).unwrap();
                let frame: Frame<ServerMessage> = decode_one(Cursor::new(encoded));
                black_box(frame)
            });
        });
    }
    group.finish();

    let controls = [
        ("attached", support::attached_message()),
        ("layout_committed", support::layout_committed_message()),
    ];
    let mut group = c.benchmark_group("control_frame_serde");
    for (name, message) in &controls {
        let mut encoded = Vec::new();
        write_control_frame(&mut encoded, message).unwrap();
        group.throughput(Throughput::Bytes(encoded.len() as u64));
        group.bench_with_input(BenchmarkId::new("encode", name), message, |b, message| {
            b.iter(|| {
                let mut output = Vec::with_capacity(encoded.len());
                write_control_frame(&mut output, black_box(message)).unwrap();
                black_box(output)
            });
        });
        group.bench_with_input(BenchmarkId::new("decode", name), &encoded, |b, encoded| {
            b.iter(|| {
                let frame: Frame<ServerMessage> = decode_one(Cursor::new(black_box(encoded)));
                black_box(frame)
            });
        });
    }
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

criterion_group!(benches, protocol_framing);
criterion_main!(benches);
