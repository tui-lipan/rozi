use std::io;
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(5);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Start a new named session server and return once it answers, without attaching a client.
///
/// The server starts with no panes; the first `split` against it opens one. A name that is already
/// running is refused rather than joined, and a server answering on the endpoint that is not the
/// one spawned here (another process won the same name) is reported instead of claimed.
pub(crate) fn create_detached_session(name: &str) -> io::Result<()> {
    if !crate::session::discovery::valid_session_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    if crate::session::discovery::discover_session(name)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Session `{name}` is already running"),
        ));
    }
    let exe = std::env::current_exe()?;
    // An updated binary unlinks the one this process runs from; see `session::bootstrap`.
    if !exe.exists() {
        return Err(io::Error::other(
            "rozi was updated on disk; run the new binary to start sessions",
        ));
    }
    let mut child = crate::platform::server_lifecycle::spawn_detached_server(
        &exe, name, true, None,
    )
    .map_err(|err| {
        io::Error::other(format!(
            "Could not start server for `{name}` ({}): {err}",
            exe.display()
        ))
    })?;
    let endpoint = super::session_endpoint(name)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "Session `{name}` server exited before it was ready ({status})"
            )));
        }
        if crate::session::discovery::query_session_endpoint(name, &endpoint).is_some() {
            let server_pid = endpoint.connect().ok().and_then(|stream| stream.peer_pid());
            if server_pid.is_some_and(|pid| pid != child.id()) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Session `{name}` was created by another process"),
                ));
            }
            return Ok(());
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
