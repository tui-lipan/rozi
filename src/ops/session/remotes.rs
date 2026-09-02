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
        .filter(|session| !session.ephemeral)
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

fn immediate_rows_for_target(
    state: &crate::state::State,
    target: &RemoteTarget,
) -> Vec<crate::session::discovery::DiscoveredSession> {
    let mut rows = cached_rows_for_target(&state.host_session_cache, target);
    for attached in crate::ops::session::attached_session_rows(state)
        .into_iter()
        .filter(|session| session.remote_target.as_ref() == Some(target))
    {
        crate::ops::session::discovery::merge_current_session_row(&mut rows, attached);
    }
    rows
}

fn host_discovery_command(
    epoch: u64,
    target: RemoteTarget,
    remote_config: crate::config::RemoteConfig,
) -> Command {
    crate::ops::session::discovery::note_remote_probe_request(&target);
    Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::spawn(move || {
            let rows = crate::ops::session::discover_remote_host_sessions(&target, &remote_config)
                .map_err(|error| error.to_string());
            link.send(crate::Msg::RemoteHostSessionsDiscovered {
                epoch,
                target,
                rows,
            });
        });
    })
}

fn install_remote_hosts(ctx: &mut Context<AppRoot>, query: String, selected: Option<RemoteTarget>) {
    crate::ops::session::seed_host_registry(ctx);
    let selected = selected
        .filter(|target| ctx.state.hosts.get(target).is_some())
        .or_else(|| {
            ctx.state
                .hosts
                .iter()
                .next()
                .map(|entry| entry.target.clone())
        });
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

fn abandon_remote_probe(state: &mut crate::state::State) {
    let target =
        state
            .remote_picker
            .as_ref()
            .and_then(|picker| match (&picker.mode, &picker.host_probe) {
                (RemotePickerMode::HostSessions { target }, crate::state::HostProbe::InFlight) => {
                    Some(target.clone())
                }
                (RemotePickerMode::Hosts, crate::state::HostProbe::InFlight) => {
                    picker.selected_host.clone()
                }
                _ => None,
            });
    if let Some(target) = target
        && matches!(
            state.hosts.get(&target).map(|entry| &entry.probe),
            Some(crate::state::HostProbe::InFlight)
        )
        && let Some(entry) = state.hosts.get_mut(&target)
    {
        entry.probe = crate::state::HostProbe::Idle;
    }
    if let Some(picker) = state.remote_picker.as_mut() {
        picker.host_probe = crate::state::HostProbe::Idle;
    }
}

pub(crate) fn dismiss_remote_picker(state: &mut crate::state::State) {
    abandon_remote_probe(state);
    state.remote_picker = None;
}

/// Open the local known-host registry. This deliberately schedules no discovery; contacting a
/// machine is an explicit consequence of activating its row.
pub(crate) fn open_remote_hosts(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.overlay_return = crate::ops::overlay_return::picker_origin(&ctx.state);
    install_remote_hosts(ctx, String::new(), None);
    Update::full()
}

/// Open the remote picker at launch with `target` selected, ready for the activation the startup
/// command sends next.
///
/// Installed synchronously so `--remote <host>` is already showing that host's row while the first
/// probe is still in flight — a launch that named a machine must never look like a generic host
/// list. No discovery is scheduled here; the activation does that, on the same path an Enter on the
/// row takes.
pub(crate) fn open_startup_remote_picker(
    ctx: &mut Context<AppRoot>,
    target: RemoteTarget,
    resume: Option<String>,
) {
    install_remote_hosts(ctx, String::new(), Some(target));
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.startup_resume = resume;
    }
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
    let rows = immediate_rows_for_target(&ctx.state, &target);
    scope_launcher_to(ctx, &target);
    if let Some(entry) = ctx.state.hosts.get_mut(&target) {
        entry.probe = crate::state::HostProbe::Reached;
    }
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target);
        let cursor = query.len();
        picker.session_input.set_text(query);
        picker.session_input.set_cursor(cursor);
        picker.session_input.set_anchor(None);
        picker.selected_session = selected;
        picker.host_probe = crate::state::HostProbe::Reached;
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
            abandon_remote_probe(&mut ctx.state);
            // The scope survives backing out. It belongs to the launcher, not to this overlay:
            // opening a host was the explicit request to work there, and stepping back to the host
            // list to look at the others does not withdraw it. Leaving a host is `Ctrl+X`, and
            // forgetting or attaching elsewhere moves the scope on its own.
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.return_to_hosts();
            }
            crate::ops::focus::request_remote_picker_focus(ctx);
            Update::full()
        }
        Some(RemotePickerMode::Hosts) => {
            if cancel_host_probe(ctx) {
                crate::ops::focus::request_remote_picker_focus(ctx);
                return Update::full();
            }
            ctx.state.remote_picker = None;
            ctx.state.commands_dirty = true;
            crate::ops::overlay_return::finish(ctx)
        }
        None => Update::none(),
    }
}

/// Give up on the host probe in flight, if there is one, and report whether there was.
///
/// Minting a fresh epoch is what makes it a give-up rather than a pause: the ssh keeps running to
/// its own conclusion, and its late answer no longer matches, so it cannot revive a picker the
/// user has moved on from. Used both by Esc on the picker and by refusing an ssh prompt, which is
/// the same decision reached from the other end.
pub(crate) fn cancel_host_probe(ctx: &mut Context<AppRoot>) -> bool {
    let in_flight = ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| matches!(picker.host_probe, crate::state::HostProbe::InFlight));
    if !in_flight {
        return false;
    }
    abandon_remote_probe(&mut ctx.state);
    let epoch = ctx.state.mint_remote_probe_epoch();
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.probe_epoch = epoch;
    }
    true
}

/// Point a *sessionless* client's launcher at `target`, so dismissing `Sessions · <host>` leaves it
/// reading `REMOTE · <host>` with no active session — a real resting state, and the thing that makes
/// the launcher's `Enter` start its shell there.
///
/// Only while in the launcher. With a session on screen the picker is something the user is looking
/// *through*, and browsing a host would otherwise quietly decide where they land after killing a
/// session they have not killed yet.
fn scope_launcher_to(ctx: &mut Context<AppRoot>, target: &RemoteTarget) {
    if ctx.state.is_launcher() {
        ctx.state.launcher_scope = Some(target.clone());
    }
}

fn reject_connecting_interaction(picker: &mut RemotePickerState) -> Update {
    picker.interaction_epoch = picker.interaction_epoch.wrapping_add(1);
    Update::full()
}

pub(crate) fn host_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        if matches!(picker.host_probe, crate::state::HostProbe::InFlight) {
            return reject_connecting_interaction(picker);
        }
        picker.host_input.set_text(query);
        picker.pending_forget = None;
    }
    Update::full()
}

pub(crate) fn host_selected(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        if matches!(picker.host_probe, crate::state::HostProbe::InFlight) {
            return reject_connecting_interaction(picker);
        }
        if picker.selected_host.as_ref() != Some(&target) {
            picker.pending_forget = None;
        }
        picker.selected_host = Some(target);
    }
    Update::full()
}

pub(crate) fn host_can_forget(state: &crate::state::State, target: &RemoteTarget) -> bool {
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
    if ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| matches!(picker.host_probe, crate::state::HostProbe::InFlight))
    {
        return Update::none();
    }
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
    crate::session::forget_last_session(Some(&target));
    if ctx.state.launcher_scope.as_ref() == Some(&target) {
        ctx.state.launcher_scope = None;
    }
    crate::session::remove_cached_host_sessions(&mut ctx.state.host_session_cache, &target);
    crate::ops::session::seed_host_registry(ctx);
    let selected = ctx
        .state
        .hosts
        .iter()
        .next()
        .map(|entry| entry.target.clone());
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.selected_host = selected;
        picker.pending_forget = None;
    }
    ctx.state.sidebar.invalidate_sessions();
    Update::full()
}

pub(crate) fn activate_host(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if ctx
        .state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| matches!(picker.host_probe, crate::state::HostProbe::InFlight))
    {
        return Update::none();
    }
    let rows = immediate_rows_for_target(&ctx.state, &target);
    let epoch = ctx.state.mint_remote_probe_epoch();
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.selected_host = Some(target.clone());
        picker.replace_sessions(rows);
        picker.probe_epoch = epoch;
        picker.host_probe = crate::state::HostProbe::InFlight;
    } else {
        return Update::none();
    }
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
    rows: std::result::Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
) -> Update {
    let current = ctx.state.remote_picker.as_ref().is_some_and(|picker| {
        picker.probe_epoch == epoch
            && matches!(picker.mode, RemotePickerMode::Hosts)
            && matches!(picker.host_probe, crate::state::HostProbe::InFlight)
            && picker.selected_host.as_ref() == Some(&target)
    });
    if !current {
        return Update::none();
    }
    match rows {
        Ok(mut rows) => {
            for attached in crate::ops::session::attached_session_rows(&ctx.state)
                .into_iter()
                .filter(|session| session.remote_target.as_ref() == Some(&target))
            {
                crate::ops::session::discovery::merge_current_session_row(&mut rows, attached);
            }
            let cached = crate::ops::session::discovery::cached_sessions_for_target(&rows, &target);
            crate::session::record_host_sessions(&target, cached.clone());
            crate::session::set_cached_host_sessions(
                &mut ctx.state.host_session_cache,
                &target,
                cached,
            );
            crate::session::record_recent_remote(&target);
            crate::ops::session::seed_host_registry(ctx);
            scope_launcher_to(ctx, &target);
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Reached;
            }
            let mut resume = None;
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.mode = RemotePickerMode::HostSessions {
                    target: target.clone(),
                };
                picker.host_probe = crate::state::HostProbe::Reached;
                picker.pending_forget = None;
                picker.pending_kill = None;
                picker.pending_restart = None;
                picker.target_prompt = None;
                picker.replace_sessions(rows);
                // Spent on this probe whatever it finds: a `last` the host no longer lists is a
                // session that stayed dead, and the user is already looking at what it does have.
                resume = picker.startup_resume.take().and_then(|name| {
                    picker
                        .sessions
                        .iter()
                        .find(|session| session.name == name)
                        .and_then(RemoteSessionIdentity::of)
                });
            }
            if let Some(identity) = resume {
                return activate_session(ctx, identity);
            }
        }
        Err(error) => {
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.host_probe = crate::state::HostProbe::Failed(error.clone());
                // An unreachable host answers nothing, so it cannot answer this either.
                picker.startup_resume = None;
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
    if matches!(picker.host_probe, crate::state::HostProbe::InFlight) {
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

pub(crate) fn target_prompt_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
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
            picker
                .sessions
                .iter()
                .find(|session| RemoteSessionIdentity::of(session).as_ref() == Some(&identity))
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
    if crate::ops::session::session_row_is_current(&ctx.state, &session) {
        dismiss_remote_picker(&mut ctx.state);
    }
    let update = crate::ops::session::kill_discovered_session(ctx, session.clone());
    let removed = session.remote_target.as_ref().is_some_and(|target| {
        crate::session::host_sessions_for(&ctx.state.host_session_cache, target)
            .is_none_or(|sessions| sessions.iter().all(|cached| cached.name != session.name))
    });
    if removed && let Some(picker) = ctx.state.remote_picker.as_mut() {
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
    if crate::ops::session::session_row_is_current(&ctx.state, &session) {
        dismiss_remote_picker(&mut ctx.state);
    }
    crate::ops::session::disconnect_discovered_attachment(ctx, session)
}

pub(crate) fn disconnect_selected_host(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = selected_target(&ctx.state) else {
        return Update::none();
    };
    if ctx.state.current().remote_target.as_ref() == Some(&target) {
        dismiss_remote_picker(&mut ctx.state);
    }
    crate::ops::session::disconnect_host(ctx, &target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    use crate::{AppRoot, Msg};

    fn state() -> crate::state::State {
        crate::state::State::new(
            crate::config::Config::default(),
            tui_lipan::prelude::Theme::default(),
        )
    }

    fn with_backend(body: impl FnOnce(&mut TestBackend<AppRoot>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                body(&mut backend);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn primed_connecting_picker(
        backend: &mut TestBackend<AppRoot>,
        target: &RemoteTarget,
        epoch: u64,
    ) {
        let state = backend.state_mut();
        state.config.remote.hosts.insert(
            target.display_label(),
            crate::config::RemoteHostConfig::default(),
        );
        state.hosts.seed(&state.config.remote, &[], &[]);
        state.hosts.get_mut(target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_epoch = epoch;
        state.remote_picker = Some(picker);
        state.remote_probe_epoch = epoch;
    }

    #[test]
    fn only_explicit_host_activation_records_a_probe_request() {
        let target = RemoteTarget::Alias("workbox".into());
        let _ = crate::ops::session::discovery::take_remote_probe_requests();
        let _picker = RemotePickerState::new(Some(target.clone()));
        assert!(crate::ops::session::discovery::take_remote_probe_requests().is_empty());

        let _command =
            host_discovery_command(1, target.clone(), crate::config::RemoteConfig::default());
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
    fn immediate_rows_restore_owned_ephemeral_without_persisting_it() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state();
        state.current_mut().session_name = Some("eph-owned".into());
        state.current_mut().session_attached = true;
        state.current_mut().remote_target = Some(target.clone());
        state.current_mut().remote_host = Some("workbox".into());
        crate::session::set_cached_host_sessions(
            &mut state.host_session_cache,
            &target,
            vec![
                crate::session::CachedHostSession {
                    name: "dev".into(),
                    ephemeral: false,
                    panes: 2,
                },
                crate::session::CachedHostSession {
                    name: "eph-stale".into(),
                    ephemeral: true,
                    panes: 1,
                },
            ],
        );

        let rows = immediate_rows_for_target(&state, &target);
        assert!(rows.iter().any(|row| row.name == "dev"));
        assert!(rows.iter().all(|row| row.name != "eph-stale"));
        assert!(
            rows.iter()
                .any(|row| row.name == "eph-owned" && row.ephemeral)
        );
        assert_eq!(
            crate::ops::session::discovery::cached_sessions_for_target(&rows, &target),
            vec![crate::session::CachedHostSession {
                name: "dev".into(),
                ephemeral: false,
                panes: 2,
            }]
        );
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
        config.remote.hosts.insert(
            "configured".into(),
            crate::config::RemoteHostConfig::default(),
        );
        let mut state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        state
            .hosts
            .seed(&state.config.remote, std::slice::from_ref(&recent), &[]);
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

    #[test]
    fn probe_epochs_survive_picker_replacement() {
        let mut state = state();
        let first = state.mint_remote_probe_epoch();
        state.remote_picker = None;
        let second = state.mint_remote_probe_epoch();

        assert_ne!(first, second);
    }

    #[test]
    fn remote_picker_counts_as_a_modal_overlay() {
        let mut state = state();
        state.remote_picker = Some(RemotePickerState::new(None));

        assert!(state.has_modal_overlay());
    }

    #[test]
    fn dismissing_picker_clears_abandoned_probe_state() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state();
        state
            .config
            .remote
            .hosts
            .insert("workbox".into(), crate::config::RemoteHostConfig::default());
        state.hosts.seed(&state.config.remote, &[], &[]);
        state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        state.remote_picker = Some(picker);

        dismiss_remote_picker(&mut state);

        assert!(state.remote_picker.is_none());
        assert_eq!(
            state.hosts.get(&target).map(|entry| &entry.probe),
            Some(&crate::state::HostProbe::Idle)
        );
    }

    #[test]
    fn cancelling_an_in_flight_probe_unlocks_the_hosts_list() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state();
        state
            .config
            .remote
            .hosts
            .insert("workbox".into(), crate::config::RemoteHostConfig::default());
        state.hosts.seed(&state.config.remote, &[], &[]);
        state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_epoch = 3;
        state.remote_picker = Some(picker);

        abandon_remote_probe(&mut state);

        assert!(state.remote_picker.is_some());
        assert!(matches!(
            state.remote_picker.as_ref().map(|picker| &picker.mode),
            Some(RemotePickerMode::Hosts)
        ));
        assert_eq!(
            state
                .remote_picker
                .as_ref()
                .map(|picker| &picker.host_probe),
            Some(&crate::state::HostProbe::Idle)
        );
        assert_eq!(
            state.hosts.get(&target).map(|entry| &entry.probe),
            Some(&crate::state::HostProbe::Idle)
        );
    }

    #[test]
    fn a_successful_host_probe_opens_that_hosts_sessions() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 4);
            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 4,
                    target: target.clone(),
                    rows: Ok(Vec::new()),
                })
                .expect("apply successful probe");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert!(matches!(
                &picker.mode,
                RemotePickerMode::HostSessions { target: active } if active == &target
            ));
            assert_eq!(picker.host_probe, crate::state::HostProbe::Reached);
        });
    }

    /// `startup = "last"` under `--remote` resumes a session, it never revives one. The host's own
    /// discovery is the authority — the launch never blocks on an SSH probe before the first frame —
    /// so a session still listed is attached, and one killed while rozi was away stays dead with the
    /// user on `Sessions · <host>`.
    #[test]
    fn a_remembered_session_is_resumed_only_when_the_host_still_lists_it() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 7);
            // The default launch queues its own ephemeral attach; this test is about what the
            // probe does, so clear it and let any attach below be the probe's doing.
            backend.state_mut().current_mut().pending_session_attach = None;
            backend
                .state_mut()
                .remote_picker
                .as_mut()
                .expect("remote picker")
                .startup_resume = Some("backend".into());

            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 7,
                    target: target.clone(),
                    rows: Ok(vec![crate::session::discovery::DiscoveredSession {
                        name: "api".into(),
                        ephemeral: false,
                        host: Some("workbox".into()),
                        remote_target: Some(target.clone()),
                        status: crate::session::discovery::DiscoveredSessionStatus::Running {
                            panes: 1,
                            clients: 0,
                            has_layout: false,
                            created_from_profile: None,
                        },
                    }]),
                })
                .expect("apply a probe that does not list the remembered session");

            let state = backend.state();
            let picker = state.remote_picker.as_ref().expect("remote picker");
            assert!(
                matches!(&picker.mode, RemotePickerMode::HostSessions { target: active } if active == &target),
                "a session the host no longer has leaves the user on that host's picker"
            );
            assert!(
                state.current().pending_session_attach.is_none(),
                "and nothing is recreated under the remembered name"
            );
            assert!(
                picker.startup_resume.is_none(),
                "the resume is spent on the first probe, whatever it found"
            );
        });

        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 8);
            backend.state_mut().current_mut().pending_session_attach = None;
            backend
                .state_mut()
                .remote_picker
                .as_mut()
                .expect("remote picker")
                .startup_resume = Some("backend".into());

            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 8,
                    target: target.clone(),
                    rows: Ok(vec![crate::session::discovery::DiscoveredSession {
                        name: "backend".into(),
                        ephemeral: false,
                        host: Some("workbox".into()),
                        remote_target: Some(target.clone()),
                        status: crate::session::discovery::DiscoveredSessionStatus::Running {
                            panes: 2,
                            clients: 0,
                            has_layout: false,
                            created_from_profile: None,
                        },
                    }]),
                })
                .expect("apply a probe that still lists the remembered session");

            let state = backend.state();
            let pending = state
                .current()
                .pending_session_attach
                .as_ref()
                .expect("the remembered session is attached");
            assert_eq!(pending.name, "backend");
            assert_eq!(pending.remote_host.as_deref(), Some("workbox"));
        });
    }

    #[test]
    fn a_failed_host_probe_stays_on_remote_hosts() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 5);
            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 5,
                    target: target.clone(),
                    rows: Err("Host key not trusted".into()),
                })
                .expect("apply failed probe");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert!(matches!(picker.mode, RemotePickerMode::Hosts));
            assert!(matches!(
                &picker.host_probe,
                crate::state::HostProbe::Failed(_)
            ));
        });
    }

    #[test]
    fn connecting_ignores_host_selection_changes() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            let other = RemoteTarget::Alias("other".into());
            primed_connecting_picker(backend, &target, 6);
            backend
                .dispatch(Msg::RemotePickerHostSelect(other))
                .expect("ignore selection while connecting");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert_eq!(picker.selected_host.as_ref(), Some(&target));
            assert!(picker.interaction_epoch > 0);
            assert!(matches!(
                picker.host_probe,
                crate::state::HostProbe::InFlight
            ));
        });
    }
}
