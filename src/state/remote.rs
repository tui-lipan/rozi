use std::collections::HashMap;

use super::HostRegistry;

/// Live remote-host monitor, cache, and discovery state.
///
/// These fields are one subsystem: host registry rows, SSH monitors, probe generations, and the
/// last-seen session cache. Overlay pickers and attachment lifecycle still live on [`super::State`].
#[derive(Default)]
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

impl RemoteRuntimeState {
    pub(crate) fn mint_probe_epoch(&mut self) -> u64 {
        self.probe_epoch = self.probe_epoch.wrapping_add(1);
        self.probe_epoch
    }
}

impl super::State {
    /// Compact host labels remain readable; colliding exact targets expose their reversible specs.
    pub fn remote_target_label(&self, target: &crate::session::remote::RemoteTarget) -> String {
        let label = target.display_label();
        let picker_targets = self.session_picker.iter().flat_map(|picker| {
            picker
                .entries
                .iter()
                .filter_map(|entry| entry.remote_target.as_ref())
                .chain(picker.tab.remote_target())
        });
        let agent_tab_target = self
            .agent_picker
            .as_ref()
            .and_then(|picker| match &picker.tab {
                super::AgentPickerTab::Session { target, .. } => target.as_ref(),
                super::AgentPickerTab::All => None,
            });
        let collision = self
            .remote
            .hosts
            .iter()
            .map(|entry| &entry.target)
            .chain(self.remote.agents.keys())
            .chain(
                self.remote
                    .live_sessions
                    .iter()
                    .filter_map(|entry| entry.remote_target.as_ref()),
            )
            .chain(picker_targets)
            .chain(self.current().remote_target.as_ref())
            .chain(self.active_launcher_scope())
            .chain(
                self.background
                    .values()
                    .filter_map(|attachment| attachment.remote_target.as_ref()),
            )
            .chain(agent_tab_target)
            .any(|other| other != target && other.display_label() == label);
        if collision {
            format!("{label} ({})", target.to_spec())
        } else {
            label
        }
    }

    /// Filtering and row actions use the same visible labels and exact target specs.
    pub(crate) fn matches_session_query(
        &self,
        entry: &crate::session::discovery::DiscoveredSession,
        query_lower: &str,
    ) -> bool {
        query_lower.is_empty()
            || entry.name.to_ascii_lowercase().contains(query_lower)
            || entry.remote_target.as_ref().is_some_and(|target| {
                self.remote_target_label(target)
                    .to_ascii_lowercase()
                    .contains(query_lower)
                    || target.to_spec().to_ascii_lowercase().contains(query_lower)
                    || format!("{}@{}", entry.name, self.remote_target_label(target))
                        .to_ascii_lowercase()
                        .contains(query_lower)
                    || format!("{}@{}", entry.name, target.to_spec())
                        .to_ascii_lowercase()
                        .contains(query_lower)
            })
            || entry
                .remote_target
                .as_ref()
                .map(|target| target.display_label())
                .or_else(|| entry.host.clone())
                .is_some_and(|host| {
                    format!("{}@{host}", entry.name)
                        .to_ascii_lowercase()
                        .contains(query_lower)
                })
    }
}
