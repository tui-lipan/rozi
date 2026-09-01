use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::focus::{request_current_pane_focus, request_layout_picker_focus};
use crate::ops::resize_move::set_layout;
use crate::state::{LayoutKind, LayoutPickerState, Mode};

/// The active workspace's current layout, and its index in [`LayoutKind::all`].
fn current_layout(ctx: &Context<AppRoot>) -> (usize, LayoutKind) {
    let current = ctx.state.active_workspace_ref().layout_kind;
    let index = LayoutKind::all()
        .iter()
        .position(|kind| *kind == current)
        .unwrap_or(0);
    (index, current)
}

/// Apply a layout for live preview. Only the client that may actually reshape the layout previews:
/// a follower cannot, and would only see its highlight flicker as server pushes overwrite it.
fn preview_layout(ctx: &mut Context<AppRoot>, kind: LayoutKind) {
    if ctx.state.scratch_visible || ctx.state.is_controller() {
        set_layout(ctx, kind, false);
    }
}

pub(crate) fn open_layout_picker(ctx: &mut Context<AppRoot>) -> Update {
    let (selected, original) = current_layout(ctx);
    ctx.state.layout_picker = Some(LayoutPickerState::new(selected, original));
    ctx.state.show_layout_picker = true;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.show_theme_picker = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_layout_picker_focus(ctx);
    Update::full()
}

pub(crate) fn cancel_layout_picker(ctx: &mut Context<AppRoot>) -> Update {
    // Leaving without committing restores the layout the picker previewed away from.
    if let Some(picker) = ctx.state.layout_picker.take() {
        let (_, current) = current_layout(ctx);
        if current != picker.original {
            preview_layout(ctx, picker.original);
        }
    }
    ctx.state.show_layout_picker = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

/// Highlight moved to a different row: preview that layout live so the workspace reflects the
/// selection before the user commits with Enter.
pub(crate) fn layout_picker_selection_changed(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(kind) = LayoutKind::all().get(index).copied() else {
        return Update::none();
    };
    if let Some(picker) = ctx.state.layout_picker.as_mut() {
        picker.selected = index;
    }
    preview_layout(ctx, kind);
    Update::full()
}

pub(crate) fn layout_picker_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.layout_picker.as_mut() {
        picker.query = query;
    }
    Update::none()
}

/// Enter on a row: commit the highlighted layout and close the picker. Live preview has usually
/// already applied it; this is the point the change becomes permanent (survives cancel).
pub(crate) fn select_layout(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(kind) = LayoutKind::all().get(index).copied() else {
        return Update::none();
    };
    ctx.state.show_layout_picker = false;
    ctx.state.layout_picker = None;
    ctx.state.commands_dirty = true;
    // A follower cannot reshape the shared layout: nudge it to take control and leave the layout
    // untouched, exactly as the cycle command does through `is_layout_mutating`.
    if !ctx.state.scratch_visible && crate::ops::session::nudge_if_follower(ctx) {
        request_current_pane_focus(ctx);
        return Update::full();
    }
    // Reshaping a session by hand engages it, the same treatment the cycle command gets.
    if !ctx.state.scratch_visible {
        ctx.state.current_mut().engaged = true;
    }
    set_layout(ctx, kind, false);
    request_current_pane_focus(ctx);
    Update::full()
}

/// Persist the highlighted layout as `[layout] default`. The picker stays open and its `default`
/// badge moves to the new row; only a write failure surfaces a toast.
pub(crate) fn layout_picker_set_default(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.layout_picker.as_ref() else {
        return Update::none();
    };
    let Some(kind) = LayoutKind::all().get(picker.selected).copied() else {
        return Update::none();
    };
    let items = [SearchItem::new(kind.label(), ())];
    if tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        &picker.query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    )
    .is_empty()
    {
        return Update::none();
    }
    match crate::config::persist_layout_default(kind) {
        Ok(_) => ctx.state.config.layout.default = kind,
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Default not set", message);
        }
    }
    Update::full()
}
