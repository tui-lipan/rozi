use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::ControlResponse;
use crate::ops::focus::{
    request_current_pane_focus, request_pick_focus, request_pick_prompt_focus,
};
use crate::state::{Mode, PickPage, PickReply, PickRow, PickState, PickTab};

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
    pub empty: Option<String>,
    pub width: Option<u16>,
    pub actions: Vec<crate::state::PickAction>,
    pub tabs: Vec<PickTab>,
    pub tab: Option<String>,
    pub extension: Option<crate::config::ExtensionProvenance>,
}

/// The pages a picker opens with: one per usable declared tab, or the single implicit page.
///
/// A tab with an empty id, or one repeating an earlier id, is dropped for the same reason an
/// unparseable action is: rows and replies address tabs by id, so it could never be told apart.
fn opening_pages(tabs: &[PickTab]) -> Vec<PickPage> {
    let mut pages: Vec<PickPage> = crate::state::usable_pick_tabs(tabs)
        .into_iter()
        .map(|tab| PickPage {
            label: tab
                .label
                .clone()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| tab.id.clone()),
            tab: Some(tab.id.clone()),
            ..PickPage::default()
        })
        .collect();
    if pages.is_empty() {
        pages.push(PickPage::default());
    }
    pages
}

pub(crate) fn open_pick_stream(
    ctx: &mut Context<AppRoot>,
    open: PickOpen,
    sender: PickReply,
    ack: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let PickOpen {
        id,
        title,
        placeholder,
        empty,
        width,
        actions,
        tabs,
        tab,
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
    let replacing_command_palette =
        ctx.state.show_palette && ctx.state.command_palette_handoff.is_some();
    if ctx.state.has_modal_overlay() && !replacing_command_palette {
        let _ = ack.send(ControlResponse::error("an overlay is open"));
        return Update::none();
    }
    let _ = ack.send(ControlResponse::empty());

    let pages = opening_pages(&tabs);
    let active = tab
        .and_then(|tab| {
            pages
                .iter()
                .position(|page| page.tab.as_ref() == Some(&tab))
        })
        .unwrap_or(0);
    ctx.state.pick = Some(PickState {
        id,
        extension,
        title: title.unwrap_or_else(|| "Pick".to_string()),
        placeholder: placeholder.unwrap_or_else(|| "Search…".to_string()),
        empty: empty.filter(|text| !text.is_empty()),
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
        pending_action: None,
        pages,
        active,
        reply: sender,
    });
    ctx.state.show_pick = true;
    ctx.state.command_palette_handoff = None;
    ctx.state.keybindings = None;
    ctx.state.show_palette = false;
    ctx.state.command_palette_sidebar_query = false;
    ctx.state.show_theme_picker = false;
    ctx.state.show_layout_picker = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_pick_focus(ctx);
    Update::full()
}

/// Replace one page's rows.
///
/// A tabbed picker needs the snapshot to name its tab, and an untabbed one needs it not to: a
/// snapshot aimed at no page, or at a tab that was never declared, is dropped like any other line
/// that does not parse, rather than landing on whichever page happens to be showing.
pub(crate) fn rows_reported(
    ctx: &mut Context<AppRoot>,
    id: u64,
    tab: Option<String>,
    rows: Vec<PickRow>,
) -> Update {
    let Some(pick) = ctx.state.pick.as_mut().filter(|p| p.id == id) else {
        return Update::none();
    };
    let Some(index) = pick.pages.iter().position(|page| page.tab == tab) else {
        return Update::none();
    };
    let page = &mut pick.pages[index];
    page.rows = rows;
    if page.selected >= page.rows.len() {
        page.selected = 0;
    }
    if index != pick.active {
        // Filled out of sight. The strip labels do not change with the rows, so there is nothing
        // to redraw until the user switches to it.
        return Update::none();
    }
    // An arming survives a refresh only while its row does; otherwise the confirmation would be
    // aimed at whatever took that id's place.
    if let Some((_, row)) = pick.pending_action.clone() {
        let still_there = pick
            .page()
            .rows
            .iter()
            .any(|candidate| row_id(candidate) == row);
        if !still_there {
            pick.pending_action = None;
        }
    }
    Update::full()
}

/// Show another page, and tell the caller so it can fill a tab only once someone looks at it.
pub(crate) fn tab_selected(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(pick) = ctx.state.pick.as_mut() else {
        return Update::none();
    };
    if index == pick.active || index >= pick.pages.len() || pick.prompt.is_some() {
        return Update::none();
    }
    // The armed row belongs to the page being left.
    pick.pending_action = None;
    pick.active = index;
    // The palette remounts for the new page, so seed it with what was typed there last time.
    let page = pick.page_mut();
    page.restore_query = page.query.clone();
    if let Some(tab) = page.tab.as_deref() {
        let payload = serde_json::json!({ "tab": tab });
        pick.reply.event(&payload);
    }
    request_pick_focus(ctx);
    Update::full()
}

/// What a row reports as `selected`: its id, or its label when it has none.
fn row_id(row: &PickRow) -> &str {
    row.id.as_deref().unwrap_or(&row.label)
}

/// Add the active tab to a reply, so ids only need to be unique within their tab.
fn with_tab(pick: &PickState, mut payload: serde_json::Value) -> serde_json::Value {
    if let (Some(tab), Some(map)) = (pick.page().tab.as_deref(), payload.as_object_mut()) {
        map.insert(
            "tab".to_string(),
            serde_json::Value::String(tab.to_string()),
        );
    }
    payload
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
        pick.reply.finish(&payload);
        ctx.state.show_pick = false;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
        return Update::full();
    }
    Update::none()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    let was_empty = ctx
        .state
        .pick
        .as_ref()
        .is_some_and(|pick| pick.page().query.trim().is_empty());
    let Some(pick) = ctx.state.pick.as_mut() else {
        return Update::none();
    };
    pick.page_mut().query = query;
    let is_empty = pick.page().query.trim().is_empty();
    let disarmed = pick.pending_action.take().is_some();
    if was_empty != is_empty || disarmed {
        Update::full()
    } else {
        Update::none()
    }
}

pub(crate) fn pick_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(pick) = ctx.state.pick.as_mut() {
        let page = pick.page_mut();
        let moved = page.selected != index;
        page.selected = index;
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
    let Some(row) = pick.page().rows.get(index) else {
        return Update::none();
    };
    if row.disabled.is_some() {
        return Update::none();
    }
    let payload = with_tab(pick, serde_json::json!({ "selected": row_id(row) }));
    close_with(ctx, &payload);
    Update::full()
}

/// End the picker with its terminal line, which [`PickReply::finish`] guarantees reaches the
/// caller however full the event queue is.
fn close_with(ctx: &mut Context<AppRoot>, payload: &serde_json::Value) {
    if let Some(pick) = ctx.state.pick.take() {
        pick.reply.finish(payload);
    }
    ctx.state.show_pick = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
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
        let row = visible_selected_row(pick).map(|row| row_id(row).to_string());
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

    if let Some(spec) = action.prompt {
        if let Some(pick) = ctx.state.pick.as_mut() {
            // The picker unmounts while the prompt is up, so capture what to rebuild it with.
            let page = pick.page_mut();
            page.restore_query = page.query.clone();
            pick.prompt = Some(crate::state::PickPrompt {
                action: index,
                title: spec.title().to_string(),
                placeholder: spec.placeholder().to_string(),
                masked: spec.masked(),
                input: tui_lipan::prelude::TextInput::new(spec.value()),
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
    let selected = visible_selected_row(pick).map(|row| row_id(row).to_string());

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
    let payload = with_tab(pick, payload);

    if action.close {
        close_with(ctx, &payload);
    } else {
        pick.reply.event(&payload);
        request_pick_focus(ctx);
    }
    Update::full()
}

fn visible_selected_row(pick: &crate::state::PickState) -> Option<&crate::state::PickRow> {
    let page = pick.page();
    let row = page.rows.get(page.selected)?;
    if page.query.trim().is_empty() {
        return Some(row);
    }
    let description = row
        .disabled
        .as_deref()
        .or(row.description.as_deref())
        .unwrap_or("");
    let items = [SearchItem::new(row.label.as_str(), ())
        .description(ItemDescription::new().right(description))];
    (!tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        &page.query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    )
    .is_empty())
    .then_some(row)
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
                crate::test_support::isolate_user_dirs();
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
            let (tx, _rx) = crate::state::PickReply::channel();
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: Some("Branches".into()),
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let ack = ack_rx.recv().expect("ack received");
            assert!(ack.ok);

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    tab: None,
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
            assert_eq!(pick.page().rows.len(), 1);
            assert_eq!(pick.page().rows[0].label, "main");
        });
    }

    #[test]
    fn pick_stream_replaces_command_palette_handoff() {
        with_backend(|backend| {
            backend.state_mut().show_palette = true;
            backend.state_mut().command_palette_handoff = Some(7);

            let (tx, _rx) = crate::state::PickReply::channel();
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: Some("Snippets".into()),
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            let ack = ack_rx.recv().expect("ack received");
            assert!(ack.ok);
            assert!(backend.state().show_pick);
            assert!(!backend.state().show_palette);
            assert!(backend.state().command_palette_handoff.is_none());
            assert_eq!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .expect("pick state present")
                    .title,
                "Snippets"
            );
        });
    }

    #[test]
    fn activating_writes_selected_json() {
        with_backend(|backend| {
            let (tx, rx) = crate::state::PickReply::channel();
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    tab: None,
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
            let (tx, rx) = crate::state::PickReply::channel();
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
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
            let (tx1, _rx1) = crate::state::PickReply::channel();
            let (ack_tx1, ack_rx1) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx1,
                    ack: ack_tx1,
                })
                .expect("dispatch open 1");
            assert!(ack_rx1.recv().unwrap().ok);

            let (tx2, _rx2) = crate::state::PickReply::channel();
            let (ack_tx2, ack_rx2) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 2,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
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
            backend.state_mut().keybindings = Some(Default::default());

            let (tx, _rx) = crate::state::PickReply::channel();
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
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
            prompt: prompt.map(|title| crate::state::PickPromptSpec::Title(title.to_string())),
            close,
            confirm: false,
        }
    }

    fn open_with(
        backend: &mut TestBackend<crate::AppRoot>,
        actions: Vec<crate::state::PickAction>,
        width: Option<u16>,
    ) -> crate::state::PickReplyReceiver {
        let (tx, rx) = crate::state::PickReply::channel();
        let (ack_tx, _ack_rx) = mpsc::channel();
        backend
            .dispatch(crate::Msg::PickStreamOpen {
                id: 1,
                title: None,
                placeholder: None,
                empty: None,
                width,
                actions,
                extension: None,
                tabs: Vec::new(),
                tab: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");
        backend
            .dispatch(crate::Msg::PickRowsReported {
                id: 1,
                tab: None,
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

    #[test]
    fn an_action_cannot_report_a_filtered_out_row() {
        with_backend(|backend| {
            let rx = open_with(backend, vec![action("create", "ctrl-n", None, false)], None);
            backend
                .dispatch(crate::Msg::PickQueryChanged("no-match".into()))
                .expect("filter out selected row");
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("dispatch global action");

            let line = rx.try_recv().expect("action written");
            assert_eq!(line.trim(), r#"{"action":"create","selected":null}"#);
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

            assert!(rx.try_recv().is_some());
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
                rx.try_recv().is_none(),
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

            assert!(rx.try_recv().is_none(), "a cancelled prompt reported");
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
            assert!(rx.try_recv().is_none(), "fired on the first press");
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
                    tab: None,
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
            assert!(rx.try_recv().is_none());
        });
    }

    /// A stacked prompt keeps the picker underneath; cancelling restores it seeded with the
    /// filter that was typed before.
    #[test]
    fn a_stacked_prompt_restores_the_picker_query() {
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
                pick.page().restore_query,
                "feat/",
                "the filter was not captured for the rebuild"
            );

            backend
                .dispatch(crate::Msg::PickPromptCancel)
                .expect("dismiss the prompt");
            let pick = backend.state().pick.as_ref().expect("picker came back");
            assert!(pick.prompt.is_none());
            assert_eq!(
                pick.page().restore_query,
                "feat/",
                "rebuild lost the filter"
            );
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

    fn prompt_fields(
        title: &str,
        placeholder: Option<&str>,
        value: Option<&str>,
        masked: bool,
    ) -> crate::state::PickAction {
        crate::state::PickAction {
            id: "edit".into(),
            key: "ctrl-e".into(),
            label: "edit".into(),
            prompt: Some(crate::state::PickPromptSpec::Fields(
                crate::state::PickPromptFields {
                    title: title.into(),
                    placeholder: placeholder.map(str::to_string),
                    value: value.map(str::to_string),
                    masked,
                },
            )),
            close: false,
            confirm: false,
        }
    }

    #[test]
    fn producer_empty_copy_is_kept_for_an_empty_filter() {
        with_backend(|backend| {
            let (tx, _rx) = crate::state::PickReply::channel();
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    title: None,
                    placeholder: None,
                    empty: Some("No snippets yet".into()),
                    width: None,
                    actions: Vec::new(),
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");
            let pick = backend.state().pick.as_ref().expect("picker open");
            assert_eq!(pick.empty.as_deref(), Some("No snippets yet"));
            backend
                .dispatch(crate::Msg::PickQueryChanged("feat".into()))
                .expect("type a filter");
            assert_eq!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .map(|pick| pick.page().query.as_str()),
                Some("feat")
            );
        });
    }

    #[test]
    fn a_prompt_object_seeds_placeholder_value_and_masking() {
        with_backend(|backend| {
            open_with(
                backend,
                vec![prompt_fields(
                    "Edit command",
                    Some("git status"),
                    Some("git status --short"),
                    true,
                )],
                None,
            );
            backend
                .dispatch(crate::Msg::PickActionKey(0))
                .expect("raise the prompt");
            let prompt = backend
                .state()
                .pick
                .as_ref()
                .and_then(|pick| pick.prompt.as_ref())
                .expect("prompt open");
            assert_eq!(prompt.title, "Edit command");
            assert_eq!(prompt.placeholder, "git status");
            assert_eq!(prompt.input.text(), "git status --short");
            assert!(prompt.masked);
        });
    }

    #[test]
    fn disabled_row_is_inert_on_activate() {
        with_backend(|backend| {
            let (tx, rx) = crate::state::PickReply::channel();
            let (ack_tx, _ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 1,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: None,
                    tabs: Vec::new(),
                    tab: None,
                    sender: tx,
                    ack: ack_tx,
                })
                .expect("dispatch open");

            backend
                .dispatch(crate::Msg::PickRowsReported {
                    id: 1,
                    tab: None,
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

            assert!(rx.try_recv().is_none());
            assert!(backend.state().show_pick);
        });
    }

    #[test]
    fn materially_unloaded_extension_cancels_its_open_picker() {
        with_backend(|backend| {
            let (tx, rx) = crate::state::PickReply::channel();
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
                    empty: None,
                    extension: Some(provenance),
                    tabs: Vec::new(),
                    tab: None,
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
            let (tx, _rx) = crate::state::PickReply::channel();
            let (ack_tx, ack_rx) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PickStreamOpen {
                    id: 7,
                    width: None,
                    actions: Vec::new(),
                    title: None,
                    placeholder: None,
                    empty: None,
                    extension: Some(crate::config::ExtensionProvenance {
                        id: "git-tools".to_string(),
                        generation: "retired".to_string(),
                    }),
                    tabs: Vec::new(),
                    tab: None,
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

    fn tab(id: &str, label: Option<&str>) -> crate::state::PickTab {
        crate::state::PickTab {
            id: id.into(),
            label: label.map(str::to_string),
        }
    }

    fn row(id: &str) -> PickRow {
        PickRow {
            id: Some(id.into()),
            label: id.into(),
            description: None,
            group: None,
            disabled: None,
            active: false,
            priority: None,
        }
    }

    fn open_tabbed(
        backend: &mut TestBackend<crate::AppRoot>,
        tabs: Vec<crate::state::PickTab>,
        tab: Option<&str>,
        actions: Vec<crate::state::PickAction>,
    ) -> crate::state::PickReplyReceiver {
        let (tx, rx) = crate::state::PickReply::channel();
        let (ack_tx, ack_rx) = mpsc::channel();
        backend
            .dispatch(crate::Msg::PickStreamOpen {
                id: 1,
                title: Some("Git".into()),
                placeholder: None,
                empty: None,
                width: None,
                actions,
                tabs,
                tab: tab.map(str::to_string),
                extension: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");
        assert!(ack_rx.recv().unwrap().ok);
        rx
    }

    fn report(backend: &mut TestBackend<crate::AppRoot>, tab: Option<&str>, ids: &[&str]) {
        backend
            .dispatch(crate::Msg::PickRowsReported {
                id: 1,
                tab: tab.map(str::to_string),
                rows: ids.iter().map(|id| row(id)).collect(),
            })
            .expect("dispatch rows");
    }

    fn page_ids(backend: &TestBackend<crate::AppRoot>) -> Vec<Vec<String>> {
        backend
            .state()
            .pick
            .as_ref()
            .expect("pick open")
            .pages
            .iter()
            .map(|page| page.rows.iter().map(|row| row.label.clone()).collect())
            .collect()
    }

    /// Tabs are addressed by id, so one that cannot be told apart from another is dropped, and
    /// the picker opens on the tab asked for when it exists.
    #[test]
    fn declared_tabs_become_pages_and_open_on_the_requested_one() {
        with_backend(|backend| {
            open_tabbed(
                backend,
                vec![
                    tab("branches", Some("Branches")),
                    tab("", Some("Nameless")),
                    tab("tags", None),
                    tab("branches", Some("Again")),
                ],
                Some("tags"),
                Vec::new(),
            );
            let pick = backend.state().pick.as_ref().unwrap();
            assert!(pick.tabbed());
            let labels: Vec<&str> = pick.pages.iter().map(|page| page.label.as_str()).collect();
            assert_eq!(labels, ["Branches", "tags"]);
            assert_eq!(pick.active, 1);
        });
        with_backend(|backend| {
            open_tabbed(
                backend,
                vec![tab("branches", None), tab("tags", None)],
                Some("missing"),
                Vec::new(),
            );
            assert_eq!(backend.state().pick.as_ref().unwrap().active, 0);
        });
    }

    /// A snapshot fills the page it names and nothing else, so a slow producer can never paint
    /// one tab's rows into whichever tab the user has since switched to.
    #[test]
    fn rows_land_only_on_the_tab_they_name() {
        with_backend(|backend| {
            open_tabbed(
                backend,
                vec![tab("branches", None), tab("tags", None)],
                None,
                Vec::new(),
            );
            report(backend, Some("tags"), &["v1"]);
            report(backend, None, &["stray"]);
            report(backend, Some("worktrees"), &["stray"]);
            assert_eq!(page_ids(backend), [Vec::<String>::new(), vec!["v1".into()]]);
        });
        with_backend(|backend| {
            open_tabbed(backend, Vec::new(), None, Vec::new());
            assert!(!backend.state().pick.as_ref().unwrap().tabbed());
            report(backend, Some("tags"), &["stray"]);
            report(backend, None, &["main"]);
            assert_eq!(page_ids(backend), [vec!["main".to_string()]]);
        });
    }

    /// Each tab keeps its own filter and highlight, and the producer hears about every switch so
    /// it can fill a tab only once someone looks at it.
    #[test]
    fn each_tab_keeps_its_own_filter_and_highlight() {
        with_backend(|backend| {
            let rx = open_tabbed(
                backend,
                vec![tab("branches", None), tab("tags", None)],
                None,
                Vec::new(),
            );
            report(backend, Some("branches"), &["main", "dev"]);
            backend
                .dispatch(crate::Msg::PickQueryChanged("de".into()))
                .unwrap();
            backend.dispatch(crate::Msg::PickSelect(1)).unwrap();

            backend.dispatch(crate::Msg::PickTabSelected(1)).unwrap();
            assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"tags"}"#);
            let pick = backend.state().pick.as_ref().unwrap();
            assert_eq!(pick.page().query, "");
            assert_eq!(pick.page().restore_query, "");

            // Choosing the tab already showing is not a switch.
            backend.dispatch(crate::Msg::PickTabSelected(1)).unwrap();
            assert!(rx.try_recv().is_none());

            backend.dispatch(crate::Msg::PickTabSelected(0)).unwrap();
            assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"branches"}"#);
            let page = backend.state().pick.as_ref().unwrap().page();
            assert_eq!(page.restore_query, "de", "the filter was not brought back");
            assert_eq!(page.selected, 1, "the highlight was not brought back");
        });
    }

    /// An armed row belongs to its page, and every reply names the tab it came from, so row ids
    /// only need to be unique within a tab.
    #[test]
    fn switching_disarms_and_replies_carry_the_tab() {
        with_backend(|backend| {
            let rx = open_tabbed(
                backend,
                vec![tab("branches", None), tab("tags", None)],
                None,
                vec![confirming("delete", "ctrl-d")],
            );
            report(backend, Some("branches"), &["main"]);
            report(backend, Some("tags"), &["main"]);

            backend.dispatch(crate::Msg::PickActionKey(0)).unwrap();
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .unwrap()
                    .pending_action
                    .is_some()
            );
            backend.dispatch(crate::Msg::PickTabSelected(1)).unwrap();
            assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"tags"}"#);
            assert!(
                backend
                    .state()
                    .pick
                    .as_ref()
                    .unwrap()
                    .pending_action
                    .is_none(),
                "the arming followed the user to another tab"
            );

            backend.dispatch(crate::Msg::PickActionKey(0)).unwrap();
            backend.dispatch(crate::Msg::PickActionKey(0)).unwrap();
            assert_eq!(
                rx.try_recv().unwrap().trim(),
                r#"{"action":"delete","selected":"main","tab":"tags"}"#
            );

            backend.dispatch(crate::Msg::PickActivate(0)).unwrap();
            assert_eq!(
                rx.try_recv().unwrap().trim(),
                r#"{"selected":"main","tab":"tags"}"#
            );
        });
    }

    /// Drain everything the picker sent, in order.
    fn drain(rx: &crate::state::PickReplyReceiver) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv())
            .map(|line| line.trim().to_string())
            .collect()
    }

    /// Flood the event queue with tab switches nobody reads, the way key repeat on Tab can while
    /// the producer is busy.
    fn saturate(backend: &mut TestBackend<crate::AppRoot>) {
        for switch in 0..crate::state::PICK_REPLY_BACKLOG * 2 {
            backend
                .dispatch(crate::Msg::PickTabSelected((switch + 1) % 2))
                .unwrap();
        }
    }

    /// Once rozi closes a picker, the caller hears why, however far behind on reading it is.
    /// Dropping the terminal line would leave it waiting on a picker that is already gone.
    #[test]
    fn a_full_event_queue_never_costs_the_terminal_line() {
        let tabs = || vec![tab("branches", None), tab("tags", None)];
        with_backend(move |backend| {
            let rx = open_tabbed(backend, tabs(), None, Vec::new());
            report(backend, Some("branches"), &["main"]);
            saturate(backend);
            backend.dispatch(crate::Msg::PickActivate(0)).unwrap();
            let lines = drain(&rx);
            assert_eq!(lines.len(), crate::state::PICK_REPLY_BACKLOG + 1);
            assert_eq!(
                lines.last().map(String::as_str),
                Some(r#"{"selected":"main","tab":"branches"}"#)
            );
        });
        with_backend(move |backend| {
            let rx = open_tabbed(
                backend,
                tabs(),
                None,
                vec![action("open", "ctrl-o", None, true)],
            );
            report(backend, Some("branches"), &["main"]);
            saturate(backend);
            backend.dispatch(crate::Msg::PickActionKey(0)).unwrap();
            assert_eq!(
                drain(&rx).last().map(String::as_str),
                Some(r#"{"action":"open","selected":"main","tab":"branches"}"#)
            );
        });
        with_backend(move |backend| {
            let rx = open_tabbed(backend, tabs(), None, Vec::new());
            saturate(backend);
            backend.dispatch(crate::Msg::ClosePick).unwrap();
            assert_eq!(
                drain(&rx).last().map(String::as_str),
                Some(r#"{"cancelled":true}"#)
            );
        });
    }

    /// Tabs live on the UI thread, so a producer cannot declare an unbounded number of them.
    #[test]
    fn tabs_are_capped_and_deduplicated() {
        with_backend(|backend| {
            let mut tabs: Vec<_> = (0..crate::state::MAX_PICK_TABS * 4)
                .map(|index| {
                    tab(
                        &format!("t{}", index % (crate::state::MAX_PICK_TABS + 8)),
                        None,
                    )
                })
                .collect();
            tabs.insert(0, tab("t1", None));
            open_tabbed(backend, tabs, Some("t35"), Vec::new());
            let pick = backend.state().pick.as_ref().unwrap();
            assert_eq!(pick.pages.len(), crate::state::MAX_PICK_TABS);
            assert_eq!(pick.pages[0].tab.as_deref(), Some("t1"));
            assert_eq!(pick.pages[1].tab.as_deref(), Some("t0"));
            assert_eq!(pick.active, 0, "a tab past the cap is not openable");
        });
    }
}
