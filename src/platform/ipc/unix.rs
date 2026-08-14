//! Unix-domain-socket IPC backend (cross-platform plan Phase 5).
//!
//! Preserves Unix sockets and the current private-directory protections
//! ([`super::super::fs_security`]) on Linux and macOS. Peer identification uses `SO_PEERCRED` on
//! Linux and `LOCAL_PEERPID` on macOS - both return the connecting process's pid directly, which is
//! all any call site in this codebase needs today (the orphaned-server `SIGTERM` fallback in
//! `ops/session.rs`). Every endpoint still requires the full protocol attach handshake to do
//! anything; `peer_pid` is a liveness/reaping aid, not an authorization check, matching the plan's
//! "every endpoint must complete the authenticated protocol handshake" rule for the Windows backend
//! - the same principle applies here even though Unix has no discovery-registry step to distrust.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A Unix-domain-socket endpoint: a path in the filesystem namespace. Naming conventions for
/// control and session endpoints live in [`super::EndpointRegistry`]; this type only knows how to
/// bind/connect once a path has been decided.
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

    /// Bind a listener at this endpoint. A stale socket file left behind by a crashed process (no
    /// live listener answers it) is replaced; a live one causes a normal `AddrInUse` bind error.
    pub fn bind(&self) -> io::Result<BoundEndpoint> {
        if self.path.exists() && !self.is_live() {
            let _ = std::fs::remove_file(&self.path);
        }
        let listener = UnixListener::bind(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(BoundEndpoint {
            listener: IpcListener(listener),
            endpoint: self.clone(),
        })
    }

    pub fn connect(&self) -> io::Result<IpcConnection> {
        Ok(IpcConnection::Local(UnixStream::connect(&self.path)?))
    }

    /// Whether some listener currently answers at this endpoint. Used to decide whether a socket
    /// file with no live listener behind it may be safely unlinked and replaced.
    pub fn is_live(&self) -> bool {
        UnixStream::connect(&self.path).is_ok()
    }

    /// Drop this endpoint's on-disk trace so a dead server stops being discoverable, without
    /// waiting for the (possibly never-arriving) teardown of the server that created it.
    ///
    /// Unlinking the socket file is *all* an endpoint is on Unix; the abstraction exists so the
    /// Windows backend - where the discoverable artifact and the transport are separate things -
    /// can retire the right one without the caller knowing the difference.
    pub fn remove_stale(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A successfully bound endpoint: the listener plus the endpoint it is bound to, so a caller can
/// still unlink/rename/inspect the underlying path without re-deriving it.
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

pub struct IpcListener(UnixListener);

impl IpcListener {
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Accept one pending connection. Returns `WouldBlock` immediately when non-blocking and no
    /// connection is pending - callers loop this the same way they looped `UnixListener::accept`.
    pub fn accept(&self) -> io::Result<IpcConnection> {
        let (stream, _addr) = self.0.accept()?;
        Ok(IpcConnection::Local(stream))
    }
}

pub enum IpcConnection {
    Local(UnixStream),
    Piped(super::piped::PipedConnection),
}

impl IpcConnection {
    pub fn connect(endpoint: &IpcEndpoint) -> io::Result<Self> {
        endpoint.connect()
    }

    /// Wrap an already-connected stream (e.g. one half of `UnixStream::pair()` in tests, or a
    /// freshly `accept`ed connection handled outside [`IpcListener::accept`]).
    pub fn from_unix(stream: UnixStream) -> Self {
        Self::Local(stream)
    }

    /// Wrap a pipe-backed duplex (remote SSH attach / tests).
    pub fn from_piped(piped: super::piped::PipedConnection) -> Self {
        Self::Piped(piped)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Local(stream) => Ok(Self::Local(stream.try_clone()?)),
            Self::Piped(piped) => Ok(Self::Piped(piped.try_clone()?)),
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Self::Local(stream) => stream.set_nonblocking(nonblocking),
            Self::Piped(piped) => piped.set_nonblocking(nonblocking),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Local(stream) => stream.set_read_timeout(timeout),
            Self::Piped(piped) => piped.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Local(stream) => stream.set_write_timeout(timeout),
            // Documented no-op for pipes — see [`super::piped`].
            Self::Piped(piped) => piped.set_write_timeout(timeout),
        }
    }

    /// Peer (connecting process) pid, if the platform can report one. `SO_PEERCRED` on Linux,
    /// `LOCAL_PEERPID` on macOS. Always `None` for [`Self::Piped`] (remote/proxy). Best-effort:
    /// `None` on any failure, never panics. A liveness/reaping aid, not an authorization check.
    pub fn peer_pid(&self) -> Option<u32> {
        match self {
            Self::Local(stream) => peer_pid(stream),
            Self::Piped(piped) => piped.peer_pid(),
        }
    }

    pub fn shutdown(&self, how: std::net::Shutdown) -> io::Result<()> {
        match self {
            Self::Local(stream) => stream.shutdown(how),
            Self::Piped(piped) => piped.shutdown(),
        }
    }
}

impl Read for IpcConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Local(stream) => stream.read(buf),
            Self::Piped(piped) => piped.read(buf),
        }
    }
}

impl Write for IpcConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Local(stream) => stream.write(buf),
            Self::Piped(piped) => piped.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Local(stream) => stream.flush(),
            Self::Piped(piped) => piped.flush(),
        }
    }
}

#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast::<libc::c_void>(),
            &mut len,
        )
    };
    (ret == 0 && cred.pid > 0).then_some(cred.pid as u32)
}

#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast::<libc::c_void>(),
            &mut len,
        )
    };
    (ret == 0 && pid > 0).then_some(pid as u32)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_pid(_stream: &UnixStream) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_endpoint(name: &str) -> IpcEndpoint {
        IpcEndpoint::at_path(std::env::temp_dir().join(format!(
            "rozi-ipc-unix-test-{name}-{}.sock",
            std::process::id()
        )))
    }

    #[test]
    fn bind_then_connect_round_trips_bytes() {
        let endpoint = temp_endpoint("roundtrip");
        let _ = std::fs::remove_file(endpoint.path());
        let bound = endpoint.bind().expect("bind");
        let listener = bound.into_listener();
        listener.set_nonblocking(true).expect("nonblocking");

        let mut client = endpoint.connect().expect("connect");
        client.write_all(b"ping").expect("write");

        let mut server_conn = loop {
            match listener.accept() {
                Ok(conn) => break conn,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                Err(err) => panic!("accept failed: {err}"),
            }
        };
        let mut buf = [0u8; 4];
        server_conn.set_nonblocking(false).unwrap();
        server_conn.read_exact(&mut buf).expect("read");
        assert_eq!(&buf, b"ping");

        let _ = std::fs::remove_file(endpoint.path());
    }

    #[test]
    fn stale_socket_file_is_replaced_on_bind() {
        let endpoint = temp_endpoint("stale");
        let _ = std::fs::remove_file(endpoint.path());
        {
            let bound = endpoint.bind().expect("first bind");
            drop(bound); // listener dropped, but the socket file itself is left behind on Unix.
        }
        assert!(endpoint.path().exists());
        assert!(!endpoint.is_live());

        // A second bind at the same path must succeed by replacing the dead file rather than
        // failing with AddrInUse.
        let rebound = endpoint.bind().expect("rebind over stale socket");
        drop(rebound);

        let _ = std::fs::remove_file(endpoint.path());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn peer_pid_reports_this_process() {
        let (a, _b) = UnixStream::pair().expect("socket pair");
        let conn = IpcConnection::Local(a);
        assert_eq!(conn.peer_pid(), Some(std::process::id()));
    }
}
