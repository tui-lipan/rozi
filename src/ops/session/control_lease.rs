use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::focus::request_current_pane_focus;

pub(crate) fn require_attached(ctx: &mut Context<AppRoot>) -> Option<()> {
    if ctx.state.current().session_attached {
        Some(())
    } else {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        None
    }
}

pub(crate) fn require_writable(ctx: &mut Context<AppRoot>) -> Option<()> {
    require_attached(ctx)?;
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        return None;
    };
    if shared.read_only {
        crate::pty_events::notify_info(ctx, "Attached read-only");
        return None;
    }
    Some(())
}

/// If this client is a follower (attached but not the controller), push the take-control nudge and
/// return `true` so the caller aborts a layout-mutating gesture. Controllers and local/unattached
/// sessions return `false`.
pub(crate) fn nudge_if_follower(ctx: &mut Context<AppRoot>) -> bool {
    if ctx.state.is_controller() {
        return false;
    }
    let who = ctx
        .state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.controller)
        .map(|id| format!("client {id}"))
        .unwrap_or_else(|| "another client".to_string());
    // Advertise the live request binding so the hint tracks any `[keys]` override.
    let allow_takeover = ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.allow_takeover);
    let verb = if allow_takeover { "take" } else { "request" };
    let how = crate::commands::command_prefix_chord(ctx, "request-control")
        .map(|chord| format!("{chord} to {verb} control"))
        .unwrap_or_else(|| format!("Try to {verb} control"));
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        None,
        format!("Layout controlled by {who}\n{how}"),
    );
    true
}

/// Request the layout-control lease. A takeover-enabled server grants immediately; cooperative
/// sessions flag the request and notify the controller for a grant or decline.
pub(crate) fn request_control(ctx: &mut Context<AppRoot>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    if ctx.state.is_controller() {
        crate::pty_events::notify_info(ctx, "You already control the layout");
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    let shared = ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked");
    let already_requested = shared
        .clients
        .iter()
        .any(|client| client.id == shared.client_id && client.requesting_control);
    let allow_takeover = shared.allow_takeover;
    let controller_label = shared
        .controller
        .and_then(|id| shared.clients.iter().find(|client| client.id == id))
        .map(|client| format!("{} #{}", client.label, client.id));
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.request_control();
    }
    let message = if allow_takeover {
        "Taking layout control".to_string()
    } else {
        match (already_requested, controller_label) {
            (true, Some(who)) => format!("Still waiting on {who} for layout control"),
            (true, None) => "Control request already pending".to_string(),
            (false, Some(who)) => format!("Requested layout control from {who}"),
            (false, None) => "Requested layout control".to_string(),
        }
    };
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        None,
        message,
    );
    Update::full()
}

/// Open the roster of everyone else on the session. The session-wide controls that go with it
/// (request control, input lock, takeover) are their own command-palette entries, not a menu here.
pub(crate) fn open_collaborators(ctx: &mut Context<AppRoot>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    ctx.state.show_palette = false;
    ctx.state.show_session_picker = false;
    ctx.state.collaboration = Some(crate::state::CollaborationState::new());
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::collaboration_key());
    Update::full()
}

/// Controller-only: remove the client at `index` in the roster. Destructive to someone else's
/// attachment, so it goes through the shared arm-then-confirm window ([`crate::ops::confirm`]) the
/// session kill and pane close use: the first press arms, a second within the window sends it, and
/// an arming left alone lapses.
pub(crate) fn evict_client(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
        return Update::full();
    }
    if target.id == shared.client_id {
        return Update::full();
    }
    let target_id = target.id;
    let label = format!("{} #{}", target.label, target.id);
    let armed = ctx
        .state
        .collaboration
        .as_ref()
        .is_some_and(|collaboration| collaboration.pending_kick == Some(target_id));
    if !armed {
        // First press only arms, on the same clock every other destructive gesture uses: the row
        // renders its own struck-through "again to kill" cue, and the arming lapses on its own
        // if the second press never comes.
        if let Some(collaboration) = ctx.state.collaboration.as_mut() {
            collaboration.pending_kick = Some(target_id);
        }
        return crate::ops::confirm::arm(ctx);
    }
    if let Some(collaboration) = ctx.state.collaboration.as_mut() {
        collaboration.pending_kick = None;
    }
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.evict_client(target_id);
    }
    crate::pty_events::notify_info(ctx, format!("Removed {label} from the session"));
    Update::full()
}

/// Whether this client can remove others: the writable controller of a session whose server is new
/// enough to understand the message.
pub(crate) fn can_evict(state: &crate::state::State) -> bool {
    state.current().shared.as_ref().is_some_and(|shared| {
        !shared.read_only && shared.is_controller() && state.current().session_client.is_some()
    })
}

/// Raise the follow prompt if this attach landed on a session another client is actively driving.
///
/// Following used to be what happened to whoever attached second, which is a poor way to learn that
/// your keyboard no longer shapes the layout. It is now a decision: watch along, ask for the lease,
/// or back out. A session with no active controller — including one whose only other client is
/// parked — needs no prompt, because attaching there gets control outright.
pub(crate) fn prompt_follow_if_occupied(ctx: &mut Context<AppRoot>) {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return;
    };
    // A read-only attach already said it is not here to drive the layout.
    if shared.read_only || shared.is_controller() {
        return;
    }
    let Some(controller) = shared.controller else {
        return;
    };
    let controller_label = shared
        .clients
        .iter()
        .find(|client| client.id == controller)
        .map(|client| client.label.clone())
        .unwrap_or_else(|| format!("client {controller}"));
    let Some(session) = ctx.state.current().session_name.clone() else {
        return;
    };
    let allow_takeover = shared.allow_takeover;
    ctx.state.show_palette = false;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.collaboration = None;
    ctx.state.follow_prompt = Some(crate::state::FollowPromptState {
        session,
        controller_label,
        allow_takeover,
        selected: 0,
    });
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::follow_prompt_key());
}

/// Apply the user's answer to the follow prompt. Cancelling leaves the session the way switching
/// away from it would, landing on the session picker when other choices remain, otherwise the
/// launcher.
pub(crate) fn resolve_follow_prompt(
    ctx: &mut Context<AppRoot>,
    choice: crate::state::FollowChoice,
) -> Update {
    ctx.state.follow_prompt = None;
    ctx.state.commands_dirty = true;
    match choice {
        crate::state::FollowChoice::Follow => {
            request_current_pane_focus(ctx);
            Update::full()
        }
        crate::state::FollowChoice::AskForControl => {
            request_current_pane_focus(ctx);
            request_control(ctx)
        }
        crate::state::FollowChoice::Cancel => {
            let name = ctx.state.current().session_name.clone();
            let detached_epoch = ctx.state.runtime_epoch;
            flush_layout_commit(ctx);
            crate::ops::exit::mark_session_detached(ctx, None);
            if let Some(client) = ctx.state.current().session_client.clone() {
                client.detach();
            }
            let update = crate::ops::session::attach::land_on_surviving_session(ctx);
            // Switching back temporarily parks the cancelled attachment. It was intentionally
            // detached, so retaining it would make discovery render the still-live server offline.
            ctx.state.background.remove(&detached_epoch);
            if let Some(name) = name {
                crate::pty_events::notify_info(ctx, format!("Left `{name}` alone"));
            }
            update
        }
    }
}

pub(crate) fn toggle_input_lock(ctx: &mut Context<AppRoot>) -> Update {
    if nudge_if_follower(ctx) {
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    let shared = ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked");
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.set_input_lock(!shared.input_locked);
    }
    Update::full()
}

pub(crate) fn toggle_control_takeover(ctx: &mut Context<AppRoot>) -> Update {
    if nudge_if_follower(ctx) {
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    if ctx.state.current().session_client.is_none() {
        return Update::full();
    }
    let allowed = !ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked")
        .allow_takeover;
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.set_control_takeover(allowed);
    }
    Update::full()
}

pub(crate) fn grant_control(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.read_only {
        crate::pty_events::notify_info(ctx, "Read-only clients cannot control the layout");
    } else if target.id != shared.client_id
        && let Some(client) = ctx.state.current().session_client.as_ref()
    {
        client.grant_control(target.id);
        ctx.state.collaboration = None;
    }
    Update::full()
}

/// Controller-only quick action: grant the lease to the client that requested it (the earliest
/// pending requester when several are waiting). Nudges a follower, and toasts when nothing is
/// pending, so the bound key always gives feedback.
pub(crate) fn grant_control_to_requester(ctx: &mut Context<AppRoot>) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
        return Update::full();
    }
    let target = shared
        .clients
        .iter()
        .filter(|client| {
            client.requesting_control && !client.read_only && client.id != shared.client_id
        })
        .min_by_key(|client| client.id)
        .map(|client| client.id);
    match target {
        Some(id) => {
            if let Some(client) = ctx.state.current().session_client.as_ref() {
                client.grant_control(id);
            }
            ctx.state.collaboration = None;
        }
        None => {
            crate::pty_events::notify_info(ctx, "No pending control requests");
        }
    }
    Update::full()
}

/// Controller-only: decline the pending control request from the client at `index` in the roster.
/// A no-op (with a follower nudge) when this client is not the controller, or when the target has no
/// pending request.
pub(crate) fn decline_control(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.requesting_control
        && target.id != shared.client_id
        && let Some(client) = ctx.state.current().session_client.as_ref()
    {
        client.decline_control(target.id);
    }
    Update::full()
}

pub(crate) const LAYOUT_COMMIT_DEBOUNCE_MS: u64 = 16;

pub(crate) fn schedule_layout_commit(ctx: &mut Context<AppRoot>) {
    if ctx.state.scratch_visible {
        return;
    }
    if !ctx.state.current().session_attached || !ctx.state.is_controller() {
        return;
    }
    let epoch = ctx.state.runtime_epoch;
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        flush_layout_commit(ctx);
        return;
    };
    if shared.layout_commit_scheduled {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        flush_layout_commit(ctx);
        return;
    };
    ctx.state
        .current_mut()
        .shared
        .as_mut()
        .expect("shared session checked above")
        .layout_commit_scheduled = true;
    // Re-armed after every message, so during sustained output this fires ~60 times a second.
    // `send_after` parks it on the shared timer thread instead of spawning one per window.
    link.send_after(
        std::time::Duration::from_millis(LAYOUT_COMMIT_DEBOUNCE_MS),
        crate::Msg::FlushLayoutCommit { epoch },
    );
}

/// If this client controls a shared session and its layout differs from the last commit, publish a
/// new [`SharedLayout`] at the optimistic base revision. The canonical canvas is this controller's
/// own pane canvas (viewport minus workbar), which followers letterbox to.
pub(crate) fn flush_layout_commit(ctx: &mut Context<AppRoot>) {
    if !ctx.state.current().session_attached || !ctx.state.is_controller() {
        return;
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let canvas = (
        bounds.w.round().max(1.0) as u16,
        bounds.h.round().max(1.0) as u16,
    );
    let layout = crate::shared_layout::shared_layout_from_state(&ctx.state, canvas);
    let Some(shared) = ctx.state.current_mut().shared.as_mut() else {
        return;
    };
    if shared.last_committed_layout.as_ref() == Some(&layout) {
        return;
    }
    let base_rev = shared.assumed_rev;
    client.commit_layout(base_rev, layout.clone());
    // Optimistically advance so a rapid burst of edits pipelines onto sequential base revisions;
    // the server's echo confirms `layout_rev`, and a reject resets `assumed_rev`.
    shared.assumed_rev = shared.assumed_rev.saturating_add(1);
    shared.last_committed_layout = Some(layout);
    shared.canonical_canvas = Some(canvas);
}
