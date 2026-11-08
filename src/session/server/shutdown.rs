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
    /// but the protocol leaves a snapshot for that start to discard, or to restore when the server
    /// was [incompatible](StoppedForRestart::Incompatible).
    Restart,
    /// Stop the server by signal so it saves a final snapshot for the replacement server to
    /// restore, keeping its snapshot and deployments.
    Upgrade,
}

impl Ending {
    fn message(self) -> ClientMessage {
        match self {
            Self::Delete => ClientMessage::Shutdown,
            Self::Restart | Self::Upgrade => ClientMessage::ShutdownForRestart,
        }
    }

    /// Discard the snapshot and deployments if this ending deletes them. Forgetting them is what
    /// makes this a deletion, so a restart leaves both alone.
    fn forget_session(self, name: &str) -> Option<io::Error> {
        match self {
            Self::Delete => super::delete_snapshot(name).err(),
            Self::Restart | Self::Upgrade => None,
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

/// How a server stopped for a restart went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoppedForRestart {
    /// It stopped through the protocol, or was already gone.
    Stopped,
    /// It refused this build's protocol version, so it was stopped by its process instead.
    ///
    /// That stop is a stop signal on Unix, which a server of any version answers by saving a final
    /// snapshot it keeps; it is killed only once its own snapshot grace has passed, leaving the
    /// newest periodic snapshot. On Windows it is a forced termination, and the newest periodic
    /// snapshot is what remains. Either way the snapshot belongs to the session's own server, so a restart
    /// can restore it rather than start the session empty.
    Incompatible,
}

/// Stop a named session's server so it can start again under the same name, keeping its
/// deployments. Reports whether a server was there to stop, and how it stopped.
///
/// A server this build cannot talk to, such as one left running across an update, is stopped by
/// its process instead, the same fallback a kill uses.
pub(crate) fn stop_named_session_for_restart(name: &str) -> io::Result<Option<StoppedForRestart>> {
    let endpoint = super::session_endpoint(name)?;
    if !path_entry_exists(endpoint.path())? {
        return Ok(None);
    }
    shutdown_existing_named_session(&endpoint, name, Ending::Restart).map(Some)
}

/// Stop a named session's server by signal so it saves its snapshot before exiting, keeping both
/// its snapshot and its deployments for the replacement server to restore.
pub(crate) fn stop_named_session_for_upgrade(name: &str) -> io::Result<bool> {
    let endpoint = super::session_endpoint(name)?;
    if !path_entry_exists(endpoint.path())? {
        return Ok(false);
    }
    shutdown_existing_named_session(&endpoint, name, Ending::Upgrade).map(|_| true)
}

/// Whether a refused shutdown handshake was a protocol-version refusal.
#[derive(Debug)]
struct GracefulFailure {
    error: io::Error,
    incompatible: bool,
}

impl From<io::Error> for GracefulFailure {
    fn from(error: io::Error) -> Self {
        Self {
            error,
            incompatible: false,
        }
    }
}

fn shutdown_existing_named_session(
    endpoint: &IpcEndpoint,
    name: &str,
    ending: Ending,
) -> io::Result<StoppedForRestart> {
    let mut stream = match endpoint.connect() {
        Ok(stream) => stream,
        Err(connect_error) => {
            return handle_connect_failure(endpoint, name, connect_error, ending)
                .map(|()| StoppedForRestart::Stopped);
        }
    };

    let server_pid = stream.peer_pid();
    let graceful = (|| -> Result<(), GracefulFailure> {
        if ending == Ending::Upgrade {
            return Err(GracefulFailure {
                error: io::Error::other("upgrade stops by signal to preserve snapshot"),
                incompatible: true,
            });
        }
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
                Retirement::Retired => snapshot_error.map_or(Ok(StoppedForRestart::Stopped), Err),
                Retirement::Recreated => Err(recreated_server_error(name)),
                Retirement::TimedOut => Err(retirement_timeout_error(name)),
            };
        }
        Err(failure) => failure,
    };
    drop(stream);
    let stopped = if graceful_error.incompatible {
        StoppedForRestart::Incompatible
    } else {
        StoppedForRestart::Stopped
    };
    let graceful_error = graceful_error.error;

    let Some(server_pid) = server_pid else {
        // Without a local peer pid there is no safe forced-termination path. Do not unlink a live
        // endpoint or pretend an incompatible live server was stopped.
        return match probe_endpoint(endpoint, None)? {
            EndpointProbe::Retired => {
                cleanup_stale_session(endpoint, name, ending).map(|()| StoppedForRestart::Stopped)
            }
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
        // An incompatible server is still healthy and answers the stop signal by saving a final
        // snapshot, which the restart then restores. Its shutdown first finishes a save already in
        // flight and then the final one, each within the snapshot grace, so allow both and a
        // margin to exit before killing it.
        EndpointProbe::Live if stopped == StoppedForRestart::Incompatible => {
            crate::platform::server_lifecycle::terminate_server_after(
                server_pid,
                super::resurrect::SNAPSHOT_SHUTDOWN_GRACE * 2 + Duration::from_secs(1),
            );
        }
        EndpointProbe::Live => crate::platform::server_lifecycle::terminate_server(server_pid),
        EndpointProbe::Retired => {
            return cleanup_stale_session(endpoint, name, ending)
                .map(|()| StoppedForRestart::Stopped);
        }
        EndpointProbe::Recreated => return Err(recreated_server_error(name)),
        EndpointProbe::LivePeerUnknown | EndpointProbe::Busy => {
            return Err(io::Error::other(format!(
                "could not shut down session {name:?}: could not verify the captured peer pid"
            )));
        }
    }
    match wait_for_retirement(endpoint, Some(server_pid))? {
        Retirement::Retired => cleanup_stale_session(endpoint, name, ending).map(|()| stopped),
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

fn graceful_shutdown(
    stream: &mut IpcConnection,
    name: &str,
    ending: Ending,
) -> Result<(), GracefulFailure> {
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
            Ok(stream.flush()?)
        }
        other => Err(GracefulFailure {
            incompatible: matches!(
                &other,
                ServerMessage::Error { code, .. } if code == protocol::PROTOCOL_MISMATCH_ERROR_CODE
            ),
            error: io::Error::new(
                io::ErrorKind::InvalidData,
                format!("server refused shutdown handshake: {other:?}"),
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ipc::PipedConnection;

    /// Run the shutdown handshake against a scripted server that answers the attach with `reply`.
    fn handshake_against(reply: ServerMessage) -> Result<(), GracefulFailure> {
        let (server_reader, client_writer) = std::io::pipe().expect("client->server pipe");
        let (client_reader, server_writer) = std::io::pipe().expect("server->client pipe");
        let server = std::thread::spawn(move || {
            let mut conn = PipedConnection::from_reader_writer(server_writer, server_reader);
            let attach: ClientMessage = protocol::read_frame(&mut conn).expect("read attach");
            assert!(matches!(attach, ClientMessage::Attach { .. }), "{attach:?}");
            protocol::write_frame(&mut conn, &reply).expect("reply");
        });
        let mut client = IpcConnection::from_piped(PipedConnection::from_reader_writer(
            client_writer,
            client_reader,
        ));
        let result = graceful_shutdown(&mut client, "skew", Ending::Restart);
        server.join().expect("scripted server");
        result
    }

    /// A restart restores the session only when its server refused this build's protocol, the one
    /// case where the fallback stop leaves a snapshot that is the session's work rather than debris.
    #[test]
    fn only_a_protocol_refusal_marks_a_server_incompatible() {
        let refusal = handshake_against(ServerMessage::Error {
            code: protocol::PROTOCOL_MISMATCH_ERROR_CODE.to_string(),
            message: "server supports 41-41".to_string(),
        })
        .expect_err("a refused handshake cannot shut the server down");
        assert!(refusal.incompatible, "{:?}", refusal.error);

        let other = handshake_against(ServerMessage::Error {
            code: "auth".to_string(),
            message: "refused".to_string(),
        })
        .expect_err("a refused handshake cannot shut the server down");
        assert!(!other.incompatible, "{:?}", other.error);
    }
}
