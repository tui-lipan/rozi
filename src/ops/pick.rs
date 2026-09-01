use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::ControlResponse;
use crate::ops::focus::{
    request_current_pane_focus, request_pick_focus, request_pick_prompt_focus,
};
use crate::state::{Mode, PickRow, PickState};

/// Narrowest and widest a caller may make the modal.
///
/// The floor keeps a label plus its right-aligned badge legible; the ceiling stops one picker
/// spanning a wide monitor when every built-in overlay sits near 60.
const PICK_MIN_WIDTH: u16 = 30;
const PICK_MAX_WIDTH: u16 = 120;
const PICK_DEFAULT_WIDTH: u16 = 60;

/// Everything a `pick` request carries, kept together so the message arm stays readable.
pub(crate) struct PickOpen {
    pub id: u64,
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub width: Option<u16>,
    pub actions: Vec<crate::state::PickAction>,
    pub extension: Option<crate::config::ExtensionProvenance>,
}

pub(crate) fn open_pick_stream(
    ctx: &mut Context<AppRoot>,
    open: PickOpen,
    sender: std::sync::mpsc::SyncSender<String>,
    ack: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let PickOpen {
        id,
        title,
        placeholder,
        width,
        actions,
        extension,
    } = open;
    if extension.as_ref().is_some_and(|provenance| {
        !crate::config::provenance_is_active(&ctx.state.extension_generations, provenance)
    }) {
        let _ = ack.send(ControlResponse::error("extension is not active"));
        return Update::none();
    }
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
        extension,
        title: title.unwrap_or_else(|| "Pick".to_string()),
        placeholder: placeholder.unwrap_or_else(|| "Search…".to_string()),
        width: width
            .unwrap_or(PICK_DEFAULT_WIDTH)
            .clamp(PICK_MIN_WIDTH, PICK_MAX_WIDTH),
        // An action whose key does not parse would be a footer hint that never fires, so drop it
        // here rather than advertising a chord the interceptor can never match.
        actions: actions
            .into_iter()
            .filter(|action| {
                !action.id.is_empty()
                    && tui_lipan::prelude::KeyBinding::from_str(&action.key).is_ok()
            })
            .collect(),
        prompt: None,
        query: String::new(),
        restore_query: String::new(),
        pending_action: None,
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
    // An arming survives a refresh only while its row does; otherwise the confirmation would be
    // aimed at whatever took that id's place.
    if let Some((_, row)) = pick.pending_action.clone() {
        let still_there = pick
            .rows
            .iter()
            .any(|candidate| candidate.id.as_ref().unwrap_or(&candidate.label) == &row);
        if !still_there {
            pick.pending_action = None;
        }
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

pub(crate) fn unload_extensions(
    ctx: &mut Context<AppRoot>,
    stale_extensions: &std::collections::HashSet<String>,
) -> Update {
    let stale = ctx
        .state
        .pick
        .as_ref()
        .and_then(|pick| pick.extension.as_ref())
        .is_some_and(|provenance| stale_extensions.contains(&provenance.id));
    if stale {
        cancel_pick(ctx, Some("extension unloaded"))
    } else {
        Update::none()
    }
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

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(pick) = ctx.state.pick.as_mut() {
        pick.query = query;
        // A filter change moves what is under the cursor, so an armed confirmation must not
        // survive it - the same reason moving the highlight disarms.
        pick.pending_action = None;
    }
    Update::none()
}

pub(crate) fn pick_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(pick) = ctx.state.pick.as_mut() {
        let moved = pick.selected != index;
        pick.selected = index;
        // Moving off the armed row disarms it, so a confirmation can never land on a row the user
        // has since navigated to.
        if moved && pick.pending_action.is_some() {
            pick.pending_action = None;
            return Update::full();
        }
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

/// Fire an action chord.
///
/// A plain action reports and leaves the palette up, so the caller can answer with a fresh row set:
/// deleting a branch and re-listing is one round trip, not a reopen. One carrying a `prompt` raises
/// the text modal first and reports on submit.
pub(crate) fn invoke_action(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Update::none();
    };
    let Some(action) = pick.actions.get(index).cloned() else {
        return Update::none();
    };

    if action.confirm {
        let row = pick
            .rows
            .get(pick.selected)
            .map(|row| row.id.clone().unwrap_or_else(|| row.label.clone()));
        let Some(row) = row else {
            return Update::none();
        };
        // Arm on the first press, fire on a second one aimed at the same row.
        if pick.pending_action.as_ref() != Some(&(index, row.clone())) {
            if let Some(pick) = ctx.state.pick.as_mut() {
                pick.pending_action = Some((index, row));
            }
            return Update::full();
        }
        if let Some(pick) = ctx.state.pick.as_mut() {
            pick.pending_action = None;
        }
    }

    if let Some(title) = action.prompt.clone() {
        if let Some(pick) = ctx.state.pick.as_mut() {
            // The picker unmounts while the prompt is up, so capture what to rebuild it with.
            pick.restore_query = pick.query.clone();
            pick.prompt = Some(crate::state::PickPrompt {
                action: index,
                title,
                input: tui_lipan::prelude::TextInput::new(""),
            });
        }
        ctx.state.commands_dirty = true;
        request_pick_prompt_focus(ctx);
        return Update::full();
    }

    report_action(ctx, index, None)
}

pub(crate) fn prompt_changed(
    ctx: &mut Context<AppRoot>,
    event: tui_lipan::prelude::InputEvent,
) -> Update {
    if let Some(prompt) = ctx
        .state
        .pick
        .as_mut()
        .and_then(|pick| pick.prompt.as_mut())
    {
        prompt.input.apply(&event);
    }
    Update::full()
}

pub(crate) fn prompt_submit(ctx: &mut Context<AppRoot>) -> Update {
    let Some((index, text)) = ctx
        .state
        .pick
        .as_mut()
        .and_then(|pick| pick.prompt.take())
        .map(|prompt| (prompt.action, prompt.input.text().to_string()))
    else {
        return Update::none();
    };
    report_action(ctx, index, Some(text))
}

/// Dismiss the prompt and go back to the picker underneath, reporting nothing: an abandoned prompt
/// is not a decision, and a caller that saw an action fire would have to undo it.
pub(crate) fn prompt_cancel(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(pick) = ctx.state.pick.as_mut() {
        pick.prompt = None;
    }
    ctx.state.commands_dirty = true;
    request_pick_focus(ctx);
    Update::full()
}

/// Write one action line, and close the picker when the action asked to be terminal.
fn report_action(ctx: &mut Context<AppRoot>, index: usize, input: Option<String>) -> Update {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Update::none();
    };
    let Some(action) = pick.actions.get(index).cloned() else {
        return Update::none();
    };
    // The row under the cursor rides along, so an action can be about a row without the caller
    // tracking the highlight itself.
    let selected = pick
        .rows
        .get(pick.selected)
        .map(|row| row.id.clone().unwrap_or_else(|| row.label.clone()));

    let mut payload = serde_json::json!({ "action": action.id });
    if let Some(map) = payload.as_object_mut() {
        map.insert(
            "selected".to_string(),
            match selected {
                Some(id) => serde_json::Value::String(id),
                None => serde_json::Value::Null,
            },
        );
        if let Some(text) = input {
            map.insert("input".to_string(), serde_json::Value::String(text));
        }
    }
    let _ = pick.reply.try_send(format!("{payload}\n"));

    if action.close {
        ctx.state.pick = None;
        ctx.state.show_pick = false;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
    } else {
        request_pick_focus(ctx);
    }
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
                    width: None,
                    actions: Vec::new(),
                    title: Some("Branches".into()),
                    placeholder: None,
                    extension: None,
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let ack = ack_rx.recv().unwrap();
            assert!(!ack.ok);
            assert_eq!(ack.error.as_deref(), Some("an overlay is open"));
        });
    }

    fn action(id: &str, key: &str, prompt: Option<&str>, close: bool) -> crate::state::PickAction {
        crate::state::PickAction {
            id: id.to_string(),
            key: key.to_string(),
            label: id.to_string(),
            prompt: prompt.map(str::to_string),
            close,
            confirm: false,
        }
    }

    fn open_with(
        backend: &mut TestBackend<crate::AppRoot>,
        actions: Vec<crate::state::PickAction>,
        width: Option<u16>,
    ) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::sync_channel(4);
        let (ack_tx, _ack_rx) = mpsc::channel();
        backend
            .dispatch(crate::Msg::PickStreamOpen {
                id: 1,
                title: None,
                placeholder: None,
                width,
                actions,
                extension: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");
        backend
            .dispatch(crate::Msg::PickRowsReported {
                id: 1,
                rows: vec![PickRow {
                    id: Some("feat/x".into()),
                    label: "feat/x".into(),
                    description: None,
                    group: None,
                    disabled: None,
                    active: false,
                    priority: None,
                }],
            })
            .expect("dispatch rows");
        rx
    }

    /// A plain action reports and leaves the palette up, so the caller can answer with a fresh row
    /// set instead of reopening the whole picker.
    #[test]
    fn a_plain_action_reports_the_row_and_keeps_the_picker_open() {
        with_backend(|backend| {
            let rx = open_with(backend, vec![action("delete", "ctrl-d", None, false)], None);
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("dispatch action");

            let line = rx.try_recv().expect("action written");
            assert_eq!(line.trim(), r#"{"action":"delete","selected":"feat/x"}"#);
            assert!(backend.state().show_pick, "picker closed on a plain action");
        });
    }

    /// `close: true` makes an action terminal, the same as a selection.
    #[test]
    fn a_closing_action_ends_the_picker() {
        with_backend(|backend| {
            let rx = open_with(backend, vec![action("edit", "ctrl-e", None, true)], None);
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("dispatch action");

            assert!(rx.try_recv().is_ok());
            assert!(!backend.state().show_pick);
        });
    }

    /// A prompt action reports nothing until the text is submitted, and carries it as `input`.
    #[test]
    fn a_prompt_action_reports_only_once_its_text_is_submitted() {
        with_backend(|backend| {
            let rx = open_with(
                backend,
                vec![action("create", "ctrl-n", Some("Branch name"), true)],
                None,
            );
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("dispatch action");
            assert!(
                rx.try_recv().is_err(),
                "reported before the prompt was answered"
            );
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.prompt.is_some()),
                "prompt did not open"
            );

            if let Some(prompt) = backend
                .state_mut()
                .pick
                .as_mut()
                .and_then(|pick| pick.prompt.as_mut())
            {
                prompt.input.set_text("feat/y".to_string());
            }
            backend
                .dispatch(crate::Msg::PickPromptSubmit)
                .expect("dispatch submit");

            let line = rx.try_recv().expect("action written");
            assert_eq!(
                line.trim(),
                r#"{"action":"create","input":"feat/y","selected":"feat/x"}"#
            );
        });
    }

    /// Abandoning the prompt is not a decision: nothing is reported and the list comes back.
    #[test]
    fn cancelling_a_prompt_reports_nothing_and_returns_to_the_picker() {
        with_backend(|backend| {
            let rx = open_with(
                backend,
                vec![action("create", "ctrl-n", Some("Branch name"), true)],
                None,
            );
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("dispatch action");
            backend
                .dispatch(crate::Msg::PickPromptCancel)
                .expect("dispatch cancel");

            assert!(rx.try_recv().is_err(), "a cancelled prompt reported");
            assert!(backend.state().show_pick, "picker did not come back");
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.prompt.is_none())
            );
        });
    }

    fn confirming(id: &str, key: &str) -> crate::state::PickAction {
        crate::state::PickAction {
            confirm: true,
            ..action(id, key, None, false)
        }
    }

    /// A `confirm` action arms on the first press and only reports on the second, the way the
    /// session picker's kill does.
    #[test]
    fn a_confirming_action_needs_a_second_press() {
        with_backend(|backend| {
            let rx = open_with(backend, vec![confirming("delete", "ctrl-d")], None);

            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("first press");
            assert!(rx.try_recv().is_err(), "fired on the first press");
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.pending_action.is_some()),
                "did not arm"
            );

            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("second press");
            let line = rx.try_recv().expect("reported on the second press");
            assert_eq!(line.trim(), r#"{"action":"delete","selected":"feat/x"}"#);
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.pending_action.is_none()),
                "stayed armed after firing"
            );
        });
    }

    /// Moving the highlight disarms, so a confirmation cannot land on a row navigated to after
    /// arming.
    #[test]
    fn moving_the_highlight_disarms_a_confirming_action() {
        with_backend(|backend| {
            let rx = open_with(backend, vec![confirming("delete", "ctrl-d")], None);
            backend.dispatch(crate::Msg::PickActionKey(0)).expect("arm");
            backend
                .dispatch(crate::Msg::PickSelect(0))
                .expect("same row is not a move");
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.pending_action.is_some()),
                "re-selecting the same row disarmed it"
            );

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    rows: vec![PickRow {
                        id: Some("other".into()),
                        label: "other".into(),
                        description: None,
                        group: None,
                        disabled: None,
                        active: false,
                        priority: None,
                    }],
                })
                .expect("refresh without the armed row");
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.pending_action.is_none()),
                "arming outlived the row it was aimed at"
            );
            assert!(rx.try_recv().is_err());
        });
    }

    /// The prompt replaces the picker rather than stacking on it, and cancelling rebuilds the
    /// picker seeded with the filter that was typed before.
    #[test]
    fn a_prompt_replaces_the_picker_and_restores_its_query() {
        with_backend(|backend| {
            open_with(
                backend,
                vec![action("create", "ctrl-n", Some("Branch name"), true)],
                None,
            );
            backend
                .dispatch(crate::Msg::PickQueryChanged("feat/".into()))
                .expect("typed a filter");
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("raise the prompt");

            let pick = backend.state().pick.as_ref().expect("session still open");
            assert!(pick.prompt.is_some(), "prompt did not open");
            assert_eq!(
                pick.restore_query, "feat/",
                "the filter was not captured for the rebuild"
            );

            backend
                .dispatch(crate::Msg::PickPromptCancel)
                .expect("dismiss the prompt");
            let pick = backend.state().pick.as_ref().expect("picker came back");
            assert!(pick.prompt.is_none());
            assert_eq!(pick.restore_query, "feat/", "rebuild lost the filter");
        });
    }

    /// Filtering moves what sits under the cursor, so it disarms for the same reason navigating
    /// does.
    #[test]
    fn changing_the_filter_disarms_a_confirming_action() {
        with_backend(|backend| {
            open_with(backend, vec![confirming("delete", "ctrl-d")], None);
            backend.dispatch(crate::Msg::PickActionKey(0)).expect("arm");
            backend
                .dispatch(crate::Msg::PickQueryChanged("oth".into()))
                .expect("filter");
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .is_some_and(|pick| pick.pending_action.is_none()),
                "arming survived a filter change"
            );
        });
    }

    /// Width is clamped, and an action whose chord cannot parse is dropped rather than becoming a
    /// footer hint that never fires.
    #[test]
    fn width_is_clamped_and_unparseable_actions_are_dropped() {
        with_backend(|backend| {
            open_with(
                backend,
                vec![
                    action("good", "ctrl-d", None, false),
                    action("bad", "not-a-key", None, false),
                ],
                Some(9999),
            );
            let pick = backend.state().pick.as_ref().expect("picker open");
            assert_eq!(pick.width, super::PICK_MAX_WIDTH);
            assert_eq!(pick.actions.len(), 1);
            assert_eq!(pick.actions[0].id, "good");
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
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: None,
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

    #[test]
    fn materially_unloaded_extension_cancels_its_open_picker() {
        with_backend(|backend| {
            let (tx, rx) = mpsc::sync_channel(1);
            let (ack_tx, ack_rx) = mpsc::channel();
            let provenance = crate::config::ExtensionProvenance {
                id: "git-tools".to_string(),
                generation: "generation-a".to_string(),
            };
            backend
                .state_mut()
                .extension_generations
                .insert(provenance.id.clone(), provenance.generation.clone());
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 7,
                    width: None,
                    actions: Vec::new(),
                    title: Some("Extension picker".into()),
                    placeholder: None,
                    extension: Some(provenance),
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            assert!(ack_rx.recv().unwrap().ok);

            backend
                .dispatch(crate::Msg::RunAction(
                    crate::input::Action::ReloadExtensions,
                ))
                .expect("reload without extension");
            assert!(!backend.state().show_pick);
            let response = rx.try_recv().expect("picker cancelled");
            assert!(response.contains("extension unloaded"), "{response}");
        });
    }

    #[test]
    fn inactive_extension_cannot_open_a_picker() {
        with_backend(|backend| {
            let (tx, _rx) = mpsc::sync_channel(1);
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 7,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    extension: Some(crate::config::ExtensionProvenance {
                        id: "git-tools".to_string(),
                        generation: "retired".to_string(),
                    }),
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let response = ack_rx.recv().unwrap();
            assert!(!response.ok);
            assert_eq!(response.error.as_deref(), Some("extension is not active"));
            assert!(!backend.state().show_pick);
        });
    }
}
