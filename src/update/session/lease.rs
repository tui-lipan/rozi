use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::protocol::{ClientInfo, ControllerChangeReason};
use crate::shared_layout::ClientId;

pub(crate) fn controller_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    controller: Option<ClientId>,
    reason: ControllerChangeReason,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.controller = controller;
            if shared.is_controller() {
                shared.assumed_rev = shared.layout_rev;
                shared.last_committed_layout = None;
            }
        }
        return Update::none();
    }
    let was_controller = ctx.state.is_controller();
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.controller = controller;
        if shared.is_controller() {
            // Gaining control: rebase optimistic commits, and clear the dirty detector so the tail
            // chokepoint republishes the layout with our canonical canvas.
            shared.assumed_rev = shared.layout_rev;
            shared.last_committed_layout = None;
        }
    }
    let now_controller = ctx.state.is_controller();
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::ControllerChanged,
            vec![
                (
                    "controller",
                    controller.map(|id| id.to_string()).unwrap_or_default(),
                ),
                ("self_controller", now_controller.to_string()),
                ("reason", controller_change_reason_id(reason).to_string()),
            ],
        ),
    );
    if was_controller && !now_controller {
        ctx.state.moving_pane = None;
        ctx.state.resizing_pane = None;
        ctx.state.split_drag = None;
        let who = controller
            .map(|id| format!("client {id}"))
            .unwrap_or_else(|| "another client".to_string());
        crate::pty_events::notify_on(
            ctx,
            crate::state::ToastChannel::LayoutControl,
            None,
            format!("Layout control taken by {who}"),
        );
    } else if !was_controller && now_controller {
        // No toast: the workbar chip flips to CTRL, and unlike losing control (which another client
        // caused off screen) gaining it is either something this client asked for or a lease the
        // server handed over - either way the chip is the whole story.
        crate::ops::session::apply_pending_background_closes(ctx);
    }
    if now_controller && !ctx.state.current().pending_resizes.is_empty() {
        crate::pty_events::flush_pending_resizes(ctx);
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn clients_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    clients: Vec<ClientInfo>,
    input_locked: bool,
    allow_takeover: bool,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.clients = clients;
            shared.input_locked = input_locked;
            shared.allow_takeover = allow_takeover;
        }
        return Update::none();
    }
    let roster_events = ctx
        .state
        .current()
        .shared
        .as_ref()
        .map(|shared| roster_diff_events(&shared.clients, &clients))
        .unwrap_or_default();
    // The lock state lives in the workbar chip (`CTRL LOCK` / `FOLLOW LOCK`), which flips with this
    // assignment, so the transition needs no separate announcement.
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.clients = clients;
        shared.input_locked = input_locked;
        shared.allow_takeover = allow_takeover;
    }
    for event in roster_events {
        crate::events::emit(&ctx.state, event);
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn control_requested(ctx: &mut Context<AppRoot>, epoch: u64, from: ClientId) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    // Only the controller can act on a request; ignore a stale one that arrived after we lost the
    // lease. The requester's badge lives in the roster (via ClientsChanged) regardless.
    if !ctx.state.is_controller() {
        return Update::none();
    }
    let who = ctx
        .state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.clients.iter().find(|client| client.id == from))
        .map(|client| format!("{} #{}", client.label, client.id))
        .unwrap_or_else(|| format!("client {from}"));
    // Advertise the live grant binding so the hint tracks any `[keys]` override instead of a
    // hardcoded key; fall back to the collaborators view when the action is unbound.
    let how = crate::commands::command_prefix_chord(ctx, "grant-control")
        .map(|chord| format!("{chord} to grant"))
        .unwrap_or_else(|| "grant from Manage collaborators".to_string());
    crate::pty_events::notify_info(ctx, format!("{who} requests layout control\n{how}"));
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn control_declined(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    crate::pty_events::notify_info(ctx, "Your control request was declined");
    Update::full()
}

/// The controller removed this client. The server has already closed the connection, so there is
/// nothing to detach — this only has to leave the session behind deliberately, before the resulting
/// disconnect reaches `disconnected` and gets answered with a reconnect.
pub(crate) fn evicted(ctx: &mut Context<AppRoot>, epoch: u64, message: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        // Removed from a session we were keeping in the background: drop the attachment rather than
        // let it sit there offline, since reconnecting it is exactly what the removal ruled out.
        // The server has closed the connection already, so there is nothing to detach.
        if ctx.state.background.remove(&epoch).is_some() {
            ctx.state.sidebar.invalidate_sessions();
        }
        return Update::none();
    }
    let name = ctx.state.current().session_name.clone();
    crate::ops::session::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    let update = crate::ops::session::land_on_surviving_session(ctx);
    // The attachment was ended by the server, not parked; retaining it would render a session we
    // are no longer welcome in as merely offline.
    ctx.state.background.remove(&epoch);
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::SessionLifecycle,
        Some("Removed from session".to_string()),
        match name {
            Some(name) => format!("`{name}`: {message}"),
            None => message,
        },
    );
    update
}

pub(crate) fn controller_change_reason_id(reason: ControllerChangeReason) -> &'static str {
    match reason {
        ControllerChangeReason::Released => "released",
        ControllerChangeReason::Expired => "expired",
        ControllerChangeReason::Granted => "granted",
    }
}

pub(crate) fn roster_diff_events(
    previous: &[ClientInfo],
    current: &[ClientInfo],
) -> Vec<crate::events::Event> {
    let count = current.len().to_string();
    let mut events = Vec::new();
    for client in current {
        if !previous.iter().any(|existing| existing.id == client.id) {
            events.push(crate::events::Event::new(
                crate::events::EventKind::ClientJoined,
                vec![
                    ("client_id", client.id.to_string()),
                    ("client_name", client.label.clone()),
                    ("count", count.clone()),
                ],
            ));
        }
    }
    for client in previous {
        if !current.iter().any(|existing| existing.id == client.id) {
            events.push(crate::events::Event::new(
                crate::events::EventKind::ClientLeft,
                vec![
                    ("client_id", client.id.to_string()),
                    ("client_name", client.label.clone()),
                    ("count", count.clone()),
                ],
            ));
        }
    }
    events
}
