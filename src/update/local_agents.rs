use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::discovery::LocalAgentSnapshot;
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
pub(super) fn apply(ctx: &mut Context<AppRoot>, snapshot: Option<LocalAgentSnapshot>) -> Update {
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
