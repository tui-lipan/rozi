//! The Agents tab: every agent this client knows about, the ones that want attention first.
//!
//! The Activity tab answers "what is happening in this session"; this one answers "what needs me,
//! wherever it is". So it stays flat and ordered by attention rather than grouped by place: a
//! blocked agent on another host belongs at the top, not under a heading that may be scrolled away.
//! The rows are [`crate::view::agents::global_agent_rows`], the projection the Agents view lists,
//! so the two surfaces can never disagree about what exists or what order it comes in.

use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, Shape, SidebarRow};
use crate::state::State;
use crate::view::agents::GlobalAgentRow;

pub(super) fn global_agents_rows(state: &State) -> Vec<SidebarRow> {
    crate::view::agents::global_agent_rows(state)
        .into_iter()
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

/// A two-line row: status glyph, agent and where it runs, then the status with what it works on.
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
    let mut content = Row::new(row.agent.clone())
        .active(active)
        .glyph(glyph)
        .title_style(super::super::fg_only(&theme.primary))
        .badge_text(place_badge(&row), super::super::fg_only(&theme.muted).dim());
    if let Some(elapsed) = row.elapsed {
        content = content.meta(super::agents::format_age(elapsed), Style::new().fg(color));
    }
    // The status word leads the detail line. Rows from other sessions carry no activity, so for
    // most of them it is all the second line has to say.
    content = content.detail(
        super::agents::row_status_label(&row.status, row.finished_unseen),
        Style::new().fg(color),
    );
    if let Some(what) = row.activity.as_deref().or(row.place.as_deref()) {
        let budget = super::agents::activity_budget(
            state.sidebar_tab_width(&crate::config::SidebarTabId::new("agents")),
        );
        content = content.detail(
            super::row::truncate(what, budget),
            super::super::fg_only(&theme.muted).dim(),
        );
    }
    content
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
            host: host.map(str::to_string),
            session: session.into(),
            status: "blocked".into(),
            finished_unseen: false,
            elapsed: None,
            activity: None,
            place: None,
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

    /// Temporary sessions have generated names; the badge spells them the way every other surface
    /// does rather than leaking the generated id.
    #[test]
    fn a_temporary_session_reads_as_ephemeral() {
        let name = crate::state::ephemeral_session_name();
        assert_eq!(place_badge(&row(None, &name)), "ephemeral");
    }
}
