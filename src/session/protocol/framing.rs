use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::session::protocol::constants::{
    FRAME_KIND_CONTROL_JSON, FRAME_KIND_PANE_INPUT, FRAME_KIND_PANE_OUTPUT,
    FRAME_KIND_PRESENTATION_ACK, FRAME_KIND_PRESENTATION_INTEREST, MAX_FRAME_SIZE,
    PANE_FRAME_HEADER_LEN, PANE_OUTPUT_METADATA_LEN,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationAck {
    pub pane_id: PaneId,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanePresentationInterest {
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
        bytes: Vec<u8>,
    },
    PresentationAcks(Vec<PresentationAck>),
    PresentationInterest(Vec<PanePresentationInterest>),
    PaneBytes {
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
    write_pane_frame(
        writer,
        FRAME_KIND_PANE_INPUT,
        pane_id,
        generation,
        local,
        bytes,
    )
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
    let local = match payload[12] {
        0 => false,
        1 => true,
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("pane byte frame has invalid locality {other}"),
            ));
        }
    };
    Ok(Frame::PaneBytes {
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
    let Frame::PaneBytes {
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
    let image = u32::from_be_bytes(bytes[17..21].try_into().expect("checked header"));
    let incarnation = read_u64(21);
    let revision = read_u64(29);
    let class = match bytes[16] {
        0 if image == 0 && incarnation == 0 && revision == 0 => OutputClass::Ordered,
        1 if image != 0 && incarnation != 0 => OutputClass::DependentGraphics {
            image,
            incarnation,
            revision,
        },
        2 if image != 0 && incarnation != 0 => OutputClass::IndependentGraphics {
            image,
            incarnation,
            revision,
        },
        _ => return Err(invalid_frame("invalid graphics output classification")),
    };
    Ok(Frame::PaneOutput {
        pane_id,
        local,
        generation,
        metadata: OutputMetadata {
            sequence: read_u64(0),
            coalesce_epoch: read_u64(8),
            class,
        },
        bytes: bytes[PANE_OUTPUT_METADATA_LEN..].to_vec(),
    })
}

pub fn write_presentation_acks<W: Write>(
    writer: &mut W,
    acks: &[PresentationAck],
) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(acks.len().saturating_mul(20));
    for ack in acks {
        payload.extend_from_slice(&ack.pane_id.to_be_bytes());
        payload.extend_from_slice(&ack.generation.to_be_bytes());
        payload.extend_from_slice(&ack.sequence.to_be_bytes());
    }
    write_frame_body(writer, FRAME_KIND_PRESENTATION_ACK, &payload)
}

fn decode_ack_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if !payload.len().is_multiple_of(20) {
        return Err(invalid_frame("invalid presentation ACK length"));
    }
    Ok(Frame::PresentationAcks(
        payload
            .chunks_exact(20)
            .map(|chunk| PresentationAck {
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
        payload.push(u8::from(pane.visible) | (u8::from(pane.focused) << 1));
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
        if chunk[12] & !3 != 0 {
            return Err(invalid_frame("invalid presentation interest flags"));
        }
        let frames_per_second =
            u16::from_be_bytes(chunk[13..15].try_into().expect("checked chunk"));
        if frames_per_second == 0 || frames_per_second > 1000 {
            return Err(invalid_frame("invalid presentation rate"));
        }
        interest.push(PanePresentationInterest {
            pane_id: u32::from_be_bytes(chunk[..4].try_into().expect("checked chunk")),
            generation: u64::from_be_bytes(chunk[4..12].try_into().expect("checked chunk")),
            visible: chunk[12] & 1 != 0,
            focused: chunk[12] & 2 != 0,
            frames_per_second,
        });
    }
    Ok(Frame::PresentationInterest(interest))
}
