//! Starting a recording: its limits, its file, and its header.
//!
//! Shared by the two recorders, so a pane recording and a UI recording take the same limits, name
//! their files the same way, and refuse the same things with the same words: the session server
//! records a pane on its host, and a UI records itself on the UI's host.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::RecordingConfig;
use crate::control::{ControlErrorCode, ControlResponse, SpanFrame};

use super::format::{KEYFRAME_INTERVAL_MS, RECORDING_FORMAT, RECORDING_VERSION};
use super::writer::MIN_MAX_BYTES;
use super::{Recorder, RecorderOptions, RecordingHeader, RecordingTarget};

/// Names a recorder tries for a file it names itself before giving up on the directory.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// A recording's limits, checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_fps: u32,
    pub duration_ms: u64,
    pub max_bytes: u64,
}

impl Limits {
    /// The limits a start asked for, with `defaults` filling what it left out.
    pub fn resolve(
        max_fps: Option<u32>,
        duration_ms: Option<u64>,
        max_bytes: Option<u64>,
        defaults: &RecordingConfig,
    ) -> Result<Self, ControlResponse> {
        let invalid = |message: String| {
            ControlResponse::error_with(ControlErrorCode::InvalidArgument, message)
        };
        let max_fps = max_fps.unwrap_or(defaults.max_fps);
        if !(1..=crate::control::MAX_RECORDING_MAX_FPS).contains(&max_fps) {
            return Err(invalid(format!(
                "max fps must be from 1 to {}",
                crate::control::MAX_RECORDING_MAX_FPS
            )));
        }
        let duration_ms = duration_ms.unwrap_or(defaults.duration_ms);
        if !(1..=crate::control::MAX_RECORDING_DURATION_MS).contains(&duration_ms) {
            return Err(invalid("a recording lasts from 1ms to 7 days".to_string()));
        }
        let max_bytes = max_bytes.unwrap_or(defaults.max_bytes);
        if max_bytes < MIN_MAX_BYTES {
            return Err(invalid(format!(
                "max bytes must be at least {} KiB",
                MIN_MAX_BYTES / 1024
            )));
        }
        Ok(Self {
            max_fps,
            duration_ms,
            max_bytes,
        })
    }
}

/// The shortest gap between two frames at `max_fps`, rounded up so the ceiling is never exceeded.
pub fn frame_interval(max_fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(max_fps.max(1))))
}

/// Where a recording is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// The file the request named.
    File(PathBuf),
    /// A new file the recorder names: `<stem>.rozirec` in `dir`, or `<stem>-2.rozirec`, … when
    /// that is taken.
    Named { dir: PathBuf, stem: String },
}

/// The directory a recording its recorder names goes in: `configured`, else `recordings` in the
/// state directory, created private when rozi makes it.
pub fn recording_dir(configured: Option<&Path>) -> Result<PathBuf, ControlResponse> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    crate::platform::paths::recording_dir(&env, configured).map_err(|error| {
        let what = match configured {
            Some(dir) => format!("cannot use {}", dir.display()),
            None => "cannot create the recordings directory".to_string(),
        };
        ControlResponse::error_with(ControlErrorCode::RequestFailed, format!("{what}: {error}"))
    })
}

/// A file name stem stamped with `now` to the second, in local time: `<prefix>-<stamp>`.
pub fn stem(prefix: &str, now: chrono::DateTime<chrono::Local>) -> String {
    format!("{prefix}-{}", now.format("%Y%m%d-%H%M%S"))
}

/// The header of a recording of `target` whose first frame is `first`.
pub fn header(
    target: RecordingTarget,
    first: &SpanFrame,
    max_fps: u32,
    started_at_unix_ms: u64,
) -> RecordingHeader {
    RecordingHeader {
        format: RECORDING_FORMAT.to_string(),
        version: RECORDING_VERSION,
        rozi: env!("CARGO_PKG_VERSION").to_string(),
        target,
        width: first.width,
        height: first.height,
        started_at_unix_ms,
        max_fps,
        keyframe_interval_ms: KEYFRAME_INTERVAL_MS,
        spans_version: crate::control::SPAN_FRAME_VERSION,
        palette: first.palette.clone(),
        compression: None,
    }
}

/// Create the recording's file, write `header`, and start its writer.
pub fn start(
    output: Output,
    force: bool,
    header: &RecordingHeader,
    max_bytes: u64,
) -> Result<(PathBuf, Recorder), ControlResponse> {
    let start = |path: &Path, overwrite: bool| {
        Recorder::start(RecorderOptions {
            path: path.to_path_buf(),
            overwrite,
            header: header.clone(),
            max_bytes,
        })
    };
    let cannot_create = |path: &Path, error: io::Error| {
        ControlResponse::error_with(
            ControlErrorCode::RequestFailed,
            format!("cannot create {}: {error}", path.display()),
        )
    };
    match output {
        Output::File(path) => match start(&path, force) {
            Ok(recorder) => Ok((path, recorder)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(ControlResponse::error_with(
                    ControlErrorCode::Conflict,
                    format!(
                        "{} already exists; pass --force to replace it",
                        path.display()
                    ),
                ))
            }
            Err(error) => Err(cannot_create(&path, error)),
        },
        Output::Named { dir, stem } => {
            for attempt in 1..=MAX_NAME_ATTEMPTS {
                let path = if attempt == 1 {
                    dir.join(format!("{stem}.rozirec"))
                } else {
                    dir.join(format!("{stem}-{attempt}.rozirec"))
                };
                match start(&path, false) {
                    Ok(recorder) => return Ok((path, recorder)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(cannot_create(&path, error)),
                }
            }
            Err(ControlResponse::error_with(
                ControlErrorCode::Conflict,
                format!("no free file name for {stem}.rozirec in {}", dir.display()),
            ))
        }
    }
}

/// A mark's label as a recording keeps it: display-safe, trimmed, and at most
/// [`crate::control::MAX_RECORDING_MARK_CHARS`] characters. `None` when nothing is left.
pub fn mark_label(label: &str) -> Option<String> {
    let label: String = tui_lipan::utils::sanitize_display_text(label)
        .trim()
        .chars()
        .take(crate::control::MAX_RECORDING_MARK_CHARS)
        .collect();
    (!label.is_empty()).then_some(label)
}
