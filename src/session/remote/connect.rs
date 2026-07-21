//! Local client: spawn `ssh … hyprmux --remote-serve` and return a Piped connection.

use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::HyprmuxRemoteConfig;
use crate::platform::command::program_exists;
use crate::platform::ipc::{self, IpcConnection};

use super::preamble::{self, RemotePreamble};
use super::{RemoteTarget, ResolvedRemote};

#[derive(Debug)]
pub enum RemoteConnectError {
    Io(io::Error),
    Message(String),
}

impl std::fmt::Display for RemoteConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Message(msg) => write!(f, "{msg}"),
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
pub fn connect_remote(
    target: &RemoteTarget,
    session: &str,
    config: &HyprmuxRemoteConfig,
) -> Result<(IpcConnection, RemotePreamble), RemoteConnectError> {
    let resolved = ResolvedRemote::resolve(target, config);
    if !program_exists("ssh") {
        return Err(RemoteConnectError::Message(
            "ssh was not found on PATH (required for --remote)".to_string(),
        ));
    }

    let remote_bin = resolved
        .binary_path
        .clone()
        .unwrap_or_else(|| "hyprmux".to_string());

    let mut command = Command::new("ssh");
    command.arg("-T"); // no tty — raw protocol bytes on the pipe
    if let Some(port) = resolved.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &resolved.identity_file {
        command.arg("-i").arg(crate::config::expand_path(identity));
    }
    for arg in &resolved.ssh_args {
        command.arg(arg);
    }
    // Fail fast into SessionDisconnected rather than hanging on a dropped link.
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
        ))
        .arg("-o")
        .arg("BatchMode=yes");
    if config.connection_timeout_secs > 0 {
        command
            .arg("-o")
            .arg(format!("ConnectTimeout={}", config.connection_timeout_secs));
    }
    command.arg(resolved.ssh_destination());
    command.arg("--");
    command.arg(&remote_bin);
    command.arg("--remote-serve");
    command.arg(session);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command.spawn().map_err(|err| {
        RemoteConnectError::Message(format!("failed to spawn ssh to {}: {err}", resolved.host))
    })?;

    let mut conn = ipc::connection_from_child(child)?;
    if config.connection_timeout_secs > 0 {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(
            config.connection_timeout_secs.max(1),
        )));
    } else {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(15)));
    }
    let preamble = preamble::read_preamble(&mut conn).map_err(|err| {
        RemoteConnectError::Message(format!(
            "remote proxy on {} did not send a valid preamble: {err}",
            resolved.host
        ))
    })?;
    let _ = conn.set_read_timeout(None);
    preamble
        .validate_for_client()
        .map_err(RemoteConnectError::Message)?;
    Ok((conn, preamble))
}
