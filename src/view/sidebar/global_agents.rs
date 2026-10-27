//! The Agents tab: every agent this client knows about, the ones that want attention first.
//!
//! The Activity tab answers "what is happening in this session"; this one answers "what needs me,
//! wherever it is". So it lists only agents that do - waiting on the user, failed, or finished
//! with a result nobody has read - and leaves agents still working, idle, or already read to
//! Activity and the Agents view. It stays flat and ordered by attention rather than grouped by
//! place: a blocked agent on another host belongs at the top, not under a heading that may be
//! scrolled away. Each row leads
//! with the work rather than the program, because twenty rows that all start with `Claude Code`
//! say nothing at a glance. The rows are [`crate::view::agents::global_agent_rows`], the
//! projection the Agents view lists, so the two surfaces can never disagree about what exists or
//! what order it comes in.

use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, Shape, SidebarRow};
use crate::state::State;
use crate::view::agents::GlobalAgentRow;

pub(super) fn global_agents_rows(state: &State) -> Vec<SidebarRow> {
    crate::view::agents::global_agent_rows(state)
        .into_iter()
        .filter(GlobalAgentRow::needs_attention)
        .map(|row| {
            let target = RowTarget::Agent(row.location.clone());
            let key = format!("agent:{:?}", row.location);
            // Ordered by attention, not grouped by place: every row is top-level.
            SidebarRow::item(agent_row(state, row), target)
                .shape(Shape::Flush)
                .keyed(key)
        })
        .collect()
}

/// Where a row's agent runs, as the badge says it: the session, `@host` when it is not this
/// machine. The session on screen is named too — on this tab every row is somewhere, and "here" is
/// only a fact about the current moment.
fn place_badge(row: &GlobalAgentRow) -> String {
    crate::view::vocabulary::session_at(&row.session, row.host.as_deref())
}

/// A two-line row: status glyph, what the agent works on and where it runs, then who it is and why
/// it is in its state.
///
/// An agent that named its work is titled by it, with its own name leading the second line. One
/// that did not is titled by its name, and the second line says where it works instead, which is
/// what tells two untitled runs of one program apart.
fn agent_row(state: &State, row: GlobalAgentRow) -> Row {
    let theme = &state.theme;
    let (glyph, color, spinner) = super::agents::row_glyph(&row.status, row.finished_unseen, theme);
    let glyph: Element = if spinner {
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
    let active = match &row.location {
        crate::state::AgentLocation::Here { pane, row: slot } => {
            state.current().focused_pane == Some(*pane)
                && slot
                    .as_ref()
                    .is_none_or(|key| published_row_active(state, *pane, key))
        }
        crate::state::AgentLocation::OtherSession { .. } => false,
    };
    let muted = super::super::fg_only(&theme.muted).dim();
    let mut content = Row::new(row.title())
        .active(active)
        .glyph(glyph)
        .title_style(if row.background {
            super::super::fg_only(&theme.primary).italic()
        } else {
            super::super::fg_only(&theme.primary)
        })
        .badge_text(place_badge(&row), muted);
    if row.background {
        content = content.detail_badge(crate::view::vocabulary::BACKGROUND_RUN, muted);
    }
    if let Some(elapsed) = row.elapsed {
        content = content.meta(super::agents::format_age(elapsed), Style::new().fg(color));
    }
    let budget = super::agents::activity_budget(
        state.sidebar_tab_width(&crate::config::SidebarTabId::new("agents")),
    );
    if row.task.is_some() {
        content = content.detail(super::row::truncate(&row.agent, budget), muted);
    }
    content = content.detail(
        super::row::truncate(&state_text(&row), budget),
        Style::new().fg(color),
    );
    if row.task.is_none()
        && let Some(place) = row.place()
    {
        content = content.detail(super::row::truncate(&place, budget), muted);
    }
    content
}

/// Why the agent is in its state when it said, else the state itself. A finished run nobody has
/// looked at always reads `Done`: that is the news, whatever the agent last gave as a reason.
fn state_text(row: &GlobalAgentRow) -> String {
    match row.reason.as_deref() {
        Some(reason) if !row.finished_unseen => reason.to_string(),
        _ => row.status_label(),
    }
}

/// Whether the program in `pane` currently shows the published row `key`. Focusing a pane reveals
/// only the row its program has on screen, so only that one reads as the active agent.
fn published_row_active(
    state: &State,
    pane: crate::state::PaneId,
    key: &crate::session::protocol::RowKey,
) -> bool {
    state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.panes)
        .find(|candidate| candidate.id == pane)
        .is_some_and(|pane| {
            pane.terminal
                .effective_rows()
                .iter()
                .any(|row| row.has_key(key) && row.active)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(host: Option<&str>, session: &str) -> GlobalAgentRow {
        GlobalAgentRow {
            location: crate::state::AgentLocation::OtherSession {
                target: None,
                session: session.into(),
                pane: 1,
                row: None,
            },
            agent: "Codex".into(),
            task: None,
            host: host.map(str::to_string),
            session: session.into(),
            status: "blocked".into(),
            reason: None,
            finished_unseen: false,
            elapsed: None,
            project: None,
            branch: None,
            ordinal: None,
            background: false,
        }
    }

    /// A row says which machine it is on before it says anything else: on a list that spans every
    /// host, "Codex" alone names nothing.
    #[test]
    fn the_badge_names_host_and_session() {
        assert_eq!(
            place_badge(&row(Some("workbox"), "backend")),
            "backend@workbox"
        );
        assert_eq!(place_badge(&row(None, "dev")), "dev");
    }

    /// A blocked agent says why; a finished run nobody has read says it is done, whatever the agent
    /// last gave as a reason.
    #[test]
    fn the_state_line_prefers_the_reason_until_a_run_is_unread() {
        let mut row = row(None, "dev");
        assert_eq!(state_text(&row), "Blocked");
        row.reason = Some("Permission required".into());
        assert_eq!(state_text(&row), "Permission required");
        row.status = "idle".into();
        row.finished_unseen = true;
        assert_eq!(state_text(&row), "Done");
    }

    /// Temporary sessions have generated names; the badge spells them the way every other surface
    /// does rather than leaking the generated id.
    #[test]
    fn a_temporary_session_reads_as_ephemeral() {
        let name = crate::state::ephemeral_session_name();
        assert_eq!(place_badge(&row(None, &name)), "ephemeral");
    }
}
