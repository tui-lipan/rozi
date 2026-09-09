use tui_lipan::prelude::*;

use crate::session::discovery::DiscoveredSession;
use crate::session::remote::RemoteTarget;
use crate::{AppRoot, Msg};

/// Reconcile explicit connection intent with independent metadata workers. Saved hosts stay idle.
pub(crate) fn sync(ctx: &mut Context<AppRoot>) {
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    let held = crate::ops::session::discovery::held_host_targets(&ctx.state);
    if held
        .iter()
        .any(|(target, _)| ctx.state.hosts.get(target).is_none())
    {
        crate::ops::session::seed_host_registry(ctx);
    }
    let mut targets: Vec<_> = ctx
        .state
        .hosts
        .iter()
        .filter(|host| {
            matches!(
                host.probe,
                crate::state::HostProbe::Reached
                    | crate::state::HostProbe::Failed(_)
                    | crate::state::HostProbe::InFlight
            )
        })
        .filter(|host| {
            !ctx.state
                .remote_picker
                .as_ref()
                .is_some_and(|picker| picker.is_connecting(&host.target))
        })
        .map(|host| host.target.clone())
        .collect();
    for (target, _) in held {
        if ctx
            .state
            .remote_picker
            .as_ref()
            .is_some_and(|picker| picker.is_connecting(&target))
        {
            continue;
        }
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    ctx.state
        .host_monitors
        .retain(|monitor| targets.contains(&monitor.target));
    for target in targets {
        if ctx
            .state
            .host_monitors
            .iter()
            .any(|monitor| monitor.target == target)
        {
            continue;
        }
        ctx.state.host_monitor_generation = ctx.state.host_monitor_generation.wrapping_add(1);
        let generation = ctx.state.host_monitor_generation;
        let destination = target.clone();
        let link = link.clone();
        let monitor = crate::session::remote::monitor::start(
            target,
            generation,
            ctx.state.config.remote.clone(),
            move |snapshot| {
                let (rows, agents) = match snapshot {
                    Ok(snapshot) => (Ok(snapshot.rows), snapshot.agents),
                    Err(error) => (Err(error), Vec::new()),
                };
                link.send(Msg::HostMetadata {
                    target: destination.clone(),
                    generation,
                    rows,
                    agents,
                });
            },
        );
        ctx.state.host_monitors.push(monitor);
    }
}

pub(crate) fn apply(
    ctx: &mut Context<AppRoot>,
    target: RemoteTarget,
    generation: u64,
    rows: std::result::Result<Vec<DiscoveredSession>, String>,
    agents: Vec<crate::session::protocol::AgentSummary>,
) -> Update {
    if !ctx
        .state
        .host_monitors
        .iter()
        .any(|monitor| monitor.target == target && monitor.generation == generation)
    {
        return Update::none();
    }
    ctx.state
        .host_live_sessions
        .retain(|row| row.remote_target.as_ref() != Some(&target));
    match rows {
        Ok(rows) => {
            apply_agents(ctx, &target, agents);
            let cached = crate::ops::session::discovery::cached_sessions_for_target(&rows, &target);
            if crate::session::host_sessions_for(&ctx.state.host_session_cache, &target)
                != Some(cached.as_slice())
            {
                crate::session::record_host_sessions(&target, cached.clone());
                crate::session::set_cached_host_sessions(
                    &mut ctx.state.host_session_cache,
                    &target,
                    cached,
                );
            }
            ctx.state.host_live_sessions.extend(rows);
            if let Some(host) = ctx.state.hosts.get_mut(&target) {
                host.probe = crate::state::HostProbe::Reached;
            }
        }
        Err(error) => {
            // The snapshot that would have refreshed these never arrived, and a remembered agent
            // state is worse than none: the sidebar's cached rows already say "last seen", and a
            // `blocked` token beside one would claim a live prompt on a machine nothing can reach.
            ctx.state.host_agents.remove(&target);
            if let Some(host) = ctx.state.hosts.get_mut(&target) {
                host.probe = crate::state::HostProbe::Failed(error);
            }
        }
    }
    ctx.state
        .sidebar
        .sessions
        .retain(|row| row.remote_target.as_ref() != Some(&target));
    for row in crate::ops::session::attached_session_rows(&ctx.state)
        .into_iter()
        .chain(ctx.state.host_live_sessions.clone())
    {
        crate::ops::session::discovery::merge_current_session_row(
            &mut ctx.state.sidebar.sessions,
            row,
        );
    }
    crate::ops::session::discovery::push_cached_known_remote_rows(
        &mut ctx.state.sidebar.sessions,
        &ctx.state.hosts,
        &ctx.state.host_session_cache,
        &[],
    );
    crate::ops::session::discovery::sort_session_rows(&mut ctx.state.sidebar.sessions);
    if let Some(picker) = ctx.state.remote_picker.as_mut()
        && matches!(&picker.mode, crate::state::RemotePickerMode::HostSessions { target: current } if current == &target)
    {
        let rows = ctx
            .state
            .sidebar
            .sessions
            .iter()
            .filter(|row| row.remote_target.as_ref() == Some(&target))
            .cloned()
            .collect();
        picker.replace_sessions(rows);
    }
    Update::full()
}

/// Fold a host monitor's semantic agent snapshot into app state, alerting for anything that
/// reached a state wanting attention while nothing here was watching it.
///
/// The first snapshot for a host only seeds the baseline. A machine that has been sitting on a
/// permission prompt since yesterday is not news that just arrived — the user learned it by
/// connecting, and announcing every standing prompt across a fleet at connect time is exactly how
/// ambient awareness turns into noise.
fn apply_agents(
    ctx: &mut Context<AppRoot>,
    target: &RemoteTarget,
    agents: Vec<crate::session::protocol::AgentSummary>,
) {
    let previous = ctx.state.host_agents.insert(target.clone(), agents.clone());
    let Some(previous) = previous else {
        return;
    };
    if ctx.state.do_not_disturb {
        return;
    }
    let host = target.display_label();
    let mut blocked_any = false;
    let mut finished_any = false;
    for agent in &agents {
        // A session this client holds reports its own pane status over the session protocol, and
        // that path owns the attendance rules. A monitor alert would double every one of them.
        if ctx
            .state
            .attachment_by_identity(&agent.session, Some(target))
            .is_some()
        {
            continue;
        }
        let before = previous
            .iter()
            .find(|earlier| earlier.same_agent(agent))
            .map(|earlier| earlier.state.as_str());
        let edges = crate::update::session::status::agent_status_edges(before, Some(&agent.state));
        crate::pane::pty_events::maybe_notify_host_agent(
            &ctx.state.config,
            &host,
            &agent.session,
            &agent.label,
            edges.became_blocked,
            edges.finished,
        );
        blocked_any |= edges.became_blocked;
        finished_any |= edges.finished;
    }
    // One cue per snapshot, not per agent: a host that reconnects with four finished runs should
    // sound like an event, not like a rattle.
    if blocked_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Blocked);
    }
    if finished_any {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::remote::monitor::Monitor;

    #[test]
    fn metadata_survives_closed_sidebar_and_disconnected_generations_are_ignored() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                let target = RemoteTarget::Alias("monitor-test.invalid".into());
                {
                    let state = backend.state_mut();
                    state.command_link = None;
                    state.sidebar_visible = false;
                    state.hosts.seed(
                        &state.config.remote,
                        std::slice::from_ref(&target),
                        &[],
                        &[],
                    );
                    state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::Reached;
                    state
                        .host_monitors
                        .push(Monitor::dormant(target.clone(), 7));
                }
                let row = DiscoveredSession {
                    name: "dev".into(),
                    host: Some(target.display_label()),
                    remote_target: Some(target.clone()),
                    ephemeral: false,
                    status: crate::session::discovery::DiscoveredSessionStatus::Running {
                        panes: 2,
                        clients: 0,
                        has_layout: false,
                        created_from_profile: None,
                    },
                };
                backend
                    .dispatch(Msg::HostMetadata {
                        agents: Vec::new(),
                        target: target.clone(),
                        generation: 7,
                        rows: Ok(vec![row.clone()]),
                    })
                    .unwrap();
                assert_eq!(backend.state().host_live_sessions, vec![row.clone()]);
                assert!(!backend.state().sidebar_visible);
                backend
                    .dispatch(Msg::HostMetadata {
                        agents: Vec::new(),
                        target: target.clone(),
                        generation: 7,
                        rows: Err("offline".into()),
                    })
                    .unwrap();
                assert!(backend.state().host_live_sessions.is_empty());
                assert!(backend.state().sidebar.sessions.iter().any(|row| matches!(
                    row.status,
                    crate::session::discovery::DiscoveredSessionStatus::LastSeen { panes: 2 }
                )));
                backend.state_mut().host_monitors.clear();
                backend.state_mut().hosts.get_mut(&target).unwrap().probe =
                    crate::state::HostProbe::Idle;
                backend
                    .dispatch(Msg::HostMetadata {
                        agents: Vec::new(),
                        target: target.clone(),
                        generation: 7,
                        rows: Ok(vec![row]),
                    })
                    .unwrap();
                assert!(backend.state().host_live_sessions.is_empty());
                assert_eq!(
                    backend.state().hosts.get(&target).unwrap().probe,
                    crate::state::HostProbe::Idle
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn summary(session: &str, state: &str) -> crate::session::protocol::AgentSummary {
        crate::session::protocol::AgentSummary {
            session: session.into(),
            pane: 1,
            generation: 0,
            row: None,
            agent: "codex".into(),
            label: "Codex".into(),
            state: state.into(),
            changed_at: 0,
        }
    }

    /// The monitor's agent snapshot is state a row reads, not an event stream — so a poll that
    /// repeats what the last one said changes nothing, and a poll that fails must not leave a
    /// remembered `blocked` beside a host nothing can reach.
    #[test]
    fn agent_snapshots_replace_wholesale_and_a_failed_poll_forgets_them() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                let target = RemoteTarget::Alias("agents-test.invalid".into());
                {
                    let state = backend.state_mut();
                    state.command_link = None;
                    state.hosts.seed(
                        &state.config.remote,
                        std::slice::from_ref(&target),
                        &[],
                        &[],
                    );
                    state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::Reached;
                    state
                        .host_monitors
                        .push(Monitor::dormant(target.clone(), 3));
                }
                let metadata = |agents: Vec<crate::session::protocol::AgentSummary>,
                                rows: std::result::Result<Vec<DiscoveredSession>, String>| {
                    Msg::HostMetadata {
                        agents,
                        target: target.clone(),
                        generation: 3,
                        rows,
                    }
                };

                backend
                    .dispatch(metadata(vec![summary("dev", "working")], Ok(Vec::new())))
                    .unwrap();
                assert_eq!(
                    backend.state().host_agents[&target],
                    vec![summary("dev", "working")]
                );

                // A second poll is the whole truth about the host, not a delta onto the first.
                backend
                    .dispatch(metadata(vec![summary("dev", "blocked")], Ok(Vec::new())))
                    .unwrap();
                assert_eq!(
                    backend.state().host_agents[&target],
                    vec![summary("dev", "blocked")]
                );

                backend
                    .dispatch(metadata(Vec::new(), Err("offline".into())))
                    .unwrap();
                assert!(!backend.state().host_agents.contains_key(&target));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
