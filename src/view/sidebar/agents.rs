use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, Shape, SidebarRow};
use crate::platform::paths::path_segments;
use crate::session::protocol::pane_status;
use crate::state::{PaneId, State, TreeExpansionKey};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentRow {
    pub pane_id: PaneId,
    pub workspace_index: usize,
    pub pane_index: usize,
    pub title: String,
    pub status: Option<String>,
    /// What the agent is doing right now, for the detail line. See [`activity_text`].
    pub activity: Option<String>,
    /// How long the current status has held, sampled when the row was built.
    pub age: Option<std::time::Duration>,
    /// How long the agent's last completed run took. Fixed once banked; see [`row_duration`].
    pub run: Option<std::time::Duration>,
    pub cwd: Option<String>,
    pub cwd_host: Option<String>,
    /// Git project containing `cwd`, as the session server resolved it. This is the grouping key
    /// where it exists, so agents working in `repo` and `repo/src` share one project.
    pub project_root: Option<String>,
    /// Where in the project this agent sits (`src`, `services/api`), `None` at its root. In a
    /// monorepo this is the only thing telling two rows of the same project apart.
    pub subpath: Option<String>,
    pub branch: Option<String>,
    /// Primary checkout when `project_root` is a linked worktree of it. Names the group, so a
    /// worktree reads as the repository it belongs to; `branch` then tells the groups apart.
    pub repository: Option<String>,
    /// The agent finished a run (went quiescent) since the pane was last focused; drives the filled
    /// attention pulse until the pane is looked at.
    pub finished_unseen: bool,
    /// Which published slot this row stands for, when its pane published any. `None` is the
    /// one-row-per-pane path every agent that publishes nothing keeps taking.
    pub slot: Option<SlotRef>,
    /// Who the agent is, for an agent row; `None` for an activity. Only agents reach the Agents
    /// view, and rows of one agent identity cluster together.
    pub identity: Option<crate::session::protocol::AgentIdentity>,
    /// What the agent works on, when it named its work: its published row's title, never a
    /// terminal title. See [`crate::session::protocol::AgentRuntime::task`].
    pub task: Option<String>,
    /// Why the occupant is in its state, as it said (`Permission required`).
    pub reason: Option<String>,
    /// Position among its pane's occupants, so one pane's rows keep the projection's order.
    pub order: usize,
    /// What a pane's own agent shows as its terminal title, when that says more than its name or
    /// its directory. `None` for a published row, whose title is its task.
    pub screen_title: Option<String>,
}

/// A row's identity within a pane that publishes several agents or activities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlotRef {
    pub key: crate::session::protocol::RowKey,
    /// Publish order, so one pane's rows keep its tab order among themselves.
    pub index: usize,
    /// The slot the publisher currently has on screen. This, not pane focus, is what makes a row
    /// read as the active one: focusing the pane only ever reveals one of its rows.
    pub active: bool,
}

/// Agents that share a project, herdr-style "space" grouping. `project` is `None` for the trailing
/// fallback group of agents whose pane has no usable cwd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentGroup {
    /// The host and directory the group stands for, `None` for the fallback group. Unlike the label,
    /// which two unrelated projects can share, this names one place.
    pub key: Option<String>,
    /// The host and repository the group's checkout belongs to - its primary checkout for a linked
    /// worktree - so a repository's checkouts can be laid out together. `None` for the fallback
    /// group.
    pub repository: Option<String>,
    pub project: Option<String>,
    /// Branch checked out in the group's project, shown on its header.
    pub branch: Option<String>,
    pub rows: Vec<AgentRow>,
}

fn normalized_status(value: &str) -> &str {
    value.trim()
}

/// Activity ordering shared with keyboard activation, including unseen successful finishes.
pub(crate) fn status_rank(status: Option<&str>, finished_unseen: bool) -> u8 {
    status.map_or(5, |status| {
        crate::session::protocol::ActivityState::from_status(status)
            .presentation_rank(finished_unseen)
    })
}

/// What the agent is currently doing, for the row's detail line.
///
/// The reason it published alongside its status is the authoritative answer, but only agents with a
/// status integration set one. Everything else falls back to the terminal title: agents write their
/// current task there, which makes it the only activity signal a detected-only agent offers.
#[cfg(test)]
fn activity_text(pane: &crate::pane::TerminalPane, kind_label: &str) -> Option<String> {
    semantic_activity_text(pane, kind_label, None)
}

fn semantic_activity_text(
    pane: &crate::pane::TerminalPane,
    kind_label: &str,
    semantic_reason: Option<&str>,
) -> Option<String> {
    if let Some(reason) = semantic_reason
        .or_else(|| {
            pane.reported_status
                .as_ref()
                .and_then(|status| status.reason.as_deref())
        })
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        return Some(reason.to_string());
    }
    screen_title(pane, kind_label)
}

/// The pane's terminal title, when it says more than the agent's name or the directory it is in:
/// agents write their current task there, which makes it the only work a detected-only agent names.
fn screen_title(pane: &crate::pane::TerminalPane, kind_label: &str) -> Option<String> {
    let title = pane.title()?;
    let title = strip_agent_title_prefix(strip_title_decoration(&title), kind_label);
    if title.is_empty()
        || title.eq_ignore_ascii_case(kind_label)
        || is_cwd_echo(title, pane.cwd.as_deref())
    {
        return None;
    }
    Some(title.to_string())
}

/// What a published row is doing, for its detail line.
///
/// The title wins here, unlike everywhere else, because it is the only thing that says *which* of a
/// pane's agents this row is: the name column now reads `OpenCode #2` for every one of them. A
/// reason stands in until the agent has titled its work — a fresh session is `blocked` on its first
/// question before it has a title to show — and the glyph already carries the state either way, so
/// nothing is lost by yielding to the title once one exists.
///
/// A title that only repeats the agent's own name says nothing the row does not already.
fn row_activity(row: &crate::session::protocol::PublishedRow, kind_label: &str) -> Option<String> {
    let title = strip_agent_title_prefix(strip_title_decoration(&row.title), kind_label);
    if !title.is_empty() && !title.eq_ignore_ascii_case(kind_label) {
        return Some(title.to_string());
    }
    row.reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string)
}

/// Drop the status glyph agents prefix their title with (`✳`, `⏺`, `●`). The row already has a
/// glyph column, so a second one beside the text is noise. Only non-ASCII symbols go — ASCII
/// punctuation is load-bearing in a real title (`~/repo`, `[2/7] running tests`).
fn strip_title_decoration(title: &str) -> &str {
    title
        .trim_start_matches(|ch: char| {
            ch.is_whitespace() || (!ch.is_ascii() && !ch.is_alphanumeric())
        })
        .trim()
}

/// OpenCode reserves the beginning of its terminal title for its own short label. The sidebar has
/// already named the row `OpenCode`, so keep only the useful title text there.
fn strip_agent_title_prefix<'a>(title: &'a str, kind_label: &str) -> &'a str {
    if kind_label.eq_ignore_ascii_case("OpenCode") {
        title
            .strip_prefix("OC |")
            .map(str::trim_start)
            .unwrap_or(title)
    } else {
        title
    }
}

/// A task as rows present it: without the status glyph or the agent's own label prefix an agent
/// decorates its titles with, and `None` when nothing is left that the agent's name does not
/// already say.
pub(crate) fn task_title(task: &str, label: &str) -> Option<String> {
    let title = strip_agent_title_prefix(strip_title_decoration(task), label);
    (!title.is_empty() && !title.eq_ignore_ascii_case(label)).then(|| title.to_string())
}

/// Whether a terminal title is just the working directory. A shell sets its title to `$PWD`, so an
/// agent that never set one of its own leaves a stale path there — and the project header already
/// says where the row is, so echoing it into the activity slot spends the line on nothing.
fn is_cwd_echo(title: &str, cwd: Option<&str>) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    if crate::platform::paths::paths_equal(title, cwd) {
        return true;
    }
    // `~/repo` against `/home/you/repo`: the last segment is what the two spellings share. A
    // one-word activity that happens to match the directory name is lost to this, which is a fair
    // trade for never presenting a path as a task.
    match (path_segments(title).last(), path_segments(cwd).last()) {
        (Some(title), Some(cwd)) => title.eq_ignore_ascii_case(cwd),
        _ => false,
    }
}

/// Every activity in the current session, agents included: the Activity tab's rows.
///
/// Built from [`crate::state::Pane::occupants`], the activity projection. The Agents view keeps
/// only the rows with [`AgentRow::agent`] set, which are exactly the agent projection's.
pub(crate) fn agent_rows(state: &State) -> Vec<AgentRow> {
    let mut rows = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(workspace_index, workspace)| {
            workspace
                .panes
                .iter()
                .enumerate()
                .filter(|(_, pane)| pane.id != crate::state::POPUP_PANE_ID && !pane.closing)
                .flat_map(move |(pane_index, pane)| {
                    // Presented as merged with the hooks' report, the same as the occupant's state,
                    // so the status, activity, age, and sort order never disagree with automation.
                    let effective = pane.terminal.effective_rows();
                    pane.occupants()
                        .into_iter()
                        .enumerate()
                        .map(move |(order, occupant)| {
                            let place = pane.agent_place(occupant.slot.as_ref());
                            let subpath = place
                                .project_root
                                .as_deref()
                                .zip(place.cwd.as_deref())
                                .and_then(|(root, cwd)| {
                                    crate::platform::paths::project_relative_path(root, cwd)
                                });
                            let slot = occupant.slot.as_ref().and_then(|key| {
                                effective
                                    .iter()
                                    .enumerate()
                                    .find(|(_, row)| row.has_key(key))
                            });
                            let kind_label = occupant
                                .agent
                                .as_ref()
                                .map_or("", |identity| identity.label.as_str());
                            let (title, status, activity, age, run, finished_unseen, slot) =
                                if let Some(record) = &occupant.program_status {
                                    let message = occupant.reason.as_deref().unwrap_or("");
                                    let detail = record.progress.map_or_else(
                                        || message.to_string(),
                                        |p| {
                                            if message.is_empty() {
                                                format!("{p}%")
                                            } else {
                                                format!("{message} · {p}%")
                                            }
                                        },
                                    );
                                    (
                                        if occupant.label.is_empty() {
                                            pane.display_title(pane.terminal.title())
                                        } else {
                                            crate::session::protocol::program_status_display(
                                                &occupant.label,
                                            )
                                        },
                                        record.state.as_str().to_string(),
                                        (!detail.is_empty()).then(|| {
                                            crate::session::protocol::program_status_display(
                                                &detail,
                                            )
                                        }),
                                        None,
                                        None,
                                        pane.terminal.program_status_attention,
                                        None,
                                    )
                                } else if let Some((index, published_row)) = slot {
                                    let key = published_row.key();
                                    let ui = pane.terminal.published_row_ui.get(&key);
                                    (
                                        // Behind a detected agent its rows are that agent's
                                        // sessions: named for the agent, told apart by their task.
                                        if occupant.is_agent()
                                            && pane.terminal.detected_agent.is_some()
                                        {
                                            kind_label.to_string()
                                        } else if published_row.title.trim().is_empty() {
                                            pane.display_title(pane.terminal.title())
                                        } else {
                                            published_row.title.clone()
                                        },
                                        published_row.status.clone(),
                                        row_activity(published_row, kind_label),
                                        pane.terminal.row_age(published_row),
                                        ui.and_then(|ui| ui.last_run),
                                        pane.row_finished_unseen(&key, &state.agent_attention),
                                        Some(SlotRef {
                                            key,
                                            index,
                                            active: published_row.active,
                                        }),
                                    )
                                } else {
                                    (
                                        if occupant.is_agent() {
                                            occupant.label.clone()
                                        } else {
                                            pane.display_title(pane.terminal.title())
                                        },
                                        pane.terminal
                                            .agent_status()
                                            .unwrap_or_else(|| occupant.state.as_str().to_string()),
                                        semantic_activity_text(
                                            &pane.terminal,
                                            kind_label,
                                            occupant.reason.as_deref(),
                                        ),
                                        pane.terminal.status_age(),
                                        pane.terminal.last_run,
                                        if occupant.is_agent() {
                                            pane.agent_runtimes().iter().any(|runtime| {
                                                runtime.reference.slot.is_none()
                                                    && state
                                                        .agent_attention
                                                        .runtime_finished_unseen(runtime)
                                            })
                                        } else {
                                            pane.terminal.finished_unseen
                                        },
                                        None,
                                    )
                                };
                            let screen_title = (occupant.is_agent() && slot.is_none())
                                .then(|| screen_title(&pane.terminal, kind_label))
                                .flatten();
                            AgentRow {
                                pane_id: pane.id,
                                workspace_index,
                                pane_index,
                                title,
                                status: Some(status),
                                activity,
                                age,
                                run,
                                cwd_host: place.host,
                                cwd: place.cwd,
                                branch: place.project_root.as_ref().and(place.branch),
                                repository: place.project_root.as_ref().and(place.repository),
                                project_root: place.project_root,
                                subpath,
                                finished_unseen,
                                slot,
                                task: occupant.agent.as_ref().and_then(|identity| {
                                    crate::session::protocol::occupant_task(
                                        &pane.terminal.published_rows,
                                        occupant.slot.as_ref(),
                                        &identity.label,
                                    )
                                }),
                                reason: occupant.reason.clone(),
                                identity: occupant.agent.clone(),
                                order,
                                screen_title,
                            }
                        })
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        (
            status_rank(row.status.as_deref(), row.finished_unseen),
            row.workspace_index,
            row.pane_index,
            row.order,
        )
    });
    rows
}

fn agent_target(row: &AgentRow) -> RowTarget {
    match &row.slot {
        Some(slot) => RowTarget::PublishedRow {
            pane_id: row.pane_id,
            row: slot.key.clone(),
        },
        None => RowTarget::Pane(row.pane_id),
    }
}

/// Which agent cluster a row belongs to within its group: the agent's identity, and in the
/// fallback group its pane too. Activities never cluster.
///
/// In a known checkout, every run of one agent working there is one cluster, whichever pane hosts
/// it: four Claude Code conversations in `rozi (master)` are four jobs in one place, not three in
/// one terminal and one in another. Without a known place nothing says two panes are related, so
/// only one pane's own sessions cluster there.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClusterKey {
    pane: Option<PaneId>,
    agent: String,
}

fn cluster_key(row: &AgentRow, known_place: bool) -> Option<ClusterKey> {
    let identity = row.identity.as_ref()?;
    Some(ClusterKey {
        pane: (!known_place).then_some(row.pane_id),
        agent: identity.id.clone(),
    })
}

/// Where a cluster's open or folded state is kept: by the place it stands for and its agent, never
/// by a display label that two places can share, so it survives panes coming and going.
fn cluster_expansion_key(group: Option<&str>, key: &ClusterKey) -> TreeExpansionKey {
    let place = match (group, key.pane) {
        (Some(group), _) => group.to_string(),
        (None, Some(pane)) => format!("pane:{pane}"),
        (None, None) => String::new(),
    };
    TreeExpansionKey {
        tab: crate::config::SidebarTab::activity().id(),
        section: String::new(),
        row: format!("cluster:{place}|{}", key.agent),
    }
}

/// Group agent rows by project (host + project root, falling back to the working directory when the
/// pane is not in one). Grouping on the *root* is what puts an agent in `repo/src` beside one in
/// `repo` instead of inventing a project called `src`; where the two differ, the row carries the
/// project-relative remainder.
///
/// Group order is stable — alphabetical by label, fallback group last — never by status, so blocks
/// do not jump while agents flip between states; urgency surfaces through the within-group row sort
/// instead. Row order inside a group keeps the [`agent_rows`] status sort.
pub(crate) fn agent_groups(state: &State) -> Vec<AgentGroup> {
    struct Keyed {
        host: Option<String>,
        /// Project root where there is one, else the raw cwd: what the label is derived from and
        /// what rows are matched against.
        path: String,
        /// The repository a linked worktree belongs to, else `path`: what the label names, and
        /// what tells a worktree of `rozi` apart from an unrelated project that is also `rozi`.
        repository: String,
        rows: Vec<AgentRow>,
    }
    let mut known: Vec<Keyed> = Vec::new();
    let mut unknown: Vec<AgentRow> = Vec::new();
    for row in agent_rows(state) {
        match row.project_root.clone().or_else(|| row.cwd.clone()) {
            Some(path) => {
                let host = row.cwd_host.clone();
                if let Some(group) = known.iter_mut().find(|group| {
                    group.host == host && crate::platform::paths::paths_equal(&group.path, &path)
                }) {
                    group.rows.push(row);
                } else {
                    known.push(Keyed {
                        host,
                        repository: row.repository.clone().unwrap_or_else(|| path.clone()),
                        path,
                        rows: vec![row],
                    });
                }
            }
            None => unknown.push(row),
        }
    }

    let mut labels: Vec<String> = known
        .iter()
        .map(|group| project_label(&group.repository, group.host.as_deref(), false))
        .collect();
    // Disambiguate duplicate final labels with one parent segment (VS Code-style); a residual
    // collision after that is accepted — group identity is the full path, the label is display.
    // Checkouts of one repository share a name on purpose: the branch on each header is what
    // tells them apart, and a parent segment would only name the worktree directory again.
    let ambiguous: Vec<bool> = known
        .iter()
        .zip(&labels)
        .map(|(group, label)| {
            known.iter().zip(&labels).any(|(other, other_label)| {
                other_label.eq_ignore_ascii_case(label)
                    && (other.host != group.host
                        || !crate::platform::paths::paths_equal(
                            &other.repository,
                            &group.repository,
                        ))
            })
        })
        .collect();
    for (index, group) in known.iter().enumerate() {
        if ambiguous[index] {
            labels[index] = project_label(&group.repository, group.host.as_deref(), true);
        }
    }

    let mut groups: Vec<(String, AgentGroup)> = known
        .into_iter()
        .zip(labels)
        .map(|(keyed, label)| {
            // Every row in the group shares a root and so a branch; the first that has one answers
            // for the group, which also covers a client that attached before the server had read it.
            let branch = keyed
                .rows
                .iter()
                .find_map(|row| row.branch.clone())
                .filter(|branch| !branch.trim().is_empty());
            (
                keyed.path.clone(),
                AgentGroup {
                    key: Some(format!(
                        "{}|{}",
                        keyed.host.as_deref().unwrap_or_default(),
                        keyed.path
                    )),
                    repository: Some(format!(
                        "{}|{}",
                        keyed.host.as_deref().unwrap_or_default(),
                        keyed.repository
                    )),
                    project: Some(label),
                    branch,
                    rows: keyed.rows,
                },
            )
        })
        .collect();
    groups.sort_by(|(path_a, a), (path_b, b)| {
        let label_a = a.project.as_deref().unwrap_or_default();
        let label_b = b.project.as_deref().unwrap_or_default();
        label_a
            .to_lowercase()
            .cmp(&label_b.to_lowercase())
            .then_with(|| path_a.cmp(path_b))
    });
    let mut groups: Vec<AgentGroup> = groups.into_iter().map(|(_, group)| group).collect();
    if !unknown.is_empty() {
        groups.push(AgentGroup {
            key: None,
            repository: None,
            project: None,
            branch: None,
            rows: unknown,
        });
    }
    groups
}

/// Display label for a project group: the directory basename, optionally prefixed with its parent
/// segment for disambiguation, plus an `@host` suffix for a remote cwd. A root-only path (`/`,
/// `C:\`) has no basename and shows the path itself.
fn project_label(cwd: &str, host: Option<&str>, with_parent: bool) -> String {
    let segments = path_segments(cwd);
    let name = match segments.split_last() {
        Some((last, rest)) => match rest.last() {
            Some(parent) if with_parent => format!("{parent}/{last}"),
            _ => (*last).to_string(),
        },
        None => cwd.to_string(),
    };
    match host {
        Some(host) if !host.is_empty() => format!("{name}@{host}"),
        _ => name,
    }
}

pub(crate) fn status_glyph(value: &str, theme: &Theme) -> (&'static str, Color) {
    let value = normalized_status(value);
    if value.eq_ignore_ascii_case(pane_status::WORKING) {
        ("⠋", theme.status.info)
    } else if value.eq_ignore_ascii_case(pane_status::BLOCKED) {
        ("▲", theme.status.warning)
    } else if value.eq_ignore_ascii_case("error") {
        ("×", theme.status.error)
    } else if value.eq_ignore_ascii_case(pane_status::DONE) {
        ("✓", theme.status.success)
    } else if value.eq_ignore_ascii_case(pane_status::IDLE) {
        (
            "○",
            theme
                .muted
                .fg
                .or(theme.primary.fg)
                .map(|paint| paint.color())
                .unwrap_or(Color::Reset),
        )
    } else {
        (
            "•",
            theme
                .primary
                .fg
                .or(theme.muted.fg)
                .map(|paint| paint.color())
                .unwrap_or(Color::Reset),
        )
    }
}

/// Compact elapsed time for the sidebar's narrow detail line: one unit, no padding. Resolution
/// coarsens as the number grows, because past a minute nobody reads the seconds.
/// A run's length, at two units so the figure keeps moving without spending the width a full
/// breakdown would.
///
/// The smaller unit is zero-padded so the token holds a stable width as it counts, which matters
/// now that it sits inline with the agent's name rather than alone on its own line. Precision
/// tapers with scale: seconds stop mattering once a run is measured in hours.
pub(crate) fn format_age(age: std::time::Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let secs = age.as_secs();
    if secs < MINUTE {
        format!("{secs}s")
    } else if secs < HOUR {
        format!("{}m{:02}s", secs / MINUTE, secs % MINUTE)
    } else if secs < DAY {
        format!("{}h{:02}m", secs / HOUR, (secs % HOUR) / MINUTE)
    } else {
        format!("{}d{:02}h", secs / DAY, (secs % DAY) / HOUR)
    }
}

/// Whether the glyph column already says this. The four known statuses each have their own glyph,
/// so spelling them out again in the detail line only spends width; anything else falls through to
/// a bare `•` in [`status_glyph`], where the word is the only signal there is.
fn status_is_canonical(status: &str) -> bool {
    let status = normalized_status(status);
    [
        pane_status::WORKING,
        pane_status::BLOCKED,
        pane_status::DONE,
        pane_status::IDLE,
        "error",
    ]
    .iter()
    .any(|known| status.eq_ignore_ascii_case(known))
}

/// The elapsed time a row shows, and whether it is still advancing.
///
/// A running state reports how long it has held. A finished one reports how long the run *took* and
/// then stops: the attention pulse already says the finish is recent, so a number climbing after
/// the work ended would measure nothing anyone asked about. An idle agent reports nothing — how
/// long a state that prompts no action has lasted is decoration, not a signal, and it would be the
/// only figure here that measures the reader rather than the agent.
fn row_duration(row: &AgentRow) -> Option<(String, bool)> {
    let status = row.status.as_deref().unwrap_or(pane_status::IDLE);
    elapsed(status, row.finished_unseen, row.age, row.run)
        .map(|(elapsed, advancing)| (format_age(elapsed), advancing))
}

/// The same rule as [`row_duration`], as a value, for every surface that shows an agent's time:
/// the Activity and Agents tabs, and the Agents picker. Returns the time and whether it advances.
pub(crate) fn elapsed(
    status: &str,
    finished_unseen: bool,
    age: Option<std::time::Duration>,
    run: Option<std::time::Duration>,
) -> Option<(std::time::Duration, bool)> {
    let label = row_status_label(status, finished_unseen);
    let label = normalized_status(&label);
    if label.eq_ignore_ascii_case(pane_status::IDLE) {
        return None;
    }
    if label.eq_ignore_ascii_case(pane_status::DONE) || label.eq_ignore_ascii_case("error") {
        // Absent when this client never saw the run start — better nothing than a wrong number.
        return run.map(|run| (run, false));
    }
    age.map(|age| (age, true))
}

/// Every *advancing* duration the Agents tab is currently showing, joined. This is what the
/// once-a-second tick compares against its previous value, so a minute of `12m` costs one
/// comparison per second rather than sixty repaints — and a screen of finished runs, whose numbers
/// are frozen, stops the tick entirely.
pub(super) fn duration_digest(state: &State) -> Option<String> {
    let digest = agent_rows(state)
        .iter()
        .filter_map(row_duration)
        .filter_map(|(text, advancing)| advancing.then_some(text))
        .collect::<Vec<_>>()
        .join(" ");
    (!digest.is_empty()).then_some(digest)
}

/// Character budget for the activity text. The detail line is the narrowest thing in the sidebar,
/// so the width actually available to it — the configured sidebar width less the row chrome and
/// whatever the duration column took — beats a fixed guess that overflows a narrow sidebar and
/// wastes a wide one.
pub(super) fn activity_budget(width: u16) -> usize {
    // Gutter, glyph, their gaps, the divider, and the scrollbar column.
    let chrome = 5;
    (usize::from(width)).saturating_sub(chrome).max(8)
}

/// The Activity tab: a heading per checkout with its agents beneath, and a cluster wherever one
/// agent runs more than once there, holding its runs unless folded. With `worktrees = "repository"`
/// a repository's checkouts share one heading instead, each a branch line with its agents beneath.
pub(super) fn agents_rows(
    state: &State,
    config: &crate::config::SidebarActivityConfig,
) -> Vec<SidebarRow> {
    let groups = agent_groups(state);
    // A lone fallback group renders flat, exactly as before grouping existed; a known project is
    // always headed, even alone — which project the agents operate in is the point of the tab.
    let show_headers = groups.len() > 1 || groups.first().is_some_and(|g| g.project.is_some());
    let mut rows = Vec::new();
    if !show_headers {
        for group in &groups {
            group_rows(state, group, 0, &mut rows);
        }
        return rows;
    }
    match config.worktrees {
        crate::config::ActivityWorktrees::Separate => separate_sections(state, &groups, &mut rows),
        crate::config::ActivityWorktrees::Repository => {
            repository_sections(state, &groups, &mut rows);
        }
    }
    rows
}

/// One section per checkout, headed by its repository and branch.
fn separate_sections(state: &State, groups: &[AgentGroup], rows: &mut Vec<SidebarRow>) {
    let labels: Vec<Option<String>> = groups
        .iter()
        .map(|group| group.project.as_deref().map(str::to_lowercase))
        .collect();
    for group in groups {
        let project = group.project.as_deref();
        // Several checkouts of one repository share its name, so only the branch tells their
        // headings apart: it then keeps its width, and the name gives way first.
        let shared = project.is_some_and(|project| {
            labels
                .iter()
                .filter(|label| label.as_deref() == Some(project.to_lowercase().as_str()))
                .count()
                > 1
        });
        rows.push(
            SidebarRow::header(group_header(
                state,
                project,
                group.branch.as_deref(),
                shared,
            ))
            .shape(Shape::Heading)
            .apart()
            .keyed(format!("project:{}", group.key.as_deref().unwrap_or(""))),
        );
        group_rows(state, group, 1, rows);
    }
}

/// One section per repository: its name, then each checkout's branch on a line of its own with
/// the checkout's agents beneath. The repository is named once however many worktrees it has, at
/// the cost of a level of nesting. A repository with a single checkout, and a directory outside
/// any repository, keep a section of their own as they would laid out separately.
fn repository_sections(state: &State, groups: &[AgentGroup], rows: &mut Vec<SidebarRow>) {
    let mut placed: Vec<&str> = Vec::new();
    for group in groups {
        let Some(repository) = group.repository.as_deref() else {
            rows.push(
                SidebarRow::header(group_header(state, None, None, false))
                    .shape(Shape::Heading)
                    .apart()
                    .keyed("project:"),
            );
            group_rows(state, group, 1, rows);
            continue;
        };
        if placed.contains(&repository) {
            continue;
        }
        placed.push(repository);
        let checkouts: Vec<&AgentGroup> = groups
            .iter()
            .filter(|other| other.repository.as_deref() == Some(repository))
            .collect();
        // A repository with one checkout has nothing to factor out: it reads as it would laid out
        // separately, its branch beside its name, without spending a level on a single line.
        if let [checkout] = checkouts.as_slice() {
            rows.push(
                SidebarRow::header(group_header(
                    state,
                    checkout.project.as_deref(),
                    checkout.branch.as_deref(),
                    false,
                ))
                .shape(Shape::Heading)
                .apart()
                .keyed(format!("project:{}", checkout.key.as_deref().unwrap_or(""))),
            );
            group_rows(state, checkout, 1, rows);
            continue;
        }
        let label = group.project.as_deref().unwrap_or_default();
        rows.push(
            SidebarRow::header(row::header(state, label, false))
                .shape(Shape::Heading)
                .apart()
                .keyed(format!("repository:{repository}")),
        );
        if checkouts.iter().all(|checkout| checkout.branch.is_none()) {
            for checkout in checkouts {
                group_rows(state, checkout, 1, rows);
            }
            continue;
        }
        for checkout in checkouts {
            let branch = checkout.branch.clone().unwrap_or_else(|| {
                checkout
                    .key
                    .as_deref()
                    .and_then(|key| path_segments(key).last().map(|name| name.to_string()))
                    .unwrap_or_default()
            });
            // A long branch keeps its end, where the line of work is named, as it does on a
            // separate heading. The budget is the tab less the gutter, the tree guide, and the
            // divider and scrollbar columns.
            let width = state.sidebar_tab_width(&crate::config::SidebarTabId::new("activity"));
            let branch = row::truncate_start(&branch, usize::from(width).saturating_sub(7).max(8));
            // An item rather than a header, so it keeps the row gutter and its tree guide lines up
            // with the agents beneath it.
            let line = Row::new(branch).title_style(super::super::fg_only(&state.theme.muted));
            rows.push(
                SidebarRow::item(line, RowTarget::Inert)
                    .shape(Shape::Heading)
                    .at(1)
                    .keyed(format!(
                        "checkout:{}",
                        checkout.key.as_deref().unwrap_or("")
                    )),
            );
            group_rows(state, checkout, 2, rows);
        }
    }
}

/// One checkout's agents and activities at `depth`: a row each, and a cluster wherever one agent
/// runs more than once there.
fn group_rows(state: &State, group: &AgentGroup, depth: usize, rows: &mut Vec<SidebarRow>) {
    let known_place = group.key.is_some();
    let mut placed: Vec<ClusterKey> = Vec::new();
    for row in &group.rows {
        let key = cluster_key(row, known_place);
        let members: Vec<&AgentRow> = group
            .rows
            .iter()
            .filter(|other| key.is_some() && cluster_key(other, known_place) == key)
            .collect();
        let Some(key) = key.filter(|_| members.len() > 1) else {
            rows.push(agent_line(state, row, None).at(depth));
            continue;
        };
        // The cluster takes the place of its most urgent run; the rest follow it.
        if placed.contains(&key) {
            continue;
        }
        let expansion = cluster_expansion_key(group.key.as_deref(), &key);
        placed.push(key);
        let expanded = state
            .sidebar
            .tree_rows_expanded
            .get(&expansion)
            .copied()
            .unwrap_or(true);
        let spans_workspaces = members
            .iter()
            .any(|member| member.workspace_index != row.workspace_index);
        rows.push(cluster_row(state, &members, expansion, expanded, spans_workspaces).at(depth));
        if expanded {
            let labels = member_labels(&members);
            rows.extend(members.iter().zip(labels).map(|(member, label)| {
                let child = ClusterChild {
                    label,
                    show_workspace: spans_workspaces,
                };
                agent_line(state, member, Some(child)).at(depth + 1)
            }));
        }
    }
}

/// How a run reads beneath its cluster, which already names the agent.
struct ClusterChild {
    label: String,
    /// The cluster spans workspaces, so each run says which one it is on.
    show_workspace: bool,
}

/// What each run in a cluster is called: its task, else what its terminal says, else the agent's
/// name - numbered only where two runs would otherwise read the same. Numbers follow where the runs
/// sit - their pane, then their place among its agents - not how urgent they are, so a run keeps
/// its number while the list reorders around it, and the Agents tab numbers it the same.
fn member_labels(members: &[&AgentRow]) -> Vec<String> {
    let base: Vec<String> = members
        .iter()
        .map(|member| {
            let agent = member
                .identity
                .as_ref()
                .map_or(member.title.as_str(), |identity| identity.label.as_str());
            member
                .task
                .as_deref()
                .and_then(|task| task_title(task, agent))
                .or_else(|| member.screen_title.clone())
                .unwrap_or_else(|| agent.to_string())
        })
        .collect();
    base.iter()
        .enumerate()
        .map(|(index, label)| {
            let mut twins: Vec<usize> = (0..members.len())
                .filter(|other| base[*other] == *label)
                .collect();
            if twins.len() < 2 {
                return label.clone();
            }
            twins.sort_by_key(|other| (members[*other].pane_id, members[*other].order));
            let number = twins.iter().position(|other| *other == index).unwrap_or(0) + 1;
            format!("{label} #{number}")
        })
        .collect()
}

/// An agent's placed row, keyed by its pane and published slot.
fn agent_line(state: &State, row: &AgentRow, child: Option<ClusterChild>) -> SidebarRow {
    let key = match &row.slot {
        Some(slot) => format!("agent:{}:{:?}", row.pane_id, slot.key),
        None => format!("agent:{}:{}", row.pane_id, row.order),
    };
    SidebarRow::item(agent_row(state, row, child), agent_target(row)).keyed(key)
}

/// Whether a row is in the "finished a run, not looked at yet" state: quiescent (neither working
/// nor blocked, which keep their own louder presentation) with an unseen finish.
fn is_finished_quiet(status: &str, finished_unseen: bool) -> bool {
    finished_unseen
        && crate::session::protocol::ActivityState::from_status(status).is_successful_terminal()
}

/// The status word a row displays. A finished-unseen agent reads "done" regardless of the raw
/// status the agent last reported, which is usually "idle" — idle describes the pane, done
/// describes the run that just ended.
pub(crate) fn row_status_label(status: &str, finished_unseen: bool) -> String {
    let value = if is_finished_quiet(status, finished_unseen) {
        pane_status::DONE
    } else {
        normalized_status(status)
    };
    // A well-known state is chrome: it labels the row rather than continuing a sentence, so it is
    // capitalized like the headings and badges around it. A custom status keeps the publisher's
    // own spelling, which may be deliberate and is not ours to normalize.
    if !status_is_canonical(value) {
        return value.to_string();
    }
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The glyph and color a row shows: the plain status glyph, except a finished-unseen agent shows a
/// filled success-colored dot. `bool` is whether the working spinner should animate.
pub(crate) fn row_glyph(
    status: &str,
    finished_unseen: bool,
    theme: &Theme,
) -> (String, Color, bool) {
    if is_finished_quiet(status, finished_unseen) {
        return ("●".to_string(), theme.status.success, false);
    }
    let working = status.trim().eq_ignore_ascii_case(pane_status::WORKING);
    let (glyph, color) = status_glyph(status, theme);
    (glyph.to_string(), color, working)
}

/// One-line project header: the project label at the same column every other tab's headers use,
/// with the branch it has checked out pinned to the right edge.
///
/// The branch earns that space because the group is a *directory*: a linked worktree is its own
/// group, labelled with the repository it belongs to, so two checkouts of `api` both read `api` and
/// only the branch says which line of work each holds. Knowing which branch an agent is committing
/// to is the difference between reading its output and trusting it. Nothing else about the repository is here — how far
/// the branch has diverged from its upstream cannot be known without a fetch, and a stale number
/// presented as current is worse than no number.
///
/// The header deliberately carries no aggregated status glyph. Groups never collapse, so every row
/// it would summarize is already on screen directly beneath it — and the glyph plus the nesting it
/// forced cost four cells on every row of the narrowest surface in the app.
///
/// Which side keeps its width depends on what tells the group apart. A repository with one checkout
/// on screen is named by its name, and the branch takes whatever width is left, down to nothing.
/// Several checkouts of one repository all carry its name, so there the branch is the identity: it
/// keeps its width, shortened from the start if it must be, and the name truncates first, down to a
/// stub. A detached checkout's branch is its short commit.
fn group_header(
    state: &State,
    project: Option<&str>,
    branch: Option<&str>,
    shared_name: bool,
) -> Element {
    let Some(label) = project else {
        return row::header(state, "Elsewhere", true);
    };
    let priority = if shared_name {
        row::Priority::Distinguishing
    } else {
        row::Priority::Label
    };
    match branch {
        Some(branch) => row::header_with_note(state, label, branch.trim(), priority),
        None => row::header(state, label, false),
    }
}

/// How much of a row's in-project path may sit in the badge, before width forces it shorter.
///
/// The badge shares the name line with the agent's own name and its elapsed run, both of which have
/// to stay readable; a deeper path keeps its tail, the part that says which component the agent is
/// actually in.
const SUBPATH_MAX_CHARS: usize = 12;

/// The subpath's share of the name line, once the name, the elapsed run, and the workspace number
/// have taken theirs.
///
/// The workspace number is the one thing in the badge that must survive: it is what the keybindings
/// address, and unlike a path it cannot be guessed from the row above. So the subpath yields to it
/// rather than the pair being clipped together from the right, which is what dropped the number.
fn subpath_budget(width: u16, name: &str, duration: Option<&str>, workspace: &str) -> usize {
    // Gutter, glyph, their gaps, the divider, the scrollbar column, the gap holding the badge off
    // the name, and the ` · ` joining the subpath to the workspace number.
    let chrome = 13;
    let taken = name.chars().count()
        + workspace.chars().count()
        + duration.map_or(0, |text| text.chars().count() + 1);
    usize::from(width)
        .saturating_sub(chrome)
        .saturating_sub(taken)
        .min(SUBPATH_MAX_CHARS)
}

/// A status glyph: the spinner while working, else the status's mark, in its color.
fn status_icon(glyph: String, color: Color, spinner: bool) -> Element {
    if spinner {
        Spinner::new()
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into()
    } else {
        Text::new(glyph)
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into()
    }
}

/// Whether a row is the one its agent shows: its pane is focused and, for a published row, the
/// program has it on screen.
fn row_is_active(state: &State, row: &AgentRow) -> bool {
    state.current().focused_pane == Some(row.pane_id)
        && row.slot.as_ref().is_none_or(|slot| slot.active)
}

/// A cluster's one-line row: the agent, the most urgent of its runs' glyphs, and its workspace when
/// they share one. Folded, it also counts its runs by state, and its arrow takes the most urgent
/// run's color.
///
/// Selecting it goes to that most urgent run - its pane and, for a published row, the row itself -
/// so the glyph and the destination always agree. The arrow only folds. Folded, it is the active
/// row whenever one of its hidden runs is; open, the run says so itself.
fn cluster_row(
    state: &State,
    members: &[&AgentRow],
    key: TreeExpansionKey,
    expanded: bool,
    spans_workspaces: bool,
) -> SidebarRow {
    let theme = &state.theme;
    let status = |row: &AgentRow| {
        row.status
            .as_deref()
            .unwrap_or(pane_status::IDLE)
            .to_string()
    };
    // Members arrive in the Activity order, most urgent first.
    let lead = members[0];
    let label = lead
        .identity
        .as_ref()
        .map_or(lead.title.as_str(), |identity| identity.label.as_str());
    let (glyph, color, spinner) = row_glyph(&status(lead), lead.finished_unseen, theme);
    let content = Row::new(label)
        .active(!expanded && members.iter().any(|member| row_is_active(state, member)))
        .title_style(super::super::fg_only(&theme.primary))
        .glyph(status_icon(glyph, color, spinner));
    let workspace =
        (!spans_workspaces).then(|| super::workspace_badge(state, lead.workspace_index));
    let muted = super::super::fg_only(&theme.muted).dim();
    let placed = |content: Row| {
        let row = SidebarRow::item(content, agent_target(lead))
            .keyed(key.row.clone())
            .collapsible(key.clone(), expanded);
        if expanded {
            row
        } else {
            row.arrow_style(Style::new().fg(color))
        }
    };
    if expanded {
        return placed(match workspace {
            Some(workspace) => content.badge_text(workspace, muted),
            None => content,
        });
    }
    // Folded, its runs are out of sight: count them by state, most urgent first, leaving idle ones
    // out as the remote session summaries do.
    let mut tally: Vec<(u8, String, Color, usize)> = Vec::new();
    for row in members {
        let value = status(row);
        if normalized_status(&value) == pane_status::IDLE && !row.finished_unseen {
            continue;
        }
        let rank = status_rank(Some(&value), row.finished_unseen);
        let (glyph, color, _) = row_glyph(&value, row.finished_unseen, theme);
        match tally.iter_mut().find(|(_, known, _, _)| *known == glyph) {
            Some((_, _, _, count)) => *count += 1,
            None => tally.push((rank, glyph, color, 1)),
        }
    }
    tally.sort_by_key(|(rank, ..)| *rank);
    let mut parts: Vec<(String, Style)> = tally
        .into_iter()
        .map(|(_, glyph, color, count)| (format!("{glyph}{count}"), Style::new().fg(color)))
        .collect();
    parts.extend(workspace.map(|workspace| (workspace, muted)));
    placed(content.badge_parts(parts))
}

/// A two-line agent row: status glyph, agent name and workspace badge, then the detail line.
///
/// Beneath a cluster, which already names the agent, a run is titled by what it works on, with its
/// state and the reason for it on the detail line. It carries no badge unless the cluster spans
/// workspaces, and then only where it sits.
fn agent_row(state: &State, row: &AgentRow, child: Option<ClusterChild>) -> Row {
    let status = row.status.as_deref().unwrap_or(pane_status::IDLE);
    let (glyph, color, spinner) = row_glyph(status, row.finished_unseen, &state.theme);
    let duration = row_duration(row).map(|(text, _)| text);
    let muted = super::super::fg_only(&state.theme.muted).dim();
    if let Some(child) = child {
        let mut content = Row::new(child.label)
            .active(row_is_active(state, row))
            .glyph(status_icon(glyph, color, spinner))
            .title_style(super::super::fg_only(&state.theme.primary))
            .detail(
                row_status_label(status, row.finished_unseen),
                Style::new().fg(color),
            );
        if let Some(reason) = row.reason.as_deref() {
            let budget = activity_budget(
                state.sidebar_tab_width(&crate::config::SidebarTabId::new("activity")),
            );
            content = content.detail(row::truncate(reason, budget), muted);
        }
        let badge = crate::view::vocabulary::fields(
            row.subpath
                .iter()
                .map(|subpath| row::truncate_start(subpath, SUBPATH_MAX_CHARS))
                .chain(
                    child
                        .show_workspace
                        .then(|| super::workspace_badge(state, row.workspace_index)),
                ),
        );
        if !badge.is_empty() {
            content = content.badge_text(badge, muted);
        }
        if let Some(duration) = duration {
            content = content.meta(duration, Style::new().fg(color));
        }
        return content;
    }
    let mut content = Row::new(row.title.clone())
        // For a published slot, being on screen in its own program decides this: a focused pane
        // shows exactly one of its slots, so marking them all active would say nothing.
        .active(row_is_active(state, row))
        .glyph(status_icon(glyph, color, spinner))
        .title_style(super::super::fg_only(&state.theme.primary))
        // Which workspace the agent lives on. Groups are projects, and a project's agents can be
        // spread across workspaces, so this is the cross-reference to the Panes tab — and the hint
        // that two rows cannot be watched side by side.
        //
        // Where the agent sits below its project root leads it: in a monorepo, `services/api` is
        // the only thing separating two rows that otherwise read identically, and the group header
        // can no longer say it now that it names the project rather than the directory.
        .badge_text(
            {
                let workspace = super::workspace_badge(state, row.workspace_index);
                match row.subpath.as_deref() {
                    Some(subpath) => {
                        let budget = subpath_budget(
                            state.sidebar_tab_width(&crate::config::SidebarTabId::new("activity")),
                            &row.title,
                            duration.as_deref(),
                            &workspace,
                        );
                        // Too little room to say anything useful about the path: keep the number
                        // alone rather than an ellipsis standing in for it.
                        if budget < 4 {
                            workspace
                        } else {
                            format!("{} · {workspace}", row::truncate_start(subpath, budget))
                        }
                    }
                    None => workspace,
                }
            },
            super::super::fg_only(&state.theme.muted).dim(),
        );

    // The elapsed run rides beside the name, in the gap the badge left empty. It qualifies the
    // agent, not the activity, and putting it here hands the whole detail line to the activity -
    // which is the part that was truncating.
    if let Some(duration) = duration.as_deref() {
        content = content.meta(duration.to_string(), Style::new().fg(color));
    }
    // Detail line: what the agent is doing. A canonical status yields to a real activity, since its
    // glyph already carries it; a custom one like "compacting" keeps its word either way, having
    // only a `•` to lean on.
    let label = row_status_label(status, row.finished_unseen);
    if !status_is_canonical(&label) || row.activity.is_none() {
        content = content.detail(label, Style::new().fg(color));
    }
    if let Some(activity) = row.activity.as_deref() {
        let budget =
            activity_budget(state.sidebar_tab_width(&crate::config::SidebarTabId::new("activity")));
        content = content.detail(
            row::truncate(activity, budget),
            super::super::fg_only(&state.theme.muted).dim(),
        );
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{AgentIdentity, DetectedAgent, DetectedAgentState, PaneStatus};
    use crate::state::Pane;

    fn pane(id: PaneId, value: Option<&str>, _exiting: bool) -> Pane {
        pane_in(id, value, false, None)
    }

    fn pane_in(id: PaneId, value: Option<&str>, _exiting: bool, cwd: Option<&str>) -> Pane {
        let mut pane = Pane::new(
            id,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 20.0,
                h: 10.0,
            },
        );
        pane.terminal.detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("claude", "Claude Code").into(),
            state: DetectedAgentState::Idle,
        });
        pane.terminal.reported_status = value.map(|value| PaneStatus {
            value: value.to_string(),
            reason: None,
            set_at: 1,
        });
        sync_single_ref(&mut pane);
        pane.terminal.cwd = cwd.map(str::to_string);
        pane
    }

    fn sync_single_ref(pane: &mut Pane) {
        pane.agents = vec![
            crate::session::protocol::AgentRef {
                pane: crate::session::protocol::PaneRef {
                    session_instance: crate::session::protocol::SessionInstanceId::for_test(
                        "server",
                    ),
                    pane_id: pane.id,
                    generation: pane.pty_generation,
                },
                slot: None,
                incarnation: 1,
            }
            .into(),
        ];
    }

    /// Give each of `pane`'s agents one finished run this client has not looked at.
    fn finish_unseen(state: &mut State, pane: &mut Pane) {
        state
            .agent_attention
            .observe_records(&pane.agents, crate::state::FirstSighting::Baseline);
        for record in &mut pane.agents {
            record.completions += 1;
        }
    }

    fn sync_published_refs(pane: &mut Pane) {
        pane.agents = pane
            .terminal
            .published_rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                crate::session::protocol::AgentRef {
                    pane: crate::session::protocol::PaneRef {
                        session_instance: crate::session::protocol::SessionInstanceId::for_test(
                            "server",
                        ),
                        pane_id: pane.id,
                        generation: pane.pty_generation,
                    },
                    slot: Some(row.key()),
                    incarnation: index as u64 + 2,
                }
                .into()
            })
            .collect();
    }

    /// A pane the session server resolved a Git project for: `cwd` may sit at `root` or below it.
    fn pane_in_project(
        id: PaneId,
        value: Option<&str>,
        cwd: &str,
        root: &str,
        branch: Option<&str>,
    ) -> Pane {
        let mut pane = pane_in(id, value, false, Some(cwd));
        pane.terminal.project_root = Some(root.to_string());
        pane.terminal.git_branch = branch.map(str::to_string);
        pane
    }

    /// A generic PR publisher beside a detected Codex: Activity lists Codex and every PR row, the
    /// Agents view lists only Codex, and no PR row is named after Codex.
    #[test]
    fn activity_render_and_activation_share_state_order() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let statuses = ["working", "idle", "done", "error", "blocked"];
        state.current_mut().workspaces[0].panes = statuses
            .iter()
            .enumerate()
            .map(|(i, status)| {
                let mut pane = pane_in(i as PaneId + 1, Some(status), false, Some("/repo"));
                pane.terminal.detected_agent = None;
                pane.agents.clear();
                pane.terminal.finished_unseen = *status == "error";
                pane
            })
            .collect();
        let rendered = agent_groups(&state)
            .into_iter()
            .flat_map(|group| group.rows)
            .map(|row| RowTarget::Pane(row.pane_id))
            .collect::<Vec<_>>();
        let projected = state
            .sidebar_rows(&crate::config::SidebarTab::activity())
            .into_iter()
            .map(|item| item.target)
            .filter(|target| *target != RowTarget::Inert)
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                RowTarget::Pane(5),
                RowTarget::Pane(4),
                RowTarget::Pane(1),
                RowTarget::Pane(3),
                RowTarget::Pane(2)
            ]
        );
        assert_eq!(projected, rendered);
        assert_eq!(
            status_rank(Some("compacting"), true),
            status_rank(Some("working"), false)
        );
    }

    #[test]
    fn published_activity_errors_bank_time_without_success_notices() {
        for previous in ["working", "compacting", "blocked"] {
            for current in ["error", "done", "idle"] {
                let mut terminal = crate::pane::TerminalPane::new(100);
                let mut task = row("build", previous, false);
                task.kind = crate::session::protocol::PublisherKind::Activity;
                task.work_started_at = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        .saturating_sub(12),
                );
                assert!(terminal.apply_rows(vec![task.clone()], None).is_empty());
                let key = task.key();
                terminal
                    .published_row_ui
                    .get_mut(&key)
                    .unwrap()
                    .finished_unseen = true;
                task.status = current.into();
                let finished = terminal.apply_rows(vec![task.clone()], None);
                assert_eq!(
                    finished.is_empty(),
                    current == "error",
                    "{previous} -> {current}"
                );
                let ui = &terminal.published_row_ui[&key];
                assert_eq!(ui.finished_unseen, current != "error");
                assert!(ui.last_run.is_some());
                assert!(terminal.apply_rows(vec![task], None).is_empty());
            }
        }
    }

    #[test]
    fn activity_error_rank_glyph_and_duration_are_terminal() {
        let theme = Theme::default();
        assert_eq!(status_glyph("blocked", &theme), ("▲", theme.status.warning));
        assert_eq!(status_glyph("error", &theme), ("×", theme.status.error));
        assert_eq!(row_glyph("error", true, &theme).0, "×");
        assert_eq!(status_rank(Some("error"), true), 1);
        for (left, right) in [
            ("blocked", "error"),
            ("error", "working"),
            ("working", "done"),
            ("done", "idle"),
        ] {
            assert!(status_rank(Some(left), false) < status_rank(Some(right), false));
        }
        assert_eq!(
            status_rank(Some("compacting"), false),
            status_rank(Some("working"), false)
        );
    }

    #[test]
    fn osc_activation_order_uses_its_own_attention_latch() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut osc = pane_in(1, None, false, Some("/repo"));
        osc.terminal.detected_agent = None;
        osc.agents.clear();
        osc.terminal.finished_unseen = true;
        let mut screen = TerminalScreen::new(24, 80, 100);
        screen.set_program_status_enabled(true);
        screen.process_bytes(b"\x1b]7501;state=idle\x07");
        osc.terminal.program_status = screen.program_status_snapshot();
        let mut done = pane_in(2, Some("done"), false, Some("/repo"));
        done.terminal.detected_agent = None;
        done.agents.clear();
        state.current_mut().workspaces[0].panes = vec![osc, done];
        let rendered = agent_groups(&state)
            .into_iter()
            .flat_map(|group| group.rows)
            .map(|row| RowTarget::Pane(row.pane_id))
            .collect::<Vec<_>>();
        let projected = state
            .sidebar_rows(&crate::config::SidebarTab::activity())
            .into_iter()
            .map(|item| item.target)
            .filter(|target| *target != RowTarget::Inert)
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec![RowTarget::Pane(2), RowTarget::Pane(1)]);
        assert_eq!(projected, rendered);
    }

    #[test]
    fn osc_activity_coexists_with_agent_and_generic_status() {
        let mut pane = pane(1, Some("working"), false);
        let before = pane.agent_runtimes();
        let mut screen = TerminalScreen::new(24, 80, 100);
        screen.set_program_status_enabled(true);
        screen.process_bytes(b"\x1b]7501;state=working:app=claude-code\x07\x1b]7501;state=error:id=build:msg=ZmFpbGVk:progress=20\x07");
        pane.terminal.program_status = screen.program_status_snapshot();
        let occupants = pane.occupants();
        assert_eq!(occupants.len(), 2);
        assert!(!occupants[1].is_agent());
        assert_eq!(
            occupants[1].state,
            crate::session::protocol::ActivityState::Error
        );
        assert_eq!(pane.agent_runtimes(), before);
        pane.terminal.detected_agent = None;
        assert_eq!(pane.occupants().len(), 2);
        assert!(pane.agent_runtimes().is_empty());
        let activity = crate::session::protocol::PublishedRow {
            kind: crate::session::protocol::PublisherKind::Activity,
            publisher: "client-a:jobs".into(),
            ..row("build", "error", false)
        };
        let agent = crate::session::protocol::PublishedRow {
            publisher: "client-b:agents".into(),
            ..row("conversation", "working", true)
        };
        pane.terminal.published_rows = vec![activity, agent];
        let published = pane.terminal.published_rows.clone();
        let agents = pane.agent_runtimes();
        assert_eq!(pane.occupants().len(), 3);
        assert_eq!(
            pane.occupants()
                .last()
                .unwrap()
                .program_status
                .as_ref()
                .unwrap()
                .id
                .as_ref(),
            "build"
        );
        screen.process_bytes(b"\x1b]7501;state=clear\x07");
        pane.terminal.program_status = screen.program_status_snapshot();
        assert_eq!(pane.occupants().len(), 2);
        assert_eq!(pane.terminal.published_rows, published);
        assert_eq!(pane.agent_runtimes(), agents);
    }

    #[test]
    fn a_generic_publisher_beside_a_detected_agent_stays_out_of_the_agents_view() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().session_name = Some("dev".into());
        let mut codex = pane(1, None, false);
        codex.terminal.detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("codex", "Codex").into(),
            state: DetectedAgentState::Working,
        });
        codex.terminal.published_rows = ["pr-1", "pr-2"]
            .into_iter()
            .map(|id| crate::session::protocol::PublishedRow {
                publisher: "publish:prs".into(),
                kind: crate::session::protocol::PublisherKind::Activity,
                title: if id == "pr-1" {
                    "#1 fix login".into()
                } else {
                    String::new()
                },
                ..row(id, "idle", id == "pr-1")
            })
            .collect();
        codex.title = "pr watcher".into();
        state.current_mut().workspaces[0].panes = vec![codex];

        let rows = agent_rows(&state);
        let listed: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.title.as_str(),
                    row.identity.is_some(),
                    row.status.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("Codex", true, Some("working")),
                ("#1 fix login", false, Some("idle")),
                ("pr watcher", false, Some("idle")),
            ],
            "Codex keeps its own state and name; PR rows never take Codex's"
        );
        let picker: Vec<_> = crate::view::agents::global_agent_rows(&state)
            .into_iter()
            .map(|row| row.agent)
            .collect();
        assert_eq!(picker, ["Codex"]);
        // The Activity tab's item list agrees with the rows it renders.
        assert_eq!(
            state
                .sidebar_rows(&crate::config::SidebarTab::activity())
                .iter()
                .filter(|item| !matches!(item.target, RowTarget::Inert))
                .count(),
            3
        );
    }

    /// A pane with only a `rozi status` is an activity of its own, named after the pane.
    #[test]
    fn a_bare_status_is_an_activity_not_an_agent() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().session_name = Some("dev".into());
        let mut build = pane(1, Some("working"), false);
        build.terminal.detected_agent = None;
        build.agents.clear();
        build.title = "cargo build".into();
        state.current_mut().workspaces[0].panes = vec![build];

        let rows = agent_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "cargo build");
        assert!(rows[0].identity.is_none());
        assert!(crate::view::agents::global_agent_rows(&state).is_empty());
    }

    #[test]
    fn rows_sort_by_status_then_workspace_and_pane_order() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane(1, Some("idle"), false),
            pane(2, Some("Custom"), false),
            pane(3, Some("BLOCKED"), false),
        ];
        state.current_mut().workspaces[1].panes = vec![
            pane(4, Some(" working "), false),
            pane(5, Some("done"), false),
            pane(6, None, false),
            pane(7, Some("blocked"), true),
        ];
        assert_eq!(
            agent_rows(&state)
                .into_iter()
                .map(|row| row.pane_id)
                .collect::<Vec<_>>(),
            vec![3, 7, 2, 4, 5, 1, 6]
        );
    }

    #[test]
    fn integration_authority_drives_rendered_rows_and_sorting() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut integrated = pane(1, Some("idle"), false);
        let reference = integrated.agents[0].reference.clone();
        integrated.terminal.agent_integration =
            Some(Box::new(crate::session::protocol::AgentIntegrationReport {
                integration: "hook-a".into(),
                identity: AgentIdentity::new("claude", "Claude Code"),
                reference,
                state: crate::session::protocol::AgentState::Blocked,
                reason: Some("approval".into()),
                native_session: None,
                seq: 1,
                reported_at_unix_ms: 42,
            }));
        integrated.terminal.detected_agent = None;
        state.current_mut().workspaces[0].panes = vec![pane(2, Some("working"), false), integrated];

        let rows = agent_rows(&state);
        assert_eq!(
            rows.iter().map(|row| row.pane_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(rows[0].status.as_deref(), Some("blocked"));
        assert_eq!(rows[0].activity.as_deref(), Some("approval"));
        assert_eq!(
            state
                .sidebar_rows(&crate::config::SidebarTab::activity())
                .into_iter()
                .filter_map(|item| match item.target {
                    RowTarget::Pane(pane_id) => Some(pane_id),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            rows.iter().map(|row| row.pane_id).collect::<Vec<_>>()
        );
    }

    fn row(id: &str, status: &str, active: bool) -> crate::session::protocol::PublishedRow {
        crate::session::protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: id.to_string(),
            title: format!("tab {id}"),
            status: status.to_string(),
            reason: None,
            active,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        }
    }

    /// The path every agent that publishes nothing takes must be untouched by row expansion.
    #[test]
    fn a_pane_without_published_rows_still_produces_exactly_one_row() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![pane(1, Some("working"), false)];

        let rows = agent_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].slot, None);
        assert_eq!(rows[0].title, "Claude Code");
    }

    #[test]
    fn a_pane_with_published_rows_becomes_one_row_per_item_in_tab_order() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![
            row("a", "working", false),
            row("b", "working", true),
            row("c", "working", false),
        ];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        let rows = agent_rows(&state);
        assert_eq!(
            rows.iter()
                .map(|row| row.slot.as_ref().unwrap().key.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "one pane's rows keep the publisher's own order among themselves"
        );
        assert!(rows.iter().all(|row| row.pane_id == 1));
        // The name column stays the agent; the row's own title is its task, which is what tells
        // one pane's rows apart.
        assert_eq!(rows[1].title, "Claude Code");
        assert_eq!(rows[1].activity.as_deref(), Some("tab b"));
        assert_eq!(rows[1].task.as_deref(), Some("tab b"));
    }

    #[test]
    fn a_pane_without_detected_agent_renders_published_rows() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.detected_agent = None;
        publisher.terminal.published_rows = vec![
            crate::session::protocol::PublishedRow {
                publisher: "sessions".into(),
                kind: crate::session::protocol::PublisherKind::Agent,
                live_session: None,
                reported: false,
                id: "build".into(),
                title: "Cargo Watch".into(),
                status: "working".into(),
                reason: Some("compiling crates".into()),
                active: true,
                work_started_at: None,
                cwd: None,
                project: None,
                native_session: None,
            },
            crate::session::protocol::PublishedRow {
                publisher: "sessions".into(),
                kind: crate::session::protocol::PublisherKind::Agent,
                live_session: None,
                reported: false,
                id: "test".into(),
                title: String::new(),
                status: "blocked".into(),
                reason: Some("assertion failed".into()),
                active: false,
                work_started_at: None,
                cwd: None,
                project: None,
                native_session: None,
            },
        ];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        let rows = agent_rows(&state);
        assert_eq!(rows.len(), 2);
        // "blocked" sorts before "working"
        assert_eq!(rows[0].title, "shell");
        assert_eq!(rows[0].status.as_deref(), Some("blocked"));
        assert_eq!(rows[0].activity.as_deref(), Some("assertion failed"));

        assert_eq!(rows[1].title, "Cargo Watch");
        assert_eq!(rows[1].status.as_deref(), Some("working"));
        assert_eq!(rows[1].activity.as_deref(), Some("compiling crates"));
    }

    /// The name column reads `Claude Code` for every row in a pane, so the title is the only thing
    /// saying which one this is. A reason cannot be allowed to hide it: a session is often blocked
    /// on its first question before anything has titled it, and the row would then be stuck
    /// reading "answer required" for the rest of the run. The reason stays beside it.
    #[test]
    fn a_titled_row_shows_its_title_even_while_blocked() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![crate::session::protocol::PublishedRow {
            title: "audit the widget layer".into(),
            reason: Some("permission required".into()),
            ..row("a", "blocked", true)
        }];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        let rows = agent_rows(&state);
        assert_eq!(rows[0].title, "Claude Code");
        assert_eq!(rows[0].activity.as_deref(), Some("audit the widget layer"));
        assert_eq!(rows[0].reason.as_deref(), Some("permission required"));
    }

    /// Until a title exists the reason is all the row has to say.
    #[test]
    fn an_untitled_row_falls_back_to_its_reason() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![crate::session::protocol::PublishedRow {
            title: String::new(),
            reason: Some("answer required".into()),
            ..row("a", "blocked", true)
        }];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        assert_eq!(
            agent_rows(&state)[0].activity.as_deref(),
            Some("answer required")
        );
    }

    /// A row titled after the agent itself would repeat the name column beside it.
    #[test]
    fn a_row_titled_after_its_agent_shows_no_activity() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![crate::session::protocol::PublishedRow {
            title: "Claude Code".into(),
            ..row("a", "idle", true)
        }];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        assert_eq!(agent_rows(&state)[0].activity, None);
    }

    #[test]
    fn well_known_states_are_capitalized_and_custom_spelling_is_not() {
        for (status, expected) in [
            ("idle", "Idle"),
            ("working", "Working"),
            ("blocked", "Blocked"),
            ("done", "Done"),
        ] {
            assert_eq!(row_status_label(status, false), expected);
        }
        assert_eq!(
            row_status_label("compacting", false),
            "compacting",
            "a publisher's own word is not ours to normalize"
        );
        assert_eq!(
            row_status_label("idle", true),
            "Done",
            "a finished-unseen agent reads Done, capitalized like the rest"
        );
    }

    /// The point of the feature: a blocked tab has to reach the top of the list even though the
    /// pane it lives in is the same pane as its working siblings.
    #[test]
    fn published_rows_sort_by_status_across_their_own_pane() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![
            row("idle-tab", "idle", false),
            row("working-tab", "working", true),
            row("blocked-tab", "blocked", false),
        ];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        assert_eq!(
            agent_rows(&state)
                .iter()
                .map(|row| row.slot.as_ref().unwrap().key.id.as_str())
                .collect::<Vec<_>>(),
            vec!["blocked-tab", "working-tab", "idle-tab"]
        );
    }

    /// A background tab's finish is not acknowledged by looking at the pane, because looking at
    /// the pane only ever showed the tab the publisher had on screen.
    #[test]
    fn attending_a_pane_clears_only_the_row_on_screen() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows =
            vec![row("shown", "idle", true), row("hidden", "idle", false)];
        sync_published_refs(&mut publisher);
        finish_unseen(&mut state, &mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];
        state.window_focused = true;
        state.current_mut().focused_pane = Some(1);

        assert!(crate::ops::focus::acknowledge_pane_if_attended(
            &mut state, 1
        ));

        let pulses = agent_rows(&state)
            .into_iter()
            .map(|row| (row.slot.unwrap().key.id, row.finished_unseen))
            .collect::<Vec<_>>();
        // The unacknowledged tab also rises above the acknowledged one: it still reads "done"
        // while the attended slot has fallen back to plain idle.
        assert_eq!(
            pulses,
            vec![("hidden".to_string(), true), ("shown".to_string(), false)]
        );
    }

    /// Location belongs to the pane, so expanding it into several rows must not split its group.
    #[test]
    fn published_rows_inherit_their_panes_project_grouping() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane_in_project(1, None, "/work/api/src", "/work/api", Some("main"));
        publisher.terminal.published_rows =
            vec![row("a", "working", true), row("b", "idle", false)];
        sync_published_refs(&mut publisher);
        state.current_mut().workspaces[0].panes = vec![publisher];

        let groups = agent_groups(&state);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rows.len(), 2);
        assert_eq!(groups[0].branch.as_deref(), Some("main"));
    }

    /// A finished-unseen agent reports `idle` but displays "done", and must sort with `done` —
    /// above plain idle rows — so the row does not sink as it lights up.
    #[test]
    fn finished_unseen_rows_sort_as_done_not_idle() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut finished = pane(2, Some("idle"), false);
        finish_unseen(&mut state, &mut finished);
        state.current_mut().workspaces[0].panes = vec![pane(1, Some("idle"), false), finished];

        assert_eq!(
            agent_rows(&state)
                .into_iter()
                .map(|row| row.pane_id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(
            status_rank(Some("idle"), true),
            status_rank(Some("done"), false)
        );
        // Working and blocked keep their own louder presentation and outrank a finished run.
        assert!(status_rank(Some("working"), true) < status_rank(Some("idle"), true));
        assert!(status_rank(Some("blocked"), true) < status_rank(Some("idle"), true));
    }

    #[test]
    fn glyph_matching_is_trimmed_and_case_insensitive() {
        let theme = Theme::default();
        assert_eq!(status_glyph(" BLOCKED ", &theme).0, "▲");
        assert_eq!(status_glyph("Working", &theme).0, "⠋");
        assert_eq!(status_glyph("Waiting", &theme).0, "•");
    }

    #[test]
    fn finished_unseen_pulses_only_over_quiescent_statuses() {
        let theme = Theme::default();
        // Quiescent statuses gain the filled pulse and stop the spinner.
        assert_eq!(row_glyph("idle", true, &theme).0, "●");
        assert_eq!(row_glyph("done", true, &theme).0, "●");
        assert_eq!(row_glyph("idle", true, &theme).1, theme.status.success);
        assert!(!row_glyph("idle", true, &theme).2);
        // Blocked and working keep their own glyph regardless of an unseen finish.
        assert_eq!(row_glyph("blocked", true, &theme).0, "▲");
        assert!(row_glyph("working", true, &theme).2);
        // Without the flag the glyph is unchanged.
        assert_eq!(row_glyph("idle", false, &theme).0, "○");
    }

    #[test]
    fn rows_use_server_detection_and_exclude_non_agents() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        assert!(agent_rows(&state).is_empty());
        state.current_mut().workspaces[0].panes[0]
            .terminal
            .detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("opencode", "OpenCode").into(),
            state: DetectedAgentState::Working,
        });
        sync_single_ref(&mut state.current_mut().workspaces[0].panes[0]);
        let rows = agent_rows(&state);
        assert_eq!(rows[0].title, "OpenCode");
        assert_eq!(rows[0].status.as_deref(), Some("working"));
    }

    #[test]
    fn groups_sort_alphabetically_with_unknown_last_and_keep_row_status_order() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane_in(1, Some("idle"), false, Some("/home/x/zebra")),
            pane_in(2, Some("blocked"), false, Some("/home/x/api")),
            pane_in(3, Some("working"), false, None),
            pane_in(4, Some("working"), false, Some("/home/x/api")),
        ];

        let groups = agent_groups(&state);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.project.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("api"), Some("zebra"), None]
        );
        // Within a group, blocked outranks working regardless of pane order.
        assert_eq!(
            groups[0]
                .rows
                .iter()
                .map(|row| row.pane_id)
                .collect::<Vec<_>>(),
            vec![2, 4]
        );
        // A blocked agent in "api" must not pull its group above alphabetical order.
        assert_eq!(groups[1].rows[0].pane_id, 1);
        assert_eq!(groups[2].rows[0].pane_id, 3);
    }

    /// The point of grouping on the project root: an agent launched in `rozi/src` belongs to
    /// `rozi`, not to a project called `src`. What it lost — where in the project it sits — comes
    /// back on the row as the subpath.
    #[test]
    fn agents_below_the_project_root_join_the_project_not_a_directory() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane_in_project(
                1,
                Some("idle"),
                "/home/x/rozi",
                "/home/x/rozi",
                Some("master"),
            ),
            pane_in_project(
                2,
                Some("working"),
                "/home/x/rozi/src/view",
                "/home/x/rozi",
                Some("master"),
            ),
        ];

        let groups = agent_groups(&state);
        assert_eq!(groups.len(), 1, "one project, one group");
        assert_eq!(groups[0].project.as_deref(), Some("rozi"));
        assert_eq!(groups[0].branch.as_deref(), Some("master"));
        // Status order still decides the rows: the nested one is working, so it leads.
        assert_eq!(
            groups[0]
                .rows
                .iter()
                .map(|row| (row.pane_id, row.subpath.clone()))
                .collect::<Vec<_>>(),
            vec![(2, Some("src/view".to_string())), (1, None)]
        );
    }

    /// Two checkouts of one repository are two directories, so they stay two groups — and the
    /// branch is what tells them apart when the directory names do not.
    #[test]
    fn worktrees_stay_separate_groups_and_head_their_own_branch() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane_in_project(
                1,
                Some("idle"),
                "/home/x/api",
                "/home/x/api",
                Some("master"),
            ),
            pane_in_project(
                2,
                Some("idle"),
                "/home/x/api-2",
                "/home/x/api-2",
                Some("feat/pricing"),
            ),
        ];

        assert_eq!(
            agent_groups(&state)
                .iter()
                .map(|group| (group.project.clone(), group.branch.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some("api".into()), Some("master".into())),
                (Some("api-2".into()), Some("feat/pricing".into())),
            ]
        );
    }

    /// A linked worktree is named after its repository, not its directory: `.claude/worktrees/x`
    /// is `rozi` on another branch, not a project called `x`. Sharing that name is not ambiguity
    /// worth a parent segment — the branch already separates them — while an unrelated project
    /// that happens to share the name still gets one.
    #[test]
    fn linked_worktrees_take_their_repositorys_name() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut worktree = pane_in_project(
            2,
            Some("idle"),
            "/home/x/rozi/.claude/worktrees/fix-login",
            "/home/x/rozi/.claude/worktrees/fix-login",
            Some("worktree-fix-login"),
        );
        worktree.terminal.repository = Some("/home/x/rozi".into());
        state.current_mut().workspaces[0].panes = vec![
            worktree,
            pane_in_project(
                1,
                Some("idle"),
                "/home/x/rozi",
                "/home/x/rozi",
                Some("master"),
            ),
        ];
        assert_eq!(
            agent_groups(&state)
                .iter()
                .map(|group| (group.project.clone(), group.branch.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some("rozi".into()), Some("master".into())),
                (Some("rozi".into()), Some("worktree-fix-login".into())),
            ]
        );

        state.current_mut().workspaces[0]
            .panes
            .push(pane_in_project(
                3,
                Some("idle"),
                "/srv/rozi",
                "/srv/rozi",
                Some("master"),
            ));
        assert_eq!(
            agent_groups(&state)
                .iter()
                .filter_map(|group| group.project.clone())
                .collect::<Vec<_>>(),
            vec!["srv/rozi", "x/rozi", "x/rozi"]
        );
    }

    /// One terminal running several sessions, each in its own worktree: a row that names its own
    /// directory is grouped there, and one that names none stays with its pane.
    fn hook_report(
        native: &str,
        state: crate::session::protocol::AgentState,
    ) -> Box<crate::session::protocol::AgentIntegrationReport> {
        Box::new(crate::session::protocol::AgentIntegrationReport {
            integration: "hook".into(),
            identity: AgentIdentity::new("claude", "Claude Code"),
            reference: crate::session::protocol::AgentRef {
                pane: crate::session::protocol::PaneRef {
                    session_instance: crate::session::protocol::SessionInstanceId::for_test(
                        "server",
                    ),
                    pane_id: 1,
                    generation: 0,
                },
                slot: None,
                incarnation: 99,
            },
            state,
            reason: Some("Permission required".into()),
            native_session: Some(native.into()),
            seq: 1,
            reported_at_unix_ms: 1,
        })
    }

    /// The publisher still calls a conversation idle while its hooks report it blocked: every
    /// surface - status, activity, and sort order - must show the hooks' word, as automation does.
    #[test]
    fn a_hook_report_drives_how_its_row_is_presented_and_sorted() {
        use crate::session::protocol::AgentState;
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut host = pane(1, None, false);
        let mut quiet = row("quiet", "idle", false);
        quiet.native_session = Some("native-quiet".into());
        let mut busy = row("busy", "working", false);
        busy.native_session = Some("native-busy".into());
        let _ = host.terminal.apply_rows(
            vec![busy.clone(), quiet.clone()],
            Some(hook_report("native-quiet", AgentState::Blocked)),
        );
        sync_published_refs(&mut host);
        state.current_mut().workspaces[0].panes = vec![host];

        let rows = agent_rows(&state);
        let presented = rows
            .iter()
            .map(|row| {
                (
                    row.slot.as_ref().unwrap().key.id.clone(),
                    row.status.clone().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        // Blocked outranks working, so the hooks' state also decides the order.
        assert_eq!(
            presented,
            vec![
                ("quiet".to_string(), "blocked".to_string()),
                ("busy".to_string(), "working".to_string()),
            ]
        );
        let runtimes = state.current().workspaces[0].panes[0].agent_runtimes();
        let quiet_runtime = runtimes
            .iter()
            .find(|runtime| {
                runtime.reference.slot.as_ref().map(|slot| slot.id.as_str()) == Some("quiet")
            })
            .unwrap();
        assert_eq!(
            rows[0].status.as_deref(),
            Some(quiet_runtime.state.as_str())
        );
    }

    /// A run the hooks report finishing, on a row the publisher never called working, still banks
    /// its length. The finish itself is the server's count, so the client reports none of its own.
    #[test]
    fn a_hook_reported_finish_banks_the_rows_run() {
        use crate::session::protocol::AgentState;
        let mut terminal = crate::pane::TerminalPane::new(100);
        let mut quiet = row("quiet", "idle", false);
        quiet.native_session = Some("native-quiet".into());
        quiet.work_started_at = Some(1);
        assert!(
            terminal
                .apply_rows(
                    vec![quiet.clone()],
                    Some(hook_report("native-quiet", AgentState::Working))
                )
                .is_empty()
        );
        let finished = terminal.apply_rows(
            vec![quiet.clone()],
            Some(hook_report("native-quiet", AgentState::Done)),
        );
        let quiet_key = crate::session::protocol::RowKey::new("sessions", "quiet");
        assert!(finished.is_empty());
        assert!(terminal.published_row_ui[&quiet_key].last_run.is_some());
        assert!(!terminal.published_row_ui[&quiet_key].finished_unseen);
    }

    #[test]
    fn published_rows_with_their_own_cwd_group_where_they_work() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut host = pane_in_project(1, None, "/home/x/rozi", "/home/x/rozi", Some("master"));
        let mut elsewhere = row("fix", "working", false);
        elsewhere.title = "fix login redirect".into();
        elsewhere.cwd = Some("/home/x/rozi/.claude/worktrees/fix".into());
        elsewhere.project = Some(crate::session::protocol::RowProject {
            root: "/home/x/rozi/.claude/worktrees/fix".into(),
            repository: Some("/home/x/rozi".into()),
            branch: Some("worktree-fix".into()),
        });
        host.terminal.published_rows = vec![row("here", "idle", true), elsewhere];
        sync_published_refs(&mut host);
        state.current_mut().workspaces[0].panes = vec![host];

        let groups = agent_groups(&state);
        assert_eq!(
            groups
                .iter()
                .map(|group| (
                    group.project.clone(),
                    group.branch.clone(),
                    group
                        .rows
                        .iter()
                        .map(|row| row.slot.as_ref().unwrap().key.id.clone())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some("rozi".into()),
                    Some("master".into()),
                    vec!["here".to_string()]
                ),
                (
                    Some("rozi".into()),
                    Some("worktree-fix".into()),
                    vec!["fix".to_string()]
                ),
            ]
        );

        // Keyboard navigation walks the same groups in the same order the view draws.
        let targets = state
            .sidebar_rows(&crate::config::SidebarTab::activity())
            .into_iter()
            .filter_map(|item| match item.target {
                RowTarget::PublishedRow { row, .. } => Some(row.id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(targets, vec!["here", "fix"]);
    }

    /// A client can see a project before the server has read its branch, and a pane can be in no
    /// project at all. Neither may lose the group or invent a branch for it.
    #[test]
    fn a_group_without_a_branch_still_heads_its_project() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane_in_project(1, Some("idle"), "/home/x/api", "/home/x/api", None),
            pane_in_project(2, Some("idle"), "/home/x/api", "/home/x/api", Some("  ")),
            pane_in(3, Some("idle"), false, Some("/home/x/notes")),
        ];

        let groups = agent_groups(&state);
        assert_eq!(groups[0].project.as_deref(), Some("api"));
        assert_eq!(groups[0].branch, None, "blank is not a branch name");
        assert_eq!(groups[1].project.as_deref(), Some("notes"));
        assert_eq!(groups[1].branch, None);
        assert!(groups[1].rows[0].subpath.is_none());
    }

    #[test]
    fn duplicate_project_basenames_gain_a_parent_segment() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes = vec![
            pane_in(1, Some("idle"), false, Some("/home/x/work/api")),
            pane_in(2, Some("idle"), false, Some("/home/x/oss/api")),
            pane_in(3, Some("idle"), false, Some("/home/x/solo")),
        ];

        assert_eq!(
            agent_groups(&state)
                .iter()
                .map(|group| group.project.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("oss/api"), Some("solo"), Some("work/api")]
        );
    }

    #[test]
    fn project_labels_handle_roots_windows_shapes_and_remote_hosts() {
        assert_eq!(project_label("/home/x/repo", None, false), "repo");
        assert_eq!(project_label("/home/x/repo/", None, false), "repo");
        assert_eq!(project_label("/", None, false), "/");
        assert_eq!(project_label("C:\\Users\\x\\repo", None, false), "repo");
        assert_eq!(project_label("C:\\", None, false), "C:");
        assert_eq!(project_label("\\\\server\\share", None, false), "share");
        // A Unix directory whose name contains a backslash is one segment, not two.
        assert_eq!(project_label("/home/x/my\\dir", None, false), "my\\dir");
        assert_eq!(
            project_label("/srv/repo", Some("build.example"), false),
            "repo@build.example"
        );
        assert_eq!(project_label("/home/x/work/api", None, true), "work/api");
    }

    #[test]
    fn activity_prefers_a_reported_reason_over_the_terminal_title() {
        let mut pane = crate::pane::TerminalPane::new(100);
        pane.title = Some("✳ writing the parser".into());
        pane.reported_status = Some(PaneStatus {
            value: "working".into(),
            reason: Some("running tests".into()),
            set_at: 1,
        });
        assert_eq!(
            activity_text(&pane, "Claude Code").as_deref(),
            Some("running tests")
        );

        // No reason: the title carries it, minus the agent's own status glyph.
        pane.reported_status = None;
        assert_eq!(
            activity_text(&pane, "Claude Code").as_deref(),
            Some("writing the parser")
        );

        // A blank reason is not an answer; fall through to the title rather than showing nothing.
        pane.reported_status = Some(PaneStatus {
            value: "working".into(),
            reason: Some("   ".into()),
            set_at: 1,
        });
        assert_eq!(
            activity_text(&pane, "Claude Code").as_deref(),
            Some("writing the parser")
        );
    }

    #[test]
    fn activity_drops_a_title_that_says_nothing_the_row_does_not_already() {
        let mut pane = crate::pane::TerminalPane::new(100);
        pane.cwd = Some("/home/you/repo".into());

        // The shell's `$PWD` title, in both spellings, and the bare directory name.
        for echo in ["/home/you/repo", "~/repo", "repo"] {
            pane.title = Some(echo.to_string());
            assert_eq!(activity_text(&pane, "Claude Code"), None, "{echo}");
        }

        // The agent's own name is already the row's title.
        pane.title = Some("Claude Code".into());
        assert_eq!(activity_text(&pane, "Claude Code"), None);

        // Decoration alone leaves nothing behind.
        pane.title = Some("✳".into());
        assert_eq!(activity_text(&pane, "Claude Code"), None);

        // A real task survives, and ASCII punctuation in it is left alone.
        pane.title = Some("[2/7] ~/repo cleanup".into());
        assert_eq!(
            activity_text(&pane, "Claude Code").as_deref(),
            Some("[2/7] ~/repo cleanup")
        );
    }

    #[test]
    fn activity_strips_opencode_title_prefix_only_for_opencode() {
        let mut pane = crate::pane::TerminalPane::new(100);
        pane.title = Some("OC | TextArea · writing tests".into());

        assert_eq!(
            activity_text(&pane, "OpenCode").as_deref(),
            Some("TextArea · writing tests")
        );
        assert_eq!(
            activity_text(&pane, "Claude Code").as_deref(),
            Some("OC | TextArea · writing tests")
        );
    }

    #[test]
    fn durations_coarsen_and_idle_rows_show_none() {
        use std::time::Duration;
        assert_eq!(format_age(Duration::from_secs(0)), "0s");
        assert_eq!(format_age(Duration::from_secs(59)), "59s");
        // Two units from a minute up, so the figure keeps moving instead of sitting on `1m` for
        // the next fifty-nine seconds. The smaller one is padded to hold a stable width.
        assert_eq!(format_age(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_age(Duration::from_secs(3 * 60 + 5)), "3m05s");
        assert_eq!(format_age(Duration::from_secs(59 * 60 + 59)), "59m59s");
        assert_eq!(format_age(Duration::from_secs(60 * 60)), "1h00m");
        assert_eq!(format_age(Duration::from_secs(2 * 3600 + 12 * 60)), "2h12m");
        assert_eq!(format_age(Duration::from_secs(24 * 60 * 60 - 1)), "23h59m");
        assert_eq!(format_age(Duration::from_secs(24 * 60 * 60)), "1d00h");
        assert_eq!(format_age(Duration::from_secs(50 * 60 * 60)), "2d02h");

        let row = |status: &str, finished_unseen: bool| AgentRow {
            pane_id: 1,
            workspace_index: 0,
            pane_index: 0,
            title: "Claude Code".into(),
            status: Some(status.to_string()),
            activity: None,
            age: Some(Duration::from_secs(90)),
            run: Some(Duration::from_secs(12 * 60)),
            cwd: None,
            cwd_host: None,
            project_root: None,
            subpath: None,
            branch: None,
            repository: None,
            finished_unseen,
            slot: None,
            identity: Some(AgentIdentity::new("claude", "Claude Code")),
            task: None,
            reason: None,
            order: 0,
            screen_title: None,
        };
        // A live state reports how long it has held, and keeps advancing.
        assert_eq!(
            row_duration(&row("working", false)),
            Some(("1m30s".into(), true))
        );
        assert_eq!(
            row_duration(&row("compacting", false)),
            Some(("1m30s".into(), true))
        );
        // Idle times nothing: the row still gets its second line, but from the status word alone.
        assert_eq!(row_duration(&row("idle", false)), None);
        // A finished run reports what it cost — the banked 12m, not the 90s since it stopped — and
        // stops advancing, so the tick has nothing left to refresh.
        assert_eq!(
            row_duration(&row("idle", true)),
            Some(("12m00s".into(), false))
        );
        assert_eq!(
            row_duration(&row("error", false)),
            Some(("12m00s".into(), false))
        );
        assert_eq!(
            row_duration(&row("done", false)),
            Some(("12m00s".into(), false))
        );
    }

    /// The picker and the Agents tab take their time from [`elapsed`], the same rule the Activity
    /// tab shows, so an idle agent reads the same on every surface: no time at all.
    #[test]
    fn every_surface_shows_the_same_elapsed_time() {
        use std::time::Duration;
        let age = Some(Duration::from_secs(90));
        let run = Duration::from_secs(12 * 60);
        assert_eq!(elapsed("idle", false, age, Some(run)), None);
        assert_eq!(elapsed("idle", true, age, Some(run)), Some((run, false)));
        assert_eq!(
            elapsed("working", false, age, Some(run)),
            Some((Duration::from_secs(90), true))
        );
        assert_eq!(elapsed("working", false, None, Some(run)), None);
    }

    /// A client that attached after a run had already finished never saw it start, so there is no
    /// honest length to show. Nothing beats a number invented from the wrong clock.
    #[test]
    fn a_finished_run_with_no_banked_length_shows_no_duration() {
        let row = AgentRow {
            pane_id: 1,
            workspace_index: 0,
            pane_index: 0,
            title: "Claude Code".into(),
            status: Some("done".into()),
            activity: None,
            age: Some(std::time::Duration::from_secs(90)),
            run: None,
            cwd: None,
            cwd_host: None,
            project_root: None,
            subpath: None,
            branch: None,
            repository: None,
            finished_unseen: false,
            slot: None,
            identity: Some(AgentIdentity::new("claude", "Claude Code")),
            task: None,
            reason: None,
            order: 0,
            screen_title: None,
        };
        assert_eq!(row_duration(&row), None);
    }

    #[test]
    fn only_statuses_without_a_glyph_of_their_own_keep_their_word() {
        for canonical in ["working", " BLOCKED ", "Done", "idle"] {
            assert!(status_is_canonical(canonical), "{canonical}");
        }
        for custom in ["compacting", "waiting-on-ci", "review"] {
            assert!(!status_is_canonical(custom), "{custom}");
        }
    }

    /// The workspace number is what keybindings address and cannot be recovered from the row
    /// above, so the subpath is what gives way as the name line fills — not the pair together,
    /// which clipped the number off the edge.
    #[test]
    fn the_subpath_yields_to_everything_else_on_the_name_line() {
        // Room to spare: the subpath gets its full allowance.
        assert_eq!(subpath_budget(48, "OpenCode", None, "1"), SUBPATH_MAX_CHARS);
        // The elapsed run takes its share out of the subpath, not the number.
        assert!(
            subpath_budget(32, "OpenCode #1", Some("2h12m"), "1")
                < subpath_budget(32, "OpenCode #1", None, "1")
        );
        // A longer name and a named workspace both come out of the same allowance.
        assert!(
            subpath_budget(32, "GitHub Copilot #12", Some("2h12m"), "2:review")
                < subpath_budget(32, "OpenCode #1", Some("2h12m"), "1")
        );
        // Nothing left over saturates at zero rather than wrapping.
        assert_eq!(
            subpath_budget(16, "GitHub Copilot #12", Some("2h12m"), "2:review"),
            0
        );
    }

    #[test]
    fn activity_budget_tracks_the_configured_width() {
        // The elapsed time moved to the title line, so the whole detail line is the activity's.
        assert_eq!(activity_budget(32), 27);
        // A wider sidebar spends the whole difference on the activity.
        assert_eq!(activity_budget(48), 43);
        // A sidebar too narrow to budget for still gets a floor rather than zero.
        assert_eq!(activity_budget(8), 8);
    }

    #[test]
    fn reported_status_overrides_detected_state() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("claude", "Claude Code").into(),
            state: DetectedAgentState::Working,
        });
        pane.terminal.reported_status = Some(PaneStatus {
            value: "blocked".into(),
            reason: Some("approval".into()),
            set_at: 1,
        });
        sync_single_ref(pane);
        let rows = agent_rows(&state);
        assert_eq!(rows[0].status.as_deref(), Some("blocked"));
        assert_eq!(rows[0].activity.as_deref(), Some("approval"));
    }

    fn item_targets(rows: &[SidebarRow]) -> Vec<(usize, RowTarget)> {
        rows.iter()
            .filter(|row| matches!(row.kind, row::RowKind::Item(_)))
            .map(|row| (row.depth, row.target.clone()))
            .collect()
    }

    /// One agent working in one checkout from two panes is one cluster: two jobs in one place, not
    /// one job in each of two terminals. The cluster leads with its most urgent run and goes there.
    #[test]
    fn one_agent_in_one_checkout_clusters_across_panes() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let first = pane_in_project(1, Some("working"), "/x/rozi", "/x/rozi", Some("master"));
        let second = pane_in_project(2, Some("blocked"), "/x/rozi/src", "/x/rozi", Some("master"));
        state.current_mut().workspaces[0].panes = vec![first];
        state.current_mut().workspaces[1].panes = vec![second];

        let rows = agents_rows(&state, &Default::default());
        assert_eq!(
            item_targets(&rows),
            vec![
                (1, RowTarget::Pane(2)),
                (2, RowTarget::Pane(2)),
                (2, RowTarget::Pane(1)),
            ]
        );
        let cluster = rows
            .iter()
            .find(|row| row.expansion.is_some())
            .expect("a cluster row");
        assert_eq!(
            cluster.expansion.as_ref().unwrap().row,
            "cluster:|/x/rozi|claude"
        );
    }

    /// Without a known place nothing says two panes are related, so their agents stay apart; one
    /// pane's own sessions still cluster.
    #[test]
    fn agents_without_a_known_place_cluster_only_within_their_pane() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut sessions = pane(1, None, false);
        sessions.terminal.published_rows = vec![row("a", "working", true), row("b", "idle", false)];
        sync_published_refs(&mut sessions);
        state.current_mut().workspaces[0].panes = vec![sessions, pane(2, Some("idle"), false)];

        let rows = agents_rows(&state, &Default::default());
        let clusters = rows.iter().filter(|row| row.expansion.is_some()).count();
        assert_eq!(clusters, 1, "only the publishing pane's sessions cluster");
        assert_eq!(item_targets(&rows).len(), 4);
    }

    /// Selecting a cluster goes to its most urgent run itself - for a published session, the
    /// session, not merely the pane that hosts it.
    #[test]
    fn a_cluster_goes_to_its_most_urgent_runs_own_row() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut sessions = pane_in_project(1, None, "/x/rozi", "/x/rozi", Some("master"));
        sessions.terminal.published_rows =
            vec![row("a", "working", true), row("b", "blocked", false)];
        sync_published_refs(&mut sessions);
        let other = pane_in_project(2, Some("idle"), "/x/rozi", "/x/rozi", Some("master"));
        state.current_mut().workspaces[0].panes = vec![sessions, other];

        let rows = agents_rows(&state, &Default::default());
        let blocked = RowTarget::PublishedRow {
            pane_id: 1,
            row: crate::session::protocol::RowKey::new("sessions", "b"),
        };
        assert_eq!(item_targets(&rows)[0], (1, blocked));
    }

    /// Runs are named by their task, else what their terminal says, else the agent - numbered
    /// only where two would read the same, by where they sit rather than how urgent they are.
    #[test]
    fn cluster_members_are_numbered_only_to_break_a_tie() {
        let member = |pane_index: usize, task: Option<&str>| AgentRow {
            pane_id: pane_index as PaneId + 1,
            workspace_index: 0,
            pane_index,
            title: "Claude Code".into(),
            status: Some("idle".into()),
            activity: None,
            age: None,
            run: None,
            cwd: None,
            cwd_host: None,
            project_root: None,
            subpath: None,
            branch: None,
            repository: None,
            finished_unseen: false,
            slot: None,
            identity: Some(AgentIdentity::new("claude", "Claude Code")),
            task: task.map(str::to_string),
            reason: None,
            order: 0,
            screen_title: None,
        };
        let later = member(2, None);
        let titled = member(0, Some("✳ fix login"));
        let earlier = member(1, None);
        assert_eq!(
            member_labels(&[&later, &titled, &earlier]),
            ["Claude Code #2", "fix login", "Claude Code #1"]
        );
    }
}
