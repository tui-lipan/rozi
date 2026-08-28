//! Local client: spawn `ssh … rozi --remote-serve` and return a Piped connection.

use std::io::{self, Read};
use std::process::{ChildStderr, Stdio};
use std::thread;
use std::time::Duration;

use crate::config::RemoteConfig;
use crate::platform::command::program_exists;
use crate::platform::ipc::{self, IpcConnection};

use super::bootstrap::{append_ssh_destination, ssh_base_command};
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
/// `ensure_remote_binary` should already have been run before the TUI starts when install policy
/// is `prompt`; this call re-checks and will not prompt again if a compatible
/// binary is already present.
pub fn connect_remote(
    target: &RemoteTarget,
    session: &str,
    config: &RemoteConfig,
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

    // Re-probe/install only when needed. Prompting here is a last resort (caller should have
    // prompted pre-TUI); interactive=false avoids a silent install on the attach thread.
    let remote_bin =
        super::ensure_remote_binary(target, config, false).map_err(RemoteConnectError::Message)?;
    validate_remote_executable_token(&remote_bin).map_err(RemoteConnectError::Message)?;

    let mut command = ssh_base_command(&resolved, config);
    command
        .arg("-o")
        .arg(format!(
            "ServerAliveInterval={}",
            config.server_alive_interval_secs
        ))
        .arg("-o")
        .arg(format!(
            "ServerAliveCountMax={}",
            config.server_alive_count_max
        ));
    append_ssh_destination(&mut command, &resolved);
    command.arg(&remote_bin);
    command.arg("--remote-serve");
    command.arg(session);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|err| {
        RemoteConnectError::Message(format!("failed to spawn ssh to {}: {err}", resolved.host))
    })?;

    let stderr_tail = child.stderr.take().map(spawn_stderr_collector);

    let mut conn = ipc::connection_from_child(child)?;
    if config.connection_timeout_secs > 0 {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(
            config.connection_timeout_secs.max(1),
        )));
    } else {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(15)));
    }
    let preamble = match preamble::read_preamble(&mut conn) {
        Ok(preamble) => preamble,
        Err(err) => {
            // Kill the proxy before joining stderr. The collector reaches EOF only after the child
            // exits, while the child is owned by this connection.
            let _ = conn.shutdown(std::net::Shutdown::Both);
            drop(conn);
            let detail = stderr_tail
                .and_then(|handle| handle.join().ok())
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!(" ({})", s.trim()))
                .unwrap_or_default();
            return Err(RemoteConnectError::Message(format!(
                "remote proxy on {} did not send a valid preamble: {err}{detail}",
                resolved.host
            )));
        }
    };
    let _ = conn.set_read_timeout(None);
    if let Err(message) = preamble.validate_for_client() {
        if message.to_ascii_lowercase().contains("protocol")
            || message.to_ascii_lowercase().contains("incompatible")
        {
            return Err(RemoteConnectError::ProtocolSkew(message));
        }
        return Err(RemoteConnectError::Message(message));
    }
    Ok((conn, preamble))
}

/// Kill a named session on the remote host via `rozi kill-session` over ssh.
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
    let remote_bin = resolved
        .binary_path
        .clone()
        .or_else(|| {
            // Prefer a previously ensured path when present on PATH remotely.
            Some("rozi".to_string())
        })
        .unwrap_or_else(|| "rozi".to_string());
    validate_remote_executable_token(&remote_bin)?;
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    command.arg(&remote_bin);
    command.arg("kill-session");
    command.arg(session);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|err| format!("remote kill-session ssh failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "remote kill-session failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn spawn_stderr_collector(mut stderr: ChildStderr) -> thread::JoinHandle<String> {
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
    fn kill_remote_session_rejects_hostile_configured_executable_before_spawning_ssh() {
        let target = RemoteTarget::Alias("workbox".to_string());
        let mut config = RemoteConfig::default();
        config.hosts.insert(
            "workbox".to_string(),
            crate::config::RemoteHostConfig {
                binary_path: Some("rozi;touch".to_string()),
                ..crate::config::RemoteHostConfig::default()
            },
        );
        let error = kill_remote_session(&target, "dev", &config)
            .expect_err("hostile executable must be rejected before ssh");
        assert!(error.contains("shell metacharacters"), "{error}");
    }
}
