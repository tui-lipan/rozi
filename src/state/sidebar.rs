use std::cell::Cell;
use std::collections::HashMap;

use crate::config::{SidebarConfig, SidebarTab, SidebarTabId};
use crate::session::protocol::PaneCommandPhase;
use crate::state::{HostStatus, State};

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
        row_id: String,
    },
    Session(Box<crate::session::discovery::DiscoveredSession>),
    /// An offline host row in the Sessions tab: connect (probe) that host.
    HostConnect(crate::session::remote::RemoteTarget),
    /// A "New session" action row. `None` creates locally; `Some(host)` creates on that host.
    NewSession(Option<crate::session::remote::RemoteTarget>),
    /// The "Connect a host…" action row, opening the remote-host connect prompt.
    ConnectHost,
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
    /// Disconnect the host: close every attachment to it — their servers keep running — and return
    /// it to offline. Not a kill, but destructive enough to deserve the same two-step ✕ as one.
    Host {
        target: crate::session::remote::RemoteTarget,
    },
}

/// A lightweight semantic projection of a sidebar item without framework Element trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarItemProjection {
    pub target: RowTarget,
    pub close: Option<SidebarClose>,
}

impl SidebarItemProjection {
    pub fn selectable(&self) -> bool {
        !matches!(self.target, RowTarget::Inert)
    }

    /// Where the cursor actually sits: the stored index if it still points at a selectable item,
    /// otherwise the nearest one.
    pub fn resolve_cursor(cursor: usize, items: &[SidebarItemProjection]) -> Option<usize> {
        if items.get(cursor).is_some_and(Self::selectable) {
            return Some(cursor);
        }
        items.iter().position(Self::selectable).map(|first| {
            items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.selectable())
                .map(|(index, _)| index)
                .min_by_key(|index| index.abs_diff(cursor))
                .unwrap_or(first)
        })
    }
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

    pub fn sidebar_item_projections(&self, tab: &SidebarTab) -> Vec<SidebarItemProjection> {
        match tab {
            SidebarTab::Panes => {
                let mut items = Vec::new();
                for workspace in &self.current().workspaces {
                    let mut ordered_ids = Vec::new();
                    for id in workspace.tiled_ids() {
                        if workspace
                            .panes
                            .iter()
                            .any(|p| p.id == id && !p.floating && !p.closing)
                        {
                            ordered_ids.push(id);
                        }
                    }
                    for pane in &workspace.panes {
                        if pane.floating && !pane.closing && !ordered_ids.contains(&pane.id) {
                            ordered_ids.push(pane.id);
                        }
                    }
                    if ordered_ids.is_empty() {
                        continue;
                    }
                    if !items.is_empty() {
                        items.push(SidebarItemProjection {
                            target: RowTarget::Inert,
                            close: None,
                        });
                    }
                    items.push(SidebarItemProjection {
                        target: RowTarget::Inert,
                        close: None,
                    });
                    for id in ordered_ids {
                        items.push(SidebarItemProjection {
                            target: RowTarget::Pane(id),
                            close: Some(SidebarClose::Pane(id)),
                        });
                    }
                }
                items
            }
            SidebarTab::Activity => self.activity_item_projections(),
            SidebarTab::Sessions => {
                let mut items = Vec::new();
                items.push(SidebarItemProjection {
                    target: RowTarget::Inert,
                    close: None,
                });
                let mut any_local = false;
                for entry in self.sidebar.sessions.iter().filter(|s| s.host.is_none()) {
                    let close = Some(SidebarClose::Session {
                        name: entry.name.clone(),
                        remote_target: entry.remote_target.clone(),
                    });
                    items.push(SidebarItemProjection {
                        target: RowTarget::Session(Box::new(entry.clone())),
                        close,
                    });
                    any_local = true;
                }
                if !any_local {
                    items.push(SidebarItemProjection {
                        target: RowTarget::Inert,
                        close: None,
                    });
                }
                items.push(SidebarItemProjection {
                    target: RowTarget::NewSession(None),
                    close: None,
                });

                for host in self.hosts.iter() {
                    items.push(SidebarItemProjection {
                        target: RowTarget::Inert,
                        close: None,
                    });
                    let live: Vec<_> = self
                        .sidebar
                        .sessions
                        .iter()
                        .filter(|s| s.remote_target.as_ref() == Some(&host.target))
                        .cloned()
                        .collect();
                    let conns: Vec<_> = std::iter::once(self.current())
                        .chain(self.background.values())
                        .filter(|a| a.remote_target.as_ref() == Some(&host.target))
                        .map(|a| a.connection)
                        .collect();
                    let status =
                        self.hosts
                            .status_for(&host.target, conns.iter(), !live.is_empty());
                    // A connected host offers no activation: disconnecting is the hover ✕, the same
                    // affordance (and the same confirmation) every other closable row uses.
                    let (header_target, header_close) = match status {
                        HostStatus::Connected | HostStatus::Reachable => (
                            RowTarget::Inert,
                            Some(SidebarClose::Host {
                                target: host.target.clone(),
                            }),
                        ),
                        HostStatus::Disconnected | HostStatus::Unreachable => {
                            (RowTarget::HostConnect(host.target.clone()), None)
                        }
                        HostStatus::Connecting => (RowTarget::Inert, None),
                    };
                    items.push(SidebarItemProjection {
                        target: header_target,
                        close: header_close,
                    });

                    match status {
                        HostStatus::Connecting => {}
                        HostStatus::Connected | HostStatus::Reachable => {
                            if live.is_empty() {
                                items.push(SidebarItemProjection {
                                    target: RowTarget::Inert,
                                    close: None,
                                });
                            } else {
                                for entry in live {
                                    let close = Some(SidebarClose::Session {
                                        name: entry.name.clone(),
                                        remote_target: entry.remote_target.clone(),
                                    });
                                    items.push(SidebarItemProjection {
                                        target: RowTarget::Session(Box::new(entry)),
                                        close,
                                    });
                                }
                            }
                            items.push(SidebarItemProjection {
                                target: RowTarget::NewSession(Some(host.target.clone())),
                                close: None,
                            });
                        }
                        HostStatus::Disconnected | HostStatus::Unreachable => {
                            if let Some(cached) = crate::session::host_sessions_for(
                                &self.host_session_cache,
                                &host.target,
                            ) {
                                for cached_entry in cached.iter().filter(|s| !s.ephemeral) {
                                    let entry = crate::session::discovery::DiscoveredSession {
                                        name: cached_entry.name.clone(),
                                        ephemeral: false,
                                        host: Some(host.alias.clone()),
                                        remote_target: Some(host.target.clone()),
                                        status:
                                            crate::session::discovery::DiscoveredSessionStatus::Running {
                                                panes: cached_entry.panes,
                                                clients: 0,
                                                has_layout: false,
                                                created_from_profile: None,
                                            },
                                    };
                                    items.push(SidebarItemProjection {
                                        target: RowTarget::Session(Box::new(entry)),
                                        close: None,
                                    });
                                }
                            }
                        }
                    }
                }

                items.push(SidebarItemProjection {
                    target: RowTarget::Inert,
                    close: None,
                });
                items.push(SidebarItemProjection {
                    target: RowTarget::ConnectHost,
                    close: None,
                });
                items
            }
            // Mirrors `view::sidebar::user_tabs::launcher_rows`: a group change inserts a spacer
            // (except at the top) and a header ahead of the entry that opened the section.
            SidebarTab::Launcher { name, entries, .. } => {
                let mut items = Vec::new();
                let mut current: Option<&String> = None;
                for (entry_index, entry) in entries.iter().enumerate() {
                    if let Some(group) =
                        entry.group.as_ref().filter(|group| Some(*group) != current)
                    {
                        if !items.is_empty() {
                            items.push(SidebarItemProjection {
                                target: RowTarget::Inert,
                                close: None,
                            });
                        }
                        items.push(SidebarItemProjection {
                            target: RowTarget::Inert,
                            close: None,
                        });
                        current = Some(group);
                    }
                    items.push(SidebarItemProjection {
                        target: RowTarget::Launcher {
                            config_epoch: self.sidebar.config_epoch,
                            tab_id: name.clone(),
                            entry_index,
                        },
                        close: None,
                    });
                }
                items
            }
            SidebarTab::Command { name, on_click, .. } => {
                let Some(output) = self.fresh_command_output(name) else {
                    return Vec::new();
                };
                // Mirrors `view::sidebar::user_tabs::command_rows`: a header line becomes a
                // header preceded by a spacer, except at the top of the list.
                let mut items = Vec::new();
                for row in &output.rows {
                    if row.header {
                        if !items.is_empty() {
                            items.push(SidebarItemProjection {
                                target: RowTarget::Inert,
                                close: None,
                            });
                        }
                        items.push(SidebarItemProjection {
                            target: RowTarget::Inert,
                            close: None,
                        });
                        continue;
                    }
                    let target = if on_click.is_some() && !row.error {
                        RowTarget::CommandRow {
                            config_epoch: self.sidebar.config_epoch,
                            tab_id: name.clone(),
                            output_epoch: output.epoch,
                            line: row.raw.clone(),
                        }
                    } else {
                        RowTarget::Inert
                    };
                    items.push(SidebarItemProjection {
                        target,
                        close: None,
                    });
                }
                items
            }
            SidebarTab::Tree { .. } => Vec::new(),
        }
    }

    fn activity_item_projections(&self) -> Vec<SidebarItemProjection> {
        struct ActivityItem {
            target: RowTarget,
            host: Option<String>,
            path: Option<String>,
            rank: u8,
            workspace: usize,
            pane: usize,
            slot: usize,
        }

        fn rank(status: Option<&str>, finished: bool) -> u8 {
            let Some(status) = status.map(str::trim) else {
                return 5;
            };
            if finished
                && !status.eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING)
                && !status.eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED)
            {
                return 3;
            }
            if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED) {
                0
            } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING) {
                1
            } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::DONE) {
                3
            } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE) {
                4
            } else {
                2
            }
        }

        fn group_label(path: Option<&str>, host: Option<&str>) -> String {
            let label = path
                .and_then(|path| {
                    crate::platform::paths::path_segments(path)
                        .last()
                        .map(|segment| (*segment).to_string())
                })
                .unwrap_or_else(|| "Unknown".to_string());
            match host.filter(|host| !host.is_empty()) {
                Some(host) => format!("{label}@{host}"),
                None => label,
            }
        }

        let mut rows = Vec::new();
        for (workspace, workspace_state) in self.current().workspaces.iter().enumerate() {
            for (pane_index, pane) in workspace_state.panes.iter().enumerate() {
                if pane.id == crate::state::POPUP_PANE_ID
                    || pane.closing
                    || (pane.terminal.published_rows.is_empty()
                        && pane.terminal.detected_agent.is_none())
                {
                    continue;
                }
                let cwd = pane
                    .terminal
                    .cwd
                    .clone()
                    .filter(|cwd| !cwd.trim().is_empty());
                let path = pane.terminal.project_root.clone().or_else(|| cwd.clone());
                let host = cwd.as_ref().and_then(|_| pane.terminal.cwd_host.clone());
                if pane.terminal.published_rows.is_empty() {
                    rows.push(ActivityItem {
                        target: RowTarget::Pane(pane.id),
                        host,
                        path,
                        rank: rank(
                            pane.terminal.agent_status().as_deref(),
                            pane.terminal.finished_unseen,
                        ),
                        workspace,
                        pane: pane_index,
                        slot: 0,
                    });
                } else {
                    for (slot, published) in pane.terminal.published_rows.iter().enumerate() {
                        let finished = pane
                            .terminal
                            .published_row_ui
                            .get(&published.id)
                            .is_some_and(|ui| ui.finished_unseen);
                        rows.push(ActivityItem {
                            target: RowTarget::PublishedRow {
                                pane_id: pane.id,
                                row_id: published.id.clone(),
                            },
                            host: host.clone(),
                            path: path.clone(),
                            rank: rank(Some(&published.status), finished),
                            workspace,
                            pane: pane_index,
                            slot,
                        });
                    }
                }
            }
        }
        rows.sort_by_key(|row| (row.rank, row.workspace, row.pane, row.slot));

        let mut groups: Vec<(Option<String>, Option<String>, Vec<ActivityItem>)> = Vec::new();
        for row in rows {
            let existing = groups.iter_mut().find(|(host, path, _)| {
                *host == row.host
                    && match (path.as_deref(), row.path.as_deref()) {
                        (Some(left), Some(right)) => {
                            crate::platform::paths::paths_equal(left, right)
                        }
                        (None, None) => true,
                        _ => false,
                    }
            });
            if let Some((_, _, items)) = existing {
                items.push(row);
            } else {
                groups.push((row.host.clone(), row.path.clone(), vec![row]));
            }
        }
        groups.sort_by(|(host_a, path_a, _), (host_b, path_b, _)| {
            path_a
                .is_none()
                .cmp(&path_b.is_none())
                .then_with(|| {
                    group_label(path_a.as_deref(), host_a.as_deref())
                        .to_lowercase()
                        .cmp(&group_label(path_b.as_deref(), host_b.as_deref()).to_lowercase())
                })
                .then_with(|| path_a.cmp(path_b))
        });

        let show_headers =
            groups.len() > 1 || groups.first().is_some_and(|(_, path, _)| path.is_some());
        let mut items = Vec::new();
        for (index, (_, _, group)) in groups.into_iter().enumerate() {
            if index > 0 {
                items.push(SidebarItemProjection {
                    target: RowTarget::Inert,
                    close: None,
                });
            }
            if show_headers {
                items.push(SidebarItemProjection {
                    target: RowTarget::Inert,
                    close: None,
                });
            }
            items.extend(group.into_iter().map(|row| SidebarItemProjection {
                target: row.target,
                close: None,
            }));
        }
        items
    }
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
        self.tree_expanded.retain(|id, _| ids.contains(id));
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
            tabs: vec![
                SidebarTab::Activity,
                SidebarTab::Panes,
                SidebarTab::Sessions,
            ],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&old);
        state.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        state.command_output.insert(
            SidebarTabId::new("removed"),
            SidebarCommandOutput {
                epoch: 1,
                cwd: None,
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
                vec![SidebarTabId::new("activity"), SidebarTabId::new("panes")],
                vec![SidebarTabId::new("sessions")],
            ],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&config);
        state.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        assert!(state.transfer_tab(0, 1, 1, 1));
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("activity"))
        );
        assert_eq!(state.panels[1].active_tab, Some(SidebarTabId::new("panes")));
    }

    #[test]
    fn reorder_and_merge_preserve_tab_identity_and_selection() {
        let config = SidebarConfig {
            split: true,
            panels: vec![
                vec![SidebarTabId::new("activity"), SidebarTabId::new("panes")],
                vec![SidebarTabId::new("sessions")],
            ],
            ..SidebarConfig::default()
        };
        let mut state = SidebarState::new(&config);
        state.panels[0].active_tab = Some(SidebarTabId::new("activity"));
        assert!(state.reorder_tab(0, 0, 1));
        assert_eq!(
            state.panels[0].tabs,
            vec![SidebarTabId::new("panes"), SidebarTabId::new("activity")]
        );
        assert_eq!(
            state.panels[0].active_tab,
            Some(SidebarTabId::new("activity"))
        );

        state.active_panel = 1;
        state.set_split(false);
        assert_eq!(state.panels.len(), 1);
        assert_eq!(
            state.panels[0].tabs,
            vec![
                SidebarTabId::new("panes"),
                SidebarTabId::new("activity"),
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
                vec![SidebarTabId::new("activity")],
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
        let mut config = crate::config::Config::default();
        config.sidebar.visible = true;
        let mut state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        assert_eq!(state.content_viewport(viewport).w, 68);
        assert_eq!(state.terminal_content_left_offset(viewport), 32);

        state.config.sidebar.position = crate::config::SidebarPosition::Right;
        assert_eq!(state.content_viewport(viewport).w, 68);
        assert_eq!(state.terminal_content_left_offset(viewport), 0);
    }

    #[test]
    fn the_reserved_columns_follow_the_slide_so_the_pane_column_resizes_with_it() {
        let viewport = tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut config = crate::config::Config::default();
        config.sidebar.visible = true;
        let state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        assert_eq!(state.effective_sidebar_width(viewport), 32);
        assert_eq!(state.sidebar_slide_width(viewport), 32);

        // Part-way in, the layout hands over part of the column and the pane column keeps the rest.
        // Both together are always the whole viewport, so neither edge of the pane column is ever
        // off the screen and no gutter can open between them.
        for (progress, reserved) in [(0.0, 0), (0.25, 8), (0.5, 16), (0.75, 24), (1.0, 32)] {
            state.sidebar_slide.set(progress);
            assert_eq!(state.effective_sidebar_width(viewport), reserved);
            assert_eq!(state.content_viewport(viewport).w, 100 - reserved);
            assert_eq!(state.terminal_content_left_offset(viewport), reserved);
            // The panel is drawn at full width and clipped, never squeezed into what it has so far.
            assert_eq!(state.sidebar_slide_width(viewport), 32);
        }

        // Both clamp the same way on a viewport too narrow for the configured width, so the slide
        // never travels further than the column actually occupies.
        let narrow = tui_lipan::prelude::Rect { w: 40, ..viewport };
        assert_eq!(state.sidebar_slide_width(narrow), 20);
        assert_eq!(state.effective_sidebar_width(narrow), 20);
        state.sidebar_slide.set(0.5);
        assert_eq!(state.effective_sidebar_width(narrow), 10);
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
