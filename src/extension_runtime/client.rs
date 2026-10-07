//! The client's end of an extension runtime.
//!
//! This machine's placed processes run through the very same runtime code a remote host does, in
//! a thread here instead of over SSH, so a contribution behaves the same wherever it is placed and
//! one path is all there is to get right.
//!
//! Every control connection a placed process opens on its runtime's bridge arrives here as a byte
//! stream and is served by the UI's ordinary control handler, told only that it came over this
//! runtime connection. Who the caller is gets decided there, from its credential.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tui_lipan::prelude::CommandLink;

use super::bundle::Bundle;
use super::protocol::{self, Frame, Hello, Message, read_frame};
use crate::Msg;
use crate::control::RequestOrigin;
use crate::events::EventHub;
use crate::platform::ipc::{IpcConnection, PipedConnection};
use crate::state::{HostKey, Unavailable};

/// How long a runtime may take to answer the opening message once its channel is up. An SSH
/// connection that needs a password never gets this far: it runs in batch mode and fails instead.
const HELLO_DEADLINE: Duration = Duration::from_secs(30);

/// What a runtime connection reports to the UI.
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Ready(Hello),
    /// The runtime could not be reached or refused this client.
    Failed {
        reason: Unavailable,
        /// Whether trying again later could help. An old remote Rozi will not grow the capability
        /// by being asked again; a dropped network may come back.
        retry: bool,
    },
    /// An established connection ended.
    Lost(String),
    Message(Message),
}

type SharedWriter = Arc<Mutex<Option<IpcConnection>>>;

/// One live runtime connection. Dropping it closes the channel, which ends the runtime and every
/// process it started.
pub struct RuntimeConnection {
    writer: SharedWriter,
}

impl RuntimeConnection {
    /// Start a runtime for `host` and connect to it on a thread of its own. Everything it learns
    /// arrives as [`Msg::ExtensionRuntime`] for `epoch`.
    pub fn start(
        host: HostKey,
        epoch: u64,
        remote: crate::config::RemoteConfig,
        link: CommandLink<Msg>,
        hub: EventHub,
    ) -> Arc<Self> {
        let writer: SharedWriter = Arc::new(Mutex::new(None));
        let connection = Arc::new(Self {
            writer: writer.clone(),
        });
        std::thread::Builder::new()
            .name("rozi-extension-runtime".to_string())
            .spawn(move || {
                let report = |event| {
                    link.send(Msg::ExtensionRuntime {
                        host: host.clone(),
                        epoch,
                        event,
                    })
                };
                let channel = match &host {
                    HostKey::Local => open_local(),
                    HostKey::Remote(target) => open_remote(target, &remote),
                };
                let (reader, hello) = match channel.and_then(|channel| handshake(channel, &writer))
                {
                    Ok(ready) => ready,
                    Err((reason, retry)) => {
                        report(RuntimeEvent::Failed { reason, retry });
                        return;
                    }
                };
                report(RuntimeEvent::Ready(hello));
                let detail = relay(reader, &writer, epoch, &link, &hub, &report);
                writer.lock().unwrap().take();
                report(RuntimeEvent::Lost(detail));
            })
            .expect("spawn extension runtime thread");
        connection
    }

    pub fn send(&self, message: &Message) -> io::Result<()> {
        self.with_writer(|out| protocol::write_message(out, message))
    }

    /// Send a bundle's bytes and commit it. The runtime answers with `Staged` or `StageFailed`.
    pub fn stage(&self, bundle: &Bundle) -> io::Result<()> {
        let archive = bundle.archive();
        self.with_writer(|out| {
            for chunk in archive.chunks(protocol::STAGE_CHUNK) {
                protocol::write_stage(out, bundle.digest(), chunk)?;
            }
            protocol::write_message(
                out,
                &Message::StageCommit {
                    digest: bundle.digest().to_string(),
                },
            )
        })
    }

    fn with_writer(
        &self,
        write: impl FnOnce(&mut IpcConnection) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?;
        match guard.as_mut() {
            Some(out) => write(out),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "extension runtime is not connected",
            )),
        }
    }

    /// Close the channel. The runtime sees end of input and stops everything it started.
    pub fn close(&self) {
        if let Some(connection) = self.writer.lock().unwrap().take() {
            let _ = connection.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for RuntimeConnection {
    fn drop(&mut self) {
        self.close();
    }
}

type Opened = (IpcConnection, Option<std::thread::JoinHandle<String>>);

fn open_local() -> Result<Opened, (Unavailable, bool)> {
    let failed = |error: io::Error| {
        (
            Unavailable::SpawnFailed {
                detail: format!("cannot start this machine's extension runtime: {error}"),
            },
            false,
        )
    };
    let (from_client, to_runtime) = std::io::pipe().map_err(failed)?;
    let (from_runtime, to_client) = std::io::pipe().map_err(failed)?;
    std::thread::Builder::new()
        .name("rozi-local-extension-runtime".to_string())
        .spawn(move || {
            let _ = super::server::serve(from_client, to_client);
        })
        .map_err(failed)?;
    Ok((
        IpcConnection::from_piped(PipedConnection::from_reader_writer(
            to_runtime,
            from_runtime,
        )),
        None,
    ))
}

fn open_remote(
    target: &crate::session::remote::RemoteTarget,
    config: &crate::config::RemoteConfig,
) -> Result<Opened, (Unavailable, bool)> {
    if !crate::platform::command::program_exists("ssh") {
        return Err((
            Unavailable::RuntimeUnreachable {
                detail: "ssh is not installed".to_string(),
            },
            false,
        ));
    }
    crate::session::remote::spawn_rozi_channel(target, config, &["extensions", "runtime"])
        .map(|(connection, stderr)| (connection, Some(stderr)))
        .map_err(|error| {
            (
                Unavailable::RuntimeUnreachable {
                    detail: error.to_string(),
                },
                true,
            )
        })
}

fn handshake(
    (mut connection, stderr): Opened,
    writer: &SharedWriter,
) -> Result<(IpcConnection, Hello), (Unavailable, bool)> {
    let failed = |error: io::Error, stderr: Option<std::thread::JoinHandle<String>>| {
        let detail = stderr
            .and_then(|stderr| stderr.join().ok())
            .unwrap_or_default();
        classify(&error, detail.trim())
    };
    let _ = connection.set_read_timeout(Some(HELLO_DEADLINE));
    if let Err(error) = protocol::write_hello(&mut connection, &Hello::current(None)) {
        let _ = connection.shutdown(std::net::Shutdown::Both);
        return Err(failed(error, stderr));
    }
    let hello = match read_frame(&mut connection, true) {
        Ok(Some(Frame::Hello(hello))) => hello,
        Ok(_) => {
            let _ = connection.shutdown(std::net::Shutdown::Both);
            return Err(failed(
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the runtime ended before answering",
                ),
                stderr,
            ));
        }
        Err(error) => {
            let _ = connection.shutdown(std::net::Shutdown::Both);
            return Err(failed(error, stderr));
        }
    };
    if let Err(error) = hello.validate() {
        let _ = connection.shutdown(std::net::Shutdown::Both);
        return Err((
            Unavailable::RuntimeUnsupported {
                detail: format!("the host's Rozi speaks another runtime protocol: {error}"),
            },
            false,
        ));
    }
    let _ = connection.set_read_timeout(None);
    let reader = connection.try_clone().map_err(|error| {
        (
            Unavailable::RuntimeUnreachable {
                detail: error.to_string(),
            },
            true,
        )
    })?;
    *writer.lock().unwrap() = Some(connection);
    Ok((reader, hello))
}

/// Explain a runtime that never answered. An old Rozi on the host refuses the subcommand, which is
/// not worth retrying; an SSH failure may be.
fn classify(error: &io::Error, stderr: &str) -> (Unavailable, bool) {
    if stderr.contains("unknown extensions command")
        || stderr.contains("unexpected argument")
        || stderr.contains("expected list, install")
    {
        return (
            Unavailable::RuntimeUnsupported {
                detail: format!(
                    "the host's Rozi cannot run extension processes ({}); update Rozi there",
                    protocol::CAPABILITY
                ),
            },
            false,
        );
    }
    let detail = if stderr.is_empty() {
        error.to_string()
    } else {
        format!("{error}: {stderr}")
    };
    (Unavailable::RuntimeUnreachable { detail }, true)
}

/// Serve the runtime's frames until the channel closes. Returns why it closed.
fn relay(
    mut reader: IpcConnection,
    writer: &SharedWriter,
    epoch: u64,
    link: &CommandLink<Msg>,
    hub: &EventHub,
    report: &impl Fn(RuntimeEvent),
) -> String {
    let mut bridges: HashMap<u64, mpsc::Sender<Vec<u8>>> = HashMap::new();
    loop {
        match read_frame(&mut reader, false) {
            Ok(Some(Frame::Message(Message::BridgeOpen { conn }))) => {
                let (to_handler, inbound) = mpsc::channel();
                bridges.insert(conn, to_handler);
                let connection = IpcConnection::from_piped(PipedConnection::from_reader_writer(
                    BridgeWriter {
                        conn,
                        out: writer.clone(),
                    },
                    ChannelReader {
                        inbound,
                        pending: Vec::new(),
                        offset: 0,
                    },
                ));
                let link = link.clone();
                let hub = hub.clone();
                std::thread::spawn(move || {
                    crate::control::serve_connection(
                        connection,
                        link,
                        hub,
                        RequestOrigin::Bridged { runtime: epoch },
                    );
                });
            }
            Ok(Some(Frame::Message(Message::BridgeClose { conn }))) => {
                bridges.remove(&conn);
            }
            Ok(Some(Frame::Bridge { conn, data })) => {
                if let Some(handler) = bridges.get(&conn)
                    && handler.send(data).is_err()
                {
                    bridges.remove(&conn);
                }
            }
            Ok(Some(Frame::Message(message))) => report(RuntimeEvent::Message(message)),
            Ok(Some(Frame::Hello(_) | Frame::Stage { .. })) => {}
            Ok(None) => return "the runtime closed its channel".to_string(),
            Err(error) => return error.to_string(),
        }
    }
}

/// The handler's writes, framed back to the runtime for one bridged connection.
struct BridgeWriter {
    conn: u64,
    out: SharedWriter,
}

impl Write for BridgeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.out.lock().map_err(|_| io::Error::other("poisoned"))?;
        let out = guard
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "runtime gone"))?;
        let take = buf.len().min(protocol::MAX_FRAME - 64);
        protocol::write_bridge(out, self.conn, &buf[..take])?;
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BridgeWriter {
    /// The handler is done with this connection: let the process on the host see end of file.
    fn drop(&mut self) {
        if let Ok(mut guard) = self.out.lock()
            && let Some(out) = guard.as_mut()
        {
            let _ = protocol::write_message(out, &Message::BridgeClose { conn: self.conn });
        }
    }
}

/// The bytes a bridged process sent, fed to the handler as a stream.
struct ChannelReader {
    inbound: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.offset >= self.pending.len() {
            match self.inbound.recv() {
                Ok(data) => {
                    self.pending = data;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len() - self.offset);
        buf[..n].copy_from_slice(&self.pending[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_remote_rozi_is_not_retried_and_a_network_failure_is() {
        let eof = io::Error::from(io::ErrorKind::UnexpectedEof);
        let (reason, retry) = classify(
            &eof,
            "rozi: unknown extensions command `runtime` (expected list, install, update, remove, new, or check)",
        );
        assert!(matches!(reason, Unavailable::RuntimeUnsupported { .. }));
        assert!(!retry);
        let (reason, retry) = classify(&eof, "ssh: connect to host pc port 22: No route to host");
        assert!(matches!(reason, Unavailable::RuntimeUnreachable { .. }));
        assert!(retry);
    }
}
