//! Local client: spawn `ssh … rozi --remote-serve` and return a Piped connection.

use std::io::{self, Read};
use std::process::{ChildStderr, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::RemoteConfig;
use crate::platform::command::program_exists;
use crate::platform::ipc::{self, IpcConnection};

use super::bootstrap::{
    append_remote_rozi_command, append_ssh_destination, ssh_base_command,
    ssh_base_command_with_connect_timeout,
};
use super::preamble::{self, RemotePreamble};
use super::{
    RemoteTarget, ResolvedRemote, validate_remote_executable_token, validate_remote_target,
};

#[derive(Debug)]
pub enum RemoteConnectError {
    Io(io::Error),
    Message(String),
    /// Remote session server protocol could not be negotiated; caller may offer a restart.
    ProtocolSkew(String),
}

impl RemoteConnectError {
    pub fn is_protocol_skew(&self) -> bool {
        matches!(self, Self::ProtocolSkew(_))
    }
}

impl std::fmt::Display for RemoteConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Message(msg) | Self::ProtocolSkew(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<io::Error> for RemoteConnectError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<String> for RemoteConnectError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

/// Spawn ssh to the resolved remote and return a protocol-ready pipe plus the preamble.
///
/// Uses the shared executable resolver. Missing binaries can be installed through the TUI's
/// confirmation broker; without a UI this remains a non-interactive, read-only check.
pub fn connect_remote(
    target: &RemoteTarget,
    session: &str,
    config: &RemoteConfig,
) -> Result<(IpcConnection, RemotePreamble), RemoteConnectError> {
    connect_remote_within(target, session, config, None, None, false)
}

/// Like [`connect_remote`], but cap SSH `ConnectTimeout` and the preamble read to `budget` so a
/// reconnect attempt cannot outrun the advertised retry window.
pub(crate) fn connect_remote_within(
    target: &RemoteTarget,
    session: &str,
    config: &RemoteConfig,
    budget: Option<Duration>,
    cancel_epoch: Option<u64>,
    recover_existing: bool,
) -> Result<(IpcConnection, RemotePreamble), RemoteConnectError> {
    if !crate::session::discovery::valid_attach_target(session) {
        return Err(RemoteConnectError::Message(
            "invalid session name".to_string(),
        ));
    }
    validate_remote_target(target).map_err(RemoteConnectError::Message)?;
    let resolved = ResolvedRemote::resolve(target, config);
    if !program_exists("ssh") {
        return Err(RemoteConnectError::Message(
            "ssh was not found on PATH (required for --remote)".to_string(),
        ));
    }

    let remote_bin = resolve_attach_binary(target, config, budget, cancel_epoch)?;
    validate_remote_executable_token(&remote_bin.path).map_err(RemoteConnectError::Message)?;

    let proxy = spawn_remote_proxy(
        &resolved,
        session,
        &remote_bin,
        config,
        budget,
        cancel_epoch,
        recover_existing,
    )?;
    let (conn, preamble) =
        read_remote_preamble(proxy, target, &resolved, config, budget, cancel_epoch)?;
    validate_remote_preamble(&preamble)?;
    Ok((conn, preamble))
}

struct SpawnedRemoteProxy {
    connection: IpcConnection,
    stderr_tail: Option<thread::JoinHandle<String>>,
    started: Instant,
}

fn spawn_remote_proxy(
    resolved: &ResolvedRemote,
    session: &str,
    remote_bin: &super::binary::RemoteBinary,
    config: &RemoteConfig,
    budget: Option<Duration>,
    cancel_epoch: Option<u64>,
    recover_existing: bool,
) -> Result<SpawnedRemoteProxy, RemoteConnectError> {
    if budget.is_some_and(|remaining| remaining.is_zero()) {
        return Err(RemoteConnectError::Message(
            "reconnect deadline elapsed".to_string(),
        ));
    }

    let started = Instant::now();
    // Keepalive comes from `ssh_base_command` now: with connection multiplexing the master decides
    // it for every client riding on it, so it has to be set wherever the master might be opened.
    let mut command = match budget {
        Some(remaining) => ssh_base_command_with_connect_timeout(
            resolved,
            config,
            capped_connect_timeout_secs(config, remaining),
        ),
        None => ssh_base_command(resolved, config),
    };
    if let Some(epoch) = cancel_epoch {
        super::askpass::scope_attach(&mut command, epoch);
    }
    append_ssh_destination(&mut command, resolved);
    let serve_flag = if recover_existing {
        "--remote-serve-existing"
    } else {
        "--remote-serve"
    };
    append_remote_rozi_command(
        &mut command,
        &remote_bin.path,
        &[serve_flag, session],
        remote_bin.family,
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|err| {
        RemoteConnectError::Message(format!("failed to spawn ssh to {}: {err}", resolved.host))
    })?;
    let stderr_tail = child.stderr.take().map(spawn_stderr_collector);
    let connection = ipc::connection_from_child(child)?;
    Ok(SpawnedRemoteProxy {
        connection,
        stderr_tail,
        started,
    })
}

fn read_remote_preamble(
    proxy: SpawnedRemoteProxy,
    target: &RemoteTarget,
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    budget: Option<Duration>,
    cancel_epoch: Option<u64>,
) -> Result<(IpcConnection, RemotePreamble), RemoteConnectError> {
    let SpawnedRemoteProxy {
        mut connection,
        stderr_tail,
        started,
    } = proxy;
    let preamble_wait = match budget {
        Some(remaining) => {
            capped_preamble_timeout(config, remaining.saturating_sub(started.elapsed()))
        }
        None => preamble_timeout(config),
    };
    let preamble_deadline = Instant::now() + preamble_wait;
    let poll = preamble_wait
        .min(Duration::from_millis(200))
        .max(Duration::from_millis(1));
    let _ = connection.set_read_timeout(Some(poll));
    let preamble = match preamble::read_preamble(&mut DeadlineReader {
        inner: &mut connection,
        deadline: preamble_deadline,
        cancel_epoch,
    }) {
        Ok(preamble) => preamble,
        Err(err) => {
            // A quiet or dropped SSH pipe is not evidence the remote binary disappeared.
            // Reconnect must not flush the remembered path and spend the retry window re-probing.
            if invalidate_binary_cache_after_preamble_failure(budget) {
                super::binary::invalidate(target, config);
            }
            // Kill the proxy, but never wait for its stderr collector here. An askpass helper can
            // outlive the killed ssh process while it waits for the UI and keep the inherited
            // stderr pipe open; joining it would let that helper outrun the reconnect deadline.
            let _ = connection.shutdown(std::net::Shutdown::Both);
            drop(connection);
            let detail = stderr_tail
                .and_then(finished_stderr_tail)
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!(" ({})", s.trim()))
                .unwrap_or_default();
            return Err(RemoteConnectError::Message(format!(
                "remote proxy on {} did not send a valid preamble: {err}{detail}",
                resolved.host
            )));
        }
    };
    let _ = connection.set_read_timeout(None);
    Ok((connection, preamble))
}

fn validate_remote_preamble(preamble: &RemotePreamble) -> Result<(), RemoteConnectError> {
    if let Err(message) = preamble.validate_for_client() {
        let lowered = message.to_ascii_lowercase();
        return if lowered.contains("protocol") || lowered.contains("incompatible") {
            Err(RemoteConnectError::ProtocolSkew(message))
        } else {
            Err(RemoteConnectError::Message(message))
        };
    }
    Ok(())
}

struct DeadlineReader<'a> {
    inner: &'a mut IpcConnection,
    deadline: Instant,
    cancel_epoch: Option<u64>,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self
                .cancel_epoch
                .is_some_and(crate::session::bootstrap::remote_attach_cancelled)
            {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "reconnect cancelled",
                ));
            }
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "remote preamble deadline elapsed",
                ));
            }
            self.inner.set_read_timeout(Some(
                remaining
                    .min(Duration::from_millis(200))
                    .max(Duration::from_millis(1)),
            ))?;
            match self.inner.read(buf) {
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                result => return result,
            }
        }
    }
}

fn resolve_attach_binary(
    target: &RemoteTarget,
    config: &RemoteConfig,
    budget: Option<Duration>,
    cancel_epoch: Option<u64>,
) -> Result<super::binary::RemoteBinary, RemoteConnectError> {
    if budget.is_some() {
        if cancel_epoch.is_some_and(crate::session::bootstrap::remote_attach_cancelled) {
            return Err(RemoteConnectError::Message(
                "reconnect cancelled".to_string(),
            ));
        }
        if budget.is_some_and(|remaining| remaining.is_zero()) {
            return Err(RemoteConnectError::Message(
                "reconnect deadline elapsed".to_string(),
            ));
        }
        // Reconnect uses the path that already worked. A dead transport is not a missing
        // binary. Never fall back to a multi-hop probe here: it can outlive the reconnect budget,
        // and an established attachment always remembered the path that launched its proxy.
        if let Some(path) = super::binary::last_known(target, config) {
            return Ok(path);
        }
        let resolved = ResolvedRemote::resolve(target, config);
        if resolved.binary_path.is_some() {
            return super::binary::resolve(target, config).map_err(RemoteConnectError::Message);
        }
        return Err(RemoteConnectError::Message(
            "remote Rozi path is no longer known; reopen the host to probe it again".to_string(),
        ));
    }
    let path = if super::askpass::may_prompt() {
        super::ensure_remote_binary_in_ui(target, config, None)
    } else {
        super::ensure_remote_binary(target, config, false)
    }
    .map_err(RemoteConnectError::Message)?;
    let binary = super::binary::resolve(target, config).map_err(RemoteConnectError::Message)?;
    if binary.path != path {
        return Err(RemoteConnectError::Message(
            "remote Rozi path changed while preparing the connection; retry".into(),
        ));
    }
    Ok(binary)
}

/// Default wait for the proxy's first bytes when `[remote] connection_timeout_secs` is `0`.
const DEFAULT_PREAMBLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Extra budget for a connection that may still have to be authenticated by hand.
///
/// `ssh` authenticates before the remote proxy can say anything, so a password typed into the
/// in-app prompt is spent *inside* this read. Charging the user's typing to the remote's latency is
/// what turns a slow password into "did not send a valid preamble". Generous, because the wait ends
/// the moment the answer is given — and because a genuinely dead remote closes the pipe rather than
/// going quiet, which fails immediately whatever this says.
const INTERACTIVE_AUTH_ALLOWANCE: Duration = Duration::from_secs(180);

/// How long to wait for the proxy's preamble.
fn preamble_timeout(config: &RemoteConfig) -> Duration {
    preamble_timeout_for(config, super::askpass::may_prompt())
}

fn preamble_timeout_for(config: &RemoteConfig, interactive: bool) -> Duration {
    let base = if config.connection_timeout_secs > 0 {
        Duration::from_secs(config.connection_timeout_secs.max(1))
    } else {
        DEFAULT_PREAMBLE_TIMEOUT
    };
    if interactive && !config.batch_mode {
        base + INTERACTIVE_AUTH_ALLOWANCE
    } else {
        base
    }
}

fn capped_connect_timeout_secs(config: &RemoteConfig, remaining: Duration) -> u64 {
    let remaining_secs = remaining.as_secs().max(1);
    if config.connection_timeout_secs > 0 {
        config.connection_timeout_secs.min(remaining_secs)
    } else {
        remaining_secs
    }
}

fn capped_preamble_timeout(config: &RemoteConfig, remaining: Duration) -> Duration {
    if remaining.is_zero() {
        Duration::from_millis(1)
    } else {
        preamble_timeout(config).min(remaining)
    }
}

/// A quiet SSH pipe during reconnect is not evidence the remote binary disappeared.
fn invalidate_binary_cache_after_preamble_failure(budget: Option<Duration>) -> bool {
    budget.is_none()
}

/// Kill a named session on the remote host via `rozi sessions kill` over ssh.
pub fn kill_remote_session(
    target: &RemoteTarget,
    session: &str,
    config: &RemoteConfig,
) -> Result<(), String> {
    if !crate::session::discovery::valid_attach_target(session) {
        return Err("invalid session name".to_string());
    }
    validate_remote_target(target)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let remote_bin = super::binary::resolve(target, config)?;
    validate_remote_executable_token(&remote_bin.path)?;
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    append_remote_rozi_command(
        &mut command,
        &remote_bin.path,
        &["sessions", "kill", session],
        remote_bin.family,
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|err| format!("remote sessions kill ssh failed: {err}"))?;
    if !output.status.success() {
        super::binary::invalidate(target, config);
        return Err(super::sessions_command_failure(
            "kill",
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    Ok(())
}

pub(super) fn spawn_stderr_collector(mut stderr: ChildStderr) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = String::new();
        let mut bytes = [0u8; 4096];
        // Cap so a verbose ssh cannot grow without bound.
        const CAP: usize = 16 * 1024;
        while buf.len() < CAP {
            match stderr.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    let take = n.min(CAP - buf.len());
                    buf.push_str(&String::from_utf8_lossy(&bytes[..take]));
                }
                Err(_) => break,
            }
        }
        buf
    })
}

fn finished_stderr_tail(handle: thread::JoinHandle<String>) -> Option<String> {
    handle.is_finished().then(|| handle.join().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;

    #[test]
    fn kill_remote_session_rejects_hostile_session_before_spawning_ssh() {
        let target = RemoteTarget::Alias("workbox".to_string());
        let config = RemoteConfig::default();
        for session in ["dev;touch /tmp/pwned", "dev\nnext", "dev\u{1b}[31m"] {
            let error = kill_remote_session(&target, session, &config)
                .expect_err("hostile session must be rejected before ssh");
            assert_eq!(error, "invalid session name");
        }
    }

    #[test]
    fn kill_remote_session_rejects_control_characters_in_configured_executable_before_ssh() {
        let target = RemoteTarget::Alias("workbox".to_string());
        let mut config = RemoteConfig::default();
        config.hosts.insert(
            "workbox".to_string(),
            crate::config::RemoteHostConfig {
                binary_path: Some("rozi\nnext".to_string()),
                ..crate::config::RemoteHostConfig::default()
            },
        );
        let error = kill_remote_session(&target, "dev", &config)
            .expect_err("control characters must be rejected before ssh");
        assert!(error.contains("control characters"), "{error}");
    }

    #[test]
    fn reconnect_budget_caps_ssh_connect_and_preamble_timeouts() {
        let config = RemoteConfig::default();
        assert_eq!(
            capped_connect_timeout_secs(&config, Duration::from_secs(120)),
            15
        );
        assert_eq!(
            capped_connect_timeout_secs(&config, Duration::from_secs(5)),
            5
        );
        assert_eq!(
            capped_preamble_timeout(&config, Duration::from_secs(120)),
            Duration::from_secs(15)
        );
        assert_eq!(
            capped_preamble_timeout(&config, Duration::from_secs(5)),
            Duration::from_secs(5)
        );
        assert_eq!(
            capped_preamble_timeout(&config, Duration::ZERO),
            Duration::from_millis(1)
        );

        let interactive = RemoteConfig {
            batch_mode: false,
            ..RemoteConfig::default()
        };
        let remaining = Duration::from_secs(120);
        assert_eq!(
            preamble_timeout_for(&interactive, true),
            Duration::from_secs(15) + INTERACTIVE_AUTH_ALLOWANCE
        );
        let requested = preamble_timeout_for(&interactive, true);
        assert_eq!(
            requested.min(remaining),
            remaining,
            "interactive auth allowance must not outrun the reconnect deadline"
        );
    }

    #[test]
    fn reconnect_preamble_failure_keeps_the_remembered_binary() {
        assert!(!invalidate_binary_cache_after_preamble_failure(Some(
            Duration::from_secs(30)
        )));
        assert!(invalidate_binary_cache_after_preamble_failure(None));
    }

    #[test]
    fn preamble_reader_enforces_one_deadline_across_poll_timeouts() {
        let (reader, writer) = std::io::pipe().unwrap();
        let piped =
            crate::platform::ipc::PipedConnection::from_reader_writer(std::io::sink(), reader);
        let mut connection = IpcConnection::from_piped(piped);
        let started = Instant::now();
        let error = DeadlineReader {
            inner: &mut connection,
            deadline: started + Duration::from_millis(30),
            cancel_epoch: None,
        }
        .read(&mut [0u8; 1])
        .expect_err("a silent pipe must hit the shared preamble deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(writer);
    }

    #[test]
    fn reconnect_failure_does_not_wait_for_an_inherited_stderr_pipe() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            release_rx.recv().unwrap();
            "late stderr".to_string()
        });

        assert_eq!(finished_stderr_tail(handle), None);
        release_tx.send(()).unwrap();
    }
}
