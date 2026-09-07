use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::lifecycle::find_pane_mut;
use crate::state::PaneId;

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    // Followers never drive shared PTY size: they letterbox to the controller's canonical canvas
    // and their screens reshape only via the server's broadcast `Resized`. Owner-local panes
    // (scratch/popup) do not affect canonical shared sizing, so their owner may resize them.
    let local = crate::pane::lifecycle::pane_is_local(&ctx.state, id);
    if follower_resize_is_suppressed(&ctx.state, local) {
        return Update::none();
    }
    if let Some(update) = resize_scratch_pane(ctx, id, cols, rows) {
        return update;
    }
    // The pane rect updates immediately, but the client-side screen only reshapes on the server's
    // ordered `Resized` broadcast, so both parsers reshape at the same byte position.
    let client = ctx.state.current().session_client.clone();
    let attach_pending = ctx.state.current().pending_session_attach.is_some();
    if let Some(update) = validate_session_resize_target(ctx, id, client.is_none(), attach_pending)
    {
        return update;
    }
    // Keep pending geometry on the attachment rather than its transport-specific shared state: a
    // reconnect replaces the latter, but the widget may not report an unchanged viewport again.
    let resize_debounce_ms = ctx.state.config.pane.resize_debounce_ms;
    let held_by_drag = !local && ctx.state.shared_tiled_drag_in_flight();
    let Some((epoch, generation)) =
        queue_pending_resize(ctx, id, cols, rows, local, held_by_drag, resize_debounce_ms)
    else {
        return Update::none();
    };
    if resize_debounce_ms == 0 {
        flush_pending_resizes(ctx, generation);
        return Update::none();
    }
    Update::with_command(schedule_pane_resize_flush(
        epoch,
        generation,
        resize_debounce_ms,
    ))
}

fn follower_resize_is_suppressed(state: &crate::state::State, local: bool) -> bool {
    state.current().shared.is_some() && !state.is_controller() && !local
}

fn resize_scratch_pane(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Option<Update> {
    if !crate::scratchpad::contains(&ctx.state, id) {
        return None;
    }
    let client = ctx.state.scratch_client();
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Some(Update::none());
    };
    let Some(client) = client else {
        pane.terminal.status = ManagedTerminalStatus::Error("scratch runtime disconnected".into());
        return Some(Update::full());
    };
    client.resize(id, pane.pty_generation, true, cols.max(1), rows.max(1));
    Some(Update::none())
}

fn validate_session_resize_target(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    disconnected: bool,
    attach_pending: bool,
) -> Option<Update> {
    match find_pane_mut(&mut ctx.state, id) {
        Some(pane) if disconnected && !attach_pending => {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            Some(Update::full())
        }
        Some(_) => None,
        None => Some(Update::none()),
    }
}

fn queue_pending_resize(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    cols: u16,
    rows: u16,
    local: bool,
    held_by_drag: bool,
    resize_debounce_ms: u64,
) -> Option<(u64, u64)> {
    let epoch = ctx.state.runtime_epoch;
    let attachment = ctx.state.current_mut();
    if held_by_drag {
        attachment.drag_held_resizes.insert((local, id));
    } else {
        attachment.drag_held_resizes.remove(&(local, id));
    }
    attachment
        .pending_resizes
        .insert((local, id), (cols.max(1), rows.max(1)));
    if resize_debounce_ms > 0 {
        if attachment.resize_flush_scheduled {
            return None;
        }
        attachment.resize_flush_generation = attachment.resize_flush_generation.wrapping_add(1);
        attachment.resize_flush_scheduled = true;
    }
    Some((epoch, attachment.resize_flush_generation))
}

/// Re-arm the debounced flush for sizes that were held back, without sending anything now.
///
/// Called when a tiled drag ends. Most held panes report their settled geometry on the post-drop
/// render and re-arm the flush themselves, but a pane that ends the gesture at the size it took
/// when the drag started never changes again - the widget reports nothing, and its held entry
/// would sit in `pending_resizes` with no timer coming for it. Re-arming here rather than flushing
/// keeps the settled sizes, not the transient ones: the timer outlives the render that overwrites
/// them.
pub(crate) fn rearm_pending_resize_flush(ctx: &mut Context<AppRoot>) {
    let epoch = ctx.state.runtime_epoch;
    let debounce = ctx.state.config.pane.resize_debounce_ms.max(1);
    let pending = !ctx.state.current().pending_resizes.is_empty();
    let generation = ctx.state.current().resize_flush_generation.wrapping_add(1);
    ctx.state.current_mut().resize_flush_generation = generation;
    ctx.state.current_mut().resize_flush_scheduled = false;
    if !pending {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        // No runtime to park a timer on. Leave the sizes pending and the flush unarmed: flushing
        // here would send the transient geometry, because the render that settles it has not
        // happened yet.
        return;
    };
    ctx.state.current_mut().resize_flush_scheduled = true;
    link.send_after(
        std::time::Duration::from_millis(debounce),
        crate::Msg::FlushPaneResizes { epoch, generation },
    );
}

fn schedule_pane_resize_flush(epoch: u64, generation: u64, resize_debounce_ms: u64) -> Command {
    Command::after(
        std::time::Duration::from_millis(resize_debounce_ms),
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::FlushPaneResizes { epoch, generation });
        },
    )
}

/// Send the latest debounced size for every pane that still exists (see the controller debounce in
/// [`handle_pane_resize`]). Clears the pending set and re-arms scheduling.
///
/// A pending size is the only record of that pane's geometry there is: `client.resize` is reached
/// from here and from [`handle_pane_resize`] alone, both driven by the terminal widget, and the
/// widget reports a viewport only when it *changes*. Nothing re-derives one. So a size dropped here
/// leaves the PTY wrong until the pane's geometry happens to change again - which for a pane the
/// user is not currently resizing may be never.
///
/// Shared-namespace sizes are also held for the length of a tiled drag (see
/// [`State::shared_tiled_drag_in_flight`](crate::state::State::shared_tiled_drag_in_flight)).
/// Nothing is dropped: the entries stay pending, the drop's own geometry report overwrites them
/// under the same key, and the flush that follows the post-drop render sends the settled size once
/// instead of every frame of the gesture.
pub(crate) fn flush_pending_resizes(ctx: &mut Context<AppRoot>, generation: u64) {
    if ctx.state.current().resize_flush_generation != generation {
        return;
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        // Mid-attach or a reconnect window. Disarm so a later report can schedule a fresh flush,
        // but keep the sizes: `flush_pending_resizes` runs again once the client is installed.
        ctx.state.current_mut().resize_flush_scheduled = false;
        return;
    };
    let publishes_shared_sizes =
        ctx.state.is_controller() && !ctx.state.shared_tiled_drag_in_flight();
    let attachment = ctx.state.current_mut();
    attachment.resize_flush_scheduled = false;
    let pending: Vec<_> = attachment.pending_resizes.drain().collect();
    let (pending, retained): (Vec<_>, Vec<_>) = pending
        .into_iter()
        .partition(|((local, _), _)| *local || publishes_shared_sizes);
    for ((local, id), _) in &pending {
        attachment.drag_held_resizes.remove(&(*local, *id));
    }
    ctx.state.current_mut().pending_resizes.extend(retained);
    for ((local, id), (cols, rows)) in pending {
        if let Some(pane) =
            crate::pane::lifecycle::find_pane_in_namespace_mut(&mut ctx.state, id, local)
        {
            client.resize(id, pane.pty_generation, local, cols.max(1), rows.max(1));
        }
    }
}

/// Complete a timer that followed its attachment into the background. Parking releases the server
/// lease, so preserve shared geometry until this attachment returns and regains control.
pub(crate) fn flush_background_resizes(
    state: &mut crate::state::State,
    epoch: u64,
    generation: u64,
) -> Update {
    let Some(attachment) = state.background.get_mut(&epoch) else {
        return Update::none();
    };
    if attachment.resize_flush_generation != generation {
        return Update::none();
    }
    attachment.resize_flush_scheduled = false;
    // Popups are torn down on a switch and never belong to a background attachment. Scratch
    // resizes use the client runtime directly and never enter this map.
    attachment.pending_resizes.retain(|(local, _), _| !*local);
    Update::none()
}
