//! `--remote-serve <NAME>`: proxy between stdio and the local session endpoint.

use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::ipc::IpcConnection;
use crate::platform::server_lifecycle;
use crate::session::discovery;
use crate::session::server;

use super::preamble::{RemotePreamble, write_preamble};

/// Connect to (or autostart) the named local session, emit a preamble, then pump bytes.
///
/// When the session socket closes, this process exits so the local ssh client's stdout pipe sees
/// EOF (a blocked `stdin.read` on this side cannot otherwise notice). When stdin closes first,
/// the socket is dropped and the stdout pump joins normally.
pub fn run_remote_serve(name: &str) -> io::Result<()> {
    if !discovery::valid_attach_target(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid session name `{name}`"),
        ));
    }

    let (mut socket, server_started) = connect_or_autostart(name)?;
    // Prefer blocking reads so we do not busy-poll on WouldBlock.
    let _ = socket.set_nonblocking(false);
    {
        let mut stdout = io::stdout().lock();
        write_preamble(&mut stdout, &RemotePreamble::current(server_started))?;
        stdout.flush()?;
    }

    let mut socket_reader = socket.try_clone()?;
    let to_stdout = thread::spawn(move || -> io::Result<()> {
        let mut stdout = io::stdout();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match socket_reader.read(&mut buf) {
                Ok(0) => {
                    let _ = stdout.flush();
                    // Session closed while stdin may still be blocked — exit so ssh tears down.
                    std::process::exit(0);
                }
                Ok(n) => {
                    stdout.write_all(&buf[..n])?;
                    stdout.flush()?;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    eprintln!("hyprmux --remote-serve: socket read failed: {err}");
                    std::process::exit(1);
                }
            }
        }
    });

    let mut stdin = io::stdin().lock();
    let mut buf = [0u8; 64 * 1024];
    let stdin_result = (|| -> io::Result<()> {
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    socket.write_all(&buf[..n])?;
                    socket.flush()?;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    })();

    // Stdin closed first: the client (or ssh) is gone. Do *not* join the stdout pump — it holds a
    // `try_clone` of this same socket, so dropping our handle does not half-close the fd and the
    // pump would block in `read` forever. Returning here exits the process, which closes every
    // remaining fd and lets the session server see the disconnect.
    drop(socket);
    drop(to_stdout);
    stdin_result
}

pub(crate) fn connect_or_autostart(name: &str) -> io::Result<(IpcConnection, bool)> {
    let endpoint = server::session_endpoint(name)?;
    if let Ok(stream) = endpoint.connect() {
        return Ok((stream, false));
    }

    let exe = std::env::current_exe()?;
    let mut child = server_lifecycle::spawn_detached_server(&exe, name, false)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_err = None;
    while Instant::now() < deadline {
        match endpoint.connect() {
            Ok(stream) => {
                // Detached session server outlives this proxy; do not kill it on drop.
                let _ = child.try_wait();
                drop(child);
                return Ok((stream, true));
            }
            Err(err) => {
                last_err = Some(err);
                thread::sleep(Duration::from_millis(40));
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for session `{name}` to start"),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::server::ServerSettings;
    use std::thread;
    use std::time::Duration;

    /// Pins why [`run_remote_serve`] must not join its stdout pump after stdin EOF: a `try_clone`d
    /// sibling shares the same underlying socket, so dropping one handle does *not* deliver EOF to
    /// the other. Joining would block forever and leak the ssh child on every detach.
    #[cfg(unix)]
    #[test]
    fn dropping_one_handle_does_not_eof_a_try_cloned_sibling() {
        use crate::platform::ipc::IpcConnection;
        use std::io::Read;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc;

        let (ours, _peer) = UnixStream::pair().expect("socket pair");
        let socket = IpcConnection::from_unix(ours);
        let mut sibling = socket.try_clone().expect("try_clone");

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 64];
            let _ = tx.send(sibling.read(&mut buf));
        });

        // Mirrors the stdin-EOF path: drop our handle while the pump still holds its clone.
        drop(socket);
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "dropping one handle must NOT wake a try_cloned sibling's read — \
             run_remote_serve therefore cannot join its stdout pump"
        );
    }

    #[test]
    fn live_endpoint_reports_server_started_false() {
        let name = format!("rserve-live-{}", std::process::id());
        let (listener, endpoint) = server::bind_session_socket(&name).expect("bind session socket");
        listener.set_nonblocking(true).unwrap();
        let session = name.clone();
        let thread = thread::spawn(move || {
            let mut server =
                server::SessionServer::new_named_with_settings(session, ServerSettings::default());
            let _ = server.run_listener(listener);
        });

        // Wait until the endpoint answers.
        let deadline = Instant::now() + Duration::from_secs(2);
        while endpoint.connect().is_err() {
            assert!(Instant::now() < deadline, "listener never became live");
            thread::sleep(Duration::from_millis(10));
        }

        let (_stream, started) = connect_or_autostart(&name).expect("connect existing");
        assert!(!started);

        // Tear down: remove the socket so run_listener eventually stops accepting usefully,
        // then detach by dropping — the server thread may linger briefly.
        endpoint.remove_stale();
        drop(thread);
    }

    #[test]
    fn preamble_server_started_flag_round_trips_for_create_only() {
        let mut buf = Vec::new();
        write_preamble(&mut buf, &RemotePreamble::current(true)).unwrap();
        let decoded = crate::session::remote::preamble::read_preamble(&mut &buf[..]).unwrap();
        assert!(decoded.server_started);
        decoded.validate_for_client().unwrap();
    }
}
