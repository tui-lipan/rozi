use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::update::sidebar::polling::{arm_agent_tick, refresh_active_tabs};

/// Re-aim keyboard focus at the active tab's body after the previous one unmounted. A no-op unless
/// the sidebar already had the keyboard — switching tabs with the mouse must not steal it.
pub(crate) fn refocus_body(ctx: &mut Context<AppRoot>) {
    if !ctx.state.sidebar.focused {
        return;
    }
    let key = crate::view::sidebar_focus_key(ctx);
    ctx.request_focus(key);
}

pub(crate) fn visibility_changed(ctx: &mut Context<AppRoot>) -> Update {
    // Hiding the sidebar unmounts the body, so hand the keyboard back before it disappears rather
    // than leaving focus on a widget that is about to stop existing.
    if !ctx.state.sidebar_visible && ctx.state.sidebar.focused {
        ctx.state.sidebar.focused = false;
        release_focus(ctx);
    }
    if !ctx.state.sidebar_visible {
        ctx.state.sidebar.width_preview = None;
    }
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.sidebar.invalidate_commands();
    arm_agent_tick(ctx);
    refresh_active_tabs(ctx)
}

/// `focus-sidebar`: reveal the sidebar if it is hidden, then move keyboard focus into its row list.
/// The body sits in a `FocusScope::Exclude` subtree, so an explicit keyed request is the only way
/// in — Tab and clicks deliberately cannot do this.
pub(crate) fn focus_body(ctx: &mut Context<AppRoot>) -> Update {
    let command = if ctx.state.sidebar_visible {
        None
    } else {
        ctx.state.sidebar_visible = true;
        visibility_changed(ctx).command
    };
    // Resolves after reconciliation, so requesting it in the same pass that reveals the sidebar is
    // fine even though the body has not mounted yet.
    let key = crate::view::sidebar_focus_key(ctx);
    ctx.request_focus(key);
    // The request resolves after reconciliation, so record the intent now. Nothing can read this
    // back off the framework — the body sits in a `FocusScope::Exclude` subtree, which is invisible
    // to `has_focus_within_key` — so `ops::focus` retracts it whenever focus goes elsewhere.
    ctx.state.sidebar.focused = true;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    ctx.state.commands_dirty = true;
    Update::with_command(command)
}

/// Escape from the sidebar or a pointer-focused explorer: give the keyboard back to the focused
/// pane.
pub(crate) fn blur_body(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.sidebar.focused = false;
    ctx.state.sidebar.explorer_entered_from_tree = false;
    ctx.state.commands_dirty = true;
    release_focus(ctx);
    Update::full()
}

pub(crate) fn explorer_focus(
    ctx: &mut Context<AppRoot>,
    origin: Option<FileTreeExplorerFocusOrigin>,
) -> Update {
    ctx.state.sidebar.explorer_entered_from_tree =
        origin == Some(FileTreeExplorerFocusOrigin::Tree);
    Update::none()
}

/// The explorer committed its query with Enter and returned focus to the tree. This is a real
/// sidebar-mode entry, unlike a pointer click into the explorer, so restore the sidebar cursor and
/// its keyboard ownership before the next key arrives.
pub(crate) fn tree_focused(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.sidebar.focused = true;
    ctx.state.sidebar.explorer_entered_from_tree = false;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

/// Drop focus from the sidebar body and hand it to the focused pane when there is one to hand it
/// to. The unconditional `blur` matters: a pane whose terminal has not come up yet refuses focus,
/// and without this the sidebar would keep the keyboard with no way out.
pub(crate) fn release_focus(ctx: &mut Context<AppRoot>) {
    ctx.blur();
    crate::ops::focus::request_current_pane_focus(ctx);
}

/// Tab / Shift-Tab while the body has focus. Cycling remounts the body under a new key, so focus
/// has to be re-requested for the tab the user just landed on.
pub(crate) fn cycle_tab(ctx: &mut Context<AppRoot>, forward: bool) -> Update {
    if !ctx.state.sidebar_visible {
        return Update::none();
    }
    let panel = ctx.state.sidebar.active_panel;
    ctx.state.sidebar.cycle(panel, forward);
    if let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) {
        panel.cursor = 0;
        panel.suppress_row_hover = true;
        panel.hovered_row = None;
    }
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

/// Store the number of selectable rows currently visible in a composed row list. The event is
/// keyed by tab because switching tabs can queue the old list's final viewport event alongside the
/// new one.
pub(crate) fn viewport_changed(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    tab_id: crate::config::SidebarTabId,
    event: ScrollViewportEvent,
) -> Update {
    if ctx.state.sidebar.active_tab_in(panel) != Some(&tab_id) {
        return Update::none();
    }
    let Some(first) = event.first_visible_index else {
        return Update::none();
    };
    let last = event.last_visible_index.unwrap_or(first).max(first);
    let Some(tab) = ctx.state.active_sidebar_tab(panel).cloned() else {
        return Update::none();
    };
    let items = ctx.state.sidebar_item_projections(&tab);
    let page_rows = items
        .iter()
        .enumerate()
        .filter(|(index, item)| *index >= first && *index <= last && item.selectable())
        .count()
        .max(1);
    let Some(panel_state) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    panel_state.page_rows = page_rows;
    Update::none()
}

/// Move by the number of selectable rows that fit in the active row list's viewport.
pub(crate) fn move_cursor_page(ctx: &mut Context<AppRoot>, down: bool) -> Update {
    let page_rows = ctx
        .state
        .sidebar
        .active_panel()
        .map(|panel| panel.page_rows.max(1) as isize)
        .unwrap_or(1);
    move_cursor(ctx, if down { page_rows } else { -page_rows })
}

pub(crate) fn focus_panel(ctx: &mut Context<AppRoot>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() < 2 {
        return Update::none();
    }
    let next = if down { 1 } else { 0 };
    if ctx.state.sidebar.active_panel == next {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = next;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    refocus_body(ctx);
    Update::full()
}

pub(crate) fn reorder_active_tab(ctx: &mut Context<AppRoot>, right: bool) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(panel_state) = ctx.state.sidebar.panels.get(panel) else {
        return Update::none();
    };
    let Some(active) = panel_state.active_tab.as_ref() else {
        return Update::none();
    };
    let Some(from) = panel_state.tabs.iter().position(|id| id == active) else {
        return Update::none();
    };
    let to = if right {
        (from + 1).min(panel_state.tabs.len().saturating_sub(1))
    } else {
        from.saturating_sub(1)
    };
    if !ctx.state.sidebar.reorder_tab(panel, from, to) {
        return Update::none();
    }
    crate::update::sidebar::sync_and_persist_panels(ctx);
    Update::layout()
}

pub(crate) fn move_active_tab_to_panel(ctx: &mut Context<AppRoot>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() == 1 {
        if !down {
            return Update::none();
        }
        crate::update::sidebar::set_split_enabled(ctx, true);
    }
    let from_panel = ctx.state.sidebar.active_panel;
    let to_panel = if down { 1 } else { 0 };
    if from_panel == to_panel {
        return Update::none();
    }
    let Some(active) = ctx.state.sidebar.active_tab().cloned() else {
        return Update::none();
    };
    let Some(from) = ctx.state.sidebar.panels[from_panel]
        .tabs
        .iter()
        .position(|id| id == &active)
    else {
        return Update::none();
    };
    let to = ctx.state.sidebar.panels[to_panel].tabs.len();
    if !ctx
        .state
        .sidebar
        .transfer_tab(from_panel, to_panel, from, to)
    {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = to_panel;
    crate::update::sidebar::sync_and_persist_panels(ctx);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

/// Move the keyboard cursor by `delta` selectable rows, stopping at the ends rather than wrapping —
/// the row list is a panel, not a carousel, and wrapping past the last agent back to the first reads
/// as a glitch. Headers and spacers are stepped over rather than landed on.
pub(crate) fn move_cursor(ctx: &mut Context<AppRoot>, delta: isize) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(tab) = ctx.state.active_sidebar_tab(panel).cloned() else {
        return Update::none();
    };
    let items = ctx.state.sidebar_item_projections(&tab);
    let selectable: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.selectable())
        .map(|(index, _)| index)
        .collect();
    if selectable.is_empty() {
        return Update::none();
    }
    let current = crate::state::SidebarItemProjection::resolve_cursor(
        ctx.state.sidebar.panels[panel].cursor,
        &items,
    );
    let position = current
        .and_then(|current| selectable.iter().position(|index| *index == current))
        .unwrap_or(0);
    let next = position
        .saturating_add_signed(delta)
        .min(selectable.len() - 1);
    let cursor = selectable[next];
    if ctx.state.sidebar.panels[panel].cursor == cursor {
        return Update::none();
    }
    // Moving off a row disarms any pending confirmation.
    ctx.state.sidebar.pending_host_disconnect = None;
    ctx.state.sidebar.pending_row_close = None;
    ctx.state.sidebar.panels[panel].cursor = cursor;
    ctx.state.sidebar.panels[panel].suppress_row_hover = true;
    Update::full()
}

/// A real pointer move ends keyboard modality and lets row hover follow the pointer again.
pub(crate) fn pointer_moved(ctx: &mut Context<AppRoot>, panel: usize) -> Update {
    let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    if !panel.suppress_row_hover {
        return Update::none();
    }
    panel.suppress_row_hover = false;
    // Re-enabling the rows' hover effects changes how the row list describes itself, so the view has
    // to run - but the description is the same shape, so reconciling it is enough.
    Update::layout()
}

/// The pointer entered or left a row, which is what reveals that row's ✕.
///
/// A leave only clears when it is still this row's: moving from a row onto the ✕ nested inside it
/// fires leave for the row and then enter for the ✕, both naming the same index, and the queue is
/// drained before the next paint — so ordering them this way keeps the glyph steady under the
/// pointer instead of flickering out from under it.
pub(crate) fn row_hover(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    index: usize,
    hovered: bool,
) -> Update {
    let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    let next = if hovered {
        Some(index)
    } else if panel.hovered_row == Some(index) {
        None
    } else {
        return Update::none();
    };
    if panel.hovered_row == next {
        return Update::none();
    }
    panel.hovered_row = next;
    // A click-only region does not repaint on a hover transition by itself, and the ✕ appearing is
    // exactly what the transition has to show — so the view has to run. `layout` rather than `full`:
    // the row list is described the same way either side of the transition, one glyph aside, so
    // re-running the view and reconciling is enough without rebuilding the whole element tree.
    Update::layout()
}
