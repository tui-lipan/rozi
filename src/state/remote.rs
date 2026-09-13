use std::collections::HashMap;

use super::HostRegistry;

/// Live remote-host monitor, cache, and discovery state.
///
/// These fields are one subsystem: host registry rows, SSH monitors, probe generations, and the
/// last-seen session cache. Overlay pickers and attachment lifecycle still live on [`super::State`].
pub struct RemoteRuntimeState {
    /// Global remote-discovery generation. Unlike picker-local state, this survives closing and
    /// reopening the picker so a late result can never match a newer request by accident.
    pub probe_epoch: u64,
    /// Known remote hosts for the unified Sessions view: configured aliases, recent ad-hoc targets,
    /// and hosts a live attachment targets. Seeded when the Sessions view opens; carries the
    /// per-host expand/collapse and error state that must survive the recurring session sweep.
    pub hosts: HostRegistry,
    pub(crate) monitors: Vec<crate::session::remote::monitor::Monitor>,
    pub(crate) monitor_generation: u64,
    /// The last agent snapshot each connected host's monitor reported, keyed the way every other
    /// runtime map on a host is. Absent for a host that is disconnected, unreachable, or forgotten.
    pub(crate) agents:
        HashMap<crate::session::remote::RemoteTarget, Vec<crate::session::protocol::AgentSummary>>,
    pub(crate) live_sessions: Vec<crate::session::discovery::DiscoveredSession>,
    /// Hosts added or edited in **Remote hosts** during this run, merged into the saved roster
    /// whenever the registry is reseeded.
    ///
    /// The roster is normally read back from disk, which makes every row depend on a write having
    /// succeeded — and a write into a state directory that is not private to its owner does not.
    /// Holding them here too means a host the user just added is listed for the rest of the
    /// session whatever the filesystem did, so a failed connection leaves a row to retry rather
    /// than a toast about a host that is no longer anywhere.
    pub added_hosts: Vec<crate::session::remote::RemoteTarget>,
    /// Last-seen sessions per remote host, loaded from disk when the Sessions view is seeded and
    /// refreshed on each successful probe. Lets an offline or unreachable host still list the
    /// workplaces it had, rather than reading as empty. Convenience only — never authoritative, and
    /// it holds no credentials.
    pub session_cache: crate::session::HostSessionCache,
}

impl Default for RemoteRuntimeState {
    fn default() -> Self {
        Self {
            probe_epoch: 0,
            hosts: HostRegistry::default(),
            monitors: Vec::new(),
            monitor_generation: 0,
            agents: HashMap::new(),
            live_sessions: Vec::new(),
            added_hosts: Vec::new(),
            session_cache: crate::session::HostSessionCache::new(),
        }
    }
}

impl RemoteRuntimeState {
    pub(crate) fn mint_probe_epoch(&mut self) -> u64 {
        self.probe_epoch = self.probe_epoch.wrapping_add(1);
        self.probe_epoch
    }
}
