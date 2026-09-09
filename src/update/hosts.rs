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
            move |rows| {
                link.send(Msg::HostMetadata {
                    target: destination.clone(),
                    generation,
                    rows,
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
                        target: target.clone(),
                        generation: 7,
                        rows: Ok(vec![row.clone()]),
                    })
                    .unwrap();
                assert_eq!(backend.state().host_live_sessions, vec![row.clone()]);
                assert!(!backend.state().sidebar_visible);
                backend
                    .dispatch(Msg::HostMetadata {
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
}
