use std::cell::Cell;
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

/// What a row's ✕ destroys. Held by identity rather than by row index or by the discovered entry
/// itself: rows are rebuilt from scratch on every session sweep and pane change, so an armed
/// confirmation has to survive its row moving, and a `DiscoveredSession` carries live client counts
/// that change underneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarClose {
    /// Kill the pane, the same as `close-pane` on it.
    Pane(crate::state::PaneId),
    /// Kill the session: shut its server down, the same as the picker's `Ctrl+K`.
    Session {
        name: String,
        remote_target: Option<crate::session::remote::RemoteTarget>,
    },
}

#[derive(Clone, Debug)]
pub struct SidebarPanelState {
    pub tabs: Vec<SidebarTabId>,
    pub active_tab: Option<SidebarTabId>,
    pub cursor: usize,
    /// Number of selectable rows visible in the active row list, used by PageUp/PageDown.
    pub page_rows: usize,
    pub suppress_row_hover: bool,
    pub hovered_row: Option<usize>,
}

impl Default for SidebarPanelState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab: None,
            cursor: 0,
            page_rows: 5,
            suppress_row_hover: false,
            hovered_row: None,
        }
    }
}

impl SidebarPanelState {
    fn new(tabs: Vec<SidebarTabId>) -> Self {
        Self {
            active_tab: tabs.first().cloned(),
            tabs,
            ..Self::default()
        }
    }

    fn reconcile_active(&mut self) {
        if self
            .active_tab
            .as_ref()
            .is_none_or(|active| !self.tabs.contains(active))
        {
            self.active_tab = self.tabs.first().cloned();
        }
    }
}

#[derive(Default)]
pub struct SidebarState {
    pub panels: Vec<SidebarPanelState>,
    /// Panel keyboard operations target. It also remembers the last panel selected by mouse.
    pub active_panel: usize,
    pub command_output: HashMap<SidebarTabId, SidebarCommandOutput>,
    pub command_in_flight: HashMap<SidebarTabId, u64>,
    pub command_epoch: u64,
    pub next_output_epoch: u64,
    pub config_epoch: u64,
    pub sessions: Vec<crate::session::discovery::DiscoveredSession>,
    pub sessions_epoch: u64,
    /// The `sessions_epoch` the auto-refresh loop is currently live for. When it lags behind
    /// `sessions_epoch` (a session switch, a create, a reopen bumped the epoch and killed the old
    /// loop), the post-update chokepoint re-arms the loop so the tab keeps updating instead of
    /// freezing until it is reopened.
    pub sessions_refresh_armed_epoch: Option<u64>,
    /// A host whose "Click to disconnect" is armed for a confirming second click. Cleared by
    /// navigating away or acting on anything else, so the confirmation never outlives the moment.
    pub pending_host_disconnect: Option<crate::session::remote::RemoteTarget>,
    /// Resolved roots for the file-tree tabs: the focused pane's local working directory, and the
    /// git repository containing it. Both are recomputed only when the pane's reported directory
    /// actually changes, so the ancestor walk does not run on every frame or every shell prompt.
    pub tree_cwd: Option<String>,
    pub tree_repo: Option<String>,
    /// Directory listings served by the session server, for the file tree's provided entry source
    /// under `--remote`. A directory absent here is pending: the widget shows a loading row and
    /// emits a request, which is why this is only ever appended to, never cleared per frame.
    pub tree_listings: Vec<tui_lipan::prelude::FileTreeDirectoryListing>,
    /// Paths with an in-flight `ListDirectory`, so an expand/collapse cycle does not re-ask.
    pub tree_pending: std::collections::HashSet<String>,
    /// Server-side change scan backing the `Changes` tab under `--remote`.
    pub tree_changes: Vec<tui_lipan::prelude::FileTreeChange>,
    /// Root of the current [`Self::tree_changes`] scan, so a root switch refetches.
    pub tree_changes_root: Option<String>,
    /// `git_refresh_token` value the server-side tree data was last fetched at. Lets a refresh
    /// (root change, command completion) re-ask the server exactly once instead of every message.
    pub tree_server_token: u64,
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
    /// Whether the explorer input was entered from the focused tree with `/`. App commands run
    /// before widget interceptors, so Escape uses this signal to let the input return to the tree.
    pub explorer_entered_from_tree: bool,
    /// A row's ✕ armed for a confirming second click. Cleared by acting on anything else or by
    /// moving the cursor, so the confirmation never outlives the moment. An armed row keeps its ✕
    /// visible even unhovered — an invisible armed state is worse than a lingering glyph.
    pub pending_row_close: Option<SidebarClose>,
    /// The elapsed-time text the Agents tab last rendered. Comparing against it turns most of the
    /// once-a-second duration ticks into a bare reschedule instead of a repaint, the same way the
    /// workbar clock avoids redrawing an identical badge.
    pub last_agent_durations: Option<String>,
    /// Whether a duration tick chain is currently running. Several sites can want one — a tab
    /// change, revealing the sidebar, an agent changing state — and without this each would start
    /// its own chain, so the sidebar would repaint once a second per arming.
    pub agent_tick_armed: bool,
    /// Requested sidebar width while its outer splitter is being dragged. This drives live layout
    /// and PTY resizing without persisting the preference until release.
    pub width_preview: Option<u16>,
    /// `(viewport_w, sidebar_w, docked_right)` — dock side is part of the signature so a live
    /// `position` flip re-applies explicit weights instead of keeping the previous index-ordered
    /// sizes (which would swap the sidebar and pane column after the children swap).
    outer_splitter_signature: Cell<Option<(u16, u16, bool)>>,
    outer_splitter_nonce: Cell<u32>,
    panel_splitter_signature: Cell<Option<(usize, u32)>>,
    panel_splitter_nonce: Cell<u32>,
}

impl SidebarState {
    pub fn new(config: &SidebarConfig) -> Self {
        Self {
            panels: displayed_panel_ids(config)
                .iter()
                .cloned()
                .map(SidebarPanelState::new)
                .collect(),
            ..Self::default()
        }
    }

    pub fn reconcile(&mut self, config: &SidebarConfig) {
        let ids: Vec<_> = config.tabs.iter().map(|tab| tab.id()).collect();
        self.apply_panel_layout(config, &ids);
        self.command_output.retain(|id, _| ids.contains(id));
        self.invalidate_commands();
        self.config_epoch = self.config_epoch.wrapping_add(1);
        self.invalidate_sessions();
    }

    pub fn apply_configured_panels(&mut self, config: &SidebarConfig) {
        let ids: Vec<_> = config.tabs.iter().map(|tab| tab.id()).collect();
        self.apply_panel_layout(config, &ids);
    }

    fn apply_panel_layout(&mut self, config: &SidebarConfig, ids: &[SidebarTabId]) {
        let selected = self.active_tab().cloned();
        let old_active: Vec<_> = selected
            .iter()
            .cloned()
            .chain(
                self.panels
                    .iter()
                    .filter_map(|panel| panel.active_tab.clone())
                    .filter(|active| Some(active) != selected.as_ref()),
            )
            .collect();
        self.panels = displayed_panel_ids(config)
            .iter()
            .map(|tabs| {
                let tabs: Vec<_> = tabs.iter().filter(|id| ids.contains(id)).cloned().collect();
                let active_tab = old_active.iter().find(|id| tabs.contains(id)).cloned();
                let mut panel = SidebarPanelState {
                    tabs,
                    active_tab,
                    ..SidebarPanelState::default()
                };
                panel.reconcile_active();
                panel
            })
            .collect();
        if self.panels.is_empty() {
            self.panels.push(SidebarPanelState::new(ids.to_vec()));
        }
        self.active_panel = self.active_panel.min(self.panels.len() - 1);
    }

    pub fn active_panel(&self) -> Option<&SidebarPanelState> {
        self.panels.get(self.active_panel)
    }

    pub fn active_panel_mut(&mut self) -> Option<&mut SidebarPanelState> {
        self.panels.get_mut(self.active_panel)
    }

    pub fn active_tab(&self) -> Option<&SidebarTabId> {
        self.active_panel()?.active_tab.as_ref()
    }

    pub fn active_tab_in(&self, panel: usize) -> Option<&SidebarTabId> {
        self.panels.get(panel)?.active_tab.as_ref()
    }

    pub fn active_tabs(&self) -> impl Iterator<Item = &SidebarTabId> {
        self.panels
            .iter()
            .filter_map(|panel| panel.active_tab.as_ref())
    }

    pub fn cycle(&mut self, panel: usize, forward: bool) {
        let Some(panel) = self.panels.get_mut(panel) else {
            return;
        };
        if panel.tabs.is_empty() {
            panel.active_tab = None;
            return;
        }
        let current = panel
            .active_tab
            .as_ref()
            .and_then(|active| panel.tabs.iter().position(|tab| tab == active))
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % panel.tabs.len()
        } else {
            current.checked_sub(1).unwrap_or(panel.tabs.len() - 1)
        };
        panel.active_tab = Some(panel.tabs[next].clone());
    }

    pub fn reorder_tab(&mut self, panel: usize, from: usize, to: usize) -> bool {
        let Some(panel) = self.panels.get_mut(panel) else {
            return false;
        };
        if from >= panel.tabs.len() || to >= panel.tabs.len() || from == to {
            return false;
        }
        let tab = panel.tabs.remove(from);
        panel.tabs.insert(to, tab);
        true
    }

    pub fn transfer_tab(
        &mut self,
        from_panel: usize,
        to_panel: usize,
        from: usize,
        to: usize,
    ) -> bool {
        if from_panel == to_panel
            || from_panel >= self.panels.len()
            || to_panel >= self.panels.len()
        {
            return false;
        }
        let Some(tab) = self.panels[from_panel].tabs.get(from).cloned() else {
            return false;
        };
        self.panels[from_panel].tabs.remove(from);
        if self.panels[from_panel].active_tab.as_ref() == Some(&tab) {
            self.panels[from_panel].active_tab = self.panels[from_panel]
                .tabs
                .get(from.min(self.panels[from_panel].tabs.len().saturating_sub(1)))
                .cloned();
        }
        let to = to.min(self.panels[to_panel].tabs.len());
        self.panels[to_panel].tabs.insert(to, tab.clone());
        self.panels[to_panel].active_tab = Some(tab);
        true
    }

    pub fn set_split(&mut self, split: bool) {
        match (split, self.panels.len()) {
            (true, 1) => self.panels.push(SidebarPanelState::default()),
            (false, 2..) => {
                let selected = self.active_tab().cloned();
                let trailing = self.panels.split_off(1);
                self.panels[0]
                    .tabs
                    .extend(trailing.into_iter().flat_map(|panel| panel.tabs));
                self.active_panel = 0;
                self.panels[0].active_tab =
                    selected.or_else(|| self.panels[0].tabs.first().cloned());
            }
            _ => {}
        }
    }

    pub fn panel_ids(&self) -> Vec<Vec<SidebarTabId>> {
        self.panels.iter().map(|panel| panel.tabs.clone()).collect()
    }

    pub fn outer_splitter_nonce(
        &self,
        viewport_width: u16,
        sidebar_width: u16,
        docked_right: bool,
    ) -> u32 {
        let signature = (viewport_width, sidebar_width, docked_right);
        if self.outer_splitter_signature.get() != Some(signature) {
            self.outer_splitter_signature.set(Some(signature));
            self.outer_splitter_nonce
                .set(self.outer_splitter_nonce.get().wrapping_add(1));
        }
        self.outer_splitter_nonce.get()
    }

    pub fn invalidate_outer_splitter(&self) {
        self.outer_splitter_signature.set(None);
    }

    pub fn panel_splitter_nonce(&self, panel_count: usize, split_ratio: f32) -> u32 {
        let signature = (panel_count, split_ratio.to_bits());
        if self.panel_splitter_signature.get() != Some(signature) {
            self.panel_splitter_signature.set(Some(signature));
            self.panel_splitter_nonce
                .set(self.panel_splitter_nonce.get().wrapping_add(1));
        }
        self.panel_splitter_nonce.get()
    }

    pub fn invalidate_panel_splitter(&self) {
        self.panel_splitter_signature.set(None);
    }

    pub fn invalidate_sessions(&mut self) {
        self.sessions.clear();
        self.sessions_epoch = self.sessions_epoch.wrapping_add(1);
    }

    pub fn invalidate_commands(&mut self) {
        self.command_epoch = self.command_epoch.wrapping_add(1);
    }
}

fn displayed_panel_ids(config: &SidebarConfig) -> Vec<Vec<SidebarTabId>> {
    if config.split {
        config.panels.clone()
    } else {
        vec![config.panels.iter().flatten().cloned().collect()]
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
        state.panels[0].active_tab = Some(SidebarTabId::new("panes"));
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
        assert_eq!(state.active_tab(), Some(&SidebarTabId::new("panes")));
        assert!(state.command_output.is_empty());

        new.tabs = vec![SidebarTab::Sessions];
        state.reconcile(&new);
        assert_eq!(state.active_tab(), Some(&SidebarTabId::new("sessions")));
    }

    #[test]
    fn transfer_keeps_both_panel_selections_valid() {
        let config = SidebarConfig {
            split: true,
            panels: vec![
                vec![SidebarTabId::new("agents"), SidebarTabId::new("panes")],
                vec![SidebarTabId::new("sessions")],
            ],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&config);
        state.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        assert!(state.transfer_tab(0, 1, 1, 1));
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("agents"))
        );
        assert_eq!(state.panels[1].active_tab, Some(SidebarTabId::new("panes")));
    }

    #[test]
    fn reorder_and_merge_preserve_tab_identity_and_selection() {
        let config = SidebarConfig {
            split: true,
            panels: vec![
                vec![SidebarTabId::new("agents"), SidebarTabId::new("panes")],
                vec![SidebarTabId::new("sessions")],
            ],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&config);
        state.panels[0].active_tab = Some(SidebarTabId::new("agents"));
        assert!(state.reorder_tab(0, 0, 1));
        assert_eq!(
            state.panels[0].tabs,
            vec![SidebarTabId::new("panes"), SidebarTabId::new("agents")]
        );
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("agents"))
        );

        state.active_panel = 1;
        state.set_split(false);
        assert_eq!(state.panels.len(), 1);
        assert_eq!(
            state.panels[0].tabs,
            vec![
                SidebarTabId::new("panes"),
                SidebarTabId::new("agents"),
                SidebarTabId::new("sessions"),
            ]
        );
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("sessions"))
        );
    }

    #[test]
    fn disabling_split_flattens_display_and_reenabling_restores_saved_panels() {
        let mut config = SidebarConfig {
            split: true,
            panels: vec![
                vec![SidebarTabId::new("agents")],
                vec![SidebarTabId::new("panes"), SidebarTabId::new("sessions")],
            ],
            ..SidebarConfig::default()
        };
        let saved = config.panels.clone();
        let mut state = SidebarState::new(&config);
        state.panels[1].active_tab = Some(SidebarTabId::new("sessions"));
        state.active_panel = 1;

        config.split = false;
        state.apply_configured_panels(&config);
        assert_eq!(config.panels, saved);
        assert_eq!(state.panels.len(), 1);
        assert_eq!(state.panels[0].tabs, saved.concat());
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("sessions"))
        );

        config.split = true;
        state.apply_configured_panels(&config);
        assert_eq!(state.panel_ids(), saved);
        assert_eq!(
            state.panels[1].active_tab,
            Some(SidebarTabId::new("sessions"))
        );
    }

    #[test]
    fn invalidating_controlled_splitters_forces_the_next_weights() {
        let state = SidebarState::default();
        let outer = state.outer_splitter_nonce(100, 32, false);
        assert_eq!(state.outer_splitter_nonce(100, 32, false), outer);
        assert_ne!(state.outer_splitter_nonce(100, 32, true), outer);
        state.invalidate_outer_splitter();
        assert_ne!(state.outer_splitter_nonce(100, 32, true), outer);

        let panels = state.panel_splitter_nonce(2, 0.5);
        assert_eq!(state.panel_splitter_nonce(2, 0.5), panels);
        state.invalidate_panel_splitter();
        assert_ne!(state.panel_splitter_nonce(2, 0.5), panels);
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
