use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;
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
    /// The agent finished a run (went quiescent) since the pane was last focused; drives the filled
    /// attention pulse until the pane is looked at.
    pub finished_unseen: bool,
}

/// Agents that share a working directory, herdr-style "space" grouping. `project` is `None` for
/// the trailing fallback group of agents whose pane has no usable cwd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentGroup {
    pub project: Option<String>,
    pub rows: Vec<AgentRow>,
}

fn detected_status(state: crate::session::protocol::DetectedAgentState) -> &'static str {
    match state {
        crate::session::protocol::DetectedAgentState::Idle => pane_status::IDLE,
        crate::session::protocol::DetectedAgentState::Working => pane_status::WORKING,
        crate::session::protocol::DetectedAgentState::Blocked => pane_status::BLOCKED,
    }
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
    let title = strip_title_decoration(&title);
    if title.is_empty()
        || title.eq_ignore_ascii_case(kind_label)
        || is_cwd_echo(title, pane.cwd.as_deref())
    {
        return None;
    }
    Some(title.to_string())
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
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(workspace_index, workspace)| {
            workspace
                .panes
                .iter()
                .enumerate()
                .filter(|(_, pane)| {
                    !pane.closing
                        && pane.id != crate::state::SCRATCH_PANE_ID
                        && pane.id != crate::state::POPUP_PANE_ID
                        && pane.terminal.detected_agent.is_some()
                })
                .map(move |(pane_index, pane)| {
                    let detected = pane
                        .terminal
                        .detected_agent
                        .as_ref()
                        .expect("agent filter and row construction agree");
                    let cwd = pane
                        .terminal
                        .cwd
                        .clone()
                        .filter(|cwd| !cwd.trim().is_empty());
                    AgentRow {
                        pane_id: pane.id,
                        workspace_index,
                        pane_index,
                        title: detected.kind.label().to_string(),
                        status: Some(
                            pane.terminal
                                .agent_status()
                                .unwrap_or_else(|| detected_status(detected.state).to_string()),
                        ),
                        activity: activity_text(&pane.terminal, detected.kind.label()),
                        age: pane.terminal.status_age(),
                        run: pane.terminal.last_run,
                        cwd_host: cwd
                            .is_some()
                            .then(|| pane.terminal.cwd_host.clone())
                            .flatten(),
                        cwd,
                        finished_unseen: pane.terminal.finished_unseen,
                    }
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        (
            status_rank(row.status.as_deref(), row.finished_unseen),
            row.workspace_index,
            row.pane_index,
        )
    });
    rows
}

/// Group agent rows by working directory (host + path). Group order is stable — alphabetical by
/// label, fallback group last — never by status, so blocks do not jump while agents flip between
/// states; urgency surfaces through the aggregated header glyph and the within-group row sort
/// instead. Row order inside a group keeps the [`agent_rows`] status sort.
pub(crate) fn agent_groups(state: &State) -> Vec<AgentGroup> {
    struct Keyed {
        host: Option<String>,
        cwd: String,
        rows: Vec<AgentRow>,
    }
    let mut known: Vec<Keyed> = Vec::new();
    let mut unknown: Vec<AgentRow> = Vec::new();
    for row in agent_rows(state) {
        match row.cwd.clone() {
            Some(cwd) => {
                let host = row.cwd_host.clone();
                if let Some(group) = known.iter_mut().find(|group| {
                    group.host == host && crate::platform::paths::paths_equal(&group.cwd, &cwd)
                }) {
                    group.rows.push(row);
                } else {
                    known.push(Keyed {
                        host,
                        cwd,
                        rows: vec![row],
                    });
                }
            }
            None => unknown.push(row),
        }
    }

    let mut labels: Vec<String> = known
        .iter()
        .map(|group| project_label(&group.cwd, group.host.as_deref(), false))
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
            labels[index] = project_label(&group.cwd, group.host.as_deref(), true);
        }
    }

    let mut groups: Vec<(String, AgentGroup)> = known
        .into_iter()
        .zip(labels)
        .map(|(keyed, label)| {
            (
                keyed.cwd,
                AgentGroup {
                    project: Some(label),
                    rows: keyed.rows,
                },
            )
        })
        .collect();
    groups.sort_by(|(cwd_a, a), (cwd_b, b)| {
        let label_a = a.project.as_deref().unwrap_or_default();
        let label_b = b.project.as_deref().unwrap_or_default();
        label_a
            .to_lowercase()
            .cmp(&label_b.to_lowercase())
            .then_with(|| cwd_a.cmp(cwd_b))
    });
    let mut groups: Vec<AgentGroup> = groups.into_iter().map(|(_, group)| group).collect();
    if !unknown.is_empty() {
        groups.push(AgentGroup {
            project: None,
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
fn format_age(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 60 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 24 * 60 * 60 {
        format!("{}h", secs / (60 * 60))
    } else {
        format!("{}d", secs / (24 * 60 * 60))
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
fn activity_budget(width: u16, duration: Option<&str>) -> usize {
    // Gutter, glyph, their gaps, the divider, and the scrollbar column.
    let chrome = 5;
    let duration = duration.map_or(0, |text| text.chars().count() + 1);
    (usize::from(width))
        .saturating_sub(chrome)
        .saturating_sub(duration)
        .max(8)
}

pub(super) fn agents_rows(ctx: &Context<HyprmuxApp>) -> Vec<SidebarRow> {
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
            let id = row.pane_id;
            rows.push(SidebarRow::item(agent_row(ctx, row), RowTarget::Pane(id)));
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
    if is_finished_quiet(status, finished_unseen) {
        return pane_status::DONE.to_string();
    }
    status.to_string()
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

/// One-line project header: the project label alone, at the same column every other tab's headers
/// use.
///
/// It deliberately carries no aggregated status glyph. Groups never collapse, so every row it would
/// summarize is already on screen directly beneath it — and the glyph plus the nesting it forced
/// cost four cells on every row of the narrowest surface in the app.
fn group_header(ctx: &Context<HyprmuxApp>, group: &AgentGroup) -> Element {
    match group.project.as_deref() {
        Some(label) => row::header(ctx, label, false),
        None => row::header(ctx, "elsewhere", true),
    }
}

/// A two-line agent row: status glyph, agent name and workspace badge, then the detail line.
fn agent_row(ctx: &Context<HyprmuxApp>, row: AgentRow) -> Row {
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
        .active(ctx.state.focused_pane == Some(row.pane_id))
        .glyph(status_icon)
        .title_style(super::super::fg_only(&ctx.state.theme.primary))
        // Which workspace the agent lives on. Groups are projects, and a project's agents can be
        // spread across workspaces, so this is the cross-reference to the Panes tab — and the hint
        // that two rows cannot be watched side by side.
        .badge(
            super::workspace_badge(&ctx.state, row.workspace_index),
            super::super::fg_only(&ctx.state.theme.muted).dim(),
        );

    // Detail line: how long, then what. It always names a subject, so the duration is never a bare
    // number with nothing to modify — the activity when the agent published one, and the status
    // word otherwise. A canonical status yields to a real activity, since its glyph already carries
    // it; a custom one like "compacting" keeps its word either way, having only a `•` to lean on.
    if let Some(duration) = duration.as_deref() {
        content = content.detail(duration.to_string(), Style::new().fg(color));
    }
    let label = row_status_label(status, row.finished_unseen);
    if !status_is_canonical(&label) || row.activity.is_none() {
        content = content.detail(label, Style::new().fg(color));
    }
    if let Some(activity) = row.activity.as_deref() {
        let budget = activity_budget(ctx.state.config.sidebar.width, duration.as_deref());
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
    use crate::session::protocol::{AgentKind, DetectedAgent, DetectedAgentState, PaneStatus};
    use crate::state::Pane;

    fn pane(id: PaneId, value: Option<&str>, closing: bool) -> Pane {
        pane_in(id, value, closing, None)
    }

    fn pane_in(id: PaneId, value: Option<&str>, closing: bool, cwd: Option<&str>) -> Pane {
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
        pane.closing = closing;
        pane.terminal.detected_agent = Some(DetectedAgent {
            kind: AgentKind::Claude,
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

    #[test]
    fn rows_sort_by_status_then_workspace_and_pane_order() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes = vec![
            pane(1, Some("idle"), false),
            pane(2, Some("Custom"), false),
            pane(3, Some("BLOCKED"), false),
        ];
        state.workspaces[1].panes = vec![
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
            vec![3, 4, 2, 5, 1, 6]
        );
    }

    /// A finished-unseen agent reports `idle` but displays "done", and must sort with `done` —
    /// above plain idle rows — so the row does not sink as it lights up.
    #[test]
    fn finished_unseen_rows_sort_as_done_not_idle() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        let mut finished = pane(2, Some("idle"), false);
        finished.terminal.finished_unseen = true;
        state.workspaces[0].panes = vec![pane(1, Some("idle"), false), finished];

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
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        assert!(agent_rows(&state).is_empty());
        state.workspaces[0].panes[0].terminal.detected_agent = Some(DetectedAgent {
            kind: AgentKind::OpenCode,
            state: DetectedAgentState::Working,
        });
        let rows = agent_rows(&state);
        assert_eq!(rows[0].title, "OpenCode");
        assert_eq!(rows[0].status.as_deref(), Some("working"));
    }

    #[test]
    fn groups_sort_alphabetically_with_unknown_last_and_keep_row_status_order() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes = vec![
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

    #[test]
    fn duplicate_project_basenames_gain_a_parent_segment() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes = vec![
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
    fn durations_coarsen_and_idle_rows_show_none() {
        use std::time::Duration;
        assert_eq!(format_age(Duration::from_secs(0)), "0s");
        assert_eq!(format_age(Duration::from_secs(59)), "59s");
        assert_eq!(format_age(Duration::from_secs(60)), "1m");
        assert_eq!(format_age(Duration::from_secs(59 * 60 + 59)), "59m");
        assert_eq!(format_age(Duration::from_secs(60 * 60)), "1h");
        assert_eq!(format_age(Duration::from_secs(24 * 60 * 60 - 1)), "23h");
        assert_eq!(format_age(Duration::from_secs(24 * 60 * 60)), "1d");

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
            finished_unseen,
        };
        // A live state reports how long it has held, and keeps advancing.
        assert_eq!(
            row_duration(&row("working", false)),
            Some(("1m".into(), true))
        );
        assert_eq!(
            row_duration(&row("compacting", false)),
            Some(("1m".into(), true))
        );
        // Idle times nothing: the row still gets its second line, but from the status word alone.
        assert_eq!(row_duration(&row("idle", false)), None);
        // A finished run reports what it cost — the banked 12m, not the 90s since it stopped — and
        // stops advancing, so the tick has nothing left to refresh.
        assert_eq!(
            row_duration(&row("idle", true)),
            Some(("12m".into(), false))
        );
        assert_eq!(
            row_duration(&row("done", false)),
            Some(("12m".into(), false))
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
            finished_unseen: false,
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

    #[test]
    fn activity_budget_tracks_the_configured_width() {
        // A default 32-wide sidebar, after a "12m" column.
        assert_eq!(activity_budget(32, Some("12m")), 23);
        // No duration column hands its width back to the text.
        assert_eq!(activity_budget(32, None), 27);
        // A wider sidebar spends the whole difference on the activity.
        assert_eq!(activity_budget(48, Some("12m")), 39);
        // A sidebar too narrow to budget for still gets a floor rather than zero.
        assert_eq!(activity_budget(8, Some("12m")), 8);
    }

    #[test]
    fn reported_status_overrides_detected_state() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        let pane = &mut state.workspaces[0].panes[0];
        pane.terminal.detected_agent = Some(DetectedAgent {
            kind: AgentKind::Claude,
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
