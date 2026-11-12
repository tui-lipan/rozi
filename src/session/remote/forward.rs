//! Independently owned SSH local port forwarding (`ssh -N -L ...`).
//!
//! Each tunnel owns its own `ssh` child process and process group / job object rather than riding
//! on Rozi's shared `ControlMaster`, so dropping a [`ForwardHandle`] tears down the listener
//! deterministically without affecting any session attachment.

use std::io::{self, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{ChildStderr, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::RemoteConfig;
use crate::platform::command::{CommandGroup, configure_command_group, kill_on_supervisor_death};

use super::{
    RemoteTarget, ResolvedRemote, SshMultiplexing, append_ssh_destination, ssh_base_command_with,
    validate_remote_target,
};

const READY_POLL_INTERVAL: Duration = Duration::from_millis(20);
const STEADY_POLL_INTERVAL: Duration = Duration::from_millis(200);
const READY_CONNECT_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_BIND_RETRIES: u32 = 3;
const STDERR_CAPTURE_CAP: usize = 16 * 1024;

/// Which host the remote `sshd` connects to at the far end of a tunnel.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ForwardHost {
    /// Remote loopback (`127.0.0.1`). Covers `localhost`, `127.0.0.1`, `0.0.0.0`, and `::`.
    Loopback,
    /// An explicit remote host (for example `::1`, `127.0.0.2`, or `database.internal`).
    Host(String),
}

impl ForwardHost {
    /// Host token passed to `ssh -L` on the remote side.
    pub fn connect_host(&self) -> &str {
        match self {
            Self::Loopback => "127.0.0.1",
            Self::Host(host) => host.as_str(),
        }
    }

    /// Compact host label for UI display.
    pub fn display_label(&self) -> &str {
        match self {
            Self::Loopback => "localhost",
            Self::Host(host) => host.as_str(),
        }
    }

    /// Parse and validate a user-entered remote forward host.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim().trim_start_matches('[').trim_end_matches(']');
        validate_connect_host(trimmed)?;
        if trimmed.eq_ignore_ascii_case("localhost")
            || trimmed == "127.0.0.1"
            || trimmed == "0.0.0.0"
            || trimmed == "::"
        {
            Ok(Self::Loopback)
        } else {
            Ok(Self::Host(trimmed.to_string()))
        }
    }
}

/// Stable client-global identity of a port forward.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForwardKey {
    pub target: RemoteTarget,
    pub remote_host: ForwardHost,
    pub remote_port: u16,
}

/// Concrete SSH forward parameters for one `ssh -N -L` process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardSpec {
    pub target: RemoteTarget,
    pub connect_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

/// Lifecycle notifications emitted by a [`ForwardHandle`] worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardEvent {
    Ready { local_port: u16 },
    Reconnecting { error: String },
    AuthRequired { error: String },
    Failed { error: String },
}

#[derive(Default)]
struct StopSignal {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl StopSignal {
    fn request(&self) {
        let mut stopped = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        *stopped = true;
        self.changed.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait up to `duration` unless stop was requested. Returns `true` when stopped.
    fn wait_timeout(&self, duration: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        if *guard {
            return true;
        }
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, duration, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *guard
    }
}

/// Owned handle to a supervised `ssh -N` port-forward worker.
///
/// Dropping the handle signals the worker, kills the active SSH child's process group / job
/// object, and waits for the worker to reap and exit so no local listener survives.
pub struct ForwardHandle {
    stop: Arc<StopSignal>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.stop.request();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Pick a free local loopback port, trying `preferred` first before falling back to an ephemeral
/// port.
pub fn preferred_local_port(preferred: Option<u16>) -> io::Result<u16> {
    if let Some(port) = preferred.filter(|&port| port != 0)
        && let Ok(listener) = TcpListener::bind(("127.0.0.1", port))
    {
        drop(listener);
        return Ok(port);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

pub(crate) fn validate_connect_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("remote forward host cannot be empty".to_string());
    }
    if host.chars().any(|ch| ch.is_control() || ch.is_whitespace()) || host.contains('/') {
        return Err(format!("invalid remote forward host `{host}`"));
    }
    Ok(())
}

/// Format `-L` argument, always binding local loopback `127.0.0.1` so forwarded ports are never
/// exposed on LAN interfaces.
pub(crate) fn format_local_forward(
    local_port: u16,
    connect_host: &str,
    remote_port: u16,
) -> String {
    let remote = if connect_host.contains(':') {
        let host = connect_host.trim_matches(['[', ']']);
        format!("[{host}]")
    } else {
        connect_host.to_string()
    };
    format!("127.0.0.1:{local_port}:{remote}:{remote_port}")
}

/// Build the `ssh -N -T` command for one local port forward, reusing Rozi's SSH target/config
/// resolution while explicitly disabling `ControlMaster` multiplexing.
pub(crate) fn build_forward_command(spec: &ForwardSpec, config: &RemoteConfig) -> Command {
    let resolved = ResolvedRemote::resolve(&spec.target, config);
    let mut command = ssh_base_command_with(
        &resolved,
        config,
        config.connection_timeout_secs,
        SshMultiplexing::Disabled,
    );
    command.arg("-N");
    command.arg("-o").arg("ExitOnForwardFailure=yes");
    command.arg("-L").arg(format_local_forward(
        spec.local_port,
        &spec.connect_host,
        spec.remote_port,
    ));
    append_ssh_destination(&mut command, &resolved);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// Start a supervised `ssh -N` port forward.
///
/// The initial connection respects `interactive` (allowing Rozi's UI askpass dialog when
/// `[remote] batch_mode = false`). Once the tunnel has become ready, subsequent automatic
/// reconnects after a drop always run unattended (`RemoteConfig::unattended`).
pub fn start_forward(
    mut spec: ForwardSpec,
    config: RemoteConfig,
    interactive: bool,
    report: impl Fn(ForwardEvent) + Send + 'static,
) -> io::Result<ForwardHandle> {
    validate_remote_target(&spec.target).map_err(io::Error::other)?;
    validate_connect_host(&spec.connect_host).map_err(io::Error::other)?;
    if spec.remote_port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote port must be non-zero",
        ));
    }
    if spec.local_port == 0 {
        spec.local_port = preferred_local_port(Some(spec.remote_port))?;
    }

    let stop = Arc::new(StopSignal::default());
    let worker_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("rozi-port-forward".to_string())
        .spawn(move || {
            run_forward_loop(spec, config, interactive, &worker_stop, &report);
        })?;

    Ok(ForwardHandle {
        stop,
        thread: Some(thread),
    })
}

fn run_forward_loop(
    mut spec: ForwardSpec,
    config: RemoteConfig,
    interactive: bool,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
) {
    let mut has_been_ready = false;
    let mut reconnect_attempt: u32 = 0;
    let mut bind_retries: u32 = 0;

    loop {
        if stop.is_stopped() {
            return;
        }
        let attempt_config = if !has_been_ready && interactive {
            config.clone()
        } else {
            config.unattended()
        };

        match run_single_attempt(&spec, &attempt_config, stop, || {
            report(ForwardEvent::Ready {
                local_port: spec.local_port,
            });
        }) {
            AttemptOutcome::Stopped => return,
            AttemptOutcome::ReadyThenExited(stderr) => {
                has_been_ready = true;
                reconnect_attempt = 0;
                bind_retries = 0;
                if handle_disconnect(&stderr, &mut spec, &mut reconnect_attempt, stop, report) {
                    return;
                }
            }
            AttemptOutcome::ExitedBeforeReady {
                stderr,
                spawn_not_found,
            } => {
                if !spawn_not_found
                    && is_local_bind_race(&stderr)
                    && bind_retries < MAX_BIND_RETRIES
                    && let Ok(next_port) = preferred_local_port(None)
                {
                    bind_retries += 1;
                    spec.local_port = next_port;
                    continue;
                }

                let message = format_forward_error(&stderr, &spec.target);
                if crate::session::discovery::ssh_failure_needs_user(&stderr) {
                    report(ForwardEvent::AuthRequired { error: message });
                    return;
                }
                if !has_been_ready {
                    report(ForwardEvent::Failed { error: message });
                    return;
                }
                if wait_reconnect_backoff(message, &mut reconnect_attempt, stop, report) {
                    return;
                }
            }
        }
    }
}

fn handle_disconnect(
    stderr: &str,
    spec: &mut ForwardSpec,
    reconnect_attempt: &mut u32,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
) -> bool {
    let message = format_forward_error(stderr, &spec.target);
    if crate::session::discovery::ssh_failure_needs_user(stderr) {
        report(ForwardEvent::AuthRequired { error: message });
        return true;
    }
    // Re-check local port availability before reconnecting so a port stolen while down does not
    // block recovery.
    if let Ok(port) = preferred_local_port(Some(spec.local_port)) {
        spec.local_port = port;
    }
    wait_reconnect_backoff(message, reconnect_attempt, stop, report)
}

fn wait_reconnect_backoff(
    message: String,
    attempt: &mut u32,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
) -> bool {
    report(ForwardEvent::Reconnecting { error: message });
    let delay = retry_delay(*attempt);
    *attempt = attempt.saturating_add(1);
    stop.wait_timeout(delay)
}

enum AttemptOutcome {
    Stopped,
    ReadyThenExited(String),
    ExitedBeforeReady {
        stderr: String,
        spawn_not_found: bool,
    },
}

fn run_single_attempt(
    spec: &ForwardSpec,
    config: &RemoteConfig,
    stop: &StopSignal,
    on_ready: impl Fn(),
) -> AttemptOutcome {
    let mut command = build_forward_command(spec, config);
    configure_command_group(&mut command);
    kill_on_supervisor_death(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let spawn_not_found = error.kind() == io::ErrorKind::NotFound;
            let message = if spawn_not_found {
                "ssh was not found on PATH".to_string()
            } else {
                format!("failed to spawn ssh: {error}")
            };
            return AttemptOutcome::ExitedBeforeReady {
                stderr: message,
                spawn_not_found,
            };
        }
    };

    let group = match CommandGroup::new(&child) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return AttemptOutcome::ExitedBeforeReady {
                stderr: format!("failed to supervise ssh process: {error}"),
                spawn_not_found: false,
            };
        }
    };

    let stderr_thread = child.stderr.take().map(spawn_stderr_collector);
    let addr = SocketAddr::from(([127, 0, 0, 1], spec.local_port));

    // Phase 1: wait for ssh to bind `local_port` (or exit).
    loop {
        if stop.is_stopped() {
            group.terminate(&mut child);
            return AttemptOutcome::Stopped;
        }
        match group.reap_if_exited(&mut child) {
            Ok(Some(status)) => {
                let stderr = finish_stderr(stderr_thread, status.code());
                return AttemptOutcome::ExitedBeforeReady {
                    stderr,
                    spawn_not_found: false,
                };
            }
            Ok(None) => {}
            Err(error) => {
                group.terminate(&mut child);
                return AttemptOutcome::ExitedBeforeReady {
                    stderr: error.to_string(),
                    spawn_not_found: false,
                };
            }
        }
        if let Ok(stream) = TcpStream::connect_timeout(&addr, READY_CONNECT_TIMEOUT) {
            drop(stream);
            // Confirm our ssh child is still alive and did not lose a bind race right as another
            // listener answered.
            match group.reap_if_exited(&mut child) {
                Ok(None) => break,
                Ok(Some(status)) => {
                    let stderr = finish_stderr(stderr_thread, status.code());
                    return AttemptOutcome::ExitedBeforeReady {
                        stderr,
                        spawn_not_found: false,
                    };
                }
                Err(error) => {
                    group.terminate(&mut child);
                    return AttemptOutcome::ExitedBeforeReady {
                        stderr: error.to_string(),
                        spawn_not_found: false,
                    };
                }
            }
        }
        if stop.wait_timeout(READY_POLL_INTERVAL) {
            group.terminate(&mut child);
            return AttemptOutcome::Stopped;
        }
    }

    on_ready();

    // Phase 2: monitor the running tunnel until stopped or ssh exits.
    loop {
        if stop.wait_timeout(STEADY_POLL_INTERVAL) {
            group.terminate(&mut child);
            return AttemptOutcome::Stopped;
        }
        match group.reap_if_exited(&mut child) {
            Ok(Some(status)) => {
                let stderr = finish_stderr(stderr_thread, status.code());
                return AttemptOutcome::ReadyThenExited(stderr);
            }
            Ok(None) => {}
            Err(error) => {
                group.terminate(&mut child);
                return AttemptOutcome::ReadyThenExited(error.to_string());
            }
        }
    }
}

fn spawn_stderr_collector(mut stderr: ChildStderr) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let overflow = (buf.len() + n).saturating_sub(STDERR_CAPTURE_CAP);
                    buf.drain(..overflow);
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn finish_stderr(thread: Option<JoinHandle<String>>, exit_code: Option<i32>) -> String {
    let text = thread
        .and_then(|handle| handle.join().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !text.is_empty() {
        return text;
    }
    match exit_code {
        Some(code) => format!("ssh exited with status {code}"),
        None => "ssh exited unexpectedly".to_string(),
    }
}

fn is_local_bind_race(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("address already in use")
        || lower.contains("cannot listen to port")
        || lower.contains("could not request local forwarding")
}

fn format_forward_error(stderr: &str, target: &RemoteTarget) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return format!("SSH tunnel to {} closed", target.display_label());
    }
    let summary = crate::session::discovery::probe_failure_reason(trimmed);
    if summary != "Connection failed" {
        format!("{summary}: {trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis((500_u64 << attempt.min(6)).min(30_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_command_argv_disables_multiplexing_and_reuses_ssh_config() {
        let mut hosts = std::collections::HashMap::new();
        hosts.insert(
            "workbox".to_string(),
            crate::config::RemoteHostConfig {
                host: Some("workbox.internal".into()),
                user: Some("alice".into()),
                port: Some(2222),
                identity_file: Some("/tmp/test-key".into()),
                ssh_args: vec!["-J".into(), "bastion".into()],
                binary_path: None,
            },
        );
        let config = RemoteConfig {
            hosts,
            connection_timeout_secs: 9,
            server_alive_interval_secs: 7,
            server_alive_count_max: 4,
            batch_mode: true,
            ..RemoteConfig::default()
        };
        let spec = ForwardSpec {
            target: RemoteTarget::Alias("workbox".into()),
            connect_host: "127.0.0.1".into(),
            remote_port: 5173,
            local_port: 49184,
        };
        let command = build_forward_command(&spec, &config);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.iter().any(|arg| arg == "-N"));
        assert!(args.iter().any(|arg| arg == "-T"));
        assert!(args.iter().any(|arg| arg == "ExitOnForwardFailure=yes"));
        assert!(args.iter().any(|arg| arg == "ControlMaster=no"));
        assert!(args.iter().any(|arg| arg == "ControlPath=none"));
        assert!(!args.iter().any(|arg| arg == "ControlMaster=auto"));
        assert!(args.iter().any(|arg| arg == "ConnectTimeout=9"));
        assert!(args.iter().any(|arg| arg == "ServerAliveInterval=7"));
        assert!(args.iter().any(|arg| arg == "ServerAliveCountMax=4"));
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));

        let fwd_pos = args.iter().position(|arg| arg == "-L").expect("-L");
        assert_eq!(args[fwd_pos + 1], "127.0.0.1:49184:127.0.0.1:5173");

        let port_pos = args.iter().position(|arg| arg == "-p").expect("-p");
        assert_eq!(args[port_pos + 1], "2222");

        let id_pos = args.iter().position(|arg| arg == "-i").expect("-i");
        assert_eq!(args[id_pos + 1], "/tmp/test-key");

        let jump_pos = args.iter().position(|arg| arg == "-J").expect("-J");
        assert_eq!(args[jump_pos + 1], "bastion");

        let marker = args.iter().position(|arg| arg == "--").expect("--");
        assert_eq!(args[marker + 1], "alice@workbox.internal");
    }

    #[test]
    fn ipv6_connect_host_is_bracketed_in_forward_spec() {
        assert_eq!(
            format_local_forward(3000, "::1", 3000),
            "127.0.0.1:3000:[::1]:3000"
        );
    }

    #[test]
    fn preferred_local_port_preserves_free_port_and_remaps_occupied_port() {
        let ephemeral = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied_port = ephemeral.local_addr().unwrap().port();
        // While held, preferred_local_port must pick a different free port.
        let remapped = preferred_local_port(Some(occupied_port)).unwrap();
        assert_ne!(remapped, occupied_port);
        assert_ne!(remapped, 0);

        // Once released, preferred_local_port can reclaim that port.
        drop(ephemeral);
        let reclaimed = preferred_local_port(Some(occupied_port)).unwrap();
        assert_eq!(reclaimed, occupied_port);
    }

    #[test]
    fn forward_host_normalizes_loopback_and_rejects_whitespace() {
        assert_eq!(
            ForwardHost::parse("localhost").unwrap(),
            ForwardHost::Loopback
        );
        assert_eq!(
            ForwardHost::parse("127.0.0.1").unwrap(),
            ForwardHost::Loopback
        );
        assert_eq!(
            ForwardHost::parse("0.0.0.0").unwrap(),
            ForwardHost::Loopback
        );
        assert_eq!(ForwardHost::parse("::").unwrap(), ForwardHost::Loopback);
        assert_eq!(
            ForwardHost::parse("[::1]").unwrap(),
            ForwardHost::Host("::1".into())
        );
        assert!(ForwardHost::parse("bad host").is_err());
        assert!(ForwardHost::parse("").is_err());
    }
}
