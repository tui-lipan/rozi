#![allow(clippy::too_many_arguments)]

use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::platform::ipc::{IpcConnection, IpcEndpoint};
use crate::session::protocol::Frame;
use crate::session::protocol::{self, ClientMessage, PROTOCOL_VERSION, ServerMessage, WirePalette};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

#[derive(Clone)]
pub struct SessionClient {
    tx: mpsc::Sender<ClientOutbound>,
}

#[derive(Clone, Debug, PartialEq)]
// `ClientMessage::SpawnPane` (with its `shell`/`command_shell` argv, cross-platform plan Phase 4)
// makes `Control` noticeably larger than `PaneInput`. Every outbound message already goes through
// an `mpsc` channel send regardless (one heap-ish allocation either way), and this is a
// per-user-action/per-input-chunk channel, not a hot per-byte loop, so boxing `ClientMessage` here
// to shrink the enum is not worth the churn across every `ClientOutbound::Control(...)` construction
// and match site.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ClientOutbound {
    Control(ClientMessage),
    PaneInput {
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
}

impl SessionClient {
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Receiver<ClientOutbound>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx }, rx)
    }

    pub fn connect(
        endpoint: &IpcEndpoint,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        let stream = endpoint.connect()?;
        Self::from_stream(stream, session, inbound)
    }

    pub fn connect_attached(
        endpoint: &IpcEndpoint,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        let stream = endpoint.connect()?;
        Self::from_stream_attached(stream, session, inbound, read_only)
    }

    pub fn from_stream(
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        Ok(Self::from_stream_attached(stream, session, inbound, false)?.0)
    }

    pub fn from_stream_attached(
        mut stream: IpcConnection,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        let mut reader = stream.try_clone()?;
        reader.set_read_timeout(Some(Duration::from_secs(2)))?;
        protocol::write_frame(
            &mut stream,
            &ClientMessage::Attach {
                session: session.into(),
                protocol_version: PROTOCOL_VERSION,
                label: std::env::var("USER").unwrap_or_else(|_| "client".to_string()),
                read_only,
            },
        )?;
        let attached = protocol::read_frame::<_, ServerMessage>(&mut reader)?;
        reader.set_read_timeout(None)?;
        validate_attached(&attached)?;
        let (tx, rx) = mpsc::channel::<ClientOutbound>();
        thread::spawn(move || {
            for message in rx {
                let result = match message {
                    ClientOutbound::Control(message) => {
                        protocol::write_frame(&mut stream, &message)
                    }
                    ClientOutbound::PaneInput {
                        pane_id,
                        generation,
                        bytes,
                    } => protocol::write_pane_input_frame(&mut stream, pane_id, generation, &bytes),
                };
                if result.is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || forward_inbound(&mut reader, &inbound));
        Ok((Self { tx }, attached))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_pane(
        &self,
        pane_id: PaneId,
        generation: u64,
        command: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        keep_open: bool,
        env: Vec<(String, String)>,
        title: Option<String>,
        palette: TerminalColorPalette,
        shell: Vec<String>,
        command_shell: Vec<String>,
    ) {
        self.send_control(ClientMessage::SpawnPane {
            pane_id,
            generation,
            command,
            cwd,
            cols,
            rows,
            keep_open,
            env,
            title,
            palette: WirePalette::from(palette),
            shell,
            command_shell,
        });
    }

    pub fn send_input(&self, pane_id: PaneId, generation: u64, bytes: Vec<u8>) {
        self.send(ClientOutbound::PaneInput {
            pane_id,
            generation,
            bytes,
        });
    }
    pub fn resize(&self, pane_id: PaneId, generation: u64, cols: u16, rows: u16) {
        self.send_control(ClientMessage::Resize {
            pane_id,
            generation,
            cols,
            rows,
        });
    }
    pub fn kill(&self, pane_id: PaneId, generation: u64) {
        self.send_control(ClientMessage::Kill {
            pane_id,
            generation,
        });
    }
    pub fn set_palette(&self, pane_id: PaneId, generation: u64, palette: TerminalColorPalette) {
        self.send_control(ClientMessage::SetPalette {
            pane_id,
            generation,
            palette: WirePalette::from(palette),
        });
    }
    pub fn set_pane_logging(&self, pane_id: PaneId, generation: u64, enabled: bool) {
        self.send_control(ClientMessage::SetPaneLogging {
            pane_id,
            generation,
            enabled,
        });
    }
    /// Commit a new shared layout, optimistically based on `base_rev`. The server accepts it only
    /// while this client holds the lease and `base_rev` matches the current revision.
    pub fn commit_layout(&self, base_rev: u64, layout: SharedLayout) {
        self.send_control(ClientMessage::CommitLayout { base_rev, layout });
    }
    /// Ask the current controller for the layout-control lease. The server auto-grants only when no
    /// controller holds it; otherwise it flags the request for the controller to grant or decline.
    pub fn request_control(&self) {
        self.send_control(ClientMessage::RequestControl);
    }
    pub fn grant_control(&self, to: ClientId) {
        self.send_control(ClientMessage::GrantControl { to });
    }
    pub fn decline_control(&self, to: ClientId) {
        self.send_control(ClientMessage::DeclineControl { to });
    }
    pub fn set_input_lock(&self, locked: bool) {
        self.send_control(ClientMessage::SetInputLock { locked });
    }
    /// Reply to a server heartbeat.
    pub fn pong(&self, seq: u64) {
        self.send_control(ClientMessage::Pong { seq });
    }
    pub fn rename(&self, name: String) {
        self.send_control(ClientMessage::Rename { name });
    }
    pub fn detach(&self) {
        self.send_control(ClientMessage::Detach);
    }

    pub fn shutdown(&self) {
        self.send_control(ClientMessage::Shutdown);
    }

    fn send_control(&self, message: ClientMessage) {
        self.send(ClientOutbound::Control(message));
    }

    fn send(&self, message: ClientOutbound) {
        let _ = self.tx.send(message);
    }
}

fn validate_attached(attached: &ServerMessage) -> io::Result<()> {
    if let ServerMessage::Error { code, message } = attached {
        // A version skew (an older server still running an earlier wire protocol) is the common
        // cause here; give the user something actionable instead of a debug dump.
        let detail = if code == "protocol-mismatch" {
            format!("runs an incompatible hyprmux version ({message}); kill it and start a new one")
        } else {
            message.clone()
        };
        return Err(io::Error::new(io::ErrorKind::InvalidData, detail));
    }
    if !matches!(attached, ServerMessage::Attached { .. }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("attach handshake failed: {attached:?}"),
        ));
    }
    Ok(())
}

fn forward_inbound<R: std::io::Read>(reader: &mut R, inbound: &mpsc::Sender<Frame<ServerMessage>>) {
    let mut decoder = protocol::FrameDecoder::default();
    loop {
        match decoder.read_from_status(reader) {
            Ok(protocol::FrameReadStatus::Eof) => break,
            Ok(protocol::FrameReadStatus::Read(_) | protocol::FrameReadStatus::WouldBlock) => {}
            Err(_) => break,
        }
        loop {
            match decoder.next_frame::<ServerMessage>() {
                Ok(Some(frame)) => {
                    if inbound.send(frame).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn attached_message() -> ServerMessage {
        ServerMessage::Attached {
            protocol_version: PROTOCOL_VERSION,
            session: "test".to_string(),
            client_id: 7,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(7),
            clients: Vec::new(),
            input_locked: false,
        }
    }

    #[test]
    fn attached_stream_decodes_control_and_pane_frames() {
        let (mut client_stream, mut server_stream) = UnixStream::pair().expect("socket pair");
        let server = std::thread::spawn(move || {
            protocol::write_frame(&mut server_stream, &ServerMessage::Ping { seq: 11 })
                .expect("write control frame");
            protocol::write_pane_output_frame(&mut server_stream, 3, 5, b"ready\n")
                .expect("write pane frame");
        });

        let (inbound_tx, inbound_rx) = mpsc::channel();
        forward_inbound(&mut client_stream, &inbound_tx);
        assert_eq!(
            inbound_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("control frame"),
            Frame::Control(ServerMessage::Ping { seq: 11 })
        );
        assert_eq!(
            inbound_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("pane frame"),
            Frame::PaneBytes {
                pane_id: 3,
                generation: 5,
                bytes: b"ready\n".to_vec(),
            }
        );
        server.join().expect("server thread");
    }

    #[test]
    fn attach_error_is_returned_without_starting_client_threads() {
        let error = validate_attached(&ServerMessage::Error {
            code: "protocol-mismatch".to_string(),
            message: "server uses protocol 2".to_string(),
        })
        .expect_err("attach must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("incompatible hyprmux version"));
        assert!(validate_attached(&attached_message()).is_ok());
        assert!(validate_attached(&ServerMessage::Ping { seq: 1 }).is_err());
    }
}
