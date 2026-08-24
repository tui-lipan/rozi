//! Windows named-pipe IPC backend (cross-platform plan Phase 5, Milestone 2).
//!
//! # Endpoint identity
//!
//! An [`IpcEndpoint`] is still identified by a *path* on Windows, exactly as on Unix - but that path
//! is a **registry entry** in the runtime directory (`%LOCALAPPDATA%\rozi\run\session-dev.sock`),
//! not the transport itself. The transport is a named pipe whose name is derived deterministically
//! from the entry's file stem and the current user's SID:
//!
//! ```text
//! %LOCALAPPDATA%\rozi\run\session-dev.sock   ->  \\.\pipe\rozi.<user-sid>.session-dev
//! %LOCALAPPDATA%\rozi\run\control-4242.sock  ->  \\.\pipe\rozi.<user-sid>.control-4242
//! ```
//!
//! Pipe names are flat - the `pipename` portion may not contain a backslash - which is why the
//! components are dot-separated rather than nested. Keeping the *entry path* as the endpoint's
//! identity is what lets `control.rs`, `cli.rs`, `session/discovery.rs`, `ROZI_SOCKET`, and
//! `--socket` stay byte-for-byte the same code on both platforms: enumeration is still a `read_dir`,
//! retirement is still an unlink, and a stale entry is still just a file with nothing behind it.
//!
//! Per the plan, registry entries are **hints only**. Nothing trusts one: the pipe name is
//! recomputed from the SID rather than read out of the file, and every connection still has to
//! complete the authenticated protocol handshake before it can do anything.
//!
//! # Protections
//!
//! - Explicit current-user SID DACL on both the pipe and the registry entry
//!   ([`super::super::fs_security::private_security_descriptor`]), never the default pipe security.
//! - `PIPE_REJECT_REMOTE_CLIENTS`, so a pipe is unreachable over SMB even if a remote user could
//!   otherwise authenticate.
//! - Non-inheritable handles, so a spawned pane never inherits a live control endpoint.
//! - `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance, which fails closed if another process
//!   already owns the name - a squatter cannot silently interpose on the endpoint.
//! - A fresh instance is created *before* each `ConnectNamedPipe`, so there is never a moment where
//!   the name exists but has no instance available to accept the next client.
//!
//! # Blocking model
//!
//! Callers (`session/server`, `control.rs`) drive these connections with non-blocking reads/writes
//! and `WouldBlock`-based backpressure, plus occasional read/write *timeouts* on handshake paths.
//! Both map onto `PIPE_NOWAIT` plus polling here. The one subtlety worth stating outright: in
//! `PIPE_NOWAIT` mode a `ReadFile`/`WriteFile` with nothing to do succeeds with a count of **zero**,
//! which in Rust's `io::Read`/`io::Write` contract would mean "EOF" and "peer is gone". Those cases
//! are translated to `WouldBlock`; a genuine peer disconnect (`ERROR_BROKEN_PIPE`) is what becomes
//! `Ok(0)`.
//!
//! **Unverified at runtime**: this workspace has no Windows host. It type-checks under
//! `cargo check --target x86_64-pc-windows-gnu` and is written against documented API contracts.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND,
    ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe, SetNamedPipeHandleState,
    WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::platform::fs_security;

/// Pipe buffer sizes. Generous on the outbound side: the session server streams raw pane output and
/// relies on writes completing without blocking whenever the client is keeping up, and only falls
/// back to its own `outbox` backpressure when they do not.
const PIPE_OUT_BUFFER: u32 = 256 * 1024;
const PIPE_IN_BUFFER: u32 = 64 * 1024;

/// How long a `connect` waits for a busy pipe (every instance momentarily in use) before failing.
const CONNECT_BUSY_WAIT: u32 = 1_000;

/// Poll interval for the `PIPE_NOWAIT` + deadline emulation of read/write timeouts.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// A named-pipe endpoint, identified by its runtime-directory registry entry. See the module doc
/// comment for why the entry path - not the pipe name - is the identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpcEndpoint {
    path: PathBuf,
}

impl IpcEndpoint {
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `\\.\pipe\rozi.<user-sid>.<entry-stem>`.
    ///
    /// Derived, never read from the registry entry: a planted or edited entry can therefore only
    /// make an endpoint *undiscoverable*, never redirect a connection to a pipe someone else owns.
    fn pipe_name(&self) -> String {
        let stem = self
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unnamed");
        format!(
            r"\\.\pipe\rozi.{}.{}",
            crate::platform::user::current_user_tag(),
            stem
        )
    }

    fn wide_pipe_name(&self) -> Vec<u16> {
        self.pipe_name()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Bind a listener at this endpoint: create the first pipe instance, then publish the registry
    /// entry that makes it discoverable.
    ///
    /// A registry entry left behind by a crashed process (no pipe answers it) is replaced. A *live*
    /// one - or a squatter already holding the pipe name - fails the `FILE_FLAG_FIRST_PIPE_INSTANCE`
    /// creation, and that failure is propagated rather than worked around.
    pub fn bind(&self) -> io::Result<BoundEndpoint> {
        if self.path.exists() && !self.is_live() {
            let _ = std::fs::remove_file(&self.path);
        }
        let instance = PipeInstance::create(self, true)?;
        self.publish_registry_entry()?;
        Ok(BoundEndpoint {
            listener: IpcListener {
                endpoint: self.clone(),
                pending: std::cell::Cell::new(instance.into_raw()),
                nonblocking: std::cell::Cell::new(false),
            },
            endpoint: self.clone(),
        })
    }

    /// Write the discovery hint. Contents are informational (a human inspecting the run directory
    /// sees which pipe an entry stands for); nothing reads them back.
    fn publish_registry_entry(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs_security::ensure_private_dir(parent)?;
        }
        std::fs::write(&self.path, self.pipe_name())
    }

    pub fn connect(&self) -> io::Result<IpcConnection> {
        let name = self.wide_pipe_name();
        for attempt in 0..2 {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(IpcConnection::Local(LocalConnection::owning(handle, false)));
            }
            let err = io::Error::last_os_error();
            // Every instance is momentarily busy between one client connecting and the listener
            // creating the next instance. Wait for one to free up, once.
            if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && attempt == 0 {
                unsafe { WaitNamedPipeW(name.as_ptr(), CONNECT_BUSY_WAIT) };
                continue;
            }
            // `NotFound` is load-bearing: `session::bootstrap` reads it as "no server yet, start
            // one", so a missing pipe must not surface as some other error kind.
            if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no listener at {}", self.pipe_name()),
                ));
            }
            if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("{} is busy", self.pipe_name()),
                ));
            }
            return Err(err);
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("{} is busy", self.pipe_name()),
        ))
    }

    /// Whether a listener currently answers here. A *busy* pipe counts as live: every instance being
    /// momentarily in use means there is very much a server behind it.
    pub fn is_live(&self) -> bool {
        match self.connect() {
            Ok(_) => true,
            Err(err) => err.kind() == io::ErrorKind::WouldBlock,
        }
    }

    /// Drop the discovery hint. The pipe itself needs no cleanup: it ceases to exist when the last
    /// handle to it closes, which is precisely what makes a Windows endpoint incapable of going
    /// stale in the way a Unix socket file can. Only the registry entry can outlive its server, and
    /// this is what removes it.
    pub fn remove_stale(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One pipe instance, owned until it is either handed to a connected client or dropped.
struct PipeInstance(OwnedHandle);

impl PipeInstance {
    /// `first` passes `FILE_FLAG_FIRST_PIPE_INSTANCE`, which fails if the name already exists.
    /// Only the very first instance may pass it (by definition), and it must: without it, a process
    /// that squatted the name earlier would keep serving clients that believe they reached us.
    fn create(endpoint: &IpcEndpoint, first: bool) -> io::Result<Self> {
        let descriptor = fs_security::private_security_descriptor()?;
        let attributes = descriptor.attributes();
        let name = endpoint.wide_pipe_name();
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_OUT_BUFFER,
                PIPE_IN_BUFFER,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(OwnedHandle(handle)))
    }

    fn into_raw(self) -> HANDLE {
        let handle = self.0.0;
        std::mem::forget(self.0);
        handle
    }
}

/// A successfully bound endpoint: the listener plus the endpoint it is bound to.
pub struct BoundEndpoint {
    listener: IpcListener,
    endpoint: IpcEndpoint,
}

impl BoundEndpoint {
    pub fn listener(&self) -> &IpcListener {
        &self.listener
    }

    pub fn into_listener(self) -> IpcListener {
        self.listener
    }

    pub fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
    }

    pub fn path(&self) -> &Path {
        self.endpoint.path()
    }
}

/// Holds the *next* pipe instance, already created and waiting for `ConnectNamedPipe`. Accepting
/// hands that instance to the caller as the connection and immediately creates its replacement, so
/// the pipe name is never momentarily instance-less (which a client would see as `ERROR_PIPE_BUSY`).
pub struct IpcListener {
    endpoint: IpcEndpoint,
    pending: std::cell::Cell<HANDLE>,
    nonblocking: std::cell::Cell<bool>,
}

// SAFETY: a `HANDLE` is a plain kernel handle with no thread affinity, and every method takes `&self`
// with interior mutability confined to `Cell`s that are only touched from the owning thread (the
// accept loop). `control::run_listener` moves the whole listener to a worker thread, which is why
// `Send` is needed at all.
unsafe impl Send for IpcListener {}

impl IpcListener {
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.set(nonblocking);
        set_pipe_mode(self.pending.get(), nonblocking)
    }

    /// Accept one pending connection. Returns `WouldBlock` immediately when non-blocking and no
    /// client is waiting, so callers can poll this exactly as they poll a non-blocking Unix
    /// `accept`.
    pub fn accept(&self) -> io::Result<IpcConnection> {
        let pending = self.pending.get();
        loop {
            let connected = unsafe { ConnectNamedPipe(pending, std::ptr::null_mut()) } != 0;
            if connected {
                break;
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error().map(|code| code as u32) {
                // The client connected in the window between `CreateNamedPipeW` and here. Not an
                // error: the instance is connected, which is all we wanted.
                Some(ERROR_PIPE_CONNECTED) => break,
                // A short-lived discovery client connected and closed before this poll. Reset the
                // same instance instead of letting one abandoned probe terminate the server.
                Some(ERROR_NO_DATA) => {
                    unsafe { DisconnectNamedPipe(pending) };
                }
                // Non-blocking mode with nobody waiting.
                Some(ERROR_PIPE_LISTENING) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "no pending client",
                    ));
                }
                _ => return Err(err),
            }
        }

        // Replace the instance we are about to give away *before* handing it over, so the name
        // always has a free instance behind it.
        let next = PipeInstance::create(&self.endpoint, false)?;
        let next = next.into_raw();
        if self.nonblocking.get() {
            set_pipe_mode(next, true)?;
        }
        self.pending.set(next);

        // The accepted connection starts blocking regardless of the listener's mode; the caller
        // (`SessionServer::accept_new`) sets whatever mode it actually wants.
        set_pipe_mode(pending, false)?;
        Ok(IpcConnection::Local(LocalConnection::owning(pending, true)))
    }
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.pending.get()) };
    }
}

pub struct LocalConnection {
    handle: OwnedHandle,
    /// Which end of the pipe this is, which decides how peer_pid asks for the *other* end's pid.
    server_end: bool,
    shutdown_signal: Arc<AtomicBool>,
    /// `Cell`s because the Unix backend's `set_*_timeout`/`set_nonblocking` take `&self` (they
    /// forward to socket options, which need no Rust-side state) and both backends must present the
    /// same signature. Here these genuinely are Rust-side state - the pipe API has no equivalent
    /// option - so they need interior mutability.
    nonblocking: std::cell::Cell<bool>,
    read_timeout: std::cell::Cell<Option<Duration>>,
    write_timeout: std::cell::Cell<Option<Duration>>,
}

impl LocalConnection {
    fn owning(handle: HANDLE, server_end: bool) -> Self {
        Self {
            handle: OwnedHandle(handle),
            server_end,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            nonblocking: std::cell::Cell::new(false),
            read_timeout: std::cell::Cell::new(None),
            write_timeout: std::cell::Cell::new(None),
        }
    }
}

// SAFETY: as for `IpcListener` - a kernel handle has no thread affinity, and the connection is
// owned by exactly one thread at a time (moved into a per-connection worker in `control.rs`, held by
// the single-threaded server loop otherwise).
unsafe impl Send for LocalConnection {}
unsafe impl Send for IpcConnection {}

pub enum IpcConnection {
    Local(LocalConnection),
    Piped(super::piped::PipedConnection),
}

impl IpcConnection {
    pub fn connect(endpoint: &IpcEndpoint) -> io::Result<Self> {
        endpoint.connect()
    }

    pub fn from_piped(piped: super::piped::PipedConnection) -> Self {
        Self::Piped(piped)
    }

    /// A second handle to the same pipe instance, so a caller can read on one and write on the
    /// other (`control.rs` does exactly this).
    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Local(local) => Ok(Self::Local(local.try_clone()?)),
            Self::Piped(piped) => Ok(Self::Piped(piped.try_clone()?)),
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Self::Local(local) => local.set_nonblocking(nonblocking),
            Self::Piped(piped) => piped.set_nonblocking(nonblocking),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Local(local) => local.set_read_timeout(timeout),
            Self::Piped(piped) => piped.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Local(local) => local.set_write_timeout(timeout),
            Self::Piped(piped) => piped.set_write_timeout(timeout),
        }
    }

    /// The pid of the process at the *other* end. Best-effort; `None` on any failure.
    /// Always `None` for [`Self::Piped`] (remote/proxy).
    pub fn peer_pid(&self) -> Option<u32> {
        match self {
            Self::Local(local) => local.peer_pid(),
            Self::Piped(piped) => piped.peer_pid(),
        }
    }

    pub fn shutdown(&self, _how: std::net::Shutdown) -> io::Result<()> {
        match self {
            Self::Local(local) => {
                local.shutdown_signal.store(true, Ordering::SeqCst);
                if local.server_end {
                    unsafe {
                        windows_sys::Win32::System::Pipes::DisconnectNamedPipe(local.handle.0)
                    };
                } else {
                    unsafe {
                        windows_sys::Win32::System::IO::CancelIoEx(
                            local.handle.0,
                            std::ptr::null_mut(),
                        )
                    };
                }
                Ok(())
            }
            Self::Piped(piped) => piped.shutdown(),
        }
    }
}

impl LocalConnection {
    /// A second handle to the same pipe instance, so a caller can read on one and write on the
    /// other (`control.rs` does exactly this).
    ///
    /// Note that pipe *mode* (`PIPE_WAIT`/`PIPE_NOWAIT`) is a property of the underlying pipe
    /// instance, not of the handle, so a mode change through either clone is seen by both. That
    /// matches how the clones are actually used - one reads, one writes, and the only caller that
    /// toggles the mode does so around a single operation - but it is not the independent-per-handle
    /// behavior a Unix `try_clone` would give.
    pub fn try_clone(&self) -> io::Result<Self> {
        let mut duplicate: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.0,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0, // non-inheritable: a spawned pane must never inherit a live endpoint.
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let clone = Self {
            handle: OwnedHandle(duplicate),
            server_end: self.server_end,
            shutdown_signal: Arc::clone(&self.shutdown_signal),
            nonblocking: std::cell::Cell::new(self.nonblocking.get()),
            read_timeout: std::cell::Cell::new(self.read_timeout.get()),
            write_timeout: std::cell::Cell::new(self.write_timeout.get()),
        };
        Ok(clone)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.set(nonblocking);
        set_pipe_mode(self.handle.0, self.needs_nowait())
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(timeout);
        set_pipe_mode(self.handle.0, self.needs_nowait())
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout.set(timeout);
        set_pipe_mode(self.handle.0, self.needs_nowait())
    }

    /// Non-blocking mode *or* a timeout on either direction forces the pipe out of `PIPE_WAIT`,
    /// because both are emulated the same way: the operation returns immediately with zero bytes and
    /// `read`/`write` decide what that means (see [`IpcConnection::wait_or_block`]).
    ///
    /// This is per-*instance*, not per-handle (see [`IpcConnection::try_clone`]), which is why the
    /// decision cannot be "does *this* operation have a deadline" - a clone reading with a timeout
    /// puts the instance into non-waiting mode, and the sibling clone writing without one must still
    /// behave as a blocking write. It does: with no deadline and `nonblocking` unset, `wait_or_block`
    /// spins rather than reporting `WouldBlock`.
    fn needs_nowait(&self) -> bool {
        self.nonblocking.get()
            || self.read_timeout.get().is_some()
            || self.write_timeout.get().is_some()
    }

    /// The pid of the process at the *other* end. Best-effort; `None` on any failure.
    ///
    /// Used only as a liveness/reaping aid (`ops::session`'s forced termination of a server that
    /// would not shut down through the protocol), never as an authorization check - authorization is
    /// the pipe's DACL plus the protocol handshake.
    pub fn peer_pid(&self) -> Option<u32> {
        let mut pid: u32 = 0;
        let ok = unsafe {
            if self.server_end {
                GetNamedPipeClientProcessId(self.handle.0, &mut pid)
            } else {
                GetNamedPipeServerProcessId(self.handle.0, &mut pid)
            }
        };
        (ok != 0 && pid != 0).then_some(pid)
    }
}

impl Read for IpcConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Local(local) => local.read(buf),
            Self::Piped(piped) => piped.read(buf),
        }
    }
}

impl Read for LocalConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let deadline = self
            .read_timeout
            .get()
            .map(|timeout| Instant::now() + timeout);
        loop {
            if self.shutdown_signal.load(Ordering::Relaxed) {
                return Ok(0);
            }
            let mut read: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    self.handle.0,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 && read > 0 {
                return Ok(read as usize);
            }
            if ok != 0 {
                // Succeeded with zero bytes: `PIPE_NOWAIT` with nothing buffered. This is *not*
                // EOF - reporting `Ok(0)` here would make every caller conclude the peer had hung
                // up (see the module doc comment).
                match self.wait_or_block(deadline)? {
                    Waited::Retry => continue,
                    Waited::WouldBlock => {
                        return Err(io::Error::new(io::ErrorKind::WouldBlock, "no data"));
                    }
                }
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error().map(|code| code as u32) {
                // The genuine "peer closed the pipe" case, and the only thing that may become the
                // `Ok(0)` that means EOF to `io::Read`.
                Some(ERROR_BROKEN_PIPE) => return Ok(0),
                Some(ERROR_NO_DATA) => match self.wait_or_block(deadline)? {
                    Waited::Retry => continue,
                    Waited::WouldBlock => {
                        return Err(io::Error::new(io::ErrorKind::WouldBlock, "no data"));
                    }
                },
                _ => return Err(err),
            }
        }
    }
}

impl Write for IpcConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Local(local) => local.write(buf),
            Self::Piped(piped) => piped.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Local(local) => local.flush(),
            Self::Piped(piped) => piped.flush(),
        }
    }
}

impl Write for LocalConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = self
            .write_timeout
            .get()
            .map(|timeout| Instant::now() + timeout);
        loop {
            if self.shutdown_signal.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "connection shut down",
                ));
            }
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteFile(
                    self.handle.0,
                    buf.as_ptr(),
                    buf.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 && written > 0 {
                return Ok(written as usize);
            }
            if ok != 0 {
                // Succeeded with zero bytes written: the outbound buffer is full and the pipe is in
                // non-waiting mode. `Ok(0)` would tell the session server's `flush_clients` that the
                // client is dead and evict it, so this must be `WouldBlock` - which is exactly the
                // signal its backpressure/eviction logic wants.
                match self.wait_or_block(deadline)? {
                    Waited::Retry => continue,
                    Waited::WouldBlock => {
                        return Err(io::Error::new(io::ErrorKind::WouldBlock, "pipe full"));
                    }
                }
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error().map(|code| code as u32) {
                // On a *write*, `ERROR_NO_DATA` means the pipe is being closed - not "nothing to do"
                // as it does on a read. Both it and `ERROR_BROKEN_PIPE` mean the peer is gone.
                Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA) => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"));
                }
                _ => return Err(err),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // Deliberately *not* `FlushFileBuffers`: on a named pipe that blocks until the peer has
        // drained everything written, which would defeat the non-blocking write path above and
        // stall the session server's broadcast loop behind its slowest client.
        Ok(())
    }
}

enum Waited {
    /// Sleep elapsed; the caller should retry the operation.
    Retry,
    /// The caller asked for non-blocking behavior: report `WouldBlock`.
    WouldBlock,
}

impl LocalConnection {
    /// Decide what a zero-byte non-waiting operation should do, in the three cases the caller can
    /// have asked for:
    ///
    /// - non-blocking (`set_nonblocking(true)`) - report `WouldBlock` immediately.
    /// - a timeout on this direction - spin until the deadline, then `TimedOut`.
    /// - neither - spin indefinitely, emulating a blocking operation. This case is not hypothetical:
    ///   a `try_clone`d sibling with a read timeout puts the shared pipe instance into non-waiting
    ///   mode, so a write with no timeout of its own still lands here and must block, not fail.
    fn wait_or_block(&self, deadline: Option<Instant>) -> io::Result<Waited> {
        if self.shutdown_signal.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection shut down",
            ));
        }
        if self.nonblocking.get() {
            return Ok(Waited::WouldBlock);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        // Cheap liveness check while spinning: a peer that hangs up mid-wait must surface
        // immediately rather than after the whole timeout elapses - or, with no deadline, never.
        let mut available: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                self.handle.0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"));
            }
        }
        std::thread::sleep(POLL_INTERVAL);
        Ok(Waited::Retry)
    }
}

/// `PIPE_NOWAIT` when `nowait`, `PIPE_WAIT` otherwise. The pipe stays byte-mode in both.
fn set_pipe_mode(handle: HANDLE, nowait: bool) -> io::Result<()> {
    let mode = PIPE_READMODE_BYTE | if nowait { PIPE_NOWAIT } else { PIPE_WAIT };
    let ok = unsafe { SetNamedPipeHandleState(handle, &mode, std::ptr::null(), std::ptr::null()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Closes its handle on drop, so none of the `?` early-returns above can leak one.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Endpoints are registry entries under a private directory, so each test needs its own to avoid
    /// colliding on the derived pipe name when the suite runs in parallel.
    fn temp_endpoint(name: &str) -> IpcEndpoint {
        let dir = crate::test_support::private_temp_dir("ipc-win-test");
        IpcEndpoint::at_path(dir.join(format!("{name}-{}.sock", std::process::id())))
    }

    #[test]
    fn bind_then_connect_round_trips_bytes_in_both_directions() {
        let endpoint = temp_endpoint("roundtrip");
        endpoint.remove_stale();
        let bound = endpoint.bind().expect("bind");
        let listener = bound.into_listener();
        listener.set_nonblocking(true).expect("nonblocking");

        let mut client = endpoint.connect().expect("connect");

        let mut server_conn = loop {
            match listener.accept() {
                Ok(conn) => break conn,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };

        client.write_all(b"ping").expect("client write");
        let mut buf = [0u8; 4];
        server_conn.read_exact(&mut buf).expect("server read");
        assert_eq!(&buf, b"ping");

        server_conn.write_all(b"pong").expect("server write");
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).expect("client read");
        assert_eq!(&buf, b"pong");

        endpoint.remove_stale();
    }

    #[test]
    fn a_nonblocking_read_with_no_data_reports_would_block_not_eof() {
        // The single most load-bearing behavior of this backend: `PIPE_NOWAIT` reports "nothing to
        // read" as a *successful zero-byte read*, which `io::Read` would take for EOF - and the
        // session server would take for "this client hung up" and evict it on its first idle poll.
        let endpoint = temp_endpoint("wouldblock");
        endpoint.remove_stale();
        let bound = endpoint.bind().expect("bind");
        let listener = bound.into_listener();
        listener.set_nonblocking(true).expect("nonblocking");

        let _client = endpoint.connect().expect("connect");
        let mut server_conn = loop {
            match listener.accept() {
                Ok(conn) => break conn,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };
        server_conn.set_nonblocking(true).expect("nonblocking");

        let mut buf = [0u8; 16];
        let err = server_conn
            .read(&mut buf)
            .expect_err("a read with nothing buffered must not succeed with 0 bytes");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);

        endpoint.remove_stale();
    }

    #[test]
    fn a_closed_peer_reads_as_eof() {
        // The other half of the same distinction: a *genuine* disconnect is the one thing that may
        // become `Ok(0)`.
        let endpoint = temp_endpoint("eof");
        endpoint.remove_stale();
        let bound = endpoint.bind().expect("bind");
        let listener = bound.into_listener();
        listener.set_nonblocking(true).expect("nonblocking");

        let client = endpoint.connect().expect("connect");
        let mut server_conn = loop {
            match listener.accept() {
                Ok(conn) => break conn,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };
        drop(client);

        let mut buf = [0u8; 16];
        assert_eq!(
            server_conn.read(&mut buf).expect("read after peer close"),
            0
        );

        endpoint.remove_stale();
    }

    #[test]
    fn each_end_reports_the_other_ends_pid() {
        let endpoint = temp_endpoint("peerpid");
        endpoint.remove_stale();
        let bound = endpoint.bind().expect("bind");
        let listener = bound.into_listener();
        listener.set_nonblocking(true).expect("nonblocking");

        let client = endpoint.connect().expect("connect");
        let server_conn = loop {
            match listener.accept() {
                Ok(conn) => break conn,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };

        // Both ends are this process here, so both directions must report this pid - which is
        // exactly what distinguishes a working `GetNamedPipe{Client,Server}ProcessId` pairing from
        // one that asks the wrong end and gets nothing.
        assert_eq!(client.peer_pid(), Some(std::process::id()));
        assert_eq!(server_conn.peer_pid(), Some(std::process::id()));

        endpoint.remove_stale();
    }

    #[test]
    fn a_stale_registry_entry_is_replaced_and_a_live_one_is_not_squattable() {
        let endpoint = temp_endpoint("stale");
        endpoint.remove_stale();

        // A registry entry with no pipe behind it (a crashed server) must not block a rebind.
        std::fs::write(endpoint.path(), "stale").expect("plant a stale entry");
        assert!(!endpoint.is_live());
        let bound = endpoint.bind().expect("rebind over a stale entry");

        // While that bind is live, a second one must fail closed rather than quietly interposing on
        // the name (FILE_FLAG_FIRST_PIPE_INSTANCE).
        assert!(endpoint.is_live());
        assert!(
            endpoint.bind().is_err(),
            "a second bind on a live endpoint must fail"
        );

        drop(bound);
        endpoint.remove_stale();
    }
}
