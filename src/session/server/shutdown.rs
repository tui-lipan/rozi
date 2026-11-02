use std::fs;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::platform::ipc::{IpcConnection, IpcEndpoint};
use crate::session::protocol::{self, ClientMessage, ServerMessage};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const RETIRE_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const RETIRE_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retirement {
    Retired,
    Recreated,
    TimedOut,
}

/// What stopping the server is for, which decides what stays under the session's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// Delete the session: its snapshot and deployments go too.
    Delete,
    /// Something starts under this name next. Its deployments stay, and a server stopped any way
    /// but the protocol leaves a snapshot for that start to discard.
    Restart,
}

impl Ending {
    fn message(self) -> ClientMessage {
        match self {
            Self::Delete => ClientMessage::Shutdown,
            Self::Restart => ClientMessage::ShutdownForRestart,
        }
    }

    /// Discard the snapshot and deployments if this ending deletes them. Forgetting them is what
    /// makes this a deletion, so a restart leaves both alone.
    fn forget_session(self, name: &str) -> Option<io::Error> {
        match self {
            Self::Delete => super::delete_snapshot(name).err(),
            Self::Restart => None,
        }
    }
}

#[derive(Debug)]
enum EndpointProbe {
    Retired,
    Live,
    LivePeerUnknown,
    Busy,
    Recreated,
}

/// Stop one local named-session server through the authenticated protocol first.
///
/// A failed attach or shutdown write may leave an older or wedged local server behind. The peer pid
/// is captured before the handshake so the platform lifecycle fallback can reap that server without
/// ever targeting an SSH transport. A successful protocol stop lets the server retire its own
/// endpoint; the client never unlinks a live endpoint after that path.
pub(crate) fn shutdown_named_session(name: &str) -> io::Result<()> {
    shutdown_named_session_if_present(name).map(|_| ())
}

/// Stop a named session and report whether a live endpoint, stale endpoint, or snapshot existed.
///
/// The CLI uses the result to distinguish a successful cleanup from a missing name. Other callers
/// retain idempotent shutdown through [`shutdown_named_session`].
pub(crate) fn shutdown_named_session_if_present(name: &str) -> io::Result<bool> {
    let endpoint = super::session_endpoint(name)?;
    let endpoint_exists = path_entry_exists(endpoint.path())?;
    let snapshot_exists = super::list_snapshot_names_by_recency()
        .iter()
        .any(|snapshot| snapshot == name);
    if !endpoint_exists && !snapshot_exists {
        return Ok(false);
    }

    shutdown_existing_named_session(&endpoint, name, Ending::Delete)?;
    Ok(true)
}

/// Stop a named session's server so it can start again under the same name, keeping its
/// deployments. Reports whether a server was there to stop.
///
/// A server this build cannot talk to, such as one left running across an update, is stopped by
/// its process instead, the same fallback a kill uses.
pub(crate) fn stop_named_session_for_restart(name: &str) -> io::Result<bool> {
    let endpoint = super::session_endpoint(name)?;
    if !path_entry_exists(endpoint.path())? {
        return Ok(false);
    }
    shutdown_existing_named_session(&endpoint, name, Ending::Restart)?;
    Ok(true)
}

fn shutdown_existing_named_session(
    endpoint: &IpcEndpoint,
    name: &str,
    ending: Ending,
) -> io::Result<()> {
    let mut stream = match endpoint.connect() {
        Ok(stream) => stream,
        Err(connect_error) => {
            return handle_connect_failure(endpoint, name, connect_error, ending);
        }
    };

    let server_pid = stream.peer_pid();
    let graceful = (|| {
        stream.set_read_timeout(Some(SHUTDOWN_TIMEOUT))?;
        stream.set_write_timeout(Some(SHUTDOWN_TIMEOUT))?;
        graceful_shutdown(&mut stream, name, ending)
    })();

    let graceful_error = match graceful {
        Ok(()) => {
            // Delete while the old server is still live. Its shutdown path also forgets the
            // snapshot, and no same-name replacement can start until this server retires its
            // endpoint.
            let snapshot_error = ending.forget_session(name);
            // Keep the authenticated connection open until the server retires so Shutdown remains
            // associated with a live attached client throughout delivery. Dropping immediately
            // makes the final control frame race the peer-EOF teardown under load.
            let retirement = wait_for_retirement(endpoint, server_pid)?;
            drop(stream);
            return match retirement {
                Retirement::Retired => snapshot_error.map_or(Ok(()), Err),
                Retirement::Recreated => Err(recreated_server_error(name)),
                Retirement::TimedOut => Err(retirement_timeout_error(name)),
            };
        }
        Err(error) => error,
    };
    drop(stream);

    let Some(server_pid) = server_pid else {
        // Without a local peer pid there is no safe forced-termination path. Do not unlink a live
        // endpoint or pretend an incompatible live server was stopped.
        return match probe_endpoint(endpoint, None)? {
            EndpointProbe::Retired => cleanup_stale_session(endpoint, name, ending),
            EndpointProbe::Live | EndpointProbe::LivePeerUnknown | EndpointProbe::Busy => {
                Err(io::Error::other(format!(
                    "could not shut down session {name:?}: {graceful_error}; peer pid unavailable"
                )))
            }
            EndpointProbe::Recreated => Err(recreated_server_error(name)),
        };
    };

    // The protocol failed, so the captured local pid is the only permitted fallback. Re-probe the
    // endpoint immediately before termination and require the same peer pid; a different peer (or
    // an endpoint that cannot report its peer) must be left completely alone.
    match probe_endpoint(endpoint, Some(server_pid))? {
        EndpointProbe::Live => crate::platform::server_lifecycle::terminate_server(server_pid),
        EndpointProbe::Retired => return cleanup_stale_session(endpoint, name, ending),
        EndpointProbe::Recreated => return Err(recreated_server_error(name)),
        EndpointProbe::LivePeerUnknown | EndpointProbe::Busy => {
            return Err(io::Error::other(format!(
                "could not shut down session {name:?}: could not verify the captured peer pid"
            )));
        }
    }
    match wait_for_retirement(endpoint, Some(server_pid))? {
        Retirement::Retired => cleanup_stale_session(endpoint, name, ending),
        Retirement::Recreated => Err(recreated_server_error(name)),
        Retirement::TimedOut => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "could not shut down session {name:?}: forced termination did not retire the old endpoint"
            ),
        )),
    }
}

fn path_entry_exists(path: &std::path::Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn handle_connect_failure(
    endpoint: &IpcEndpoint,
    name: &str,
    connect_error: io::Error,
    ending: Ending,
) -> io::Result<()> {
    match probe_endpoint(endpoint, None)? {
        EndpointProbe::Retired => cleanup_stale_session(endpoint, name, ending),
        EndpointProbe::Live | EndpointProbe::LivePeerUnknown | EndpointProbe::Busy => {
            Err(io::Error::new(
                connect_error.kind(),
                format!(
                    "could not connect to session {name:?}: endpoint is live or busy: {connect_error}"
                ),
            ))
        }
        EndpointProbe::Recreated => Err(recreated_server_error(name)),
    }
}

fn cleanup_stale_session(endpoint: &IpcEndpoint, name: &str, ending: Ending) -> io::Result<()> {
    // Bind the endpoint before deleting any snapshot. This claims the stale name while the old
    // server is gone; a replacement that appeared in the meantime makes the bind fail instead of
    // letting a later unlink remove its endpoint. Keep the claim alive while removing the registry
    // entry: Unix listeners remain valid after unlink, and Windows keeps the named pipe alive until
    // the claim is dropped.
    let _claim = endpoint.bind().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not claim stale endpoint for session {name:?}: {error}"),
        )
    })?;
    let snapshot_error = ending.forget_session(name);
    endpoint.remove_stale();
    snapshot_error.map_or(Ok(()), Err)
}

fn wait_for_retirement(endpoint: &IpcEndpoint, old_pid: Option<u32>) -> io::Result<Retirement> {
    let deadline = Instant::now() + RETIRE_WAIT_TIMEOUT;
    loop {
        match probe_endpoint(endpoint, old_pid)? {
            EndpointProbe::Retired => return Ok(Retirement::Retired),
            EndpointProbe::Recreated => return Ok(Retirement::Recreated),
            EndpointProbe::Live | EndpointProbe::LivePeerUnknown | EndpointProbe::Busy => {
                let now = Instant::now();
                if now >= deadline {
                    return Ok(Retirement::TimedOut);
                }
                let remaining = deadline.saturating_duration_since(now);
                std::thread::sleep(RETIRE_POLL_INTERVAL.min(remaining));
            }
        }
    }
}

fn probe_endpoint(endpoint: &IpcEndpoint, old_pid: Option<u32>) -> io::Result<EndpointProbe> {
    match endpoint.connect() {
        Ok(stream) => {
            let peer_pid = stream.peer_pid();
            match old_pid {
                Some(old_pid) => match peer_pid {
                    Some(peer_pid) if peer_pid != old_pid => Ok(EndpointProbe::Recreated),
                    Some(_) => Ok(EndpointProbe::Live),
                    None => Ok(EndpointProbe::LivePeerUnknown),
                },
                None => Ok(EndpointProbe::Live),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(EndpointProbe::Busy),
        Err(_error) if endpoint.is_live() => Ok(EndpointProbe::Busy),
        Err(error) if is_stale_connection_error(error.kind()) => Ok(EndpointProbe::Retired),
        Err(error) => Err(error),
    }
}

fn is_stale_connection_error(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::AddrNotAvailable
    )
}

fn recreated_server_error(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("session {name:?} was recreated while the old server was shutting down"),
    )
}

fn retirement_timeout_error(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out waiting for session {name:?} server endpoint to retire"),
    )
}

fn graceful_shutdown(stream: &mut IpcConnection, name: &str, ending: Ending) -> io::Result<()> {
    protocol::write_frame(
        stream,
        &protocol::attach_message(
            name,
            crate::platform::user::current_user_label(),
            false,
            true,
        ),
    )?;
    match protocol::read_frame::<_, ServerMessage>(stream)? {
        ServerMessage::Attached { .. } => {
            protocol::write_frame(stream, &ending.message())?;
            stream.flush()
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("server refused shutdown handshake: {other:?}"),
        )),
    }
}
