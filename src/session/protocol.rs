use std::io::{Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::state::PaneId;

pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
const FRAME_KIND_CONTROL_JSON: u8 = 1;
const FRAME_KIND_PANE_OUTPUT: u8 = 2;
const FRAME_KIND_PANE_INPUT: u8 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireSpan {
    pub content: String,
    pub style: Style,
    pub allow_row_style: bool,
}

impl From<&Span> for WireSpan {
    fn from(span: &Span) -> Self {
        Self {
            content: span.content.to_string(),
            style: span.style,
            allow_row_style: span.allow_row_style,
        }
    }
}

impl From<WireSpan> for Span {
    fn from(span: WireSpan) -> Self {
        Span::new(Arc::<str>::from(span.content))
            .style(span.style)
            .allow_row_style(span.allow_row_style)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireSnapshot {
    pub version: u32,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub text: String,
    pub color_lines: Vec<Vec<WireSpan>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub sequence: u64,
    pub scrollback_offset: usize,
    pub total_scrollback_rows: usize,
    pub mouse_mode: MouseModeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePalette {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub ansi: [Color; 16],
}

impl From<TerminalColorPalette> for WirePalette {
    fn from(palette: TerminalColorPalette) -> Self {
        Self {
            foreground: palette.foreground,
            background: palette.background,
            ansi: palette.ansi,
        }
    }
}

impl From<WirePalette> for TerminalColorPalette {
    fn from(palette: WirePalette) -> Self {
        Self {
            foreground: palette.foreground,
            background: palette.background,
            ansi: palette.ansi,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSearchMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttachedPane {
    pub pane_id: PaneId,
    pub generation: u64,
    pub snapshot: WireSnapshot,
    pub exited: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotVersionError {
    pub found: u32,
    pub expected: u32,
}

impl std::fmt::Display for SnapshotVersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unsupported wire snapshot version {}, expected {}",
            self.found, self.expected
        )
    }
}

impl std::error::Error for SnapshotVersionError {}

impl WireSnapshot {
    pub fn from_snapshot(
        title: Option<String>,
        cwd: Option<String>,
        snapshot: &TerminalRenderSnapshot,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            title,
            cwd,
            text: snapshot.text.to_string(),
            color_lines: snapshot
                .color_lines
                .iter()
                .map(|line| line.iter().map(WireSpan::from).collect())
                .collect(),
            cursor_row: snapshot.cursor_row,
            cursor_col: snapshot.cursor_col,
            cursor_visible: snapshot.cursor_visible,
            sequence: snapshot.sequence,
            scrollback_offset: snapshot.scrollback_offset,
            total_scrollback_rows: snapshot.total_scrollback_rows,
            mouse_mode: snapshot.mouse_mode,
        }
    }

    #[allow(dead_code)]
    pub fn try_into_snapshot(
        self,
    ) -> std::result::Result<TerminalRenderSnapshot, SnapshotVersionError> {
        if self.version != PROTOCOL_VERSION {
            return Err(SnapshotVersionError {
                found: self.version,
                expected: PROTOCOL_VERSION,
            });
        }
        Ok(TerminalRenderSnapshot::from_parts(
            self.text,
            self.color_lines
                .into_iter()
                .map(|line| line.into_iter().map(Span::from).collect())
                .collect(),
            self.cursor_row,
            self.cursor_col,
            self.cursor_visible,
            self.sequence,
            self.scrollback_offset,
            self.total_scrollback_rows,
            self.mouse_mode,
        ))
    }
}

impl From<&TerminalRenderSnapshot> for WireSnapshot {
    fn from(snapshot: &TerminalRenderSnapshot) -> Self {
        Self::from_snapshot(None, None, snapshot)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Attach {
        session: String,
        protocol_version: u32,
    },
    SpawnPane {
        pane_id: PaneId,
        generation: u64,
        command: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        keep_open: bool,
        env: Vec<(String, String)>,
        title: Option<String>,
    },
    Resize {
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    Scroll {
        pane_id: PaneId,
        generation: u64,
        offset: usize,
    },
    Kill {
        pane_id: PaneId,
        generation: u64,
    },
    Search {
        request_id: u64,
        pane_id: PaneId,
        generation: u64,
        query: String,
    },
    SetPalette {
        pane_id: PaneId,
        generation: u64,
        palette: WirePalette,
    },
    ConfigurePane {
        pane_id: PaneId,
        generation: u64,
        palette: Option<WirePalette>,
        title: Option<String>,
        cwd: Option<String>,
    },
    PushLayout {
        blob: String,
    },
    Detach,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerMessage {
    Error {
        code: String,
        message: String,
    },
    Attached {
        protocol_version: u32,
        session: String,
        panes: Vec<AttachedPane>,
        layout_blob: Option<String>,
    },
    Snapshot {
        pane_id: PaneId,
        generation: u64,
        snapshot: WireSnapshot,
    },
    Exited {
        pane_id: PaneId,
        generation: u64,
        code: i32,
    },
    Bell {
        pane_id: PaneId,
        generation: u64,
    },
    SearchResult {
        request_id: u64,
        pane_id: PaneId,
        generation: u64,
        query: String,
        matches: Vec<WireSearchMatch>,
    },
    SpawnResult {
        pane_id: PaneId,
        generation: u64,
        ok: bool,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame<C> {
    Control(C),
    PaneBytes {
        pane_id: PaneId,
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

#[allow(dead_code)]
pub fn write_pane_output_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_frame(writer, FRAME_KIND_PANE_OUTPUT, pane_id, generation, bytes)
}

pub fn write_pane_input_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_frame(writer, FRAME_KIND_PANE_INPUT, pane_id, generation, bytes)
}

fn write_pane_frame<W: Write>(
    writer: &mut W,
    kind: u8,
    pane_id: PaneId,
    generation: u64,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut body = Vec::with_capacity(12 + bytes.len());
    body.extend_from_slice(&pane_id.to_be_bytes());
    body.extend_from_slice(&generation.to_be_bytes());
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
        FRAME_KIND_PANE_OUTPUT | FRAME_KIND_PANE_INPUT => decode_pane_frame(payload),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown frame kind {kind}"),
        )),
    }
}

fn decode_pane_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if payload.len() < 12 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pane byte frame missing header",
        ));
    }
    let pane_id = u32::from_be_bytes(payload[..4].try_into().expect("slice length"));
    let generation = u64::from_be_bytes(payload[4..12].try_into().expect("slice length"));
    Ok(Frame::PaneBytes {
        pane_id,
        generation,
        bytes: payload[12..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_frame_round_trips() {
        let msg = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn protocol_attach_shape_round_trips() {
        let msg = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(9_u32).to_be_bytes());
        buf.extend_from_slice(b"{}");
        let err = read_frame_with_limit::<_, ClientMessage>(&mut &buf[..], 8).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn oversized_write_frame_is_rejected() {
        let msg = ClientMessage::SpawnPane {
            pane_id: 1,
            generation: 1,
            command: Some("x".repeat(MAX_FRAME_SIZE)),
            cwd: None,
            cols: 80,
            rows: 24,
            keep_open: false,
            env: Vec::new(),
            title: None,
        };
        let err = write_frame(&mut Vec::new(), &msg).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn frame_decoder_preserves_partial_bytes_until_complete() {
        let msg = ClientMessage::Detach;
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &msg).unwrap();
        let split = 6.min(encoded.len() - 1);
        let mut decoder = FrameDecoder::default();
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[..split]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert!(decoder.next_frame::<ClientMessage>().unwrap().is_none());
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[split..]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::Control(msg))
        );
    }

    #[test]
    fn wire_snapshot_converts_to_render_snapshot() {
        let snapshot = TerminalRenderSnapshot::from_parts(
            "hello",
            vec![vec![Span::new("hello").style(Style::new().bold())]],
            0,
            4,
            true,
            12,
            1,
            5,
            MouseModeState::default(),
        );
        let wire =
            WireSnapshot::from_snapshot(Some("title".into()), Some("/tmp".into()), &snapshot);
        let restored = wire.clone().try_into_snapshot().unwrap();
        assert_eq!(wire.title.as_deref(), Some("title"));
        assert_eq!(restored.text.as_ref(), "hello");
        assert_eq!(restored.cursor_col, 4);
        assert_eq!(restored.color_lines[0][0].content.as_ref(), "hello");
    }

    #[test]
    fn wire_snapshot_version_mismatch_is_error() {
        let mut wire = WireSnapshot::from_snapshot(None, None, &TerminalRenderSnapshot::default());
        wire.version = PROTOCOL_VERSION + 1;
        assert!(wire.try_into_snapshot().is_err());
    }

    #[test]
    fn golden_client_attach_json_shape() {
        let value = serde_json::to_value(ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"attach","session":"dev","protocol_version":2})
        );
    }

    #[test]
    fn binary_pane_frame_has_golden_shape() {
        let mut buf = Vec::new();
        write_pane_output_frame(&mut buf, 7, 9, b"abc").unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&16_u32.to_be_bytes());
        expected.push(FRAME_KIND_PANE_OUTPUT);
        expected.extend_from_slice(&7_u32.to_be_bytes());
        expected.extend_from_slice(&9_u64.to_be_bytes());
        expected.extend_from_slice(b"abc");

        assert_eq!(buf, expected);
    }

    #[test]
    fn frame_decoder_decodes_interleaved_control_and_binary_frames() {
        let attach = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &attach).unwrap();
        write_pane_input_frame(&mut encoded, 7, 9, b"abc").unwrap();

        let mut decoder = FrameDecoder::default();
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[..]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::Control(attach))
        );
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::PaneBytes {
                pane_id: 7,
                generation: 9,
                bytes: b"abc".to_vec(),
            })
        );
        assert_eq!(decoder.next_frame::<ClientMessage>().unwrap(), None);
    }

    #[test]
    fn golden_client_spawn_json_shape() {
        let value = serde_json::to_value(ClientMessage::SpawnPane {
            pane_id: 7,
            generation: 9,
            command: Some("bash".into()),
            cwd: Some("/repo".into()),
            cols: 80,
            rows: 24,
            keep_open: true,
            env: vec![("A".into(), "B".into())],
            title: Some("shell".into()),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"spawn-pane","pane_id":7,"generation":9,"command":"bash","cwd":"/repo","cols":80,"rows":24,"keep_open":true,"env":[["A","B"]],"title":"shell"})
        );
    }

    #[test]
    fn golden_server_messages_json_shape() {
        let snapshot = WireSnapshot::from_snapshot(None, None, &TerminalRenderSnapshot::default());
        assert_eq!(
            serde_json::to_value(ServerMessage::Snapshot {
                pane_id: 1,
                generation: 2,
                snapshot
            })
            .unwrap()["type"],
            "snapshot"
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::SearchResult {
                request_id: 4,
                pane_id: 1,
                generation: 2,
                query: "abc".into(),
                matches: vec![WireSearchMatch {
                    offset: 0,
                    line: 1,
                    start_col: 2,
                    end_col: 5,
                    text: "xxabc".into()
                }],
            })
            .unwrap(),
            serde_json::json!({"type":"search-result","request_id":4,"pane_id":1,"generation":2,"query":"abc","matches":[{"offset":0,"line":1,"start_col":2,"end_col":5,"text":"xxabc"}]})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::Error {
                code: "bad".into(),
                message: "no".into()
            })
            .unwrap(),
            serde_json::json!({"type":"error","code":"bad","message":"no"})
        );
    }
}
