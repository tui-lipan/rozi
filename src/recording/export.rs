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
    replay: Replay<R>,
    dir: &Path,
    scale: u8,
    overwrite: bool,
) -> Result<ExportSummary, String> {
    write_png_frames(replay, dir, scale, overwrite, None)
}

/// Export fixed-size PNGs for video encoding. Resizes keep their original cell size and position;
/// unused space is filled with each frame's default background. Timing is unchanged.
pub fn video_frames(
    input: &Path,
    dir: &Path,
    scale: u8,
    overwrite: bool,
) -> Result<ExportSummary, String> {
    let canvas = video_canvas(open(input)?)?;
    write_png_frames(open(input)?, dir, scale, overwrite, Some(canvas))
}

fn video_canvas<R: BufRead>(mut replay: Replay<R>) -> Result<(u16, u16), String> {
    let (mut width, mut height) = (0, 0);
    while let Some(step) = replay.step()? {
        if let ReplayStep::Frame { .. } = step {
            let frame = replay.frame().expect("a frame step has a frame");
            width = width.max(frame.width);
            height = height.max(frame.height);
        }
    }
    if usize::from(width) * usize::from(height) > super::read::MAX_FRAME_CELLS {
        return Err(format!(
            "video canvas of {width}x{height} cells is too large"
        ));
    }
    Ok((width, height))
}

/// Pad the rendered bitmap, never the cell grid: glyphs and images must remain clipped to
/// their native terminal bounds before extra space is added.
fn pad_png(
    bytes: Vec<u8>,
    native: (u16, u16),
    canvas: (u16, u16),
    background: tui_lipan::prelude::Color,
) -> Result<Vec<u8>, String> {
    if native.0 == 0 || native.1 == 0 || native.0 > canvas.0 || native.1 > canvas.1 {
        return Err(
            "recording dimensions changed during export; stop recording before exporting"
                .to_string(),
        );
    }
    if native == canvas {
        return Ok(bytes);
    }
    // These bytes come from our bounded native-frame renderer, not from an input PNG.
    let image = super::frame::DecodedImage::from_png(&bytes, usize::MAX)?;
    let width = image.width / u32::from(native.0) * u32::from(canvas.0);
    let height = image.height / u32::from(native.1) * u32::from(canvas.1);
    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "video bitmap is too large".to_string())?;
    let tui_lipan::prelude::Color::Rgb(r, g, b) = background else {
        return Err("video padding requires an RGB background".to_string());
    };
    let mut rgba = vec![0; pixels];
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[r, g, b, 255]);
    }
    let source_stride = image.width as usize * 4;
    let target_stride = width as usize * 4;
    for (source, target) in image
        .rgba
        .chunks_exact(source_stride)
        .zip(rgba.chunks_exact_mut(target_stride))
    {
        target[..source_stride].copy_from_slice(source);
    }
    encode_png(width, height, &rgba)
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(bytes)
}

fn write_png_frames<R: BufRead>(
    mut replay: Replay<R>,
    dir: &Path,
    scale: u8,
    overwrite: bool,
    canvas: Option<(u16, u16)>,
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
                let captured = captured_frame(frame, &images);
                let colors = palette(&frame.palette);
                let mut png = crate::pane::png_bytes(&captured, colors, scale)
                    .map_err(|error| format!("cannot draw frame {}: {error}", frames.len() + 1))?;
                if let Some(canvas) = canvas {
                    let background = colors
                        .background
                        .unwrap_or(tui_lipan::prelude::Color::Rgb(0, 0, 0));
                    png = pad_png(png, (frame.width, frame.height), canvas, background)?;
                }
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
/// twice, as the concat demuxer needs to honor its duration. PNG inputs use a millisecond
/// time base so ffmpeg does not round timestamps to its default 25 fps.
fn concat_listing(frames: &[(String, u64)], end: u64) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    let stop = last_frame_end(frames, end);
    for (index, (name, t)) in frames.iter().enumerate() {
        let next = frames.get(index + 1).map_or(stop, |(_, next)| *next);
        out.push_str(&format!(
            "file '{name}'\noption framerate 1000\nduration {:.3}\n",
            next.saturating_sub(*t) as f64 / 1000.0
        ));
    }
    if let Some((name, _)) = frames.last() {
        out.push_str(&format!("file '{name}'\noption framerate 1000\n"));
    }
    out
}

/// Write `replay` as an asciinema cast v2 file. Images appear as the half-block stand-ins their
/// cells hold, since a cast is text. Marks become cast markers, and a change of size a resize
/// event; meta events are left out.
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
        super::format::RecordingTarget::Ui {
            session: Some(session),
        } => format!("rozi {session}"),
        super::format::RecordingTarget::Ui { session: None }
        | super::format::RecordingTarget::Unknown => "rozi".to_string(),
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
            ReplayStep::Mark { t, label } => cast.push_marker(t as f64 / 1000.0, label),
            ReplayStep::End(summary) => end = Some(summary.t),
            ReplayStep::Meta { .. } => {}
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
