//! Remote SSH session attach (`--remote`).
//!
//! The local client speaks the normal session protocol over a pipe to
//! `ssh … hyprmux --remote-serve <NAME>`, which proxies to the remote host's local session
//! endpoint. The session server itself is unchanged.

mod bootstrap;
mod connect;
mod preamble;
mod proxy;
mod target;

pub use bootstrap::ensure_remote_binary;
pub(crate) use bootstrap::ssh_base_command;
#[allow(unused_imports)] // re-exported for callers/tests
pub use connect::RemoteConnectError;
pub use connect::{connect_remote, kill_remote_session};
#[allow(unused_imports)] // public API surface for remote attach callers
pub use preamble::RemotePreamble;
pub use proxy::run_remote_serve;
pub use target::{RemoteTarget, parse_remote_target};

use crate::config::{HyprmuxRemoteConfig, RemoteHostConfig};

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
    pub fn resolve(target: &RemoteTarget, config: &HyprmuxRemoteConfig) -> Self {
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
