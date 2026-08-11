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
pub(crate) fn request_activation(state: &mut crate::state::State, pane_id: PaneId, slot_id: &str) {
    let Some(sender) = state.agent_slot_streams.get(&pane_id) else {
        return;
    };
    let line = format!("{}\n", serde_json::json!({ "activate": slot_id }));
    // A publisher that is not draining its activations is either wedged or gone; either way the
    // stream is no longer useful, so drop it rather than blocking the UI thread on it.
    if sender.try_send(line).is_err() {
        state.agent_slot_streams.remove(&pane_id);
    }
}

#[cfg(test)]
mod tests {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{AgentSlot, ClientMessage};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn slot(id: &str, status: &str) -> AgentSlot {
        AgentSlot {
            id: id.into(),
            title: format!("tab {id}"),
            status: status.into(),
            reason: None,
            active: true,
            work_started_at: None,
        }
    }

    /// Run on a worker thread with a large stack, matching the other `TestBackend` control tests.
    fn with_backend(body: impl FnOnce(&mut TestBackend<crate::HyprmuxApp>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                body(&mut backend);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn attach(backend: &mut TestBackend<crate::HyprmuxApp>) -> mpsc::Receiver<ClientOutbound> {
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        state.current_mut().workspaces[0].panes[0].pty_generation = 4;
        outbound
    }

    #[test]
    fn a_reported_slot_list_reaches_the_session_server() {
        with_backend(|backend| {
            let outbound = attach(backend);
            backend
                .dispatch(crate::Msg::AgentSlotsReported {
                    pane_id: 1,
                    slots: vec![slot("a", "working")],
                })
                .expect("dispatch slots");
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneSlots {
                    pane_id: 1,
                    generation: 4,
                    slots,
                }) if slots.len() == 1 && slots[0].id == "a"
            )));
        });
    }

    /// Closing the stream is how a publisher withdraws, including by dying, so it must reach the
    /// server as an explicit empty list rather than being left for a timeout to notice.
    #[test]
    fn closing_a_stream_withdraws_the_panes_slots() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            backend
                .dispatch(crate::Msg::AgentSlotStreamOpen {
                    pane_id: 1,
                    sender: tx,
                })
                .expect("dispatch stream open");
            backend
                .dispatch(crate::Msg::AgentSlotStreamClosed { pane_id: 1 })
                .expect("dispatch stream close");
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneSlots { slots, .. })
                    if slots.is_empty()
            )));
        });
    }

    #[test]
    fn activating_a_slot_writes_to_its_publisher() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            backend
                .dispatch(crate::Msg::AgentSlotStreamOpen {
                    pane_id: 1,
                    sender: tx,
                })
                .expect("dispatch stream open");
            super::request_activation(backend.state_mut(), 1, "ses_b");
            let line = rx.try_recv().expect("activation was written");
            assert_eq!(line.trim(), r#"{"activate":"ses_b"}"#);
        });
    }

    /// A publisher that stopped reading is wedged or gone; the UI thread must not block on it.
    #[test]
    fn a_full_activation_queue_drops_the_stream() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            backend
                .dispatch(crate::Msg::AgentSlotStreamOpen {
                    pane_id: 1,
                    sender: tx,
                })
                .expect("dispatch stream open");
            super::request_activation(backend.state_mut(), 1, "one");
            super::request_activation(backend.state_mut(), 1, "two");
            assert!(
                !backend.state_mut().agent_slot_streams.contains_key(&1),
                "a stream that stopped draining is dropped"
            );
            drop(rx);
        });
    }
}
