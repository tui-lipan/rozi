//! The Agents tab: every agent this client knows about, in every session and on every connected
//! host, arranged by the checkout each one works in.
//!
//! Activity answers "what is happening in this session"; this tab answers "what are my agents
//! doing, wherever they run". It lists every agent, idle ones too, so a row stays where it is while
//! its agent works, waits, and finishes: the layout is something to learn, not a queue that empties.
//! Places are ordered by name, never by state, and expansion is the user's alone, so nothing moves
//! a folded checkout. Urgency shows through each row's glyph, the order of agents within a
//! checkout, and the counts a folded row keeps. Going to what needs you is the job of the
//! `focus-next-agent-needing-attention` action and the Agents view.
//!
//! ```text
//! host → checkout → agent cluster → run
//! ```
//!
//! A host heading appears only once another machine has agents, and an agent becomes a row of its
//! own only where it runs more than once in one checkout. The session a run lives in rides on its
//! row: it is where activating the row goes, not how the work is organized. The rows come from
//! [`crate::view::agents::global_agent_rows`], the projection the Agents view lists, so the two
//! surfaces never disagree about what exists, and a checkout is the one Projects counts the agent
//! under.

use tui_lipan::prelude::*;

use super::row::{Priority, Row, RowTarget, Shape, SidebarRow};
use crate::config::{SidebarAgentsConfig, SidebarTabId, WorktreeLayout};
use crate::session::protocol::pane_status;
use crate::session::remote::RemoteTarget;
use crate::state::{AgentLocation, State, TreeExpansionKey};
use crate::view::agents::GlobalAgentRow;

fn tab_id() -> SidebarTabId {
    SidebarTabId::new("agents")
}

/// One checkout's agents on one host, in the projection's order: most urgent first.
struct Checkout<'a> {
    /// What the checkout is, by its host's path rules: every spelling of one directory gives the
    /// same key. `None` for the agents outside any Git checkout, listed last as `Elsewhere`.
    key: Option<String>,
    /// The repository's key, as `key` is the checkout's.
    repository_key: Option<String>,
    /// The checkout's path as the first of its agents reported it, for showing.
    root: Option<&'a str>,
    repository: Option<&'a str>,
    branch: Option<&'a str>,
    /// The repository's name, disambiguated against the host's other repositories.
    label: String,
    agents: Vec<&'a GlobalAgentRow>,
}

impl Checkout<'_> {
    /// Where its open or folded state is kept: the checkout's path, never a label two can share.
    fn row_id(&self) -> String {
        self.key
            .as_deref()
            .map_or_else(|| "elsewhere".to_string(), |key| format!("checkout:{key}"))
    }

    fn is_primary(&self) -> bool {
        self.key.is_some() && self.key == self.repository_key
    }

    /// What names the checkout beside its repository: the branch, or its directory when detached
    /// commits and plain directories have none.
    fn branch_label(&self) -> Option<String> {
        self.branch
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string)
            .or_else(|| self.root.map(crate::state::projects::path_name))
    }
}

/// The host a row's agent runs on, `None` for this machine.
fn host_of<'a>(state: &'a State, row: &'a GlobalAgentRow) -> Option<&'a RemoteTarget> {
    match &row.location {
        AgentLocation::Here { .. } => state.current().remote_target.as_ref(),
        AgentLocation::OtherSession { target, .. } => target.as_ref(),
    }
}

pub(super) fn global_agents_rows(state: &State, config: &SidebarAgentsConfig) -> Vec<SidebarRow> {
    let agents = crate::view::agents::global_agent_rows(state);
    let mut hosts: Vec<Option<&RemoteTarget>> =
        agents.iter().map(|row| host_of(state, row)).collect();
    // This machine first, then hosts by name: an order that never depends on what agents do.
    hosts.sort_by_key(|host| host.map(|target| state.remote_target_label(target)));
    hosts.dedup();
    let headed = hosts.iter().any(Option::is_some);
    let mut rows = Vec::new();
    for host in hosts {
        let section = host.map_or_else(|| "local".to_string(), RemoteTarget::to_spec);
        if headed {
            let label = host.map_or_else(
                || "LOCAL".to_string(),
                |target| state.remote_target_label(target).to_uppercase(),
            );
            rows.push(
                SidebarRow::header(super::row::header(state, label, false))
                    .shape(Shape::Heading)
                    .apart()
                    .keyed(format!("host:{section}")),
            );
        }
        let on_host: Vec<&GlobalAgentRow> = agents
            .iter()
            .filter(|row| host_of(state, row) == host)
            .collect();
        let layout = Layout {
            state,
            section,
            width: state.sidebar_tab_width(&tab_id()),
        };
        let checkouts = checkouts(&on_host);
        let first = rows.len();
        match config.worktrees {
            WorktreeLayout::Separate => {
                for checkout in &checkouts {
                    let shared = shares_label(&checkouts, checkout);
                    layout.checkout(checkout, shared, &mut rows);
                }
            }
            WorktreeLayout::Repository => layout.by_repository(&checkouts, &mut rows),
        }
        // Checkouts stand apart from one another; the first sits right under its host's heading.
        if headed && let Some(row) = rows.get_mut(first) {
            row.gap_before = 0;
        }
    }
    rows
}

/// A host's agents by checkout: repositories by name, each one's primary checkout first and its
/// worktrees by branch, then `Elsewhere`.
fn checkouts<'a>(agents: &[&'a GlobalAgentRow]) -> Vec<Checkout<'a>> {
    let mut checkouts: Vec<Checkout<'a>> = Vec::new();
    for agent in agents {
        let checkout = agent.checkout.as_ref();
        let key = checkout.map(crate::view::agents::AgentCheckout::root_key);
        match checkouts.iter_mut().find(|known| known.key == key) {
            Some(known) => known.agents.push(agent),
            None => {
                checkouts.push(Checkout {
                    key,
                    repository_key: checkout
                        .map(crate::view::agents::AgentCheckout::repository_key),
                    root: checkout.map(|checkout| checkout.root.as_str()),
                    repository: checkout.map(|checkout| checkout.repository.as_str()),
                    branch: checkout.and_then(|checkout| checkout.branch.as_deref()),
                    label: String::new(),
                    agents: vec![agent],
                });
            }
        }
    }
    // One spelling per repository, the first one seen, so its label is derived once.
    let mut repositories: Vec<(&str, &str)> = Vec::new();
    for checkout in &checkouts {
        if let (Some(key), Some(path)) = (checkout.repository_key.as_deref(), checkout.repository)
            && !repositories.iter().any(|(known, _)| *known == key)
        {
            repositories.push((key, path));
        }
    }
    repositories.sort_unstable();
    let paths: Vec<&str> = repositories.iter().map(|(_, path)| *path).collect();
    let labels = crate::state::projects::repository_labels(&paths);
    let labels: Vec<(String, String)> = checkouts
        .iter()
        .map(|checkout| match checkout.repository_key.as_deref() {
            Some(key) => repositories
                .iter()
                .position(|(known, _)| *known == key)
                .map(|index| (key.to_string(), labels[index].clone()))
                .unwrap_or_default(),
            None => (String::new(), "Elsewhere".to_string()),
        })
        .collect();
    for (checkout, (_, label)) in checkouts.iter_mut().zip(labels) {
        checkout.label = label;
    }
    checkouts.sort_by_cached_key(|checkout| {
        (
            checkout.key.is_none(),
            checkout.label.to_lowercase(),
            checkout.repository_key.clone(),
            !checkout.is_primary(),
            checkout.branch_label(),
        )
    });
    checkouts
}

/// Several checkouts of one repository read alike but for the branch, so a long repository name
/// gives it a share of the row.
fn shares_label(checkouts: &[Checkout<'_>], checkout: &Checkout<'_>) -> bool {
    checkout.repository_key.is_some()
        && checkouts
            .iter()
            .filter(|other| other.repository_key == checkout.repository_key)
            .count()
            > 1
}

/// What a row that folds agents beneath it names, which decides how it reads.
#[derive(Clone, Copy)]
enum Label {
    /// A repository or checkout.
    Project,
    /// One checkout's branch beneath its repository.
    Branch,
    /// The agents in no Git checkout.
    Elsewhere,
}

/// What every row on one host needs to place itself.
struct Layout<'a> {
    state: &'a State,
    /// Namespaces expansion state by host, so `rozi` here and on another machine fold apart.
    section: String,
    width: u16,
}

impl Layout<'_> {
    fn key(&self, row: String) -> TreeExpansionKey {
        TreeExpansionKey {
            tab: tab_id(),
            section: self.section.clone(),
            row,
        }
    }

    fn expanded(&self, key: &TreeExpansionKey) -> bool {
        self.state
            .sidebar
            .tree_rows_expanded
            .get(key)
            .copied()
            .unwrap_or(true)
    }

    /// One checkout as a top-level row named by its repository, with its branch on the right.
    fn checkout(&self, checkout: &Checkout<'_>, shared: bool, rows: &mut Vec<SidebarRow>) {
        let key = self.key(checkout.row_id());
        let expanded = self.expanded(&key);
        let priority = if shared {
            Priority::Balanced
        } else {
            Priority::Label
        };
        rows.push(
            self.group_row(
                checkout.label.clone(),
                checkout.repository.and(checkout.branch_label()),
                if checkout.root.is_none() {
                    Label::Elsewhere
                } else {
                    Label::Project
                },
                &checkout.agents,
                key,
                expanded,
                priority,
            )
            .apart(),
        );
        if expanded {
            self.agent_rows(checkout, 1, rows);
        }
    }

    /// One row per repository, its checkouts as branch lines beneath it. A repository with one
    /// checkout, and the agents in none, read as they would laid out separately.
    fn by_repository(&self, checkouts: &[Checkout<'_>], rows: &mut Vec<SidebarRow>) {
        let mut placed: Vec<&str> = Vec::new();
        for checkout in checkouts {
            let Some(repository) = checkout.repository_key.as_deref() else {
                self.checkout(checkout, false, rows);
                continue;
            };
            if placed.contains(&repository) {
                continue;
            }
            placed.push(repository);
            let siblings: Vec<&Checkout<'_>> = checkouts
                .iter()
                .filter(|other| other.repository_key.as_deref() == Some(repository))
                .collect();
            if let [only] = siblings.as_slice() {
                self.checkout(only, false, rows);
                continue;
            }
            let key = self.key(format!("repository:{repository}"));
            let expanded = self.expanded(&key);
            let agents: Vec<&GlobalAgentRow> = siblings
                .iter()
                .flat_map(|sibling| sibling.agents.iter().copied())
                .collect();
            rows.push(
                self.group_row(
                    checkout.label.clone(),
                    None,
                    Label::Project,
                    &agents,
                    key,
                    expanded,
                    Priority::Label,
                )
                .apart(),
            );
            if !expanded {
                continue;
            }
            for sibling in siblings {
                let key = self.key(sibling.row_id());
                let expanded = self.expanded(&key);
                // A long branch keeps its end, where the line of work is named. The budget is the
                // tab less the gutter, the guide, the arrow, and the divider and scrollbar columns.
                let branch = sibling.branch_label().unwrap_or_default();
                let branch = super::row::truncate_start(
                    &branch,
                    usize::from(self.width).saturating_sub(9).max(8),
                );
                rows.push(
                    self.group_row(
                        branch,
                        None,
                        Label::Branch,
                        &sibling.agents,
                        key,
                        expanded,
                        Priority::Label,
                    )
                    .at(1),
                );
                if expanded {
                    self.agent_rows(sibling, 2, rows);
                }
            }
        }
    }

    /// A row that folds what is beneath it. Folded, it counts its agents by state - idle ones too,
    /// since this tab lists every agent - and its arrow takes the most urgent one's color.
    #[allow(clippy::too_many_arguments)]
    fn group_row(
        &self,
        label: String,
        note: Option<String>,
        kind: Label,
        agents: &[&GlobalAgentRow],
        key: TreeExpansionKey,
        expanded: bool,
        priority: Priority,
    ) -> SidebarRow {
        let theme = &self.state.theme;
        let dim = super::super::fg_only(&theme.muted).dim();
        // Projects read as headings, in Rozi's colour as Activity's do; a branch beneath its
        // repository and the agents in no checkout stay quiet.
        let title_style = match kind {
            Label::Project => super::super::rozi_fg(theme).bold(),
            Label::Branch | Label::Elsewhere => super::super::fg_only(&theme.muted),
        };
        let counts = if expanded {
            Vec::new()
        } else {
            status_counts(agents, theme)
        };
        let mut parts: Vec<(String, Style)> = note.into_iter().map(|note| (note, dim)).collect();
        if !parts.is_empty() && !counts.is_empty() {
            parts.push(("·".into(), dim));
        }
        parts.extend(counts);
        let content = Row::new(label)
            .active(!expanded && agents.iter().any(|agent| is_active(self.state, agent)))
            .title_style(title_style)
            .priority(priority)
            // A long branch keeps its end, where the line of work is named.
            .badge_keeps_tail()
            .badge_parts(parts);
        let row = SidebarRow::item(content, RowTarget::TreeToggle(key.clone()))
            .keyed(format!("{}:{}", key.section, key.row))
            .collapsible(key, expanded);
        // A repository's agents arrive checkout by checkout, so the lead is chosen, not assumed.
        let lead = agents.iter().copied().min_by_key(|agent| agent.rank());
        match lead.filter(|_| !expanded) {
            Some(lead) => {
                let (_, color, _) =
                    super::agents::row_glyph(status(lead), lead.finished_unseen, theme);
                row.arrow_style(Style::new().fg(color))
            }
            None => row,
        }
    }

    /// A checkout's agents at `depth`: a row each, and a cluster wherever one agent runs more than
    /// once there. The cluster takes the place of its most urgent run.
    ///
    /// In a checkout, every run of one agent there is one cluster, whichever session or pane hosts
    /// it. Outside any checkout nothing says two panes are related, so only one pane's own runs
    /// cluster, as in Activity.
    fn agent_rows(&self, checkout: &Checkout<'_>, depth: usize, rows: &mut Vec<SidebarRow>) {
        let cluster = |agent: &GlobalAgentRow| match checkout.key {
            Some(_) => agent.agent_id.clone(),
            None => format!(
                "{}|{}|{}",
                agent.session,
                agent.location.pane(),
                agent.agent_id
            ),
        };
        let mut placed: Vec<String> = Vec::new();
        for agent in &checkout.agents {
            let id = cluster(agent);
            let members: Vec<&GlobalAgentRow> = checkout
                .agents
                .iter()
                .copied()
                .filter(|other| cluster(other) == id)
                .collect();
            if members.len() < 2 {
                rows.push(self.run_row(agent, false).at(depth));
                continue;
            }
            if placed.contains(&id) {
                continue;
            }
            let key = self.key(format!("cluster:{}|{id}", checkout.row_id()));
            placed.push(id);
            let expanded = self.expanded(&key);
            rows.push(self.cluster_row(&members, key, expanded).at(depth));
            if expanded {
                rows.extend(
                    members
                        .iter()
                        .map(|member| self.run_row(member, true).at(depth + 1)),
                );
            }
        }
    }

    /// An agent that runs more than once in one checkout: its name and its most urgent run's glyph.
    /// Folded, it counts its runs by state, idle ones included.
    fn cluster_row(
        &self,
        members: &[&GlobalAgentRow],
        key: TreeExpansionKey,
        expanded: bool,
    ) -> SidebarRow {
        let theme = &self.state.theme;
        let lead = members[0];
        let (glyph, color, spinner) =
            super::agents::row_glyph(status(lead), lead.finished_unseen, theme);
        let mut content = Row::new(lead.agent.clone())
            .active(!expanded && members.iter().any(|member| is_active(self.state, member)))
            .title_style(super::super::fg_only(&theme.primary))
            .glyph(super::agents::status_icon(glyph, color, spinner));
        if !expanded {
            content = content.badge_parts(status_counts(members, theme));
        }
        let row = SidebarRow::item(content, RowTarget::TreeToggle(key.clone()))
            .keyed(format!("{}:{}", key.section, key.row))
            .collapsible(key, expanded);
        if expanded {
            row
        } else {
            row.arrow_style(Style::new().fg(color))
        }
    }

    /// One agent run: what it works on and the session it is in, then its state and time and why it
    /// is in it.
    ///
    /// On its own, a run that named its work ends its detail line with the agent's name, the first
    /// thing to give way. Beneath a cluster, which already names the agent, it leaves it out.
    fn run_row(&self, row: &GlobalAgentRow, in_cluster: bool) -> SidebarRow {
        let theme = &self.state.theme;
        let title_style = super::super::fg_only(&theme.primary);
        let agent = (!in_cluster && row.task.is_some()).then(|| row.agent.clone());
        let mut content = super::agents::RunLines {
            title: row.title(),
            title_style: if row.background {
                title_style.italic()
            } else {
                title_style
            },
            location: Some(crate::view::vocabulary::session_name(&row.session).to_string()),
            status: status(row),
            finished_unseen: row.finished_unseen,
            duration: row.elapsed.map(super::agents::format_age),
            notes: row.reason.clone().into_iter().chain(agent).collect(),
            active: is_active(self.state, row),
        }
        .row(theme, self.width);
        if row.background {
            content = content.detail_badge(
                crate::view::vocabulary::BACKGROUND_RUN,
                super::super::fg_only(&theme.muted).dim(),
            );
        }
        SidebarRow::item(content, RowTarget::Agent(row.location.clone()))
            .keyed(format!("agent:{:?}", row.location))
    }
}

/// The row's status, an agent that reported none reading as idle.
fn status(row: &GlobalAgentRow) -> &str {
    if row.status.trim().is_empty() {
        pane_status::IDLE
    } else {
        &row.status
    }
}

fn status_counts(agents: &[&GlobalAgentRow], theme: &Theme) -> Vec<(String, Style)> {
    super::agents::status_count_parts(
        agents
            .iter()
            .map(|agent| (status(agent), agent.finished_unseen)),
        true,
        theme,
    )
}

fn is_active(state: &State, row: &GlobalAgentRow) -> bool {
    row.is_on_screen(state)
}
