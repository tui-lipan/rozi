//! Hold one large terminal replay in either a `Vec` or Rozi's file spool for external memory
//! accounting. This is intentionally a process probe rather than a timing benchmark.
//!
//! Build with `cargo build --release --bench replay_memory`, then run the generated
//! `target/release/deps/replay_memory-<hash> <vec|spool> <control-dir>` executable.
//!
//! The process writes `ready`, waits for `go`, exports and writes `done`, then waits for `stop`.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use tui_lipan::prelude::TerminalScreen;

const ROWS: u16 = 64;
const COLS: u16 = 253;
const HISTORY: usize = 5_000;
const FRAME_BYTES: usize = 256 * 1024;

enum HeldReplay {
    Vec(Vec<u8>),
    File(fs::File),
}

fn screen() -> TerminalScreen {
    let mut screen = TerminalScreen::new(ROWS, COLS, HISTORY);
    for line in 0..HISTORY + usize::from(ROWS) + 2 {
        let mut bytes = Vec::new();
        for col in 0..COLS {
            write!(
                &mut bytes,
                "\x1b[38;5;{}mX",
                (line + usize::from(col)) % 256
            )
            .unwrap();
        }
        bytes.extend_from_slice(b"\x1b[0m\r\n");
        screen.process_bytes(&bytes);
    }
    screen
}

fn wait_for(path: &Path) {
    while !path.exists() {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode: vec or spool");
    let control = args.next().expect("control directory");
    assert!(args.next().is_none(), "unexpected extra argument");
    let control = Path::new(&control);
    fs::create_dir_all(control).expect("control directory");

    let mut screen = screen();
    fs::write(control.join("ready"), b"").expect("ready marker");
    wait_for(&control.join("go"));

    let started = Instant::now();
    let replay = match mode.as_str() {
        "vec" => HeldReplay::Vec(screen.export_replay_bytes()),
        "spool" => {
            let mut file = rozi::platform::paths::replay_spool_file(
                &rozi::platform::paths::PlatformEnv::from_process(),
            )
            .expect("replay spool");
            {
                let mut writer = BufWriter::with_capacity(FRAME_BYTES, &mut file);
                screen
                    .write_replay_bytes(&mut writer)
                    .expect("stream replay");
                writer.flush().expect("flush replay");
            }
            HeldReplay::File(file)
        }
        other => panic!("unknown mode {other:?}"),
    };
    let len = match &replay {
        HeldReplay::Vec(bytes) => bytes.len() as u64,
        HeldReplay::File(file) => file.metadata().expect("spool metadata").len(),
    };
    fs::write(
        control.join("done"),
        format!("{len} {}\n", started.elapsed().as_micros()),
    )
    .expect("done marker");

    wait_for(&control.join("stop"));
    std::hint::black_box((&screen, &replay));
    fs::remove_dir_all(control).expect("remove control directory");
}
