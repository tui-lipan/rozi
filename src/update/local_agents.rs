use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::discovery::{LocalAgentSnapshot, retain_busy_agent_summaries};
use crate::session::protocol::AgentSummary;
use crate::state::State;

fn alert_edges_for(
    state: &State,
    previous: &[AgentSummary],
    agent: &AgentSummary,
) -> Option<crate::update::session::status::AgentEdges> {
    // A held attachment gets live pane events, including while parked. Its monitor copy must not
    // raise a second alert for the same transition.
    if state.attachment_by_identity(&agent.session, None).is_some() {
        return None;
    }
    let before = previous
        .iter()
        .find(|earlier| earlier.same_agent(agent))
        .map(|earlier| earlier.state.as_str());
    Some(crate::update::session::status::agent_status_edges(
        before,
        Some(&agent.state),
    ))
}

/// Replace one local metadata snapshot. A failed sweep clears live claims; the next successful
/// sweep becomes a quiet baseline, just as the first remote host snapshot does.
pub(super) fn apply(
    ctx: &mut Context<AppRoot>,
    mut snapshot: Option<LocalAgentSnapshot>,
) -> Update {
    if let (Some(current), Some(previous)) =
        (snapshot.as_mut(), ctx.state.local_agent_snapshot.as_ref())
    {
        retain_busy_agent_summaries(&current.sessions, &mut current.agents, &previous.agents);
    }
    if ctx.state.local_agent_snapshot == snapshot {
        return Update::none();
    }
    let previous = std::mem::replace(&mut ctx.state.local_agent_snapshot, snapshot);
    let (Some(previous), Some(current)) = (previous, ctx.state.local_agent_snapshot.as_ref())
    else {
        return Update::full();
    };
    if ctx.state.do_not_disturb {
        return Update::full();
    }
    let mut blocked_any = false;
    let mut finished_any = false;
    for agent in &current.agents {
        let Some(edges) = alert_edges_for(&ctx.state, &previous.agents, agent) else {
            continue;
        };
        crate::pane::pty_events::maybe_notify_session_agent(
            &ctx.state.config,
            None,
            &agent.session,
            &agent.label,
            edges.became_blocked,
            edges.finished,
        );
        blocked_any |= edges.became_blocked;
        finished_any |= edges.finished;
    }
    if blocked_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Blocked);
    }
    if finished_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Done);
    }
    Update::full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};

    fn summary(session: &str, state: &str) -> AgentSummary {
        AgentSummary {
            session: session.into(),
            pane: 3,
            generation: 1,
            row: None,
            agent: "codex".into(),
            label: "Codex".into(),
            state: state.into(),
            changed_at: 1,
        }
    }

    fn busy_session(name: &str) -> DiscoveredSession {
        DiscoveredSession {
            name: name.into(),
            status: DiscoveredSessionStatus::Busy,
            origin: Default::default(),
            ephemeral: false,
            host: None,
            remote_target: None,
        }
    }

    #[test]
    fn busy_probe_keeps_agent_visible_and_preserves_alert_baseline() {
        for (before, expect_blocked_edge) in [("blocked", false), ("working", true)] {
            let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    sessions: Vec::new(),
                    agents: vec![summary("dev", before)],
                })))
                .unwrap();
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    sessions: vec![busy_session("dev")],
                    agents: Vec::new(),
                })))
                .unwrap();

            let during_busy = backend.state().local_agent_snapshot.as_ref().unwrap();
            assert_eq!(during_busy.agents, vec![summary("dev", before)]);
            assert!(
                crate::view::agents::global_agent_rows(backend.state())
                    .iter()
                    .any(|row| row.session == "dev" && row.status == before)
            );
            let edge = alert_edges_for(
                backend.state(),
                &during_busy.agents,
                &summary("dev", "blocked"),
            )
            .unwrap();
            assert_eq!(edge.became_blocked, expect_blocked_edge);

            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    sessions: Vec::new(),
                    agents: vec![summary("dev", "blocked")],
                })))
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .local_agent_snapshot
                    .as_ref()
                    .unwrap()
                    .agents,
                vec![summary("dev", "blocked")]
            );
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    sessions: Vec::new(),
                    agents: Vec::new(),
                })))
                .unwrap();
            assert!(
                backend
                    .state()
                    .local_agent_snapshot
                    .as_ref()
                    .unwrap()
                    .agents
                    .is_empty()
            );
        }
    }

    #[test]
    fn local_monitor_alerts_only_for_unattached_session_transitions() {
        let mut state = State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        );
        let working = summary("dev", "working");
        let blocked = summary("dev", "blocked");
        assert!(
            alert_edges_for(&state, std::slice::from_ref(&working), &blocked)
                .unwrap()
                .became_blocked
        );

        state.current_mut().session_name = Some("dev".into());
        assert!(alert_edges_for(&state, std::slice::from_ref(&working), &blocked).is_none());
        state.current_mut().session_name = Some("other".into());
        let parked = crate::state::Attachment {
            session_name: Some("dev".into()),
            ..Default::default()
        };
        state.background.insert(7, parked);
        assert!(alert_edges_for(&state, &[working], &blocked).is_none());
    }

    #[test]
    fn local_monitor_snapshot_replaces_stale_state_and_recovers() {
        let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
        let snapshot = |state| LocalAgentSnapshot {
            sessions: Vec::new(),
            agents: vec![summary("dev", state)],
        };
        backend
            .dispatch(crate::Msg::LocalAgentMetadata(Some(snapshot("working"))))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .local_agent_snapshot
                .as_ref()
                .unwrap()
                .agents[0]
                .state,
            "working"
        );
        backend
            .dispatch(crate::Msg::LocalAgentMetadata(None))
            .unwrap();
        assert!(backend.state().local_agent_snapshot.is_none());
        backend
            .dispatch(crate::Msg::LocalAgentMetadata(Some(snapshot("blocked"))))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .local_agent_snapshot
                .as_ref()
                .unwrap()
                .agents[0]
                .state,
            "blocked"
        );
    }
}
