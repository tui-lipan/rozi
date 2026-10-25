use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::session::protocol::constants::{
    FRAME_KIND_ACQUISITION_ACK, FRAME_KIND_CONTROL_JSON, FRAME_KIND_PANE_INPUT,
    FRAME_KIND_PANE_OUTPUT, FRAME_KIND_PRESENTATION_ACK, FRAME_KIND_PRESENTATION_INTEREST,
    MAX_FRAME_SIZE, PANE_FRAME_HEADER_LEN, PANE_OUTPUT_METADATA_LEN, PANE_OUTPUT_SEMANTICS_LEN,
};
use crate::state::PaneId;

/// Producer bytes may only be superseded when the authoritative parser proves independence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputClass {
    #[default]
    Ordered,
    DependentGraphics {
        image: u32,
        incarnation: u64,
        revision: u64,
    },
    IndependentGraphics {
        image: u32,
        incarnation: u64,
        revision: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputMetadata {
    pub sequence: u64,
    pub coalesce_epoch: u64,
    pub class: OutputClass,
}

impl OutputMetadata {
    pub(crate) fn replacement_key(
        self,
        pane: PaneId,
        local: bool,
        generation: u64,
    ) -> Option<crate::session::queue::ReplacementKey> {
        let OutputClass::IndependentGraphics {
            image, incarnation, ..
        } = self.class
        else {
            return None;
        };
        Some(crate::session::queue::ReplacementKey {
            pane,
            local,
            generation,
            epoch: self.coalesce_epoch,
            image,
            incarnation,
        })
    }
}

/// A private immutable snapshot bound to an exact rewritten command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImmutableFileBinding {
    pub command_range: std::ops::Range<usize>,
    pub path: std::path::PathBuf,
    pub byte_len: usize,
    pub format: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMedia {
    pub acquisition_sequence: u64,
    pub files: Vec<ImmutableFileBinding>,
    /// Client-private resources acquired before deferred dispatch and acknowledgement.
    pub acquired: Vec<tui_lipan::TerminalOutputMedia>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcquisitionAck {
    pub local: bool,
    pub pane_id: PaneId,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationAck {
    pub local: bool,
    pub pane_id: PaneId,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanePresentationInterest {
    pub local: bool,
    pub pane_id: PaneId,
    pub generation: u64,
    pub visible: bool,
    pub focused: bool,
    pub frames_per_second: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame<C> {
    Control(C),
    PaneOutput {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        metadata: OutputMetadata,
        media: Option<OutputMedia>,
        bytes: Vec<u8>,
    },
    PresentationAcks(Vec<PresentationAck>),
    AcquisitionAcks(Vec<AcquisitionAck>),
    PresentationInterest(Vec<PanePresentationInterest>),
    PaneBytes {
        latency_sensitive: bool,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        bytes: Vec<u8>,
    },
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> std::io::Result<()> {
    write_control_frame(writer, value)
}

pub fn write_control_frame<W: Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_frame_body(writer, FRAME_KIND_CONTROL_JSON, &body)
}

pub fn write_pane_output_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    metadata: OutputMetadata,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_output_media_frame(writer, pane_id, generation, local, metadata, None, bytes)
}

pub fn write_pane_output_media_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    metadata: OutputMetadata,
    media: Option<&OutputMedia>,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(PANE_OUTPUT_METADATA_LEN + bytes.len());
    payload.extend_from_slice(&metadata.sequence.to_be_bytes());
    payload.extend_from_slice(&metadata.coalesce_epoch.to_be_bytes());
    let (class, image, incarnation, revision) = match metadata.class {
        OutputClass::Ordered => (0, 0, 0, 0),
        OutputClass::DependentGraphics {
            image,
            incarnation,
            revision,
        } => (1, image, incarnation, revision),
        OutputClass::IndependentGraphics {
            image,
            incarnation,
            revision,
        } => (2, image, incarnation, revision),
    };
    payload.push(class);
    payload.extend_from_slice(&image.to_be_bytes());
    payload.extend_from_slice(&incarnation.to_be_bytes());
    payload.extend_from_slice(&revision.to_be_bytes());
    debug_assert_eq!(payload.len(), PANE_OUTPUT_SEMANTICS_LEN);
    let bindings = media
        .map(|m| serde_json::to_vec(&m.files))
        .transpose()
        .map_err(std::io::Error::other)?
        .unwrap_or_default();
    payload.extend_from_slice(&media.map_or(0, |m| m.acquisition_sequence).to_be_bytes());
    payload.extend_from_slice(&(bindings.len() as u32).to_be_bytes());
    payload.extend_from_slice(&bindings);
    payload.extend_from_slice(bytes);
    write_pane_frame(
        writer,
        FRAME_KIND_PANE_OUTPUT,
        pane_id,
        generation,
        local,
        &payload,
    )
}

pub fn write_pane_input_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_input_with_priority(writer, pane_id, generation, local, true, bytes)
}

pub fn write_pane_input_with_priority<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    latency_sensitive: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut body = Vec::with_capacity(PANE_FRAME_HEADER_LEN + bytes.len());
    body.extend_from_slice(&pane_id.to_be_bytes());
    body.extend_from_slice(&generation.to_be_bytes());
    body.push(u8::from(local) | (u8::from(!latency_sensitive) << 1));
    body.extend_from_slice(bytes);
    write_frame_body(writer, FRAME_KIND_PANE_INPUT, &body)
}

fn write_pane_frame<W: Write>(
    writer: &mut W,
    kind: u8,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut body = Vec::with_capacity(PANE_FRAME_HEADER_LEN + bytes.len());
    body.extend_from_slice(&pane_id.to_be_bytes());
    body.extend_from_slice(&generation.to_be_bytes());
    body.push(u8::from(local));
    body.extend_from_slice(bytes);
    write_frame_body(writer, kind, &body)
}

fn write_frame_body<W: Write>(writer: &mut W, kind: u8, body: &[u8]) -> std::io::Result<()> {
    let frame_len = body.len().saturating_add(1);
    if frame_len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "frame length {} exceeds maximum {MAX_FRAME_SIZE}",
                frame_len
            ),
        ));
    }
    let len: u32 = frame_len
        .try_into()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(body)
}

pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> std::io::Result<T> {
    read_frame_with_limit(reader, MAX_FRAME_SIZE)
}

pub fn read_frame_with_limit<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
    max_size: usize,
) -> std::io::Result<T> {
    let mut len = [0; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds maximum {max_size}"),
        ));
    }
    if len == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty frame",
        ));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let (kind, payload) = body.split_first().expect("non-empty frame");
    if *kind != FRAME_KIND_CONTROL_JSON {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected control frame, got kind {kind}"),
        ));
    }
    serde_json::from_slice(payload).map_err(std::io::Error::other)
}

#[derive(Debug)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    max_size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameReadStatus {
    Read(usize),
    WouldBlock,
    Eof,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new(MAX_FRAME_SIZE)
    }
}

impl FrameDecoder {
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_size,
        }
    }

    pub fn read_from_status<R: Read>(
        &mut self,
        reader: &mut R,
    ) -> std::io::Result<FrameReadStatus> {
        let mut chunk = [0_u8; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => Ok(FrameReadStatus::Eof),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                Ok(FrameReadStatus::Read(n))
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(FrameReadStatus::WouldBlock)
            }
            Err(err) => Err(err),
        }
    }

    pub fn next_frame<T: for<'de> Deserialize<'de>>(
        &mut self,
    ) -> std::io::Result<Option<Frame<T>>> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buffer[..4].try_into().expect("slice length")) as usize;
        if len > self.max_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("frame length {len} exceeds maximum {}", self.max_size),
            ));
        }
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buffer[4..4 + len].to_vec();
        self.buffer.drain(..4 + len);
        decode_frame(&body).map(Some)
    }
}

fn decode_frame<T: for<'de> Deserialize<'de>>(body: &[u8]) -> std::io::Result<Frame<T>> {
    let Some((&kind, payload)) = body.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty frame",
        ));
    };
    match kind {
        FRAME_KIND_CONTROL_JSON => serde_json::from_slice(payload)
            .map(Frame::Control)
            .map_err(std::io::Error::other),
        FRAME_KIND_PANE_INPUT => decode_pane_frame(payload),
        FRAME_KIND_PANE_OUTPUT => decode_output_frame(payload),
        FRAME_KIND_PRESENTATION_ACK => decode_ack_frame(payload),
        FRAME_KIND_ACQUISITION_ACK if payload.len() <= 4096 * 21 => decode_ack_frame::<T>(payload)
            .map(|frame| {
                let Frame::PresentationAcks(acks) = frame else {
                    unreachable!()
                };
                Frame::AcquisitionAcks(
                    acks.into_iter()
                        .map(|a| AcquisitionAck {
                            local: a.local,
                            pane_id: a.pane_id,
                            generation: a.generation,
                            sequence: a.sequence,
                        })
                        .collect(),
                )
            }),
        FRAME_KIND_PRESENTATION_INTEREST => decode_interest_frame(payload),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown frame kind {kind}"),
        )),
    }
}

fn decode_pane_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if payload.len() < PANE_FRAME_HEADER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pane byte frame missing header",
        ));
    }
    let pane_id = u32::from_be_bytes(payload[..4].try_into().expect("slice length"));
    let generation = u64::from_be_bytes(payload[4..12].try_into().expect("slice length"));
    if payload[12] & !3 != 0 {
        return Err(invalid_frame("invalid input flags"));
    }
    let local = payload[12] & 1 != 0;
    let latency_sensitive = payload[12] & 2 == 0;
    Ok(Frame::PaneBytes {
        latency_sensitive,
        pane_id,
        local,
        generation,
        bytes: payload[PANE_FRAME_HEADER_LEN..].to_vec(),
    })
}

fn invalid_frame(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

fn decode_output_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if payload.get(12).is_some_and(|flags| *flags > 1) {
        return Err(invalid_frame("invalid output flags"));
    }
    let Frame::PaneBytes {
        latency_sensitive: _,
        pane_id,
        local,
        generation,
        bytes,
    } = decode_pane_frame::<T>(payload)?
    else {
        unreachable!()
    };
    if bytes.len() < PANE_OUTPUT_METADATA_LEN {
        return Err(invalid_frame("pane output missing semantic metadata"));
    }
    let read_u64 =
        |start| u64::from_be_bytes(bytes[start..start + 8].try_into().expect("checked header"));
    let class = decode_output_class(&bytes)?;
    let (media, start) = decode_output_media(&bytes)?;
    Ok(Frame::PaneOutput {
        pane_id,
        local,
        generation,
        metadata: OutputMetadata {
            sequence: read_u64(0),
            coalesce_epoch: read_u64(8),
            class,
        },
        media,
        bytes: bytes[start..].to_vec(),
    })
}

fn decode_output_class(bytes: &[u8]) -> std::io::Result<OutputClass> {
    let read_u64 =
        |start| u64::from_be_bytes(bytes[start..start + 8].try_into().expect("checked header"));
    let image = u32::from_be_bytes(bytes[17..21].try_into().expect("checked header"));
    let incarnation = read_u64(21);
    let revision = read_u64(29);
    match bytes[16] {
        0 if image == 0 && incarnation == 0 && revision == 0 => Ok(OutputClass::Ordered),
        1 if image != 0 && incarnation != 0 => Ok(OutputClass::DependentGraphics {
            image,
            incarnation,
            revision,
        }),
        2 if image != 0 && incarnation != 0 => Ok(OutputClass::IndependentGraphics {
            image,
            incarnation,
            revision,
        }),
        _ => Err(invalid_frame("invalid graphics output classification")),
    }
}

fn validate_file_bindings(
    files: &[ImmutableFileBinding],
    output_len: usize,
) -> std::io::Result<()> {
    let mut end = 0;
    for file in files {
        if !file.valid_after(end, output_len) {
            return Err(invalid_frame("invalid immutable file binding"));
        }
        end = file.command_range.end;
    }
    Ok(())
}

impl ImmutableFileBinding {
    fn valid_after(&self, end: usize, output_len: usize) -> bool {
        self.command_range.start >= end
            && self.command_range.start < self.command_range.end
            && self.command_range.end <= output_len
            && self.byte_len != 0
            && matches!(self.format, 24 | 32)
    }
}

fn decode_output_media(bytes: &[u8]) -> std::io::Result<(Option<OutputMedia>, usize)> {
    let sequence_end = PANE_OUTPUT_SEMANTICS_LEN + size_of::<u64>();
    let sequence = u64::from_be_bytes(
        bytes[PANE_OUTPUT_SEMANTICS_LEN..sequence_end]
            .try_into()
            .expect("checked header"),
    );
    let binding_len = u32::from_be_bytes(
        bytes[sequence_end..PANE_OUTPUT_METADATA_LEN]
            .try_into()
            .expect("checked header"),
    ) as usize;
    let start = PANE_OUTPUT_METADATA_LEN
        .checked_add(binding_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid_frame("invalid output media length"))?;
    let media = if binding_len == 0 {
        if sequence != 0 {
            return Err(invalid_frame("sequence without media"));
        }
        None
    } else {
        let files: Vec<ImmutableFileBinding> =
            serde_json::from_slice(&bytes[PANE_OUTPUT_METADATA_LEN..start])
                .map_err(std::io::Error::other)?;
        if files.is_empty() || files.len() > 64 || sequence == 0 {
            return Err(invalid_frame("invalid acquisition envelope"));
        }
        validate_file_bindings(&files, bytes.len() - start)?;
        Some(OutputMedia {
            acquisition_sequence: sequence,
            files,
            acquired: Vec::new(),
        })
    };
    Ok((media, start))
}

pub fn write_acquisition_acks<W: Write>(
    writer: &mut W,
    acks: &[AcquisitionAck],
) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(acks.len().saturating_mul(21));
    for ack in acks {
        payload.extend_from_slice(&ack.pane_id.to_be_bytes());
        payload.extend_from_slice(&ack.generation.to_be_bytes());
        payload.extend_from_slice(&ack.sequence.to_be_bytes());
        payload.push(u8::from(ack.local));
    }
    write_frame_body(writer, FRAME_KIND_ACQUISITION_ACK, &payload)
}

pub fn write_presentation_acks<W: Write>(
    writer: &mut W,
    acks: &[PresentationAck],
) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(acks.len().saturating_mul(21));
    for ack in acks {
        payload.extend_from_slice(&ack.pane_id.to_be_bytes());
        payload.extend_from_slice(&ack.generation.to_be_bytes());
        payload.extend_from_slice(&ack.sequence.to_be_bytes());
        payload.push(u8::from(ack.local));
    }
    write_frame_body(writer, FRAME_KIND_PRESENTATION_ACK, &payload)
}

fn decode_ack_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if !payload.len().is_multiple_of(21) {
        return Err(invalid_frame("invalid presentation ACK length"));
    }
    if payload.chunks_exact(21).any(|chunk| chunk[20] > 1) {
        return Err(invalid_frame("invalid presentation ACK flags"));
    }
    Ok(Frame::PresentationAcks(
        payload
            .chunks_exact(21)
            .map(|chunk| PresentationAck {
                local: chunk[20] != 0,
                pane_id: u32::from_be_bytes(chunk[..4].try_into().expect("checked chunk")),
                generation: u64::from_be_bytes(chunk[4..12].try_into().expect("checked chunk")),
                sequence: u64::from_be_bytes(chunk[12..20].try_into().expect("checked chunk")),
            })
            .collect(),
    ))
}

pub fn write_presentation_interest<W: Write>(
    writer: &mut W,
    interest: &[PanePresentationInterest],
) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(interest.len().saturating_mul(15));
    for pane in interest {
        payload.extend_from_slice(&pane.pane_id.to_be_bytes());
        payload.extend_from_slice(&pane.generation.to_be_bytes());
        payload.push(
            u8::from(pane.visible) | (u8::from(pane.focused) << 1) | (u8::from(pane.local) << 2),
        );
        payload.extend_from_slice(&pane.frames_per_second.to_be_bytes());
    }
    write_frame_body(writer, FRAME_KIND_PRESENTATION_INTEREST, &payload)
}

fn decode_interest_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if !payload.len().is_multiple_of(15) {
        return Err(invalid_frame("invalid presentation interest length"));
    }
    let mut interest = Vec::with_capacity(payload.len() / 15);
    for chunk in payload.chunks_exact(15) {
        if chunk[12] & !7 != 0 {
            return Err(invalid_frame("invalid presentation interest flags"));
        }
        let frames_per_second =
            u16::from_be_bytes(chunk[13..15].try_into().expect("checked chunk"));
        if frames_per_second == 0 || frames_per_second > 1000 {
            return Err(invalid_frame("invalid presentation rate"));
        }
        interest.push(PanePresentationInterest {
            local: chunk[12] & 4 != 0,
            pane_id: u32::from_be_bytes(chunk[..4].try_into().expect("checked chunk")),
            generation: u64::from_be_bytes(chunk[4..12].try_into().expect("checked chunk")),
            visible: chunk[12] & 1 != 0,
            focused: chunk[12] & 2 != 0,
            frames_per_second,
        });
    }
    Ok(Frame::PresentationInterest(interest))
}
