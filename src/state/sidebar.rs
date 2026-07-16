use std::collections::HashMap;

use crate::config::{SidebarConfig, SidebarTabId};

#[derive(Default)]
pub struct SidebarState {
    pub active_tab: Option<SidebarTabId>,
    pub command_output: HashMap<SidebarTabId, Vec<String>>,
    pub command_epoch: u64,
    pub sessions: Vec<crate::session::discovery::DiscoveredSession>,
    pub sessions_epoch: u64,
    pub pending_session_open: Option<String>,
}

impl SidebarState {
    pub fn new(config: &SidebarConfig) -> Self {
        Self {
            active_tab: config.tabs.first().map(|tab| tab.id()),
            ..Self::default()
        }
    }

    pub fn reconcile(&mut self, config: &SidebarConfig) {
        let ids: Vec<_> = config.tabs.iter().map(|tab| tab.id()).collect();
        if self
            .active_tab
            .as_ref()
            .is_none_or(|active| !ids.contains(active))
        {
            self.active_tab = ids.first().cloned();
        }
        self.command_output.retain(|id, _| ids.contains(id));
        self.command_epoch = self.command_epoch.saturating_add(1);
        self.invalidate_sessions();
    }

    pub fn cycle(&mut self, config: &SidebarConfig, forward: bool) {
        if config.tabs.is_empty() {
            self.active_tab = None;
            return;
        }
        let current = self
            .active_tab
            .as_ref()
            .and_then(|active| config.tabs.iter().position(|tab| tab.id() == *active))
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % config.tabs.len()
        } else {
            current.checked_sub(1).unwrap_or(config.tabs.len() - 1)
        };
        self.active_tab = Some(config.tabs[next].id());
    }

    pub fn invalidate_sessions(&mut self) {
        self.sessions.clear();
        self.pending_session_open = None;
        self.sessions_epoch = self.sessions_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidebarTab;

    #[test]
    fn reload_reconciles_by_stable_id_and_clears_removed_cache() {
        let old = SidebarConfig {
            tabs: vec![SidebarTab::Agents, SidebarTab::Panes, SidebarTab::Sessions],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&old);
        state.active_tab = Some(SidebarTabId::new("panes"));
        state
            .command_output
            .insert(SidebarTabId::new("removed"), vec!["old".into()]);

        let mut new = SidebarConfig {
            tabs: vec![SidebarTab::Sessions, SidebarTab::Panes],
            ..SidebarConfig::default()
        };
        state.reconcile(&new);
        assert_eq!(state.active_tab, Some(SidebarTabId::new("panes")));
        assert!(state.command_output.is_empty());

        new.tabs = vec![SidebarTab::Sessions];
        state.reconcile(&new);
        assert_eq!(state.active_tab, Some(SidebarTabId::new("sessions")));
    }

    #[test]
    fn dock_position_changes_only_terminal_space_offset() {
        let viewport = tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut config = crate::config::HyprmuxConfig::default();
        config.sidebar.visible = true;
        let mut state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        assert_eq!(state.content_viewport(viewport).w, 68);
        assert_eq!(state.terminal_content_left_offset(viewport), 32);

        state.config.sidebar.position = crate::config::SidebarPosition::Right;
        assert_eq!(state.content_viewport(viewport).w, 68);
        assert_eq!(state.terminal_content_left_offset(viewport), 0);
    }
}
