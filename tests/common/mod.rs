#![allow(dead_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rozi::platform::ipc::{IpcConnection, IpcEndpoint};
use rozi::session::protocol::{
    ClientMessage, Frame, FrameDecoder, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
    write_control_frame, write_pane_input_frame,
};
use rozi::session::server::{ServerSettings, SessionServer, bind_session_socket, session_endpoint};

/// Deadline for every wait in this harness.
///
/// This is a safety net against a hang, not an assertion about speed: a wait returns the moment its
/// condition holds, so a generous value costs nothing on a passing run and only changes how long a
/// genuine failure takes to report. Five seconds was tight enough that these tests failed
/// intermittently on CI - a shared runner with a few vCPUs, executing test binaries in parallel,
/// while each of these spawns a real session server or subprocess - and the test that lost the race
/// moved from run to run.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(30);

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ServerGuard {
    child: Option<Child>,
    runtime_base: PathBuf,
}

impl ServerGuard {
    pub(crate) fn new(child: Child, runtime_base: PathBuf) -> Self {
        Self {
            child: Some(child),
            runtime_base,
        }
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("server process is running")
    }

    pub(crate) fn kill_for_restart(&mut self) {
        let child = self.child_mut();
        if child.try_wait().expect("poll server before kill").is_none() {
            child.kill().expect("kill server for restart");
        }
        child.wait().expect("reap killed server");
    }

    pub(crate) fn replace_child(&mut self, child: Child) {
        assert!(
            self.child_mut()
                .try_wait()
                .expect("poll old server")
                .is_some(),
            "old server is still running"
        );
        self.child = Some(child);
    }

    pub(crate) fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            if self
                .child_mut()
                .try_wait()
                .expect("poll server shutdown")
                .is_some()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "session server did not shut down"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.runtime_base);
    }
}

pub(crate) struct TestConnection {
    stream: IpcConnection,
    decoder: FrameDecoder,
}

impl TestConnection {
    fn new(stream: IpcConnection) -> Self {
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .expect("set client read timeout");
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .expect("set client write timeout");
        Self {
            stream,
            decoder: FrameDecoder::default(),
        }
    }

    pub(crate) fn connect(endpoint: &IpcEndpoint) -> Self {
        Self::new(IpcConnection::connect(endpoint).expect("connect to session server"))
    }

    pub(crate) fn write_control(&mut self, message: &ClientMessage) {
        write_control_frame(&mut self.stream, message).expect("write client frame");
        self.stream.flush().expect("flush client frame");
    }

    pub(crate) fn write_pane_input(&mut self, pane_id: u32, generation: u64, bytes: &[u8]) {
        self.write_pane_input_ns(pane_id, generation, false, bytes)
    }

    pub(crate) fn write_pane_input_ns(
        &mut self,
        pane_id: u32,
        generation: u64,
        local: bool,
        bytes: &[u8],
    ) {
        write_pane_input_frame(&mut self.stream, pane_id, generation, local, bytes)
            .expect("write pane input frame");
        self.stream.flush().expect("flush pane input frame");
    }

    pub(crate) fn write_raw(&mut self, bytes: &[u8]) {
        self.stream
            .write_all(bytes)
            .expect("write raw client bytes");
        self.stream.flush().expect("flush raw client bytes");
    }

    /// Read and discard whatever the server has sent, briefly.
    ///
    /// An attached client that stops reading is not a passive observer: the server queues its
    /// outbound frames per client and drops one whose backlog exceeds `max_backlog`, discarding
    /// everything that client had already sent in the other direction. A connection that has to
    /// stay alive while waiting therefore has to keep consuming, the way a real attached client
    /// does.
    pub(crate) fn drain_available(&mut self) {
        let _ = self.stream.set_read_timeout(Some(Duration::from_millis(5)));
        let mut scratch = [0u8; 16 * 1024];
        // Bounded so a server streaming continuously cannot hold this loop forever.
        for _ in 0..64 {
            match self.stream.read(&mut scratch) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        let _ = self.stream.set_read_timeout(Some(IO_TIMEOUT));
    }
}

pub(crate) struct ListenerGuard {
    session: String,
    endpoint: IpcEndpoint,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl ListenerGuard {
    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    pub(crate) fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
    }

    fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + IO_TIMEOUT;
        // Held, not dropped, once the shutdown is sent. Attaching seeds this client with the whole
        // session state; if it stops reading, that backlog passes `max_backlog` and the server
        // drops it as a slow consumer - throwing away the `Shutdown` sitting unread in the other
        // direction. The server then runs until the join deadline and the test reports a hang whose
        // cause is several layers away. Keeping the connection and draining it is what a real
        // attached client does.
        let mut shutdown_client: Option<TestConnection> = None;
        while Instant::now() < deadline && shutdown_client.is_none() {
            if let Ok(stream) = IpcConnection::connect(&self.endpoint) {
                let mut client = TestConnection::new(stream);
                client.write_control(&attach_message(&self.session, "test-harness-shutdown"));
                let mut owns_control = false;
                read_until_deadline(&mut client, deadline, |frame| {
                    if let Frame::Control(ServerMessage::Attached {
                        client_id,
                        controller,
                        ..
                    }) = frame
                    {
                        owns_control = *controller == Some(*client_id);
                        true
                    } else {
                        false
                    }
                });
                if owns_control {
                    client.write_control(&ClientMessage::Shutdown);
                    shutdown_client = Some(client);
                }
            }
            if shutdown_client.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let mut shutdown_client =
            shutdown_client.expect("could not acquire control to stop test server");
        // Bound the join too. Every other wait here has a deadline, and a listener that misses the
        // shutdown would otherwise park the whole test binary indefinitely instead of failing.
        let join_deadline = Instant::now() + IO_TIMEOUT;
        while !thread.is_finished() {
            assert!(
                Instant::now() < join_deadline,
                "session listener thread did not exit after shutdown"
            );
            shutdown_client.drain_available();
        }
        drop(shutdown_client);
        let result = thread.join().expect("session listener thread panicked");
        result.expect("session listener failed");
        self.endpoint.remove_stale();
    }
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn spawn_listener(settings: ServerSettings) -> ListenerGuard {
    let session = unique_session_name();
    let (listener, endpoint) = bind_session_socket(&session).expect("bind test session listener");
    let server_session = session.clone();
    let thread = std::thread::Builder::new()
        .name(format!("session-test-{session}"))
        .spawn(move || {
            SessionServer::new_named_with_settings(server_session, settings).run_listener(listener)
        })
        .expect("spawn session listener thread");
    ListenerGuard {
        session,
        endpoint,
        thread: Some(thread),
    }
}

pub(crate) fn read_until(
    client: &mut TestConnection,
    done: impl FnMut(&Frame<ServerMessage>) -> bool,
) {
    assert!(
        read_until_deadline(client, Instant::now() + IO_TIMEOUT, done),
        "timed out waiting for server frame"
    );
}

fn read_until_deadline(
    client: &mut TestConnection,
    deadline: Instant,
    mut done: impl FnMut(&Frame<ServerMessage>) -> bool,
) -> bool {
    loop {
        if Instant::now() >= deadline {
            return false;
        }

        while let Some(frame) = client
            .decoder
            .next_frame::<ServerMessage>()
            .expect("decode server frame")
        {
            if done(&frame) {
                return true;
            }
        }

        match client.decoder.read_from_status(&mut client.stream) {
            Ok(_) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return false,
        }
    }
}

pub(crate) fn attach_message(session: &str, label: &str) -> ClientMessage {
    ClientMessage::Attach {
        session: session.to_string(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: label.to_string(),
        read_only: false,
    }
}

pub(crate) fn attach_client(
    endpoint: &IpcEndpoint,
    session: &str,
    label: &str,
) -> (TestConnection, ServerMessage) {
    let mut client = TestConnection::connect(endpoint);
    client.write_control(&attach_message(session, label));
    let mut attached = None;
    read_until(&mut client, |frame| {
        if let Frame::Control(message @ ServerMessage::Attached { .. }) = frame {
            attached = Some(message.clone());
            true
        } else {
            false
        }
    });
    (client, attached.expect("attached response"))
}

pub(crate) fn connect_when_ready(endpoint: &IpcEndpoint, child: &mut Child) -> TestConnection {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        if let Ok(stream) = IpcConnection::connect(endpoint) {
            return TestConnection::new(stream);
        }
        if let Some(status) = child.try_wait().expect("poll server process") {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.as_mut() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("session server exited early ({status}): {stderr}");
        }
        assert!(
            Instant::now() < deadline,
            "session server did not create {}",
            endpoint.path().display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Base directory for a test's runtime state, kept short enough to hold a Unix socket.
///
/// `sockaddr_un.sun_path` is 104 bytes on macOS (108 on Linux), and this base has a whole runtime
/// layout appended to it before a socket name lands at the end: `<base>/rozi/<session>.sock`.
/// macOS's per-user `TMPDIR` is `/var/folders/<2>/<30ish>/T/` on its own, which leaves too little
/// room, so prefer the short system temp directory there. `/private/tmp` rather than `/tmp`
/// because the latter is a symlink and [`ensure_private_dir`] rejects those by design.
pub(crate) fn private_temp_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .subsec_nanos();
    let path = base.join(format!(
        "hmux-{}-{nonce:x}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    rozi::platform::fs_security::ensure_private_dir(&path).expect("create private runtime base");
    path
}

pub(crate) fn unique_session_name() -> String {
    format!(
        "protocol-test-{}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn subprocess_endpoint(runtime_base: &std::path::Path, session: &str) -> IpcEndpoint {
    if cfg!(windows) {
        session_endpoint(session).expect("resolve subprocess session endpoint")
    } else {
        rozi::platform::ipc::EndpointRegistry::session_endpoint(&runtime_base.join("rozi"), session)
    }
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
