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
    let before = previous.iter().find(|earlier| earlier.same_agent(agent));
    Some(crate::update::session::status::summary_edges(before, agent))
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
        crate::session::discovery::retain_busy_project_summaries(
            &current.sessions,
            &mut current.projects,
            &previous.projects,
        );
    }
    if ctx.state.local_agent_snapshot == snapshot {
        // Each sweep also collects what other paths left behind - an attachment closed, a pane
        // gone - so nothing keeps an agent's attention alive once no source reports it.
        ctx.state.prune_agent_attention();
        return Update::none();
    }
    let previous = std::mem::replace(&mut ctx.state.local_agent_snapshot, snapshot);
    if let Some(current) = &ctx.state.local_agent_snapshot {
        let sighting = if previous.is_some() {
            crate::state::FirstSighting::New
        } else {
            crate::state::FirstSighting::Baseline
        };
        ctx.state
            .agent_attention
            .observe_summaries(&current.agents, sighting);
    }
    ctx.state.prune_agent_attention();
    let (Some(previous), Some(current)) = (previous, ctx.state.local_agent_snapshot.as_ref())
    else {
        return Update::full();
    };
    if ctx.state.do_not_disturb {
        return Update::full();
    }
    let mut blocked_any = false;
    let mut finished_any = false;
    let mut failed_any = false;
    for agent in &current.agents {
        let Some(mut edges) = alert_edges_for(&ctx.state, &previous.agents, agent) else {
            continue;
        };
        edges.claim(&agent.reference.pane);
        crate::pane::pty_events::maybe_notify_session_agent(
            &ctx.state.config,
            None,
            &agent.session,
            &agent.label,
            &edges,
        );
        blocked_any |= edges.became_blocked;
        finished_any |= edges.finished;
        failed_any |= edges.failed;
    }
    if blocked_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Blocked);
    }
    if failed_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Error);
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
            reference: crate::session::protocol::AgentRef::for_test(3, 1, None),
            session: session.into(),
            agent: "codex".into(),
            label: "Codex".into(),
            state: crate::session::protocol::AgentState::from_status(state),
            reason: None,
            task: None,
            completions: 0,
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

    fn alert_backend() -> tui_lipan::TestBackend<AppRoot> {
        let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
        let state = backend.state_mut();
        state.config.notifications.enabled = false;
        state.config.sounds.enabled = true;
        state.config.sounds.player = Some("/nonexistent-rozi-test-sound-player".into());
        backend
    }

    fn monitor_transition(
        backend: &mut tui_lipan::TestBackend<AppRoot>,
        reference: &crate::session::protocol::AgentRef,
    ) {
        for status in ["working", "blocked"] {
            let mut agent = summary("unattached", status);
            agent.reference = reference.clone();
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    agents: vec![agent],
                    sessions: Vec::new(),
                    projects: Vec::new(),
                })))
                .unwrap();
        }
    }

    #[test]
    fn windows_share_alert_claims_and_silenced_monitors_leave_them_available() {
        crate::test_support::on_large_stack(|| {
            for silenced in [false, true] {
                let mut reference = crate::session::protocol::AgentRef::for_test(7, 2, None);
                reference.pane.session_instance =
                    crate::session::protocol::SessionInstanceId::generate();
                let mut first = alert_backend();
                first.state_mut().do_not_disturb = silenced;
                monitor_transition(&mut first, &reference);
                assert_eq!(
                    first
                        .state()
                        .sound_cues
                        .contains_key(&crate::platform::sound::Cue::Blocked),
                    !silenced
                );
                let mut second = alert_backend();
                monitor_transition(&mut second, &reference);
                assert_eq!(
                    second
                        .state()
                        .sound_cues
                        .contains_key(&crate::platform::sound::Cue::Blocked),
                    silenced
                );
            }
        });
    }

    #[test]
    fn attended_controller_claims_and_a_follower_does_not() {
        use crate::session::protocol::{
            AgentIdentity, AgentRecord, DetectedAgent, DetectedAgentState, PaneRuntimeState,
        };
        crate::test_support::on_large_stack(|| {
            for controller in [true, false] {
                let mut reference = crate::session::protocol::AgentRef::for_test(7, 2, None);
                reference.pane.session_instance =
                    crate::session::protocol::SessionInstanceId::generate();
                let mut attached = alert_backend();
                {
                    let state = attached.state_mut();
                    let mut pane =
                        crate::state::Pane::new(7, 100, tui_lipan::prelude::FloatRect::default());
                    pane.pty_generation = 2;
                    state.current_mut().workspaces[0].panes = vec![pane];
                    state.current_mut().focused_pane = Some(7);
                    state.window_focused = true;
                    let mut shared = crate::state::SharedSessionState::new(1);
                    shared.controller = Some(if controller { 1 } else { 2 });
                    state.current_mut().shared = Some(shared);
                }
                for (sequence, agent_state) in [
                    (1, DetectedAgentState::Working),
                    (2, DetectedAgentState::Blocked),
                ] {
                    attached
                        .dispatch(crate::Msg::SessionPaneRuntimeChanged {
                            epoch: attached.state().runtime_epoch,
                            pane_id: 7,
                            generation: 2,
                            local: false,
                            agents: vec![AgentRecord::from(reference.clone())],
                            state: PaneRuntimeState {
                                sequence,
                                detected_agent: Some(DetectedAgent {
                                    agent: AgentIdentity::new("codex", "Codex").into(),
                                    state: agent_state,
                                }),
                                ..Default::default()
                            },
                        })
                        .unwrap();
                }
                assert!(
                    attached.state().sound_cues.is_empty(),
                    "attended panes stay silent"
                );
                let mut monitor = alert_backend();
                monitor_transition(&mut monitor, &reference);
                assert_eq!(
                    monitor
                        .state()
                        .sound_cues
                        .contains_key(&crate::platform::sound::Cue::Blocked),
                    !controller
                );
            }
        });
    }

    #[test]
    fn busy_probe_keeps_agent_visible_and_preserves_alert_baseline() {
        // Renders the whole app, a view tree deeper than the default test stack holds.
        crate::test_support::on_large_stack(
            busy_probe_keeps_agent_visible_and_preserves_alert_baseline_body,
        );
    }

    fn busy_probe_keeps_agent_visible_and_preserves_alert_baseline_body() {
        for (before, expect_blocked_edge) in [("blocked", false), ("working", true)] {
            let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    projects: Vec::new(),
                    sessions: Vec::new(),
                    agents: vec![summary("dev", before)],
                })))
                .unwrap();
            backend
                .dispatch(crate::Msg::LocalAgentMetadata(Some(LocalAgentSnapshot {
                    projects: Vec::new(),
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
                    projects: Vec::new(),
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
                    projects: Vec::new(),
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
        // Renders the whole app, a view tree deeper than the default test stack holds.
        crate::test_support::on_large_stack(
            local_monitor_snapshot_replaces_stale_state_and_recovers_body,
        );
    }

    fn local_monitor_snapshot_replaces_stale_state_and_recovers_body() {
        let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
        let snapshot = |state| LocalAgentSnapshot {
            projects: Vec::new(),
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
            crate::session::protocol::AgentState::Working
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
            crate::session::protocol::AgentState::Blocked
        );
    }
}
