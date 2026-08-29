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

pub(crate) fn host_can_forget(
    state: &crate::state::State,
    target: &RemoteTarget,
) -> bool {
    let Some(entry) = state.hosts.get(target) else {
        return false;
    };
    entry.origin == crate::state::HostOrigin::Recent
        && !matches!(entry.probe, crate::state::HostProbe::InFlight)
        && std::iter::once(state.current())
            .chain(state.background.values())
            .all(|attachment| {
                attachment.remote_target.as_ref() != Some(target)
                    || !matches!(
                        attachment.connection,
                        crate::state::ConnectionState::Connected
                            | crate::state::ConnectionState::Connecting
                            | crate::state::ConnectionState::Reconnecting
                            | crate::state::ConnectionState::AuthRequired
                    )
            })
}

pub(crate) fn forget_host(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = ctx
        .state
        .remote_picker
        .as_ref()
        .and_then(|picker| picker.selected_host.clone())
    else {
        return Update::none();
    };
    if !host_can_forget(&ctx.state, &target) {
        return Update::none();
    }
    let armed = ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_forget.as_ref() == Some(&target));
    if !armed {
        if let Some(picker) = ctx.state.remote_picker.as_mut() {
            picker.pending_forget = Some(target);
        }
        return crate::ops::confirm::arm(ctx);
    }
    crate::session::forget_recent_remote(&target);
    crate::session::forget_host_sessions(&target);
    crate::session::remove_cached_host_sessions(&mut ctx.state.host_session_cache, &target);
    crate::ops::session::seed_host_registry(ctx);
    let selected = ctx.state.hosts.iter().next().map(|entry| entry.target.clone());
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.selected_host = selected;
        picker.pending_forget = None;
    }
    ctx.state.sidebar.invalidate_sessions();
    Update::full()
}

pub(crate) fn activate_host(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    let rows = cached_rows_for_target(&ctx.state.host_session_cache, &target);
    let epoch = if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target.clone());
        picker.replace_sessions(rows);
        picker.probe_epoch = picker.probe_epoch.wrapping_add(1);
        picker.host_probe = crate::state::HostProbe::InFlight;
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
                picker.host_probe = crate::state::HostProbe::Reached;
                picker.replace_sessions(rows);
            }
        }
        Err(error) => {
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.host_probe = crate::state::HostProbe::Failed(error.clone());
            }
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Failed(error);
            }
        }
    }
    Update::full()
}

pub(crate) fn open_new_host_prompt(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.remote_picker.as_mut() else {
        return Update::none();
    };
    if !matches!(picker.mode, RemotePickerMode::Hosts) {
        return Update::none();
    }
    let initial = picker.host_input.text().trim().to_string();
    picker.target_prompt = Some(crate::state::RemoteTargetPromptState::new(initial));
    picker.pending_forget = None;
    crate::ops::focus::request_remote_target_focus(ctx);
    Update::full()
}

pub(crate) fn open_new_host_flow(ctx: &mut Context<AppRoot>) -> Update {
    let _ = open_remote_hosts(ctx);
    open_new_host_prompt(ctx)
}

pub(crate) fn target_prompt_changed(
    ctx: &mut Context<AppRoot>,
    event: InputEvent,
) -> Update {
    if let Some(prompt) = ctx
        .state
        .remote_picker
        .as_mut()
        .and_then(|picker| picker.target_prompt.as_mut())
    {
        event.apply_to(&mut prompt.input);
        prompt.error = None;
    }
    crate::ops::focus::request_remote_target_focus(ctx);
    Update::full()
}

pub(crate) fn close_target_prompt(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.target_prompt = None;
    }
    crate::ops::focus::request_remote_picker_focus(ctx);
    Update::full()
}

pub(crate) fn submit_remote_target(ctx: &mut Context<AppRoot>) -> Update {
    let Some(raw) = ctx
        .state
        .remote_picker
        .as_ref()
        .and_then(|picker| picker.target_prompt.as_ref())
        .map(|prompt| prompt.input.text().trim().to_string())
    else {
        return Update::none();
    };
    let target = match crate::session::remote::parse_remote_target(&raw) {
        Ok(target) => target,
        Err(error) => {
            if let Some(prompt) = ctx
                .state
                .remote_picker
                .as_mut()
                .and_then(|picker| picker.target_prompt.as_mut())
            {
                prompt.error = Some(error);
            }
            crate::ops::focus::request_remote_target_focus(ctx);
            return Update::full();
        }
    };
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.target_prompt = None;
    }
    activate_host(ctx, target)
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

fn selected_session(
    state: &crate::state::State,
) -> Option<crate::session::discovery::DiscoveredSession> {
    let picker = state.remote_picker.as_ref()?;
    let selected = picker.selected_session.as_ref()?;
    picker
        .sessions
        .iter()
        .find(|session| RemoteSessionIdentity::of(session).as_ref() == Some(selected))
        .cloned()
}

fn selected_target(state: &crate::state::State) -> Option<RemoteTarget> {
    match &state.remote_picker.as_ref()?.mode {
        RemotePickerMode::HostSessions { target } => Some(target.clone()),
        RemotePickerMode::Hosts => None,
    }
}

pub(crate) fn activate_session(
    ctx: &mut Context<AppRoot>,
    identity: RemoteSessionIdentity,
) -> Update {
    let Some(session) = ctx
        .state
        .remote_picker
        .as_ref()
        .and_then(|picker| {
            picker.sessions.iter().find(|session| {
                RemoteSessionIdentity::of(session).as_ref() == Some(&identity)
            })
        })
        .cloned()
    else {
        return Update::none();
    };
    crate::ops::session::activate_discovered_session(ctx, session)
}

pub(crate) fn create_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = selected_target(&ctx.state) else {
        return Update::none();
    };
    crate::ops::session::open_create_session_on_host(ctx, target)
}

pub(crate) fn open_ephemeral(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = selected_target(&ctx.state) else {
        return Update::none();
    };
    crate::ops::session::open_ephemeral_session_on_host(ctx, target)
}

pub(crate) fn kill_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(session) = selected_session(&ctx.state) else {
        return Update::none();
    };
    let Some(identity) = RemoteSessionIdentity::of(&session) else {
        return Update::none();
    };
    let armed = ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_kill.as_ref() == Some(&identity));
    if !armed {
        if let Some(picker) = ctx.state.remote_picker.as_mut() {
            picker.pending_kill = Some(identity);
            picker.pending_restart = None;
        }
        return crate::ops::confirm::arm(ctx);
    }
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.pending_kill = None;
        picker.pending_restart = None;
    }
    let update = crate::ops::session::kill_discovered_session(ctx, session.clone());
    let removed = session.remote_target.as_ref().is_some_and(|target| {
        crate::session::host_sessions_for(&ctx.state.host_session_cache, target)
            .is_none_or(|sessions| sessions.iter().all(|cached| cached.name != session.name))
    });
    if removed
        && let Some(picker) = ctx.state.remote_picker.as_mut()
    {
        let rows = picker
            .sessions
            .iter()
            .filter(|listed| RemoteSessionIdentity::of(listed).as_ref() != Some(&identity))
            .cloned()
            .collect();
        picker.replace_sessions(rows);
    }
    update
}

pub(crate) fn restart_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(session) = selected_session(&ctx.state) else {
        return Update::none();
    };
    if !crate::ops::session::session_row_can_restart(&session) {
        return Update::none();
    }
    let Some(identity) = RemoteSessionIdentity::of(&session) else {
        return Update::none();
    };
    let armed = ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_restart.as_ref() == Some(&identity));
    if !armed {
        if let Some(picker) = ctx.state.remote_picker.as_mut() {
            picker.pending_restart = Some(identity);
            picker.pending_kill = None;
        }
        return crate::ops::confirm::arm(ctx);
    }
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.pending_restart = None;
        picker.pending_kill = None;
    }
    super::lifecycle::restart_discovered_session(ctx, session)
}

pub(crate) fn disconnect_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(session) = selected_session(&ctx.state) else {
        return Update::none();
    };
    crate::ops::session::disconnect_discovered_attachment(ctx, session)
}

pub(crate) fn disconnect_selected_host(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = selected_target(&ctx.state) else {
        return Update::none();
    };
    crate::ops::session::disconnect_host(ctx, &target)
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
    fn main_session_discovery_records_no_remote_probe_request() {
        let _ = crate::ops::session::discovery::take_remote_probe_requests();
        let _ = crate::ops::session::discover_picker_sessions(None);
        assert!(crate::ops::session::discovery::take_remote_probe_requests().is_empty());
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

    #[test]
    fn picker_target_wins_over_the_current_attachment_target() {
        let current = RemoteTarget::Alias("current".into());
        let selected = RemoteTarget::Alias("selected".into());
        let mut state = crate::state::State::new(
            crate::config::Config::default(),
            tui_lipan::prelude::Theme::default(),
        );
        state.current_mut().remote_target = Some(current);
        let mut picker = RemotePickerState::new(Some(selected.clone()));
        picker.enter_host_sessions(selected.clone());
        state.remote_picker = Some(picker);
        assert_eq!(selected_target(&state), Some(selected));
    }

    #[test]
    fn only_offline_recent_hosts_are_forgettable() {
        let recent = RemoteTarget::Alias("recent".into());
        let configured = RemoteTarget::Alias("configured".into());
        let mut config = crate::config::Config::default();
        config
            .remote
            .hosts
            .insert("configured".into(), crate::config::RemoteHostConfig::default());
        let mut state = crate::state::State::new(
            config,
            tui_lipan::prelude::Theme::default(),
        );
        state.hosts.seed(
            &state.config.remote,
            std::slice::from_ref(&recent),
            &[],
        );
        assert!(host_can_forget(&state, &recent));
        assert!(!host_can_forget(&state, &configured));

        state.current_mut().remote_target = Some(recent.clone());
        state.current_mut().connection = crate::state::ConnectionState::Connected;
        assert!(!host_can_forget(&state, &recent));
        state.current_mut().connection = crate::state::ConnectionState::Disconnected;
        assert!(host_can_forget(&state, &recent));

        state.hosts.get_mut(&recent).unwrap().probe = crate::state::HostProbe::InFlight;
        assert!(!host_can_forget(&state, &recent));
    }
}
