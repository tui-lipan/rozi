use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::platform::ipc::IpcEndpoint;
use crate::platform::server_lifecycle::DetachedServer;
use crate::session::protocol::{self, ClientMessage, ServerMessage};

const READY_TIMEOUT: Duration = Duration::from_secs(5);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

/// Start a new named session server and return once it answers, without leaving a client attached.
///
/// The server starts with no panes; the first `split` against it opens one. A name that is already
/// running is refused rather than joined. The spawned server carries a startup nonce, and success
/// means a server proved it holds that nonce in a completed attach handshake, so a server another
/// process started under the same name is never claimed, on any platform. Every failure after the
/// spawn stops the spawned server, so a reported failure never leaves a session behind.
pub(crate) fn create_detached_session(name: &str, resurrect: bool) -> io::Result<()> {
    if !crate::session::discovery::valid_session_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    if crate::session::discovery::discover_session(name)?.is_some() {
        return Err(already_running(name));
    }
    let exe = std::env::current_exe()?;
    // An updated binary unlinks the one this process runs from; see `session::bootstrap`.
    if !exe.exists() {
        return Err(io::Error::other(
            "rozi was updated on disk; run the new binary to start sessions",
        ));
    }
    let nonce = crate::session::remote::fresh_server_nonce();
    let child = crate::platform::server_lifecycle::spawn_detached_server(
        &exe,
        name,
        crate::platform::server_lifecycle::ServerStart {
            fresh: true,
            startup_nonce: Some(&nonce),
            no_resurrect: !resurrect,
        },
    )
    .map_err(|err| {
        io::Error::other(format!(
            "Could not start server for `{name}` ({}): {err}",
            exe.display()
        ))
    })?;
    let mut spawned = SpawnedServer(Some(child));
    let endpoint = super::session_endpoint(name)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match prove_ownership(&endpoint, name, &nonce) {
            Ok(Ownership::Ours) => {
                spawned.keep();
                return Ok(());
            }
            Ok(Ownership::Another) => return Err(already_running(name)),
            Ok(Ownership::Refused(message)) => {
                return Err(io::Error::other(format!("Session `{name}`: {message}")));
            }
            // Not listening yet, or still starting up and not answering in time.
            Err(_) => {}
        }
        if let Some(status) = spawned.try_wait()? {
            return Err(io::Error::other(format!(
                "Session `{name}` server exited before it was ready ({status})"
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "Session `{name}` did not start within {}s",
                    READY_TIMEOUT.as_secs()
                ),
            ));
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
}

fn already_running(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("Session `{name}` is already running"),
    )
}

enum Ownership {
    /// The server on the endpoint holds this nonce and has finished starting.
    Ours,
    /// A different server owns the name.
    Another,
    /// The server answered but refused the handshake for another reason.
    Refused(String),
}

/// Attach read-only while presenting `nonce`, then detach at once. An I/O error means the server is
/// not reachable or not answering yet, and is worth retrying.
fn prove_ownership(endpoint: &IpcEndpoint, name: &str, nonce: &str) -> io::Result<Ownership> {
    let mut stream = endpoint.connect()?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    protocol::write_frame(
        &mut stream,
        &protocol::attach_message_with_server_nonce(
            name,
            crate::platform::user::current_user_label(),
            true,
            true,
            Some(nonce.to_string()),
        ),
    )?;
    match protocol::read_frame::<_, ServerMessage>(&mut stream)? {
        ServerMessage::Attached { .. } => {
            protocol::write_frame(&mut stream, &ClientMessage::Detach)?;
            let _ = stream.flush();
            Ok(Ownership::Ours)
        }
        ServerMessage::Error { code, .. } if code == "server-identity-mismatch" => {
            Ok(Ownership::Another)
        }
        ServerMessage::Error { message, .. } => Ok(Ownership::Refused(message)),
        other => Ok(Ownership::Refused(format!(
            "unexpected handshake reply: {other:?}"
        ))),
    }
}

/// The spawned server until it has proved it is ready: dropped without [`Self::keep`], it is
/// killed and reaped, so no early return can leave a half-started session running.
struct SpawnedServer(Option<DetachedServer>);

impl SpawnedServer {
    fn keep(&mut self) {
        self.0 = None;
    }

    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        match self.0.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
