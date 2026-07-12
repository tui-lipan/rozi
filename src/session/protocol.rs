use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

/// Bumped 7 -> 8 for the cross-platform plan's launch-policy and runtime-state changes (Phases
/// 4/6/7): `SpawnPane` now carries a client-resolved `shell`/`command_shell` argv instead of the
/// server resolving from its own process environment or on-disk config, which could be stale
/// relative to a live-reloaded client.
pub const PROTOCOL_VERSION: u32 = 8;
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
const FRAME_KIND_CONTROL_JSON: u8 = 1;
const FRAME_KIND_PANE_OUTPUT: u8 = 2;
const FRAME_KIND_PANE_INPUT: u8 = 3;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneMeta {
    pub pane_id: PaneId,
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub pid: Option<u32>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub exited: Option<i32>,
    pub logging: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: ClientId,
    pub label: String,
    pub read_only: bool,
    /// True while this client has an outstanding request for the layout-control lease that the
    /// controller has not yet granted or declined. Broadcast in the roster so every client can badge
    /// the pending request; cleared when control moves to it or the controller declines.
    #[serde(default)]
    pub requesting_control: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Attach {
        session: String,
        protocol_version: u32,
        label: String,
        read_only: bool,
    },
    /// Picker probe: report session status without registering the connection as a client and
    /// without any replay seeding. Cheap enough to run against many sockets concurrently.
    Query {
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
        /// Terminal color palette seeded onto the server screen *before* the PTY spawns, so the
        /// child's startup OSC 4/10/11 color queries are answered with the theme palette rather
        /// than the screen default. Sending it out-of-band via `SetPalette` races the child's
        /// query, so it must ride along with the spawn request.
        palette: WirePalette,
        /// Resolved interactive-shell argv (non-empty; program then fixed args), resolved
        /// client-side via `platform::command::resolve_interactive_shell` against the live
        /// config. Used verbatim when `command` is `None`; also used to `exec` into after
        /// `command` completes when `keep_open` is set (see [`ServerPane`] doc comment).
        shell: Vec<String>,
        /// Resolved command-runner argv (non-empty; program then fixed args), resolved
        /// client-side via `platform::command::resolve_command_shell`. Only used when `command`
        /// is `Some`; the command string becomes its final argument.
        command_shell: Vec<String>,
    },
    Resize {
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    Kill {
        pane_id: PaneId,
        generation: u64,
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
    SetPaneLogging {
        pane_id: PaneId,
        generation: u64,
        enabled: bool,
    },
    /// Commit a new shared layout. Accepted only from the controller and only when `base_rev`
    /// equals the server's current revision; otherwise the server replies [`ServerMessage::LayoutRejected`].
    CommitLayout {
        base_rev: u64,
        layout: SharedLayout,
    },
    /// Ask the current controller for the layout-control lease. The server auto-grants when there is
    /// no controller; otherwise it flags this client as requesting and notifies the controller (see
    /// [`ServerMessage::ControlRequested`]). Never steals from a present controller.
    RequestControl,
    /// Controller-only: grant the lease to `to`, which also clears `to`'s pending request.
    GrantControl {
        to: ClientId,
    },
    /// Controller-only: reject `to`'s pending control request, clearing its flag and notifying it
    /// (see [`ServerMessage::ControlDeclined`]).
    DeclineControl {
        to: ClientId,
    },
    SetInputLock {
        locked: bool,
    },
    /// Heartbeat reply to a [`ServerMessage::Ping`].
    Pong {
        seq: u64,
    },
    Rename {
        name: String,
    },
    Detach,
    Shutdown,
}

/// Why the layout-control lease moved. Carried by [`ServerMessage::ControllerChanged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControllerChangeReason {
    /// The controller detached or dropped cleanly.
    Released,
    /// The controller missed heartbeats and was disconnected.
    Expired,
    /// The lease was granted: the first attacher, promotion of the oldest survivor, a controller's
    /// explicit grant, or an auto-grant to a requester when no controller held the lease.
    Granted,
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
        client_id: ClientId,
        panes: Vec<PaneMeta>,
        layout_rev: u64,
        layout: Option<SharedLayout>,
        controller: Option<ClientId>,
        clients: Vec<ClientInfo>,
        input_locked: bool,
    },
    /// Reply to a [`ClientMessage::Query`] probe.
    SessionInfo {
        session: String,
        panes: usize,
        clients: u32,
        has_layout: bool,
    },
    Resized {
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    Exited {
        pane_id: PaneId,
        generation: u64,
        code: i32,
    },
    SpawnResult {
        pane_id: PaneId,
        generation: u64,
        pid: Option<u32>,
        ok: bool,
        error: Option<String>,
    },
    PaneLoggingChanged {
        pane_id: PaneId,
        generation: u64,
        enabled: bool,
        path: Option<String>,
        error: Option<String>,
    },
    Renamed {
        session: String,
    },
    /// A new layout revision was accepted; broadcast to every client including its author (so the
    /// author confirms its own rev from the echo and all clients see one identical rev sequence).
    LayoutCommitted {
        rev: u64,
        author: ClientId,
        layout: SharedLayout,
    },
    /// A commit was rejected (stale base rev or non-controller). Sent to the committer only, with
    /// the authoritative layout so the rejection self-heals.
    LayoutRejected {
        current_rev: u64,
        layout: Option<SharedLayout>,
    },
    ControllerChanged {
        controller: Option<ClientId>,
        reason: ControllerChangeReason,
    },
    /// Sent only to the current controller when `from` requests the lease. Debounced per requester so
    /// repeated requests cannot spam the controller; the sticky badge lives in the roster instead.
    ControlRequested {
        from: ClientId,
    },
    /// Sent only to a requester whose pending control request the controller declined.
    ControlDeclined,
    ClientsChanged {
        clients: Vec<ClientInfo>,
        input_locked: bool,
    },
    Ping {
        seq: u64,
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
    use crate::shared_layout::{
        SHARED_LAYOUT_VERSION, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
        SharedWorkspace,
    };

    #[test]
    fn golden_layout_commit_json_shape() {
        let layout = SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: 120,
            canvas_rows: 40,
            workspaces: vec![SharedWorkspace {
                index: 0,
                name: Some("dev".to_string()),
                synchronized: true,
                layout: SharedLayoutKind::Master,
                start_axis: SharedSplitAxis::Vertical,
                split_ratios: vec![0.4],
                tree: Some(SharedTree::Split {
                    axis: SharedSplitAxis::Vertical,
                    ratio: 0.375,
                    first: Box::new(SharedTree::Leaf { pane: 2 }),
                    second: Box::new(SharedTree::Leaf { pane: 9 }),
                }),
                panes: vec![SharedPane {
                    pane_id: 2,
                    generation: 7,
                    title: Some("editor".to_string()),
                    profile_name: None,
                    cwd: Some("/repo".to_string()),
                    command: Some("nvim".to_string()),
                    keep_open: false,
                    floating: false,
                    fullscreen: false,
                    rect: None,
                }],
            }],
        };

        assert_eq!(
            serde_json::to_value(ServerMessage::LayoutCommitted {
                rev: 4,
                author: 3,
                layout,
            })
            .unwrap(),
            serde_json::json!({
                "type": "layout-committed",
                "rev": 4,
                "author": 3,
                "layout": {
                    "version": 1,
                    "canvas_cols": 120,
                    "canvas_rows": 40,
                    "workspaces": [{
                        "index": 0,
                        "name": "dev",
                        "synchronized": true,
                        "layout": "master",
                        "start_axis": "vertical",
                        "split_ratios": [0.4000000059604645],
                        "tree": {
                            "kind": "split",
                            "axis": "vertical",
                            "ratio": 0.375,
                            "first": {"kind": "leaf", "pane": 2},
                            "second": {"kind": "leaf", "pane": 9}
                        },
                        "panes": [{
                            "pane_id": 2,
                            "generation": 7,
                            "title": "editor",
                            "profile_name": null,
                            "cwd": "/repo",
                            "command": "nvim",
                            "keep_open": false,
                            "floating": false,
                            "fullscreen": false,
                            "rect": null
                        }]
                    }]
                }
            })
        );
    }

    #[test]
    fn protocol_frame_round_trips() {
        let msg = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            label: "alice".into(),
            read_only: false,
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
            label: "alice".into(),
            read_only: true,
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
            palette: WirePalette {
                foreground: None,
                background: None,
                ansi: [Color::Black; 16],
            },
            shell: vec!["/bin/sh".to_string()],
            command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
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
    fn golden_client_attach_json_shape() {
        let value = serde_json::to_value(ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            label: "alice".into(),
            read_only: true,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"attach","session":"dev","protocol_version":8,"label":"alice","read_only":true})
        );
    }

    #[test]
    fn golden_query_json_shape() {
        let value = serde_json::to_value(ClientMessage::Query {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"query","session":"dev","protocol_version":8})
        );
    }

    #[test]
    fn golden_request_control_and_pong_json_shape() {
        assert_eq!(
            serde_json::to_value(ClientMessage::RequestControl).unwrap(),
            serde_json::json!({"type":"request-control"})
        );
        assert_eq!(
            serde_json::to_value(ClientMessage::Pong { seq: 5 }).unwrap(),
            serde_json::json!({"type":"pong","seq":5})
        );
    }

    #[test]
    fn grant_control_and_input_lock_round_trip() {
        for message in [
            ClientMessage::GrantControl { to: 7 },
            ClientMessage::DeclineControl { to: 7 },
            ClientMessage::RequestControl,
            ClientMessage::SetInputLock { locked: true },
        ] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &message).unwrap();
            assert_eq!(
                read_frame::<_, ClientMessage>(&mut &bytes[..]).unwrap(),
                message
            );
        }
    }

    #[test]
    fn golden_controller_changed_and_clients_changed_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::ControllerChanged {
                controller: Some(3),
                reason: ControllerChangeReason::Granted,
            })
            .unwrap(),
            serde_json::json!({"type":"controller-changed","controller":3,"reason":"granted"})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::ClientsChanged {
                clients: vec![ClientInfo {
                    id: 1,
                    label: "alice".into(),
                    read_only: false,
                    requesting_control: true,
                }],
                input_locked: true,
            })
            .unwrap(),
            serde_json::json!({"type":"clients-changed","clients":[{"id":1,"label":"alice","read_only":false,"requesting_control":true}],"input_locked":true})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::Ping { seq: 9 }).unwrap(),
            serde_json::json!({"type":"ping","seq":9})
        );
    }

    #[test]
    fn golden_session_info_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::SessionInfo {
                session: "dev".into(),
                panes: 2,
                clients: 1,
                has_layout: true,
            })
            .unwrap(),
            serde_json::json!({"type":"session-info","session":"dev","panes":2,"clients":1,"has_layout":true})
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
            label: "alice".into(),
            read_only: false,
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
        let palette = WirePalette {
            foreground: Some(Color::White),
            background: Some(Color::Black),
            ansi: [Color::Black; 16],
        };
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
            palette,
            shell: vec!["/bin/zsh".to_string()],
            command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"spawn-pane","pane_id":7,"generation":9,"command":"bash","cwd":"/repo","cols":80,"rows":24,"keep_open":true,"env":[["A","B"]],"title":"shell","palette":serde_json::to_value(palette).unwrap(),"shell":["/bin/zsh"],"command_shell":["/bin/sh","-c"]})
        );
    }

    #[test]
    fn golden_server_messages_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::Resized {
                pane_id: 1,
                generation: 2,
                cols: 80,
                rows: 24,
            })
            .unwrap(),
            serde_json::json!({"type":"resized","pane_id":1,"generation":2,"cols":80,"rows":24})
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
