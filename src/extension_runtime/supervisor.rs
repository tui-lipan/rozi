//! UI-free process lifecycle and service supervision for extension runtimes and session servers.
//!
//! Spawns extension and service processes inside a [`CommandGroup`], sanitizes their environment,
//! routes output into [`super::log`], tracks exit statuses and capture deadlines, and applies the
//! service restart and backoff ladder without depending on any UI or `tui-lipan` types.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::log::{self, Attached, Log};
use super::protocol::{Capture, Launch, SpawnCwd, SpawnFailure};
use crate::config::{LaunchTemplate, ServiceRestart};
use crate::platform::command::{
    CommandGroup, ShellEnv, configure_command_group, kill_on_supervisor_death, program_exists,
    resolve_command_shell, spawn_retrying_on_busy,
};

/// Initial delay before restarting a failed or exited service.
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum delay between service restart attempts.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Continuous uptime after which a service's failure count and backoff reset.
pub const UPTIME_RESET_SECS: u64 = 60;

/// Consecutive failures before a restarting service gives up and becomes dormant/stopped.
pub const MAX_FAILURES: u32 = 5;

/// Default poll interval for checking running processes and pending service restarts.
pub const DEFAULT_SUPERVISOR_TICK: Duration = Duration::from_millis(50);

/// Environment the supervisor sets for every extension process it starts, and never takes from the
/// caller.
pub const OWNED_ENV: &[&str] = &[
    "ROZI",
    "ROZI_BIN",
    "ROZI_SOCKET",
    "ROZI_EXTENSION_DIR",
    crate::config::STATE_DIR_ENV,
    crate::state::CREDENTIAL_ENV,
];

/// Environment a process must not inherit from the supervisor itself: the supervisor may have been
/// started from inside a pane, and those values describe that pane, not this process.
pub const INHERITED_ENV_REMOVED: &[&str] = &[
    "ROZI_PANE",
    "ROZI_SESSION_INSTANCE",
    "ROZI_EXTENSION",
    "ROZI_EXTENSION_GENERATION",
    "ROZI_EXTENSION_CONFIG",
    crate::config::STATE_DIR_ENV,
    crate::config::SESSION_NAME_ENV,
    "ROZI_SERVICE",
];

/// The delay after `current`. Every restart path steps the ladder through here so a change to the
/// curve cannot apply to crashes but miss clean exits, or apply to exits but miss failed spawns.
pub fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// Parameters governing service restart backoff and dormancy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub initial: Duration,
    pub max: Duration,
    pub uptime_reset: Duration,
    pub max_failures: u32,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: INITIAL_BACKOFF,
            max: MAX_BACKOFF,
            uptime_reset: Duration::from_secs(UPTIME_RESET_SECS),
            max_failures: MAX_FAILURES,
        }
    }
}

impl BackoffPolicy {
    pub fn next_delay(&self, current: Duration) -> Duration {
        (current * 2).min(self.max)
    }

    /// Evaluate what to do when a service fails to spawn on attempt `failures` (1-based).
    pub fn evaluate_spawn_failure(
        &self,
        restart: ServiceRestart,
        failures: u32,
        backoff: Duration,
        now: Instant,
    ) -> RestartOutcome {
        if restart == ServiceRestart::Never {
            return RestartOutcome::Stop {
                reason: StopReason::NeverRestart,
                failures,
            };
        }
        if failures >= self.max_failures {
            return RestartOutcome::Stop {
                reason: StopReason::ExhaustedBackoff,
                failures,
            };
        }
        RestartOutcome::Restart {
            restart_at: now + backoff,
            delay: backoff,
            next_backoff: self.next_delay(backoff),
            failures,
        }
    }

    /// Evaluate what to do when a running service exits.
    pub fn evaluate_exit(
        &self,
        restart: ServiceRestart,
        is_success: bool,
        started_at: Option<Instant>,
        mut failures: u32,
        mut backoff: Duration,
        now: Instant,
    ) -> RestartOutcome {
        if started_at.is_some_and(|at| now.saturating_duration_since(at) >= self.uptime_reset) {
            failures = 0;
            backoff = self.initial;
        }
        match restart {
            ServiceRestart::Never => RestartOutcome::Stop {
                reason: StopReason::NeverRestart,
                failures,
            },
            ServiceRestart::OnFailure => {
                if is_success {
                    RestartOutcome::Stop {
                        reason: StopReason::NormalExit,
                        failures,
                    }
                } else {
                    failures += 1;
                    if failures >= self.max_failures {
                        RestartOutcome::Stop {
                            reason: StopReason::ExhaustedBackoff,
                            failures,
                        }
                    } else {
                        RestartOutcome::Restart {
                            restart_at: now + backoff,
                            delay: backoff,
                            next_backoff: self.next_delay(backoff),
                            failures,
                        }
                    }
                }
            }
            ServiceRestart::Always => {
                if is_success {
                    // A clean exit is not a failure, so it never counts toward dormancy - but it
                    // still steps the ladder so a service that exits 0 immediately does not
                    // respawn in a tight loop.
                    RestartOutcome::Restart {
                        restart_at: now + backoff,
                        delay: backoff,
                        next_backoff: self.next_delay(backoff),
                        failures,
                    }
                } else {
                    failures += 1;
                    if failures >= self.max_failures {
                        RestartOutcome::Stop {
                            reason: StopReason::ExhaustedBackoff,
                            failures,
                        }
                    } else {
                        RestartOutcome::Restart {
                            restart_at: now + backoff,
                            delay: backoff,
                            next_backoff: self.next_delay(backoff),
                            failures,
                        }
                    }
                }
            }
        }
    }
}

/// Why a supervised service stopped instead of scheduling another restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    NeverRestart,
    NormalExit,
    ExhaustedBackoff,
}

/// Decision returned by the restart policy after a spawn failure or process exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartOutcome {
    Restart {
        restart_at: Instant,
        delay: Duration,
        next_backoff: Duration,
        failures: u32,
    },
    Stop {
        reason: StopReason,
        failures: u32,
    },
}

/// Evaluate a service spawn failure against the default [`BackoffPolicy`].
pub fn evaluate_spawn_failure(
    restart: ServiceRestart,
    failures: u32,
    backoff: Duration,
    now: Instant,
) -> RestartOutcome {
    BackoffPolicy::default().evaluate_spawn_failure(restart, failures, backoff, now)
}

/// Evaluate a service exit against the default [`BackoffPolicy`].
pub fn evaluate_exit(
    restart: ServiceRestart,
    is_success: bool,
    started_at: Option<Instant>,
    failures: u32,
    backoff: Duration,
    now: Instant,
) -> RestartOutcome {
    BackoffPolicy::default().evaluate_exit(restart, is_success, started_at, failures, backoff, now)
}

fn verify_process_bundle(directory: &Path) -> Result<(), SpawnFailure> {
    use super::store::{BundleStore, StoreError};
    let digest = directory
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| super::bundle::is_digest(name))
        .ok_or(SpawnFailure::BundleMissing)?;
    let parent = directory.parent().ok_or(SpawnFailure::BundleMissing)?;
    let store =
        BundleStore::open(parent.to_path_buf()).map_err(|error| SpawnFailure::SpawnFailed {
            detail: error.to_string(),
        })?;
    store
        .verified(digest)
        .map(|_| ())
        .map_err(|error| match error {
            StoreError::Missing => SpawnFailure::BundleMissing,
            StoreError::Corrupt(detail) => SpawnFailure::BundleCorrupt { detail },
            StoreError::Io(detail) => SpawnFailure::SpawnFailed { detail },
        })
}

/// Launch specification for one supervised extension process.
#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub bundle_dir: PathBuf,
    pub socket_path: PathBuf,
    pub launch: Launch,
    pub cwd: SpawnCwd,
    pub env: Vec<(String, String)>,
    pub credential: String,
    pub platforms: Vec<String>,
    pub capture: Option<Capture>,
    pub unit: Option<String>,
}

/// Launch and restart specification for a long-running supervised service.
#[derive(Clone, Debug)]
pub struct ServiceSpec {
    pub process: ProcessSpec,
    pub restart: ServiceRestart,
}

/// How a supervised process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitOutcome {
    pub code: Option<i32>,
    pub killed: bool,
    pub timed_out: bool,
    pub output: Option<String>,
}

impl ExitOutcome {
    pub fn is_success(&self) -> bool {
        !self.killed && !self.timed_out && self.code == Some(0)
    }

    pub fn describe(&self) -> String {
        if self.timed_out {
            "stopped: timed out".to_string()
        } else if let Some(code) = self.code {
            format!("exited with status {code}")
        } else {
            "was terminated".to_string()
        }
    }

    fn log_note(&self) -> String {
        match (self.code, self.timed_out) {
            (_, true) => "stopped: timed out".to_string(),
            (Some(code), false) => format!("exited with status {code}"),
            (None, false) => "was terminated by a signal".to_string(),
        }
    }
}

/// Configure a [`Command`] with its process group, supervisor-death signal, and [`Log`] routing,
/// then spawn it and attach the log reader thread.
pub fn spawn_with_log(
    command: &mut Command,
    log: Option<&Log>,
    capture_stdout: bool,
) -> io::Result<(Child, CommandGroup, Option<Attached>)> {
    if capture_stdout {
        command.stdout(Stdio::piped());
        Log::route_stderr(command, log);
    } else {
        Log::route(command, log);
    }
    configure_command_group(command);
    kill_on_supervisor_death(command);
    let mut child = match spawn_retrying_on_busy(command) {
        Ok(child) => child,
        Err(error) => {
            if let Some(log) = log {
                log.note(&format!("could not start: {error}"));
            }
            return Err(error);
        }
    };
    let group = match CommandGroup::new(&child) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let attached = log.map(|log| {
        log.note(&format!("started pid {}", child.id()));
        if capture_stdout {
            log.attach_stderr(&mut child)
        } else {
            log.attach(&mut child)
        }
    });
    Ok((child, group, attached))
}

/// Terminate a child's process group and record the final `note` in its attached log.
pub fn terminate_with_log(
    child: &mut Child,
    group: &CommandGroup,
    log: Option<&Attached>,
    note: &str,
) {
    group.terminate(child);
    if let Some(log) = log {
        log.finish(note);
    }
}

/// One live process managed inside a [`CommandGroup`] with optional stdout capture and log
/// routing.
pub struct SupervisedProcess {
    child: Child,
    group: CommandGroup,
    // Never written. Held open so the process sees end of file once the supervisor is gone,
    // however it went.
    _stdin: Option<ChildStdin>,
    started: Instant,
    capture: Option<(Capture, std::thread::JoinHandle<Vec<u8>>)>,
    log: Option<Attached>,
}

impl SupervisedProcess {
    /// Validate `spec`, prepare the extension's state directory, and spawn the child process in
    /// its own [`CommandGroup`].
    pub fn spawn(spec: ProcessSpec) -> Result<Self, SpawnFailure> {
        let os = std::env::consts::OS;
        if !spec.platforms.is_empty() && !spec.platforms.iter().any(|platform| platform == os) {
            return Err(SpawnFailure::UnsupportedPlatform { os: os.to_string() });
        }
        // Verified now, immediately before the launch, not when it was staged.
        verify_process_bundle(&spec.bundle_dir)?;
        let extension = spec
            .env
            .iter()
            .find(|(key, _)| key == "ROZI_EXTENSION")
            .map(|(_, id)| id.clone())
            .ok_or_else(|| SpawnFailure::SpawnFailed {
                detail: "the launch names no extension".to_string(),
            })?;
        let state_dir = crate::platform::paths::ensure_extension_state_dir(
            &crate::platform::paths::PlatformEnv::from_process(),
            &extension,
        )
        .map_err(|error| SpawnFailure::SpawnFailed {
            detail: format!("cannot prepare the extension's state directory: {error}"),
        })?;
        let mut command = build_command(&spec.launch, &spec.bundle_dir)?;
        match &spec.cwd {
            SpawnCwd::Bundle { path } => {
                if !super::bundle::is_contained(path) {
                    return Err(SpawnFailure::SpawnFailed {
                        detail: format!("working directory `{path}` is outside the extension"),
                    });
                }
                command.current_dir(spec.bundle_dir.join(path));
            }
            SpawnCwd::Host { path } => {
                if Path::new(path).is_dir() {
                    command.current_dir(path);
                }
            }
            SpawnCwd::Inherit => {}
        }
        for key in INHERITED_ENV_REMOVED {
            command.env_remove(key);
        }
        for (key, value) in spec.env {
            if !OWNED_ENV.contains(&key.as_str()) {
                command.env(key, value);
            }
        }
        command.env("ROZI", "1");
        if let Ok(binary) = std::env::current_exe() {
            command.env("ROZI_BIN", binary);
        }
        command.env("ROZI_SOCKET", &spec.socket_path);
        command.env("ROZI_EXTENSION_DIR", &spec.bundle_dir);
        command.env(crate::config::STATE_DIR_ENV, &state_dir);
        command.env(crate::state::CREDENTIAL_ENV, &spec.credential);
        command.stdin(Stdio::piped());

        let log = log::open(&extension, spec.unit.as_deref().unwrap_or("placed"));
        let (mut child, group, log) =
            spawn_with_log(&mut command, log.as_ref(), spec.capture.is_some()).map_err(
                |error| SpawnFailure::SpawnFailed {
                    detail: error.to_string(),
                },
            )?;
        let capture = spec.capture.map(|capture| {
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
        Ok(Self {
            _stdin: child.stdin.take(),
            child,
            group,
            started: Instant::now(),
            capture,
            log,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn started_at(&self) -> Instant {
        self.started
    }

    /// Check whether the process has exited or exceeded its capture timeout. When it has, reaps or
    /// terminates its [`CommandGroup`], finishes its log, and returns the [`ExitOutcome`].
    pub fn poll(&mut self) -> Option<ExitOutcome> {
        let (code, timed_out) = if let Some((capture, _)) = &self.capture
            && self.started.elapsed() > Duration::from_millis(capture.timeout_ms)
        {
            self.group.terminate(&mut self.child);
            (None, true)
        } else {
            match self.group.reap_if_exited(&mut self.child) {
                Ok(Some(status)) => (status.code(), false),
                Ok(None) => return None,
                Err(_) => (None, false),
            }
        };
        let output = self.capture.take().map(|(_, reader)| collect(reader));
        let outcome = ExitOutcome {
            code,
            killed: false,
            timed_out,
            output,
        };
        if let Some(log) = &self.log {
            log.finish(&outcome.log_note());
        }
        Some(outcome)
    }

    /// Terminate the process and its descendants immediately, recording `note` in its log.
    pub fn kill(mut self, note: &str) -> ExitOutcome {
        terminate_with_log(&mut self.child, &self.group, self.log.as_ref(), note);
        let output = self.capture.take().map(|(_, reader)| collect(reader));
        ExitOutcome {
            code: None,
            killed: true,
            timed_out: false,
            output,
        }
    }
}

fn collect(reader: std::thread::JoinHandle<Vec<u8>>) -> String {
    String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned()
}

pub fn build_command(launch: &Launch, bundle_dir: &Path) -> Result<Command, SpawnFailure> {
    match launch.template().resolve(bundle_dir) {
        LaunchTemplate::Direct(argv) => {
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
            let mut command = Command::new(path);
            command.args(args);
            Ok(command)
        }
        LaunchTemplate::Shell(line) => {
            let runner = resolve_command_shell(None, &ShellEnv::from_process());
            if !program_exists(&runner.program) {
                return Err(SpawnFailure::MissingExecutable {
                    program: runner.program,
                });
            }
            let mut command = Command::new(&runner.program);
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
    program_exists(program).then(|| PathBuf::from(program))
}

/// Lifecycle events emitted by [`Supervisor`] to a non-UI or UI host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorEvent {
    WorkerSpawned {
        worker: u64,
        pid: u32,
    },
    WorkerSpawnFailed {
        worker: u64,
        failure: SpawnFailure,
    },
    WorkerExited {
        worker: u64,
        outcome: ExitOutcome,
    },
    ServiceSpawned {
        name: String,
        pid: u32,
    },
    ServiceRestartScheduled {
        name: String,
        delay: Duration,
        failures: u32,
        detail: String,
    },
    ServiceStopped {
        name: String,
        reason: StopReason,
        failures: u32,
        detail: String,
    },
}

enum CommandMsg {
    SpawnWorker { worker: u64, spec: ProcessSpec },
    KillWorker { worker: u64 },
    StartService { name: String, spec: ServiceSpec },
    StopService { name: String },
    Shutdown,
}

struct ManagedService {
    spec: ServiceSpec,
    state: ManagedServiceState,
    failures: u32,
    backoff: Duration,
}

enum ManagedServiceState {
    Running(SupervisedProcess),
    PendingRestart { restart_at: Instant },
}

/// A background process and service supervisor that runs on its own thread and reports lifecycle
/// events via a callback without any UI or `tui-lipan` dependencies.
pub struct Supervisor {
    tx: Mutex<Option<mpsc::Sender<CommandMsg>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Supervisor {
    /// Start a background supervisor thread with the default [`BackoffPolicy`] and poll interval.
    pub fn start(report: impl Fn(SupervisorEvent) + Send + 'static) -> Self {
        Self::start_with_policy(BackoffPolicy::default(), DEFAULT_SUPERVISOR_TICK, report)
    }

    /// Start a background supervisor thread with custom [`BackoffPolicy`] and `tick` interval.
    pub fn start_with_policy(
        policy: BackoffPolicy,
        tick: Duration,
        report: impl Fn(SupervisorEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("rozi-extension-supervisor".to_string())
            .spawn(move || run_supervisor_loop(rx, policy, tick, report))
            .expect("spawn extension supervisor thread");
        Self {
            tx: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(handle)),
        }
    }

    /// Spawn a one-shot worker process.
    pub fn spawn_worker(&self, worker: u64, spec: ProcessSpec) -> io::Result<()> {
        self.send(CommandMsg::SpawnWorker { worker, spec })
    }

    /// Kill a running one-shot worker process and its process group.
    pub fn kill_worker(&self, worker: u64) -> io::Result<()> {
        self.send(CommandMsg::KillWorker { worker })
    }

    /// Start or replace a supervised service.
    pub fn start_service(&self, name: impl Into<String>, spec: ServiceSpec) -> io::Result<()> {
        self.send(CommandMsg::StartService {
            name: name.into(),
            spec,
        })
    }

    /// Stop a supervised service, killing its process group if running.
    pub fn stop_service(&self, name: impl Into<String>) -> io::Result<()> {
        self.send(CommandMsg::StopService { name: name.into() })
    }

    /// Stop every managed worker and service and wait for the supervisor thread to exit.
    pub fn shutdown(&self) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(CommandMsg::Shutdown);
        }
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    fn send(&self, msg: CommandMsg) -> io::Result<()> {
        let guard = self
            .tx
            .lock()
            .map_err(|_| io::Error::other("supervisor poisoned"))?;
        let Some(tx) = guard.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "supervisor has shut down",
            ));
        };
        tx.send(msg).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "supervisor thread disconnected")
        })
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_supervisor_loop(
    rx: mpsc::Receiver<CommandMsg>,
    policy: BackoffPolicy,
    tick: Duration,
    report: impl Fn(SupervisorEvent),
) {
    let mut workers: HashMap<u64, SupervisedProcess> = HashMap::new();
    let mut services: HashMap<String, ManagedService> = HashMap::new();

    loop {
        match rx.recv_timeout(tick) {
            Ok(CommandMsg::SpawnWorker { worker, spec }) => {
                if let Some(previous) = workers.remove(&worker) {
                    let _ = previous.kill("stopped by rozi");
                }
                match SupervisedProcess::spawn(spec) {
                    Ok(process) => {
                        let pid = process.pid();
                        workers.insert(worker, process);
                        report(SupervisorEvent::WorkerSpawned { worker, pid });
                    }
                    Err(failure) => {
                        report(SupervisorEvent::WorkerSpawnFailed { worker, failure });
                    }
                }
            }
            Ok(CommandMsg::KillWorker { worker }) => {
                if let Some(process) = workers.remove(&worker) {
                    let outcome = process.kill("stopped by rozi");
                    report(SupervisorEvent::WorkerExited { worker, outcome });
                }
            }
            Ok(CommandMsg::StartService { name, spec }) => {
                if let Some(previous) = services.remove(&name)
                    && let ManagedServiceState::Running(process) = previous.state
                {
                    let _ = process.kill("stopped by rozi");
                }
                launch_managed_service(
                    &mut services,
                    name,
                    spec,
                    0,
                    policy.initial,
                    &policy,
                    &report,
                );
            }
            Ok(CommandMsg::StopService { name }) => {
                if let Some(service) = services.remove(&name)
                    && let ManagedServiceState::Running(process) = service.state
                {
                    let _ = process.kill("stopped by rozi");
                }
            }
            Ok(CommandMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        // 1. Poll one-shot workers.
        let mut exited_workers = Vec::new();
        for (worker, process) in &mut workers {
            if let Some(outcome) = process.poll() {
                exited_workers.push((*worker, outcome));
            }
        }
        for (worker, outcome) in exited_workers {
            workers.remove(&worker);
            report(SupervisorEvent::WorkerExited { worker, outcome });
        }

        // 2. Poll running services and handle exits.
        let mut exited_services = Vec::new();
        for (name, service) in &mut services {
            if let ManagedServiceState::Running(process) = &mut service.state
                && let Some(outcome) = process.poll()
            {
                exited_services.push((name.clone(), process.started_at(), outcome));
            }
        }
        let now = Instant::now();
        for (name, started_at, outcome) in exited_services {
            let Some(mut service) = services.remove(&name) else {
                continue;
            };
            let detail = outcome.describe();
            match policy.evaluate_exit(
                service.spec.restart,
                outcome.is_success(),
                Some(started_at),
                service.failures,
                service.backoff,
                now,
            ) {
                RestartOutcome::Restart {
                    restart_at,
                    delay,
                    next_backoff,
                    failures,
                } => {
                    service.failures = failures;
                    service.backoff = next_backoff;
                    service.state = ManagedServiceState::PendingRestart { restart_at };
                    services.insert(name.clone(), service);
                    report(SupervisorEvent::ServiceRestartScheduled {
                        name,
                        delay,
                        failures,
                        detail,
                    });
                }
                RestartOutcome::Stop { reason, failures } => {
                    report(SupervisorEvent::ServiceStopped {
                        name,
                        reason,
                        failures,
                        detail,
                    });
                }
            }
        }

        // 3. Respawn services whose restart backoff has elapsed.
        let now = Instant::now();
        let due: Vec<String> = services
            .iter()
            .filter_map(|(name, service)| match service.state {
                ManagedServiceState::PendingRestart { restart_at } if now >= restart_at => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        for name in due {
            if let Some(service) = services.remove(&name) {
                launch_managed_service(
                    &mut services,
                    name,
                    service.spec,
                    service.failures,
                    service.backoff,
                    &policy,
                    &report,
                );
            }
        }
    }

    for (_, process) in workers.drain() {
        let _ = process.kill("stopped: the runtime shut down");
    }
    for (_, service) in services.drain() {
        if let ManagedServiceState::Running(process) = service.state {
            let _ = process.kill("stopped: the runtime shut down");
        }
    }
}

fn launch_managed_service(
    services: &mut HashMap<String, ManagedService>,
    name: String,
    spec: ServiceSpec,
    failures: u32,
    backoff: Duration,
    policy: &BackoffPolicy,
    report: &impl Fn(SupervisorEvent),
) {
    match SupervisedProcess::spawn(spec.process.clone()) {
        Ok(process) => {
            let pid = process.pid();
            services.insert(
                name.clone(),
                ManagedService {
                    spec,
                    state: ManagedServiceState::Running(process),
                    failures,
                    backoff,
                },
            );
            report(SupervisorEvent::ServiceSpawned { name, pid });
        }
        Err(failure) => {
            let detail = failure.to_string();
            match policy.evaluate_spawn_failure(spec.restart, failures + 1, backoff, Instant::now())
            {
                RestartOutcome::Restart {
                    restart_at,
                    delay,
                    next_backoff,
                    failures,
                } => {
                    services.insert(
                        name.clone(),
                        ManagedService {
                            spec,
                            state: ManagedServiceState::PendingRestart { restart_at },
                            failures,
                            backoff: next_backoff,
                        },
                    );
                    report(SupervisorEvent::ServiceRestartScheduled {
                        name,
                        delay,
                        failures,
                        detail,
                    });
                }
                RestartOutcome::Stop { reason, failures } => {
                    report(SupervisorEvent::ServiceStopped {
                        name,
                        reason,
                        failures,
                        detail,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::extension_runtime::bundle::{Bundle, BundleFile};
    #[cfg(unix)]
    use crate::extension_runtime::store::BundleStore;

    #[cfg(unix)]
    fn stage_script(store: &BundleStore, body: &str) -> PathBuf {
        let bundle = Bundle::from_files(vec![BundleFile {
            path: "bin/probe".to_string(),
            executable: true,
            contents: format!("#!/bin/sh\n{body}\n").into_bytes(),
        }])
        .unwrap();
        store.stage(&bundle).unwrap()
    }

    #[cfg(unix)]
    fn spec_for(bundle_dir: PathBuf, ext: &str, unit: &str, argv: Vec<String>) -> ProcessSpec {
        ProcessSpec {
            bundle_dir,
            socket_path: PathBuf::from("/nonexistent/rozi.sock"),
            launch: Launch::Direct { argv },
            cwd: SpawnCwd::Bundle {
                path: ".".to_string(),
            },
            env: vec![("ROZI_EXTENSION".to_string(), ext.to_string())],
            credential: "c".repeat(64),
            platforms: Vec::new(),
            capture: None,
            unit: Some(unit.to_string()),
        }
    }

    #[test]
    fn backoff_policy_evaluates_exit_and_uptime_reset() {
        let policy = BackoffPolicy::default();
        let now = Instant::now();

        // Clean exit under OnFailure stops normally.
        assert_eq!(
            policy.evaluate_exit(
                ServiceRestart::OnFailure,
                true,
                Some(now),
                0,
                INITIAL_BACKOFF,
                now
            ),
            RestartOutcome::Stop {
                reason: StopReason::NormalExit,
                failures: 0,
            }
        );

        // Failed exit steps the backoff ladder until MAX_FAILURES.
        let mut failures = 0;
        let mut backoff = INITIAL_BACKOFF;
        for expected_failures in 1..MAX_FAILURES {
            match policy.evaluate_exit(
                ServiceRestart::OnFailure,
                false,
                Some(now),
                failures,
                backoff,
                now,
            ) {
                RestartOutcome::Restart {
                    next_backoff: next,
                    failures: f,
                    ..
                } => {
                    assert_eq!(f, expected_failures);
                    failures = f;
                    backoff = next;
                }
                other => panic!("expected restart, got {other:?}"),
            }
        }
        assert_eq!(
            policy.evaluate_exit(
                ServiceRestart::OnFailure,
                false,
                Some(now),
                failures,
                backoff,
                now,
            ),
            RestartOutcome::Stop {
                reason: StopReason::ExhaustedBackoff,
                failures: MAX_FAILURES,
            }
        );

        // Running healthy for UPTIME_RESET_SECS resets failure count and backoff.
        let healthy_start = now - Duration::from_secs(UPTIME_RESET_SECS + 1);
        assert_eq!(
            policy.evaluate_exit(
                ServiceRestart::OnFailure,
                false,
                Some(healthy_start),
                MAX_FAILURES - 1,
                MAX_BACKOFF,
                now,
            ),
            RestartOutcome::Restart {
                restart_at: now + INITIAL_BACKOFF,
                delay: INITIAL_BACKOFF,
                next_backoff: Duration::from_secs(2),
                failures: 1,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_supervised_process_refuses_bundle_tampering_before_its_next_launch() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        let directory = stage_script(&store, "exit 0");
        let spec = spec_for(
            directory.clone(),
            "supverify",
            "service.watch",
            vec!["./bin/probe".into()],
        );
        let mut process = SupervisedProcess::spawn(spec.clone()).expect("verified launch");
        let deadline = Instant::now() + Duration::from_secs(5);
        while process.poll().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let file = directory.join("bin/probe");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // Simulate tampering in this scratch bundle.
        permissions.set_readonly(false);
        std::fs::set_permissions(file, permissions).unwrap();
        assert!(matches!(
            SupervisedProcess::spawn(spec),
            Err(SpawnFailure::BundleCorrupt { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_restarts_crashing_service_and_stops_when_backoff_exhausted() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        let counter_file = temp.path().join("runs");
        let bundle_dir = stage_script(
            &store,
            &format!(
                "echo run >> '{}'; echo boom >&2; exit 3",
                counter_file.display()
            ),
        );

        let (tx, rx) = mpsc::channel();
        let policy = BackoffPolicy {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(80),
            uptime_reset: Duration::from_secs(60),
            max_failures: 3,
        };
        let supervisor =
            Supervisor::start_with_policy(policy, Duration::from_millis(10), move |event| {
                let _ = tx.send(event);
            });

        supervisor
            .start_service(
                "crasher.watch",
                ServiceSpec {
                    process: spec_for(
                        bundle_dir,
                        "supcrash",
                        "service.watch",
                        vec!["./bin/probe".to_string()],
                    ),
                    restart: ServiceRestart::OnFailure,
                },
            )
            .unwrap();

        let mut spawned = 0;
        let mut scheduled = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stopped = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(SupervisorEvent::ServiceSpawned { name, .. }) => {
                    assert_eq!(name, "crasher.watch");
                    spawned += 1;
                }
                Ok(SupervisorEvent::ServiceRestartScheduled {
                    name,
                    failures,
                    delay,
                    ..
                }) => {
                    assert_eq!(name, "crasher.watch");
                    scheduled.push((failures, delay));
                }
                Ok(SupervisorEvent::ServiceStopped {
                    name,
                    reason,
                    failures,
                    detail,
                }) => {
                    assert_eq!(name, "crasher.watch");
                    break (reason, failures, detail);
                }
                Ok(other) => panic!("unexpected event: {other:?}"),
                Err(err) => panic!("timed out waiting for service stop: {err}"),
            }
        };

        assert_eq!(spawned, 3, "ran initial spawn plus two restarts");
        assert_eq!(
            scheduled,
            vec![
                (1, Duration::from_millis(20)),
                (2, Duration::from_millis(40)),
            ]
        );
        assert_eq!(stopped.0, StopReason::ExhaustedBackoff);
        assert_eq!(stopped.1, 3);
        assert!(stopped.2.contains("exited with status 3"), "{}", stopped.2);

        let runs = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(runs.lines().count(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn stopping_supervised_service_kills_its_entire_process_tree() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        let pidfile = temp.path().join("grandchild.pid");
        let bundle_dir = stage_script(
            &store,
            &format!("sleep 300 & echo $! > '{}'; wait", pidfile.display()),
        );

        let (tx, rx) = mpsc::channel();
        let supervisor = Supervisor::start_with_policy(
            BackoffPolicy::default(),
            Duration::from_millis(10),
            move |event| {
                let _ = tx.send(event);
            },
        );

        supervisor
            .start_service(
                "tree.watch",
                ServiceSpec {
                    process: spec_for(
                        bundle_dir,
                        "suptree",
                        "service.watch",
                        vec!["./bin/probe".to_string()],
                    ),
                    restart: ServiceRestart::Always,
                },
            )
            .unwrap();

        let parent_pid = match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            SupervisorEvent::ServiceSpawned { pid, .. } => pid,
            other => panic!("expected ServiceSpawned, got {other:?}"),
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        let grandchild_pid: u32 = loop {
            if let Ok(content) = std::fs::read_to_string(&pidfile)
                && let Ok(pid) = content.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "grandchild pid never written");
            std::thread::sleep(Duration::from_millis(10));
        };

        supervisor.stop_service("tree.watch").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while crate::platform::command::process_is_alive(parent_pid)
            || crate::platform::command::process_is_alive(grandchild_pid)
        {
            assert!(
                Instant::now() < deadline,
                "process tree survived stop_service"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn captured_worker_timeout_terminates_process_group_and_returns_partial_stdout() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        let pidfile = temp.path().join("child.pid");
        let bundle_dir = stage_script(
            &store,
            &format!(
                "printf 'partial-out'; sleep 300 & echo $! > '{}'; wait",
                pidfile.display()
            ),
        );

        let (tx, rx) = mpsc::channel();
        let supervisor = Supervisor::start_with_policy(
            BackoffPolicy::default(),
            Duration::from_millis(10),
            move |event| {
                let _ = tx.send(event);
            },
        );

        let mut spec = spec_for(
            bundle_dir,
            "suptimeout",
            "command.slow",
            vec!["./bin/probe".to_string()],
        );
        spec.capture = Some(Capture {
            max_bytes: 1024,
            timeout_ms: 150,
        });

        supervisor.spawn_worker(42, spec).unwrap();
        let parent_pid = match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            SupervisorEvent::WorkerSpawned { worker: 42, pid } => pid,
            other => panic!("expected WorkerSpawned, got {other:?}"),
        };

        let outcome = match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            SupervisorEvent::WorkerExited {
                worker: 42,
                outcome,
            } => outcome,
            other => panic!("expected WorkerExited, got {other:?}"),
        };
        assert!(outcome.timed_out);
        assert!(!outcome.killed);
        assert_eq!(outcome.output.as_deref(), Some("partial-out"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while crate::platform::command::process_is_alive(parent_pid) {
            assert!(Instant::now() < deadline, "timed-out worker survived");
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Ok(content) = std::fs::read_to_string(&pidfile)
            && let Ok(grandchild_pid) = content.trim().parse::<u32>()
        {
            while crate::platform::command::process_is_alive(grandchild_pid) {
                assert!(
                    Instant::now() < deadline,
                    "timed-out worker's child survived"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
