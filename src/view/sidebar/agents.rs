use tui_lipan::prelude::*;

use crate::session::protocol::pane_status;
use crate::state::{PaneId, State};
use crate::{HyprmuxApp, Msg};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentRow {
    pub pane_id: PaneId,
    pub workspace_index: usize,
    pub pane_index: usize,
    pub title: String,
    pub status: Option<String>,
    pub reason: Option<String>,
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

fn status_rank(status: Option<&str>) -> u8 {
    let Some(status) = status.map(normalized_status) else {
        return 5;
    };
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
                        reason: pane
                            .terminal
                            .reported_status
                            .as_ref()
                            .and_then(|status| status.reason.clone()),
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
            status_rank(row.status.as_deref()),
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

/// Whether a path uses the Windows drive/UNC shape. Unix paths may legally contain `\` inside a
/// segment name, so backslash splitting must be reserved for paths that are actually Windows-like.
fn is_windows_path_shape(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
}

fn path_segments(path: &str) -> Vec<&str> {
    if is_windows_path_shape(path) {
        path.split(['\\', '/']).filter(|s| !s.is_empty()).collect()
    } else {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }
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

fn truncate_reason(value: &str) -> String {
    const MAX_CHARS: usize = 28;
    if value.chars().count() <= MAX_CHARS {
        value.to_string()
    } else {
        let mut value = value
            .chars()
            .take(MAX_CHARS.saturating_sub(1))
            .collect::<String>();
        value.push('…');
        value
    }
}

pub(super) fn agents_tab(ctx: &Context<HyprmuxApp>) -> Element {
    let groups = agent_groups(&ctx.state);
    if groups.is_empty() {
        return Text::new("No agents detected")
            .style(super::super::fg_only(&ctx.state.theme.muted))
            .into();
    }
    // A lone fallback group renders flat, exactly as before grouping existed; a known project is
    // always headed, even alone — which project the agents operate in is the point of the tab.
    let show_headers = groups.len() > 1 || groups[0].project.is_some();
    let mut body = VStack::new().gap(0).padding((1, 0));
    for (index, group) in groups.into_iter().enumerate() {
        if show_headers {
            if index > 0 {
                body = body.child(Text::new("").height(Length::Px(1)));
            }
            body = body.child(group_header(ctx, &group));
        }
        for row in group.rows {
            body = body.child(agent_row(ctx, row, show_headers));
        }
    }
    body.into()
}

/// The glyph and color a row shows: the plain status glyph, except a finished-unseen agent that is
/// no longer working (and not blocked, which keeps its own loud glyph) shows a filled success dot
/// to pull the eye to a completed run. `bool` is whether the working spinner should animate.
fn row_glyph(status: &str, finished_unseen: bool, theme: &Theme) -> (String, Color, bool) {
    let working = status.trim().eq_ignore_ascii_case(pane_status::WORKING);
    let blocked = status.trim().eq_ignore_ascii_case(pane_status::BLOCKED);
    if finished_unseen && !working && !blocked {
        return ("●".to_string(), theme.status.success, false);
    }
    let (glyph, color) = status_glyph(status, theme);
    (glyph.to_string(), color, working)
}

/// One-line project header: the group's most urgent status as a glyph (rows are status-sorted, so
/// the first row carries it), aligned with the row glyph column, then the project label. A quiet
/// group with an unseen finish still pulses so the header alone flags a project worth revisiting.
fn group_header(ctx: &Context<HyprmuxApp>, group: &AgentGroup) -> Element {
    let status = group
        .rows
        .first()
        .and_then(|row| row.status.clone())
        .unwrap_or_else(|| pane_status::IDLE.to_string());
    let finished_unseen = group.rows.iter().any(|row| row.finished_unseen);
    let (glyph, color, _) = row_glyph(&status, finished_unseen, &ctx.state.theme);
    let label = group.project.as_deref().unwrap_or("elsewhere");
    let label_style = if group.project.is_some() {
        super::super::fg_only(&ctx.state.theme.accent).bold()
    } else {
        super::super::fg_only(&ctx.state.theme.muted).bold()
    };
    HStack::new()
        .gap(0)
        .height(Length::Px(1))
        .child(Text::new(format!(" {glyph} ")).style(Style::new().fg(color)))
        .child(Text::new(label.to_string()).style(label_style))
        .into()
}

/// A two-line agent row. `indent` nests the row under a project header: the status icon moves to
/// the header's label column so groups read as a tree; a flat (headerless) list keeps the icon at
/// the header glyph column.
fn agent_row(ctx: &Context<HyprmuxApp>, row: AgentRow, indent: bool) -> Element {
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
    let status_label = row
        .status
        .clone()
        .unwrap_or_else(|| pane_status::IDLE.to_string());
    let mut detail = HStack::new()
        .gap(1)
        .height(Length::Px(1))
        .child(Text::new(status_label).style(Style::new().fg(color)));
    if let Some(reason) = row.reason.as_deref() {
        detail = detail.child(
            Text::new(truncate_reason(reason))
                .style(super::super::fg_only(&ctx.state.theme.muted).dim()),
        );
    }
    let id = row.pane_id;
    let focused = ctx.state.focused_pane == Some(id);
    let marker = if focused { "▎" } else { " " };
    let content = HStack::new()
        .gap(0)
        .height(Length::Px(2))
        .style(if focused {
            Style::new().bg(ctx.state.theme.surface.element.elevate(0.04))
        } else {
            Style::default()
        })
        .child(
            VStack::new()
                .gap(0)
                .width(Length::Auto)
                .height(Length::Px(2))
                .child(
                    Text::new(marker)
                        .height(Length::Px(1))
                        .style(super::super::fg_only(&ctx.state.theme.accent)),
                )
                .child(
                    Text::new(marker)
                        .height(Length::Px(1))
                        .style(super::super::fg_only(&ctx.state.theme.accent)),
                ),
        )
        .child({
            let mut cells = HStack::new().gap(1).height(Length::Px(2));
            if indent {
                cells = cells.child(Text::new(" "));
            }
            cells.child(status_icon).child(
                VStack::new()
                    .gap(0)
                    .child(
                        Text::new(row.title).style(super::super::fg_only(&ctx.state.theme.primary)),
                    )
                    .child(detail),
            )
        });
    MouseRegion::new()
        .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
        .on_click(ctx.link().callback(move |_| Msg::SidebarFocusPane(id)))
        .child(content)
        .key(format!("sidebar-agent-{id}"))
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
        assert_eq!(rows[0].reason.as_deref(), Some("approval"));
    }
}
