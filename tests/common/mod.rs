#![allow(dead_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hyprmux::platform::ipc::{IpcConnection, IpcEndpoint};
use hyprmux::session::protocol::{
    ClientMessage, Frame, FrameDecoder, PROTOCOL_VERSION, ServerMessage, write_control_frame,
    write_pane_input_frame,
};
use hyprmux::session::server::{
    ServerSettings, SessionServer, bind_session_socket, session_endpoint,
};

pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(5);

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
        write_pane_input_frame(&mut self.stream, pane_id, generation, bytes)
            .expect("write pane input frame");
        self.stream.flush().expect("flush pane input frame");
    }

    pub(crate) fn write_raw(&mut self, bytes: &[u8]) {
        self.stream
            .write_all(bytes)
            .expect("write raw client bytes");
        self.stream.flush().expect("flush raw client bytes");
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
        let mut shutdown_sent = false;
        while Instant::now() < deadline && !shutdown_sent {
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
                    shutdown_sent = true;
                }
            }
            if !shutdown_sent {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(
            shutdown_sent,
            "could not acquire control to stop test server"
        );
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

pub(crate) fn private_temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .subsec_nanos();
    let path = std::env::temp_dir().join(format!(
        "hmux-{}-{nonce:x}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    hyprmux::platform::fs_security::ensure_private_dir(&path).expect("create private runtime base");
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
        hyprmux::platform::ipc::EndpointRegistry::session_endpoint(
            &runtime_base.join("hyprmux"),
            session,
        )
    }
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
