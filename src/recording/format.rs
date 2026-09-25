//! The `rozi-recording` file format, version 1: its header and events.
//!
//! A recording is line-delimited JSON. The first line is a [`RecordingHeader`]; every later line is
//! one [`RecordingEvent`]. Lines are appended as the recording runs, so a file cut short by a crash
//! or a full disk is still readable up to its last complete line.

use serde::{Deserialize, Serialize};

use crate::control::{SpanCursor, SpanFrame, SpanImage, SpanPalette, SpanRun};
use crate::state::PaneId;

/// [`RecordingHeader::format`]: what a reader checks before anything else.
pub const RECORDING_FORMAT: &str = "rozi-recording";
/// [`RecordingHeader::version`]. It moves only when a field changes meaning or goes away: a reader
/// ignores fields and event kinds it does not know. Independent of the control API version and of
/// the `rozi-spans` version the frames use.
pub const RECORDING_VERSION: u32 = 1;
/// The longest a recording goes between keyframes while its screen keeps changing.
pub const KEYFRAME_INTERVAL_MS: u64 = 10_000;
/// The most deltas written between two keyframes, however little time they span.
pub const MAX_DELTAS_PER_KEYFRAME: u32 = 1_000;

/// The first line of a recording.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RecordingHeader {
    /// Always `"rozi-recording"`.
    pub format: String,
    /// The recording format's version, currently 1.
    pub version: u32,
    /// The version of rozi that wrote the file.
    pub rozi: String,
    pub target: RecordingTarget,
    /// The recorded screen's size in cells when the recording started. A `resize` event changes it.
    pub width: u16,
    pub height: u16,
    pub started_at_unix_ms: u64,
    /// The most frames a second the recorder writes. Changes arriving faster are coalesced into the
    /// latest state per interval.
    pub max_fps: u32,
    /// The longest gap between keyframes while the screen keeps changing.
    pub keyframe_interval_ms: u64,
    /// The `rozi-spans` version of every frame in the file.
    pub spans_version: u32,
    /// The colors the screen was drawn in when the recording started, so a player can show what the
    /// user saw. A frame carries its own palette, which wins when the two differ.
    pub palette: SpanPalette,
    /// How the lines after this one are compressed. Absent means they are plain text; a reader
    /// refuses a value it does not know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
}

/// What a recording shows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum RecordingTarget {
    /// One pane's terminal screen, as the session server holds it: the canonical screen
    /// `capture-pane --session` sees, without any client's chrome.
    Pane { session: String, pane: PaneId },
}

/// One line after the header. Every event carries `t`, the milliseconds since the recording
/// started, and events are in time order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum RecordingEvent {
    /// The whole screen. Written first, after a resize, and periodically, so a player can start
    /// from any keyframe.
    Keyframe { t: u64, frame: SpanFrame },
    /// What changed since the previous frame.
    Delta(FrameDelta),
    /// The screen changed size. A keyframe at the new size follows.
    Resize { t: u64, width: u16, height: u16 },
    /// Pixels of an image a frame shows, written once per distinct content before the first frame
    /// that references it by `id`.
    Image(RecordedImage),
    /// A label someone added with `rozi record mark`.
    Mark { t: u64, label: String },
    /// Something the session server noticed about the pane.
    Meta {
        t: u64,
        #[serde(flatten)]
        meta: RecordingMeta,
    },
    /// The last event of a finished recording.
    End(RecordingEnd),
    /// An event kind this version does not know. Readers skip it.
    #[serde(other)]
    Unknown,
}

impl RecordingEvent {
    /// Milliseconds since the start, for the events that carry it.
    pub fn time(&self) -> Option<u64> {
        match self {
            Self::Keyframe { t, .. }
            | Self::Resize { t, .. }
            | Self::Mark { t, .. }
            | Self::Meta { t, .. } => Some(*t),
            Self::Delta(delta) => Some(delta.t),
            Self::Image(image) => Some(image.t),
            Self::End(end) => Some(end.t),
            Self::Unknown => None,
        }
    }
}

/// The rows, cursor, images, and palette that changed since the previous frame. A field that is
/// absent did not change.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct FrameDelta {
    pub t: u64,
    /// Replaced rows, or the parts of them that changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowChange>,
    /// The new cursor, or `null` when the frame no longer has one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(feature = "schema-gen", schemars(with = "Option<SpanCursor>"))]
    pub cursor: Option<Option<SpanCursor>>,
    /// The frame's whole image list, when anything about its images changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<SpanImage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<SpanPalette>,
}

/// A field that is present, even as `null`, is `Some`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A replaced row, or part of one.
///
/// Whole, `runs` is the row as a `rozi-spans` frame writes it. `partial` replaces only the columns
/// the runs cover, from the first run's `x` to the end of the last; those runs tile that range
/// without gaps, blanks included. A row can have several partial changes in one delta.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RowChange {
    pub y: u16,
    pub runs: Vec<SpanRun>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// An image's pixels, stored once for however many frames show them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RecordedImage {
    pub t: u64,
    /// The content id frames name in a `rozi-spans` image's `id`: a hash of the pixels.
    pub id: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// The pixels as a base64 PNG with alpha, uncropped by whatever covers the image in a frame.
    pub png_base64: String,
}

/// Semantic events the session server already tracks for a pane.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum RecordingMeta {
    /// The pane's title changed.
    Title { title: Option<String> },
    /// Shell integration reported a command starting.
    CommandStarted,
    /// Shell integration reported a command finishing.
    CommandFinished { status: Option<i32> },
    /// The pane's detected agent, or its state, changed.
    Agent {
        agent: Option<String>,
        state: Option<String>,
    },
    /// A script set or cleared the pane's status.
    Status { status: Option<String> },
    /// The pane's program exited.
    Exited { status: i32 },
    #[serde(other)]
    Unknown,
}

/// Why a recording ended, and what it holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RecordingEnd {
    pub t: u64,
    pub reason: EndReason,
    #[serde(flatten)]
    pub totals: RecordingTotals,
}

/// Counts a recording keeps while it runs.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RecordingTotals {
    /// Frames written: keyframes and deltas.
    pub frames: u64,
    pub keyframes: u64,
    pub deltas: u64,
    /// Distinct images stored.
    pub images: u64,
    pub marks: u64,
    /// Changes the recorder never wrote because it had fallen behind and kept only the newest
    /// state. Zero unless the disk could not keep up.
    pub dropped: u64,
    /// Bytes written before the `end` event.
    pub bytes: u64,
}

/// Why a recording stopped.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum EndReason {
    /// `rozi record stop`, or the foreground `rozi record pane` exiting.
    #[default]
    Stopped,
    /// It reached its `--duration`.
    Duration,
    /// It reached its `--max-bytes`.
    MaxBytes,
    /// The pane's program exited.
    PaneExited,
    /// The pane was closed or replaced.
    PaneClosed,
    /// The session was killed.
    SessionEnded,
    /// The session server shut down.
    ServerShutdown,
    /// Writing the file failed, such as on a full disk. The file holds what was written before.
    WriteFailed,
    #[serde(other)]
    Unknown,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Duration => "duration",
            Self::MaxBytes => "max-bytes",
            Self::PaneExited => "pane-exited",
            Self::PaneClosed => "pane-closed",
            Self::SessionEnded => "session-ended",
            Self::ServerShutdown => "server-shutdown",
            Self::WriteFailed => "write-failed",
            Self::Unknown => "unknown",
        }
    }
}
