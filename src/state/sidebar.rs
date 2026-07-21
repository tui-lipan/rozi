use std::collections::HashMap;

use crate::config::{SidebarConfig, SidebarTabId};
use crate::session::protocol::PaneCommandPhase;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarCommandRow {
    pub raw: String,
    pub display: String,
    pub error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarCommandOutput {
    pub epoch: u64,
    pub rows: Vec<SidebarCommandRow>,
}

#[derive(Default)]
pub struct SidebarState {
    pub active_tab: Option<SidebarTabId>,
    pub command_output: HashMap<SidebarTabId, SidebarCommandOutput>,
    pub command_in_flight: HashMap<SidebarTabId, u64>,
    pub command_epoch: u64,
    pub next_output_epoch: u64,
    pub config_epoch: u64,
    pub sessions: Vec<crate::session::discovery::DiscoveredSession>,
    pub sessions_epoch: u64,
    pub pending_session_open: Option<String>,
    /// Resolved roots for the file-tree tabs: the focused pane's local working directory, and the
    /// git repository containing it. Both are recomputed only when the pane's reported directory
    /// actually changes, so the ancestor walk does not run on every frame or every shell prompt.
    pub tree_cwd: Option<String>,
    pub tree_repo: Option<String>,
    /// Monotonic token handed to `FileTree::git_refresh_token`. The widget ignores a token that
    /// does not increase, so this only ever counts up.
    pub git_refresh_token: u64,
    /// Focused pane and its last observed command phase, used to refresh git status on the edge
    /// into `Completed` — the moment a command has finished changing the working tree.
    pub last_command_phase: Option<(crate::state::PaneId, PaneCommandPhase)>,
    /// Whether the row list currently owns keyboard focus. Mirrored from the body widget's
    /// `on_focus`/`on_blur` rather than set directly, so it cannot disagree with the framework
    /// about where focus actually is — clicking a pane blurs the body and clears this on its own.
    pub focused: bool,
    /// Row index of the keyboard cursor. `List` keeps no selection state of its own — it is fully
    /// controlled — so the cursor lives here and moves in response to the widget's `on_select`.
    /// Reset whenever the row list is replaced wholesale, which is a tab change.
    pub cursor: usize,
    /// Ignore stale pointer position after keyboard navigation changes the row cursor. The next
    /// real mouse movement clears this, matching `List`/`Tree` item-hover behavior.
    pub suppress_row_hover: bool,
    /// The elapsed-time text the Agents tab last rendered. Comparing against it turns most of the
    /// once-a-second duration ticks into a bare reschedule instead of a repaint, the same way the
    /// workbar clock avoids redrawing an identical badge.
    pub last_agent_durations: Option<String>,
    /// Whether a duration tick chain is currently running. Several sites can want one — a tab
    /// change, revealing the sidebar, an agent changing state — and without this each would start
    /// its own chain, so the sidebar would repaint once a second per arming.
    pub agent_tick_armed: bool,
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
        self.invalidate_commands();
        self.config_epoch = self.config_epoch.wrapping_add(1);
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

    pub fn invalidate_commands(&mut self) {
        self.command_epoch = self.command_epoch.wrapping_add(1);
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
        state.command_output.insert(
            SidebarTabId::new("removed"),
            SidebarCommandOutput {
                epoch: 1,
                rows: Vec::new(),
            },
        );

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

    #[test]
    fn command_invalidation_preserves_running_process_marker() {
        let mut state = SidebarState::default();
        let id = SidebarTabId::new("rows");
        state.command_in_flight.insert(id.clone(), 4);
        state.command_epoch = 4;
        state.invalidate_commands();
        assert_eq!(state.command_epoch, 5);
        assert_eq!(state.command_in_flight.get(&id), Some(&4));
    }
}
