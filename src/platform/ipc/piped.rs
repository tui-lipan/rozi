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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PUMP_CHUNK: usize = 16 * 1024;
const PIPED_BUFFER_CAPACITY: usize = 8 * 1024 * 1024;
const PIPED_BUFFER_LOW_WATER: usize = 4 * 1024 * 1024;

struct PipedBuf {
    bytes: VecDeque<u8>,
    /// Sticky terminal error / EOF once the pump exits. `Ok(())` means clean EOF.
    done: Option<io::Result<()>>,
    shutdown: bool,
    high_water: usize,
}

impl PipedBuf {
    fn read_into(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.bytes.len());
        let (first, second) = self.bytes.as_slices();
        let first_n = n.min(first.len());
        out[..first_n].copy_from_slice(&first[..first_n]);
        let second_n = n - first_n;
        out[first_n..n].copy_from_slice(&second[..second_n]);
        self.bytes.drain(..n);
        n
    }

    fn stats(&self) -> PipedBufferStats {
        PipedBufferStats {
            current: self.bytes.len(),
            high_water: self.high_water,
            capacity: PIPED_BUFFER_CAPACITY,
        }
    }
}

struct PipedShared {
    /// `None` once the last connection handle closes the write half (EOF to the remote).
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    buf: Mutex<PipedBuf>,
    data: Condvar,
    connections: AtomicUsize,
}

impl PipedShared {
    fn shutdown(&self) {
        let mut buf = self.buf.lock().expect("piped buf");
        buf.shutdown = true;
        if buf.done.is_none() {
            buf.done = Some(Ok(()));
        }
        self.data.notify_all();
    }
}

/// Current and peak memory retained by a pipe reader pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipedBufferStats {
    pub current: usize,
    pub high_water: usize,
    pub capacity: usize,
}

/// Cheap cloneable observer for a pipe reader buffer. It keeps no transport handle alive.
#[derive(Clone)]
pub struct PipedBufferStatsHandle {
    shared: std::sync::Weak<PipedShared>,
}

impl PipedBufferStatsHandle {
    pub fn stats(&self) -> Option<PipedBufferStats> {
        let shared = self.shared.upgrade()?;
        Some(shared.buf.lock().expect("piped buf").stats())
    }
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
                shutdown: false,
                high_water: 0,
            }),
            data: Condvar::new(),
            connections: AtomicUsize::new(1),
        });
        let pump = Arc::clone(&shared);
        thread::spawn(move || {
            let mut chunk = vec![0u8; PUMP_CHUNK];
            loop {
                let result = reader.read(&mut chunk);
                if result
                    .as_ref()
                    .is_err_and(|err| err.kind() == io::ErrorKind::Interrupted)
                {
                    continue;
                }
                let mut buf = pump.buf.lock().expect("piped buf");
                match result {
                    Ok(0) => {
                        if buf.done.is_none() {
                            buf.done = Some(Ok(()));
                        }
                        pump.data.notify_all();
                        break;
                    }
                    Ok(n) => {
                        if buf.bytes.len() + n > PIPED_BUFFER_CAPACITY {
                            while !buf.shutdown && buf.bytes.len() > PIPED_BUFFER_LOW_WATER {
                                buf = pump.data.wait(buf).expect("piped producer wait");
                            }
                        }
                        if buf.shutdown {
                            break;
                        }
                        debug_assert!(buf.bytes.len() + n <= PIPED_BUFFER_CAPACITY);
                        buf.bytes.extend(&chunk[..n]);
                        buf.high_water = buf.high_water.max(buf.bytes.len());
                        pump.data.notify_all();
                    }
                    Err(err) => {
                        if buf.done.is_none() {
                            buf.done = Some(Err(err));
                        }
                        pump.data.notify_all();
                        break;
                    }
                }
            }
        });
        Self {
            shared,
            child,
            read_timeout: std::cell::Cell::new(None),
            nonblocking: std::cell::Cell::new(false),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        self.shared.connections.fetch_add(1, Ordering::Relaxed);
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

    pub fn shutdown(&self) -> io::Result<()> {
        self.close_writer();
        Ok(())
    }

    /// Snapshot the bounded reader buffer's current and peak retained byte counts.
    pub fn buffer_stats(&self) -> PipedBufferStats {
        self.shared.buf.lock().expect("piped buf").stats()
    }

    pub fn buffer_stats_handle(&self) -> PipedBufferStatsHandle {
        PipedBufferStatsHandle {
            shared: Arc::downgrade(&self.shared),
        }
    }

    fn close_writer(&self) {
        self.shared.shutdown();
        if let Some(child) = &self.child
            && let Ok(mut guard) = child.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut writer) = self.shared.writer.try_lock() {
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
                let was_above_low_water = buf.bytes.len() > PIPED_BUFFER_LOW_WATER;
                let n = buf.read_into(out);
                if was_above_low_water && buf.bytes.len() <= PIPED_BUFFER_LOW_WATER {
                    self.shared.data.notify_all();
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
        if self.shared.connections.fetch_sub(1, Ordering::AcqRel) == 1 {
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
        if self.shared.buf.lock().expect("piped buf").shutdown {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "piped connection shut down",
            ));
        }
        let mut guard = self.shared.writer.lock().expect("piped writer");
        if self.shared.buf.lock().expect("piped buf").shutdown {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "piped connection shut down",
            ));
        }
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
    use std::io::{Cursor, pipe};
    #[cfg(unix)]
    use std::process::Command;
    use std::sync::mpsc;

    struct DropSignalReader {
        inner: Cursor<Vec<u8>>,
        dropped: mpsc::Sender<()>,
    }

    impl Read for DropSignalReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl Drop for DropSignalReader {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }

    fn patterned_bytes(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    fn wait_for_buffer(
        conn: &PipedConnection,
        predicate: impl Fn(PipedBufferStats) -> bool,
    ) -> PipedBufferStats {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = conn.shared.buf.lock().expect("piped buf");
        loop {
            let stats = buf.stats();
            if predicate(stats) {
                return stats;
            }
            let now = Instant::now();
            assert!(now < deadline, "buffer condition timed out: {stats:?}");
            let (guard, wait) = conn
                .shared
                .data
                .wait_timeout(buf, deadline - now)
                .expect("wait for piped buffer");
            buf = guard;
            assert!(!wait.timed_out(), "buffer condition timed out: {stats:?}");
        }
    }

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
    fn reader_buffer_caps_then_resumes_losslessly_below_low_water() {
        let expected = patterned_bytes(PIPED_BUFFER_CAPACITY + 4 * 1024 * 1024);
        let mut conn =
            PipedConnection::from_reader_writer(io::sink(), Cursor::new(expected.clone()));

        let full = wait_for_buffer(&conn, |stats| stats.current == stats.capacity);
        assert_eq!(full.high_water, PIPED_BUFFER_CAPACITY);

        let drain_len = PIPED_BUFFER_CAPACITY - PIPED_BUFFER_LOW_WATER + 1;
        let mut received = vec![0; drain_len];
        conn.read_exact(&mut received)
            .expect("drain below low water");
        wait_for_buffer(&conn, |stats| stats.current > PIPED_BUFFER_LOW_WATER);
        conn.read_to_end(&mut received).expect("drain through EOF");

        assert_eq!(received, expected);
        let stats = conn.buffer_stats();
        assert_eq!(stats.current, 0);
        assert_eq!(stats.high_water, PIPED_BUFFER_CAPACITY);
        assert_eq!(stats.capacity, PIPED_BUFFER_CAPACITY);
    }

    #[test]
    fn close_writer_wakes_a_capacity_blocked_producer() {
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let reader = DropSignalReader {
            inner: Cursor::new(patterned_bytes(PIPED_BUFFER_CAPACITY + PUMP_CHUNK)),
            dropped: dropped_tx,
        };
        let mut conn = PipedConnection::from_reader_writer(io::sink(), reader);
        wait_for_buffer(&conn, |stats| stats.current == stats.capacity);

        conn.close_writer();
        dropped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("blocked producer must exit on close");

        let mut retained = Vec::new();
        conn.read_to_end(&mut retained)
            .expect("buffered bytes precede terminal EOF");
        assert_eq!(retained.len(), PIPED_BUFFER_CAPACITY);
    }

    #[test]
    fn final_drop_wakes_a_capacity_blocked_producer_without_joining() {
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let reader = DropSignalReader {
            inner: Cursor::new(patterned_bytes(PIPED_BUFFER_CAPACITY + PUMP_CHUNK)),
            dropped: dropped_tx,
        };
        let conn = PipedConnection::from_reader_writer(io::sink(), reader);
        wait_for_buffer(&conn, |stats| stats.current == stats.capacity);

        drop(conn);
        dropped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("blocked producer must exit after the final connection drops");
    }

    #[test]
    fn wrapped_buffer_reads_both_slices_and_drains_once() {
        let mut bytes = VecDeque::with_capacity(8);
        bytes.extend(0_u8..8);
        bytes.drain(..6);
        bytes.extend(8_u8..12);
        assert!(!bytes.as_slices().1.is_empty(), "fixture must wrap");
        let mut buf = PipedBuf {
            bytes,
            done: None,
            shutdown: false,
            high_water: 8,
        };
        let mut out = [0; 6];

        assert_eq!(buf.read_into(&mut out), out.len());
        assert_eq!(out, [6, 7, 8, 9, 10, 11]);
        assert!(buf.bytes.is_empty());
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

    #[test]
    fn piped_connection_shutdown_unblocks_immediately() {
        let (reader, writer) = pipe().unwrap();
        let mut conn = PipedConnection::from_reader_writer(writer, reader);
        let clone = conn.try_clone().unwrap();

        // Writing some bytes
        assert!(conn.write_all(b"hello").is_ok());

        // Shutdown one end
        assert!(clone.shutdown().is_ok());

        // Further writes should fail or be closed immediately
        let write_res = conn.write_all(b"world");
        assert!(write_res.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn shutdown_unblocks_a_backpressured_child_writer() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child that does not read stdin");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let conn = PipedConnection::from_child_stdio(stdin, stdout, child);
        let mut writer = conn.try_clone().expect("writer clone");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            writer.write_all(&vec![0; 8 * 1024 * 1024])
        });
        started_rx.recv().unwrap();
        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        conn.shutdown().expect("shutdown");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown must not wait behind the blocked writer mutex"
        );
        assert!(worker.join().expect("writer exits").is_err());
    }
}
