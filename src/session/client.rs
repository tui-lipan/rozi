#![allow(clippy::too_many_arguments)]

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::session::protocol::{
    self, ClientMessage, PROTOCOL_VERSION, ServerMessage, WirePalette, WireSnapshot,
};
use crate::state::PaneId;

#[derive(Clone)]
pub struct SessionClient {
    tx: mpsc::Sender<ClientMessage>,
    next_request_id: Arc<AtomicU64>,
}

impl SessionClient {
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Receiver<ClientMessage>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx,
                next_request_id: Arc::new(AtomicU64::new(1)),
            },
            rx,
        )
    }

    pub fn connect(
        path: &Path,
        session: impl Into<String>,
        inbound: mpsc::Sender<ServerMessage>,
    ) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Self::from_stream(stream, session, inbound)
    }

    pub fn connect_attached(
        path: &Path,
        session: impl Into<String>,
        inbound: mpsc::Sender<ServerMessage>,
    ) -> io::Result<(Self, ServerMessage)> {
        let stream = UnixStream::connect(path)?;
        Self::from_stream_attached(stream, session, inbound)
    }

    pub fn from_stream(
        stream: UnixStream,
        session: impl Into<String>,
        inbound: mpsc::Sender<ServerMessage>,
    ) -> io::Result<Self> {
        Ok(Self::from_stream_attached(stream, session, inbound)?.0)
    }

    pub fn from_stream_attached(
        mut stream: UnixStream,
        session: impl Into<String>,
        inbound: mpsc::Sender<ServerMessage>,
    ) -> io::Result<(Self, ServerMessage)> {
        let mut reader = stream.try_clone()?;
        reader.set_read_timeout(Some(Duration::from_secs(2)))?;
        protocol::write_frame(
            &mut stream,
            &ClientMessage::Attach {
                session: session.into(),
                protocol_version: PROTOCOL_VERSION,
            },
        )?;
        let attached = protocol::read_frame::<_, ServerMessage>(&mut reader)?;
        reader.set_read_timeout(None)?;
        if !matches!(attached, ServerMessage::Attached { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("attach handshake failed: {attached:?}"),
            ));
        }
        let (tx, rx) = mpsc::channel::<ClientMessage>();
        thread::spawn(move || {
            for message in rx {
                if protocol::write_frame(&mut stream, &message).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            while let Ok(message) = protocol::read_frame::<_, ServerMessage>(&mut reader) {
                if inbound.send(message).is_err() {
                    break;
                }
            }
        });
        Ok((
            Self {
                tx,
                next_request_id: Arc::new(AtomicU64::new(1)),
            },
            attached,
        ))
    }

    pub fn spawn_pane(
        &self,
        pane_id: PaneId,
        generation: u64,
        command: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        keep_open: bool,
        title: Option<String>,
    ) {
        self.send(ClientMessage::SpawnPane {
            pane_id,
            generation,
            command,
            cwd,
            cols,
            rows,
            keep_open,
            env: Vec::new(),
            title,
        });
    }

    #[cfg(unix)]
    pub fn adopt_pane(
        &self,
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
        pid: Option<u32>,
        title: Option<String>,
        cwd: Option<String>,
        snapshot: WireSnapshot,
        socket_path: String,
    ) {
        self.send(ClientMessage::AdoptPane {
            pane_id,
            generation,
            cols,
            rows,
            pid,
            title,
            cwd,
            snapshot,
            socket_path,
        });
    }

    pub fn send_input(&self, pane_id: PaneId, generation: u64, bytes: Vec<u8>) {
        self.send(ClientMessage::Input {
            pane_id,
            generation,
            bytes,
        });
    }
    pub fn resize(&self, pane_id: PaneId, generation: u64, cols: u16, rows: u16) {
        self.send(ClientMessage::Resize {
            pane_id,
            generation,
            cols,
            rows,
        });
    }
    pub fn scroll(&self, pane_id: PaneId, generation: u64, offset: usize) {
        self.send(ClientMessage::Scroll {
            pane_id,
            generation,
            offset,
        });
    }
    pub fn kill(&self, pane_id: PaneId, generation: u64) {
        self.send(ClientMessage::Kill {
            pane_id,
            generation,
        });
    }
    pub fn set_palette(&self, pane_id: PaneId, generation: u64, palette: TerminalColorPalette) {
        self.send(ClientMessage::SetPalette {
            pane_id,
            generation,
            palette: WirePalette::from(palette),
        });
    }
    pub fn push_layout(&self, blob: String) {
        self.send(ClientMessage::PushLayout { blob });
    }
    pub fn detach(&self) {
        self.send(ClientMessage::Detach);
    }

    pub fn shutdown(&self) {
        self.send(ClientMessage::Shutdown);
    }

    pub fn search(&self, pane_id: PaneId, generation: u64, query: String) -> u64 {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.send(ClientMessage::Search {
            request_id,
            pane_id,
            generation,
            query,
        });
        request_id
    }

    fn send(&self, message: ClientMessage) {
        let _ = self.tx.send(message);
    }
}

pub fn apply_wire_snapshot(
    wire: WireSnapshot,
) -> std::result::Result<TerminalRenderSnapshot, protocol::SnapshotVersionError> {
    wire.try_into_snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_wire_snapshot_rejects_bad_version() {
        let mut wire = WireSnapshot::from_snapshot(None, None, &TerminalRenderSnapshot::default());
        wire.version = PROTOCOL_VERSION + 1;
        assert!(apply_wire_snapshot(wire).is_err());
    }
}
