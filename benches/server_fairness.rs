#[path = "support/mod.rs"]
mod bench_support;

use criterion::{BenchmarkId, Criterion, SamplingMode, Throughput};
use rozi::platform::ipc::EndpointRegistry;
use rozi::runtime_metrics::ServerRuntimeMetrics;
use rozi::session::client::{SessionClient, SpawnPaneRequest};
use rozi::session::protocol::{Frame, ServerMessage};
use rozi::session::server::{ServerSettings, SessionServer};
use std::collections::HashMap;
use std::hint::black_box;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tui_lipan::prelude::TerminalColorPalette;

const HELPER_ARG: &str = "--server-fairness-helper";
const SNAPSHOT_HELPER_ARG: &str = "--resurrection-snapshot-helper";
const SERVER_ARG: &str = "--server-fairness-server";
const SATURATION_PROBE_ARG: &str = "--saturation-probe";
const PANE_ID: u32 = 1;
const SNAPSHOT_PANE_ID: u32 = 1;
const SATURATION_PANES: u32 = 2;
const GENERATION: u64 = 1;
const READY: &[u8] = b"__ROZI_FAIRNESS_READY__";
const ACK_PREFIX: &[u8] = b"__ROZI_ACK_";
const SNAPSHOT_READY_PREFIX: &[u8] = b"__ROZI_SNAPSHOT_READY_";
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_IO_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PTY_EVENT_CHUNK: u64 = 64 * 1024;
const FAIRNESS_TAIL_CAP: usize = 256;

#[derive(Clone, Copy)]
struct ServerLaunch {
    resurrect: bool,
    scrollback: usize,
}

impl Default for ServerLaunch {
    fn default() -> Self {
        Self {
            resurrect: false,
            scrollback: 5_000,
        }
    }
}

enum DrainEvent {
    SpawnResult {
        pane_id: u32,
        ok: bool,
        error: Option<String>,
    },
    Ready(u32),
    Ack(String),
}

enum SnapshotDrainEvent {
    SpawnResult {
        pane_id: u32,
        ok: bool,
        error: Option<String>,
    },
    Ready(u32),
}

struct ServerOwner {
    child: Option<Child>,
    session: String,
    endpoint: rozi::platform::ipc::IpcEndpoint,
    root: PathBuf,
}

impl ServerOwner {
    fn start(launch: ServerLaunch) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos() as u64;
        let root = std::env::temp_dir().join(format!("hmsf-{}-{nonce:x}", std::process::id()));
        let session = format!("b{nonce:x}");
        let endpoint = EndpointRegistry::session_endpoint(&root, &session);
        let mut owner = Self {
            child: None,
            session,
            endpoint,
            root,
        };
        rozi::platform::fs_security::ensure_private_dir(&owner.root)
            .expect("create private benchmark runtime");
        let executable = std::env::current_exe().expect("locate benchmark executable");
        owner.child = Some(
            Command::new(executable)
                .arg(SERVER_ARG)
                .arg(&owner.root)
                .arg(&owner.session)
                .arg(if launch.resurrect { "1" } else { "0" })
                .arg(launch.scrollback.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn benchmark server process"),
        );
        owner
    }

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
            rozi::platform::server_lifecycle::terminate_server(child.id());
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

    fn snapshot_path(&self) -> PathBuf {
        self.root.join("snapshots").join(&self.session)
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
    ingress_started: Instant,
    server: ServerOwner,
}

impl FairnessFixture {
    fn new(pane_count: u32, pace_millis: u64) -> Self {
        let mut server = ServerOwner::start(ServerLaunch::default());
        let session = server.session.clone();
        let endpoint = server.endpoint.clone();
        let executable = std::env::current_exe().expect("locate benchmark executable");

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
        for pane_id in 1..=pane_count {
            client.spawn_pane(SpawnPaneRequest {
                pane_id,
                local: false,
                generation: GENERATION,
                launch: None,
                cwd: None,
                cols: 250,
                rows: 60,
                keep_open: false,
                env: Vec::new(),
                title: Some(format!("server-fairness-{pane_id}")),
                palette: TerminalColorPalette::default(),
                shell: vec![
                    helper.clone(),
                    HELPER_ARG.to_string(),
                    pace_millis.to_string(),
                ],
                command_shell: Vec::new(),
            });
            wait_for_fairness_pane(&events, pane_id);
        }
        let ingress_started = Instant::now();
        for pane_id in 1..=pane_count {
            client.send_input(pane_id, GENERATION, false, b"GO\n".to_vec());
        }

        Self {
            client: Some(client),
            events,
            drain_done,
            drain: Some(drain),
            ingress_started,
            server,
        }
    }

    fn wait_for_saturation(&self) -> ServerRuntimeMetrics {
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut observed_high_water = 0;
        loop {
            let client = self.client.as_ref().expect("fairness client available");
            client.request_runtime_metrics();
            if let Some(server) = client.runtime_stats().server {
                let ingress = server.sample.pty_ingress.bytes;
                observed_high_water = observed_high_water.max(ingress.high_water_bytes);
                assert_eq!(
                    ingress.capacity_bytes,
                    4 * 1024 * 1024,
                    "unexpected PTY ingress capacity"
                );
                if ingress.high_water_bytes + MAX_PTY_EVENT_CHUNK >= ingress.capacity_bytes {
                    eprintln!(
                        "saturated PTY ingress: high_water={} capacity={} outbox={}/{}/{} clients={}",
                        ingress.high_water_bytes,
                        ingress.capacity_bytes,
                        server.sample.client_outboxes.bytes.current_bytes,
                        server.sample.client_outboxes.bytes.high_water_bytes,
                        server.sample.client_outboxes.bytes.capacity_bytes,
                        server.sample.client_outboxes.clients,
                    );
                    return server.sample;
                }
            }
            assert!(
                Instant::now() < deadline,
                "PTY ingress did not saturate within {IO_TIMEOUT:?}; expected high-water within \
                 {MAX_PTY_EVENT_CHUNK} bytes of the 4 MiB capacity, observed {observed_high_water}"
            );
            std::thread::sleep(Duration::from_millis(1));
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

    fn expect_disconnect(&mut self) -> Duration {
        self.drain_done
            .recv_timeout(IO_TIMEOUT)
            .expect("saturated client did not reach the bounded disconnect");
        if let Some(drain) = self.drain.take() {
            drain.join().expect("join disconnected frame drain");
        }
        self.ingress_started.elapsed()
    }
}

impl Drop for FairnessFixture {
    fn drop(&mut self) {
        self.server.stop(self.client.as_ref());
        self.client.take();
        if self.drain.is_some()
            && self.drain_done.recv_timeout(SHUTDOWN_TIMEOUT).is_ok()
            && let Some(drain) = self.drain.take()
        {
            let _ = drain.join();
        }
    }
}

fn wait_for_fairness_pane(events: &mpsc::Receiver<DrainEvent>, pane_id: u32) {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut spawned = false;
    let mut ready = false;
    while !(spawned && ready) {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("fairness helper did not become ready");
        match event {
            DrainEvent::SpawnResult {
                pane_id: event_pane,
                ok,
                error,
            } if event_pane == pane_id => {
                assert!(ok, "fairness helper {pane_id} spawn failed: {error:?}");
                spawned = true;
            }
            DrainEvent::Ready(event_pane) if event_pane == pane_id => ready = true,
            _ => {}
        }
    }
}

struct SnapshotFixture {
    client: Option<SessionClient>,
    last_attempts: u64,
    /// Server-loop blocking of the most recent attempt. Criterion times the whole attempt, so this
    /// is reported alongside rather than through the measured value.
    last_blocking_us: u64,
    drain_done: mpsc::Receiver<()>,
    drain: Option<JoinHandle<()>>,
    server: ServerOwner,
}

impl SnapshotFixture {
    fn new(pane_count: usize, history_rows: usize) -> Self {
        let mut server = ServerOwner::start(ServerLaunch {
            resurrect: true,
            scrollback: history_rows,
        });
        let (tx, inbound) = mpsc::channel();
        let deadline = Instant::now() + SNAPSHOT_IO_TIMEOUT;
        let (client, attached) = loop {
            match SessionClient::connect_attached(
                &server.endpoint,
                server.session.clone(),
                tx.clone(),
                false,
            ) {
                Ok(attached) => break attached,
                Err(error) => {
                    if let Some(status) = server
                        .child
                        .as_mut()
                        .expect("server child installed")
                        .try_wait()
                        .expect("poll benchmark server")
                    {
                        panic!("snapshot server exited during startup ({status}): {error}");
                    }
                    assert!(
                        Instant::now() < deadline,
                        "snapshot server did not accept connections: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        assert!(matches!(attached, ServerMessage::Attached { .. }));
        // The metrics this benchmark reads arrived in protocol 18. Pinning equality meant every
        // later protocol bump broke the benchmark instead of exercising it; 20 is current.
        assert!(
            client.effective_protocol() >= 18,
            "snapshot benchmark requires protocol 18 metrics, negotiated {}",
            client.effective_protocol()
        );
        let (events, drain_done, drain) = spawn_snapshot_drain(inbound);
        let executable = std::env::current_exe()
            .expect("locate benchmark executable")
            .to_string_lossy()
            .into_owned();

        for pane_index in 0..pane_count {
            let pane_id = pane_index as u32 + 1;
            client.spawn_pane(SpawnPaneRequest {
                pane_id,
                local: false,
                generation: GENERATION,
                launch: None,
                cwd: None,
                cols: bench_support::SNAPSHOT_COLS,
                rows: bench_support::SNAPSHOT_ROWS,
                keep_open: false,
                env: Vec::new(),
                title: Some(format!("snapshot-{pane_id:02}")),
                palette: TerminalColorPalette::default(),
                shell: vec![
                    executable.clone(),
                    SNAPSHOT_HELPER_ARG.to_string(),
                    pane_id.to_string(),
                    history_rows.to_string(),
                ],
                command_shell: Vec::new(),
            });
            wait_for_snapshot_pane(&events, pane_id);
        }

        let metrics = wait_for_server_metrics(&client, SNAPSHOT_IO_TIMEOUT, |metrics| {
            metrics.resurrection.attempts > 0
                && metrics.resurrection.attempts
                    == metrics.resurrection.successes + metrics.resurrection.failures
        });
        assert_eq!(
            metrics.resurrection.failures, 0,
            "snapshot fixture setup must complete successfully"
        );
        validate_snapshot_shape(&server.snapshot_path(), pane_count, history_rows);

        Self {
            client: Some(client),
            last_attempts: metrics.resurrection.attempts,
            last_blocking_us: 0,
            drain_done,
            drain: Some(drain),
            server,
        }
    }

    fn measure_snapshot(&mut self, sequence: u64) -> Duration {
        let client = self.client.as_ref().expect("snapshot client available");
        client.send_input(
            SNAPSHOT_PANE_ID,
            GENERATION,
            false,
            format!("{sequence:016x}\n").into_bytes(),
        );
        let expected_attempts = self.last_attempts.saturating_add(1);
        // Attempts are counted when the server dispatches, durations only when the worker reports
        // back. Waiting on the completion counters is what keeps `last_duration_us` from being
        // read off the *previous* attempt.
        let metrics = wait_for_server_metrics(client, SNAPSHOT_IO_TIMEOUT, |metrics| {
            metrics.resurrection.successes + metrics.resurrection.failures >= expected_attempts
        });
        assert_eq!(
            metrics.resurrection.attempts, expected_attempts,
            "one trigger must produce exactly one dirty snapshot generation"
        );
        assert_eq!(
            metrics.resurrection.failures, 0,
            "timed snapshot attempt failed"
        );
        assert!(
            metrics.resurrection.last_duration_us > 0,
            "server reported a zero-duration snapshot"
        );
        assert!(
            metrics.resurrection.last_blocking_us <= metrics.resurrection.last_duration_us,
            "server-loop blocking cannot exceed the whole attempt"
        );
        self.last_attempts = expected_attempts;
        self.last_blocking_us = metrics.resurrection.last_blocking_us;
        Duration::from_micros(metrics.resurrection.last_duration_us)
    }
}

impl Drop for SnapshotFixture {
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

fn wait_for_server_metrics(
    client: &SessionClient,
    timeout: Duration,
    predicate: impl Fn(&ServerRuntimeMetrics) -> bool,
) -> ServerRuntimeMetrics {
    let deadline = Instant::now() + timeout;
    loop {
        client.request_runtime_metrics();
        if let Some(server) = client.runtime_stats().server
            && predicate(&server.sample)
        {
            return server.sample;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the requested protocol-18 server metrics"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_snapshot_pane(events: &mpsc::Receiver<SnapshotDrainEvent>, pane_id: u32) {
    let deadline = Instant::now() + SNAPSHOT_IO_TIMEOUT;
    let mut spawned = false;
    let mut ready = false;
    while !(spawned && ready) {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("snapshot helper did not become ready");
        match event {
            SnapshotDrainEvent::SpawnResult {
                pane_id: event_pane,
                ok,
                error,
            } if event_pane == pane_id => {
                assert!(ok, "snapshot helper {pane_id} spawn failed: {error:?}");
                spawned = true;
            }
            SnapshotDrainEvent::Ready(event_pane) if event_pane == pane_id => ready = true,
            _ => {}
        }
    }
}

fn validate_snapshot_shape(path: &std::path::Path, pane_count: usize, history_rows: usize) {
    let meta: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path.join("meta.json")).expect("read snapshot fixture metadata"),
    )
    .expect("parse snapshot fixture metadata");
    let panes = meta["panes"]
        .as_array()
        .expect("snapshot metadata panes array");
    assert_eq!(panes.len(), pane_count, "snapshot fixture pane count");
    for pane in panes {
        let pane_id = pane["pane_id"].as_u64().expect("snapshot pane id") as u32;
        assert_eq!(
            pane["cols"].as_u64(),
            Some(u64::from(bench_support::SNAPSHOT_COLS))
        );
        assert_eq!(
            pane["rows"].as_u64(),
            Some(u64::from(bench_support::SNAPSHOT_ROWS))
        );
        let replay = std::fs::read(path.join("panes").join(format!("{pane_id}.replay")))
            .expect("read snapshot pane replay");
        let mut screen = tui_lipan::prelude::TerminalScreen::new(
            bench_support::SNAPSHOT_ROWS,
            bench_support::SNAPSHOT_COLS,
            history_rows,
        );
        screen.process_bytes(&replay);
        assert_eq!(
            screen.total_scrollback_rows(),
            history_rows,
            "snapshot pane {pane_id} retained history"
        );
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
            let mut tails: HashMap<u32, Vec<u8>> = HashMap::new();
            let mut ready = HashMap::<u32, bool>::new();
            while let Ok(frame) = inbound.recv() {
                match frame {
                    Frame::Control(ServerMessage::SpawnResult {
                        pane_id,
                        generation: GENERATION,
                        ok,
                        error,
                        ..
                    }) => {
                        if event_tx
                            .send(DrainEvent::SpawnResult { pane_id, ok, error })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Frame::PaneBytes {
                        pane_id,
                        generation: GENERATION,
                        bytes,
                        ..
                    } => {
                        let tail = tails.entry(pane_id).or_default();
                        tail.extend_from_slice(&bytes);
                        if !ready.get(&pane_id).copied().unwrap_or(false) && contains(tail, READY) {
                            ready.insert(pane_id, true);
                            if event_tx.send(DrainEvent::Ready(pane_id)).is_err() {
                                break;
                            }
                        }
                        if pane_id == PANE_ID {
                            while let Some(start) = find_bytes(tail, ACK_PREFIX) {
                                let value_start = start + ACK_PREFIX.len();
                                let Some(relative_end) = find_bytes(&tail[value_start..], b"__")
                                else {
                                    tail.drain(..start);
                                    break;
                                };
                                let value_end = value_start + relative_end;
                                let key = String::from_utf8_lossy(&tail[value_start..value_end])
                                    .into_owned();
                                tail.drain(..value_end + 2);
                                if event_tx.send(DrainEvent::Ack(key)).is_err() {
                                    break;
                                }
                            }
                        }
                        if tail.len() > FAIRNESS_TAIL_CAP {
                            tail.drain(..tail.len() - FAIRNESS_TAIL_CAP);
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

fn spawn_snapshot_drain(
    inbound: mpsc::Receiver<Frame<ServerMessage>>,
) -> (
    mpsc::Receiver<SnapshotDrainEvent>,
    mpsc::Receiver<()>,
    JoinHandle<()>,
) {
    let (event_tx, event_rx) = mpsc::sync_channel(4);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let drain = std::thread::Builder::new()
        .name("resurrection-snapshot-drain".to_string())
        .spawn(move || {
            let mut tails: HashMap<u32, Vec<u8>> = HashMap::new();
            let mut ready = HashMap::<u32, bool>::new();
            while let Ok(frame) = inbound.recv() {
                match frame {
                    Frame::Control(ServerMessage::SpawnResult {
                        pane_id, ok, error, ..
                    }) => {
                        if event_tx
                            .send(SnapshotDrainEvent::SpawnResult { pane_id, ok, error })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Frame::PaneBytes { pane_id, bytes, .. }
                        if !ready.get(&pane_id).copied().unwrap_or(false) =>
                    {
                        let tail = tails.entry(pane_id).or_default();
                        tail.extend_from_slice(&bytes);
                        let marker = format!(
                            "{}{pane_id:08x}__",
                            String::from_utf8_lossy(SNAPSHOT_READY_PREFIX)
                        );
                        if contains(tail, marker.as_bytes()) {
                            ready.insert(pane_id, true);
                            tails.remove(&pane_id);
                            if event_tx.send(SnapshotDrainEvent::Ready(pane_id)).is_err() {
                                break;
                            }
                        } else {
                            let keep = marker.len().saturating_sub(1).min(tail.len());
                            tail.drain(..tail.len() - keep);
                        }
                    }
                    _ => {}
                }
            }
            let _ = done_tx.send(());
        })
        .expect("spawn snapshot frame drain");
    (event_rx, done_rx, drain)
}

fn saturation_probe() {
    let mut fixture = FairnessFixture::new(SATURATION_PANES, 0);
    let metrics = fixture.wait_for_saturation();
    let disconnected_after = fixture.expect_disconnect();
    assert!(
        fixture
            .server
            .child
            .as_mut()
            .expect("saturation server child installed")
            .try_wait()
            .expect("poll saturation server")
            .is_none(),
        "saturation probe server exited instead of disconnecting only the bounded client"
    );
    assert_eq!(
        metrics.client_outboxes.bytes.capacity_bytes,
        8 * 1024 * 1024,
        "unexpected single-client outbox capacity"
    );
    eprintln!(
        "saturation_probe pty_high_water={} pty_capacity={} outbox_high_water={} \
         outbox_capacity={} disconnected_after_ms={}",
        metrics.pty_ingress.bytes.high_water_bytes,
        metrics.pty_ingress.bytes.capacity_bytes,
        metrics.client_outboxes.bytes.high_water_bytes,
        metrics.client_outboxes.bytes.capacity_bytes,
        disconnected_after.as_millis()
    );
}

fn server_fairness(c: &mut Criterion) {
    let mut group = c.benchmark_group("server_fairness");
    group.throughput(Throughput::Elements(1));
    let mut fairness_fixture = None;
    group.bench_function("key_round_trip/continuous_pty_ingress", |b| {
        let fixture = fairness_fixture.get_or_insert_with(|| FairnessFixture::new(1, 1));
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
                        false,
                        black_box(format!("{key}\n").into_bytes()),
                    );
                fixture.wait_for_ack(&key);
            }
            started.elapsed()
        });
    });
    group.finish();

    let mut group = c.benchmark_group("resurrection_snapshot");
    group
        .sample_size(10)
        .sampling_mode(SamplingMode::Flat)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2));
    for pane_count in bench_support::SNAPSHOT_PANE_COUNTS {
        for history_rows in bench_support::SNAPSHOT_HISTORY_ROWS {
            let mut snapshot_fixture = None;
            group.bench_with_input(
                BenchmarkId::new(
                    format!("panes_{pane_count}"),
                    format!("history_{history_rows}"),
                ),
                &(pane_count, history_rows),
                |b, &(pane_count, history_rows)| {
                    let fixture = snapshot_fixture
                        .get_or_insert_with(|| SnapshotFixture::new(pane_count, history_rows));
                    let mut max_blocking_us = 0;
                    b.iter_custom(|iterations| {
                        let mut measured = Duration::ZERO;
                        for sequence in 0..iterations {
                            measured += fixture.measure_snapshot(sequence);
                            max_blocking_us = max_blocking_us.max(fixture.last_blocking_us);
                        }
                        measured
                    });
                    // Criterion times the whole attempt. The figure that bounds input latency is
                    // how long the server loop itself was held, so report it explicitly rather
                    // than leaving the total to imply it.
                    eprintln!(
                        "resurrection_snapshot/panes_{pane_count}/history_{history_rows} \
                         max_server_loop_blocking_us={max_blocking_us}"
                    );
                },
            );
        }
    }
    group.finish();
}

fn run_helper(pace_millis: u64) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(READY)?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()?;

    let (ack_tx, ack_rx) = mpsc::sync_channel::<String>(1);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                return;
            };
            if ack_tx.send(line).is_err() {
                return;
            }
        }
    });
    let start = ack_rx.recv().map_err(|_| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "fairness helper lost its start command",
        )
    })?;
    if start != "GO" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fairness helper expected GO",
        ));
    }

    let mut chunk = Vec::with_capacity(8 * 1024);
    while chunk.len() < 8 * 1024 {
        chunk.extend_from_slice(
            b"server-fairness-continuous-pty-ingress-0123456789abcdef0123456789abcdef\r\n",
        );
    }
    loop {
        while let Ok(line) = ack_rx.try_recv() {
            stdout.write_all(format!("\r\n__ROZI_ACK_{line}__\r\n").as_bytes())?;
        }
        stdout.write_all(&chunk)?;
        if pace_millis > 0 {
            std::thread::sleep(Duration::from_millis(pace_millis));
        }
    }
}

fn run_snapshot_helper(pane_id: u32, history_rows: usize) -> io::Result<()> {
    disable_stdin_echo()?;
    let mut stdout = io::stdout();
    stdout.write_all(&bench_support::resurrection_snapshot_corpus(
        pane_id,
        history_rows,
    ))?;
    stdout.write_all(format!("\x1b]2;__ROZI_SNAPSHOT_READY_{pane_id:08x}__\x07").as_bytes())?;
    stdout.flush()?;

    for line in io::stdin().lock().lines() {
        let line = line?;
        let cell = b'a' + line.bytes().fold(0_u8, u8::wrapping_add) % 26;
        stdout.write_all(
            format!(
                "\x1b[{};1H{}\x1b[{};1H",
                bench_support::SNAPSHOT_ROWS,
                char::from(cell),
                bench_support::SNAPSHOT_ROWS
            )
            .as_bytes(),
        )?;
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(unix)]
fn disable_stdin_echo() -> io::Result<()> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: stdin is a live PTY in this helper mode and `termios` points to writable storage.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `tcgetattr` initialized `termios` after the successful return above.
    let mut termios = unsafe { termios.assume_init() };
    termios.c_lflag &= !(libc::ECHO | libc::ECHONL);
    // SAFETY: the initialized termios value belongs to this helper's stdin PTY.
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn disable_stdin_echo() -> io::Result<()> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };

    // SAFETY: these calls only query and update this helper process's standard-input console mode.
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0
            || SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn run_server(
    root: PathBuf,
    session: String,
    resurrect: bool,
    scrollback: usize,
) -> io::Result<()> {
    if let Err(error) = rozi::platform::server_lifecycle::contain_children() {
        eprintln!("server fairness containment unavailable: {error}");
    }
    if let Err(error) = rozi::platform::server_lifecycle::install_shutdown_handler() {
        eprintln!("server fairness shutdown handler unavailable: {error}");
    }
    let endpoint = EndpointRegistry::session_endpoint(&root, &session);
    let listener = endpoint.bind()?.into_listener();
    let settings = ServerSettings {
        resurrect,
        snapshot_dir: resurrect.then(|| root.join("snapshots")),
        snapshot_interval: if resurrect {
            Duration::ZERO
        } else {
            ServerSettings::default().snapshot_interval
        },
        scrollback,
        ..ServerSettings::default()
    };
    let result = SessionServer::new_named_with_settings(session, settings).run_listener(listener);
    endpoint.remove_stale();
    result
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next();
    if mode.as_deref() == Some(SATURATION_PROBE_ARG) {
        saturation_probe();
        return;
    }
    if mode.as_deref() == Some(HELPER_ARG) {
        let pace_millis = args
            .next()
            .expect("server fairness helper requires pace")
            .parse()
            .expect("server fairness helper pace");
        run_helper(pace_millis).expect("server fairness helper failed");
        return;
    }
    if mode.as_deref() == Some(SNAPSHOT_HELPER_ARG) {
        let pane_id = args
            .next()
            .expect("snapshot helper requires pane id")
            .parse()
            .expect("snapshot helper pane id");
        let history_rows = args
            .next()
            .expect("snapshot helper requires retained history")
            .parse()
            .expect("snapshot helper retained history");
        run_snapshot_helper(pane_id, history_rows).expect("resurrection snapshot helper failed");
        return;
    }
    if mode.as_deref() == Some(SERVER_ARG) {
        let root = PathBuf::from(args.next().expect("server helper requires endpoint root"));
        let session = args.next().expect("server helper requires session name");
        let resurrect = args.next().expect("server helper requires resurrect flag") == "1";
        let scrollback = args
            .next()
            .expect("server helper requires scrollback")
            .parse()
            .expect("server helper scrollback");
        run_server(root, session, resurrect, scrollback).expect("server fairness server failed");
        return;
    }
    let mut criterion = Criterion::default().configure_from_args();
    server_fairness(&mut criterion);
    criterion.final_summary();
}
