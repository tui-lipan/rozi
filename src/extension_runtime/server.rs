//! `rozi extensions runtime`: the host side of one client's extension runtime.
//!
//! Started by the client over its own SSH channel and owned by it: it serves exactly one client,
//! and when that channel closes - the client detached, exited, or lost the network - it kills every
//! process it started and exits. It is never a daemon and never part of a session.
//!
//! It runs what it is told from bundles it verifies, relays the control connections those processes
//! open on its extension bridge back to the client as raw bytes, and decides nothing about them:
//! the client authenticates and authorizes every request itself.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::bundle::Bundle;
use super::protocol::{
    self, Capture, Frame, Hello, Launch, Message, SpawnCwd, SpawnFailure, read_frame,
};
use super::store::{BundleStore, StoreError};
use crate::platform::command::{CommandGroup, configure_command_group, kill_on_supervisor_death};
use crate::platform::ipc::{EndpointRegistry, IpcConnection};

/// Environment the runtime sets for every process it starts, and never takes from the caller.
const OWNED_ENV: &[&str] = &[
    "ROZI",
    "ROZI_BIN",
    "ROZI_SOCKET",
    "ROZI_EXTENSION_DIR",
    crate::state::CREDENTIAL_ENV,
];

/// Environment a process must not inherit from the runtime itself: the runtime may have been
/// started from inside a pane, and those values describe that pane, not this process.
const INHERITED_ENV_REMOVED: &[&str] = &[
    "ROZI_PANE",
    "ROZI_SESSION_INSTANCE",
    "ROZI_EXTENSION",
    "ROZI_EXTENSION_GENERATION",
    "ROZI_EXTENSION_CONFIG",
    "ROZI_SERVICE",
];

/// How often running processes are checked for exit.
const TICK: Duration = Duration::from_millis(50);

/// Largest total of bundle bytes held in memory while being received.
const MAX_PENDING_STAGE: usize = 2 * super::bundle::MAX_BUNDLE_BYTES as usize;

#[derive(Default)]
pub struct ServeOptions {
    /// Bundle cache directory. Defaults to the user's cache directory.
    pub store_root: Option<PathBuf>,
    /// Directory for the bridge endpoint. Defaults to the runtime directory.
    pub runtime_dir: Option<PathBuf>,
}

/// Serve one client over `input`/`output` (the SSH channel's stdin and stdout) until it closes.
pub fn serve(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
) -> io::Result<()> {
    serve_with(input, output, ServeOptions::default())
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

enum Event {
    Frame(Frame),
    InputClosed,
    BridgeOpened {
        conn: u64,
        writer: mpsc::Sender<Option<Vec<u8>>>,
    },
    BridgeEnded {
        conn: u64,
    },
}

struct Running {
    child: Child,
    group: CommandGroup,
    // Never written. Held open so the process sees end of file once the runtime is gone,
    // however it went.
    _stdin: Option<ChildStdin>,
    started: Instant,
    digest: String,
    capture: Option<(Capture, std::thread::JoinHandle<Vec<u8>>)>,
}

pub fn serve_with(
    mut input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    options: ServeOptions,
) -> io::Result<()> {
    let writer: SharedWriter = Arc::new(Mutex::new(Box::new(output)));
    let hello = match read_frame(&mut input, true)? {
        Some(Frame::Hello(hello)) => hello,
        Some(_) => return Err(io::Error::other("extension runtime expected a hello")),
        None => return Ok(()),
    };
    hello.validate()?;
    let store = match options.store_root {
        Some(root) => BundleStore::open(root)?,
        None => BundleStore::open_default()?,
    };
    store.prune(&HashSet::new());
    let runtime_dir = match options.runtime_dir {
        Some(dir) => dir,
        None => crate::control::runtime_dir()?,
    };
    let endpoint = EndpointRegistry::extension_bridge_endpoint(&runtime_dir, std::process::id());
    let bridge_path = endpoint.path().to_path_buf();
    let listener = endpoint.bind()?.into_listener();
    let _bridge_guard = RemoveOnDrop(bridge_path.clone());
    send(&writer, |out| {
        protocol::write_hello(out, &Hello::current(Some(std::env::consts::OS.to_string())))
    })?;

    let (events, inbox) = mpsc::channel();
    {
        let events = events.clone();
        std::thread::spawn(move || {
            loop {
                match read_frame(&mut input, false) {
                    Ok(Some(frame)) => {
                        if events.send(Event::Frame(frame)).is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = events.send(Event::InputClosed);
                        return;
                    }
                }
            }
        });
    }
    {
        let events = events.clone();
        let writer = writer.clone();
        listener.set_nonblocking(false)?;
        std::thread::spawn(move || accept_bridge(listener, events, writer));
    }

    let mut runtime = Runtime {
        writer,
        store,
        bridge_path,
        running: HashMap::new(),
        bridges: HashMap::new(),
        pending_stage: HashMap::new(),
    };
    loop {
        match inbox.recv_timeout(TICK) {
            Ok(Event::InputClosed) => break,
            Ok(Event::Frame(frame)) => {
                if let Err(error) = runtime.handle(frame) {
                    // A write failure means the client is gone; nothing left to serve.
                    let _ = error;
                    break;
                }
            }
            Ok(Event::BridgeOpened { conn, writer }) => {
                runtime.bridges.insert(conn, writer);
            }
            Ok(Event::BridgeEnded { conn }) => {
                runtime.bridges.remove(&conn);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if runtime.tick().is_err() {
            break;
        }
    }
    runtime.shutdown();
    Ok(())
}

struct Runtime {
    writer: SharedWriter,
    store: BundleStore,
    bridge_path: PathBuf,
    running: HashMap<u64, Running>,
    bridges: HashMap<u64, mpsc::Sender<Option<Vec<u8>>>>,
    pending_stage: HashMap<String, Vec<u8>>,
}

impl Runtime {
    fn handle(&mut self, frame: Frame) -> io::Result<()> {
        match frame {
            Frame::Hello(_) => Ok(()),
            Frame::Stage { digest, data } => {
                let pending: usize = self.pending_stage.values().map(Vec::len).sum();
                if pending + data.len() > MAX_PENDING_STAGE {
                    self.pending_stage.remove(&digest);
                    return self.message(&Message::StageFailed {
                        digest,
                        detail: "bundle exceeds the size limit".to_string(),
                    });
                }
                self.pending_stage.entry(digest).or_default().extend(data);
                Ok(())
            }
            Frame::Bridge { conn, data } => {
                if let Some(writer) = self.bridges.get(&conn) {
                    let _ = writer.send(Some(data));
                }
                Ok(())
            }
            Frame::Message(message) => self.handle_message(message),
        }
    }

    fn handle_message(&mut self, message: Message) -> io::Result<()> {
        match message {
            Message::StageCommit { digest } => {
                let archive = self.pending_stage.remove(&digest).unwrap_or_default();
                let result = Bundle::from_archive(&archive, &digest)
                    .and_then(|bundle| self.store.stage(&bundle).map_err(|e| e.to_string()));
                let keep: HashSet<String> = self
                    .running
                    .values()
                    .map(|running| running.digest.clone())
                    .chain(std::iter::once(digest.clone()))
                    .collect();
                self.store.prune(&keep);
                match result {
                    Ok(_) => self.message(&Message::Staged { digest }),
                    Err(detail) => self.message(&Message::StageFailed { digest, detail }),
                }
            }
            Message::Spawn {
                worker,
                digest,
                launch,
                cwd,
                env,
                credential,
                platforms,
                capture,
            } => {
                let reply = match self.spawn(
                    worker,
                    &digest,
                    &launch,
                    &cwd,
                    env,
                    &credential,
                    &platforms,
                    capture,
                ) {
                    Ok(pid) => Message::Spawned { worker, pid },
                    Err(failure) => Message::SpawnFailed { worker, failure },
                };
                self.message(&reply)
            }
            Message::Kill { worker } => {
                if let Some(mut running) = self.running.remove(&worker) {
                    running.group.terminate(&mut running.child);
                    let output = running.capture.take().map(|(_, reader)| collect(reader));
                    self.message(&Message::Exited {
                        worker,
                        code: None,
                        killed: true,
                        timed_out: false,
                        output,
                    })?;
                }
                Ok(())
            }
            Message::BridgeClose { conn } => {
                if let Some(writer) = self.bridges.remove(&conn) {
                    let _ = writer.send(None);
                }
                Ok(())
            }
            // Messages only the runtime sends.
            Message::Staged { .. }
            | Message::StageFailed { .. }
            | Message::Spawned { .. }
            | Message::SpawnFailed { .. }
            | Message::Exited { .. }
            | Message::BridgeOpen { .. } => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        worker: u64,
        digest: &str,
        launch: &Launch,
        cwd: &SpawnCwd,
        env: Vec<(String, String)>,
        credential: &str,
        platforms: &[String],
        capture: Option<Capture>,
    ) -> Result<u32, SpawnFailure> {
        let os = std::env::consts::OS;
        if !platforms.is_empty() && !platforms.iter().any(|platform| platform == os) {
            return Err(SpawnFailure::UnsupportedPlatform { os: os.to_string() });
        }
        // Verified now, immediately before the launch, not when it was staged.
        let bundle_dir = self.store.verified(digest).map_err(|error| match error {
            StoreError::Missing => SpawnFailure::BundleMissing,
            StoreError::Corrupt(detail) => SpawnFailure::BundleCorrupt { detail },
            StoreError::Io(detail) => SpawnFailure::SpawnFailed { detail },
        })?;
        let mut command = build_command(launch, &bundle_dir)?;
        match cwd {
            SpawnCwd::Bundle { path } => {
                if !super::bundle::is_contained(path) {
                    return Err(SpawnFailure::SpawnFailed {
                        detail: format!("working directory `{path}` is outside the extension"),
                    });
                }
                command.current_dir(bundle_dir.join(path));
            }
            SpawnCwd::Host { path } => {
                if Path::new(path).is_dir() {
                    command.current_dir(path);
                }
            }
        }
        for key in INHERITED_ENV_REMOVED {
            command.env_remove(key);
        }
        for (key, value) in env {
            if !OWNED_ENV.contains(&key.as_str()) {
                command.env(key, value);
            }
        }
        command.env("ROZI", "1");
        if let Ok(binary) = std::env::current_exe() {
            command.env("ROZI_BIN", binary);
        }
        command.env("ROZI_SOCKET", &self.bridge_path);
        command.env("ROZI_EXTENSION_DIR", &bundle_dir);
        command.env(crate::state::CREDENTIAL_ENV, credential);
        command.stdin(std::process::Stdio::piped());
        command.stdout(if capture.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        command.stderr(std::process::Stdio::null());
        configure_command_group(&mut command);
        kill_on_supervisor_death(&mut command);
        let mut child = command.spawn().map_err(|error| SpawnFailure::SpawnFailed {
            detail: error.to_string(),
        })?;
        let group = match CommandGroup::new(&child) {
            Ok(group) => group,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SpawnFailure::SpawnFailed {
                    detail: error.to_string(),
                });
            }
        };
        let capture = capture.map(|capture| {
            let mut stdout = child.stdout.take().expect("piped stdout");
            let limit = capture.max_bytes;
            let reader = std::thread::spawn(move || {
                let mut collected = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let room = limit.saturating_sub(collected.len());
                            collected.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                }
                collected
            });
            (capture, reader)
        });
        let pid = child.id();
        self.running.insert(
            worker,
            Running {
                _stdin: child.stdin.take(),
                child,
                group,
                started: Instant::now(),
                digest: digest.to_string(),
                capture,
            },
        );
        Ok(pid)
    }

    fn tick(&mut self) -> io::Result<()> {
        let mut ended = Vec::new();
        for (worker, running) in &mut self.running {
            if let Some((capture, _)) = &running.capture
                && running.started.elapsed() > Duration::from_millis(capture.timeout_ms)
            {
                running.group.terminate(&mut running.child);
                ended.push((*worker, None, true));
                continue;
            }
            match running.group.reap_if_exited(&mut running.child) {
                Ok(Some(status)) => ended.push((*worker, status.code(), false)),
                Ok(None) => {}
                Err(_) => ended.push((*worker, None, false)),
            }
        }
        for (worker, code, timed_out) in ended {
            let Some(mut running) = self.running.remove(&worker) else {
                continue;
            };
            let output = running.capture.take().map(|(_, reader)| collect(reader));
            self.message(&Message::Exited {
                worker,
                code,
                killed: false,
                timed_out,
                output,
            })?;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        for (_, mut running) in self.running.drain() {
            running.group.terminate(&mut running.child);
        }
        for (_, writer) in self.bridges.drain() {
            let _ = writer.send(None);
        }
    }

    fn message(&self, message: &Message) -> io::Result<()> {
        send(&self.writer, |out| protocol::write_message(out, message))
    }
}

fn collect(reader: std::thread::JoinHandle<Vec<u8>>) -> String {
    String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned()
}

fn build_command(
    launch: &Launch,
    bundle_dir: &Path,
) -> Result<std::process::Command, SpawnFailure> {
    match launch.template().resolve(bundle_dir) {
        crate::config::LaunchTemplate::Direct(argv) => {
            let Some((program, args)) = argv.split_first() else {
                return Err(SpawnFailure::SpawnFailed {
                    detail: "empty argv".to_string(),
                });
            };
            let path = if Path::new(program).is_absolute() {
                // Only a `./` path inside the bundle resolves to an absolute one; check it is there.
                if !Path::new(program).is_file() {
                    return Err(SpawnFailure::MissingExecutable {
                        program: program.clone(),
                    });
                }
                PathBuf::from(program)
            } else {
                locate(program).ok_or_else(|| SpawnFailure::MissingExecutable {
                    program: program.clone(),
                })?
            };
            let mut command = std::process::Command::new(path);
            command.args(args);
            Ok(command)
        }
        crate::config::LaunchTemplate::Shell(line) => {
            let runner = crate::platform::command::resolve_command_shell(
                None,
                &crate::platform::command::ShellEnv::from_process(),
            );
            if !crate::platform::command::program_exists(&runner.program) {
                return Err(SpawnFailure::MissingExecutable {
                    program: runner.program,
                });
            }
            let mut command = std::process::Command::new(&runner.program);
            command.args(&runner.args);
            command.arg(line);
            Ok(command)
        }
    }
}

/// A bare program name on this host's `PATH`, or `None` when it is not installed.
#[cfg(windows)]
fn locate(program: &str) -> Option<PathBuf> {
    crate::platform::command::lookup_program(program)
}

#[cfg(not(windows))]
fn locate(program: &str) -> Option<PathBuf> {
    crate::platform::command::program_exists(program).then(|| PathBuf::from(program))
}

fn send(
    writer: &SharedWriter,
    write: impl FnOnce(&mut Box<dyn Write + Send>) -> io::Result<()>,
) -> io::Result<()> {
    let mut guard = writer
        .lock()
        .map_err(|_| io::Error::other("writer poisoned"))?;
    write(&mut guard)
}

/// Accept bridge connections for the life of the runtime, relaying each one's bytes to the client.
fn accept_bridge(
    listener: crate::platform::ipc::IpcListener,
    events: mpsc::Sender<Event>,
    writer: SharedWriter,
) {
    let mut next = 0u64;
    loop {
        let connection = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        next += 1;
        let conn = next;
        let Ok(reader) = connection.try_clone() else {
            continue;
        };
        let (to_process, outbound) = mpsc::channel::<Option<Vec<u8>>>();
        if events
            .send(Event::BridgeOpened {
                conn,
                writer: to_process,
            })
            .is_err()
        {
            return;
        }
        if send(&writer, |out| {
            protocol::write_message(out, &Message::BridgeOpen { conn })
        })
        .is_err()
        {
            return;
        }
        std::thread::spawn(move || relay_to_process(connection, outbound));
        let writer = writer.clone();
        let events = events.clone();
        std::thread::spawn(move || relay_to_client(reader, conn, writer, events));
    }
}

fn relay_to_process(mut connection: IpcConnection, outbound: mpsc::Receiver<Option<Vec<u8>>>) {
    while let Ok(Some(data)) = outbound.recv() {
        if connection
            .write_all(&data)
            .and_then(|()| connection.flush())
            .is_err()
        {
            break;
        }
    }
    let _ = connection.shutdown(std::net::Shutdown::Write);
}

fn relay_to_client(
    mut reader: IpcConnection,
    conn: u64,
    writer: SharedWriter,
    events: mpsc::Sender<Event>,
) {
    let _ = reader.set_read_timeout(None);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if send(&writer, |out| {
                    protocol::write_bridge(out, conn, &chunk[..n])
                })
                .is_err()
                {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    let _ = send(&writer, |out| {
        protocol::write_message(out, &Message::BridgeClose { conn })
    });
    let _ = events.send(Event::BridgeEnded { conn });
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        crate::platform::ipc::IpcEndpoint::at_path(&self.0).remove_stale();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::extension_runtime::bundle::BundleFile;
    use crate::extension_runtime::protocol::{write_hello, write_message, write_stage};

    struct Client {
        to_runtime: std::io::PipeWriter,
        from_runtime: std::io::PipeReader,
        server: Option<std::thread::JoinHandle<io::Result<()>>>,
        _dirs: (tempfile::TempDir, tempfile::TempDir),
        runtime_dir: PathBuf,
    }

    impl Client {
        fn start() -> Self {
            let store = tempfile::tempdir().unwrap();
            // A socket path has to fit in `sun_path`, so not under a deep temp directory.
            let run = tempfile::Builder::new()
                .prefix("rozi-rt")
                .tempdir_in("/tmp")
                .unwrap();
            let runtime_dir = run.path().to_path_buf();
            let (from_client, to_runtime) = std::io::pipe().unwrap();
            let (from_runtime, to_client) = std::io::pipe().unwrap();
            let options = ServeOptions {
                store_root: Some(store.path().join("bundles")),
                runtime_dir: Some(runtime_dir.clone()),
            };
            let server = std::thread::spawn(move || serve_with(from_client, to_client, options));
            let mut client = Self {
                to_runtime,
                from_runtime,
                server: Some(server),
                _dirs: (store, run),
                runtime_dir,
            };
            write_hello(&mut client.to_runtime, &Hello::current(None)).unwrap();
            let Some(Frame::Hello(hello)) = read_frame(&mut client.from_runtime, true).unwrap()
            else {
                panic!("runtime answers with a hello");
            };
            hello.validate().unwrap();
            assert_eq!(hello.os.as_deref(), Some(std::env::consts::OS));
            client
        }

        fn send(&mut self, message: Message) {
            write_message(&mut self.to_runtime, &message).unwrap();
        }

        fn next(&mut self) -> Frame {
            read_frame(&mut self.from_runtime, false)
                .unwrap()
                .expect("runtime still serving")
        }

        fn next_message(&mut self) -> Message {
            loop {
                if let Frame::Message(message) = self.next() {
                    return message;
                }
            }
        }

        fn stage(&mut self, bundle: &Bundle) {
            for chunk in bundle.archive().chunks(protocol::STAGE_CHUNK) {
                write_stage(&mut self.to_runtime, bundle.digest(), chunk).unwrap();
            }
            self.send(Message::StageCommit {
                digest: bundle.digest().to_string(),
            });
            assert_eq!(
                self.next_message(),
                Message::Staged {
                    digest: bundle.digest().to_string()
                }
            );
        }

        fn spawn(&mut self, worker: u64, bundle: &Bundle, launch: Launch, capture: bool) {
            self.send(Message::Spawn {
                worker,
                digest: bundle.digest().to_string(),
                launch,
                cwd: SpawnCwd::Bundle {
                    path: ".".to_string(),
                },
                env: vec![
                    ("ROZI_EXTENSION".to_string(), "probe".to_string()),
                    (
                        "ROZI_SOCKET".to_string(),
                        "/client/path/must/not/leak".to_string(),
                    ),
                ],
                credential: "c".repeat(64),
                platforms: Vec::new(),
                capture: capture.then_some(Capture {
                    max_bytes: 4096,
                    timeout_ms: 10_000,
                }),
            });
        }

        /// Close the channel and wait for the runtime to finish. Returns its directories, which
        /// outlive it so a test can look at what it left behind.
        fn close(mut self) -> (tempfile::TempDir, tempfile::TempDir) {
            drop(self.to_runtime);
            self.server.take().unwrap().join().unwrap().unwrap();
            self._dirs
        }
    }

    fn script(body: &str) -> Bundle {
        Bundle::from_files(vec![BundleFile {
            path: "bin/probe".to_string(),
            executable: true,
            contents: format!("#!/bin/sh\n{body}\n").into_bytes(),
        }])
        .unwrap()
    }

    fn probe() -> Launch {
        Launch::Direct {
            argv: vec!["./bin/probe".to_string()],
        }
    }

    #[test]
    fn a_process_runs_from_its_verified_bundle_with_the_hosts_own_endpoints() {
        let mut client = Client::start();
        let bundle = script(
            "printf '%s\\n' \"$ROZI\" \"$ROZI_EXTENSION\" \"$ROZI_EXTENSION_DIR\" \"$PWD\" \
             \"$ROZI_SOCKET\" \"$ROZI_EXTENSION_CREDENTIAL\" \"$ROZI_BIN\"",
        );
        client.stage(&bundle);
        client.spawn(1, &bundle, probe(), true);
        assert!(matches!(
            client.next_message(),
            Message::Spawned { worker: 1, .. }
        ));
        let Message::Exited {
            worker: 1,
            code: Some(0),
            output: Some(output),
            ..
        } = client.next_message()
        else {
            panic!("probe exits cleanly");
        };
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines[0], "1");
        assert_eq!(lines[1], "probe");
        assert!(lines[2].ends_with(bundle.digest()), "{}", lines[2]);
        assert_eq!(
            std::fs::canonicalize(lines[3]).unwrap(),
            std::fs::canonicalize(lines[2]).unwrap()
        );
        // The client's endpoint means nothing here: the runtime's bridge replaces it.
        assert!(
            lines[4].starts_with(client.runtime_dir.to_str().unwrap())
                && lines[4].contains("extension-bridge-"),
            "{}",
            lines[4]
        );
        assert_eq!(lines[5], "c".repeat(64));
        assert!(!lines[6].is_empty());
        client.close();
    }

    #[test]
    fn unavailable_contributions_are_reported_with_their_reason() {
        let mut client = Client::start();
        let bundle = script("exit 0");
        // Not staged yet.
        client.spawn(1, &bundle, probe(), false);
        assert_eq!(
            client.next_message(),
            Message::SpawnFailed {
                worker: 1,
                failure: SpawnFailure::BundleMissing
            }
        );
        client.stage(&bundle);
        client.spawn(
            2,
            &bundle,
            Launch::Direct {
                argv: vec!["definitely-not-installed-rozi-probe".to_string()],
            },
            false,
        );
        assert_eq!(
            client.next_message(),
            Message::SpawnFailed {
                worker: 2,
                failure: SpawnFailure::MissingExecutable {
                    program: "definitely-not-installed-rozi-probe".to_string()
                }
            }
        );
        client.send(Message::Spawn {
            worker: 3,
            digest: bundle.digest().to_string(),
            launch: probe(),
            cwd: SpawnCwd::Bundle {
                path: ".".to_string(),
            },
            env: Vec::new(),
            credential: "c".repeat(64),
            platforms: vec!["plan9".to_string()],
            capture: None,
        });
        assert_eq!(
            client.next_message(),
            Message::SpawnFailed {
                worker: 3,
                failure: SpawnFailure::UnsupportedPlatform {
                    os: std::env::consts::OS.to_string()
                }
            }
        );
        client.close();
    }

    #[test]
    fn an_archive_that_does_not_match_its_digest_is_refused() {
        let mut client = Client::start();
        let bundle = script("exit 0");
        let mut archive = bundle.archive();
        let last = archive.len() - 1;
        archive[last] ^= 1;
        write_stage(&mut client.to_runtime, bundle.digest(), &archive).unwrap();
        client.send(Message::StageCommit {
            digest: bundle.digest().to_string(),
        });
        assert!(matches!(client.next_message(), Message::StageFailed { .. }));
        client.close();
    }

    fn alive(pid: u32) -> bool {
        crate::platform::command::process_is_alive(pid)
    }

    fn wait_dead(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid) {
            assert!(
                Instant::now() < deadline,
                "process {pid} outlived its runtime"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_killed_process_takes_its_children_with_it() {
        let mut client = Client::start();
        let bundle = script("sleep 300 & echo $! > \"$1\"; wait");
        client.stage(&bundle);
        let out = tempfile::tempdir().unwrap();
        let pidfile = out.path().join("child");
        client.spawn(
            7,
            &bundle,
            Launch::Direct {
                argv: vec!["./bin/probe".to_string(), pidfile.display().to_string()],
            },
            false,
        );
        let Message::Spawned { pid, .. } = client.next_message() else {
            panic!("spawned");
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let grandchild = loop {
            if let Ok(text) = std::fs::read_to_string(&pidfile)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        };
        client.send(Message::Kill { worker: 7 });
        assert!(matches!(
            client.next_message(),
            Message::Exited {
                worker: 7,
                killed: true,
                ..
            }
        ));
        wait_dead(pid);
        wait_dead(grandchild);
        client.close();
    }

    /// The runtime belongs to its client's channel. When the channel closes - detach, exit, or a
    /// dropped network - nothing it started survives it.
    #[test]
    fn closing_the_channel_kills_every_process_and_removes_the_bridge() {
        let mut client = Client::start();
        let bundle = script("exec sleep 300");
        client.stage(&bundle);
        client.spawn(1, &bundle, probe(), false);
        let Message::Spawned { pid, .. } = client.next_message() else {
            panic!("spawned");
        };
        assert!(alive(pid));
        let runtime_dir = client.runtime_dir.clone();
        let bridge_present = || {
            std::fs::read_dir(&runtime_dir)
                .unwrap()
                .flatten()
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("extension-bridge-")
                })
        };
        assert!(bridge_present());
        let _dirs = client.close();
        wait_dead(pid);
        assert!(!bridge_present());
    }

    /// The bridge relays bytes and nothing else, both ways, per connection.
    #[test]
    fn bridge_connections_are_relayed_to_the_client_and_back() {
        let mut client = Client::start();
        let bridge = std::fs::read_dir(&client.runtime_dir)
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("extension-bridge-")
            })
            .unwrap()
            .path();
        let mut process = crate::platform::ipc::IpcEndpoint::at_path(&bridge)
            .connect()
            .unwrap();
        process
            .write_all(b"{\"command\":\"list-panes\"}\n")
            .unwrap();
        let Message::BridgeOpen { conn } = client.next_message() else {
            panic!("open first");
        };
        let mut received = Vec::new();
        while received.len() < 25 {
            match client.next() {
                Frame::Bridge { conn: from, data } if from == conn => received.extend(data),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(received, b"{\"command\":\"list-panes\"}\n");
        protocol::write_bridge(&mut client.to_runtime, conn, b"{\"ok\":true}\n").unwrap();
        client.send(Message::BridgeClose { conn });
        let mut reply = String::new();
        process.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "{\"ok\":true}\n");
        drop(process);
        assert_eq!(client.next_message(), Message::BridgeClose { conn });
        client.close();
    }
}
