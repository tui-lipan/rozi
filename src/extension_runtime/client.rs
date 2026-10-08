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
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Most control connections one runtime may have open through its bridge at once. A placed
/// process holds one per `rozi` call in flight, and streams hold theirs for as long as they run.
pub const MAX_BRIDGES: usize = 64;

/// Most bytes one bridged connection may have waiting for the control handler to read. A request
/// line is at most [`crate::control::MAX_CONTROL_MESSAGE`]; this leaves room for a second.
pub const MAX_BRIDGE_QUEUED: usize = 2 * crate::control::MAX_CONTROL_MESSAGE;

/// What a bridged connection reads ahead of its control handler.
const BRIDGE_READ_AHEAD: usize = 64 * 1024;

/// Most bytes all of one runtime's bridged connections together may have waiting.
pub const MAX_QUEUED: usize = 16 * 1024 * 1024;

/// Most runtime messages waiting for the UI to handle them.
pub const MAX_UNDELIVERED: usize = 256;

/// Largest single runtime message. The biggest a runtime legitimately sends is an exit carrying a
/// sidebar listing, which is capped well below this.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;

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
///
/// The runtime is on a host that may be compromised, and it can open bridge connections and send
/// bytes before anything is authenticated. So everything it can make this client hold - handler
/// threads, queued bytes, undelivered messages - is bounded, and a runtime that goes past a bound
/// is disconnected rather than served.
pub struct RuntimeConnection {
    writer: SharedWriter,
    undelivered: Arc<AtomicUsize>,
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
        let undelivered = Arc::new(AtomicUsize::new(0));
        let connection = Arc::new(Self {
            writer: writer.clone(),
            undelivered: undelivered.clone(),
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
                let serve = |connection: IpcConnection, done: Box<dyn FnOnce() + Send>| {
                    let link = link.clone();
                    let hub = hub.clone();
                    std::thread::spawn(move || {
                        crate::control::serve_connection(
                            connection,
                            link,
                            hub,
                            RequestOrigin::Bridged { runtime: epoch },
                        );
                        done();
                    });
                };
                let ended = relay(reader, &writer, serve, &report, &undelivered);
                if let Some(connection) = writer.lock().unwrap().take() {
                    let _ = connection.shutdown(std::net::Shutdown::Both);
                }
                report(match ended {
                    Ended::Closed(detail) => RuntimeEvent::Lost(detail),
                    Ended::Violation(detail) => RuntimeEvent::Failed {
                        reason: Unavailable::RuntimeUnreachable {
                            detail: format!(
                                "the runtime broke protocol and was disconnected: {detail}"
                            ),
                        },
                        retry: false,
                    },
                });
            })
            .expect("spawn extension runtime thread");
        connection
    }

    /// The UI handled one runtime message.
    pub fn delivered(&self) {
        let _ = self
            .undelivered
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            });
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

/// Why a relay stopped.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// The channel closed or failed.
    Closed(String),
    /// The runtime went past a bound or broke the protocol.
    Violation(String),
}

/// One bridged connection as the relay sees it.
struct Bridge {
    to_handler: mpsc::Sender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
}

/// Serve the runtime's frames until the channel closes or the runtime misbehaves. `serve` starts a
/// control handler for a new bridged connection and calls the callback it is given once that
/// handler is done.
fn relay(
    mut reader: impl Read,
    writer: &SharedWriter,
    serve: impl Fn(IpcConnection, Box<dyn FnOnce() + Send>),
    report: &impl Fn(RuntimeEvent),
    undelivered: &AtomicUsize,
) -> Ended {
    let mut bridges: HashMap<u64, Bridge> = HashMap::new();
    let handlers = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    loop {
        let frame = match read_frame(&mut reader, false) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ended::Closed("the runtime closed its channel".to_string()),
            Err(error) => return Ended::Closed(error.to_string()),
        };
        match frame {
            Frame::Message(Message::BridgeOpen { conn }) => {
                if bridges.contains_key(&conn) {
                    return Ended::Violation(format!("opened bridge connection {conn} twice"));
                }
                if handlers.load(Ordering::Acquire) >= MAX_BRIDGES {
                    return Ended::Violation(format!(
                        "opened more than {MAX_BRIDGES} bridge connections at once"
                    ));
                }
                let (to_handler, inbound) = mpsc::channel();
                let queued = Arc::new(AtomicUsize::new(0));
                bridges.insert(
                    conn,
                    Bridge {
                        to_handler,
                        queued: queued.clone(),
                    },
                );
                // A small read-ahead buffer, so what the runtime sends stays counted in the relay's
                // queue - where its bound is enforced - instead of piling up behind it.
                let connection =
                    IpcConnection::from_piped(PipedConnection::from_reader_writer_bounded(
                        BridgeWriter {
                            conn,
                            out: writer.clone(),
                        },
                        ChannelReader {
                            inbound,
                            pending: Vec::new(),
                            offset: 0,
                            queued,
                            total: total.clone(),
                        },
                        BRIDGE_READ_AHEAD,
                    ));
                handlers.fetch_add(1, Ordering::AcqRel);
                let finished = handlers.clone();
                serve(
                    connection,
                    Box::new(move || {
                        finished.fetch_sub(1, Ordering::AcqRel);
                    }),
                );
            }
            Frame::Message(Message::BridgeClose { conn }) => {
                bridges.remove(&conn);
            }
            Frame::Bridge { conn, data } => {
                // Bytes for a connection already closed on this side are a race, not an attack.
                let Some(bridge) = bridges.get(&conn) else {
                    continue;
                };
                let len = data.len();
                if bridge.queued.load(Ordering::Acquire) + len > MAX_BRIDGE_QUEUED {
                    return Ended::Violation(format!(
                        "queued more than {MAX_BRIDGE_QUEUED} bytes on bridge connection {conn}"
                    ));
                }
                if total.load(Ordering::Acquire) + len > MAX_QUEUED {
                    return Ended::Violation(format!(
                        "queued more than {MAX_QUEUED} bytes across its bridge connections"
                    ));
                }
                bridge.queued.fetch_add(len, Ordering::AcqRel);
                total.fetch_add(len, Ordering::AcqRel);
                if bridge.to_handler.send(data).is_err() {
                    // The handler is gone; nothing will drain what was just counted.
                    bridge.queued.fetch_sub(len, Ordering::AcqRel);
                    total.fetch_sub(len, Ordering::AcqRel);
                    bridges.remove(&conn);
                }
            }
            Frame::Message(message) => {
                let size = serde_json::to_vec(&message).map_or(0, |bytes| bytes.len());
                if size > MAX_MESSAGE_BYTES {
                    return Ended::Violation(format!("sent a {size}-byte message"));
                }
                if undelivered.fetch_add(1, Ordering::AcqRel) >= MAX_UNDELIVERED {
                    return Ended::Violation(format!(
                        "sent more than {MAX_UNDELIVERED} messages faster than they were handled"
                    ));
                }
                report(RuntimeEvent::Message(message));
            }
            Frame::Hello(_) | Frame::Stage { .. } => {
                return Ended::Violation("sent a frame only a client sends".to_string());
            }
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

/// The bytes a bridged process sent, fed to the handler as a stream. Bytes count as queued until
/// the handler takes them off the channel.
struct ChannelReader {
    inbound: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
    queued: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

impl ChannelReader {
    fn release(&self, len: usize) {
        self.queued.fetch_sub(len, Ordering::AcqRel);
        self.total.fetch_sub(len, Ordering::AcqRel);
    }
}

impl Drop for ChannelReader {
    /// Whatever was still queued for a handler that has gone is no longer held.
    fn drop(&mut self) {
        self.release(self.pending.len() - self.offset);
        while let Ok(data) = self.inbound.try_recv() {
            self.release(data.len());
        }
    }
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
        // Bytes stop counting as queued only once they are actually read, so a chunk sitting
        // here half-consumed is still inside the bound.
        let n = buf.len().min(self.pending.len() - self.offset);
        buf[..n].copy_from_slice(&self.pending[self.offset..self.offset + n]);
        self.offset += n;
        self.release(n);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the relay over `frames` from a runtime that never authenticates anything. Handlers are
    /// held and never read, the way a wedged or slow control handler would behave. Returns how the
    /// relay ended and how many handlers it started.
    fn relay_frames(frames: Vec<u8>, undelivered: &AtomicUsize) -> (Ended, usize) {
        let (_runtime_side, to_runtime) = std::io::pipe().unwrap();
        let (from_nowhere, _keep) = std::io::pipe().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(Some(IpcConnection::from_piped(
            PipedConnection::from_reader_writer(to_runtime, from_nowhere),
        ))));
        let held = Mutex::new(Vec::new());
        let ended = relay(
            frames.as_slice(),
            &writer,
            |connection, done| held.lock().unwrap().push((connection, done)),
            &|_| {},
            undelivered,
        );
        let started = held.lock().unwrap().len();
        (ended, started)
    }

    fn open(frames: &mut Vec<u8>, conn: u64) {
        protocol::write_message(frames, &Message::BridgeOpen { conn }).unwrap();
    }

    #[test]
    fn a_runtime_opening_bridges_without_end_is_cut_off_at_the_bound() {
        let mut frames = Vec::new();
        for conn in 0..10_000 {
            open(&mut frames, conn);
        }
        let (ended, started) = relay_frames(frames, &AtomicUsize::new(0));
        assert!(matches!(ended, Ended::Violation(_)), "{ended:?}");
        assert_eq!(started, MAX_BRIDGES);
    }

    #[test]
    fn a_bridge_opened_twice_is_a_violation() {
        let mut frames = Vec::new();
        open(&mut frames, 1);
        open(&mut frames, 1);
        let (ended, started) = relay_frames(frames, &AtomicUsize::new(0));
        assert!(matches!(ended, Ended::Violation(_)));
        assert_eq!(started, 1);
    }

    /// Bytes for a handler that is not reading pile up only to the bound, per connection and
    /// across all of them.
    #[test]
    fn bytes_a_handler_has_not_read_are_bounded() {
        let chunk = vec![b'x'; 1024 * 1024];
        let mut one = Vec::new();
        open(&mut one, 1);
        for _ in 0..=(MAX_BRIDGE_QUEUED / chunk.len()) {
            protocol::write_bridge(&mut one, 1, &chunk).unwrap();
        }
        let (ended, _) = relay_frames(one, &AtomicUsize::new(0));
        assert!(
            matches!(&ended, Ended::Violation(detail) if detail.contains("on bridge connection 1")),
            "{ended:?}"
        );

        let mut many = Vec::new();
        for conn in 0..MAX_BRIDGES as u64 {
            open(&mut many, conn);
        }
        'fill: for _ in 0..(MAX_BRIDGE_QUEUED / chunk.len()) {
            for conn in 0..MAX_BRIDGES as u64 {
                protocol::write_bridge(&mut many, conn, &chunk).unwrap();
                if many.len() > MAX_QUEUED + 8 * chunk.len() {
                    break 'fill;
                }
            }
        }
        let (ended, _) = relay_frames(many, &AtomicUsize::new(0));
        assert!(
            matches!(&ended, Ended::Violation(detail) if detail.contains("across")),
            "{ended:?}"
        );
    }

    #[test]
    fn bytes_for_a_connection_never_opened_are_dropped_not_held() {
        let mut frames = Vec::new();
        for _ in 0..64 {
            protocol::write_bridge(&mut frames, 99, &[b'x'; 64 * 1024]).unwrap();
        }
        let (ended, started) = relay_frames(frames, &AtomicUsize::new(0));
        assert!(matches!(ended, Ended::Closed(_)));
        assert_eq!(started, 0);
    }

    #[test]
    fn messages_faster_than_the_ui_handles_them_or_too_large_are_violations() {
        let mut flood = Vec::new();
        for worker in 0..=(MAX_UNDELIVERED as u64) {
            protocol::write_message(&mut flood, &Message::Spawned { worker, pid: 1 }).unwrap();
        }
        let undelivered = AtomicUsize::new(0);
        let (ended, _) = relay_frames(flood, &undelivered);
        assert!(matches!(ended, Ended::Violation(_)), "{ended:?}");
        assert_eq!(undelivered.load(Ordering::Acquire), MAX_UNDELIVERED + 1);

        let mut large = Vec::new();
        protocol::write_message(
            &mut large,
            &Message::StageFailed {
                digest: "a".repeat(64),
                detail: "x".repeat(MAX_MESSAGE_BYTES),
            },
        )
        .unwrap();
        let (ended, _) = relay_frames(large, &AtomicUsize::new(0));
        assert!(matches!(ended, Ended::Violation(_)), "{ended:?}");

        let mut client_only = Vec::new();
        protocol::write_message(&mut client_only, &Message::Kill { worker: 1 }).unwrap();
        // Kill is only reported, and judged by the UI; a stage frame is refused here.
        protocol::write_stage(&mut client_only, &"a".repeat(64), b"x").unwrap();
        let (ended, _) = relay_frames(client_only, &AtomicUsize::new(0));
        assert!(matches!(ended, Ended::Violation(_)), "{ended:?}");
    }

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
