use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{SidebarTab, SidebarTabId, UserCommandAction};
use crate::state::RowTarget;

/// A row's ✕ was clicked. The first click arms a confirmation (the row strikes through and its
/// detail line asks for the second click), the second commits it — within
/// [`crate::ops::confirm::CONFIRM_WINDOW`], after which the arming lapses on its own.
///
/// Deliberately confirms regardless of `[confirm]`, which gates the *keyboard* close/kill actions.
/// This is a one-cell pointer target sitting on a row whose ordinary click merely focuses a pane or
/// attaches a session, so a slip is both easy and expensive — the two are not the same gesture and
/// do not share a switch.
pub(crate) fn row_close(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let mut rows = crate::view::sidebar::body_rows(ctx, &tab);
    if index >= rows.len() {
        return Update::none();
    }
    let row = rows.swap_remove(index);
    let Some(close) = row.close else {
        return Update::none();
    };
    // Any other pending confirmation is abandoned by acting here, as it is on an activation.
    ctx.state.sidebar.pending_host_disconnect = None;
    if ctx.state.sidebar.pending_row_close.take() != Some(close.clone()) {
        ctx.state.sidebar.pending_row_close = Some(close);
        return crate::ops::confirm::arm(ctx);
    }
    match close {
        crate::state::SidebarClose::Pane(id) => {
            crate::ops::exit::clear_pending(ctx);
            crate::pane_lifecycle::close_pane(ctx, id)
        }
        // The row carries the live discovered entry the identity was built from, so the kill acts
        // on what is actually on screen rather than re-looking it up and risking a stale match.
        crate::state::SidebarClose::Session { .. } => match row.target {
            RowTarget::Session(entry) => crate::ops::session::kill_discovered_session(ctx, *entry),
            _ => Update::none(),
        },
    }
}

/// Enter: run whatever the row under the cursor does — the same path a click on it takes.
pub(crate) fn activate_cursor(ctx: &mut Context<AppRoot>) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let rows = crate::view::sidebar::body_rows(ctx, &tab);
    match crate::view::sidebar::resolve_cursor(ctx.state.sidebar.panels[panel].cursor, &rows) {
        Some(index) => row_activate(ctx, panel, index),
        None => Update::none(),
    }
}

/// A row was activated by Enter or by a click. The index is resolved against a freshly rebuilt row
/// list — the same pure function of `State` the view rendered from — so both gestures land on the
/// same handler and a row list that changed underneath simply resolves to nothing.
pub(crate) fn row_activate(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let mut rows = crate::view::sidebar::body_rows(ctx, &tab);
    if index >= rows.len() {
        return Update::none();
    }
    // Acting on anything disarms a pending confirmation; capture the host one first so the matching
    // disconnect row can still see its own armed state below.
    let armed_disconnect = ctx.state.sidebar.pending_host_disconnect.take();
    ctx.state.sidebar.pending_row_close = None;
    match rows.swap_remove(index).target {
        RowTarget::Inert => Update::none(),
        RowTarget::Pane(id) => focus_pane(ctx, id),
        RowTarget::PublishedRow { pane_id, row_id } => activate_published_row(ctx, pane_id, row_id),
        RowTarget::Session(entry) => {
            crate::update::sidebar::sessions::activate_session(ctx, *entry)
        }
        RowTarget::HostConnect(target) => {
            crate::update::sidebar::sessions::connect_host(ctx, target)
        }
        RowTarget::HostDisconnect(target) => {
            crate::update::sidebar::sessions::disconnect_host(ctx, target, armed_disconnect)
        }
        RowTarget::NewSession(None) => crate::ops::session::open_create_session(ctx),
        RowTarget::NewSession(Some(target)) => {
            crate::ops::session::open_create_session_on_host(ctx, target)
        }
        RowTarget::ConnectHost => crate::ops::session::open_connect_remote_host(ctx),
        RowTarget::Launcher {
            config_epoch,
            tab_id,
            entry_index,
        } => launcher_activate(ctx, config_epoch, tab_id, entry_index),
        RowTarget::CommandRow {
            config_epoch,
            tab_id,
            output_epoch,
            line,
        } => command_row_activate(ctx, config_epoch, tab_id, output_epoch, line),
    }
}

pub(crate) fn launcher_activate(
    ctx: &mut Context<AppRoot>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    entry_index: usize,
) -> Update {
    if config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Launcher { name, entries, .. } if name == &tab_id => {
                entries.get(entry_index).map(|entry| entry.action.clone())
            }
            _ => None,
        });
    action.map_or_else(Update::none, |action| {
        crate::actions::execute_user_command_action(ctx, &action)
    })
}

pub(crate) fn command_row_activate(
    ctx: &mut Context<AppRoot>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    output_epoch: u64,
    line: String,
) -> Update {
    if config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let current = ctx.state.sidebar.command_output.get(&tab_id);
    if current.is_none_or(|output| {
        output.epoch != output_epoch || !output.rows.iter().any(|row| !row.error && row.raw == line)
    }) {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Command {
                name,
                on_click: Some(action),
                ..
            } if name == &tab_id => Some(action.clone()),
            _ => None,
        });
    action
        .map(|action| resolve_row_action(&action, &line))
        .map_or_else(Update::none, |action| {
            crate::actions::execute_user_command_action(ctx, &action)
        })
}

pub(crate) fn resolve_row_action(action: &UserCommandAction, line: &str) -> UserCommandAction {
    substitute(action, "{line}", line)
}

pub(crate) fn substitute(
    action: &UserCommandAction,
    placeholder: &str,
    value: &str,
) -> UserCommandAction {
    match action {
        UserCommandAction::Send(text) => UserCommandAction::Send(text.replace(placeholder, value)),
        // Config validation rejects placeholders here; run/popup commands are always fixed.
        action => action.clone(),
    }
}

pub(crate) fn focus_pane(ctx: &mut Context<AppRoot>, id: crate::state::PaneId) -> Update {
    if crate::ops::focus::focus_pane_anywhere(ctx, id) {
        Update::full()
    } else {
        Update::none()
    }
}

/// Focus a row's pane and ask its program to bring that row on screen.
///
/// The request travels back over the connection the publisher opened; a program that has since
/// stopped listening still gets its pane focused, which is the part rozi can do alone.
pub(crate) fn activate_published_row(
    ctx: &mut Context<AppRoot>,
    pane_id: crate::state::PaneId,
    row_id: String,
) -> Update {
    let update = focus_pane(ctx, pane_id);
    // Looking at a row acknowledges its finish, exactly as focusing a pane acknowledges the
    // pane's. The pane-wide chokepoint cannot do this: it does not know which row was asked for.
    if let Some(pane) = crate::pane_lifecycle::find_pane_mut(&mut ctx.state, pane_id)
        && let Some(ui) = pane.terminal.published_row_ui.get_mut(&row_id)
    {
        ui.finished_unseen = false;
    }
    crate::ops::published_rows::request_activation(&mut ctx.state, pane_id, &row_id);
    update
}
