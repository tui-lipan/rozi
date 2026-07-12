//! Transport-neutral local IPC abstraction (cross-platform plan Phase 5).
//!
//! [`IpcEndpoint`], [`IpcListener`], [`IpcConnection`], and [`BoundEndpoint`] are re-exported from
//! the per-platform backend ([`unix`] on Linux/macOS, [`windows`] on Windows - see each module's
//! doc comment for what is actually implemented versus stubbed). [`EndpointRegistry`] is
//! transport-neutral: it only knows the naming convention for control/session endpoints and how to
//! enumerate them, delegating the actual bind/connect to whichever backend's [`IpcEndpoint`] it
//! constructs.
//!
//! Migrated onto this abstraction: `control.rs`, `session/client.rs`, `session/discovery.rs`,
//! `session/server/*`, `cli.rs`, `ops/session.rs`. `main.rs` holds the bound control listener
//! across startup the same way it held a raw `UnixListener` before.
//!
//! Not yet built on this abstraction: the session server's per-connection loop still polls
//! non-blocking connections directly inside [`crate::session::server::SessionServer::run_listener`]
//! rather than using dedicated reader/writer actor threads. The plan calls for that refactor
//! primarily so the *Windows* backend (which cannot poll a named pipe as a non-blocking socket the
//! way Unix polls a non-blocking `UnixStream`) has a sane connection model; Unix non-blocking
//! sockets already support today's polling loop equivalently on Linux and macOS, so Milestone 1
//! keeps it as-is and this refactor is deferred to land together with the Windows backend in
//! Milestone 2.

use std::io;
use std::path::Path;

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

/// Naming convention and enumeration for the two endpoint families this app uses: one per-process
/// *control* endpoint (`--socket`/`HYPRMUX_SOCKET`-discoverable CLI control plane) and one
/// per-name *session* endpoint (named/ephemeral session servers). Endpoint identity is always
/// derived from a runtime directory plus a logical id, never constructed ad hoc at call sites, so
/// the naming scheme only needs to change in one place if a future backend (Windows pipe names are
/// flat and dot-separated, unlike Unix socket paths) needs a different literal form.
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
    /// `cli.rs` to find the running UI's control socket when `--socket`/`HYPRMUX_SOCKET` are unset).
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
            Path::new("/run/hyprmux/control-1234.sock")
        );
        assert_eq!(
            EndpointRegistry::session_endpoint(dir, "dev").path(),
            Path::new("/run/hyprmux/session-dev.sock")
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
