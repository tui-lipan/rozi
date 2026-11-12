//! Independently owned SSH local port forwarding (`ssh -N -L ...`).
//!
//! Each tunnel owns its own `ssh` child process and process group / job object rather than riding
//! on Rozi's shared `ControlMaster`, so dropping a [`ForwardHandle`] tears down the listener
//! deterministically without affecting any session attachment.

use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
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
    /// Remote `localhost`, letting the remote host's resolver pick IPv4 or IPv6 loopback.
    Localhost,
    /// Remote IPv4 loopback (`127.0.0.1`). Covers `127.0.0.1` and `0.0.0.0`.
    Ipv4Loopback,
    /// Remote IPv6 loopback (`::1`). Covers `::1` and `::`.
    Ipv6Loopback,
    /// An explicit remote host (for example `127.0.0.2` or `database.internal`).
    Host(String),
}

impl ForwardHost {
    /// Host token passed to `ssh -L` on the remote side.
    pub fn connect_host(&self) -> &str {
        match self {
            Self::Localhost => "localhost",
            Self::Ipv4Loopback => "127.0.0.1",
            Self::Ipv6Loopback => "::1",
            Self::Host(host) => host.as_str(),
        }
    }

    /// Compact host label for UI display.
    pub fn display_label(&self) -> &str {
        match self {
            Self::Localhost => "localhost",
            Self::Ipv4Loopback => "127.0.0.1",
            Self::Ipv6Loopback => "::1",
            Self::Host(host) => host.as_str(),
        }
    }

    /// Classify a loopback or wildcard IP address into its remote forward target.
    pub fn from_loopback_ip(ip: IpAddr) -> Option<Self> {
        match ip {
            IpAddr::V4(ip) => {
                if ip == Ipv4Addr::LOCALHOST || ip.is_unspecified() {
                    Some(Self::Ipv4Loopback)
                } else if ip.is_loopback() {
                    Some(Self::Host(ip.to_string()))
                } else {
                    None
                }
            }
            IpAddr::V6(ip) => {
                if ip.is_unspecified() || ip == Ipv6Addr::LOCALHOST {
                    Some(Self::Ipv6Loopback)
                } else if let Some(v4) = ip.to_ipv4_mapped() {
                    Self::from_loopback_ip(IpAddr::V4(v4))
                } else {
                    None
                }
            }
        }
    }

    /// Parse and validate a user-entered remote forward host.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim().trim_start_matches('[').trim_end_matches(']');
        validate_connect_host(trimmed)?;
        if trimmed.eq_ignore_ascii_case("localhost") {
            return Ok(Self::Localhost);
        }
        if let Ok(ip) = trimmed.parse::<IpAddr>()
            && let Some(loopback) = Self::from_loopback_ip(ip)
        {
            return Ok(loopback);
        }
        Ok(Self::Host(trimmed.to_string()))
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
    /// Refuse remapping `local_port` on bind conflicts even before the first `Ready`
    /// (for example when restarting a forward whose local endpoint was already published).
    pub pin_local_port: bool,
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

impl ForwardHandle {
    #[cfg(test)]
    pub(crate) fn dormant() -> Self {
        Self {
            stop: Arc::new(StopSignal::default()),
            thread: None,
        }
    }
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
    if spec.pin_local_port && spec.local_port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pinned local port must be non-zero",
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
    spec: ForwardSpec,
    config: RemoteConfig,
    interactive: bool,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
) {
    run_forward_loop_with(
        spec,
        config,
        interactive,
        stop,
        report,
        retry_delay,
        |spec, attempt_config, stop, on_ready| {
            run_single_attempt(spec, attempt_config, stop, on_ready)
        },
    );
}

fn run_forward_loop_with(
    mut spec: ForwardSpec,
    config: RemoteConfig,
    interactive: bool,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
    delay_for_attempt: fn(u32) -> Duration,
    mut run_attempt: impl FnMut(&ForwardSpec, &RemoteConfig, &StopSignal, &dyn Fn()) -> AttemptOutcome,
) {
    let has_been_ready = std::cell::Cell::new(false);
    let port_pinned = std::cell::Cell::new(spec.pin_local_port);
    let mut reconnect_attempt: u32 = 0;
    let mut bind_retries: u32 = 0;

    loop {
        if stop.is_stopped() {
            return;
        }
        // Check local port availability before spawning ssh:
        // - While unpinned before the first Ready, a bind conflict can remap to a fresh ephemeral
        //   port.
        // - Once pinned (either from a prior session or after Ready), the local port stays fixed
        //   so existing URLs/tabs never silently point at a different service or port. Checking here
        //   also avoids mistaking a thief's listener for ssh during its handshake.
        if let Err(error) = TcpListener::bind(("127.0.0.1", spec.local_port)) {
            if !port_pinned.get()
                && bind_retries < MAX_BIND_RETRIES
                && let Ok(next_port) = preferred_local_port(None)
            {
                bind_retries += 1;
                spec.local_port = next_port;
                continue;
            }
            let message = format!("Local port {} is already in use: {error}", spec.local_port);
            if !has_been_ready.get() {
                report(ForwardEvent::Failed { error: message });
                return;
            }
            if wait_reconnect_backoff(
                message,
                &mut reconnect_attempt,
                stop,
                report,
                delay_for_attempt,
            ) {
                return;
            }
            continue;
        }

        let attempt_config = if !has_been_ready.get() && interactive {
            config.clone()
        } else {
            config.unattended()
        };

        let on_ready = || {
            has_been_ready.set(true);
            port_pinned.set(true);
            report(ForwardEvent::Ready {
                local_port: spec.local_port,
            });
        };
        match run_attempt(&spec, &attempt_config, stop, &on_ready) {
            AttemptOutcome::Stopped => return,
            AttemptOutcome::ReadyThenExited(stderr) => {
                has_been_ready.set(true);
                port_pinned.set(true);
                reconnect_attempt = 0;
                bind_retries = 0;
                if handle_disconnect(
                    &stderr,
                    &spec.target,
                    &mut reconnect_attempt,
                    stop,
                    report,
                    delay_for_attempt,
                ) {
                    return;
                }
            }
            AttemptOutcome::ExitedBeforeReady {
                stderr,
                spawn_not_found,
            } => {
                if !port_pinned.get()
                    && !spawn_not_found
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
                if !has_been_ready.get() {
                    report(ForwardEvent::Failed { error: message });
                    return;
                }
                if wait_reconnect_backoff(
                    message,
                    &mut reconnect_attempt,
                    stop,
                    report,
                    delay_for_attempt,
                ) {
                    return;
                }
            }
        }
    }
}

fn handle_disconnect(
    stderr: &str,
    target: &RemoteTarget,
    reconnect_attempt: &mut u32,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
    delay_for_attempt: fn(u32) -> Duration,
) -> bool {
    let message = format_forward_error(stderr, target);
    if crate::session::discovery::ssh_failure_needs_user(stderr) {
        report(ForwardEvent::AuthRequired { error: message });
        return true;
    }
    wait_reconnect_backoff(message, reconnect_attempt, stop, report, delay_for_attempt)
}

fn wait_reconnect_backoff(
    message: String,
    attempt: &mut u32,
    stop: &StopSignal,
    report: &impl Fn(ForwardEvent),
    delay_for_attempt: fn(u32) -> Duration,
) -> bool {
    report(ForwardEvent::Reconnecting { error: message });
    let delay = delay_for_attempt(*attempt);
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
            pin_local_port: false,
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
    fn ipv6_and_localhost_connect_hosts_are_preserved_in_forward_spec() {
        assert_eq!(
            format_local_forward(3000, ForwardHost::Ipv6Loopback.connect_host(), 3000),
            "127.0.0.1:3000:[::1]:3000"
        );
        assert_eq!(
            format_local_forward(3000, ForwardHost::Localhost.connect_host(), 3000),
            "127.0.0.1:3000:localhost:3000"
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
    fn forward_host_distinguishes_localhost_ipv4_and_ipv6_loopbacks() {
        assert_eq!(
            ForwardHost::parse("localhost").unwrap(),
            ForwardHost::Localhost
        );
        assert_eq!(ForwardHost::Localhost.connect_host(), "localhost");
        assert_eq!(
            ForwardHost::parse("127.0.0.1").unwrap(),
            ForwardHost::Ipv4Loopback
        );
        assert_eq!(
            ForwardHost::parse("0.0.0.0").unwrap(),
            ForwardHost::Ipv4Loopback
        );
        assert_eq!(ForwardHost::Ipv4Loopback.connect_host(), "127.0.0.1");
        assert_eq!(ForwardHost::parse("::").unwrap(), ForwardHost::Ipv6Loopback);
        assert_eq!(
            ForwardHost::parse("[::1]").unwrap(),
            ForwardHost::Ipv6Loopback
        );
        assert_eq!(ForwardHost::Ipv6Loopback.connect_host(), "::1");
        assert_eq!(
            ForwardHost::parse("127.0.0.2").unwrap(),
            ForwardHost::Host("127.0.0.2".into())
        );
        assert!(ForwardHost::parse("bad host").is_err());
        assert!(ForwardHost::parse("").is_err());
    }

    #[test]
    fn forward_loop_remaps_before_first_ready_and_pins_local_port_after_ready() {
        let stop = StopSignal::default();
        let initial_port = preferred_local_port(None).unwrap();
        let spec = ForwardSpec {
            target: RemoteTarget::Alias("workbox".into()),
            connect_host: "127.0.0.1".into(),
            remote_port: 5173,
            local_port: initial_port,
            pin_local_port: false,
        };

        let events = std::sync::Mutex::new(Vec::new());
        let thief = std::sync::Mutex::new(None::<TcpListener>);
        let mut pinned_port = None;
        let mut step = 0_u32;

        run_forward_loop_with(
            spec,
            RemoteConfig::default(),
            false,
            &stop,
            &|event| {
                if let ForwardEvent::Reconnecting { error } = &event
                    && error.contains("already in use")
                {
                    *thief.lock().unwrap() = None;
                }
                events.lock().unwrap().push(event);
            },
            |_| Duration::from_millis(1),
            |spec, _cfg, stop, on_ready| {
                step += 1;
                match step {
                    // 1. Pre-Ready bind race migrates to a free ephemeral port.
                    1 => AttemptOutcome::ExitedBeforeReady {
                        stderr: "bind [127.0.0.1]: Address already in use".into(),
                        spawn_not_found: false,
                    },
                    // 2. First Ready locks in `spec.local_port`, then disconnects while another
                    //    local service steals that port.
                    2 => {
                        assert_ne!(spec.local_port, initial_port);
                        pinned_port = Some(spec.local_port);
                        on_ready();
                        *thief.lock().unwrap() =
                            Some(TcpListener::bind(("127.0.0.1", spec.local_port)).unwrap());
                        AttemptOutcome::ReadyThenExited("connection reset".into())
                    }
                    // 3. Even after the thief releases the port and an in-attempt bind race
                    //    happens, post-Ready recovery must keep the pinned port.
                    3 => {
                        assert_eq!(Some(spec.local_port), pinned_port);
                        AttemptOutcome::ExitedBeforeReady {
                            stderr: "Address already in use".into(),
                            spawn_not_found: false,
                        }
                    }
                    4 => {
                        assert_eq!(Some(spec.local_port), pinned_port);
                        on_ready();
                        stop.request();
                        AttemptOutcome::Stopped
                    }
                    other => panic!("unexpected attempt {other}"),
                }
            },
        );

        let pinned = pinned_port.expect("first ready port");
        let events = events.into_inner().unwrap();
        assert_eq!(
            events.first(),
            Some(&ForwardEvent::Ready { local_port: pinned })
        );
        assert!(events.iter().any(
            |event| matches!(event, ForwardEvent::Reconnecting { error } if error.contains("already in use"))
        ));
        assert_eq!(
            events.last(),
            Some(&ForwardEvent::Ready { local_port: pinned })
        );
    }

    #[test]
    fn pinned_pre_ready_forward_allows_interactive_auth_and_refuses_remapping() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind occupied port");
        let occupied_port = listener.local_addr().expect("local addr").port();

        let target = RemoteTarget::Alias("workbox".into());
        let config = RemoteConfig {
            batch_mode: false,
            ..RemoteConfig::default()
        };

        // Even while still pre-Ready and interactive, a pinned port must not remap on bind races
        // or pre-spawn bind conflicts, nor silently force BatchMode on the initial attempt.
        let attempts = std::cell::Cell::new(0u32);
        let race_port = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("bind free probe port");
            probe.local_addr().expect("probe addr").port()
        };
        let events = std::sync::Mutex::new(Vec::new());
        let stop = StopSignal::default();
        run_forward_loop_with(
            ForwardSpec {
                target: target.clone(),
                connect_host: "127.0.0.1".into(),
                remote_port: 5173,
                local_port: race_port,
                pin_local_port: true,
            },
            config.clone(),
            true,
            &stop,
            &|event| events.lock().unwrap().push(event),
            |_| Duration::ZERO,
            |spec, attempt_config, _stop, _on_ready| {
                attempts.set(attempts.get() + 1);
                assert_eq!(spec.local_port, race_port);
                assert!(
                    !attempt_config.batch_mode,
                    "pinned restart from AuthRequired must keep interactive auth on first attempt"
                );
                AttemptOutcome::ExitedBeforeReady {
                    stderr: "Address already in use".into(),
                    spawn_not_found: false,
                }
            },
        );
        assert_eq!(attempts.get(), 1);
        let race_events = events.into_inner().unwrap();
        assert!(matches!(
            race_events.as_slice(),
            [ForwardEvent::Failed { .. }]
        ));

        let occupied_attempts = std::cell::Cell::new(0u32);
        let occupied_events = std::sync::Mutex::new(Vec::new());
        run_forward_loop_with(
            ForwardSpec {
                target,
                connect_host: "127.0.0.1".into(),
                remote_port: 5173,
                local_port: occupied_port,
                pin_local_port: true,
            },
            config,
            true,
            &stop,
            &|event| occupied_events.lock().unwrap().push(event),
            |_| Duration::ZERO,
            |_spec, _attempt_config, _stop, _on_ready| {
                occupied_attempts.set(occupied_attempts.get() + 1);
                AttemptOutcome::Stopped
            },
        );
        assert_eq!(occupied_attempts.get(), 0);
        let occupied_events = occupied_events.into_inner().unwrap();
        assert!(matches!(
            occupied_events.as_slice(),
            [ForwardEvent::Failed { error }] if error.contains(&occupied_port.to_string())
        ));
    }
}
