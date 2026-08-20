use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, SidebarRow};
use crate::AppRoot;
use crate::platform::paths::path_segments;
use crate::session::protocol::pane_status;
use crate::state::{PaneId, State};

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
    /// The agent finished a run (went quiescent) since the pane was last focused; drives the filled
    /// attention pulse until the pane is looked at.
    pub finished_unseen: bool,
    /// Which published slot this row stands for, when its pane published any. `None` is the
    /// one-row-per-pane path every agent that publishes nothing keeps taking.
    pub slot: Option<SlotRef>,
}

/// A row's identity within a pane that publishes several agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlotRef {
    pub id: String,
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
    pub project: Option<String>,
    /// Branch checked out in the group's project, shown on its header.
    pub branch: Option<String>,
    pub rows: Vec<AgentRow>,
}

fn normalized_status(value: &str) -> &str {
    value.trim()
}

/// Sort rank for a row, keyed on the status the row *displays* rather than the raw one the agent
/// reported. A finished-unseen agent reports `idle` but reads "done", so it ranks with `done`:
/// ranking it as idle would sink the row to the bottom of its group at the same moment the filled
/// dot lights up to draw the eye to it.
fn status_rank(status: Option<&str>, finished_unseen: bool) -> u8 {
    let Some(status) = status.map(normalized_status) else {
        return 5;
    };
    if is_finished_quiet(status, finished_unseen) {
        return 3;
    }
    if status.eq_ignore_ascii_case(pane_status::BLOCKED) {
        0
    } else if status.eq_ignore_ascii_case(pane_status::WORKING) {
        1
    } else if status.eq_ignore_ascii_case(pane_status::DONE) {
        3
    } else if status.eq_ignore_ascii_case(pane_status::IDLE) {
        4
    } else {
        2
    }
}

/// What the agent is currently doing, for the row's detail line.
///
/// The reason it published alongside its status is the authoritative answer, but only agents with a
/// status integration set one. Everything else falls back to the terminal title: agents write their
/// current task there, which makes it the only activity signal a detected-only agent offers.
fn activity_text(pane: &crate::pane::TerminalPane, kind_label: &str) -> Option<String> {
    if let Some(reason) = pane
        .reported_status
        .as_ref()
        .and_then(|status| status.reason.as_deref())
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        return Some(reason.to_string());
    }
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
                .filter(|(_, pane)| {
                    pane.id != crate::state::POPUP_PANE_ID
                        && !pane.closing
                        && (!pane.terminal.published_rows.is_empty()
                            || pane.terminal.detected_agent.is_some())
                })
                .flat_map(move |(pane_index, pane)| {
                    let cwd = pane
                        .terminal
                        .cwd
                        .clone()
                        .filter(|cwd| !cwd.trim().is_empty());
                    let project_root = cwd
                        .is_some()
                        .then(|| pane.terminal.project_root.clone())
                        .flatten();
                    let subpath =
                        project_root
                            .as_deref()
                            .zip(cwd.as_deref())
                            .and_then(|(root, cwd)| {
                                crate::platform::paths::project_relative_path(root, cwd)
                            });

                    if let Some(detected) = pane.terminal.detected_agent.as_ref() {
                        let row = AgentRow {
                            pane_id: pane.id,
                            workspace_index,
                            pane_index,
                            title: detected.agent.label.clone(),
                            // Always `Some`: `agent_status` returns `None` only when there is no
                            // detected agent, and the filter above already established one.
                            status: pane.terminal.agent_status(),
                            activity: activity_text(&pane.terminal, &detected.agent.label),
                            age: pane.terminal.status_age(),
                            run: pane.terminal.last_run,
                            cwd_host: cwd
                                .is_some()
                                .then(|| pane.terminal.cwd_host.clone())
                                .flatten(),
                            cwd: cwd.clone(),
                            branch: project_root
                                .is_some()
                                .then(|| pane.terminal.git_branch.clone())
                                .flatten(),
                            project_root: project_root.clone(),
                            subpath: subpath.clone(),
                            finished_unseen: pane.terminal.finished_unseen,
                            slot: None,
                        };
                        if pane.terminal.published_rows.is_empty() {
                            return vec![row];
                        }
                        pane.terminal
                            .published_rows
                            .iter()
                            .enumerate()
                            .map(|(index, published_row)| {
                                let ui = pane.terminal.published_row_ui.get(&published_row.id);
                                AgentRow {
                                    // The name column answers "what is this", and for every other row
                                    // that is the agent. Slots keep it and add their position, so a
                                    // pane's rows are distinguishable at a glance and still read as
                                    // the same program; the slot's own title is what it is *doing*,
                                    // which belongs on the detail line with every other activity.
                                    title: format!(
                                        "{} #{}",
                                        detected.agent.label,
                                        index.saturating_add(1)
                                    ),
                                    status: Some(published_row.status.clone()),
                                    activity: row_activity(published_row, &detected.agent.label),
                                    age: pane.terminal.row_age(published_row),
                                    run: ui.and_then(|ui| ui.last_run),
                                    finished_unseen: ui.is_some_and(|ui| ui.finished_unseen),
                                    slot: Some(SlotRef {
                                        id: published_row.id.clone(),
                                        index,
                                        active: published_row.active,
                                    }),
                                    ..row.clone()
                                }
                            })
                            .collect()
                    } else {
                        // No detected agent, but rows were published.
                        let base_row = AgentRow {
                            pane_id: pane.id,
                            workspace_index,
                            pane_index,
                            title: pane.display_title(pane.terminal.title()),
                            status: None,
                            activity: None,
                            age: None,
                            run: None,
                            cwd_host: cwd
                                .is_some()
                                .then(|| pane.terminal.cwd_host.clone())
                                .flatten(),
                            cwd: cwd.clone(),
                            branch: project_root
                                .is_some()
                                .then(|| pane.terminal.git_branch.clone())
                                .flatten(),
                            project_root: project_root.clone(),
                            subpath: subpath.clone(),
                            finished_unseen: false,
                            slot: None,
                        };
                        pane.terminal
                            .published_rows
                            .iter()
                            .enumerate()
                            .map(|(index, published_row)| {
                                let ui = pane.terminal.published_row_ui.get(&published_row.id);
                                let title = if !published_row.title.trim().is_empty() {
                                    published_row.title.clone()
                                } else {
                                    pane.display_title(pane.terminal.title())
                                };
                                AgentRow {
                                    title,
                                    status: Some(published_row.status.clone()),
                                    activity: published_row
                                        .reason
                                        .as_deref()
                                        .map(str::trim)
                                        .filter(|r| !r.is_empty())
                                        .map(str::to_string),
                                    age: pane.terminal.row_age(published_row),
                                    run: ui.and_then(|ui| ui.last_run),
                                    finished_unseen: ui.is_some_and(|ui| ui.finished_unseen),
                                    slot: Some(SlotRef {
                                        id: published_row.id.clone(),
                                        index,
                                        active: published_row.active,
                                    }),
                                    ..base_row.clone()
                                }
                            })
                            .collect()
                    }
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        (
            status_rank(row.status.as_deref(), row.finished_unseen),
            row.workspace_index,
            row.pane_index,
            row.slot.as_ref().map_or(0, |slot| slot.index),
        )
    });
    rows
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
        .map(|group| project_label(&group.path, group.host.as_deref(), false))
        .collect();
    // Disambiguate duplicate final labels with one parent segment (VS Code-style); a residual
    // collision after that is accepted — group identity is the full path, the label is display.
    let ambiguous: Vec<bool> = labels
        .iter()
        .map(|label| {
            labels
                .iter()
                .filter(|other| other.eq_ignore_ascii_case(label))
                .count()
                > 1
        })
        .collect();
    for (index, group) in known.iter().enumerate() {
        if ambiguous[index] {
            labels[index] = project_label(&group.path, group.host.as_deref(), true);
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
                keyed.path,
                AgentGroup {
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
        ("!", theme.status.error)
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
fn format_age(age: std::time::Duration) -> String {
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
    let label = row_status_label(status, row.finished_unseen);
    let label = normalized_status(&label);
    if label.eq_ignore_ascii_case(pane_status::IDLE) {
        return None;
    }
    if label.eq_ignore_ascii_case(pane_status::DONE) {
        // Absent when this client never saw the run start — better nothing than a wrong number.
        return row.run.map(|run| (format_age(run), false));
    }
    row.age.map(|age| (format_age(age), true))
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
fn activity_budget(width: u16) -> usize {
    // Gutter, glyph, their gaps, the divider, and the scrollbar column.
    let chrome = 5;
    (usize::from(width)).saturating_sub(chrome).max(8)
}

pub(super) fn agents_rows(ctx: &Context<AppRoot>) -> Vec<SidebarRow> {
    let groups = agent_groups(&ctx.state);
    if groups.is_empty() {
        return Vec::new();
    }
    // A lone fallback group renders flat, exactly as before grouping existed; a known project is
    // always headed, even alone — which project the agents operate in is the point of the tab.
    let show_headers = groups.len() > 1 || groups[0].project.is_some();
    let mut rows = Vec::new();
    for group in groups {
        if !rows.is_empty() {
            rows.push(SidebarRow::spacer());
        }
        if show_headers {
            rows.push(SidebarRow::header(group_header(ctx, &group)));
        }
        for row in group.rows {
            let target = match &row.slot {
                Some(slot) => RowTarget::PublishedRow {
                    pane_id: row.pane_id,
                    row_id: slot.id.clone(),
                },
                None => RowTarget::Pane(row.pane_id),
            };
            rows.push(SidebarRow::item(agent_row(ctx, row), target));
        }
    }
    rows
}

/// Whether a row is in the "finished a run, not looked at yet" state: quiescent (neither working
/// nor blocked, which keep their own louder presentation) with an unseen finish.
fn is_finished_quiet(status: &str, finished_unseen: bool) -> bool {
    let working = status.trim().eq_ignore_ascii_case(pane_status::WORKING);
    let blocked = status.trim().eq_ignore_ascii_case(pane_status::BLOCKED);
    finished_unseen && !working && !blocked
}

/// The status word a row displays. A finished-unseen agent reads "done" regardless of the raw
/// status the agent last reported, which is usually "idle" — idle describes the pane, done
/// describes the run that just ended.
fn row_status_label(status: &str, finished_unseen: bool) -> String {
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
fn row_glyph(status: &str, finished_unseen: bool, theme: &Theme) -> (String, Color, bool) {
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
/// The branch earns that space because the group is a *directory*: a worktree of the same
/// repository is its own group under a name that says nothing about which line of work it holds
/// (`api` and `api-2`), and knowing which branch an agent is committing to is the difference
/// between reading its output and trusting it. Nothing else about the repository is here — how far
/// the branch has diverged from its upstream cannot be known without a fetch, and a stale number
/// presented as current is worse than no number.
///
/// The header deliberately carries no aggregated status glyph. Groups never collapse, so every row
/// it would summarize is already on screen directly beneath it — and the glyph plus the nesting it
/// forced cost four cells on every row of the narrowest surface in the app.
fn group_header(ctx: &Context<AppRoot>, group: &AgentGroup) -> Element {
    let Some(label) = group.project.as_deref() else {
        return row::header(ctx, "elsewhere", true);
    };
    match group.branch.as_deref() {
        Some(branch) => row::header_with_note(
            ctx,
            label,
            row::truncate_start(branch, branch_budget(ctx.state.sidebar_requested_width())),
        ),
        None => row::header(ctx, label, false),
    }
}

/// Character budget for the branch on a project header. Half the sidebar is the most a branch may
/// take from the project name beside it; below that the pair stops being two readable things.
fn branch_budget(width: u16) -> usize {
    (usize::from(width) / 2).max(6)
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
    // Gutter, glyph, their gaps, the divider, the scrollbar column, and the ` · ` joining the
    // subpath to the workspace number.
    let chrome = 10;
    let taken = name.chars().count()
        + workspace.chars().count()
        + duration.map_or(0, |text| text.chars().count() + 1);
    usize::from(width)
        .saturating_sub(chrome)
        .saturating_sub(taken)
        .min(SUBPATH_MAX_CHARS)
}

/// A two-line agent row: status glyph, agent name and workspace badge, then the detail line.
fn agent_row(ctx: &Context<AppRoot>, row: AgentRow) -> Row {
    let status = row.status.as_deref().unwrap_or(pane_status::IDLE);
    let (glyph, color, spinner) = row_glyph(status, row.finished_unseen, &ctx.state.theme);
    let status_icon: Element = if spinner {
        Spinner::new()
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into()
    } else {
        Text::new(glyph)
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into()
    };
    let duration = row_duration(&row).map(|(text, _)| text);
    let mut content = Row::new(row.title.clone())
        // For a published slot, being on screen in its own program decides this: a focused pane
        // shows exactly one of its slots, so marking them all active would say nothing.
        .active(
            ctx.state.current().focused_pane == Some(row.pane_id)
                && row.slot.as_ref().is_none_or(|slot| slot.active),
        )
        .glyph(status_icon)
        .title_style(super::super::fg_only(&ctx.state.theme.primary))
        // Which workspace the agent lives on. Groups are projects, and a project's agents can be
        // spread across workspaces, so this is the cross-reference to the Panes tab — and the hint
        // that two rows cannot be watched side by side.
        //
        // Where the agent sits below its project root leads it: in a monorepo, `services/api` is
        // the only thing separating two rows that otherwise read identically, and the group header
        // can no longer say it now that it names the project rather than the directory.
        .badge_text(
            {
                let workspace = super::workspace_badge(&ctx.state, row.workspace_index);
                match row.subpath.as_deref() {
                    Some(subpath) => {
                        let budget = subpath_budget(
                            ctx.state.sidebar_requested_width(),
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
            super::super::fg_only(&ctx.state.theme.muted).dim(),
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
        let budget = activity_budget(ctx.state.sidebar_requested_width());
        content = content.detail(
            row::truncate(activity, budget),
            super::super::fg_only(&ctx.state.theme.muted).dim(),
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
        pane.terminal.cwd = cwd.map(str::to_string);
        pane
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
            vec![3, 7, 4, 2, 5, 1, 6]
        );
    }

    fn row(id: &str, status: &str, active: bool) -> crate::session::protocol::PublishedRow {
        crate::session::protocol::PublishedRow {
            id: id.to_string(),
            title: format!("tab {id}"),
            status: status.to_string(),
            reason: None,
            active,
            work_started_at: None,
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
        state.current_mut().workspaces[0].panes = vec![publisher];

        let rows = agent_rows(&state);
        assert_eq!(
            rows.iter()
                .map(|row| row.slot.as_ref().unwrap().id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "one pane's rows keep the publisher's own order among themselves"
        );
        assert!(rows.iter().all(|row| row.pane_id == 1));
        // The name column stays the agent, numbered so one pane's rows are distinguishable; the
        // row's own title is what it is doing, and moves to the detail line.
        assert_eq!(rows[1].title, "Claude Code #2");
        assert_eq!(rows[1].activity.as_deref(), Some("tab b"));
    }

    #[test]
    fn a_pane_without_detected_agent_renders_published_rows() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.detected_agent = None;
        publisher.terminal.published_rows = vec![
            crate::session::protocol::PublishedRow {
                id: "build".into(),
                title: "Cargo Watch".into(),
                status: "working".into(),
                reason: Some("compiling crates".into()),
                active: true,
                work_started_at: None,
            },
            crate::session::protocol::PublishedRow {
                id: "test".into(),
                title: String::new(),
                status: "blocked".into(),
                reason: Some("assertion failed".into()),
                active: false,
                work_started_at: None,
            },
        ];
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

    /// The name column reads `Claude Code #1` for every row in a pane, so the title is the only
    /// thing saying which one this is. A reason cannot be allowed to hide it: a session is often
    /// blocked on its first question before anything has titled it, and the row would then be
    /// stuck reading "answer required" for the rest of the run.
    #[test]
    fn a_titled_row_shows_its_title_even_while_blocked() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let mut publisher = pane(1, None, false);
        publisher.terminal.published_rows = vec![crate::session::protocol::PublishedRow {
            title: "audit the widget layer".into(),
            reason: Some("permission required".into()),
            ..row("a", "blocked", true)
        }];
        state.current_mut().workspaces[0].panes = vec![publisher];

        let rows = agent_rows(&state);
        assert_eq!(rows[0].title, "Claude Code #1");
        assert_eq!(rows[0].activity.as_deref(), Some("audit the widget layer"));
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
        state.current_mut().workspaces[0].panes = vec![publisher];

        assert_eq!(
            agent_rows(&state)
                .iter()
                .map(|row| row.slot.as_ref().unwrap().id.as_str())
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
        for id in ["shown", "hidden"] {
            publisher.terminal.published_row_ui.insert(
                id.to_string(),
                crate::pane::PublishedRowUiState {
                    finished_unseen: true,
                    last_run: None,
                },
            );
        }
        state.current_mut().workspaces[0].panes = vec![publisher];
        state.window_focused = true;
        state.current_mut().focused_pane = Some(1);

        assert!(crate::ops::focus::acknowledge_pane_if_attended(
            &mut state, 1
        ));

        let pulses = agent_rows(&state)
            .into_iter()
            .map(|row| (row.slot.unwrap().id, row.finished_unseen))
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
        finished.terminal.finished_unseen = true;
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
        assert_eq!(status_glyph(" BLOCKED ", &theme).0, "!");
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
        assert_eq!(row_glyph("blocked", true, &theme).0, "!");
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

    /// The branch column may take at most half the header, and keeps the tail of a long branch
    /// name — `feat/pricing-v2` and `feat/pricing-v3` differ only there.
    #[test]
    fn branch_budget_splits_the_header_and_keeps_the_distinguishing_tail() {
        assert_eq!(branch_budget(32), 16);
        assert_eq!(branch_budget(48), 24);
        assert_eq!(branch_budget(2), 6, "a floor, never zero");
        assert_eq!(row::truncate_start("master", branch_budget(32)), "master");
        assert_eq!(
            row::truncate_start("feat/show-working-directory-v2", branch_budget(32)),
            "…ng-directory-v2"
        );
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
            finished_unseen,
            slot: None,
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
            row_duration(&row("done", false)),
            Some(("12m00s".into(), false))
        );
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
            finished_unseen: false,
            slot: None,
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
        let rows = agent_rows(&state);
        assert_eq!(rows[0].status.as_deref(), Some("blocked"));
        assert_eq!(rows[0].activity.as_deref(), Some("approval"));
    }
}
