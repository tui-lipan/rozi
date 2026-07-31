#![allow(clippy::too_many_arguments)]

use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::platform::ipc::{IpcConnection, IpcEndpoint};
use crate::session::protocol::Frame;
use crate::session::protocol::{
    self, ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

#[derive(Clone)]
pub struct SessionClient {
    tx: mpsc::Sender<ClientOutbound>,
    server_pid: Option<u32>,
    /// Wire version agreed with this server. Gates messages added after the minimum supported
    /// version so an older server never receives a variant it cannot deserialize.
    effective_protocol: u32,
    /// This client's host cell size in pixels, sent with the canonical PTY size so the server's
    /// PTYs report pixel dimensions the child can size images against. Read once: it is a
    /// property of the terminal this process is attached to, not of any one pane.
    cell: tui_lipan::TerminalCellSize,
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
        (
            Self {
                tx,
                cell: tui_lipan::TerminalCellSize::default(),
                server_pid: None,
                effective_protocol: PROTOCOL_VERSION,
            },
            rx,
        )
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
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        let mut stream = stream;
        let server_pid = stream.peer_pid();
        let mut reader = stream.try_clone()?;
        reader.set_read_timeout(Some(Duration::from_secs(2)))?;
        protocol::write_frame(
            &mut stream,
            &protocol::attach_message(
                session,
                crate::platform::user::current_user_label(),
                read_only,
            ),
        )?;
        let attached = protocol::read_frame::<_, ServerMessage>(&mut reader)?;
        reader.set_read_timeout(None)?;
        #[cfg(windows)]
        // A pending synchronous ReadFile on a duplicated named-pipe handle can hold up WriteFile on
        // its sibling, delaying both keys and heartbeat pongs. Polling keeps the duplex path live.
        reader.set_nonblocking(true)?;
        let effective_protocol = validate_attached(&attached)?;
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
        let heartbeat_tx = tx.clone();
        thread::spawn(move || forward_inbound(&mut reader, &inbound, Some(&heartbeat_tx)));
        Ok((
            Self {
                tx,
                server_pid,
                effective_protocol,
                cell: tui_lipan::host_cell_size(),
            },
            attached,
        ))
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.server_pid
    }

    /// Negotiated wire version for this connection.
    pub fn effective_protocol(&self) -> u32 {
        self.effective_protocol
    }

    /// Whether this server can serve the sidebar file tree's filesystem queries.
    pub fn supports_file_tree(&self) -> bool {
        self.effective_protocol >= crate::session::protocol::FILE_TREE_PROTOCOL
    }

    /// Tell the server whether this client is parked — attached with its screens kept live, but not
    /// displaying the session. A parked client gives up the layout-control lease, so keeping a
    /// session open in the background never makes it look occupied to the next client to attach.
    ///
    /// No-op against a pre-14 server, which has no notion of parking: it keeps treating every
    /// attached client as an occupant, which is the behavior that build already had.
    pub fn set_parked(&self, parked: bool) {
        if self.effective_protocol < crate::session::protocol::PARKED_PROTOCOL {
            return;
        }
        self.send_control(ClientMessage::SetParked { parked });
    }

    /// Ask the server to list one directory on its own host. No-op against an older server.
    pub fn list_directory(&self, path: String, show_hidden: bool) {
        if !self.supports_file_tree() {
            return;
        }
        self.send_control(ClientMessage::ListDirectory { path, show_hidden });
    }

    /// Ask the server to scan a repository on its own host for changed paths.
    pub fn list_changes(&self, root: String) {
        if !self.supports_file_tree() {
            return;
        }
        self.send_control(ClientMessage::ListChanges { root });
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
            cell_width: self.cell.width,
            cell_height: self.cell.height,
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
            cell_width: self.cell.width,
            cell_height: self.cell.height,
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
    pub fn set_pane_status(
        &self,
        pane_id: PaneId,
        generation: u64,
        status: Option<String>,
        reason: Option<String>,
    ) {
        self.send_control(ClientMessage::SetPaneStatus {
            pane_id,
            generation,
            status,
            reason,
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
    pub fn set_control_takeover(&self, allowed: bool) {
        if self.effective_protocol < crate::session::protocol::CONTROL_TAKEOVER_PROTOCOL {
            return;
        }
        self.send_control(ClientMessage::SetControlTakeover { allowed });
    }
    pub fn grant_control(&self, to: ClientId) {
        self.send_control(ClientMessage::GrantControl { to });
    }
    pub fn decline_control(&self, to: ClientId) {
        self.send_control(ClientMessage::DeclineControl { to });
    }
    /// Controller-only: remove another client from the session. Silently does nothing against a
    /// server too old to understand the message, which is why the UI gates the affordance on the
    /// same version rather than letting a key press vanish.
    pub fn evict_client(&self, target: ClientId) {
        if self.effective_protocol < crate::session::protocol::EVICT_CLIENT_PROTOCOL {
            return;
        }
        self.send_control(ClientMessage::EvictClient { target });
    }
    pub fn set_input_lock(&self, locked: bool) {
        self.send_control(ClientMessage::SetInputLock { locked });
    }

    pub fn set_session_origin(&self, profile: String) {
        self.send_control(ClientMessage::SetSessionOrigin { profile });
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

/// Validate the attach reply and return the negotiated wire version.
fn validate_attached(attached: &ServerMessage) -> io::Result<u32> {
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
    let ServerMessage::Attached {
        protocol_version: server_max,
        effective_protocol,
        ..
    } = attached
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("attach handshake failed: {attached:?}"),
        ));
    };
    // Pre-negotiation servers omit effective_protocol (serde default 0) and echo their own
    // protocol_version; treat that echo as the effective version.
    let effective = if *effective_protocol == 0 {
        *server_max
    } else {
        *effective_protocol
    };
    if !(MIN_SUPPORTED_PROTOCOL..=PROTOCOL_VERSION).contains(&effective) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "runs an incompatible hyprmux version (negotiated protocol {effective}; client supports {MIN_SUPPORTED_PROTOCOL}-{PROTOCOL_VERSION}); kill it and start a new one"
            ),
        ));
    }
    Ok(effective)
}

fn forward_inbound<R: std::io::Read>(
    reader: &mut R,
    inbound: &mpsc::Sender<Frame<ServerMessage>>,
    outbound: Option<&mpsc::Sender<ClientOutbound>>,
) {
    let mut decoder = protocol::FrameDecoder::default();
    loop {
        let would_block = match decoder.read_from_status(reader) {
            Ok(protocol::FrameReadStatus::Eof) => break,
            Ok(protocol::FrameReadStatus::Read(_)) => false,
            Ok(protocol::FrameReadStatus::WouldBlock) => true,
            Err(_) => break,
        };
        loop {
            match decoder.next_frame::<ServerMessage>() {
                Ok(Some(frame)) => {
                    if let Frame::Control(ServerMessage::Ping { seq }) = frame
                        && let Some(outbound) = outbound
                    {
                        if outbound
                            .send(ClientOutbound::Control(ClientMessage::Pong { seq }))
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    if inbound.send(frame).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
        if would_block {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    fn attached_message() -> ServerMessage {
        ServerMessage::Attached {
            protocol_version: PROTOCOL_VERSION,
            effective_protocol: PROTOCOL_VERSION,
            session: "test".to_string(),
            client_id: 7,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(7),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            created_from_profile: None,
        }
    }

    #[test]
    #[cfg(unix)]
    fn attached_stream_decodes_control_and_pane_frames() {
        let (mut client_stream, mut server_stream) = UnixStream::pair().expect("socket pair");
        let server = std::thread::spawn(move || {
            protocol::write_frame(&mut server_stream, &ServerMessage::Ping { seq: 11 })
                .expect("write control frame");
            protocol::write_pane_output_frame(&mut server_stream, 3, 5, b"ready\n")
                .expect("write pane frame");
        });

        let (inbound_tx, inbound_rx) = mpsc::channel();
        forward_inbound(&mut client_stream, &inbound_tx, None);
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

    #[test]
    fn transport_replies_to_ping_without_waiting_for_ui_dispatch() {
        let mut bytes = Vec::new();
        protocol::write_frame(&mut bytes, &ServerMessage::Ping { seq: 42 }).unwrap();
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();

        forward_inbound(
            &mut std::io::Cursor::new(bytes),
            &inbound_tx,
            Some(&outbound_tx),
        );

        assert_eq!(
            outbound_rx.try_recv().unwrap(),
            ClientOutbound::Control(ClientMessage::Pong { seq: 42 })
        );
        assert!(inbound_rx.try_recv().is_err());
    }

    #[test]
    fn set_pane_status_queues_control_message() {
        let (client, outbound) = SessionClient::test_channel();
        client.set_pane_status(
            3,
            5,
            Some("blocked".to_string()),
            Some("needs approval".to_string()),
        );
        assert_eq!(
            outbound.try_recv().unwrap(),
            ClientOutbound::Control(ClientMessage::SetPaneStatus {
                pane_id: 3,
                generation: 5,
                status: Some("blocked".to_string()),
                reason: Some("needs approval".to_string()),
            })
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_duplex_transport_delivers_pong_while_reader_polls() {
        let endpoint = IpcEndpoint::at_path(
            std::env::temp_dir().join(format!("hyprmux-client-duplex-{}.sock", std::process::id())),
        );
        endpoint.remove_stale();
        let listener = endpoint.bind().unwrap().into_listener();
        listener.set_nonblocking(true).unwrap();

        let server = thread::spawn(move || {
            let mut stream = loop {
                match listener.accept() {
                    Ok(stream) => break stream,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            assert!(matches!(
                protocol::read_frame::<_, ClientMessage>(&mut stream).unwrap(),
                ClientMessage::Attach { .. }
            ));
            protocol::write_frame(&mut stream, &attached_message()).unwrap();
            protocol::write_frame(&mut stream, &ServerMessage::Ping { seq: 77 }).unwrap();
            assert_eq!(
                protocol::read_frame::<_, ClientMessage>(&mut stream).unwrap(),
                ClientMessage::Pong { seq: 77 }
            );
        });

        let (inbound_tx, _inbound_rx) = mpsc::channel();
        let (_client, attached) =
            SessionClient::connect_attached(&endpoint, "test", inbound_tx, false).unwrap();
        assert!(matches!(attached, ServerMessage::Attached { .. }));
        server.join().unwrap();
        endpoint.remove_stale();
    }
}
