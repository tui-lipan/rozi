//! What the global Agents view lists, as a pure projection of `State`.
//!
//! Kept apart from the overlay that draws it because two sources feed it and the rule joining them
//! is the interesting part: the session in front of the user publishes live pane state, while every
//! other machine is known only through its host monitor's semantic summaries. The overlay renders
//! whatever comes out; this is where the boundary between the two is decided.

use crate::state::{AgentLocation, State};

/// One row of the global Agents view: an agent, where it is running, and what it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobalAgentRow {
    pub location: AgentLocation,
    /// The agent's own name, already disambiguated per published row (`Codex`, `Claude #2`).
    pub agent: String,
    /// The machine it is on, `None` for this one.
    pub host: Option<String>,
    pub session: String,
    /// The status exactly as reported. Rendered through the Agents tab's own vocabulary, so the two
    /// surfaces never spell the same state two ways.
    pub status: String,
    /// The agent finished a run since its pane was last looked at. Live panes only: an unseen
    /// finish is a fact about this client's attention, and nothing on another machine can know it.
    pub finished_unseen: bool,
    /// How long the status has held.
    ///
    /// Live panes only, deliberately. A summary's `changed_at` is stamped by the *remote* machine's
    /// wall clock, so subtracting it from this one's would report a duration wrong by however far
    /// the two clocks have drifted — and an age is exactly the kind of field a reader trusts
    /// without checking. No number is better than a plausible wrong one.
    pub age: Option<std::time::Duration>,
}

impl GlobalAgentRow {
    /// How much this row wants attention, lowest first — the Agents tab's own ranking, so blocked
    /// leads, idle trails, and a publisher's custom word sits with the states still in progress.
    pub fn rank(&self) -> u8 {
        crate::view::sidebar::agents::status_rank(Some(&self.status), self.finished_unseen)
    }

    pub fn is_here(&self) -> bool {
        matches!(self.location, AgentLocation::Here { .. })
    }

    /// `Codex · workbox/backend` — who, then where, on one searchable line. The place belongs in
    /// the label rather than the description because it is the half a user types.
    pub fn label(&self) -> String {
        let session = if crate::state::is_ephemeral_session_name(&self.session) {
            "ephemeral"
        } else {
            self.session.as_str()
        };
        match self.host.as_deref() {
            Some(host) => format!("{} · {host}/{session}", self.agent),
            None => format!("{} · {session}", self.agent),
        }
    }

    pub fn description(&self) -> String {
        let status =
            crate::view::sidebar::agents::row_status_label(&self.status, self.finished_unseen);
        match self.age {
            Some(age) => format!(
                "{status} · {}",
                crate::view::sidebar::agents::format_age(age)
            ),
            None => status,
        }
    }
}

/// Every agent this client knows about, the ones most in need of attention first.
///
/// Ordering is by state and then by identity, never by timestamp. The only clock the remote rows
/// carry belongs to the machine that stamped it, so sorting one host's minutes against another's
/// would reshuffle the list whenever two machines disagreed about the time. A stable order is also
/// what lets a monitor poll land while the overlay is open without moving the cursor onto a
/// different agent than the one under it a moment ago.
pub(crate) fn global_agent_rows(state: &State) -> Vec<GlobalAgentRow> {
    let here_target = state.current().remote_target.clone();
    let here_host = here_target
        .as_ref()
        .map(crate::session::remote::RemoteTarget::display_label);
    let here_session = state.current().session_name.clone();
    let mut rows = Vec::new();
    if let Some(session) = here_session.as_deref() {
        for row in crate::view::sidebar::agents::agent_rows(state) {
            rows.push(GlobalAgentRow {
                location: AgentLocation::Here {
                    pane: row.pane_id,
                    row: row.slot.as_ref().map(|slot| slot.id.clone()),
                },
                agent: row.title,
                host: here_host.clone(),
                session: session.to_string(),
                status: row.status.unwrap_or_default(),
                finished_unseen: row.finished_unseen,
                age: row.age,
            });
        }
    }
    for (target, agents) in &state.host_agents {
        let host = target.display_label();
        for agent in agents {
            // The session on screen is already listed from its live panes, which are fresher than a
            // snapshot taken between polls. Two answers for one agent, disagreeing by up to a poll
            // interval and sitting next to each other, is worse than either alone.
            if here_target.as_ref() == Some(target)
                && here_session.as_deref() == Some(&agent.session)
            {
                continue;
            }
            rows.push(GlobalAgentRow {
                location: AgentLocation::Elsewhere {
                    target: target.clone(),
                    session: agent.session.clone(),
                    pane: agent.pane,
                    row: agent.row.clone(),
                },
                agent: agent.label.clone(),
                host: Some(host.clone()),
                session: agent.session.clone(),
                status: agent.state.clone(),
                finished_unseen: false,
                age: None,
            });
        }
    }
    rows.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then_with(|| b.is_here().cmp(&a.is_here()))
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.location.pane().cmp(&b.location.pane()))
            .then_with(|| a.location.row().cmp(&b.location.row()))
    });
    rows
}

/// The row the Agents view opens on: whichever one most wants attention.
pub(crate) fn first_agent_location(state: &State) -> Option<AgentLocation> {
    global_agent_rows(state)
        .into_iter()
        .next()
        .map(|row| row.location)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::AgentSummary;
    use crate::session::remote::RemoteTarget;

    fn summary(session: &str, pane: crate::state::PaneId, state: &str) -> AgentSummary {
        AgentSummary {
            session: session.into(),
            pane,
            generation: 0,
            row: None,
            agent: "codex".into(),
            label: "Codex".into(),
            state: state.into(),
            changed_at: 0,
        }
    }

    fn state_with_hosts(hosts: &[(&str, Vec<AgentSummary>)]) -> State {
        let mut state = State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        );
        for (alias, agents) in hosts {
            state
                .host_agents
                .insert(RemoteTarget::Alias((*alias).into()), agents.clone());
        }
        state
    }

    /// The whole point of the view: the thing that wants an answer is the thing under the cursor
    /// when it opens, whichever machine it happens to be on.
    #[test]
    fn rows_lead_with_what_wants_attention_across_every_host() {
        let state = state_with_hosts(&[
            ("alpha", vec![summary("dev", 1, "working")]),
            (
                "beta",
                vec![summary("api", 2, "idle"), summary("api", 3, "blocked")],
            ),
        ]);
        let rows = global_agent_rows(&state);
        let order: Vec<_> = rows
            .iter()
            .map(|row| (row.host.as_deref().unwrap(), row.status.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![("beta", "blocked"), ("alpha", "working"), ("beta", "idle")]
        );
        assert_eq!(
            first_agent_location(&state),
            Some(AgentLocation::Elsewhere {
                target: RemoteTarget::Alias("beta".into()),
                session: "api".into(),
                pane: 3,
                row: None,
            })
        );
    }

    /// A row has to say which machine it is on before it says anything else about itself — the
    /// label is what a user searches, and "Codex" alone is the one thing they already know.
    #[test]
    fn a_remote_row_names_its_host_and_session() {
        let state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "blocked")])]);
        let rows = global_agent_rows(&state);
        assert_eq!(rows[0].label(), "Codex · workbox/backend");
        assert_eq!(rows[0].description(), "Blocked");
    }

    /// An age is the field a reader trusts without checking, and the only timestamp a summary
    /// carries was stamped by another machine's clock. See [`GlobalAgentRow::age`].
    #[test]
    fn a_remote_row_claims_no_age() {
        let state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "working")])]);
        assert_eq!(global_agent_rows(&state)[0].age, None);
    }

    /// The session on screen reports itself through its live panes. A monitor snapshot of the same
    /// session is the staler of the two answers, and listing both would show them side by side.
    #[test]
    fn the_session_on_screen_is_not_listed_twice() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "blocked")])]);
        assert_eq!(global_agent_rows(&state).len(), 1);

        state.current_mut().session_name = Some("backend".into());
        state.current_mut().remote_target = Some(target);
        // No panes in this bare state, so the live half contributes nothing and the snapshot half
        // is suppressed: the row is gone rather than duplicated.
        assert!(global_agent_rows(&state).is_empty());
    }

    /// A different session on the same host is a different agent, and stays listed.
    #[test]
    fn another_session_on_the_attached_host_stays_listed() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state_with_hosts(&[(
            "workbox",
            vec![summary("backend", 1, "blocked"), summary("web", 2, "idle")],
        )]);
        state.current_mut().session_name = Some("backend".into());
        state.current_mut().remote_target = Some(target);
        let rows = global_agent_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session, "web");
    }
}
