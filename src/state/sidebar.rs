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
                let active = self.current().active_workspace;
                for (workspace_index, workspace) in self.current().workspaces.iter().enumerate() {
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
                    if ordered_ids.is_empty() && workspace_index != active {
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
                    items.push(SidebarItemProjection {
                        target: RowTarget::NewPane {
                            workspace: workspace_index,
                        },
                        close: None,
                    });
                }
                items
            }
            SidebarTab::Activity => self.activity_item_projections(),
            SidebarTab::Worktrees => self
                .worktree_tab_items()
                .into_iter()
                .map(|item| match item {
                    WorktreeTabItem::Checkout(row) => SidebarItemProjection {
                        close: row.closable.then(|| SidebarClose::Worktree {
                            path: row.tree.path.clone(),
                            force: row.force,
                        }),
                        target: RowTarget::Worktree(row.tree.path),
                    },
                    WorktreeTabItem::New => SidebarItemProjection {
                        target: RowTarget::NewWorktree,
                        close: None,
                    },
                    WorktreeTabItem::Header { .. }
                    | WorktreeTabItem::Creating { .. }
                    | WorktreeTabItem::Message(_) => SidebarItemProjection {
                        target: RowTarget::Inert,
                        close: None,
                    },
                })
                .collect(),
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

                for host in self.remote.hosts.iter() {
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
                    // Must match the view: cached last-seen rows are not evidence the host answered.
                    let has_live = live
                        .iter()
                        .any(|entry| !crate::ops::session::session_row_is_last_seen(entry));
                    let status = self
                        .remote
                        .hosts
                        .status_for(&host.target, conns.iter(), has_live);
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
                        HostStatus::Connecting | HostStatus::Installing => (RowTarget::Inert, None),
                    };
                    items.push(SidebarItemProjection {
                        target: header_target,
                        close: header_close,
                    });

                    match status {
                        HostStatus::Connecting | HostStatus::Installing => {}
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
                                &self.remote.session_cache,
                                &host.target,
                            ) {
                                for cached_entry in cached.iter().filter(|s| !s.ephemeral) {
                                    let entry = crate::session::discovery::DiscoveredSession {
                                        name: cached_entry.name.clone(),
                                        origin: Default::default(),
                                        ephemeral: false,
                                        host: Some(host.alias.clone()),
                                        remote_target: Some(host.target.clone()),
                                        status:
                                            crate::session::discovery::DiscoveredSessionStatus::Running {
                                                panes: cached_entry.panes,
                                                clients: 0,
                                                has_layout: false,
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

    pub(crate) fn activity_item_projections(&self) -> Vec<SidebarItemProjection> {
        struct ActivityItem {
            target: RowTarget,
            host: Option<String>,
            path: Option<String>,
            /// What the group is named after; see [`crate::state::AgentPlace::label_path`].
            label_path: Option<String>,
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
                let runtimes = pane.agent_runtimes();
                if pane.id == crate::state::POPUP_PANE_ID || pane.closing || runtimes.is_empty() {
                    continue;
                }
                for (slot, runtime) in runtimes.into_iter().enumerate() {
                    let place = pane.agent_place(&runtime);
                    let path = place.group_path().map(str::to_string);
                    let label_path = place.label_path().map(str::to_string);
                    let host = place.host;
                    let row_id = runtime.reference.slot.as_deref();
                    let finished = row_id.map_or(pane.terminal.finished_unseen, |row_id| {
                        pane.terminal
                            .published_row_ui
                            .get(row_id)
                            .is_some_and(|ui| ui.finished_unseen)
                    });
                    let target = match runtime.reference.slot {
                        Some(row_id) => RowTarget::PublishedRow {
                            pane_id: pane.id,
                            row_id,
                        },
                        None => RowTarget::Pane(pane.id),
                    };
                    rows.push(ActivityItem {
                        target,
                        host,
                        path,
                        label_path,
                        rank: rank(Some(runtime.state.as_str()), finished),
                        workspace,
                        pane: pane_index,
                        slot,
                    });
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
        // Every row of a group shares its root, and so the repository it is named after.
        let label = |host: &Option<String>, items: &[ActivityItem]| {
            let path = items.first().and_then(|item| item.label_path.as_deref());
            group_label(path, host.as_deref()).to_lowercase()
        };
        groups.sort_by(|(host_a, path_a, items_a), (host_b, path_b, items_b)| {
            path_a
                .is_none()
                .cmp(&path_b.is_none())
                .then_with(|| label(host_a, items_a).cmp(&label(host_b, items_b)))
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
    pub dock: crate::config::SidebarPosition,
    pub home: usize,
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
            dock: crate::config::SidebarPosition::Left,
            home: 0,
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

/// One line of the Worktrees tab. The view draws these and the semantic projection maps them, so
/// the two cannot disagree about which row an index names.
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
    pub active_panel: usize,
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
        let shown = config.startup.docks();
        let active_panel = panels
            .iter()
            .position(|panel| {
                shown[usize::from(panel.dock == crate::config::SidebarPosition::Right)]
            })
            .or_else(|| panels.iter().position(|panel| !panel.tabs.is_empty()))
            .unwrap_or(0);
        Self {
            panels,
            shown,
            restore: [false, false],
            active_panel,
            right_slide: Cell::new(if shown[1] { 1.0 } else { 0.0 }),
            ..Self::default()
        }
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
                    panel.page_rows = old.page_rows;
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
            ["activity", "sessions", "files", "git", "worktrees"]
        );
        assert!(state.panels[1].tabs.is_empty());
        state.panels[0].active_tab = Some(SidebarTabId::new("git"));
        config.layout.left.panel_count = 3;
        config.layout.left.panels.push(SidebarDockPanel::default());
        state.apply_configured_panels(&config);
        assert_eq!(state.active_tab(), Some(&SidebarTabId::new("git")));
        assert_eq!(state.active_panel, 1);
        assert_eq!(config.layout.left.panels[..2], saved.left.panels);
        assert_eq!(config.layout.right, saved.right);
    }

    #[test]
    fn geometry_uses_one_budget_and_restores_after_resize() {
        let mut config = crate::config::Config::default();
        config.sidebar.startup = crate::config::SidebarStartup::Both;
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
        assert!(state.transfer_tab(0, 2, 1, 0));
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
