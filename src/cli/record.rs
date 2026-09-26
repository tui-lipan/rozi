//! `rozi record`: send a recording command to a session, or export or play a recording file.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use tui_lipan::Result;

use super::args::{ControlEndpoint, ExportTarget, RecordCli};
use crate::recording::export;
use crate::recording::play::{Cue, Playback, Seek};

pub(crate) fn run_record_cli(command: RecordCli) -> Result<()> {
    let result = match command {
        RecordCli::Control {
            mut control,
            foreground,
        } => {
            // A local session shares this filesystem, so a relative path means here. A remote one
            // resolves it on its own host, against the directory its login starts in. A UI's
            // control socket is always on this machine, and so is the file a UI recording writes.
            let local = matches!(control.endpoint, ControlEndpoint::Session(_))
                || matches!(
                    control.request.command,
                    crate::control::ControlCommand::RecordUiStart { .. }
                );
            if local && let Ok(cwd) = std::env::current_dir() {
                control.request.command.resolve_output_against(&cwd);
            }
            if foreground
                && let crate::control::ControlCommand::RecordStart { target, output, .. } =
                    &control.request.command
            {
                let pane = target.map_or_else(|| "the pane".to_string(), |id| format!("pane {id}"));
                match output {
                    Some(output) => {
                        eprintln!("Recording {pane} to {output}; press Ctrl-C to stop.")
                    }
                    None => eprintln!("Recording {pane}; press Ctrl-C to stop."),
                }
            }
            return super::run_control_cli(*control);
        }
        RecordCli::Export {
            input,
            to,
            scale,
            force,
        } => run_export(&input, &to, scale, force),
        RecordCli::Play { input, speed, from } => play(&input, speed, from.as_ref()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
    Ok(())
}

fn run_export(
    input: &Path,
    to: &ExportTarget,
    scale: u8,
    force: bool,
) -> std::result::Result<(), String> {
    let (summary, written) = match to {
        ExportTarget::PngFrames(dir) => (
            export::png_frames(export::open(input)?, dir, scale, force)?,
            dir,
        ),
        ExportTarget::Cast(path) => (export::cast(export::open(input)?, path, force)?, path),
    };
    println!(
        "Wrote {} frames covering {:.1}s to {}",
        summary.frames,
        summary.duration_ms as f64 / 1000.0,
        written.display()
    );
    if summary.truncated {
        eprintln!("The recording ends partway through; exported up to its last complete event.");
    }
    if let ExportTarget::PngFrames(dir) = to {
        let listing = dir.join(export::CONCAT_LISTING);
        eprintln!(
            "Make a GIF with: ffmpeg -f concat -safe 0 -i {} -vf \"split[a][b];[a]palettegen[p];[b][p]paletteuse\" out.gif",
            listing.display()
        );
    }
    Ok(())
}

/// Replay a recording in this terminal at its own pace, or `speed` times faster.
fn play(input: &Path, speed: f64, from: Option<&Seek>) -> std::result::Result<(), String> {
    let mut playback = Playback::new(export::open(input)?, from.cloned());
    let mut stdout = std::io::stdout().lock();
    let mut previous = None;
    let started = Instant::now();
    let wait_until = |at: u64| {
        let due = started + Duration::from_secs_f64(at as f64 / 1000.0 / speed);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    };
    let result = loop {
        match playback.next_cue() {
            Ok(Some(Cue::Show { at, frame })) => {
                wait_until(at);
                let ansi = frame.to_ansi_diff(previous.as_ref());
                if stdout
                    .write_all(ansi.as_bytes())
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    return Ok(());
                }
                previous = Some(frame);
            }
            Ok(Some(Cue::Hold { until })) => wait_until(until),
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    if previous.is_some() {
        let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\r\n");
        let _ = stdout.flush();
    }
    result
}
