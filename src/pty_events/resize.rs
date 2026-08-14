use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::PaneId;

/// Trailing-edge debounce window for controller PTY resizes, coalescing a resize storm (drag,
/// tiling reflow) into one `pty.resize`/SIGWINCH per pane.
const RESIZE_DEBOUNCE_MS: u64 = 16;

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    // Followers never drive shared PTY size: they letterbox to the controller's canonical canvas
    // and their screens reshape only via the server's broadcast `Resized`. Owner-local panes
    // (scratch/popup) do not affect canonical shared sizing, so their owner may resize them.
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    if !ctx.state.is_controller() && !local {
        return Update::none();
    }
    // The pane rect updates immediately, but the client-side screen only reshapes on the server's
    // ordered `Resized` broadcast, so both parsers reshape at the same byte position.
    let client = ctx.state.current().session_client.clone();
    let generation = match find_pane_mut(&mut ctx.state, id) {
        Some(pane) => {
            if client.is_none() {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                return Update::full();
            }
            pane.pty_generation
        }
        None => return Update::none(),
    };
    // Debounce through the shared bookkeeping when attached: record the latest size and arm a single
    // trailing-edge flush. Without shared state (a brief unattached window), send immediately.
    let epoch = ctx.state.runtime_epoch;
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared
            .pending_resizes
            .insert((local, id), (cols.max(1), rows.max(1)));
        if shared.resize_flush_scheduled {
            return Update::none();
        }
        shared.resize_flush_scheduled = true;
        return Update::with_command(schedule_pane_resize_flush(epoch));
    }
    if let Some(client) = client {
        client.resize(id, generation, local, cols.max(1), rows.max(1));
    }
    Update::none()
}

fn schedule_pane_resize_flush(epoch: u64) -> Command {
    Command::after(
        std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS),
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::FlushPaneResizes { epoch });
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
pub(crate) fn flush_pending_resizes(ctx: &mut Context<AppRoot>) {
    let Some(client) = ctx.state.current().session_client.clone() else {
        // Mid-attach or a reconnect window. Disarm so a later report can schedule a fresh flush,
        // but keep the sizes: `flush_pending_resizes` runs again once the client is installed.
        if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
            shared.resize_flush_scheduled = false;
        }
        return;
    };
    let pending: Vec<((bool, PaneId), (u16, u16))> = match ctx.state.current_mut().shared.as_mut() {
        Some(shared) => {
            shared.resize_flush_scheduled = false;
            shared.pending_resizes.drain().collect()
        }
        None => return,
    };
    for ((local, id), (cols, rows)) in pending {
        if let Some(pane) =
            crate::pane_lifecycle::find_pane_in_namespace_mut(&mut ctx.state, id, local)
        {
            client.resize(id, pane.pty_generation, local, cols.max(1), rows.max(1));
        }
    }
}
