//! Turning a stream of frames into recording events: keyframes, row diffs, and stored images.

use std::collections::HashSet;
use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tui_lipan::prelude::*;
use tui_lipan::{CapturedFrame, CapturedImage};

use super::format::{
    FrameDelta, KEYFRAME_INTERVAL_MS, MAX_DELTAS_PER_KEYFRAME, RecordedImage, RecordingEvent,
    RowChange,
};
use crate::control::SpanFrame;

/// Decides, frame by frame, what a recording writes: nothing for an unchanged screen, a keyframe
/// first, after a resize, and periodically, and otherwise the rows that changed.
#[derive(Debug)]
pub struct FrameEncoder {
    previous: Option<SpanFrame>,
    last_keyframe: u64,
    deltas_since_keyframe: u32,
    keyframe_interval_ms: u64,
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new(KEYFRAME_INTERVAL_MS)
    }
}

impl FrameEncoder {
    pub fn new(keyframe_interval_ms: u64) -> Self {
        Self {
            previous: None,
            last_keyframe: 0,
            deltas_since_keyframe: 0,
            keyframe_interval_ms,
        }
    }

    /// The events that take a player from the previous frame to `frame`, shown at `t`.
    pub fn encode(&mut self, t: u64, frame: SpanFrame) -> Vec<RecordingEvent> {
        let Some(previous) = &self.previous else {
            return self.keyframe(t, frame, Vec::new());
        };
        if *previous == frame {
            return Vec::new();
        }
        if (previous.width, previous.height) != (frame.width, frame.height) {
            let resize = RecordingEvent::Resize {
                t,
                width: frame.width,
                height: frame.height,
            };
            return self.keyframe(t, frame, vec![resize]);
        }
        if t.saturating_sub(self.last_keyframe) >= self.keyframe_interval_ms
            || self.deltas_since_keyframe >= MAX_DELTAS_PER_KEYFRAME
        {
            return self.keyframe(t, frame, Vec::new());
        }
        let delta = diff(previous, &frame, t);
        self.deltas_since_keyframe += 1;
        self.previous = Some(frame);
        vec![RecordingEvent::Delta(delta)]
    }

    fn keyframe(
        &mut self,
        t: u64,
        frame: SpanFrame,
        mut events: Vec<RecordingEvent>,
    ) -> Vec<RecordingEvent> {
        self.last_keyframe = t;
        self.deltas_since_keyframe = 0;
        self.previous = Some(frame.clone());
        events.push(RecordingEvent::Keyframe { t, frame });
        events
    }
}

/// What differs between two frames of the same size.
fn diff(previous: &SpanFrame, next: &SpanFrame, t: u64) -> FrameDelta {
    FrameDelta {
        t,
        rows: next
            .rows
            .iter()
            .enumerate()
            .filter(|(y, row)| previous.rows.get(*y) != Some(*row))
            .flat_map(|(y, row)| match previous.rows.get(y) {
                Some(old) => super::rows::diff(y as u16, old, row, next.width),
                None => vec![RowChange {
                    y: y as u16,
                    runs: row.clone(),
                    partial: false,
                }],
            })
            .collect(),
        cursor: (previous.cursor != next.cursor).then(|| next.cursor.clone()),
        images: (previous.images != next.images).then(|| next.images.clone()),
        palette: (previous.palette != next.palette).then(|| next.palette.clone()),
    }
}

/// Apply `delta` to `frame`, the frame it was taken against.
pub fn apply_delta(frame: &mut SpanFrame, delta: &FrameDelta) -> std::result::Result<(), String> {
    for change in &delta.rows {
        let width = frame.width;
        let row = frame.rows.get_mut(usize::from(change.y)).ok_or_else(|| {
            format!(
                "delta replaces row {} of a frame {} tall",
                change.y, frame.height
            )
        })?;
        super::rows::apply(row, change, width);
    }
    if let Some(cursor) = &delta.cursor {
        frame.cursor.clone_from(cursor);
    }
    if let Some(images) = &delta.images {
        frame.images.clone_from(images);
    }
    if let Some(palette) = &delta.palette {
        frame.palette.clone_from(palette);
    }
    Ok(())
}

/// Stores each distinct image once, and names it in every frame that shows it.
#[derive(Debug, Default)]
pub struct ImageStore {
    stored: HashSet<String>,
    /// The previous frame's pixels and their ids. A still image keeps the same shared buffer from
    /// frame to frame, so it is recognized without hashing it again.
    previous: Vec<(Arc<[u8]>, String)>,
}

impl ImageStore {
    /// `captured` as a `rozi-spans` frame with each image named by content, plus an `image` event
    /// for every image not stored yet, which must be written before the frame.
    pub fn frame(
        &mut self,
        t: u64,
        captured: &CapturedFrame,
        palette: TerminalColorPalette,
    ) -> std::result::Result<(Vec<RecordingEvent>, SpanFrame), String> {
        let mut frame = crate::pane::spans::span_frame(captured, palette, false)
            .map_err(|response| response.error.unwrap_or_default())?;
        let mut events = Vec::new();
        let mut current = Vec::with_capacity(captured.images.len());
        for (image, span) in captured.images.iter().zip(frame.images.iter_mut()) {
            let id = match self
                .previous
                .iter()
                .find(|(rgba, _)| Arc::ptr_eq(rgba, &image.rgba))
            {
                Some((_, id)) => id.clone(),
                None => image_id(image),
            };
            if self.stored.insert(id.clone()) {
                let whole =
                    CapturedImage::new(image.area, image.width, image.height, image.rgba.clone());
                let png = whole
                    .to_png()
                    .map_err(|error| format!("cannot encode an image: {error}"))?;
                events.push(RecordingEvent::Image(RecordedImage {
                    t,
                    id: id.clone(),
                    pixel_width: image.width,
                    pixel_height: image.height,
                    png_base64: base64::engine::general_purpose::STANDARD.encode(png),
                }));
            }
            span.id = Some(id.clone());
            current.push((image.rgba.clone(), id));
        }
        self.previous = current;
        Ok((events, frame))
    }
}

/// A content id for an image's pixels: 128 bits of their SHA-256, in hex.
fn image_id(image: &CapturedImage) -> String {
    let mut hasher = Sha256::new();
    hasher.update(image.width.to_le_bytes());
    hasher.update(image.height.to_le_bytes());
    hasher.update(&image.rgba);
    hasher.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
