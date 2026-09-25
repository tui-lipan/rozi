//! Turning a recording into files other tools read: PNG frames with an ffmpeg concat listing, or an
//! asciinema cast.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use tui_lipan::CastRecording;

use super::frame::{captured_frame, palette};
use super::read::{Replay, ReplayStep};

/// The ffmpeg concat listing `png-frames` writes beside the frames.
pub const CONCAT_LISTING: &str = "frames.ffconcat";
/// How long the last frame lasts when the recording does not say when it ended.
const FALLBACK_LAST_FRAME_MS: u64 = 1_000;

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub frames: u64,
    pub duration_ms: u64,
    /// The recording ended partway through a line, and was exported up to there.
    pub truncated: bool,
}

/// Write every frame of `replay` as `frame-NNNNNN.png` in `dir`, plus [`CONCAT_LISTING`] giving
/// each frame its real duration. `dir` is created if missing; existing frames are refused unless
/// `overwrite`.
pub fn png_frames<R: BufRead>(
    mut replay: Replay<R>,
    dir: &Path,
    scale: u8,
    overwrite: bool,
) -> Result<ExportSummary, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let mut frames: Vec<(String, u64)> = Vec::new();
    let mut end = None;
    while let Some(step) = replay.step()? {
        match step {
            ReplayStep::Frame { t } => {
                let name = format!("frame-{:06}.png", frames.len() + 1);
                let images = replay.frame_images()?.clone();
                let frame = replay.frame().expect("a frame step has a frame");
                let png = crate::pane::png_bytes(
                    &captured_frame(frame, &images),
                    palette(&frame.palette),
                    scale,
                )
                .map_err(|error| format!("cannot draw frame {}: {error}", frames.len() + 1))?;
                write_new(&dir.join(&name), &png, overwrite)?;
                frames.push((name, t));
            }
            ReplayStep::End(summary) => end = Some(summary.t),
            ReplayStep::Mark { .. } | ReplayStep::Meta { .. } => {}
        }
    }
    let end = end.unwrap_or_else(|| replay.elapsed());
    let listing = concat_listing(&frames, end);
    write_new(&dir.join(CONCAT_LISTING), listing.as_bytes(), overwrite)?;
    Ok(ExportSummary {
        frames: frames.len() as u64,
        duration_ms: frames
            .first()
            .map_or(0, |(_, first)| last_frame_end(&frames, end) - first),
        truncated: replay.truncated(),
    })
}

/// When the last frame stops showing: at the recording's end, or a second later when the end is
/// not after it.
fn last_frame_end(frames: &[(String, u64)], end: u64) -> u64 {
    let last = frames.last().map_or(0, |(_, t)| *t);
    if end > last {
        end
    } else {
        last + FALLBACK_LAST_FRAME_MS
    }
}

/// An ffmpeg concat listing that shows each frame until the next one. The last file is listed
/// twice, as the concat demuxer needs to honor its duration.
fn concat_listing(frames: &[(String, u64)], end: u64) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    let stop = last_frame_end(frames, end);
    for (index, (name, t)) in frames.iter().enumerate() {
        let next = frames.get(index + 1).map_or(stop, |(_, next)| *next);
        out.push_str(&format!(
            "file '{name}'\nduration {:.3}\n",
            next.saturating_sub(*t) as f64 / 1000.0
        ));
    }
    if let Some((name, _)) = frames.last() {
        out.push_str(&format!("file '{name}'\n"));
    }
    out
}

/// Write `replay` as an asciinema cast v2 file. Images appear as the half-block stand-ins their
/// cells hold, since a cast is text; marks and meta events are left out.
pub fn cast<R: BufRead>(
    mut replay: Replay<R>,
    path: &Path,
    overwrite: bool,
) -> Result<ExportSummary, String> {
    let header = replay.header().clone();
    let mut cast = CastRecording::new(header.width, header.height).title(match &header.target {
        super::format::RecordingTarget::Pane { session, pane } => {
            format!("rozi {session} pane {pane}")
        }
    });
    let mut frames = 0;
    let mut end = None;
    while let Some(step) = replay.step()? {
        match step {
            ReplayStep::Frame { t } => {
                let images = replay.frame_images()?.clone();
                let frame = replay.frame().expect("a frame step has a frame");
                if cast.push_frame(t as f64 / 1000.0, &captured_frame(frame, &images)) {
                    frames += 1;
                }
            }
            ReplayStep::End(summary) => end = Some(summary.t),
            ReplayStep::Mark { .. } | ReplayStep::Meta { .. } => {}
        }
    }
    let end = end.unwrap_or_else(|| replay.elapsed());
    cast.mark_time(end as f64 / 1000.0);
    write_new(path, cast.to_cast().as_bytes(), overwrite)?;
    Ok(ExportSummary {
        frames,
        duration_ms: end,
        truncated: replay.truncated(),
    })
}

fn write_new(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    let describe = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{} already exists; pass --force to replace it",
                path.display()
            )
        } else {
            format!("cannot write {}: {error}", path.display())
        }
    };
    let mut file =
        crate::platform::persist::create_private_file(path, overwrite).map_err(describe)?;
    file.write_all(bytes).map_err(describe)
}

/// Open a recording file for reading.
pub fn open(path: &Path) -> Result<Replay<std::io::BufReader<std::fs::File>>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    Replay::new(std::io::BufReader::new(file))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// `path` made absolute against `base`, for a path the user typed relative to where they are.
pub fn absolute(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}
