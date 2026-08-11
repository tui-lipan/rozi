//! The pane side of published agent slots.
//!
//! A program that runs several agents behind one terminal opens an `agent-slots` stream and keeps
//! it open: it writes a slot list whenever its own state changes, and reads back the activations a
//! user clicks in the sidebar. The stream's lifetime *is* the slots' lifetime, so a publisher that
//! exits or crashes withdraws its rows without needing to say so.

use tui_lipan::prelude::*;

use crate::app::HyprmuxApp;
use crate::session::protocol::AgentSlot;
use crate::state::PaneId;

/// Register a newly opened stream, replacing any stream the same pane had open.
///
/// One pane publishes one list, so a second stream means the first is stale - a publisher that
/// restarted inside a `keep_open` pane, say. Dropping the old sender closes it.
pub(crate) fn stream_opened(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    sender: std::sync::mpsc::SyncSender<String>,
) -> Update {
    ctx.state.agent_slot_streams.insert(pane_id, sender);
    Update::none()
}

/// Publish a pane's slots to the session server, which owns their run clocks and broadcasts them
/// to every attached client.
pub(crate) fn slots_reported(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    slots: Vec<AgentSlot>,
) -> Update {
    let Some(generation) = crate::pane_lifecycle::find_pane(&ctx.state, pane_id)
        .filter(|pane| !pane.closing)
        .map(|pane| pane.pty_generation)
    else {
        return Update::none();
    };
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.report_pane_slots(pane_id, generation, slots);
    }
    Update::none()
}

/// Withdraw a pane's slots because its publisher went away.
pub(crate) fn stream_closed(ctx: &mut Context<HyprmuxApp>, pane_id: PaneId) -> Update {
    if ctx.state.agent_slot_streams.remove(&pane_id).is_none() {
        return Update::none();
    }
    slots_reported(ctx, pane_id, Vec::new())
}

/// Ask a pane's publisher to bring one of its slots on screen.
///
/// Best-effort by design: hyprmux cannot move another program's view itself, and a publisher that
/// has stopped reading is indistinguishable from one that is slow. The pane is focused either way,
/// so the click is never a no-op.
pub(crate) fn request_activation(ctx: &mut Context<HyprmuxApp>, pane_id: PaneId, slot_id: &str) {
    let Some(sender) = ctx.state.agent_slot_streams.get(&pane_id) else {
        return;
    };
    let line = format!("{}\n", serde_json::json!({ "activate": slot_id }));
    // A publisher that is not draining its activations is either wedged or gone; either way the
    // stream is no longer useful, so drop it rather than blocking the UI thread on it.
    if sender.try_send(line).is_err() {
        ctx.state.agent_slot_streams.remove(&pane_id);
    }
}
