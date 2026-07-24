//! The host dimension of the unified Sessions view.
//!
//! A remote host is *known* — and therefore listed in the Sessions view even while disconnected —
//! when it is configured (`[remote.hosts.*]` or `default_host`), a recently used ad-hoc `--remote`
//! target, or currently/previously attached. Keeping known hosts visible is the whole point: a
//! user's remote workplaces are locations they return to, so a host must not vanish from the tree
//! just because its link is down or it happens to have no live sessions right now.
//!
//! The registry owns only the *host-level* state that has to persist across the recurring session
//! sweep — which group is expanded, and the last probe error to surface inline. A host's connection
//! *status* is derived from the live attachments plus that error at render time (see
//! [`HostRegistry::status_for`]); it is never stored, so it can never go stale behind the
//! attachments it describes.

use crate::config::HyprmuxRemoteConfig;
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
    /// Whether the group is expanded in the sidebar tree. Persisted across refreshes so a live
    /// session sweep never re-collapses a group the user opened.
    pub expanded: bool,
    /// Last discovery/connection error, surfaced on the group header instead of dropping the host.
    pub error: Option<String>,
}

/// Derived, render-time connection status for a host. Never stored — computed from the live
/// attachments on the host plus any recorded probe error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus {
    /// At least one attachment on this host is live.
    Connected,
    /// An attachment on this host is (re)connecting, or the host is being probed.
    Connecting,
    /// Reachable — discovery found sessions here — but this client holds no attachment.
    Reachable,
    /// Known but idle: no attachment, no listed sessions, no error.
    Disconnected,
    /// The last probe/connect attempt failed.
    Unreachable,
}

/// The set of known remote hosts, kept sorted by [`HostOrigin`] then alias so the tree order is
/// stable across refreshes.
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

    /// Rebuild the known-host set from its three sources, preserving per-host UI state
    /// (`expanded` / `error`) for hosts that survive. Called whenever the Sessions view opens or
    /// refreshes.
    ///
    /// - `remote_config`: configured `[remote.hosts.*]` aliases and `default_host`.
    /// - `recents`: recently used ad-hoc `--remote` target strings, most-recent first.
    /// - `held`: `(target, alias)` for every host a live attachment currently targets.
    ///
    /// A host present in more than one source keeps its strongest origin (Configured wins over
    /// Recent wins over Attached) so it renders and sorts as the more permanent thing it is.
    pub fn seed(
        &mut self,
        remote_config: &HyprmuxRemoteConfig,
        recents: &[String],
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
                expanded: false,
                error: None,
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

        for raw in recents {
            if let Ok(target) = parse_remote_target(raw) {
                let alias = target.display_label();
                upsert(target, alias, HostOrigin::Recent);
            }
        }

        for (target, alias) in held {
            upsert(target.clone(), alias.clone(), HostOrigin::Attached);
        }

        // Carry over per-host UI state for surviving hosts. A host appearing for the first time
        // starts collapsed — a launch must not fan out to every configured host — *unless* an
        // attachment already targets it, in which case it opens so the session you just connected to
        // is visible without a keystroke. A host the user has since collapsed stays collapsed across
        // the refresh; only its first appearance auto-expands.
        for entry in &mut rebuilt {
            match self.entries.iter().find(|old| old.target == entry.target) {
                Some(prior) => {
                    entry.expanded = prior.expanded;
                    entry.error = prior.error.clone();
                }
                None => {
                    entry.expanded = held.iter().any(|(target, _)| *target == entry.target);
                }
            }
        }

        rebuilt.sort_by(|a, b| a.origin.cmp(&b.origin).then_with(|| a.alias.cmp(&b.alias)));
        self.entries = rebuilt;
    }

    /// Derive the display status of a host from `connections` — the connection state of every live
    /// attachment on this host — plus whether discovery currently lists any session on it and the
    /// host's recorded probe error. A live/connecting attachment always wins over a stale error, so
    /// a reconnect that succeeds clears the "unreachable" look without the registry having to be
    /// told; a host with listed sessions but no attachment reads as *reachable*, distinct from both
    /// "attached" and "idle".
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
        if self.get(target).is_some_and(|entry| entry.error.is_some()) {
            return HostStatus::Unreachable;
        }
        if has_sessions {
            return HostStatus::Reachable;
        }
        HostStatus::Disconnected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteHostConfig;

    fn config(hosts: &[&str], default_host: Option<&str>) -> HyprmuxRemoteConfig {
        let mut cfg = HyprmuxRemoteConfig::default();
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
            &["scratch".to_string(), "workbox".to_string()],
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
    fn reseed_preserves_expanded_and_error_for_surviving_hosts() {
        let mut registry = HostRegistry::default();
        registry.seed(&config(&["workbox"], None), &[], &[]);
        let target = RemoteTarget::Alias("workbox".into());
        {
            let entry = registry.get_mut(&target).unwrap();
            entry.expanded = true;
            entry.error = Some("timed out".to_string());
        }
        // A recent is added, but workbox survives and keeps its UI state.
        registry.seed(&config(&["workbox"], None), &["scratch".to_string()], &[]);
        let entry = registry.get(&target).unwrap();
        assert!(entry.expanded);
        assert_eq!(entry.error.as_deref(), Some("timed out"));
        // A host that dropped out of every source is gone.
        registry.seed(&config(&[], None), &[], &[]);
        assert!(registry.is_empty());
    }

    #[test]
    fn status_prefers_live_attachments_over_a_stale_error() {
        let mut registry = HostRegistry::default();
        registry.seed(&config(&["workbox"], None), &[], &[]);
        let target = RemoteTarget::Alias("workbox".into());
        registry.get_mut(&target).unwrap().error = Some("was down".to_string());

        // No attachments + recorded error → unreachable, even with sessions listed.
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), true),
            HostStatus::Unreachable
        );
        // A connecting attachment overrides the error.
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

        // No error, no attachment, but sessions listed → reachable; nothing at all → disconnected.
        registry.get_mut(&target).unwrap().error = None;
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), true),
            HostStatus::Reachable
        );
        assert_eq!(
            registry.status_for(&target, std::iter::empty(), false),
            HostStatus::Disconnected
        );
    }
}
