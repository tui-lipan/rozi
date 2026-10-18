//! Stream tabs: admitting `rozi sidebar publish` producers, taking their snapshots, and telling them
//! what the user did with their rows. See [`crate::state::SidebarStreams`] for the ownership rules.

use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{ExtensionProvenance, SidebarTab, SidebarTabId};
use crate::control::{ControlErrorCode, ControlResponse};
use crate::state::{
    SidebarActionSpec, SidebarProducer, SidebarProducerKey, SidebarSnapshot, SidebarStreamEvent,
    State, StreamRowRef, TreeExpansionKey, WorkerId,
};

/// What a `sidebar-publish` request asked for, before the UI decides whether to admit it.
#[derive(Clone, Debug)]
pub struct StreamOpen {
    pub stream_id: u64,
    pub tab: String,
    pub name: Option<String>,
    pub extension: Option<ExtensionProvenance>,
    pub worker: Option<WorkerId>,
}

fn refuse(code: ControlErrorCode, message: impl Into<String>) -> ControlResponse {
    ControlResponse::error_with(code, message)
}

/// The stream tab a request names, if the caller may publish to it.
///
/// An extension's processes publish only to that extension's own tabs, named by their local name.
/// Anything else publishes only to a `config.toml` stream tab, never into an extension's.
fn admitted_tab(
    state: &State,
    tab: &str,
    extension: Option<&ExtensionProvenance>,
) -> std::result::Result<SidebarTabId, ControlResponse> {
    let id = match extension {
        Some(provenance) => format!("{}.{tab}", provenance.id),
        None if crate::config::is_extension_scoped_id(tab) => {
            return Err(refuse(
                ControlErrorCode::NotPermitted,
                format!(
                    "`{tab}` is an extension's tab; only that extension's processes publish to it"
                ),
            ));
        }
        None => tab.to_string(),
    };
    let id = SidebarTabId::new(id);
    match state
        .config
        .sidebar
        .tabs
        .iter()
        .find(|candidate| candidate.id() == id)
    {
        Some(SidebarTab::Stream { .. }) => Ok(id),
        Some(_) => Err(refuse(
            ControlErrorCode::InvalidRequest,
            format!("sidebar tab `{}` is not a stream tab", id.as_str()),
        )),
        None => Err(refuse(
            ControlErrorCode::InvalidArgument,
            format!("no sidebar tab `{}`", id.as_str()),
        )),
    }
}

/// A producer's `--name`, validated the way a publisher's is.
fn producer_name(name: Option<String>) -> std::result::Result<Option<String>, ControlResponse> {
    match name {
        Some(name) if !crate::ops::published_rows::publisher_name_is_valid(&name) => Err(refuse(
            ControlErrorCode::InvalidArgument,
            "a producer name is 1 to 64 lowercase letters, digits, `-`, or `_`",
        )),
        name => Ok(name),
    }
}

pub(crate) fn open(
    ctx: &mut Context<AppRoot>,
    open: StreamOpen,
    sender: std::sync::mpsc::SyncSender<String>,
    ack: std::sync::mpsc::Sender<std::result::Result<(SidebarTabId, String), ControlResponse>>,
) -> Update {
    let admitted = (|| {
        if open.extension.as_ref().is_some_and(|provenance| {
            !crate::config::provenance_is_active(&ctx.state.extension_generations, provenance)
        }) {
            return Err(refuse(
                ControlErrorCode::ExtensionInactive,
                "extension is not active",
            ));
        }
        let tab = admitted_tab(&ctx.state, &open.tab, open.extension.as_ref())?;
        let binding = match open.worker {
            Some(worker) => Some(
                crate::ops::extension_workers::live_worker(&ctx.state, worker)?
                    .binding
                    .clone(),
            ),
            None => None,
        };
        let key = SidebarProducerKey {
            binding,
            name: producer_name(open.name)?,
        };
        Ok((tab, key))
    })();
    let (tab, key) = match admitted {
        Ok(admitted) => admitted,
        Err(response) => {
            let _ = ack.send(Err(response));
            return Update::none();
        }
    };
    let id = key.id();
    let producers = ctx
        .state
        .sidebar_streams
        .tabs
        .entry(tab.clone())
        .or_default();
    // A producer that comes back - restarted, or reconnected after a network drop - is the same
    // producer: its new stream takes over, and the rows it published last stay until its first
    // snapshot replaces them, so a restart does not blink the tab empty.
    let (rows, actions) = producers
        .remove(&id)
        .map(|previous| (previous.rows, previous.actions))
        .unwrap_or_default();
    producers.insert(
        id.clone(),
        SidebarProducer {
            key,
            extension: open.extension.map(|provenance| provenance.id),
            stream_id: open.stream_id,
            sender,
            rows,
            actions,
            visible: None,
            wedged: std::cell::Cell::new(false),
        },
    );
    let _ = ack.send(Ok((tab, id)));
    sync_visibility(&mut ctx.state);
    Update::full()
}

/// A producer's new snapshot. A stream that has since been replaced is ignored.
pub(crate) fn snapshot(
    ctx: &mut Context<AppRoot>,
    stream_id: u64,
    tab: SidebarTabId,
    producer: String,
    snapshot: SidebarSnapshot,
) -> Update {
    let Some(current) = ctx
        .state
        .sidebar_streams
        .tabs
        .get_mut(&tab)
        .and_then(|producers| producers.get_mut(&producer))
        .filter(|current| current.stream_id == stream_id)
    else {
        return Update::none();
    };
    let rows = crate::state::sanitize_rows(snapshot.rows);
    let actions = snapshot.actions.map(valid_actions);
    let changed = current.rows != rows
        || actions
            .as_ref()
            .is_some_and(|actions| *actions != current.actions);
    current.rows = rows;
    if let Some(actions) = actions {
        current.actions = actions;
    }
    if changed {
        ctx.state.sidebar.pending_row_close = None;
        Update::full()
    } else {
        Update::none()
    }
}

/// A producer's stream ended. Its rows go, unless a newer stream already speaks for it.
pub(crate) fn closed(
    ctx: &mut Context<AppRoot>,
    stream_id: u64,
    tab: SidebarTabId,
    producer: String,
) -> Update {
    let Some(producers) = ctx.state.sidebar_streams.tabs.get_mut(&tab) else {
        return Update::none();
    };
    if producers
        .get(&producer)
        .is_none_or(|current| current.stream_id != stream_id)
    {
        return Update::none();
    }
    producers.remove(&producer);
    if producers.is_empty() {
        ctx.state.sidebar_streams.tabs.remove(&tab);
    }
    Update::full()
}

/// Drop producers of tabs that are no longer stream tabs, after a configuration change.
pub(crate) fn reconcile(state: &mut State) {
    if state.sidebar_streams.tabs.is_empty() {
        return;
    }
    let tabs: Vec<SidebarTabId> = state
        .config
        .sidebar
        .tabs
        .iter()
        .filter(|tab| matches!(tab, SidebarTab::Stream { .. }))
        .map(SidebarTab::id)
        .collect();
    state.sidebar_streams.tabs.retain(|tab, producers| {
        producers.retain(|_, producer| !producer.wedged.get());
        tabs.contains(tab) && !producers.is_empty()
    });
}

/// Drop the producers of extensions whose generation retired: disabled, removed, or changed in a
/// way that restarts their processes.
pub(crate) fn unload_extensions(
    state: &mut State,
    stale_extensions: &std::collections::HashSet<String>,
) {
    state.sidebar_streams.tabs.retain(|_, producers| {
        producers.retain(|_, producer| {
            producer
                .extension
                .as_ref()
                .is_none_or(|extension| !stale_extensions.contains(extension))
        });
        !producers.is_empty()
    });
}

fn send(producer: &SidebarProducer, event: &SidebarStreamEvent) {
    if let Ok(mut line) = serde_json::to_string(event) {
        line.push('\n');
        // A full backlog means the producer stopped reading. It is dropped rather than left showing
        // rows that no longer answer, the way a wedged `publish` stream is.
        if let Err(std::sync::mpsc::TrySendError::Full(_)) = producer.sender.try_send(line) {
            producer.wedged.set(true);
        }
    }
}

/// Tell every producer whether its tab is on screen, when that has changed.
pub(crate) fn sync_visibility(state: &mut State) {
    if state.sidebar_streams.tabs.is_empty() {
        return;
    }
    let shown = state.sidebar_shown();
    let visible: Vec<SidebarTabId> = if shown {
        state.visible_sidebar_tabs().cloned().collect()
    } else {
        Vec::new()
    };
    for (tab, producers) in &mut state.sidebar_streams.tabs {
        let on_screen = visible.contains(tab);
        for producer in producers.values_mut() {
            if producer.visible != Some(on_screen) {
                producer.visible = Some(on_screen);
                send(
                    producer,
                    &SidebarStreamEvent::Visible { visible: on_screen },
                );
            }
        }
    }
}

/// A stream row was activated: tell its producer.
pub(crate) fn activate(ctx: &mut Context<AppRoot>, row: StreamRowRef) -> Update {
    if let Some((producer, spec)) = ctx.state.stream_row(&row)
        && spec.disabled.is_none()
    {
        send(
            producer,
            &SidebarStreamEvent::Activate {
                activate: spec.id.clone(),
            },
        );
    }
    Update::none()
}

/// A row was expanded. A stream producer that lists children only on request is asked for them.
pub(crate) fn expanded(state: &mut State, key: &TreeExpansionKey) {
    let row = StreamRowRef {
        tab: key.tab.clone(),
        producer: key.section.clone(),
        row: key.row.clone(),
    };
    if let Some((producer, spec)) = state.stream_row(&row)
        && spec.expandable
    {
        send(
            producer,
            &SidebarStreamEvent::Expand {
                expand: spec.id.clone(),
            },
        );
    }
}

/// Fire one of a producer's actions on its row.
pub(crate) fn fire_action(state: &State, row: &StreamRowRef, action: &str) {
    if let Some((producer, spec)) = state.stream_row(row)
        && producer.actions.iter().any(|candidate| {
            candidate.id == action && crate::state::action_applies(spec, candidate)
        })
    {
        send(
            producer,
            &SidebarStreamEvent::Action {
                action: action.to_string(),
                row: spec.id.clone(),
            },
        );
    }
}

/// Whether the focused sidebar already uses `key` for itself: moving, opening and closing rows,
/// switching tabs, activating, closing. A producer may not take those away.
fn reserved(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::Tab
        | KeyCode::BackTab => true,
        KeyCode::Char(c) => {
            let plain = !key.mods.ctrl && !key.mods.alt;
            matches!(c.to_ascii_lowercase(), 'h' | 'j' | 'k' | 'l')
                || (plain && matches!(c, 'g' | 'G' | 'x' | 'X' | '?' | ' '))
        }
        _ => false,
    }
}

/// The single key event an action's chord fires on, if it is one the sidebar can give it.
fn action_key(spec: &str) -> Option<KeyEvent> {
    let binding = KeyBinding::from_str(spec).ok()?;
    let events = binding.key_events().ok()?;
    match events.as_slice() {
        [event] if !reserved(event) => Some(*event),
        _ => None,
    }
}

/// A producer's declared actions, keeping only those that can actually fire: an id, a label, a
/// one-step chord the sidebar does not already use, and no id or key declared twice.
fn valid_actions(actions: Vec<SidebarActionSpec>) -> Vec<SidebarActionSpec> {
    let mut kept: Vec<SidebarActionSpec> = Vec::new();
    let mut close_taken = false;
    for mut action in actions.into_iter().take(crate::state::SIDEBAR_MAX_ACTIONS) {
        action.id = action.id.trim().to_string();
        action.label = tui_lipan::utils::sanitize_display_text(&action.label)
            .trim()
            .chars()
            .take(40)
            .collect();
        if action.id.is_empty() || action.label.is_empty() {
            continue;
        }
        let Some(key) = action_key(&action.key) else {
            continue;
        };
        if kept.iter().any(|other| {
            other.id == action.id
                || action_key(&other.key).is_some_and(|other| {
                    KeyBinding::from_str(&action.key)
                        .is_ok_and(|binding| binding.matches_sequence(&[other]))
                        || other == key
                })
        }) {
            continue;
        }
        // One ✕ per row: a second `close` action keeps its key but not the ✕.
        if action.close {
            action.close = !close_taken;
            close_taken = true;
        }
        kept.push(action);
    }
    kept
}

/// A key pressed in a focused stream tab: one of the producer's actions on the row under the
/// cursor, if it declared one for that key. `None` lets the key fall through.
pub(crate) fn key(ctx: &mut Context<AppRoot>, key: KeyEvent) -> Option<Update> {
    let panel = ctx.state.sidebar.active_panel_index();
    let tab = ctx.state.active_sidebar_tab(panel)?;
    if !matches!(tab, SidebarTab::Stream { .. }) {
        return None;
    }
    let mut rows = ctx.state.sidebar_rows(tab);
    let index =
        crate::view::sidebar::resolve_cursor(ctx.state.sidebar.panels[panel].cursor, &rows)?;
    let crate::state::RowTarget::StreamRow(row) = rows.swap_remove(index).target else {
        return None;
    };
    let (producer, spec) = ctx.state.stream_row(&row)?;
    let action = producer
        .actions
        .iter()
        .filter(|action| crate::state::action_applies(spec, action))
        .find(|action| {
            KeyBinding::from_str(&action.key).is_ok_and(|binding| binding.matches_sequence(&[key]))
        })?
        .clone();
    // A confirming action arms on the first press, through the same pending confirmation the ✕
    // uses, and fires on a second press of the same key on the same row.
    if action.confirm || action.close {
        let pending = crate::state::SidebarClose::StreamAction {
            row: row.clone(),
            action: action.id.clone(),
        };
        if ctx.state.sidebar.pending_row_close.take() != Some(pending.clone()) {
            ctx.state.sidebar.pending_row_close = Some(pending);
            return Some(crate::ops::confirm::arm(ctx));
        }
    }
    fire_action(&ctx.state, &row, &action.id);
    Some(Update::full())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: &str, key: &str) -> SidebarActionSpec {
        SidebarActionSpec {
            id: id.into(),
            key: key.into(),
            label: id.into(),
            confirm: false,
            close: false,
        }
    }

    /// A producer cannot take the sidebar's own keys, bind a chord, or declare one key twice.
    #[test]
    fn actions_keep_only_keys_the_sidebar_can_give() {
        let mut first_close = action("rm", "d");
        first_close.close = true;
        let mut second_close = action("kill", "ctrl-d");
        second_close.close = true;
        let kept = valid_actions(vec![
            first_close,
            action("nav", "j"),
            action("enter", "enter"),
            action("chord", "ctrl-a d"),
            action("again", "d"),
            second_close,
            action("refresh", "r"),
            action("", "z"),
        ]);
        let ids: Vec<_> = kept.iter().map(|action| action.id.as_str()).collect();
        assert_eq!(ids, ["rm", "kill", "refresh"]);
        assert!(kept[0].close);
        assert!(!kept[1].close, "only one ✕ per row");
    }
}
