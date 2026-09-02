//! Remote SSH session attach (`--remote`).
//!
//! The local client speaks the normal session protocol over a pipe to
//! `ssh … rozi --remote-serve <NAME>`, which proxies to the remote host's local session
//! endpoint. The session server itself is unchanged.

pub mod askpass;
mod bootstrap;
mod connect;
mod preamble;
mod proxy;
mod target;

pub use askpass::AskpassKind;
pub use bootstrap::ensure_remote_binary;
pub(crate) use bootstrap::{append_ssh_destination, ssh_base_command};
#[allow(unused_imports)] // re-exported for callers/tests
pub use connect::RemoteConnectError;
pub use connect::{connect_remote, kill_remote_session};
#[allow(unused_imports)] // public API surface for remote attach callers and proxy tests
pub use preamble::{RemotePreamble, read_preamble};
pub use proxy::run_remote_serve;
pub use target::{RemoteTarget, parse_remote_target};
pub(crate) use target::{validate_remote_executable_token, validate_remote_target};

use crate::config::{RemoteConfig, RemoteHostConfig};

/// Format a failed namespaced session command, translating an old remote parser's diagnostics into
/// a version-skew message while preserving stderr for unrelated failures.
pub(crate) fn sessions_command_failure(verb: &str, stderr: &str) -> String {
    let stderr = stderr.trim();
    let parser_rejected_namespace = [
        "unknown flag",
        "unexpected argument",
        "No session or profile named",
    ]
    .iter()
    .any(|marker| stderr.contains(marker));

    if parser_rejected_namespace {
        format!(
            "remote sessions {verb} failed: the remote rozi is older than 0.0.16 and does not understand `rozi sessions {verb}`"
        )
    } else {
        format!("remote sessions {verb} failed: {stderr}")
    }
}

/// Resolved SSH destination after merging CLI target with `[remote]` / `[remote.hosts.*]` config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRemote {
    pub alias: Option<String>,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub ssh_args: Vec<String>,
    pub binary_path: Option<String>,
}

impl ResolvedRemote {
    pub fn resolve(target: &RemoteTarget, config: &RemoteConfig) -> Self {
        let default_cfg = config
            .default_host
            .as_ref()
            .and_then(|name| config.hosts.get(name));
        let alias_key = match target {
            RemoteTarget::Alias(alias) => Some(alias.as_str()),
            RemoteTarget::Url { .. } => None,
        };
        let host_cfg: Option<&RemoteHostConfig> =
            alias_key.and_then(|alias| config.hosts.get(alias));

        match target {
            RemoteTarget::Alias(alias) => {
                let host = host_cfg
                    .and_then(|h| h.host.clone())
                    .unwrap_or_else(|| alias.clone());
                Self {
                    alias: Some(alias.clone()),
                    host,
                    user: host_cfg
                        .and_then(|h| h.user.clone())
                        .or_else(|| default_cfg.and_then(|h| h.user.clone())),
                    port: host_cfg
                        .and_then(|h| h.port)
                        .or_else(|| default_cfg.and_then(|h| h.port)),
                    identity_file: host_cfg
                        .and_then(|h| h.identity_file.clone())
                        .or_else(|| default_cfg.and_then(|h| h.identity_file.clone())),
                    ssh_args: host_cfg
                        .map(|h| h.ssh_args.clone())
                        .filter(|args| !args.is_empty())
                        .or_else(|| default_cfg.map(|h| h.ssh_args.clone()))
                        .unwrap_or_default(),
                    binary_path: host_cfg
                        .and_then(|h| h.binary_path.clone())
                        .or_else(|| default_cfg.and_then(|h| h.binary_path.clone())),
                }
            }
            RemoteTarget::Url { user, host, port } => {
                // URL fields win; fall back to a matching `[remote.hosts.<host>]` entry, then
                // `[remote] default_host`'s host table, for ssh_args / identity / binary.
                let host_cfg = config.hosts.get(host).or(default_cfg);
                Self {
                    alias: None,
                    host: host.clone(),
                    user: user
                        .clone()
                        .or_else(|| host_cfg.and_then(|h| h.user.clone())),
                    port: port.or_else(|| host_cfg.and_then(|h| h.port)),
                    identity_file: host_cfg.and_then(|h| h.identity_file.clone()),
                    ssh_args: host_cfg.map(|h| h.ssh_args.clone()).unwrap_or_default(),
                    binary_path: host_cfg.and_then(|h| h.binary_path.clone()),
                }
            }
        }
    }

    pub fn ssh_destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sessions_command_failure;

    #[test]
    fn old_remote_parser_errors_become_version_skew_errors() {
        for stderr in [
            "unknown flag `--format`",
            "unexpected argument `list`",
            "No session or profile named `sessions`.",
        ] {
            assert_eq!(
                sessions_command_failure("list", stderr),
                "remote sessions list failed: the remote rozi is older than 0.0.16 and does not understand `rozi sessions list`"
            );
        }
    }

    #[test]
    fn unrelated_remote_failures_keep_raw_stderr() {
        assert_eq!(
            sessions_command_failure("kill", " ssh: Connection refused\n"),
            "remote sessions kill failed: ssh: Connection refused"
        );
    }
}
