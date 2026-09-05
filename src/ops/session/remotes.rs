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
    let target = state.remote_picker.as_ref().and_then(|picker| {
        matches!(picker.host_probe, crate::state::HostProbe::InFlight)
            .then(|| picker.probe_target.clone())
            .flatten()
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
        picker.probe_target = None;
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
        // A launch that named a machine has already said where it wants to work, so its probe goes
        // on into that host's sessions. An `Enter` typed on this list has said no such thing.
        picker.auto_open = true;
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

/// One host being contacted no longer holds the whole list still: reading the other machines while
/// `workbox` connects is ordinary browsing, and freezing it bought nothing. What stays refused is a
/// *second* act on the row already in flight, which is guarded per action.
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

/// Whether the selected host is the one currently being reached, which is the only row the list
/// refuses to act on.
fn host_is_connecting(state: &crate::state::State, target: &RemoteTarget) -> bool {
    state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| picker.is_connecting(target))
}

/// Whether this host is rozi's to edit: one the user saved or last reached, not one the config file
/// defines and not one that only exists because something is attached to it.
pub(crate) fn host_can_edit(state: &crate::state::State, target: &RemoteTarget) -> bool {
    state
        .hosts
        .get(target)
        .is_some_and(|entry| entry.origin.is_user_owned())
        && !host_is_connecting(state, target)
}

pub(crate) fn host_can_forget(state: &crate::state::State, target: &RemoteTarget) -> bool {
    let Some(entry) = state.hosts.get(target) else {
        return false;
    };
    entry.origin.is_user_owned()
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
    ctx.state.added_hosts.retain(|entry| entry != &target);
    crate::session::forget_saved_host(&target);
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

/// Step into `Sessions · <host>` for a host already reached, listing what the last probe found.
fn open_host_sessions(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    let rows = immediate_rows_for_target(&ctx.state, &target);
    scope_launcher_to(ctx, &target);
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target);
        picker.host_probe = crate::state::HostProbe::Reached;
        picker.replace_sessions(rows);
    }
    crate::ops::focus::request_remote_picker_focus(ctx);
    Update::full()
}

/// Whether a probe this picker started is still outstanding, on any host.
///
/// One at a time, deliberately. Navigation is free while a host connects, so without this the user
/// can highlight another machine and start a second ssh whose answer would land on a picker that
/// tracks exactly one in-flight target — the first host would be left spinning forever with nothing
/// coming for it. Genuinely parallel connections want per-host probe state, which is more machinery
/// than this surface needs.
fn probe_in_flight(state: &crate::state::State) -> bool {
    state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| matches!(picker.host_probe, crate::state::HostProbe::InFlight))
}

/// Contact `target` and leave the user on the host list while it happens.
///
/// A no-op while another host is being reached: the caller has already done its own persisting, and
/// the row it added stays exactly where it is, ready for an `Enter` once the outstanding probe
/// settles.
fn connect_host(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if probe_in_flight(&ctx.state) {
        return Update::full();
    }
    let epoch = ctx.state.mint_remote_probe_epoch();
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.selected_host = Some(target.clone());
        picker.probe_epoch = epoch;
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_target = Some(target.clone());
        picker.pending_forget = None;
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

/// `Enter` on a host row. Connecting a host and opening it are two acts, and this key does whichever
/// one is next.
///
/// A disconnected — or previously failed — host is contacted, and the user stays on **Remote hosts**
/// watching that row go from `○` to `●`. That is the confirmation: a list that silently replaced
/// itself with a session list gave the user nothing to see, and made `Enter` mean two things at
/// once. A host already reached opens, because there is nothing left to confirm.
pub(crate) fn activate_host(ctx: &mut Context<AppRoot>, target: RemoteTarget) -> Update {
    if probe_in_flight(&ctx.state) {
        return Update::none();
    }
    let reached = matches!(
        ctx.state.hosts.get(&target).map(|entry| &entry.probe),
        Some(crate::state::HostProbe::Reached)
    );
    if reached {
        open_host_sessions(ctx, target)
    } else {
        connect_host(ctx, target)
    }
}

/// `Ctrl+R`: contact the selected host again whatever state it is in — the way back from a host that
/// went away under a connection this client still believes in, and the refresh for a session list
/// that has moved on since the last probe.
pub(crate) fn reconnect_host(ctx: &mut Context<AppRoot>) -> Update {
    if probe_in_flight(&ctx.state) {
        return Update::none();
    }
    let Some(target) = ctx
        .state
        .remote_picker
        .as_ref()
        .filter(|picker| matches!(picker.mode, RemotePickerMode::Hosts))
        .and_then(|picker| picker.selected_host.clone())
    else {
        return Update::none();
    };
    connect_host(ctx, target)
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
            && picker.is_connecting(&target)
    });
    if !current {
        return Update::none();
    }
    let label = target.display_label();
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
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Reached;
            }
            let mut auto_open = false;
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.host_probe = crate::state::HostProbe::Reached;
                picker.probe_target = None;
                picker.selected_host = Some(target.clone());
                picker.pending_forget = None;
                auto_open = std::mem::take(&mut picker.auto_open);
                if !auto_open {
                    // `Enter` reached the host and stops there. The row now reads connected with
                    // its session count, which is the whole confirmation, and a second `Enter`
                    // opens it. Nothing is attached and no shell is started: connecting to a
                    // machine says nothing about wanting to work on it yet.
                    picker.startup_resume = None;
                }
            }
            if !auto_open {
                crate::pane::pty_events::notify_info(ctx, format!("Connected to {label}"));
                return Update::full();
            }
            let update = open_host_sessions(ctx, target);
            // Spent on this probe whatever it finds: a `last` the host no longer lists is a
            // session that stayed dead, and the user is already looking at what it does have.
            let resume = ctx.state.remote_picker.as_mut().and_then(|picker| {
                picker.startup_resume.take().and_then(|name| {
                    picker
                        .sessions
                        .iter()
                        .find(|session| session.name == name)
                        .and_then(RemoteSessionIdentity::of)
                })
            });
            match resume {
                Some(identity) => activate_session(ctx, identity),
                None => update,
            }
        }
        Err(error) => {
            // The row stays, carrying the failure. A host is listed because the user said it is one
            // of their machines; a sleeping laptop, a VPN that is down, or a login typed wrong is
            // not that statement being withdrawn. `Enter` retries, `Ctrl+E` corrects, `Ctrl+K`
            // forgets — and none of those exist if the row disappears.
            if let Some(picker) = ctx.state.remote_picker.as_mut() {
                picker.host_probe = crate::state::HostProbe::Failed(error.clone());
                picker.probe_target = None;
                // An unreachable host answers nothing, so it cannot answer these either.
                picker.startup_resume = None;
                picker.auto_open = false;
            }
            if let Some(entry) = ctx.state.hosts.get_mut(&target) {
                entry.probe = crate::state::HostProbe::Failed(error.clone());
            }
            let reason = crate::session::discovery::probe_failure_reason(&error);
            crate::pane::pty_events::notify_error(
                ctx,
                format!("Could not connect to {label}"),
                reason,
            );
            Update::full()
        }
    }
}

pub(crate) fn open_add_host_form(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.remote_picker.as_mut() else {
        return Update::none();
    };
    if !matches!(picker.mode, RemotePickerMode::Hosts) {
        return Update::none();
    }
    let initial = picker.host_input.text().trim().to_string();
    picker.host_form = Some(crate::state::HostFormState::add(initial));
    picker.pending_forget = None;
    crate::ops::focus::request_host_form_focus(ctx);
    Update::full()
}

pub(crate) fn open_new_host_flow(ctx: &mut Context<AppRoot>) -> Update {
    let _ = open_remote_hosts(ctx);
    open_add_host_form(ctx)
}

pub(crate) fn open_edit_host_form(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = ctx
        .state
        .remote_picker
        .as_ref()
        .filter(|picker| matches!(picker.mode, RemotePickerMode::Hosts))
        .and_then(|picker| picker.selected_host.clone())
    else {
        return Update::none();
    };
    if !host_can_edit(&ctx.state, &target) {
        return Update::none();
    }
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.host_form = Some(crate::state::HostFormState::edit(&target));
        picker.pending_forget = None;
    }
    crate::ops::focus::request_host_form_focus(ctx);
    Update::full()
}

pub(crate) fn host_form_changed(
    ctx: &mut Context<AppRoot>,
    field: crate::state::HostFormField,
    event: InputEvent,
) -> Update {
    if let Some(form) = ctx
        .state
        .remote_picker
        .as_mut()
        .and_then(|picker| picker.host_form.as_mut())
    {
        form.focus = field;
        event.apply_to(form.input_mut(field));
        form.error = None;
    }
    Update::full()
}

/// Whether the host editor is open, and therefore owns `Tab`.
pub(crate) fn host_form_is_open(state: &crate::state::State) -> bool {
    state
        .remote_picker
        .as_ref()
        .is_some_and(|picker| picker.host_form.is_some())
}

/// `Tab` / `Shift+Tab` between the editor's lines.
///
/// Claimed at the root rather than left to the framework's focus ring: traversal visits this
/// dialog's three inputs in its own order, which ran host → port → username — not the order they
/// are drawn in, and not the order anyone reads them.
pub(crate) fn host_form_cycle_focus(ctx: &mut Context<AppRoot>, forward: bool) -> Update {
    if let Some(form) = ctx
        .state
        .remote_picker
        .as_mut()
        .and_then(|picker| picker.host_form.as_mut())
    {
        form.cycle_focus(forward);
    }
    crate::ops::focus::request_host_form_focus(ctx);
    Update::full()
}

pub(crate) fn close_host_form(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.host_form = None;
    }
    crate::ops::focus::request_remote_picker_focus(ctx);
    Update::full()
}

fn reject_host_form(ctx: &mut Context<AppRoot>, error: String) -> Update {
    if let Some(form) = ctx
        .state
        .remote_picker
        .as_mut()
        .and_then(|picker| picker.host_form.as_mut())
    {
        form.error = Some(error);
    }
    crate::ops::focus::request_host_form_focus(ctx);
    Update::full()
}

/// Submit the host editor.
///
/// The login line may be left empty, which is a real answer rather than a missing one:
/// `~/.ssh/config` stays the advanced layer, and rozi stores only enough to make its own targets
/// convenient.
///
/// Whatever the form describes is saved **before** the connection is attempted, and stays saved
/// however that attempt ends.
pub(crate) fn submit_host_form(ctx: &mut Context<AppRoot>) -> Update {
    let Some((mode, target)) = ctx
        .state
        .remote_picker
        .as_ref()
        .and_then(|picker| picker.host_form.as_ref())
        .map(|form| (form.mode.clone(), form.target()))
    else {
        return Update::none();
    };
    let target = match target {
        Ok(target) => target,
        Err(error) => return reject_host_form(ctx, error),
    };

    let stored = match &mode {
        crate::state::HostFormMode::Add => crate::session::save_host(&target),
        crate::state::HostFormMode::Edit(previous) if previous == &target => Ok(()),
        crate::state::HostFormMode::Edit(previous) => {
            if ctx.state.launcher_scope.as_ref() == Some(previous) {
                ctx.state.launcher_scope = None;
            }
            ctx.state.added_hosts.retain(|entry| entry != previous);
            // An edited host that was only a recent becomes a saved one: correcting an entry is
            // the user deliberately configuring it, and the result should outlive the MRU it came
            // from. `replace_saved_host` adds it when the old identity was never in the roster.
            let stored = crate::session::replace_saved_host(previous, &target);
            crate::session::forget_recent_remote(previous);
            crate::session::forget_host_sessions(previous);
            crate::session::forget_last_session(Some(previous));
            crate::session::remove_cached_host_sessions(
                &mut ctx.state.host_session_cache,
                previous,
            );
            stored
        }
    };
    // Held for this run whatever the disk did, so the row exists to retry, correct, or forget even
    // when it could not be written.
    if !ctx.state.added_hosts.contains(&target) {
        ctx.state.added_hosts.push(target.clone());
    }
    crate::ops::session::seed_host_registry(ctx);
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.host_form = None;
        picker.selected_host = Some(target.clone());
    }
    ctx.state.sidebar.invalidate_sessions();
    if let Err(error) = stored {
        // The host is listed and usable; what it will not do is survive a restart. Saying so beats
        // both silence and refusing to continue.
        crate::pane::pty_events::notify_warning(
            ctx,
            format!("{} not saved for next time", target.display_label()),
            error,
        );
    }
    // Editing is about the stored entry, not about going somewhere: leave the corrected row
    // selected and let the user decide when to try it. Adding one is a request to reach it.
    if matches!(mode, crate::state::HostFormMode::Edit(_)) {
        crate::ops::focus::request_remote_picker_focus(ctx);
        return Update::full();
    }
    if probe_in_flight(&ctx.state) {
        // Only one connection at a time, and one is already out. The row is here and selected;
        // saying so beats appearing to do nothing with the `Enter` that just added it.
        let label = target.display_label();
        crate::pane::pty_events::notify_info(
            ctx,
            format!("Added {label} — press Enter to connect once the current host answers"),
        );
        crate::ops::focus::request_remote_picker_focus(ctx);
        return Update::full();
    }
    connect_host(ctx, target)
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

    /// One form line's worth of typing, as the bound `Input` would report it.
    fn typed(value: &str) -> InputEvent {
        InputEvent {
            value: value.into(),
            cursor: value.len(),
            anchor: None,
        }
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
        state.hosts.seed(&state.config.remote, &[], &[], &[]);
        state.hosts.get_mut(target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_target = Some(target.clone());
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
    fn only_offline_hosts_rozi_owns_are_forgettable() {
        let saved = RemoteTarget::Alias("saved".into());
        let recent = RemoteTarget::Alias("recent".into());
        let configured = RemoteTarget::Alias("configured".into());
        let mut config = crate::config::Config::default();
        config.remote.hosts.insert(
            "configured".into(),
            crate::config::RemoteHostConfig::default(),
        );
        let mut state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        state.hosts.seed(
            &state.config.remote,
            std::slice::from_ref(&saved),
            std::slice::from_ref(&recent),
            &[],
        );
        assert!(host_can_forget(&state, &saved));
        assert!(host_can_forget(&state, &recent));
        assert!(!host_can_forget(&state, &configured));
        assert!(host_can_edit(&state, &saved));
        assert!(!host_can_edit(&state, &configured));

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
        state.hosts.seed(&state.config.remote, &[], &[], &[]);
        state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_target = Some(target.clone());
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
        state.hosts.seed(&state.config.remote, &[], &[], &[]);
        state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::InFlight;
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.host_probe = crate::state::HostProbe::InFlight;
        picker.probe_target = Some(target.clone());
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

    /// Reaching a host and opening it are two acts. The first one ends on **Remote hosts** with the
    /// row now reading connected — that visible change is the whole confirmation, and it is what
    /// the old behaviour (silently swapping the list for a session list) denied the user.
    #[test]
    fn a_successful_host_probe_stays_on_remote_hosts() {
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

            let state = backend.state();
            let picker = state.remote_picker.as_ref().expect("remote picker");
            assert!(matches!(picker.mode, RemotePickerMode::Hosts));
            assert_eq!(picker.host_probe, crate::state::HostProbe::Reached);
            assert_eq!(picker.selected_host.as_ref(), Some(&target));
            assert_eq!(
                state.hosts.get(&target).map(|entry| &entry.probe),
                Some(&crate::state::HostProbe::Reached),
                "the row itself reports the host as reached"
            );
            assert!(
                state.launcher_scope.is_none(),
                "reaching a machine is not yet a request to work on it"
            );
        });
    }

    /// And the second `Enter` opens it, without contacting the host again.
    #[test]
    fn a_second_activation_opens_a_reached_host() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 9);
            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 9,
                    target: target.clone(),
                    rows: Ok(Vec::new()),
                })
                .expect("apply successful probe");

            backend
                .dispatch(Msg::RemotePickerHostActivate(target.clone()))
                .expect("open the reached host");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert!(matches!(
                &picker.mode,
                RemotePickerMode::HostSessions { target: active } if active == &target
            ));
            assert!(
                picker.probe_target.is_none()
                    && !matches!(picker.host_probe, crate::state::HostProbe::InFlight),
                "opening a host already reached contacts nothing"
            );
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
            // `--remote <host>` is the only caller that carries a resume, and it is the caller that
            // goes on into the host rather than stopping on the list.
            backend
                .state_mut()
                .remote_picker
                .as_mut()
                .expect("remote picker")
                .auto_open = true;

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
            // `--remote <host>` is the only caller that carries a resume, and it is the caller that
            // goes on into the host rather than stopping on the list.
            backend
                .state_mut()
                .remote_picker
                .as_mut()
                .expect("remote picker")
                .auto_open = true;

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

    /// A failure has to leave something behind to act on. The row stays, holding the reason, so
    /// `Enter` can retry it and `Ctrl+E` can correct it.
    #[test]
    fn a_failed_host_probe_leaves_the_row_carrying_the_reason() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            primed_connecting_picker(backend, &target, 5);
            backend
                .dispatch(Msg::RemoteHostSessionsDiscovered {
                    epoch: 5,
                    target: target.clone(),
                    rows: Err("Permission denied (publickey,password)".into()),
                })
                .expect("apply failed probe");

            let state = backend.state();
            let picker = state.remote_picker.as_ref().expect("remote picker");
            assert!(matches!(picker.mode, RemotePickerMode::Hosts));
            assert!(matches!(
                &picker.host_probe,
                crate::state::HostProbe::Failed(_)
            ));
            let entry = state
                .hosts
                .get(&target)
                .expect("the host that could not be reached is still listed");
            assert_eq!(
                crate::session::discovery::probe_failure_reason(
                    entry.probe.error().expect("the row carries the failure")
                ),
                "SSH login rejected"
            );
        });
    }

    /// One machine being contacted is no reason to freeze the other rows — but it is a reason to
    /// refuse a *second* connection. This picker tracks one in-flight target, so a probe started on
    /// another row would strand the first host spinning with no answer coming for it.
    #[test]
    fn connecting_leaves_navigation_free_but_starts_no_second_probe() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("workbox".into());
            let other = RemoteTarget::Alias("other".into());
            primed_connecting_picker(backend, &target, 6);
            backend
                .dispatch(Msg::RemotePickerHostSelect(other.clone()))
                .expect("move the highlight while connecting");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert_eq!(picker.selected_host.as_ref(), Some(&other));
            assert!(matches!(
                picker.host_probe,
                crate::state::HostProbe::InFlight
            ));
            assert_eq!(
                picker.probe_target.as_ref(),
                Some(&target),
                "the answer still belongs to the host that was asked, not to the highlight"
            );

            // A probe that started would mint a fresh epoch and claim `probe_target` for its own
            // host, so the pair being unchanged is the evidence that none did.
            let epoch = picker.probe_epoch;
            backend
                .dispatch(Msg::RemotePickerHostActivate(other))
                .expect("Enter on another row while one host connects");
            backend
                .dispatch(Msg::RemotePickerHostActivate(target.clone()))
                .expect("Enter on the connecting row itself");
            backend
                .dispatch(Msg::RemotePickerReconnectHost)
                .expect("Ctrl+R while one host connects");

            let picker = backend
                .state()
                .remote_picker
                .as_ref()
                .expect("remote picker");
            assert_eq!(
                picker.probe_target.as_ref(),
                Some(&target),
                "no key starts a second connection while one is outstanding"
            );
            assert_eq!(
                picker.probe_epoch, epoch,
                "and the outstanding one is untouched"
            );
        });
    }

    /// The add flow, end to end.
    ///
    /// The entry is written *before* the connection is attempted, so a first attempt that fails
    /// leaves a row to retry rather than nothing at all. A login left empty is a real answer, and
    /// one spelled into the host line is taken from there.
    ///
    /// Both halves live in one test because the saved-host file is process-wide under
    /// [`crate::test_support::isolate_user_dirs`], and two tests writing it would race.
    #[test]
    fn adding_a_host_saves_it_before_connecting() {
        with_backend(|backend| {
            crate::test_support::isolate_user_dirs();
            backend
                .dispatch(Msg::SessionPickerRemoteHosts)
                .expect("open remote hosts");

            // A bare alias with the login left empty: OpenSSH keeps deciding.
            let alias = RemoteTarget::Alias("workbox".into());
            backend
                .dispatch(Msg::RemotePickerNewHost)
                .expect("open the add form");
            backend
                .dispatch(Msg::HostFormChanged(
                    crate::state::HostFormField::Host,
                    typed("workbox"),
                ))
                .expect("type the host");
            backend
                .dispatch(Msg::SubmitHostForm)
                .expect("submit the form");
            assert_eq!(crate::session::read_saved_hosts(), vec![alias.clone()]);
            assert!(
                backend
                    .state()
                    .remote_picker
                    .as_ref()
                    .expect("remote picker")
                    .is_connecting(&alias),
                "and the connection is attempted after the entry exists, not before"
            );

            // A host line that names a login carries it into the target on its own.
            let endpoint = crate::session::remote::parse_remote_target("adam@10.0.0.5")
                .expect("a login and a host is a valid target");
            backend
                .dispatch(Msg::RemotePickerNewHost)
                .expect("open the add form again");
            backend
                .dispatch(Msg::HostFormChanged(
                    crate::state::HostFormField::Host,
                    typed("adam@10.0.0.5"),
                ))
                .expect("type the endpoint");
            backend
                .dispatch(Msg::SubmitHostForm)
                .expect("submit the form");

            assert_eq!(
                crate::session::read_saved_hosts(),
                vec![alias, endpoint.clone()]
            );
            let state = backend.state();
            assert_eq!(
                state.hosts.get(&endpoint).map(|entry| entry.origin),
                Some(crate::state::HostOrigin::Saved),
                "the row exists before the probe has answered anything"
            );
            let picker = state.remote_picker.as_ref().expect("remote picker");
            assert!(picker.host_form.is_none());
            assert_eq!(picker.selected_host.as_ref(), Some(&endpoint));
            // Whether this one connects depends on the first probe having answered, which is a real
            // ssh and not this test's to time. `connecting_leaves_navigation_free_but_starts_no_
            // second_probe` covers the guard on a probe held in flight deliberately.

            // Editing a host rozi only *remembered* is the user configuring it on purpose, so the
            // corrected entry joins the roster rather than staying an MRU byproduct.
            let remembered = RemoteTarget::Alias("scratch".into());
            let corrected = crate::session::remote::parse_remote_target("adam@scratch")
                .expect("a login and a host is a valid target");
            crate::session::replace_saved_host(&remembered, &corrected)
                .expect("promote a remembered host into the roster");
            assert!(
                crate::session::read_saved_hosts().contains(&corrected),
                "an edited recent becomes a saved host"
            );
        });
    }

    /// The bug this guards: the roster is normally read back from disk, so a write that does not
    /// land — a state directory that is not private to its owner is the usual reason — took the row
    /// with it, and a host that then failed to connect left the user with nothing but a toast.
    #[test]
    fn a_host_added_this_run_is_listed_even_if_it_reached_no_disk() {
        with_backend(|backend| {
            let target = RemoteTarget::Alias("unwritable".into());
            backend.state_mut().added_hosts.push(target.clone());
            backend
                .dispatch(Msg::SessionPickerRemoteHosts)
                .expect("open remote hosts");

            let state = backend.state();
            assert_eq!(
                state.hosts.get(&target).map(|entry| entry.origin),
                Some(crate::state::HostOrigin::Saved),
                "a host held for this run is listed like any other saved host"
            );
            assert!(
                host_can_forget(state, &target) && host_can_edit(state, &target),
                "and it can be retried, corrected, or forgotten like one"
            );
        });
    }
}
