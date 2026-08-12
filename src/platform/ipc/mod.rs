//! Transport-neutral local IPC abstraction (cross-platform plan Phase 5).
//!
//! [`IpcEndpoint`], [`IpcListener`], [`IpcConnection`], and [`BoundEndpoint`] are re-exported from
//! the per-platform backend: Unix-domain sockets on Linux/macOS ([`unix`]), named pipes on Windows
//! ([`windows`]). [`EndpointRegistry`] is transport-neutral: it only knows the naming convention for
//! control/session endpoints and how to enumerate them, delegating bind/connect to whichever
//! backend's [`IpcEndpoint`] it constructs.
//!
//! Both backends identify an endpoint by a *path in the runtime directory*. On Unix that path is the
//! socket. On Windows it is a discovery-registry entry standing for a named pipe whose name is
//! derived from it (see [`windows`]'s module doc comment for why, and for why nothing trusts the
//! entry's contents). Keeping the identity the same shape on both is what lets `control.rs`,
//! `cli.rs`, `session/discovery.rs`, `ROZI_SOCKET`, and `--socket` remain one implementation:
//! enumeration is a `read_dir`, retirement is an unlink, and liveness is a connect attempt,
//! everywhere.
//!
//! [`IpcConnection`] is an enum: the platform-local stream plus a [`piped`] variant that wraps a
//! child process's stdin/stdout (used by remote SSH attach). Remote connections report no
//! [`IpcConnection::peer_pid`], so forced `terminate_server` fallbacks never fire against a local
//! ssh pid.
//!
//! Migrated onto this abstraction: `control.rs`, `session/client.rs`, `session/discovery.rs`,
//! `session/server/*`, `cli.rs`, `ops/session.rs`. `main.rs` holds the bound control listener
//! across startup the same way it held a raw `UnixListener` before.
//!
//! The session server drives connections with non-blocking reads/writes and `WouldBlock`-based
//! backpressure rather than reader/writer actor threads. The plan floated the actor refactor to give
//! the Windows backend a workable connection model; it turned out not to be needed - a `PIPE_NOWAIT`
//! named pipe supports exactly the same poll-and-`WouldBlock` loop a non-blocking `UnixStream` does,
//! so both backends serve the existing loop and the refactor would have bought nothing but churn.

use std::io;
use std::path::Path;
use std::process::Child;

pub(crate) mod piped;
pub use piped::{PipedBufferStats, PipedBufferStatsHandle, PipedConnection};

impl IpcConnection {
    /// Return a non-owning observer only for the SSH pipe-backed transport.
    pub fn piped_buffer_stats_handle(&self) -> Option<PipedBufferStatsHandle> {
        match self {
            Self::Piped(piped) => Some(piped.buffer_stats_handle()),
            Self::Local(_) => None,
        }
    }
}

// `BoundEndpoint` is part of the plan's public abstraction surface and every call site that binds
// an endpoint does receive one, but each immediately calls `.into_listener()` inline (type
// inferred) rather than naming `BoundEndpoint` explicitly, so the re-export itself looks unused to
// the lint despite being live API surface.
#[cfg(unix)]
mod unix;
#[cfg(unix)]
#[allow(unused_imports)]
pub use unix::{BoundEndpoint, IpcConnection, IpcEndpoint, IpcListener};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
#[allow(unused_imports)]
pub use windows::{BoundEndpoint, IpcConnection, IpcEndpoint, IpcListener};

/// Wrap a spawned child's stdin/stdout as an [`IpcConnection`].
///
/// Used by remote SSH attach: the child is `ssh … hyprmux --remote-serve`, and the resulting
/// connection speaks the normal session protocol over the pipe. [`IpcConnection::peer_pid`] is
/// `None`, so shutdown fallbacks must not call `terminate_server` on it.
pub fn connection_from_child(mut child: Child) -> io::Result<IpcConnection> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "child stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "child stdout was not piped"))?;
    Ok(IpcConnection::from_piped(
        PipedConnection::from_child_stdio(stdin, stdout, child),
    ))
}

/// Naming convention and enumeration for the two endpoint families this app uses: one per-process
/// *control* endpoint (`--socket`/`ROZI_SOCKET`-discoverable CLI control plane) and one
/// per-name *session* endpoint (named/ephemeral session servers). Endpoint identity is always
/// derived from a runtime directory plus a logical id, never constructed ad hoc at call sites.
///
/// The `.sock` suffix is literal on Unix and vestigial on Windows, where the file is a registry
/// entry rather than a socket - but it is the *same* name on both, which is the point: one
/// enumeration, one naming scheme, no platform branch at any call site.
pub struct EndpointRegistry;

impl EndpointRegistry {
    /// This process's own control endpoint (one per running hyprmux client, named by pid).
    pub fn control_endpoint(runtime_dir: &Path, pid: u32) -> IpcEndpoint {
        IpcEndpoint::at_path(runtime_dir.join(format!("control-{pid}.sock")))
    }

    /// The named/ephemeral session endpoint for `name`.
    pub fn session_endpoint(runtime_dir: &Path, name: &str) -> IpcEndpoint {
        IpcEndpoint::at_path(runtime_dir.join(format!("session-{name}.sock")))
    }

    /// Every session endpoint discoverable in `runtime_dir`, alongside the session name decoded
    /// from its file name. Does not probe liveness itself - a stale entry with no listener behind
    /// it is still returned; callers (`session::discovery`) query the protocol handshake to tell.
    pub fn list_session_endpoints(runtime_dir: &Path) -> io::Result<Vec<(String, IpcEndpoint)>> {
        let entries = match std::fs::read_dir(runtime_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut endpoints = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(name) = file_name
                .strip_prefix("session-")
                .and_then(|name| name.strip_suffix(".sock"))
            else {
                continue;
            };
            endpoints.push((name.to_string(), IpcEndpoint::at_path(path)));
        }
        Ok(endpoints)
    }

    /// Every control endpoint discoverable in `runtime_dir` that currently answers (used by
    /// `cli.rs` to find the running UI's control socket when `--socket`/`ROZI_SOCKET` are unset).
    pub fn list_live_control_endpoints(runtime_dir: &Path) -> io::Result<Vec<IpcEndpoint>> {
        let entries = match std::fs::read_dir(runtime_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut endpoints = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let is_control = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("control-") && name.ends_with(".sock"));
            if !is_control {
                continue;
            }
            let endpoint = IpcEndpoint::at_path(path);
            if endpoint.is_live() {
                endpoints.push(endpoint);
            }
        }
        Ok(endpoints)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn control_and_session_endpoints_use_the_expected_naming_convention() {
        let dir = Path::new("/run/hyprmux");
        assert_eq!(
            EndpointRegistry::control_endpoint(dir, 1234).path(),
            Path::new("/run/rozi/control-1234.sock")
        );
        assert_eq!(
            EndpointRegistry::session_endpoint(dir, "dev").path(),
            Path::new("/run/rozi/session-dev.sock")
        );
    }

    #[test]
    fn list_session_endpoints_decodes_names_and_ignores_unrelated_files() {
        let dir = std::env::temp_dir().join(format!(
            "hyprmux-endpoint-registry-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session-dev.sock"), b"").unwrap();
        std::fs::write(dir.join("session-eph-42.sock"), b"").unwrap();
        std::fs::write(dir.join("control-1.sock"), b"").unwrap();
        std::fs::write(dir.join("not-a-socket.txt"), b"").unwrap();

        let mut names: Vec<String> = EndpointRegistry::list_session_endpoints(&dir)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["dev".to_string(), "eph-42".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_session_endpoints_on_missing_dir_returns_empty_not_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "hyprmux-endpoint-registry-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            EndpointRegistry::list_session_endpoints(&dir).unwrap(),
            Vec::new()
        );
    }
}
