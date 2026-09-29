//! Reading a recording back: its header, its events, and the frames they add up to.
//!
//! A recording can come from anywhere, such as a bug report, so reading one is bounded: a line,
//! a frame, an image, and the images kept for later frames each have a ceiling well above anything
//! rozi writes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, Read as _};

use base64::Engine as _;

use super::encode::apply_delta;
use super::format::{
    RECORDING_FORMAT, RECORDING_VERSION, RecordedImage, RecordingEnd, RecordingEvent,
    RecordingHeader, RecordingMeta,
};
use super::frame::DecodedImage;
use crate::control::SpanFrame;

/// The longest header line a reader takes.
const MAX_HEADER_LINE: usize = 1024 * 1024;
/// The longest event line a reader takes: room for the largest image a pane holds, as base64.
pub const MAX_EVENT_LINE: usize = 64 * 1024 * 1024;
/// The most cells a frame, or an image's area in one, may cover.
pub const MAX_FRAME_CELLS: usize = 1 << 20;
/// The most bytes one image may take decoded.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// The most encoded image data a replay keeps for the frames to come, each image charged its
/// bookkeeping too. Past it the oldest images are forgotten, and a later frame showing one shows
/// its half-block stand-in.
pub const MAX_RETAINED_IMAGE_BYTES: usize = 256 * 1024 * 1024;
/// The most bytes the decoded images of one frame may take together. Images past it are left
/// out, and show as their half-block stand-ins: a frame can name many images, each cheap to store
/// compressed and within [`MAX_IMAGE_BYTES`] decoded.
pub const MAX_DECODED_FRAME_BYTES: usize = 256 * 1024 * 1024;
/// The longest image id a reader takes. rozi writes 32 hex digits.
const MAX_IMAGE_ID_LEN: usize = 64;
/// What keeping one image costs besides its id and pixels: its entry in the map and the queue.
const IMAGE_ENTRY_COST: usize = 256;

/// Read through the next newline into `buffer`, taking at most `max` bytes before it. `Err` names
/// a longer line.
fn read_line(reader: &mut impl BufRead, buffer: &mut Vec<u8>, max: usize) -> Result<usize, String> {
    let read = reader
        .take(max as u64 + 1)
        .read_until(b'\n', buffer)
        .map_err(|error| format!("cannot read the recording: {error}"))?;
    if buffer.len() > max && !buffer.ends_with(b"\n") {
        return Err(format!(
            "a line is longer than the {} MiB a recording line may be",
            max / (1024 * 1024)
        ));
    }
    Ok(read)
}

/// Refuse a frame too large to draw.
fn check_frame(frame: &SpanFrame) -> Result<(), String> {
    check_cells("a frame", frame.width, frame.height)?;
    check_images(&frame.images)
}

fn check_images(images: &[crate::control::SpanImage]) -> Result<(), String> {
    images
        .iter()
        .try_for_each(|image| check_cells("an image", image.width, image.height))
}

fn check_cells(what: &str, width: u16, height: u16) -> Result<(), String> {
    if usize::from(width) * usize::from(height) > MAX_FRAME_CELLS {
        return Err(format!(
            "{what} of {width}x{height} cells is larger than rozi reads"
        ));
    }
    Ok(())
}

/// What keeping `image` costs a replay: its pixels, its id in the map and in the queue, and the
/// entries themselves, so an image with no pixels is not free.
fn retained_cost(image: &RecordedImage) -> usize {
    image.png_base64.len() + 2 * image.id.len() + IMAGE_ENTRY_COST
}

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
        read_line(&mut reader, &mut first, MAX_HEADER_LINE)
            .map_err(|_| "not a rozi recording: the first line is too long".to_string())?;
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
        if header.spans_version > crate::control::SPAN_FRAME_VERSION {
            return Err(format!(
                "recording frames use rozi-spans version {}, newer than this rozi reads ({}); update rozi",
                header.spans_version,
                crate::control::SPAN_FRAME_VERSION
            ));
        }
        if let Some(compression) = &header.compression {
            return Err(format!(
                "recording is compressed with `{compression}`, which this rozi cannot read"
            ));
        }
        check_cells("a recording", header.width, header.height)?;
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
            let read = read_line(&mut self.reader, &mut buffer, MAX_EVENT_LINE)
                .map_err(|error| format!("line {}: {error}", self.line + 1))?;
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
    Frame {
        t: u64,
    },
    Mark {
        t: u64,
        label: String,
    },
    Meta {
        t: u64,
        meta: RecordingMeta,
    },
    End(RecordingEnd),
}

/// A recording played forward event by event, keeping the current frame and every image so far.
pub struct Replay<R> {
    reader: RecordingReader<R>,
    frame: Option<SpanFrame>,
    images: HashMap<String, RecordedImage>,
    /// `images`' ids, oldest first, and the base64 bytes they hold.
    image_order: VecDeque<String>,
    image_bytes: usize,
    image_budget: usize,
    /// Pixels of the images the last drawn frame showed.
    decoded: HashMap<String, DecodedImage>,
    decoded_budget: usize,
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
            image_order: VecDeque::new(),
            image_bytes: 0,
            image_budget: MAX_RETAINED_IMAGE_BYTES,
            decoded_budget: MAX_DECODED_FRAME_BYTES,
            decoded: HashMap::new(),
            last_t: 0,
        })
    }

    pub fn header(&self) -> &RecordingHeader {
        self.reader.header()
    }

    #[cfg(test)]
    pub(crate) fn with_image_budget(mut self, bytes: usize) -> Self {
        self.image_budget = bytes;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_decoded_budget(mut self, bytes: usize) -> Self {
        self.decoded_budget = bytes;
        self
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
        let ids: Vec<String> = self
            .frame
            .iter()
            .flat_map(|frame| frame.images.iter().filter_map(|image| image.id.clone()))
            .collect();
        self.decode(&ids)?;
        Ok(&self.decoded)
    }

    /// `frame`, one this replay has produced, as cells and pixels to draw.
    pub fn captured(&mut self, frame: &SpanFrame) -> Result<tui_lipan::CapturedFrame, String> {
        let ids: Vec<String> = frame
            .images
            .iter()
            .filter_map(|image| image.id.clone())
            .collect();
        self.decode(&ids)?;
        Ok(super::frame::captured_frame(frame, &self.decoded))
    }

    /// Decode the images `ids` names, and let go of the pixels of any other. Together they stay
    /// within the decoded budget; an image past it is left undecoded.
    fn decode(&mut self, ids: &[String]) -> Result<(), String> {
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        self.decoded.retain(|id, _| wanted.contains(id.as_str()));
        let mut used: usize = self.decoded.values().map(|image| image.rgba.len()).sum();
        for id in ids {
            if self.decoded.contains_key(id) {
                continue;
            }
            let Some(image) = self.images.get(id) else {
                continue;
            };
            let declared = u128::from(image.pixel_width) * u128::from(image.pixel_height) * 4;
            if used as u128 + declared > self.decoded_budget as u128 {
                continue;
            }
            let png = base64::engine::general_purpose::STANDARD
                .decode(&image.png_base64)
                .map_err(|error| format!("image {id}: {error}"))?;
            let decoded = DecodedImage::from_png(&png, MAX_IMAGE_BYTES)
                .map_err(|error| format!("image {id}: {error}"))?;
            // The declared size is the file's word; the pixels are what count.
            if used + decoded.rgba.len() > self.decoded_budget {
                continue;
            }
            used += decoded.rgba.len();
            self.decoded.insert(id.clone(), decoded);
        }
        Ok(())
    }

    /// Keep `image` for the frames that show it, forgetting the oldest past the budget.
    fn store(&mut self, image: RecordedImage) -> Result<(), String> {
        if image.id.len() > MAX_IMAGE_ID_LEN {
            return Err(format!(
                "an image id of {} bytes is longer than rozi reads",
                image.id.len()
            ));
        }
        let decoded = u128::from(image.pixel_width) * u128::from(image.pixel_height) * 4;
        if decoded > MAX_IMAGE_BYTES as u128 {
            return Err(format!(
                "image {} of {}x{} pixels is larger than rozi reads",
                image.id, image.pixel_width, image.pixel_height
            ));
        }
        if self.images.contains_key(&image.id) {
            return Ok(());
        }
        self.image_bytes += retained_cost(&image);
        self.image_order.push_back(image.id.clone());
        self.images.insert(image.id.clone(), image);
        while self.image_bytes > self.image_budget
            && let Some(oldest) = self.image_order.pop_front()
        {
            if let Some(forgotten) = self.images.remove(&oldest) {
                self.image_bytes -= retained_cost(&forgotten);
            }
            self.decoded.remove(&oldest);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retained_images(&self) -> usize {
        self.images.len()
    }

    /// Read events until one says something, or the recording ends.
    pub fn step(&mut self) -> Result<Option<ReplayStep>, String> {
        while let Some(event) = self.reader.next_event()? {
            if let Some(t) = event.time() {
                self.last_t = self.last_t.max(t);
            }
            match event {
                RecordingEvent::Keyframe { t, frame } => {
                    check_frame(&frame)?;
                    self.frame = Some(frame);
                    return Ok(Some(ReplayStep::Frame { t }));
                }
                RecordingEvent::Delta(delta) => {
                    let frame = self
                        .frame
                        .as_mut()
                        .ok_or_else(|| "a delta precedes the first keyframe".to_string())?;
                    if let Some(images) = &delta.images {
                        check_images(images)?;
                    }
                    apply_delta(frame, &delta)?;
                    return Ok(Some(ReplayStep::Frame { t: delta.t }));
                }
                RecordingEvent::Image(image) => self.store(image)?,
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
