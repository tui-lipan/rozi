//! The pane side of published activity rows.
//!
//! A program that runs several agents or background tasks behind one terminal opens a `publish`
//! stream and keeps it open: it writes a row list whenever its own state changes, and reads back the
//! activations a user clicks in the sidebar. The stream's lifetime *is* the rows' lifetime, so a
//! publisher that exits or crashes withdraws its rows without needing to say so.

use tui_lipan::prelude::*;

use crate::app::AppRoot;
use crate::session::protocol::PublishedRow;
use crate::state::PaneId;

/// Register a newly opened stream, replacing any stream the same pane had open.
///
/// One pane publishes one list, so a second stream means the first is stale - a publisher that
/// restarted inside a `keep_open` pane, say. Dropping the old sender closes it.
pub(crate) fn stream_opened(
    ctx: &mut Context<AppRoot>,
    stream_id: u64,
    requested_pane: Option<PaneId>,
    extension: Option<crate::config::ExtensionProvenance>,
    sender: std::sync::mpsc::SyncSender<String>,
    ack: std::sync::mpsc::Sender<std::result::Result<PaneId, String>>,
) -> Update {
    if extension.as_ref().is_some_and(|provenance| {
        !crate::config::provenance_is_active(&ctx.state.extension_generations, provenance)
    }) {
        let _ = ack.send(Err("extension is not active".to_string()));
        return Update::none();
    }
    let Some(pane_id) = requested_pane.or_else(|| ctx.state.focused_pane()) else {
        let _ = ack.send(Err("publish requires a live pane".to_string()));
        return Update::none();
    };
    if crate::pane_lifecycle::find_pane(&ctx.state, pane_id).is_none() {
        let _ = ack.send(Err(format!("pane {pane_id} not found")));
        return Update::none();
    }
    ctx.state.publish_streams.insert(
        pane_id,
        crate::state::PublishStreamState {
            id: stream_id,
            sender,
            extension,
        },
    );
    let _ = ack.send(Ok(pane_id));
    Update::none()
}

/// Publish a pane's rows to the session server, which owns their run clocks and broadcasts them
/// to every attached client.
pub(crate) fn rows_reported(
    ctx: &mut Context<AppRoot>,
    stream_id: u64,
    pane_id: PaneId,
    rows: Vec<PublishedRow>,
) -> Update {
    if ctx
        .state
        .publish_streams
        .get(&pane_id)
        .is_none_or(|stream| stream.id != stream_id)
    {
        return Update::none();
    }
    report_rows(ctx, pane_id, rows)
}

fn report_rows(ctx: &mut Context<AppRoot>, pane_id: PaneId, rows: Vec<PublishedRow>) -> Update {
    let Some(generation) = crate::pane_lifecycle::find_pane(&ctx.state, pane_id)
        .filter(|pane| !pane.closing)
        .map(|pane| pane.pty_generation)
    else {
        return Update::none();
    };
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.report_pane_rows(
            pane_id,
            generation,
            crate::pane_lifecycle::pane_is_local(&ctx.state, pane_id),
            rows,
        );
    }
    Update::none()
}

/// Withdraw a pane's rows because its publisher went away.
pub(crate) fn stream_closed(ctx: &mut Context<AppRoot>, stream_id: u64, pane_id: PaneId) -> Update {
    if ctx
        .state
        .publish_streams
        .get(&pane_id)
        .is_none_or(|stream| stream.id != stream_id)
    {
        return Update::none();
    }
    ctx.state.publish_streams.remove(&pane_id);
    report_rows(ctx, pane_id, Vec::new())
}

/// Drop streams owned by extensions that disappeared or materially changed during reload.
pub(crate) fn unload_extensions(
    ctx: &mut Context<AppRoot>,
    stale_extensions: &std::collections::HashSet<String>,
) -> Update {
    let stale: Vec<_> = ctx
        .state
        .publish_streams
        .iter()
        .filter_map(|(pane_id, stream)| {
            stream
                .extension
                .as_ref()
                .is_some_and(|provenance| stale_extensions.contains(&provenance.id))
                .then_some(*pane_id)
        })
        .collect();
    for pane_id in &stale {
        ctx.state.publish_streams.remove(pane_id);
        report_rows(ctx, *pane_id, Vec::new());
    }
    if stale.is_empty() {
        Update::none()
    } else {
        Update::full()
    }
}

/// Ask a pane's publisher to bring one of its rows on screen.
///
/// Best-effort by design: rozi cannot move another program's view itself, and a publisher that
/// has stopped reading is indistinguishable from one that is slow. The pane is focused either way,
/// so the click is never a no-op.
pub(crate) fn request_activation(state: &mut crate::state::State, pane_id: PaneId, row_id: &str) {
    let Some(stream) = state.publish_streams.get(&pane_id) else {
        return;
    };
    let line = format!("{}\n", serde_json::json!({ "activate": row_id }));
    // A publisher that is not draining its activations is either wedged or gone; either way the
    // stream is no longer useful, so drop it rather than blocking the UI thread on it.
    if stream.sender.try_send(line).is_err() {
        state.publish_streams.remove(&pane_id);
    }
}

#[cfg(test)]
mod tests {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientMessage, PublishedRow};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn row(id: &str, status: &str) -> PublishedRow {
        PublishedRow {
            id: id.into(),
            title: format!("tab {id}"),
            status: status.into(),
            reason: None,
            active: true,
            work_started_at: None,
        }
    }

    /// Run on a worker thread with a large stack, matching the other `TestBackend` control tests.
    fn with_backend(body: impl FnOnce(&mut TestBackend<crate::AppRoot>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                body(&mut backend);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn attach(backend: &mut TestBackend<crate::AppRoot>) -> mpsc::Receiver<ClientOutbound> {
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        state.current_mut().workspaces[0].panes[0].pty_generation = 4;
        outbound
    }

    fn open_stream(
        backend: &mut TestBackend<crate::AppRoot>,
        stream_id: u64,
        requested_pane: Option<crate::state::PaneId>,
        extension: Option<&str>,
        sender: std::sync::mpsc::SyncSender<String>,
    ) {
        let (ack, reply) = mpsc::channel();
        let extension = extension.map(|id| crate::config::ExtensionProvenance {
            id: id.to_string(),
            generation: backend.state().extension_generations[id].clone(),
        });
        backend
            .dispatch(crate::Msg::PublishStreamOpen {
                stream_id,
                requested_pane,
                extension,
                sender,
                ack,
            })
            .expect("dispatch stream open");
        assert_eq!(reply.recv().unwrap(), Ok(1));
    }

    #[test]
    fn a_reported_row_list_reaches_the_session_server() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, 1, Some(1), None, tx);
            backend
                .dispatch(crate::Msg::PublishedRowsReported {
                    stream_id: 1,
                    pane_id: 1,
                    rows: vec![row("a", "working")],
                })
                .expect("dispatch rows");
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows {
                    pane_id: 1,
                    local: false,
                    generation: 4,
                    rows,
                }) if rows.len() == 1 && rows[0].id == "a"
            )));
        });
    }

    /// Closing the stream is how a publisher withdraws, including by dying, so it must reach the
    /// server as an explicit empty list rather than being left for a timeout to notice.
    #[test]
    fn closing_a_stream_withdraws_the_panes_rows() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, 1, Some(1), None, tx);
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 1,
                    pane_id: 1,
                })
                .expect("dispatch stream close");
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows { rows, .. })
                    if rows.is_empty()
            )));
        });
    }

    #[test]
    fn activating_a_row_writes_to_its_publisher() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            open_stream(backend, 1, Some(1), None, tx);
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
            open_stream(backend, 1, Some(1), None, tx);
            super::request_activation(backend.state_mut(), 1, "one");
            super::request_activation(backend.state_mut(), 1, "two");
            assert!(
                !backend.state_mut().publish_streams.contains_key(&1),
                "a stream that stopped draining is dropped"
            );
            drop(rx);
        });
    }

    #[test]
    fn stale_events_from_replaced_publisher_generation_are_ignored() {
        with_backend(|backend| {
            let outbound = attach(backend);
            backend
                .state_mut()
                .extension_generations
                .insert("dashboard".to_string(), "generation-a".to_string());
            let (old_tx, old_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, 1, Some(1), Some("dashboard"), old_tx);
            let (new_tx, _new_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, 2, Some(1), Some("dashboard"), new_tx);
            assert!(matches!(
                old_rx.recv_timeout(std::time::Duration::from_secs(1)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));

            backend
                .dispatch(crate::Msg::PublishedRowsReported {
                    stream_id: 1,
                    pane_id: 1,
                    rows: vec![row("stale", "working")],
                })
                .expect("stale rows");
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 1,
                    pane_id: 1,
                })
                .expect("stale close");
            assert_eq!(backend.state().publish_streams[&1].id, 2);
            assert!(!outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows { .. })
            )));

            backend
                .dispatch(crate::Msg::PublishedRowsReported {
                    stream_id: 2,
                    pane_id: 1,
                    rows: vec![row("current", "working")],
                })
                .expect("current rows");
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows { rows, .. })
                    if rows.first().is_some_and(|row| row.id == "current")
            )));
        });
    }

    #[test]
    fn service_publisher_uses_focused_pane_and_unloads_with_its_extension() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            backend
                .state_mut()
                .extension_generations
                .insert("dashboard".to_string(), "generation-a".to_string());
            open_stream(backend, 1, None, Some("dashboard"), tx);
            assert_eq!(
                backend
                    .state_mut()
                    .publish_streams
                    .get(&1)
                    .and_then(|stream| stream.extension.as_ref())
                    .map(|provenance| provenance.id.as_str()),
                Some("dashboard")
            );
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::ReloadConfig))
                .expect("reload without extension");
            assert!(matches!(
                rx.recv_timeout(std::time::Duration::from_secs(1)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows { rows, .. })
                    if rows.is_empty()
            )));
        });
    }

    #[test]
    fn inactive_extension_cannot_open_a_publish_stream() {
        with_backend(|backend| {
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            let (ack, reply) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PublishStreamOpen {
                    stream_id: 1,
                    requested_pane: None,
                    extension: Some(crate::config::ExtensionProvenance {
                        id: "dashboard".to_string(),
                        generation: "retired".to_string(),
                    }),
                    sender: tx,
                    ack,
                })
                .expect("dispatch stream open");
            assert_eq!(
                reply.recv().unwrap(),
                Err("extension is not active".to_string())
            );
            assert!(backend.state().publish_streams.is_empty());
        });
    }
}
