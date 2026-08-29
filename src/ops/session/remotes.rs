use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::remote::RemoteTarget;
use crate::state::{RemotePickerMode, RemotePickerState, RemoteSessionIdentity};

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
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target);
        let cursor = query.len();
        picker.session_input.set_text(query);
        picker.session_input.set_cursor(cursor);
        picker.session_input.set_anchor(None);
        picker.selected_session = selected;
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
    if let Some(picker) = ctx.state.remote_picker.as_mut() {
        picker.enter_host_sessions(target);
    }
    crate::ops::focus::request_remote_picker_focus(ctx);
    Update::full()
}
