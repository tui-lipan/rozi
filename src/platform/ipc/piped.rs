//! Pipe-backed [`super::IpcConnection`] transport for remote SSH sessions.
//!
//! Child stdin/stdout (or any pair of pipe ends) do not expose socket-style read timeouts on either
//! platform. A dedicated reader pump thread feeds an in-process buffer so [`PipedConnection::read`]
//! can honor [`set_read_timeout`](PipedConnection::set_read_timeout) via `Condvar::wait_timeout`,
//! matching the socket path's deadline semantics uniformly on Unix and Windows.
//!
//! [`set_write_timeout`](PipedConnection::set_write_timeout) is a documented no-op: pipe writes are
//! not given a deadline here (SSH backpressure is handled by the OS pipe buffer and keepalive).
//!
//! [`peer_pid`](PipedConnection::peer_pid) is always `None`. Callers that fall back to
//! `terminate_server` (notably `ops::session::shutdown_session`) must skip that path for remote
//! connections — a local pid would be the ssh client, not the remote session server.
//!
//! ## Child lifetime
//!
//! The reader pump holds only the shared buffer/`writer` state — never the `Child`. The child is
//! owned by an [`OwnedChild`] arc shared across connection clones. Dropping the last connection
//! kills and waits on the child so its stdout closes, the pump unblocks, and no ssh process is
//! left behind.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PUMP_CHUNK: usize = 16 * 1024;

struct PipedBuf {
    bytes: VecDeque<u8>,
    /// Sticky terminal error / EOF once the pump exits. `Ok(())` means clean EOF.
    done: Option<io::Result<()>>,
}

struct PipedShared {
    /// `None` once the last connection handle closes the write half (EOF to the remote).
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    buf: Mutex<PipedBuf>,
    data: Condvar,
    pump_started: AtomicBool,
}

/// Owns the ssh (or other) child so dropping the last connection reaps it.
struct OwnedChild {
    child: Mutex<Option<Child>>,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Shared pipe duplex used as an [`super::IpcConnection`] variant.
pub struct PipedConnection {
    shared: Arc<PipedShared>,
    child: Option<Arc<OwnedChild>>,
    read_timeout: std::cell::Cell<Option<Duration>>,
    nonblocking: std::cell::Cell<bool>,
}

impl PipedConnection {
    /// Wrap already-taken child stdio. `child` is reaped when the last connection clone drops.
    pub fn from_child_stdio(
        stdin: impl Write + Send + 'static,
        stdout: impl Read + Send + 'static,
        child: Child,
    ) -> Self {
        Self::new(
            Box::new(stdin),
            Box::new(stdout),
            Some(Arc::new(OwnedChild {
                child: Mutex::new(Some(child)),
            })),
        )
    }

    /// Test / local helper: any reader/writer pair (for example [`std::io::pipe`]).
    pub fn from_reader_writer(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
    ) -> Self {
        Self::new(Box::new(writer), Box::new(reader), None)
    }

    fn new(
        writer: Box<dyn Write + Send>,
        mut reader: Box<dyn Read + Send>,
        child: Option<Arc<OwnedChild>>,
    ) -> Self {
        let shared = Arc::new(PipedShared {
            writer: Mutex::new(Some(writer)),
            buf: Mutex::new(PipedBuf {
                bytes: VecDeque::new(),
                done: None,
            }),
            data: Condvar::new(),
            pump_started: AtomicBool::new(false),
        });
        let pump = Arc::clone(&shared);
        // Only one pump per shared state; try_clone must not spawn another.
        if !pump.pump_started.swap(true, Ordering::SeqCst) {
            thread::spawn(move || {
                let mut chunk = vec![0u8; PUMP_CHUNK];
                loop {
                    let result = reader.read(&mut chunk);
                    let mut buf = pump.buf.lock().expect("piped buf");
                    match result {
                        Ok(0) => {
                            buf.done = Some(Ok(()));
                            pump.data.notify_all();
                            break;
                        }
                        Ok(n) => {
                            buf.bytes.extend(chunk[..n].iter().copied());
                            pump.data.notify_all();
                        }
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => {
                            buf.done = Some(Err(err));
                            pump.data.notify_all();
                            break;
                        }
                    }
                }
            });
        }
        Self {
            shared,
            child,
            read_timeout: std::cell::Cell::new(None),
            nonblocking: std::cell::Cell::new(false),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            shared: Arc::clone(&self.shared),
            child: self.child.clone(),
            read_timeout: std::cell::Cell::new(self.read_timeout.get()),
            nonblocking: std::cell::Cell::new(self.nonblocking.get()),
        })
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.set(nonblocking);
        Ok(())
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(timeout);
        Ok(())
    }

    /// No-op: pipe writes are not deadline-bounded (see module docs).
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    /// Always `None` — there is no meaningful local peer pid for a remote/proxy pipe.
    pub fn peer_pid(&self) -> Option<u32> {
        None
    }

    fn close_writer(&self) {
        if let Ok(mut writer) = self.shared.writer.lock() {
            *writer = None;
        }
    }

    fn read_inner(&self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let deadline = self
            .read_timeout
            .get()
            .map(|timeout| Instant::now() + timeout);
        let mut buf = self.shared.buf.lock().expect("piped buf");
        loop {
            if !buf.bytes.is_empty() {
                let n = out.len().min(buf.bytes.len());
                for slot in out.iter_mut().take(n) {
                    *slot = buf.bytes.pop_front().expect("len checked");
                }
                return Ok(n);
            }
            if let Some(done) = buf.done.as_ref() {
                return match done {
                    Ok(()) => Ok(0),
                    Err(err) => Err(io::Error::new(err.kind(), err.to_string())),
                };
            }
            if self.nonblocking.get() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "piped connection has no data ready",
                ));
            }
            match deadline {
                None => {
                    buf = self.shared.data.wait(buf).expect("piped wait");
                }
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "piped read timed out",
                        ));
                    }
                    let (guard, wait) = self
                        .shared
                        .data
                        .wait_timeout(buf, deadline - now)
                        .expect("piped wait_timeout");
                    buf = guard;
                    if wait.timed_out() && buf.bytes.is_empty() && buf.done.is_none() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "piped read timed out",
                        ));
                    }
                }
            }
        }
    }
}

impl Drop for PipedConnection {
    fn drop(&mut self) {
        // Connections + pump share `shared`. When this is the last connection (count == 2: us +
        // pump), close the write half so the remote sees EOF on stdin.
        if Arc::strong_count(&self.shared) <= 2 {
            self.close_writer();
        }
        // `self.child` drops next; when the last connection clone drops, OwnedChild kills ssh.
    }
}

impl Read for PipedConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_inner(buf)
    }
}

impl Write for PipedConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.shared.writer.lock().expect("piped writer");
        match guard.as_mut() {
            Some(writer) => writer.write(buf),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "piped connection writer closed",
            )),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.shared.writer.lock().expect("piped writer");
        match guard.as_mut() {
            Some(writer) => writer.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{self, ClientMessage, ServerMessage};
    use std::io::pipe;
    #[cfg(unix)]
    use std::process::Command;

    #[test]
    fn peer_pid_is_always_none() {
        let (reader, writer) = pipe().unwrap();
        let conn = PipedConnection::from_reader_writer(writer, reader);
        assert_eq!(conn.peer_pid(), None);
        assert_eq!(conn.try_clone().unwrap().peer_pid(), None);
    }

    #[test]
    fn round_trip_protocol_frames_over_pipe() {
        let (server_reader, client_writer) = pipe().unwrap();
        let (client_reader, server_writer) = pipe().unwrap();

        let server = thread::spawn(move || {
            let mut conn = PipedConnection::from_reader_writer(server_writer, server_reader);
            let msg: ClientMessage = protocol::read_frame(&mut conn).unwrap();
            assert_eq!(msg, ClientMessage::Detach);
            protocol::write_frame(&mut conn, &ServerMessage::Ping { seq: 42 }).unwrap();
        });

        let mut client = PipedConnection::from_reader_writer(client_writer, client_reader);
        protocol::write_frame(&mut client, &ClientMessage::Detach).unwrap();
        let reply: ServerMessage = protocol::read_frame(&mut client).unwrap();
        assert_eq!(reply, ServerMessage::Ping { seq: 42 });
        server.join().unwrap();
    }

    #[test]
    fn read_timeout_returns_timed_out_when_idle() {
        let (reader, writer) = pipe().unwrap();
        let mut conn = PipedConnection::from_reader_writer(writer, reader);
        conn.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let started = Instant::now();
        let err = conn
            .read(&mut [0u8; 8])
            .expect_err("idle pipe must time out");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn forced_terminate_fallback_is_unreachable_without_peer_pid() {
        let (reader, writer) = pipe().unwrap();
        let conn = PipedConnection::from_reader_writer(writer, reader);
        // Mirrors ops::session::shutdown_session: only terminate when peer_pid is Some.
        let server_pid = conn.peer_pid();
        assert!(server_pid.is_none());
        let mut terminated = false;
        if let Some(_pid) = server_pid {
            terminated = true;
        }
        assert!(
            !terminated,
            "piped/remote connections must never reach terminate_server"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropping_connection_reaps_owned_child() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let conn = PipedConnection::from_child_stdio(stdin, stdout, child);
        drop(conn);
        // Give the kill/wait a moment; then confirm the pid is gone.
        thread::sleep(Duration::from_millis(50));
        let still_alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(
            !still_alive,
            "owned child pid {pid} must not survive PipedConnection drop"
        );
    }
}
