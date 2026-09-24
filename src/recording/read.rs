//! Reading a recording back: its header, its events, and the frames they add up to.

use std::collections::HashMap;
use std::io::BufRead;

use base64::Engine as _;

use super::encode::apply_delta;
use super::format::{
    RECORDING_FORMAT, RECORDING_VERSION, RecordedImage, RecordingEnd, RecordingEvent,
    RecordingHeader, RecordingMeta,
};
use super::frame::DecodedImage;
use crate::control::SpanFrame;

/// A recording's events, in order, one line at a time.
///
/// A last line with no newline is what a recording cut short leaves behind, so it is skipped and
/// [`Self::truncated`] reports it; everything before it reads normally.
pub struct RecordingReader<R> {
    reader: R,
    header: RecordingHeader,
    line: usize,
    truncated: bool,
}

impl<R: BufRead> RecordingReader<R> {
    pub fn new(mut reader: R) -> Result<Self, String> {
        let mut first = Vec::new();
        reader
            .read_until(b'\n', &mut first)
            .map_err(|error| format!("cannot read the recording: {error}"))?;
        if !first.ends_with(b"\n") {
            return Err("not a rozi recording: the header is incomplete".to_string());
        }
        let value: serde_json::Value = serde_json::from_slice(&first)
            .map_err(|_| "not a rozi recording: the first line is not JSON".to_string())?;
        if value.get("format").and_then(serde_json::Value::as_str) != Some(RECORDING_FORMAT) {
            return Err("not a rozi recording".to_string());
        }
        let header: RecordingHeader = serde_json::from_value(value)
            .map_err(|error| format!("invalid recording header: {error}"))?;
        if header.version > RECORDING_VERSION {
            return Err(format!(
                "recording version {} is newer than this rozi reads ({RECORDING_VERSION}); update rozi",
                header.version
            ));
        }
        if let Some(compression) = &header.compression {
            return Err(format!(
                "recording is compressed with `{compression}`, which this rozi cannot read"
            ));
        }
        Ok(Self {
            reader,
            header,
            line: 1,
            truncated: false,
        })
    }

    pub fn header(&self) -> &RecordingHeader {
        &self.header
    }

    /// Whether the file ended partway through a line.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The next event, or `None` at the end of the file.
    pub fn next_event(&mut self) -> Result<Option<RecordingEvent>, String> {
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let read = self
                .reader
                .read_until(b'\n', &mut buffer)
                .map_err(|error| format!("cannot read the recording: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            self.line += 1;
            if !buffer.ends_with(b"\n") {
                self.truncated = true;
                return Ok(None);
            }
            if buffer.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return serde_json::from_slice(&buffer)
                .map(Some)
                .map_err(|error| format!("line {}: {error}", self.line));
        }
    }
}

/// What a step of a [`Replay`] produced.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayStep {
    /// The screen changed; [`Replay::frame`] is the new one.
    Frame { t: u64 },
    Mark { t: u64, label: String },
    Meta { t: u64, meta: RecordingMeta },
    End(RecordingEnd),
}

/// A recording played forward event by event, keeping the current frame and every image so far.
pub struct Replay<R> {
    reader: RecordingReader<R>,
    frame: Option<SpanFrame>,
    images: HashMap<String, RecordedImage>,
    decoded: HashMap<String, DecodedImage>,
    /// The time of the last event with one, so an ended recording's length is known without an
    /// `end` event.
    last_t: u64,
}

impl<R: BufRead> Replay<R> {
    pub fn new(reader: R) -> Result<Self, String> {
        Ok(Self {
            reader: RecordingReader::new(reader)?,
            frame: None,
            images: HashMap::new(),
            decoded: HashMap::new(),
            last_t: 0,
        })
    }

    pub fn header(&self) -> &RecordingHeader {
        self.reader.header()
    }

    pub fn truncated(&self) -> bool {
        self.reader.truncated()
    }

    /// The screen as of the last [`ReplayStep::Frame`].
    pub fn frame(&self) -> Option<&SpanFrame> {
        self.frame.as_ref()
    }

    /// The time of the latest event read.
    pub fn elapsed(&self) -> u64 {
        self.last_t
    }

    /// Decoded pixels of every image the current frame names, decoding each only once.
    pub fn frame_images(&mut self) -> Result<&HashMap<String, DecodedImage>, String> {
        if let Some(frame) = &self.frame {
            for id in frame.images.iter().filter_map(|image| image.id.as_ref()) {
                if self.decoded.contains_key(id) {
                    continue;
                }
                let Some(image) = self.images.get(id) else {
                    continue;
                };
                let png = base64::engine::general_purpose::STANDARD
                    .decode(&image.png_base64)
                    .map_err(|error| format!("image {id}: {error}"))?;
                self.decoded
                    .insert(id.clone(), DecodedImage::from_png(&png)?);
            }
        }
        Ok(&self.decoded)
    }

    /// Read events until one says something, or the recording ends.
    pub fn step(&mut self) -> Result<Option<ReplayStep>, String> {
        while let Some(event) = self.reader.next_event()? {
            if let Some(t) = event.time() {
                self.last_t = self.last_t.max(t);
            }
            match event {
                RecordingEvent::Keyframe { t, frame } => {
                    self.frame = Some(frame);
                    return Ok(Some(ReplayStep::Frame { t }));
                }
                RecordingEvent::Delta(delta) => {
                    let frame = self
                        .frame
                        .as_mut()
                        .ok_or_else(|| "a delta precedes the first keyframe".to_string())?;
                    apply_delta(frame, &delta)?;
                    return Ok(Some(ReplayStep::Frame { t: delta.t }));
                }
                RecordingEvent::Image(image) => {
                    self.images.insert(image.id.clone(), image);
                }
                RecordingEvent::Mark { t, label } => {
                    return Ok(Some(ReplayStep::Mark { t, label }));
                }
                RecordingEvent::Meta { t, meta } => {
                    return Ok(Some(ReplayStep::Meta { t, meta }));
                }
                RecordingEvent::End(end) => return Ok(Some(ReplayStep::End(end))),
                RecordingEvent::Resize { .. } | RecordingEvent::Unknown => {}
            }
        }
        Ok(None)
    }
}
