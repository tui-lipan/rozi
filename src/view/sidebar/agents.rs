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
                    AgentRow {
                        pane_id: pane.id,
                        workspace_index,
                        pane_index,
                        title: detected.kind.label().to_string(),
                        status: Some(
                            pane.terminal
                                .reported_status
                                .as_ref()
                                .map(|status| status.value.as_str())
                                .unwrap_or_else(|| detected_status(detected.state))
                                .to_string(),
                        ),
                        reason: pane
                            .terminal
                            .reported_status
                            .as_ref()
                            .and_then(|status| status.reason.clone()),
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
    let rows = agent_rows(&ctx.state);
    if rows.is_empty() {
        return Text::new("No agents detected")
            .style(super::super::fg_only(&ctx.state.theme.muted))
            .into();
    }
    rows.into_iter()
        .fold(VStack::new().gap(0).padding((1, 0)), |body, row| {
            let status = row.status.as_deref().unwrap_or(pane_status::IDLE);
            let (glyph, color) = status_glyph(status, &ctx.state.theme);
            let status_icon: Element = if status.trim().eq_ignore_ascii_case(pane_status::WORKING) {
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
                .child(
                    HStack::new().gap(1).height(Length::Px(2)).child(status_icon).child(
                        VStack::new()
                            .gap(0)
                            .child(
                                Text::new(row.title)
                                    .style(super::super::fg_only(&ctx.state.theme.primary)),
                            )
                            .child(detail),
                    ),
                );
            body.child(
                MouseRegion::new()
                    .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                    .on_click(ctx.link().callback(move |_| Msg::SidebarFocusPane(id)))
                    .child(content)
                    .key(format!("sidebar-agent-{id}")),
            )
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{AgentKind, DetectedAgent, DetectedAgentState, PaneStatus};
    use crate::state::Pane;

    fn pane(id: PaneId, value: Option<&str>, closing: bool) -> Pane {
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
