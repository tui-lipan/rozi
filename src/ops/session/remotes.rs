use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::remote::RemoteTarget;
use crate::state::{RemotePickerMode, RemotePickerState, RemoteSessionIdentity};

fn cached_rows_for_target(
    cache: &crate::session::HostSessionCache,
    target: &RemoteTarget,
) -> Vec<crate::session::discovery::DiscoveredSession> {
    let label = target.display_label();
    crate::session::host_sessions_for(cache, target)
        .unwrap_or_default()
        .iter()
        .map(|session| crate::session::discovery::DiscoveredSession {
            name: session.name.clone(),
            ephemeral: session.ephemeral,
            host: Some(label.clone()),
            remote_target: Some(target.clone()),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: session.panes,
                clients: 0,
                has_layout: false,
                created_from_profile: None,
            },
        })
        .collect()
}

fn host_discovery_command(
    epoch: u64,
    target: RemoteTarget,
    remote_config: crate::config::RemoteConfig,
) -> Command {
    crate::ops::session::discovery::note_remote_probe_request(&target);
    Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::spawn(move || {
            let rows = crate::ops::session::discover_remote_host_sessions(
                &target,
                &remote_config,
            )
            .map_err(|error| error.to_string());
            link.send(crate::Msg::RemoteHostSessionsDiscovered {
                epoch,
                target,
                rows,
            });
        });
    })
}

fn install_remote_hosts(
    ctx: &mut Context<AppRoot>,
    query: String,
    selected: Option<RemoteTarget>,
) {
    crate::ops::session::seed_host_registry(ctx);
    let selected = selected
        .filter(|target| ctx.state.hosts.get(target).is_some())
        .or_else(|| ctx.state.hosts.iter().next().map(|entry| entry.target.clone()));
    let mut picker = RemotePickerState::new(selected);
    let cursor = query.len();
    picker.host_input.set_text(query);
    picker.host_input.set_cursor(cursor);
    picker.host_input.set_anchor(None);
    ctx.state.remote_picker = Some(picker);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_remote_picker_focus(ctx);
}

/// Open the local known-host registry. This deliberately schedules no discovery; contacting a
/// machine is an explicit consequence of activating its row.
pub(crate) fn open_remote_hosts(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.overlay_return = crate::ops::overlay_return::picker_origin(&ctx.state);
    install_remote_hosts(ctx, String::new(), None);
    Update::full()
}

pub(crate) fn restore_remote_hosts(
    ctx: &mut Context<AppRoot>,
    query: String,
    selected: Option<RemoteTarget>,
) -> Update {
    install_remote_hosts(ctx, query, selected);
    Update::full()
}

pub(crate) fn restore_remote_host_sessions(
    ctx: &mut Context<AppRoot>,
    target: RemoteTarget,
    query: String,
    selected: Option<RemoteSessionIdentity>,
) -> Update {
    install_remote_hosts(ctx, String::new(), Some(target.clone()));
    let rows = cached_rows_for_target(&ctx.state.host_session_cache, &target);
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target);
        let cursor = query.len();
        picker.session_input.set_text(query);
        picker.session_input.set_cursor(cursor);
        picker.session_input.set_anchor(None);
        picker.selected_session = selected;
        picker.replace_sessions(rows);
    }
    Update::full()
}

pub(crate) fn close_remote_picker(ctx: &mut Context<AppRoot>) -> Update {
    let mode = ctx
        .state
        .remote_picker
        .as_ref()
        .map(|picker| picker.mode.clone());
    match mode {
        Some(RemotePickerMode::HostSessions { .. }) => {
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.return_to_hosts();
            }
            crate::ops::focus::request_remote_picker_focus(ctx);
            Update::full()
        }
        Some(RemotePickerMode::Hosts) => {
            ctx.state.remote_picker = None;
            ctx.state.commands_dirty = true;
            crate::ops::overlay_return::finish(ctx)
        }
        None => Update::none(),
    }
}

pub(crate) fn host_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.host_input.set_text(query);
        picker.pending_forget = None;
    }
    Update::full()
}

pub(crate) fn host_selected(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        if picker.selected_host.as_ref() != Some(&target) {
            picker.pending_forget = None;
        }
        picker.selected_host = Some(target);
    }
    Update::full()
}

pub(crate) fn activate_host(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if ctx.state.hosts.get(&target).is_none() {
        return Update::none();
    }
    let rows = cached_rows_for_target(&ctx.state.host_session_cache, &target);
    let epoch = if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target.clone());
        picker.replace_sessions(rows);
        picker.probe_epoch = picker.probe_epoch.wrapping_add(1);
        picker.probe_epoch
    } else {
        return Update::none();
    };
    if let Some(entry) = ctx.state.hosts.get_mut(&target) {
        entry.probe = crate::state::HostProbe::InFlight;
    }
    crate::ops::focus::request_remote_picker_focus(ctx);
    Update::with_command(host_discovery_command(
        epoch,
        target,
        ctx.state.config.remote.clone(),
    ))
}

pub(crate) fn apply_host_discovery(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    target: RemoteTarget,
    rows: Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
) -> Update {
    let current = ctx.state.remote_picker.as_ref().is_some_and(|picker| {
        picker.probe_epoch == epoch
            && matches!(
                &picker.mode,
                RemotePickerMode::HostSessions { target: active } if active == &target
            )
    });
    if !current {
        return Update::none();
    }
    match rows {
        Ok(rows) => {
            let cached = crate::ops::session::discovery::cached_sessions_for_target(
                &rows,
                &target,
            );
            crate::session::record_host_sessions(&target, cached.clone());
            crate::session::set_cached_host_sessions(
                &mut ctx.state.host_session_cache,
                &target,
                cached,
            );
            crate::session::record_recent_remote(&target);
            crate::ops::session::seed_host_registry(ctx);
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Reached;
            }
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.replace_sessions(rows);
            }
        }
        Err(error) => {
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Failed(error);
            }
        }
    }
    Update::full()
}

pub(crate) fn session_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.session_input.set_text(query);
        picker.pending_kill = None;
        picker.pending_restart = None;
    }
    Update::full()
}

pub(crate) fn session_selected(
    ctx: &mut Context<AppRoot>,
    identity: RemoteSessionIdentity,
) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        if picker.selected_session.as_ref() != Some(&identity) {
            picker.pending_kill = None;
            picker.pending_restart = None;
        }
        picker.selected_session = Some(identity);
    }
    Update::full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_host_activation_records_a_probe_request() {
        let target = RemoteTarget::Alias("workbox".into());
        let _ = crate::ops::session::discovery::take_remote_probe_requests();
        let _picker = RemotePickerState::new(Some(target.clone()));
        assert!(crate::ops::session::discovery::take_remote_probe_requests().is_empty());

        let _command = host_discovery_command(
            1,
            target.clone(),
            crate::config::RemoteConfig::default(),
        );
        assert_eq!(
            crate::ops::session::discovery::take_remote_probe_requests(),
            vec![target]
        );
    }

    #[test]
    fn cached_rows_keep_the_exact_target_identity() {
        let target = RemoteTarget::Url {
            user: Some("adam".into()),
            host: "workbox".into(),
            port: Some(2222),
        };
        let mut cache = crate::session::HostSessionCache::new();
        crate::session::set_cached_host_sessions(
            &mut cache,
            &target,
            vec![crate::session::CachedHostSession {
                name: "dev".into(),
                ephemeral: false,
                panes: 3,
            }],
        );
        let rows = cached_rows_for_target(&cache, &target);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host.as_deref(), Some("adam@workbox:2222"));
        assert_eq!(rows[0].remote_target.as_ref(), Some(&target));
    }
}
