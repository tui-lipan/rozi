use std::io::{BufWriter, Read, Seek, Write};
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use tui_lipan::prelude::TerminalScreen;

fn replay_screen(rows: u16, cols: u16, history: usize) -> TerminalScreen {
    let mut input = Vec::new();
    for line in 0..history + usize::from(rows) + 2 {
        for col in 0..cols {
            write!(
                &mut input,
                "\x1b[38;5;{}mX",
                (line + usize::from(col)) % 256
            )
            .unwrap();
        }
        input.extend_from_slice(b"\x1b[0m\r\n");
    }
    let mut screen = TerminalScreen::new(rows, cols, history);
    screen.process_bytes(&input);
    screen
}

fn replay_export(c: &mut Criterion) {
    let mut screen = replay_screen(32, 120, 1_000);
    let replay_len = screen.export_replay_bytes().len();
    let mut group = c.benchmark_group("replay_export");
    group.throughput(Throughput::Bytes(replay_len as u64));

    group.bench_function("collected_vec", |b| {
        b.iter(|| std::hint::black_box(screen.export_replay_bytes()));
    });

    let spool_dir = tempfile::tempdir().expect("temporary replay spool directory");
    let mut read_buffer = vec![0; 256 * 1024];
    group.bench_function("spool_write_read", |b| {
        b.iter(|| {
            let mut spool =
                tempfile::tempfile_in(spool_dir.path()).expect("temporary replay spool");
            {
                let mut writer = BufWriter::with_capacity(256 * 1024, &mut spool);
                screen.write_replay_bytes(&mut writer).unwrap();
                writer.flush().unwrap();
            }
            spool.rewind().unwrap();
            let mut consumed = 0;
            loop {
                let read = spool.read(&mut read_buffer).unwrap();
                if read == 0 {
                    break;
                }
                consumed += read;
                std::hint::black_box(&read_buffer[..read]);
            }
            assert_eq!(consumed, replay_len);
        });
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));
    targets = replay_export
}
criterion_main!(benches);
