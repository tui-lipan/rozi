use criterion::{Criterion, Throughput};
use hyprmux::platform::ipc::EndpointRegistry;
use hyprmux::session::client::SessionClient;
use hyprmux::session::protocol::{Frame, ServerMessage};
use hyprmux::session::server::{ServerSettings, SessionServer};
use std::hint::black_box;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tui_lipan::prelude::TerminalColorPalette;

const HELPER_ARG: &str = "--server-fairness-helper";
const SERVER_ARG: &str = "--server-fairness-server";
const PANE_ID: u32 = 1;
const GENERATION: u64 = 1;
const READY: &[u8] = b"__HYPRMUX_FAIRNESS_READY__";
const ACK_PREFIX: &[u8] = b"__HYPRMUX_ACK_";
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

enum DrainEvent {
    SpawnResult { ok: bool, error: Option<String> },
    Ready,
    Ack(String),
}

struct ServerOwner {
    child: Option<Child>,
    session: String,
    endpoint: hyprmux::platform::ipc::IpcEndpoint,
    root: PathBuf,
}

impl ServerOwner {
    fn stop(&mut self, primary: Option<&SessionClient>) {
        if let Some(primary) = primary {
            primary.shutdown();
        }
        let Some(mut child) = self.child.take() else {
            self.cleanup_files();
            return;
        };
        if !wait_for_child(&mut child, SHUTDOWN_TIMEOUT) {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || while rx.recv().is_ok() {});
            if let Ok((fallback, _)) =
                SessionClient::connect_attached(&self.endpoint, self.session.clone(), tx, false)
            {
                fallback.request_control();
                fallback.shutdown();
            }
        }
        if !wait_for_child(&mut child, SHUTDOWN_TIMEOUT) {
            hyprmux::platform::server_lifecycle::terminate_server(child.id());
        }
        if !wait_for_child(&mut child, SHUTDOWN_TIMEOUT) {
            let _ = child.kill();
            let _ = wait_for_child(&mut child, SHUTDOWN_TIMEOUT);
        }
        self.cleanup_files();
    }

    fn cleanup_files(&self) {
        self.endpoint.remove_stale();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Drop for ServerOwner {
    fn drop(&mut self) {
        self.stop(None);
    }
}

struct FairnessFixture {
    client: Option<SessionClient>,
    events: mpsc::Receiver<DrainEvent>,
    drain_done: mpsc::Receiver<()>,
    drain: Option<JoinHandle<()>>,
    server: ServerOwner,
}

impl FairnessFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos() as u64;
        let root = std::env::temp_dir().join(format!("hmsf-{}-{nonce:x}", std::process::id()));
        let session = format!("b{nonce:x}");
        let endpoint = EndpointRegistry::session_endpoint(&root, &session);
        let mut server = ServerOwner {
            child: None,
            session: session.clone(),
            endpoint: endpoint.clone(),
            root,
        };
        hyprmux::platform::fs_security::ensure_private_dir(&server.root)
            .expect("create private benchmark runtime");
        let executable = std::env::current_exe().expect("locate benchmark executable");
        server.child = Some(
            Command::new(&executable)
                .arg(SERVER_ARG)
                .arg(&server.root)
                .arg(&session)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn benchmark server process"),
        );

        let (tx, inbound) = mpsc::channel();
        let deadline = Instant::now() + IO_TIMEOUT;
        let (client, attached) = loop {
            match SessionClient::connect_attached(&endpoint, session.clone(), tx.clone(), false) {
                Ok(attached) => break attached,
                Err(error) => {
                    if let Some(status) = server
                        .child
                        .as_mut()
                        .expect("server child installed")
                        .try_wait()
                        .expect("poll benchmark server")
                    {
                        panic!("benchmark server exited during startup ({status}): {error}");
                    }
                    assert!(
                        Instant::now() < deadline,
                        "benchmark server did not accept connections: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        assert!(matches!(attached, ServerMessage::Attached { .. }));
        let (events, drain_done, drain) = spawn_drain(inbound);

        let helper = executable.to_string_lossy().into_owned();
        client.spawn_pane(
            PANE_ID,
            GENERATION,
            None,
            None,
            250,
            60,
            false,
            Vec::new(),
            Some("server-fairness".to_string()),
            TerminalColorPalette::default(),
            vec![helper, HELPER_ARG.to_string()],
            Vec::new(),
        );

        let deadline = Instant::now() + IO_TIMEOUT;
        let mut spawned = false;
        let mut ready = false;
        while !(spawned && ready) {
            let event = events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("helper did not become ready");
            match event {
                DrainEvent::SpawnResult { ok, error } => {
                    assert!(ok, "helper spawn failed: {error:?}");
                    spawned = true;
                }
                DrainEvent::Ready => ready = true,
                DrainEvent::Ack(_) => {}
            }
        }

        Self {
            client: Some(client),
            events,
            drain_done,
            drain: Some(drain),
            server,
        }
    }

    fn wait_for_ack(&self, expected: &str) {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("timed out waiting for helper acknowledgement");
            if let DrainEvent::Ack(key) = event
                && key == expected
            {
                return;
            }
        }
    }
}

impl Drop for FairnessFixture {
    fn drop(&mut self) {
        self.server.stop(self.client.as_ref());
        self.client.take();
        if self.drain_done.recv_timeout(SHUTDOWN_TIMEOUT).is_ok()
            && let Some(drain) = self.drain.take()
        {
            let _ = drain.join();
        }
    }
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn spawn_drain(
    inbound: mpsc::Receiver<Frame<ServerMessage>>,
) -> (
    mpsc::Receiver<DrainEvent>,
    mpsc::Receiver<()>,
    JoinHandle<()>,
) {
    let (event_tx, event_rx) = mpsc::sync_channel(8);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let drain = std::thread::Builder::new()
        .name("server-fairness-drain".to_string())
        .spawn(move || {
            let mut tail = Vec::new();
            let mut ready_sent = false;
            while let Ok(frame) = inbound.recv() {
                match frame {
                    Frame::Control(ServerMessage::SpawnResult {
                        pane_id: PANE_ID,
                        generation: GENERATION,
                        ok,
                        error,
                        ..
                    }) => {
                        if event_tx
                            .send(DrainEvent::SpawnResult { ok, error })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Frame::PaneBytes {
                        pane_id: PANE_ID,
                        generation: GENERATION,
                        bytes,
                    } => {
                        tail.extend_from_slice(&bytes);
                        if !ready_sent && contains(&tail, READY) {
                            ready_sent = true;
                            if event_tx.send(DrainEvent::Ready).is_err() {
                                break;
                            }
                        }
                        while let Some(start) = find_bytes(&tail, ACK_PREFIX) {
                            let value_start = start + ACK_PREFIX.len();
                            let Some(relative_end) = find_bytes(&tail[value_start..], b"__") else {
                                tail.drain(..start);
                                break;
                            };
                            let value_end = value_start + relative_end;
                            let key =
                                String::from_utf8_lossy(&tail[value_start..value_end]).into_owned();
                            tail.drain(..value_end + 2);
                            if event_tx.send(DrainEvent::Ack(key)).is_err() {
                                break;
                            }
                        }
                        let keep = READY
                            .len()
                            .max(ACK_PREFIX.len())
                            .saturating_sub(1)
                            .min(tail.len());
                        if find_bytes(&tail, ACK_PREFIX).is_none() {
                            tail.drain(..tail.len() - keep);
                        }
                    }
                    _ => {}
                }
            }
            let _ = done_tx.send(());
        })
        .expect("spawn continuous frame drain");
    (event_rx, done_rx, drain)
}

fn server_fairness(c: &mut Criterion) {
    let fixture = FairnessFixture::new();
    let mut group = c.benchmark_group("server_fairness");
    group.throughput(Throughput::Elements(1));
    group.bench_function("key_round_trip/continuous_pty_ingress", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for sequence in 0..iterations {
                let key = format!("key-{sequence:016x}");
                fixture
                    .client
                    .as_ref()
                    .expect("fairness client available")
                    .send_input(
                        PANE_ID,
                        GENERATION,
                        black_box(format!("{key}\n").into_bytes()),
                    );
                fixture.wait_for_ack(&key);
            }
            started.elapsed()
        });
    });
    group.finish();

    // TODO(perf): add a resurrection snapshot-duration matrix when the public protocol exposes a
    // completion acknowledgement after the durable write/rename/sync boundary. Watching the
    // snapshot path would mix server scheduling and observer polling delay into the measurement,
    // while SpawnResult currently includes unrelated spawn and response work.
}

fn run_helper() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(READY)?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()?;
    std::thread::sleep(Duration::from_millis(50));

    std::thread::spawn(|| {
        let mut chunk = Vec::with_capacity(8 * 1024);
        while chunk.len() < 8 * 1024 {
            chunk.extend_from_slice(
                b"server-fairness-continuous-pty-ingress-0123456789abcdef0123456789abcdef\r\n",
            );
        }
        loop {
            if io::stdout().write_all(&chunk).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    for line in io::stdin().lock().lines() {
        let line = line?;
        stdout.write_all(format!("\r\n__HYPRMUX_ACK_{line}__\r\n").as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}

fn run_server(root: PathBuf, session: String) -> io::Result<()> {
    if let Err(error) = hyprmux::platform::server_lifecycle::contain_children() {
        eprintln!("server fairness containment unavailable: {error}");
    }
    if let Err(error) = hyprmux::platform::server_lifecycle::install_shutdown_handler() {
        eprintln!("server fairness shutdown handler unavailable: {error}");
    }
    let endpoint = EndpointRegistry::session_endpoint(&root, &session);
    let listener = endpoint.bind()?.into_listener();
    let result = SessionServer::new_named_with_settings(session, ServerSettings::default())
        .run_listener(listener);
    endpoint.remove_stale();
    result
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next();
    if mode.as_deref() == Some(HELPER_ARG) {
        run_helper().expect("server fairness helper failed");
        return;
    }
    if mode.as_deref() == Some(SERVER_ARG) {
        let root = PathBuf::from(args.next().expect("server helper requires endpoint root"));
        let session = args.next().expect("server helper requires session name");
        run_server(root, session).expect("server fairness server failed");
        return;
    }
    let mut criterion = Criterion::default().configure_from_args();
    server_fairness(&mut criterion);
    criterion.final_summary();
}
