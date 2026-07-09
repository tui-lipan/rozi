#![allow(clippy::too_many_arguments)]

use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::session::protocol::Frame;
use crate::session::protocol::{self, ClientMessage, PROTOCOL_VERSION, ServerMessage, WirePalette};
use crate::state::PaneId;

#[derive(Clone)]
pub struct SessionClient {
    tx: mpsc::Sender<ClientOutbound>,
}

#[derive(Clone, Debug, PartialEq)]
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
        path: &Path,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Self::from_stream(stream, session, inbound)
    }

    pub fn connect_attached(
        path: &Path,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<(Self, ServerMessage)> {
        let stream = UnixStream::connect(path)?;
        Self::from_stream_attached(stream, session, inbound)
    }

    pub fn from_stream(
        stream: UnixStream,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        Ok(Self::from_stream_attached(stream, session, inbound)?.0)
    }

    pub fn from_stream_attached(
        mut stream: UnixStream,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
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
        thread::spawn(move || {
            let mut decoder = protocol::FrameDecoder::default();
            loop {
                match decoder.read_from_status(&mut reader) {
                    Ok(protocol::FrameReadStatus::Eof) => break,
                    Ok(
                        protocol::FrameReadStatus::Read(_) | protocol::FrameReadStatus::WouldBlock,
                    ) => {}
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
        });
        Ok((Self { tx }, attached))
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
        self.send_control(ClientMessage::SpawnPane {
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
    pub fn push_layout(&self, blob: String) {
        self.send_control(ClientMessage::PushLayout { blob });
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
