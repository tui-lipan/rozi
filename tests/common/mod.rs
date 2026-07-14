use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hyprmux::platform::ipc::{IpcConnection, IpcEndpoint};
use hyprmux::session::protocol::{
    ClientMessage, Frame, FrameDecoder, ServerMessage, write_control_frame,
};

pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(5);

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ServerGuard {
    child: Child,
    runtime_base: PathBuf,
}

impl ServerGuard {
    pub(crate) fn new(child: Child, runtime_base: PathBuf) -> Self {
        Self {
            child,
            runtime_base,
        }
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(crate) fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            if self
                .child
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
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
}

pub(crate) fn read_until(
    client: &mut TestConnection,
    mut done: impl FnMut(&Frame<ServerMessage>) -> bool,
) {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for server frame"
        );

        while let Some(frame) = client
            .decoder
            .next_frame::<ServerMessage>()
            .expect("decode server frame")
        {
            if done(&frame) {
                return;
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
            Err(err) => panic!("failed to read server frame: {err}"),
        }
    }
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
        "protocol-smoke-{}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
