//! `rozi record`: send a recording command to a session, or export or play a recording file.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use tui_lipan::Result;

use super::args::{ControlEndpoint, ExportTarget, PlayFrom, RecordCli};
use crate::recording::export;
use crate::recording::frame::captured_frame;
use crate::recording::ReplayStep;

pub(crate) fn run_record_cli(command: RecordCli) -> Result<()> {
    let result = match command {
        RecordCli::Control {
            mut control,
            foreground,
        } => {
            // A local session shares this filesystem, so a relative path means here. A remote one
            // resolves it on its own host, against the directory its login starts in.
            if matches!(control.endpoint, ControlEndpoint::Session(_))
                && let Ok(cwd) = std::env::current_dir()
            {
                control.request.command.resolve_output_against(&cwd);
            }
            if foreground
                && let crate::control::ControlCommand::RecordStart { target, output, .. } =
                    &control.request.command
            {
                let pane = target.map_or_else(|| "the pane".to_string(), |id| format!("pane {id}"));
                eprintln!("Recording {pane} to {output}; press Ctrl-C to stop.");
            }
            return super::run_control_cli(control);
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

fn run_export(input: &Path, to: &ExportTarget, scale: u8, force: bool) -> std::result::Result<(), String> {
    let replay = export::open(input)?;
    let (summary, written) = match to {
        ExportTarget::PngFrames(dir) => (export::png_frames(replay, dir, scale, force)?, dir),
        ExportTarget::Cast(path) => (export::cast(replay, path, force)?, path),
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
fn play(input: &Path, speed: f64, from: Option<&PlayFrom>) -> std::result::Result<(), String> {
    let mut replay = export::open(input)?;
    let mut stdout = std::io::stdout().lock();
    let mut previous = None;
    let mut playing = from.is_none_or(|from| matches!(from, PlayFrom::Time(0)));
    // The recording time and wall-clock instant playback is measured from.
    let mut origin: Option<(u64, Instant)> = None;
    let mut end = None;
    while let Some(step) = replay.step()? {
        let t = match step {
            ReplayStep::Frame { t } => {
                if !playing && let Some(PlayFrom::Time(at)) = from {
                    playing = t >= *at;
                }
                t
            }
            ReplayStep::Mark { t, label } => {
                if !playing
                    && let Some(PlayFrom::Mark(want)) = from
                    && &label == want
                {
                    playing = true;
                    origin = Some((t, Instant::now()));
                    t
                } else {
                    continue;
                }
            }
            ReplayStep::End(summary) => {
                end = Some(summary.t);
                break;
            }
            ReplayStep::Meta { .. } => continue,
        };
        if !playing || replay.frame().is_none() {
            continue;
        }
        let (base, started) = *origin.get_or_insert((t, Instant::now()));
        let due = started + Duration::from_secs_f64(t.saturating_sub(base) as f64 / 1000.0 / speed);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let images = replay.frame_images()?.clone();
        let frame = replay.frame().expect("checked above");
        let captured = captured_frame(frame, &images);
        let ansi = captured.to_ansi_diff(previous.as_ref());
        if stdout
            .write_all(ansi.as_bytes())
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return Ok(());
        }
        previous = Some(captured);
    }
    let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\r\n");
    let _ = stdout.flush();
    if !playing && let Some(from) = from {
        return Err(match from {
            PlayFrom::Mark(label) => format!("the recording has no mark named {label:?}"),
            PlayFrom::Time(ms) => format!(
                "the recording is {:.1}s long, shorter than {:.1}s",
                end.unwrap_or_else(|| replay.elapsed()) as f64 / 1000.0,
                *ms as f64 / 1000.0
            ),
        });
    }
    Ok(())
}
