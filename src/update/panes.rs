use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::key_routing::handle_key_routing;
use crate::ops::focus::{focus_pane as focus, request_pane_focus};
use crate::pane_lifecycle::{find_pane_mut, handle_prune_closed};
use crate::pty_events::{
    handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
};
use crate::state::{PaneId, ResizeCorner, State};
use crate::{HyprmuxApp, control};

pub(super) fn close_popup(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::popup::close(ctx)
}

pub(super) fn focus_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    if ctx
        .state
        .copy_flash
        .is_some_and(|flash| flash.target == id && flash.clearing)
    {
        ctx.state.copy_flash = None;
    }
    focus(&mut ctx.state, id);
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        pane.activity.has_unseen_output = false;
    }
    request_pane_focus(ctx, id);
    Update::full()
}

pub(super) fn hover_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    crate::ops::focus::hover_focus_pane(ctx, id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn begin_move(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    current_rect: FloatRect,
    from_local_x: u16,
    from_local_y: u16,
    target_w: u16,
    target_h: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::begin_move(
        ctx,
        id,
        current_rect,
        from_local_x,
        from_local_y,
        target_w,
        target_h,
        modified,
    )
}

pub(super) fn move_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::move_pane(ctx, id, dx, dy, modified)
}

pub(super) fn end_move(ctx: &mut Context<HyprmuxApp>, id: PaneId, x: u16, y: u16) -> Update {
    crate::ops::resize_move::end_move(ctx, id, x, y)
}

pub(super) fn begin_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::begin_resize(ctx, id, corner, x, y, modified)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resize_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::resize_pane(ctx, id, corner, (from_x, from_y), (x, y), modified)
}

pub(super) fn end_resize(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    if ctx
        .state
        .resizing_pane
        .as_ref()
        .is_some_and(|session| session.id == id)
    {
        ctx.state.resizing_pane = None;
    }
    Update::full()
}

pub(super) fn begin_resize_split(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    horizontal_split: bool,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::begin_resize_split_drag(ctx, id, horizontal_split, x, y)
}

pub(super) fn resize_split(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    horizontal_split: bool,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::resize_split_by_drag(ctx, id, horizontal_split, from_x, from_y, x, y)
}

pub(super) fn begin_resize_split_junction(ctx: &mut Context<HyprmuxApp>, x: u16, y: u16) -> Update {
    crate::ops::resize_move::begin_resize_split_junction_drag(ctx, x, y)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resize_split_junction(
    ctx: &mut Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::resize_split_junction_by_drag(
        ctx,
        &horizontal_panes,
        &vertical_panes,
        from_x,
        from_y,
        x,
        y,
    )
}

pub(super) fn end_resize_split(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.split_drag = None;
    Update::full()
}

pub(super) fn begin_scratch_resize(ctx: &mut Context<HyprmuxApp>, _from_y: u16) -> Update {
    crate::scratchpad::begin_resize(ctx)
}

pub(super) fn scratch_resize(ctx: &mut Context<HyprmuxApp>, from_y: u16, y: u16) -> Update {
    crate::scratchpad::resize(ctx, from_y, y)
}

pub(super) fn end_scratch_resize(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::scratchpad::end_resize(ctx)
}

pub(super) fn finish_open(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        pane.opening = false;
        if !pane.closing {
            ctx.state.animation = GeometryAnimation::Spawn;
        }
    }
    Update::full()
}

pub(super) fn activate_pane(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let focused = ctx.state.current().focused_pane == Some(id)
        || (id == crate::state::POPUP_PANE_ID && ctx.state.popup.is_some());
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        pane.terminal_active = true;
        if !pane.closing && focused {
            request_pane_focus(ctx, id);
        }
    }
    Update::full()
}

pub(super) fn prune_closed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    handle_prune_closed(ctx, id, generation)
}

pub(super) fn pane_input(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    handle_pane_input(ctx, id, input)
}

pub(super) fn copy_flash_expired(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    flash_id: u64,
) -> Update {
    crate::copy_mode::expire_flash(ctx, id, flash_id)
}

pub(super) fn pane_key(ctx: &mut Context<HyprmuxApp>, id: PaneId, key: KeyEvent) -> Update {
    if logical_focus_pending_activation(&ctx.state).is_none_or(|pending| pending == id) {
        focus(&mut ctx.state, id);
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        pane.activity.has_unseen_output = false;
    }
    let (_handled, update) = handle_key_routing(ctx, key, Some(id));
    update
}

pub(super) fn pane_mouse(ctx: &mut Context<HyprmuxApp>, id: PaneId, bytes: Vec<u8>) -> Update {
    handle_pane_mouse(ctx, id, bytes)
}

pub(super) fn pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    handle_pane_resize(ctx, id, cols, rows)
}

pub(super) fn pane_scroll(ctx: &mut Context<HyprmuxApp>, id: PaneId, offset: usize) -> Update {
    handle_pane_scroll(ctx, id, offset)
}

pub(super) fn control_request(
    ctx: &mut Context<HyprmuxApp>,
    envelope: control::ControlEnvelope,
) -> Update {
    crate::ops::control::handle_control_request(ctx, envelope)
}

fn logical_focus_pending_activation(state: &State) -> Option<PaneId> {
    let id = state.current().focused_pane?;
    state.current().workspaces[state.current().active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
        .then_some(id)
}
