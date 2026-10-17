use std::cell::Cell;
use std::collections::HashMap;

use crate::config::{SidebarConfig, SidebarTab, SidebarTabId};
use crate::session::protocol::PaneCommandPhase;
use crate::state::State;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarCommandRow {
    pub raw: String,
    pub display: String,
    pub error: bool,
    /// An output line the tab's `group_prefix` marked as a section header. It labels the rows under
    /// it rather than being one, so it is never selectable and never activates.
    pub header: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarCommandOutput {
    pub epoch: u64,
    pub rows: Vec<SidebarCommandRow>,
    /// A JSON command tab's rows. `None` for a line tab, and for a JSON tab whose command failed or
    /// printed something that is not a snapshot, whose `rows` then say why.
    pub tree: Option<Vec<crate::state::SidebarRowSpec>>,
    /// Directory the command was run in. Output describes one project, so it stops being an answer
    /// the moment the focused pane moves to another one - including while the tab is off screen,
    /// where nothing else would notice before it is shown again.
    pub cwd: Option<String>,
}

/// What activating a row does. Rows are built as a pure function of `State`, so the update side can
/// rebuild the same list and resolve an index back to one of these — which is what lets Enter and a
/// click share a single code path instead of two callbacks that can drift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowTarget {
    /// Headers, spacers, and error rows: present in the list, never selected or activated.
    Inert,
    Pane(crate::state::PaneId),
    /// One agent or activity inside a pane that publishes several. Focuses the pane and asks its
    /// program to bring that row on screen, since focusing alone would only ever reveal the row it
    /// already draws.
    PublishedRow {
        pane_id: crate::state::PaneId,
        row: crate::session::protocol::RowKey,
    },
    Session(Box<crate::session::discovery::DiscoveredSession>),
    /// An offline host row in the Sessions tab: connect (probe) that host.
    HostConnect(crate::session::remote::RemoteTarget),
    /// A row in a hierarchical tab whose only job is to hold children: activating it shows or hides
    /// them.
    TreeToggle(crate::state::TreeExpansionKey),
    /// One agent in the Agents tab, wherever it is running: a focus change for the session on
    /// screen, a switch or attach first for any other.
    Agent(crate::state::AgentLocation),
    /// A "New session" action row. `None` creates locally; `Some(host)` creates on that host.
    NewSession(Option<crate::session::remote::RemoteTarget>),
    /// A "New pane" action row. Spawns an interactive pane on that workspace.
    NewPane {
        workspace: usize,
    },
    /// The "Connect a host…" action row, opening the remote-host connect prompt.
    ConnectHost,
    /// A checkout in the Worktrees tab: open its session, or create one there.
    Worktree(String),
    /// The Worktrees tab's "New worktree" action row.
    NewWorktree,
    Launcher {
        config_epoch: u64,
        tab_id: SidebarTabId,
        entry_index: usize,
    },
    CommandRow {
        config_epoch: u64,
        tab_id: SidebarTabId,
        output_epoch: u64,
        line: String,
    },
    /// A row of a JSON command tab, by the id its command gave it.
    CommandTreeRow {
        config_epoch: u64,
        tab_id: SidebarTabId,
        output_epoch: u64,
        id: String,
    },
    /// A row a stream producer published: activating it is reported back to that producer.
    StreamRow(crate::state::StreamRowRef),
}

/// What a row's ✕ destroys. Held by identity rather than by row index or by the discovered entry
/// itself: rows are rebuilt from scratch on every session sweep and pane change, so an armed
/// confirmation has to survive its row moving, and a `DiscoveredSession` carries live client counts
/// that change underneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarClose {
    /// Kill the pane, the same as `close-pane` on it.
    Pane(crate::state::PaneId),
    /// Remove a linked checkout, never its branch. `force` once Git has refused a dirty one.
    Worktree { path: String, force: bool },
    /// Kill the session: shut its server down, the same as the picker's `Ctrl+K`.
    Session {
        name: String,
        remote_target: Option<crate::session::remote::RemoteTarget>,
    },
    /// Fire a stream producer's `close` action on one of its rows.
    StreamAction {
        row: crate::state::StreamRowRef,
        action: String,
    },
    /// Disconnect the host: close every attachment to it — their servers keep running — and return
    /// it to offline. Not a kill, but destructive enough to deserve the same two-step ✕ as one.
    Host {
        target: crate::session::remote::RemoteTarget,
    },
}

impl State {
    /// Cached output for a command tab, as long as it still describes where the focused pane is.
    ///
    /// A tab that is not on screen keeps polling nothing, so its cache can outlive the directory it
    /// was collected in. Treating that as absent is what stops a hidden tab from flashing the last
    /// project's rows when it is shown again, and keeps a visible one from answering for a project
    /// you have already left.
    pub fn fresh_command_output(&self, id: &SidebarTabId) -> Option<&SidebarCommandOutput> {
        self.sidebar
            .command_output
            .get(id)
            .filter(|output| output.cwd == self.sidebar.command_cwd)
    }

    pub fn active_sidebar_tab(&self, panel: usize) -> Option<&SidebarTab> {
        let id = self.sidebar.active_tab_in(panel)?;
        self.config.sidebar.tabs.iter().find(|tab| tab.id() == *id)
    }

    /// The sections a tab built from a semantic projection lists: Projects, JSON command tabs, and
    /// extension streams. `None` for a tab whose rows the view builds.
    pub fn tree_sections(&self, tab: &SidebarTab) -> Option<Vec<crate::state::TreeSection>> {
        match tab {
            SidebarTab::Projects => Some(self.project_sections()),
            SidebarTab::Command {
                name,
                format: crate::config::SidebarCommandFormat::Json,
                on_click,
                ..
            } => {
                let output = self.fresh_command_output(name)?;
                let rows = output.tree.as_ref()?;
                let config_epoch = self.sidebar.config_epoch;
                Some(vec![crate::state::TreeSection {
                    key: "rows".into(),
                    header: None,
                    rows: rows
                        .iter()
                        .map(|spec| {
                            // With nothing to run, a row only expands or collapses.
                            let target = if on_click.is_some() {
                                RowTarget::CommandTreeRow {
                                    config_epoch,
                                    tab_id: name.clone(),
                                    output_epoch: output.epoch,
                                    id: spec.id.clone(),
                                }
                            } else {
                                RowTarget::Inert
                            };
                            crate::state::TreeRow::from_spec(spec, target, None)
                        })
                        .collect(),
                    lazy: false,
                }])
            }
            SidebarTab::Stream { name, .. } => Some(self.stream_sections(name)),
            _ => None,
        }
    }

    /// The visible lines of a tab built from tree sections, in display order. `None` otherwise.
    pub fn tree_items(&self, tab: &SidebarTab) -> Option<Vec<crate::state::TreeItem>> {
        let sections = self.tree_sections(tab)?;
        Some(crate::state::flatten(
            &tab.id(),
            &sections,
            &self.sidebar.tree_rows_expanded,
        ))
    }

    /// A row tab's rows, in display order: what the view draws and what an index resolves to.
    pub(crate) fn sidebar_rows(&self, tab: &SidebarTab) -> Vec<crate::view::sidebar::SidebarRow> {
        crate::view::sidebar::rows(self, tab)
    }

    /// What activating each of a row tab's rows does, in display order.
    pub fn sidebar_targets(&self, tab: &SidebarTab) -> Vec<RowTarget> {
        self.sidebar_rows(tab)
            .into_iter()
            .map(|row| row.target)
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct SidebarPanelState {
    pub dock: crate::config::SidebarPosition,
    pub home: usize,
    pub tabs: Vec<SidebarTabId>,
    pub active_tab: Option<SidebarTabId>,
    pub cursor: usize,
    pub suppress_row_hover: bool,
    pub hovered_row: Option<usize>,
}

impl Default for SidebarPanelState {
    fn default() -> Self {
        Self {
            dock: crate::config::SidebarPosition::Left,
            home: 0,
            tabs: Vec::new(),
            active_tab: None,
            cursor: 0,
            suppress_row_hover: false,
            hovered_row: None,
        }
    }
}

impl SidebarPanelState {
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

/// One line of the Worktrees tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeTabItem {
    /// The repository's name and the host it is on.
    Header {
        repository: String,
        host: String,
    },
    Checkout(WorktreeTabRow),
    /// A checkout Git is creating right now.
    Creating {
        branch: String,
    },
    /// Loading, or a Git error listed under the repository heading.
    Message(String),
    /// The "New worktree" action.
    New,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeTabRow {
    pub tree: crate::git::worktrees::WorktreeInfo,
    /// Sessions recording this checkout as their origin.
    pub sessions: Vec<crate::session::protocol::WorktreeSession>,
    /// The checkout the focused pane is in.
    pub current: bool,
    /// A removal of this checkout is running.
    pub removing: bool,
    /// Whether its ✕ is offered: a linked checkout no session uses, unlocked or with a stale lock.
    pub closable: bool,
    /// Git refused to remove it as dirty, so its ✕ forces the removal.
    pub force: bool,
}

impl crate::state::State {
    pub fn worktree_tab_items(&self) -> Vec<WorktreeTabItem> {
        let listing = &self.sidebar.worktrees;
        let Some((target, cwd)) = listing.source.as_ref() else {
            // No repository: the tab is empty, and the view shows the reason as a placeholder.
            return Vec::new();
        };
        let primary = listing
            .entries
            .iter()
            .find(|tree| !tree.linked)
            .map(|tree| tree.path.clone());
        let repository = primary
            .as_deref()
            .unwrap_or(cwd)
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or("repository")
            .to_string();
        let host = target
            .as_ref()
            .map_or_else(|| "local".to_string(), |target| target.display_label());
        let mut items = vec![WorktreeTabItem::Header { repository, host }];

        // Only an operation this attachment can still hear back about is shown as running.
        let operation = self
            .worktree_operation
            .as_ref()
            .filter(|op| &op.cwd == cwd && self.worktree_operation_reachable());
        if !listing.loaded {
            items.push(WorktreeTabItem::Message("Loading…".to_string()));
        } else if let Some(error) = listing.error.as_ref() {
            items.push(WorktreeTabItem::Message(format!("Git: {error}")));
        }
        if listing.loaded {
            for tree in &listing.entries {
                let sessions = listing
                    .sessions
                    .get(&tree.path)
                    .cloned()
                    .unwrap_or_default();
                let removing = self.worktree_removing(&tree.path, target.as_ref());
                items.push(WorktreeTabItem::Checkout(WorktreeTabRow {
                    current: &tree.path == cwd,
                    closable: tree.linked
                        && !tree.bare
                        && tree.lock.as_ref().is_none_or(|lock| lock.stale)
                        && sessions.is_empty()
                        && !removing,
                    force: listing.force_remove.as_ref() == Some(&tree.path),
                    removing,
                    sessions,
                    tree: tree.clone(),
                }));
            }
        }
        if let Some(crate::state::WorktreeOperationKind::Create { branch }) =
            operation.map(|op| &op.kind)
        {
            items.push(WorktreeTabItem::Creating {
                branch: branch.clone(),
            });
        }
        items.push(WorktreeTabItem::New);
        items
    }
}

/// What the Worktrees tab shows: the checkouts of the focused pane's repository, on its host.
#[derive(Clone, Debug, Default)]
pub struct SidebarWorktrees {
    /// Visual-sketch override for comparing PR/CI detail contrast before choosing a default.
    #[cfg(feature = "ui-snapshot")]
    pub detail_dim_preview: Option<f32>,
    pub source_epoch: Option<u64>,
    pub statuses: crate::git::pull_requests::WorktreeStatuses,
    pub pending_status: Option<u64>,
    pub list_refresh: super::WorktreeReadRefresh,
    pub status_refresh: super::WorktreeReadRefresh,
    /// The repository listed, as the host and project root of the pane it follows. `None` when the
    /// focused pane is not in a Git repository the session host can reach.
    pub source: Option<(Option<crate::session::remote::RemoteTarget>, String)>,
    /// Why there is no `source`: not in a repository, a nested SSH pane, no session yet.
    pub unavailable: Option<String>,
    pub entries: Vec<crate::git::worktrees::WorktreeInfo>,
    /// Sessions recording each checkout as their origin, keyed by its listed path.
    pub sessions:
        std::collections::BTreeMap<String, Vec<crate::session::protocol::WorktreeSession>>,
    /// Whether `entries` reflects a reply (or the cache) for `source`, rather than nothing yet.
    pub loaded: bool,
    pub pending: Option<u64>,
    /// The refresh signal the last request answered, so a list is asked for once per change.
    pub requested_token: Option<u64>,
    pub error: Option<String>,
    /// A checkout Git refused to remove because it is dirty. Its ✕ now forces the removal.
    pub force_remove: Option<String>,
}

#[derive(Default)]
pub struct SidebarState {
    pub layout_epoch: u64,
    selection_memory: HashMap<(crate::config::SidebarPosition, usize), SidebarTabId>,
    /// Docks currently displayed by this client, in left/right order.
    pub shown: [bool; 2],
    /// Last nonempty combination hidden by the global action or the last individual hide.
    pub restore: [bool; 2],
    pub right_slide: Cell<f32>,
    pub right_width_preview: Option<u16>,
    pub width_drag_side: Option<crate::config::SidebarPosition>,
    pub panels: Vec<SidebarPanelState>,
    /// Panel keyboard operations target. It also remembers the last panel selected by mouse.
    active_panel: usize,
    /// Last selected home panel in each dock.
    panel_memory: [usize; 2],
    pub command_output: HashMap<SidebarTabId, SidebarCommandOutput>,
    pub command_poll_scheduled: HashMap<SidebarTabId, u64>,
    pub command_in_flight: HashMap<SidebarTabId, u64>,
    pub command_epoch: u64,
    pub next_output_epoch: u64,
    pub config_epoch: u64,
    /// Focused pane directory the command tabs were last polled from. `None` until a pane reports
    /// one, and under `--remote`, where the pane's path is not the client's to run in.
    pub command_cwd: Option<String>,
    pub sessions: Vec<crate::session::discovery::DiscoveredSession>,
    pub sessions_epoch: u64,
    /// The `sessions_epoch` the auto-refresh loop is currently live for. When it lags behind
    /// `sessions_epoch` (a session switch, a create, a reopen bumped the epoch and killed the old
    /// loop), the post-update chokepoint re-arms the loop so the tab keeps updating instead of
    /// freezing until it is reopened.
    pub sessions_refresh_armed_epoch: Option<u64>,
    /// Resolved roots for the file-tree tabs: the focused pane's local working directory, and the
    /// git repository containing it. Both are recomputed only when the pane's reported directory
    /// actually changes, so the ancestor walk does not run on every frame or every shell prompt.
    pub tree_cwd: Option<String>,
    pub tree_repo: Option<String>,
    /// Attachment identity that produced the current roots and remote listings. Paths alone are
    /// insufficient: two retained or remote sessions may report the same cwd on different hosts.
    pub tree_source_epoch: Option<crate::state::AttachmentId>,
    /// Directories the user has expanded in each file-tree tab, so expansion survives the tree
    /// unmounting. It remounts constantly: the tab keys on its root, so focusing a pane in another
    /// directory, switching tabs, or hiding the sidebar all discard the widget's own expansion.
    /// Paths are absolute, which is what lets one tab's memory carry across roots — expanding
    /// `src/` under a repo root leaves it expanded when a pane re-roots the tab inside it.
    ///
    /// Per tab rather than shared: Files and Git are different projections, and collapsing a
    /// directory that only exists in the changes view should not close it while browsing. The set
    /// only ever grows by an explicit user expansion, and dies with the client — this is view
    /// state, not a preference worth persisting.
    pub tree_expanded: HashMap<SidebarTabId, std::collections::HashSet<String>>,
    /// Rows the user expanded or collapsed in the hierarchical row tabs: Projects, JSON command
    /// tabs, and stream tabs. Like [`Self::tree_expanded`], view state that dies with the client.
    pub tree_rows_expanded: crate::state::TreeExpansion,
    /// Successful local Git snapshots shared by keyed Files/Git tree mounts. The framework keeps
    /// this bounded and revalidates every mount; retaining the handle here prevents indicators from
    /// disappearing while a remounted tree's background scan runs.
    pub tree_git_status_cache: tui_lipan::prelude::FileTreeGitStatusCache,
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
    /// Root most recently requested from the server. A completion for any other root is stale.
    pub tree_changes_pending_root: Option<String>,
    /// Last initial remote change-scan failure. Successful data remains visible across refresh
    /// failures, so this is only populated when there is no snapshot to preserve.
    pub tree_changes_error: Option<String>,
    /// `git_refresh_token` value the server-side tree data was last fetched at. Lets a refresh
    /// re-ask the server exactly once instead of every message.
    pub tree_server_token: u64,
    /// Tokens handed to the local FileTree. Git refreshes on root changes, completed commands, and
    /// the visible-tree poll; directory entries refresh only on the poll.
    pub tree_entry_refresh_token: u64,
    pub git_refresh_token: u64,
    /// Generation of the visible-tree refresh chain and the generation currently armed. A new arm
    /// always gets a new epoch so a delayed tick from before a hide/show cycle cannot fork the loop.
    pub tree_refresh_epoch: u64,
    pub tree_refresh_armed_epoch: Option<u64>,
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
    /// A row's ✕ armed for a confirming second click or `x`. Cleared by acting on anything else or
    /// by moving the cursor, so the confirmation never outlives the moment. An armed row keeps its
    /// ✕ visible even unhovered — an invisible armed state is worse than a lingering glyph.
    pub pending_row_close: Option<SidebarClose>,
    /// The Worktrees tab: the focused pane's repository and its checkouts.
    pub worktrees: SidebarWorktrees,
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
    splitter_revisions: std::cell::RefCell<HashMap<String, (Vec<u32>, u32)>>,
}

impl SidebarState {
    pub fn new(config: &SidebarConfig) -> Self {
        let panels = resolved_panels(config);
        let mut state = Self {
            panels,
            ..Self::default()
        };
        state.initialize_visibility(config.startup, config.migrated_docks);
        state
    }

    /// Seed a new client only. Configuration reloads never reapply startup visibility.
    pub fn initialize_visibility(&mut self, startup: bool, remembered: Option<[bool; 2]>) {
        self.shown = [false, false];
        self.restore = remembered
            .filter(|docks| docks.iter().any(|shown| *shown))
            .unwrap_or([false, false]);
        if startup {
            self.show_restored();
        }
        let target = self
            .panels
            .iter()
            .position(|panel| {
                self.shown[usize::from(panel.dock == crate::config::SidebarPosition::Right)]
            })
            .or_else(|| self.panels.iter().position(|panel| !panel.tabs.is_empty()))
            .unwrap_or(0);
        self.select_panel(target);
        self.right_slide.set(if self.shown[1] { 1.0 } else { 0.0 });
    }

    /// The last nonempty combination, suitable for saving when this client leaves.
    pub fn remembered_docks(&self) -> Option<[bool; 2]> {
        let docks = if self.any_shown() {
            self.shown
        } else {
            self.restore
        };
        docks.iter().any(|shown| *shown).then_some(docks)
    }

    pub fn any_shown(&self) -> bool {
        self.shown.iter().any(|shown| *shown)
    }

    pub fn hide_all(&mut self) {
        if self.any_shown() {
            self.restore = self.shown;
            self.shown = [false, false];
        }
    }

    pub fn show_restored(&mut self) {
        self.shown = self.restore;
        if !self.any_shown() {
            for panel in &self.panels {
                if !panel.tabs.is_empty() {
                    self.shown[usize::from(panel.dock == crate::config::SidebarPosition::Right)] =
                        true;
                }
            }
            if !self.any_shown() {
                self.shown[0] = true;
            }
        }
    }

    pub fn toggle_all(&mut self) {
        if self.any_shown() {
            self.hide_all();
        } else {
            self.show_restored();
        }
    }

    pub fn toggle_dock(&mut self, side: crate::config::SidebarPosition) {
        let index = usize::from(side == crate::config::SidebarPosition::Right);
        let previous = self.shown;
        self.shown[index] = !self.shown[index];
        if !self.any_shown() {
            self.restore = previous;
        }
    }

    pub fn reconcile(&mut self, config: &SidebarConfig) {
        let ids: Vec<_> = config.tabs.iter().map(|tab| tab.id()).collect();
        self.apply_panel_layout(config, &ids);
        self.command_output.retain(|id, _| ids.contains(id));
        self.tree_expanded.retain(|id, _| ids.contains(id));
        self.tree_rows_expanded
            .retain(|key, _| ids.contains(&key.tab));
        self.invalidate_commands();
        self.config_epoch = self.config_epoch.wrapping_add(1);
        self.invalidate_sessions();
    }

    pub fn apply_configured_panels(&mut self, config: &SidebarConfig) {
        let ids: Vec<_> = config.tabs.iter().map(|tab| tab.id()).collect();
        self.apply_panel_layout(config, &ids);
    }

    fn apply_panel_layout(&mut self, config: &SidebarConfig, ids: &[SidebarTabId]) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        let old_address = self.active_panel().map(|p| (p.dock, p.home));
        let old_panels = self.panels.clone();
        for panel in &old_panels {
            if let Some(id) = &panel.active_tab
                && config.layout.location(id.as_str()) == Some((panel.dock, panel.home))
            {
                self.selection_memory
                    .insert((panel.dock, panel.home), id.clone());
            }
        }
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
        self.panels = resolved_panels(config)
            .into_iter()
            .map(|mut panel| {
                panel.tabs.retain(|id| ids.contains(id));
                panel.active_tab = selected
                    .as_ref()
                    .filter(|id| panel.tabs.contains(id))
                    .cloned()
                    .or_else(|| {
                        self.selection_memory
                            .get(&(panel.dock, panel.home))
                            .filter(|id| panel.tabs.contains(id))
                            .cloned()
                    })
                    .or_else(|| {
                        old_active
                            .iter()
                            .find(|id| panel.tabs.contains(id))
                            .cloned()
                    });
                if let Some(old) = old_panels.iter().find(|old| {
                    old.dock == panel.dock
                        && old.home == panel.home
                        && old.active_tab == panel.active_tab
                }) {
                    panel.cursor = old.cursor;
                    panel.suppress_row_hover = old.suppress_row_hover;
                }
                panel.reconcile_active();
                panel
            })
            .collect();
        if let Some(index) = selected
            .as_ref()
            .and_then(|id| self.panels.iter().position(|p| p.tabs.contains(id)))
            .or_else(|| {
                old_address.and_then(|(side, home)| {
                    self.panels.iter().position(|p| {
                        p.dock == side
                            && p.home == home.min(config.layout.dock(side).panel_count - 1)
                    })
                })
            })
        {
            self.active_panel = index;
        }

        self.active_panel = self.active_panel.min(self.panels.len() - 1);
    }

    pub fn active_panel_index(&self) -> usize {
        self.active_panel
    }

    pub fn select_panel(&mut self, index: usize) {
        for selected in [self.active_panel, index] {
            if let Some(panel) = self.panels.get(selected) {
                self.panel_memory
                    [usize::from(panel.dock == crate::config::SidebarPosition::Right)] = panel.home;
            }
        }
        self.active_panel = index;
    }

    /// Recover a hidden keyboard target without changing which docks are shown.
    pub fn select_shown_panel(&mut self) {
        if self.active_panel().is_some_and(|panel| {
            self.shown[usize::from(panel.dock == crate::config::SidebarPosition::Right)]
        }) {
            self.select_panel(self.active_panel);
            return;
        }
        let target = self
            .panels
            .iter()
            .enumerate()
            .filter(|(_, panel)| {
                let dock = usize::from(panel.dock == crate::config::SidebarPosition::Right);
                self.shown[dock] && panel.home <= self.panel_memory[dock]
            })
            .max_by_key(|(_, panel)| panel.home)
            .map(|(index, _)| index);
        if let Some(index) = target {
            self.select_panel(index);
        }
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

    pub fn panel_ids(&self) -> Vec<Vec<SidebarTabId>> {
        self.panels.iter().map(|panel| panel.tabs.clone()).collect()
    }

    /// tui-lipan requires monotonically increasing weight revisions, rather than a signature hash.
    pub fn splitter_nonce(&self, id: &str, signature: Vec<u32>) -> u32 {
        let mut revisions = self.splitter_revisions.borrow_mut();
        let entry = revisions
            .entry(id.to_string())
            .or_insert_with(|| (signature.clone(), 0));
        if entry.0 != signature {
            entry.0 = signature;
            entry.1 = entry.1.wrapping_add(1);
        }
        entry.1
    }

    pub fn invalidate_sessions(&mut self) {
        self.sessions.clear();
        self.sessions_epoch = self.sessions_epoch.wrapping_add(1);
    }

    pub fn invalidate_commands(&mut self) {
        self.command_epoch = self.command_epoch.wrapping_add(1);
    }
}

fn resolved_panels(config: &SidebarConfig) -> Vec<SidebarPanelState> {
    [
        crate::config::SidebarPosition::Left,
        crate::config::SidebarPosition::Right,
    ]
    .into_iter()
    .flat_map(|side| {
        let dock = config.layout.dock(side);
        let mut panels: Vec<_> = (0..dock.panel_count)
            .map(|home| SidebarPanelState {
                dock: side,
                home,
                ..SidebarPanelState::default()
            })
            .collect();
        for (home, saved) in dock.panels.iter().enumerate() {
            let target = home.min(panels.len() - 1);
            panels[target].tabs.extend(
                saved
                    .tabs
                    .iter()
                    .filter(|id| !config.layout.hidden.contains(id))
                    .map(|id| SidebarTabId::new(id.clone()))
                    .filter(|id| config.tabs.iter().any(|tab| tab.id() == *id)),
            );
        }
        for panel in &mut panels {
            panel.reconcile_active();
        }
        panels
    })
    .collect()
}

#[cfg(test)]
mod docking_tests {
    use super::*;
    use crate::config::{
        SidebarDockPanel,
        SidebarPosition::{Left, Right},
        SidebarTab,
    };
    use tui_lipan::prelude::{Rect, Theme};

    #[test]
    fn resolving_compacted_panels_preserves_preferences_and_filters_hidden_and_absent_tabs() {
        let mut config = SidebarConfig::default();
        config.layout.left.panel_count = 1;
        config.layout.hidden = vec!["panes".into()];
        config.layout.place("missing.tab", Right, 4);
        let saved = config.layout.clone();
        let mut state = SidebarState::new(&config);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(
            state.panels[0]
                .tabs
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            [
                "activity",
                "agents",
                "sessions",
                "files",
                "git",
                "worktrees",
                "projects"
            ]
        );
        assert!(state.panels[1].tabs.is_empty());
        state.panels[0].active_tab = Some(SidebarTabId::new("git"));
        config.layout.left.panel_count = 3;
        config.layout.left.panels.push(SidebarDockPanel::default());
        state.apply_configured_panels(&config);
        assert_eq!(state.active_tab(), Some(&SidebarTabId::new("git")));
        assert_eq!(state.active_panel_index(), 1);
        assert_eq!(config.layout.left.panels[..2], saved.left.panels);
        assert_eq!(config.layout.right, saved.right);
    }

    #[test]
    fn geometry_uses_one_budget_and_restores_after_resize() {
        let mut config = crate::config::Config::default();
        config.sidebar.startup = true;
        config.sidebar.migrated_docks = Some([true, true]);
        let state = crate::state::State::new(config, Theme::default());
        for width in 0..180 {
            let rect = Rect {
                x: 0,
                y: 0,
                w: width,
                h: 30,
            };
            let docks = state.dock_deployed_widths(rect);
            let minimum = if width > 20 { 20 } else { width - width / 2 };
            assert!(
                state.content_viewport(rect).w >= minimum,
                "{width}: {docks:?}"
            );
            assert_eq!(
                state.content_viewport(rect).w + state.effective_sidebar_width(rect),
                width
            );
            assert_eq!(
                state.terminal_content_left_offset(rect),
                state.dock_reserved_width(rect, Left)
            );
        }
        assert_eq!(
            state.dock_deployed_widths(Rect {
                x: 0,
                y: 0,
                w: 160,
                h: 30
            }),
            [32, 32]
        );
        assert_eq!(state.config.sidebar.layout.left.width, 32);
        assert_eq!(state.config.sidebar.layout.right.width, 32);
    }

    #[test]
    fn transfer_recovers_source_selection_and_reload_fences_old_events() {
        let config = SidebarConfig::default();
        let mut state = SidebarState::new(&config);
        state.panels[0].active_tab = Some(SidebarTab::Panes.id());
        assert!(state.transfer_tab(0, 2, 2, 0));
        assert_eq!(state.panels[0].active_tab, Some(SidebarTab::Sessions.id()));
        assert_eq!(state.panels[2].active_tab, Some(SidebarTab::Panes.id()));
        let epoch = state.layout_epoch;
        state.reconcile(&config);
        assert_ne!(epoch, state.layout_epoch);
    }

    #[test]
    fn command_invalidation_keeps_process_fencing() {
        let mut state = SidebarState::default();
        state.command_in_flight.insert(SidebarTabId::new("jobs"), 4);
        state.command_epoch = 4;
        state.invalidate_commands();
        assert_eq!(state.command_epoch, 5);
        assert_eq!(
            state.command_in_flight.get(&SidebarTabId::new("jobs")),
            Some(&4)
        );
    }
}
