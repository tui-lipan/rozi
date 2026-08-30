//! The host dimension of the unified Sessions view.
//!
//! A remote host is *known* — and therefore listed in the Sessions view even while disconnected —
//! when it is configured (`[remote.hosts.*]` or `default_host`), a recently used ad-hoc `--remote`
//! target, or currently/previously attached. Keeping known hosts visible is the whole point: a
//! user's remote workplaces are locations they return to, so a host must not vanish from the tree
//! just because its link is down or it happens to have no live sessions right now.
//!
//! The registry owns only the *host-level* connection state that has to persist across the recurring
//! session sweep — whether each host is connected/connecting and the last probe error. A host's
//! display *status* is derived from that plus the live attachments at render time (see
//! [`HostRegistry::status_for`]).

use crate::config::RemoteConfig;
use crate::session::remote::{RemoteTarget, parse_remote_target};

/// Where a known host came from. Ordered so configured hosts sort ahead of ad-hoc recents, which in
/// turn sort ahead of hosts we only know because something is attached to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostOrigin {
    Configured,
    Recent,
    Attached,
}

/// One known remote host in the Sessions view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    /// Short display label: the config alias, or the target's display form for an ad-hoc URL.
    pub alias: String,
    /// Exact SSH endpoint. The registry key — user/port distinctions are preserved so
    /// `dev@box:22` and `box` never collapse into one row.
    pub target: RemoteTarget,
    pub origin: HostOrigin,
    /// Live connection state for the host, driving its status (Online / Offline / Connecting) and
    /// any inline error. A host is *connected* — its sessions are listed and kept fresh — while this
    /// is `InFlight`/`Reached`; activating an offline host row moves it there and the host row's ✕
    /// returns
    /// it to `Idle`.
    pub probe: HostProbe,
}

/// The state of this client's link to a host: what connecting/disconnecting and the on-demand probe
/// move through. Distinct from any *session* attachment on the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HostProbe {
    /// Disconnected — never contacted, or explicitly disconnected.
    #[default]
    Idle,
    /// A probe is in flight because the user just connected the host.
    InFlight,
    /// The last probe reached the host.
    Reached,
    /// The last probe failed; carries the reason to surface on the host header.
    Failed(String),
}

impl HostProbe {
    /// The failure reason, if the last probe failed.
    pub fn error(&self) -> Option<&str> {
        match self {
            HostProbe::Failed(message) => Some(message),
            _ => None,
        }
    }
}

/// Derived, render-time connection status for a host. Never stored — computed from the live
/// attachments on the host plus its probe state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus {
    /// At least one attachment on this host is live.
    Connected,
    /// An attachment on this host is (re)connecting, or the host is being probed (just connected).
    Connecting,
    /// Reachable — the probe reached the host (whether or not it has sessions) — but this client
    /// holds no attachment.
    Reachable,
    /// Collapsed / not contacted: no attachment, no live probe.
    Disconnected,
    /// The last probe/connect attempt failed.
    Unreachable,
}

/// The set of known remote hosts: configured aliases alphabetically, recents in MRU order, then
/// attached-only hosts alphabetically.
#[derive(Clone, Debug, Default)]
pub struct HostRegistry {
    entries: Vec<HostEntry>,
}

impl HostRegistry {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &HostEntry> {
        self.entries.iter()
    }

    pub fn get(&self, target: &RemoteTarget) -> Option<&HostEntry> {
        self.entries.iter().find(|entry| &entry.target == target)
    }

    pub fn get_mut(&mut self, target: &RemoteTarget) -> Option<&mut HostEntry> {
        self.entries
            .iter_mut()
            .find(|entry| &entry.target == target)
    }

    /// Rebuild the known-host set from its three sources, preserving each surviving host's `probe`
    /// (connection) state. Called whenever the Sessions view opens or refreshes.
    ///
    /// - `remote_config`: configured `[remote.hosts.*]` aliases and `default_host`.
    /// - `recents`: recently used remote targets, most-recent first.
    /// - `held`: `(target, alias)` for every host a live attachment currently targets.
    ///
    /// A host present in more than one source keeps its strongest origin (Configured wins over
    /// Recent wins over Attached) so it renders and sorts as the more permanent thing it is.
    pub fn seed(
        &mut self,
        remote_config: &RemoteConfig,
        recents: &[RemoteTarget],
        held: &[(RemoteTarget, String)],
    ) {
        let mut rebuilt: Vec<HostEntry> = Vec::new();

        let mut upsert = |target: RemoteTarget, alias: String, origin: HostOrigin| {
            if let Some(existing) = rebuilt.iter_mut().find(|entry| entry.target == target) {
                // Strongest origin wins; keep the better display alias that came with it.
                if origin < existing.origin {
                    existing.origin = origin;
                    existing.alias = alias;
                }
                return;
            }
            rebuilt.push(HostEntry {
                alias,
                target,
                origin,
                // Preserved below from the prior entry if the host survives.
                probe: HostProbe::Idle,
            });
        };

        // Configured aliases first (highest-priority origin), including the default host.
        let mut aliases: Vec<String> = remote_config.hosts.keys().cloned().collect();
        if let Some(default_host) = &remote_config.default_host
            && !aliases.iter().any(|alias| alias == default_host)
        {
            aliases.push(default_host.clone());
        }
        aliases.sort();
        for alias in aliases {
            if let Ok(target) = parse_remote_target(&alias) {
                upsert(target, alias, HostOrigin::Configured);
            }
        }

        for target in recents {
            upsert(target.clone(), target.display_label(), HostOrigin::Recent);
        }

        for (target, alias) in held {
            upsert(target.clone(), alias.clone(), HostOrigin::Attached);
        }

        // Carry over each surviving host's connection state across the refresh; a host appearing for
        // the first time starts disconnected (a launch contacts nothing until the user connects it).
        for entry in &mut rebuilt {
            if let Some(prior) = self.entries.iter().find(|old| old.target == entry.target) {
                entry.probe = prior.probe.clone();
            }
        }

        // Configured entries were inserted alphabetically and recents in persisted MRU order.
        // Only the attached-only tail needs normalizing: sorting the whole registry would destroy
        // the order that makes Recent useful.
        let attached_start = rebuilt.partition_point(|entry| entry.origin != HostOrigin::Attached);
        rebuilt[attached_start..].sort_by(|a, b| a.alias.cmp(&b.alias));
        self.entries = rebuilt;
    }

    /// Derive the display status of a host from `connections` — the connection state of every live
    /// attachment on this host — plus its probe state and whether discovery lists any session on it.
    /// A live/connecting attachment wins over everything (a reconnect that succeeds clears an
    /// "unreachable" look without the registry being told); otherwise the probe decides: in-flight
    /// reads as *connecting*, a reached host (even with zero sessions) as *reachable*, a failed
    /// probe as *unreachable*, and an idle host as *disconnected*.
    pub fn status_for<'a>(
        &self,
        target: &RemoteTarget,
        connections: impl IntoIterator<Item = &'a super::ConnectionState>,
        has_sessions: bool,
    ) -> HostStatus {
        let mut any_connecting = false;
        for connection in connections {
            match connection {
                super::ConnectionState::Connected => return HostStatus::Connected,
                super::ConnectionState::Connecting | super::ConnectionState::Reconnecting => {
                    any_connecting = true;
                }
                _ => {}
            }
        }
        if any_connecting {
            return HostStatus::Connecting;
        }
        match self.get(target).map(|entry| &entry.probe) {
            Some(HostProbe::InFlight) => HostStatus::Connecting,
            Some(HostProbe::Failed(_)) => HostStatus::Unreachable,
            Some(HostProbe::Reached) => HostStatus::Reachable,
            _ if has_sessions => HostStatus::Reachable,
            _ => HostStatus::Disconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteHostConfig;

    fn config(hosts: &[&str], default_host: Option<&str>) -> RemoteConfig {
        let mut cfg = RemoteConfig::default();
        for host in hosts {
            cfg.hosts
                .insert((*host).to_string(), RemoteHostConfig::default());
        }
        cfg.default_host = default_host.map(str::to_string);
        cfg
    }

    #[test]
    fn seeds_from_config_recents_and_held_without_duplicates() {
        let mut registry = HostRegistry::default();
        registry.seed(
            &config(&["workbox"], Some("prod")),
            &[
                RemoteTarget::Alias("scratch".into()),
                RemoteTarget::Alias("workbox".into()),
            ],
            &[(RemoteTarget::Alias("adhoc".into()), "adhoc".into())],
        );
        let aliases: Vec<&str> = registry.iter().map(|entry| entry.alias.as_str()).collect();
        // Configured (prod, workbox) sort ahead of recents (scratch) ahead of attached (adhoc);
        // `workbox` appears once, keeping its Configured origin despite also being a recent.
        assert_eq!(aliases, vec!["prod", "workbox", "scratch", "adhoc"]);
        assert_eq!(
            registry
                .get(&RemoteTarget::Alias("workbox".into()))
                .unwrap()
                .origin,
            HostOrigin::Configured
        );
    }

    #[test]
    fn reseed_preserves_probe_state_for_surviving_hosts() {
        let mut registry = HostRegistry::default();
        registry.seed(&config(&["workbox"], None), &[], &[]);
        let target = RemoteTarget::Alias("workbox".into());
        registry.get_mut(&target).unwrap().probe = HostProbe::Failed("timed out".to_string());
        // A recent is added, but workbox survives and keeps its connection state.
        registry.seed(
            &config(&["workbox"], None),
            &[RemoteTarget::Alias("scratch".into())],
            &[],
        );
        let entry = registry.get(&target).unwrap();
        assert_eq!(entry.probe.error(), Some("timed out"));
        // A host that dropped out of every source is gone.
        registry.seed(&config(&[], None), &[], &[]);
        assert!(registry.is_empty());
    }

    #[test]
    fn status_reflects_attachments_then_probe_state() {
        let mut registry = HostRegistry::default();
        registry.seed(&config(&["workbox"], None), &[], &[]);
        let target = RemoteTarget::Alias("workbox".into());
        registry.get_mut(&target).unwrap().probe = HostProbe::Failed("was down".to_string());

        // Failed probe + no attachment → unreachable, even with sessions listed.
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), true),
            HostStatus::Unreachable
        );
        // A connecting attachment overrides the failed probe.
        assert_eq!(
            registry.status_for(&target, &[super::super::ConnectionState::Connecting], false),
            HostStatus::Connecting
        );
        // A connected attachment wins outright.
        assert_eq!(
            registry.status_for(
                &target,
                &[
                    super::super::ConnectionState::Reconnecting,
                    super::super::ConnectionState::Connected,
                ],
                false,
            ),
            HostStatus::Connected
        );

        // An in-flight probe reads as connecting.
        registry.get_mut(&target).unwrap().probe = HostProbe::InFlight;
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), false),
            HostStatus::Connecting
        );
        // A reached host is online even with zero sessions.
        registry.get_mut(&target).unwrap().probe = HostProbe::Reached;
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), false),
            HostStatus::Reachable
        );
        // Idle + no sessions → disconnected; idle + sessions listed → reachable.
        registry.get_mut(&target).unwrap().probe = HostProbe::Idle;
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), false),
            HostStatus::Disconnected
        );
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), true),
            HostStatus::Reachable
        );
    }

    #[test]
    fn configured_is_alphabetical_while_recent_preserves_mru() {
        let mut registry = HostRegistry::default();
        registry.seed(
            &config(&["zeta", "alpha"], None),
            &[
                RemoteTarget::Alias("recent-z".into()),
                RemoteTarget::Alias("recent-a".into()),
            ],
            &[
                (RemoteTarget::Alias("held-z".into()), "held-z".into()),
                (RemoteTarget::Alias("held-a".into()), "held-a".into()),
            ],
        );
        let aliases = registry
            .iter()
            .map(|entry| entry.alias.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            aliases,
            vec!["alpha", "zeta", "recent-z", "recent-a", "held-a", "held-z"]
        );
    }

    #[test]
    fn target_keeps_its_strongest_origin_without_duplication() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut registry = HostRegistry::default();
        registry.seed(
            &config(&["workbox"], None),
            std::slice::from_ref(&target),
            &[(target.clone(), "workbox".into())],
        );
        assert_eq!(registry.iter().count(), 1);
        assert_eq!(
            registry.get(&target).unwrap().origin,
            HostOrigin::Configured
        );
    }
}
