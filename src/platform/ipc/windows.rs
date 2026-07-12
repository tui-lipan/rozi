//! Windows named-pipe IPC backend (cross-platform plan Phase 5).
//!
//! **Not implemented and untested** - this environment has no Windows target to run it on. Per the
//! plan, this backend is meant to use duplex byte-mode named pipes with flat, dot-separated names
//! (`\\.\pipe\hyprmux.<user-sid>.control.<pid>`, `\\.\pipe\hyprmux.<user-sid>.session.<name>`), an
//! explicit current-user SID DACL, `PIPE_REJECT_REMOTE_CLIENTS`, non-inheritable handles, and the
//! "fresh instance before each `ConnectNamedPipe`, `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first
//! instance, fail closed otherwise" multi-instance accept pattern (Milestone 2).
//!
//! This stub exists so higher-level modules (`control.rs`, `session/*`, `cli.rs`, `ops/session.rs`)
//! can be written once against [`IpcEndpoint`]/[`IpcListener`]/[`IpcConnection`] without `cfg`
//! branching at every call site, and so `cargo check --target x86_64-pc-windows-*` gives an early
//! signal about anything in those call sites that does not type-check on Windows independent of
//! this backend's real implementation. Every operation fails closed with
//! [`io::ErrorKind::Unsupported`] rather than compiling to a silent no-op. Only compiled under
//! `cfg(windows)` (see `ipc/mod.rs`), so it cannot affect the Linux/macOS build either way.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "windows named-pipe IPC backend is not implemented yet (cross-platform plan Milestone 2)",
    )
}

/// Placeholder endpoint identity. Real Windows endpoints are named pipes, not filesystem paths;
/// [`path`](IpcEndpoint::path) is kept only so unmigrated call sites that still expect a `Path`
/// (there should be none left after Phase 5 lands) fail loudly instead of silently misbehaving.
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

    pub fn bind(&self) -> io::Result<BoundEndpoint> {
        Err(unsupported())
    }

    pub fn connect(&self) -> io::Result<IpcConnection> {
        Err(unsupported())
    }

    pub fn is_live(&self) -> bool {
        false
    }
}

pub struct BoundEndpoint {
    endpoint: IpcEndpoint,
}

impl BoundEndpoint {
    pub fn listener(&self) -> &IpcListener {
        unreachable!("BoundEndpoint is never constructed by this unimplemented backend")
    }

    pub fn into_listener(self) -> IpcListener {
        unreachable!("BoundEndpoint is never constructed by this unimplemented backend")
    }

    pub fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
    }

    pub fn path(&self) -> &Path {
        self.endpoint.path()
    }
}

pub struct IpcListener;

impl IpcListener {
    pub fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn accept(&self) -> io::Result<IpcConnection> {
        Err(unsupported())
    }
}

pub struct IpcConnection;

impl IpcConnection {
    pub fn connect(endpoint: &IpcEndpoint) -> io::Result<Self> {
        endpoint.connect()
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Err(unsupported())
    }

    pub fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Err(unsupported())
    }

    /// Windows process inspection is intentionally out of scope for this plan (no PEB/process-tree
    /// probing); always `None` rather than an error so cross-platform callers that treat this as a
    /// best-effort hint do not need Windows-specific branching.
    pub fn peer_pid(&self) -> Option<u32> {
        None
    }
}

impl Read for IpcConnection {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(unsupported())
    }
}

impl Write for IpcConnection {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(unsupported())
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(unsupported())
    }
}
