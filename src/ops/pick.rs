use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::ControlResponse;
use crate::ops::focus::{request_current_pane_focus, request_pick_focus};
use crate::state::{Mode, PickRow, PickState};

pub(crate) fn open_pick_stream(
    ctx: &mut Context<AppRoot>,
    id: u64,
    title: Option<String>,
    placeholder: Option<String>,
    sender: std::sync::mpsc::SyncSender<String>,
    ack: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    if ctx.state.show_pick {
        let _ = ack.send(ControlResponse::error("a picker is already open"));
        return Update::none();
    }
    if ctx.state.has_modal_overlay() {
        let _ = ack.send(ControlResponse::error("an overlay is open"));
        return Update::none();
    }
    let _ = ack.send(ControlResponse::empty());

    ctx.state.pick = Some(PickState {
        id,
        title: title.unwrap_or_else(|| "Pick".to_string()),
        placeholder: placeholder.unwrap_or_else(|| "Search…".to_string()),
        rows: Vec::new(),
        selected: 0,
        reply: sender,
    });
    ctx.state.show_pick = true;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.show_theme_picker = false;
    ctx.state.show_layout_picker = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_pick_focus(ctx);
    Update::full()
}

pub(crate) fn rows_reported(ctx: &mut Context<AppRoot>, id: u64, rows: Vec<PickRow>) -> Update {
    let Some(pick) = ctx.state.pick.as_mut().filter(|p| p.id == id) else {
        return Update::none();
    };
    pick.rows = rows;
    if pick.selected >= pick.rows.len() {
        pick.selected = 0;
    }
    Update::full()
}

pub(crate) fn stream_closed(ctx: &mut Context<AppRoot>, id: u64) -> Update {
    if ctx.state.pick.as_ref().is_some_and(|p| p.id == id) {
        ctx.state.pick = None;
        ctx.state.show_pick = false;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
        return Update::full();
    }
    Update::none()
}

pub(crate) fn close_pick(ctx: &mut Context<AppRoot>) -> Update {
    cancel_pick(ctx, None)
}

/// Cancel an open picker, telling the caller why.
///
/// `reason` separates a user pressing Esc from the client going away underneath them, which a
/// caller otherwise cannot tell apart - both arrive as a bare `cancelled`.
pub(crate) fn cancel_pick(ctx: &mut Context<AppRoot>, reason: Option<&str>) -> Update {
    if let Some(pick) = ctx.state.pick.take() {
        let payload = match reason {
            Some(reason) => serde_json::json!({ "cancelled": true, "reason": reason }),
            None => serde_json::json!({ "cancelled": true }),
        };
        let _ = pick.reply.try_send(format!("{payload}\n"));
        ctx.state.show_pick = false;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
        return Update::full();
    }
    Update::none()
}

pub(crate) fn pick_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(pick) = ctx.state.pick.as_mut() {
        pick.selected = index;
    }
    Update::none()
}

pub(crate) fn pick_activate(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Update::none();
    };
    let Some(row) = pick.rows.get(index) else {
        return Update::none();
    };
    if row.disabled.is_some() {
        return Update::none();
    }
    let selected_id = row.id.as_deref().unwrap_or(&row.label);
    let line = format!("{}\n", serde_json::json!({ "selected": selected_id }));
    let _ = pick.reply.try_send(line);

    ctx.state.pick = None;
    ctx.state.show_pick = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    use crate::state::PickRow;

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

    #[test]
    fn reported_rows_reach_state() {
        with_backend(|backend| {
            let (tx, _rx) = mpsc::sync_channel(1);
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: Some("Branches".into()),
                    placeholder: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let ack = ack_rx.recv().expect("ack received");
            assert!(ack.ok);

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    rows: vec![PickRow {
                        id: Some("main".into()),
                        label: "main".into(),
                        description: Some("default".into()),
                        group: Some("Local".into()),
                        disabled: None,
                        active: true,
                        priority: None,
                    }],
                })
                .expect("dispatch rows");

            let pick = backend.state().pick.as_ref().expect("pick state present");
            assert_eq!(pick.rows.len(), 1);
            assert_eq!(pick.rows[0].label, "main");
        });
    }

    #[test]
    fn activating_writes_selected_json() {
        with_backend(|backend| {
            let (tx, rx) = mpsc::sync_channel(1);
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    rows: vec![PickRow {
                        id: Some("feat/x".into()),
                        label: "Feature X".into(),
                        description: None,
                        group: None,
                        disabled: None,
                        active: false,
                        priority: None,
                    }],
                })
                .expect("dispatch rows");

            backend
                .dispatch(crate::Msg::PickActivate(0))
                .expect("dispatch activate");

            let line = rx.try_recv().expect("selection written");
            assert_eq!(line.trim(), r#"{"selected":"feat/x"}"#);
            assert!(!backend.state().show_pick);
        });
    }

    #[test]
    fn close_pick_writes_cancelled_json() {
        with_backend(|backend| {
            let (tx, rx) = mpsc::sync_channel(1);
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            backend
                .dispatch(crate::Msg::ClosePick)
                .expect("dispatch close");

            let line = rx.try_recv().expect("cancel written");
            assert_eq!(line.trim(), r#"{"cancelled":true}"#);
            assert!(!backend.state().show_pick);
        });
    }

    #[test]
    fn a_second_open_is_rejected() {
        with_backend(|backend| {
            let (tx1, _rx1) = mpsc::sync_channel(1);
            let (ack_tx1, ack_rx1) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    sender: tx1,
                    ack: ack_tx1,
                })
                .expect("dispatch open 1");
            assert!(ack_rx1.recv().unwrap().ok);

            let (tx2, _rx2) = mpsc::sync_channel(1);
            let (ack_tx2, ack_rx2) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 2,
                    title: None,
                    placeholder: None,
                    sender: tx2,
                    ack: ack_tx2,
                })
                .expect("dispatch open 2");
            let ack2 = ack_rx2.recv().unwrap();
            assert!(!ack2.ok);
            assert_eq!(ack2.error.as_deref(), Some("a picker is already open"));
        });
    }

    #[test]
    fn open_is_rejected_when_overlay_is_open() {
        with_backend(|backend| {
            backend.state_mut().show_help = true;

            let (tx, _rx) = mpsc::sync_channel(1);
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let ack = ack_rx.recv().unwrap();
            assert!(!ack.ok);
            assert_eq!(ack.error.as_deref(), Some("an overlay is open"));
        });
    }

    #[test]
    fn disabled_row_is_inert_on_activate() {
        with_backend(|backend| {
            let (tx, rx) = mpsc::sync_channel(1);
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    rows: vec![PickRow {
                        id: Some("locked".into()),
                        label: "Locked option".into(),
                        description: None,
                        group: None,
                        disabled: Some("Needs admin".into()),
                        active: false,
                        priority: None,
                    }],
                })
                .expect("dispatch rows");

            backend
                .dispatch(crate::Msg::PickActivate(0))
                .expect("dispatch activate");

            assert!(rx.try_recv().is_err());
            assert!(backend.state().show_pick);
        });
    }
}
