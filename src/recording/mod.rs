//! Recordings: a screen's changes over time, written as they happen and read back later.
//!
//! A domain core of its own because both ends of it are layer-independent: the session server
//! writes a recording from its pane screens with no client involved, and the CLI exports and plays
//! one with no session at all. The format is `rozi-recording` ([`format`]), line-delimited JSON
//! whose frames are `rozi-spans` frames.

pub mod encode;
pub mod export;
pub mod format;
pub mod frame;
pub mod read;
mod rows;
pub mod writer;

#[cfg(test)]
mod tests;

pub use format::{
    EndReason, RECORDING_FORMAT, RECORDING_VERSION, RecordingEnd, RecordingEvent, RecordingHeader,
    RecordingMeta, RecordingTarget, RecordingTotals,
};
pub use read::{RecordingReader, Replay, ReplayStep};
pub use writer::{Recorder, RecorderOptions, RecorderOutcome};
