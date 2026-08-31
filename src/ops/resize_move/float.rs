use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::anim::GeometryAnimation;
use crate::layout::geometry::{
    canvas_local_point_from_mouse, clamp_floating_rect, grabbed_edge_on_outer_border,
    resize_float_rect_from_corner, workspace_tile_bounds,
};
use crate::layout::tiling::{
    SplitEdge, allocate_dwindle, move_tiled_window_around_target, resize_tiled_split_for_edge,
};
use crate::layout::{
    self, placement_for, target_tiled_pane_for_drop, workspace_target_rects,
    workspace_target_rects_excluding,
};
use crate::ops::focus::{active_pane_mut, focus_pane, request_pane_focus, sync_scrollable_reveal};
use crate::state::{
    self, Direction, EVEN_SPLIT_RATIO, LayoutKind, MoveSession, PaneId, ResizeCorner,
    ResizeSession, State, TileGap, Workspace,
};

use super::tiling::{
    master_available_width, resize_master_split_by_pixels, resize_scrollable_width_by_pixels,
};

/// Whether tree-based split resizing applies to this layout. Grid and monocle place panes
/// purely by formula - there is no ratio to adjust, and writing into the dwindle tree
/// would silently rearrange the layouts that do read it.
pub(super) fn layout_has_resizable_splits(kind: LayoutKind) -> bool {
    kind == LayoutKind::Dwindle
}

pub(super) fn ensure_tile_tree(workspace: &mut Workspace) {
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    }
}

fn float_keyboard_delta(direction: Direction, bounds: FloatRect) -> (f32, f32) {
    match direction {
        Direction::Left => (-super::keyboard_step_cells(bounds.w), 0.0),
        Direction::Right => (super::keyboard_step_cells(bounds.w), 0.0),
        Direction::Up => (0.0, -super::keyboard_step_cells(bounds.h)),
        Direction::Down => (0.0, super::keyboard_step_cells(bounds.h)),
    }
}

/// Translate the focused floating pane one step. Returns whether the focus was floating at all, so
/// `super::tiling::reorder_focused_in_direction` can fall through to the tiled reorder when it was
/// not.
pub(super) fn move_focused_float(ctx: &mut Context<AppRoot>, direction: Direction) -> bool {
    let Some(id) = ctx.state.focused_pane() else {
        return false;
    };
    let bounds = ctx.state.layout_bounds(ctx.viewport());
    let (dx, dy) = float_keyboard_delta(direction, bounds);
    let Some(pane) = active_pane_mut(&mut ctx.state, id) else {
        return false;
    };
    if !pane.floating || pane.fullscreen {
        return false;
    }
    // Same clamp as the pointer drag, so a pane can be parked partly offscreen either way.
    let moved = clamp_floating_rect(
        FloatRect {
            x: pane.floating_rect.x + dx,
            y: pane.floating_rect.y + dy,
            ..pane.floating_rect
        },
        bounds,
    );
    if moved != pane.floating_rect {
        pane.floating_rect = moved;
        // Snap, matching the pointer drag. Held keys repeat faster than a glide completes, so each
        // step would restart the previous one mid-flight and the pane would trail the keypresses.
        ctx.state.animation = GeometryAnimation::None;
    }
    true
}

/// Grow (`Right`/`Down`) or shrink (`Left`/`Up`) the focused floating pane one step, anchored at
/// its top-left corner - dragging the top-left instead would walk the pane across the workspace as
/// it resized. Returns whether the focus was floating at all.
pub(super) fn resize_focused_float(ctx: &mut Context<AppRoot>, direction: Direction) -> bool {
    let Some(id) = ctx.state.focused_pane() else {
        return false;
    };
    let bounds = ctx.state.layout_bounds(ctx.viewport());
    let (dx, dy) = float_keyboard_delta(direction, bounds);
    let Some(pane) = active_pane_mut(&mut ctx.state, id) else {
        return false;
    };
    if !pane.floating || pane.fullscreen {
        return false;
    }
    let mut resized =
        resize_float_rect_from_corner(pane.floating_rect, ResizeCorner::LowerRight, dx, dy, bounds);
    // A pane already flush against the right or bottom edge cannot grow that way, which would
    // leave the key dead. Fall back to the opposite corner so it grows inward instead.
    if resized == pane.floating_rect {
        resized = resize_float_rect_from_corner(
            pane.floating_rect,
            ResizeCorner::UpperLeft,
            -dx,
            -dy,
            bounds,
        );
    }
    if resized != pane.floating_rect {
        pane.floating_rect = resized;
        ctx.state.animation = GeometryAnimation::None;
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_move(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    current_rect: FloatRect,
    from_local_x: u16,
    from_local_y: u16,
    target_w: u16,
    target_h: u16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    // The scratchpad is client-local, so a follower moves and resizes its panes freely; only the
    // shared workspace needs the lease.
    if !ctx.state.scratch_visible && crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    let content_left = ctx.state.terminal_content_left_offset(ctx.viewport());
    let content_top = ctx.state.content_top_offset();
    let mut session = None;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        if !pane.fullscreen {
            let was_floating = pane.floating;
            let drag_rect = current_rect;
            if was_floating {
                pane.floating_rect = drag_rect;
            }
            session = Some(MoveSession {
                id,
                was_floating,
                drag_rect,
                pointer_x: i32::from(content_left)
                    + current_rect.x.round() as i32
                    + i32::from(from_local_x.min(target_w.saturating_sub(1))),
                pointer_y: i32::from(content_top)
                    + current_rect.y.round() as i32
                    + i32::from(from_local_y.min(target_h.saturating_sub(1))),
            });
        }
    }
    ctx.state.moving_pane = session;
    ctx.state.animation = if session.is_some_and(|session| !session.was_floating) {
        GeometryAnimation::TileFloat
    } else {
        GeometryAnimation::None
    };
    Update::full()
}

pub(crate) fn move_pane(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    let bounds = ctx.state.layout_bounds(ctx.viewport());
    let mut persisted_floating_rect = None;
    if let Some(session) = ctx
        .state
        .moving_pane
        .as_mut()
        .filter(|session| session.id == id)
    {
        session.drag_rect.x += f32::from(dx);
        session.drag_rect.y += f32::from(dy);
        session.drag_rect = clamp_floating_rect(session.drag_rect, bounds);
        session.pointer_x += i32::from(dx);
        session.pointer_y += i32::from(dy);
        if session.was_floating {
            persisted_floating_rect = Some(session.drag_rect);
        }
        ctx.state.animation = if session.was_floating {
            GeometryAnimation::None
        } else {
            GeometryAnimation::TileFloat
        };
    }
    if let Some(rect) = persisted_floating_rect
        && let Some(pane) = active_pane_mut(&mut ctx.state, id)
    {
        pane.floating_rect = rect;
    }
    Update::full()
}

pub(crate) fn end_move(ctx: &mut Context<AppRoot>, id: PaneId, x: u16, y: u16) -> Update {
    let session = ctx.state.moving_pane.filter(|session| session.id == id);
    if session.is_some() {
        ctx.state.moving_pane = None;
    }
    if let Some(session) = session {
        if session.was_floating {
            if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
                pane.floating_rect = session.drag_rect;
            }
        } else {
            let viewport = ctx.viewport();
            drop_tiled_pane_at(&mut ctx.state, id, x, y, viewport);
        }
    }
    Update::full()
}

/// Finish any pointer-driven layout edit before an action changes pane/layout mode. Mouse drag-end
/// events arriving afterward become harmless because their session has already been cleared.
pub(crate) fn finish_pointer_layout_interaction(ctx: &mut Context<AppRoot>) {
    if let Some(session) = ctx.state.moving_pane {
        focus_pane(&mut ctx.state, session.id);
        request_pane_focus(ctx, session.id);
        let x = session.pointer_x.clamp(0, i32::from(u16::MAX)) as u16;
        let y = session.pointer_y.clamp(0, i32::from(u16::MAX)) as u16;
        end_move(ctx, session.id, x, y);
    }
    ctx.state.resizing_pane = None;
    ctx.state.split_drag = None;
}

pub(crate) fn begin_resize(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    corner: ResizeCorner,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    // The scratchpad is client-local, so a follower moves and resizes its panes freely; only the
    // shared workspace needs the lease.
    if !ctx.state.scratch_visible && crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    let workspace = ctx.state.layout_target();
    let scrollable_layout =
        ctx.state.workspace_for(workspace).layout_kind == LayoutKind::Scrollable;
    // Clear first; focus_pane re-arms AxisChange only when Scrollable focus/anchor actually moves.
    ctx.state.animation = GeometryAnimation::None;
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    ensure_tile_tree(ctx.state.workspace_for_mut(workspace));
    let start_floating_rect = active_pane_mut(&mut ctx.state, id)
        .filter(|pane| pane.floating)
        .map(|pane| pane.floating_rect);
    let start_scrollable_width = {
        let ws = ctx.state.workspace_for(workspace);
        (scrollable_layout && start_floating_rect.is_none())
            .then(|| {
                ws.panes
                    .iter()
                    .find(|pane| pane.id == id && !pane.floating && !pane.closing)
                    .map(|pane| pane.scrollable_width)
            })
            .flatten()
    };
    // Grabbing an upper corner of a pane against the dropdown's top edge grabs the dropdown's own
    // border: there is no split above it to move, so the drag's vertical component resizes the
    // scratchpad instead. Recorded once here so every event applies its delta from this origin.
    let start_scratch_height = (ctx.state.scratch_visible
        && start_floating_rect.is_none()
        && matches!(corner, ResizeCorner::UpperLeft | ResizeCorner::UpperRight)
        && crate::scratchpad::pane_touches_top_edge(&ctx.state, id))
    .then(|| crate::scratchpad::scratch_height_fraction(&ctx.state));
    ctx.state.resizing_pane = Some(ResizeSession {
        id,
        corner,
        workspace,
        start_x: x,
        start_y: y,
        start_tile_tree: ctx.state.workspace_for(workspace).tile_tree.clone(),
        start_split_ratios: ctx.state.workspace_for(workspace).split_ratios.clone(),
        start_floating_rect,
        start_scrollable_width,
        start_scratch_height,
    });
    Update::full()
}

pub(crate) fn resize_pane(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    // The corner is fixed when the resize begins; the per-event callback value is ignored in favor
    // of the session's stored corner so a jittery pointer near a border does not flip axes mid-drag.
    _corner: ResizeCorner,
    from: (u16, u16),
    current: (u16, u16),
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    // The drag delta is applied relative to the start snapshot captured by `begin_resize`. Without
    // a matching session (a follower blocked from mutating the layout, or a stray drag event) there
    // is no tree to restore first, so each event would restack the delta onto the already-resized
    // tree and compound the drag. Bail instead of resizing.
    let Some(session) = ctx.state.resizing_pane.as_ref().filter(|session| {
        session.id == id && session.start_x == from.0 && session.start_y == from.1
    }) else {
        return Update::none();
    };
    let corner = session.corner;
    let target = session.workspace;
    let start_tile_tree = session.start_tile_tree.clone();
    let start_split_ratios = session.start_split_ratios.clone();
    let start_floating_rect = session.start_floating_rect;
    let start_scrollable_width = session.start_scrollable_width;
    let start_scratch_height = session.start_scratch_height;
    let scrollable_resize = start_scrollable_width.is_some();
    if !scrollable_resize {
        // Scrollable mouse resize retains an AxisChange armed by begin_resize so strip siblings
        // can animate; other layouts keep snapping during the drag.
        ctx.state.animation = GeometryAnimation::None;
    }
    let workspace = ctx.state.workspace_for_mut(target);
    workspace.tile_tree = start_tile_tree;
    workspace.split_ratios = start_split_ratios;
    if let Some(rect) = start_floating_rect
        && let Some(pane) = active_pane_mut(&mut ctx.state, id)
    {
        pane.floating_rect = rect;
    }
    if let Some(width) = start_scrollable_width
        && let Some(pane) = ctx
            .state
            .workspace_for_mut(target)
            .panes
            .iter_mut()
            .find(|pane| pane.id == id)
    {
        pane.scrollable_width = width;
    }
    // Keep signed deltas in i32: casting through i16 wraps for |delta| > 32767 and can flip
    // grow/shrink direction on extreme pointer coordinates.
    let dx = i32::from(current.0) - i32::from(from.0);
    let dy = i32::from(current.1) - i32::from(from.1);
    let viewport = ctx.viewport();
    // Dragging the grabbed top edge up (negative dy) grows the bottom-anchored dropdown. The
    // vertical delta is spent here, so `resize_pane_state` only applies the horizontal one and
    // does not also hunt for a vertical split that the outer border has no room for.
    let dy = if let Some(start) = start_scratch_height {
        crate::scratchpad::set_height_from(&mut ctx.state, start, -dy as f32, viewport);
        0
    } else {
        dy
    };
    resize_pane_state(&mut ctx.state, id, corner, dx, dy, viewport);
    Update::full()
}

fn resize_pane_state(
    state: &mut State,
    id: PaneId,
    corner: ResizeCorner,
    dx: i32,
    dy: i32,
    viewport: Rect,
) {
    focus_pane(state, id);
    let bounds = state.layout_bounds(viewport);
    let Some(pane) = active_pane_mut(state, id) else {
        return;
    };

    if pane.fullscreen {
        return;
    }

    if pane.floating {
        pane.floating_rect =
            resize_float_rect_from_corner(pane.floating_rect, corner, dx as f32, dy as f32, bounds);
        return;
    }

    let effective_dx = match corner {
        ResizeCorner::UpperLeft | ResizeCorner::LowerLeft => -dx,
        ResizeCorner::UpperRight | ResizeCorner::LowerRight => dx,
    };
    let effective_dy = match corner {
        ResizeCorner::UpperLeft | ResizeCorner::UpperRight => -dy,
        ResizeCorner::LowerLeft | ResizeCorner::LowerRight => dy,
    };

    let layout_kind = state.active_workspace_ref().layout_kind;
    if layout_kind == LayoutKind::Master {
        let tile_bounds = workspace_tile_bounds(bounds, state.layout_top_gap());
        let focused_rect = {
            let placements = workspace_target_rects(
                state.active_workspace_ref(),
                bounds,
                state.layout_top_gap(),
                state.tile_gap(),
            );
            placement_for(&placements, id)
        };
        if focused_rect.is_some_and(|rect| {
            grabbed_edge_on_outer_border(rect, tile_bounds, corner, state::SplitAxis::Horizontal)
        }) {
            return;
        }
        let tile_gap = state.tile_gap();
        resize_master_split_by_pixels(
            state.active_workspace_mut(),
            id,
            effective_dx as f32,
            master_available_width(tile_bounds, tile_gap),
        );
        state.animation = GeometryAnimation::None;
        return;
    }
    if layout_kind == LayoutKind::Scrollable {
        // Horizontal delta only; vertical is ignored. Corner convention matches dwindle/master.
        let tile_bounds = workspace_tile_bounds(bounds, state.layout_top_gap());
        let resized = resize_scrollable_width_by_pixels(
            state.active_workspace_mut(),
            id,
            effective_dx as f32,
            tile_bounds.w.max(1.0),
        );
        if resized {
            // Sync local reveal from post-resize geometry without arming AxisChange. Do not force
            // None here — begin_resize may have armed AxisChange so strip siblings still animate.
            sync_scrollable_reveal(state, id, false);
        }
        return;
    }
    if !layout_has_resizable_splits(layout_kind) {
        return;
    }

    let tile_bounds = workspace_tile_bounds(bounds, state.layout_top_gap());
    ensure_tile_tree(state.active_workspace_mut());
    let Some(tree) = layout::effective_tile_tree(state.active_workspace_ref(), None) else {
        return;
    };

    // The grabbed corner's edge on each axis. An edge on the terminal boundary has no
    // divider to drag, so skip resizing that axis instead of inverting the inner divider.
    let focused_rect = {
        let mut placements = Vec::new();
        allocate_dwindle(&tree, tile_bounds, TileGap::DEFAULT, &mut placements);
        placement_for(&placements, id)
    };

    for (axis, pixels) in [
        (state::SplitAxis::Horizontal, effective_dx as f32),
        (state::SplitAxis::Vertical, effective_dy as f32),
    ] {
        if pixels == 0.0 {
            continue;
        }
        if focused_rect.is_some_and(|r| grabbed_edge_on_outer_border(r, tile_bounds, corner, axis))
        {
            continue;
        }
        let edge = split_edge_for_corner(axis, corner);
        let tile_gap = state.tile_gap();
        resize_tiled_split_for_edge(
            state.active_workspace_mut(),
            tile_bounds,
            tile_gap,
            id,
            axis,
            edge,
            pixels,
        );
    }

    state.animation = GeometryAnimation::None;
}

fn split_edge_for_corner(axis: state::SplitAxis, corner: ResizeCorner) -> SplitEdge {
    match axis {
        state::SplitAxis::Horizontal => match corner {
            ResizeCorner::UpperLeft | ResizeCorner::LowerLeft => SplitEdge::Leading,
            ResizeCorner::UpperRight | ResizeCorner::LowerRight => SplitEdge::Trailing,
        },
        state::SplitAxis::Vertical => match corner {
            ResizeCorner::UpperLeft | ResizeCorner::UpperRight => SplitEdge::Leading,
            ResizeCorner::LowerLeft | ResizeCorner::LowerRight => SplitEdge::Trailing,
        },
    }
}

fn drop_tiled_pane_at(state: &mut State, id: PaneId, x: u16, y: u16, viewport: Rect) {
    state.animation = GeometryAnimation::TileFloat;
    let bounds = state.layout_bounds(viewport);
    let top_gap = state.layout_top_gap();
    let tile_gap = state.tile_gap();
    let drop_point = canvas_local_point_from_mouse(
        x,
        y,
        bounds,
        state.terminal_content_left_offset(viewport),
        state.content_top_offset(),
    );
    let target = {
        let workspace = state.active_workspace_ref();
        let placements =
            workspace_target_rects_excluding(workspace, bounds, Some(id), top_gap, tile_gap);
        let tiled_ids: Vec<PaneId> = workspace
            .tiled_ids()
            .into_iter()
            .filter(|target_id| *target_id != id)
            .collect();
        target_tiled_pane_for_drop(&placements, &tiled_ids, drop_point).and_then(|target_id| {
            placement_for(&placements, target_id).map(|rect| (target_id, rect))
        })
    };

    let Some((target_id, target_rect)) = target else {
        return;
    };

    let (axis, moving_first) = layout::drop_split_for_target(target_rect, drop_point);
    // `target_rect` comes from the layout with the dragged pane already taken out, which is both
    // what the drop edge is measured against and the slot the new split will divide. That slot is
    // always halved: dropping a pane onto another is a fresh split, so it reads as one regardless
    // of how wide or tall either pane happened to be beforehand.
    let workspace = state.active_workspace_mut();
    move_tiled_window_around_target(
        workspace,
        id,
        target_id,
        axis,
        moving_first,
        EVEN_SPLIT_RATIO,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tiling::DwindleTree;
    use crate::ops::resize_move::test_util::in_test_stack;
    use crate::state::{Pane, SharedSessionState, SplitAxis};
    use crate::{AppRoot, Msg};
    use tui_lipan::TestBackend;

    /// A backend whose active workspace holds one focused floating pane at `rect`, in a 100x30
    /// viewport. `RATIO_STEP` (4%) of that canvas rounds to a 4-column / 1-row keyboard step.
    fn floating_backend(rect: FloatRect) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        {
            let state = backend.state_mut();
            let workspace = state.active_workspace_mut();
            workspace.panes.clear();
            let mut pane = Pane::new(1, 100, rect);
            pane.floating = true;
            pane.opening = false;
            workspace.panes.push(pane);
            workspace.focused_pane = Some(1);
            state.current_mut().focused_pane = Some(1);
        }
        backend.render();
        backend
    }

    fn floating_rect(backend: &mut TestBackend<AppRoot>) -> FloatRect {
        let state = backend.state_mut();
        state.current().workspaces[state.current().active_workspace].panes[0].floating_rect
    }

    #[test]
    fn both_directional_actions_slide_a_floating_pane() {
        in_test_stack(|| {
            let start = FloatRect {
                x: 20.0,
                y: 8.0,
                w: 40.0,
                h: 12.0,
            };
            // A float occupies no slot: it has nothing to trade with and nothing to be re-inserted
            // beside, so `Swap` and `Move` both degrade to the same translation.
            for action in [crate::input::Action::Swap, crate::input::Action::Move] {
                let mut backend = floating_backend(start);

                backend
                    .dispatch(Msg::RunAction(action(Direction::Right)))
                    .expect("dispatch slide right");
                backend
                    .dispatch(Msg::RunAction(action(Direction::Up)))
                    .expect("dispatch slide up");

                let moved = floating_rect(&mut backend);
                assert_eq!(
                    (moved.x, moved.y),
                    (start.x + 4.0, start.y - 1.0),
                    "a floating pane should translate by one keyboard step per press"
                );
                assert_eq!(
                    (moved.w, moved.h),
                    (start.w, start.h),
                    "sliding must not change the pane's size"
                );
            }
        });
    }

    #[test]
    fn resize_mode_grows_and_shrinks_a_floating_pane() {
        in_test_stack(|| {
            let start = FloatRect {
                x: 20.0,
                y: 8.0,
                w: 40.0,
                h: 12.0,
            };
            let mut backend = floating_backend(start);
            backend
                .dispatch(Msg::RunAction(crate::input::Action::EnterResizeMode))
                .expect("enter resize mode");

            let key = |code| KeyEvent {
                code,
                mods: KeyMods::default(),
            };
            let _ = backend.send_key(key(KeyCode::Char('l')));
            let _ = backend.send_key(key(KeyCode::Char('j')));

            let grown = floating_rect(&mut backend);
            assert_eq!(
                (grown.w, grown.h),
                (start.w + 4.0, start.h + 1.0),
                "`l`/`j` should grow a floating pane"
            );
            assert_eq!(
                (grown.x, grown.y),
                (start.x, start.y),
                "resizing anchors the top-left corner"
            );

            let _ = backend.send_key(key(KeyCode::Char('h')));
            let _ = backend.send_key(key(KeyCode::Char('k')));
            let shrunk = floating_rect(&mut backend);
            assert_eq!(
                (shrunk.w, shrunk.h),
                (start.w, start.h),
                "`h`/`k` should shrink it back"
            );
        });
    }

    #[test]
    fn resize_mode_grows_a_flush_floating_pane_inward() {
        in_test_stack(|| {
            // Flush against the right edge: the bottom-right corner has nowhere to go.
            let start = FloatRect {
                x: 60.0,
                y: 8.0,
                w: 40.0,
                h: 12.0,
            };
            let mut backend = floating_backend(start);
            backend
                .dispatch(Msg::RunAction(crate::input::Action::EnterResizeMode))
                .expect("enter resize mode");

            let _ = backend.send_key(KeyEvent {
                code: KeyCode::Char('l'),
                mods: KeyMods::default(),
            });

            let grown = floating_rect(&mut backend);
            assert_eq!(
                (grown.x, grown.w),
                (start.x - 4.0, start.w + 4.0),
                "growing at the workspace edge should extend the other side, not no-op"
            );
        });
    }

    #[test]
    fn follower_mouse_resize_leaves_the_layout_untouched() {
        in_test_stack(|| {
            let split = DwindleTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(1)),
                second: Box::new(DwindleTree::Leaf(2)),
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                // A follower: attached, but another client holds the layout lease.
                state.current_mut().session_attached = true;
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(2);
                state.current_mut().shared = Some(shared);

                let bounds = FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 30.0,
                };
                let workspace = state.active_workspace_mut();
                workspace.panes.clear();
                workspace.panes.push(Pane::new(1, 100, bounds));
                workspace.panes.push(Pane::new(2, 100, bounds));
                workspace.tile_tree = Some(split.clone());
                state.current_mut().focused_pane = Some(1);
            }
            backend.render();

            // A drag that never got a `begin_resize` (followers are nudged and blocked there)
            // must not resize: without the start snapshot every event would restack its delta
            // onto the previous one and compound the drag.
            for step in 1..=5u16 {
                backend
                    .dispatch(Msg::ResizePane(
                        1,
                        ResizeCorner::LowerRight,
                        10,
                        10,
                        10 + step,
                        10,
                        true,
                    ))
                    .expect("dispatch follower resize");
            }

            let state = backend.state_mut();
            assert_eq!(
                state.current().workspaces[state.current().active_workspace].tile_tree,
                Some(split),
                "a follower's mouse resize must not mutate the layout"
            );
            assert!(
                state.resizing_pane.is_none(),
                "no resize session should be opened for a follower"
            );
        });
    }

    fn scrollable_backend(focus: PaneId) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 28.0,
        };
        {
            let state = backend.state_mut();
            let workspace = state.active_workspace_mut();
            workspace.layout_kind = LayoutKind::Scrollable;
            workspace.panes.clear();
            for id in 1..=4 {
                workspace.panes.push(Pane::new(id, 100, bounds));
                crate::layout::tiling::append_tiled_window(workspace, id);
            }
            workspace.focused_pane = Some(focus);
            workspace.scrollable_anchor = Some(focus);
            workspace.scrollable_reveal_edge = if focus == 1 {
                crate::state::ScrollableRevealEdge::Left
            } else {
                crate::state::ScrollableRevealEdge::Right
            };
            state.current_mut().focused_pane = Some(focus);
        }
        backend.render();
        backend
    }

    #[test]
    fn scrollable_mouse_resize_uses_horizontal_corners_and_ignores_dy() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            let before = backend.state().current().workspaces[0].panes[0].scrollable_width;
            backend
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerRight, 10, 10, true))
                .expect("begin");
            backend
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::LowerRight,
                    10,
                    10,
                    20,
                    40,
                    true,
                ))
                .expect("resize right");
            let after_right = backend.state().current().workspaces[0].panes[0].scrollable_width;
            assert!(after_right > before, "right corner grows with +dx");

            backend.state_mut().current_mut().workspaces[0].panes[0].scrollable_width = before;
            backend
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerLeft, 50, 10, true))
                .expect("begin left");
            backend
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::LowerLeft,
                    50,
                    10,
                    60,
                    40,
                    true,
                ))
                .expect("resize left");
            let after_left = backend.state().current().workspaces[0].panes[0].scrollable_width;
            assert!(
                after_left < before,
                "left corner uses -dx so +pointer-x shrinks"
            );
        });
    }

    #[test]
    fn mouse_resize_extreme_delta_keeps_grow_direction() {
        // u16 delta 40000 narrows through i16 to a negative value and would flip grow→shrink.
        const EXTREME: u16 = 40_000;
        in_test_stack(|| {
            let mut scrollable = scrollable_backend(1);
            let before = scrollable.state().current().workspaces[0].panes[0].scrollable_width;
            scrollable
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerRight, 0, 0, true))
                .expect("begin scrollable");
            scrollable
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::LowerRight,
                    0,
                    0,
                    EXTREME,
                    0,
                    true,
                ))
                .expect("extreme scrollable resize");
            let after = scrollable.state().current().workspaces[0].panes[0].scrollable_width;
            assert!(
                after > before,
                "scrollable LowerRight +{EXTREME} must grow (got {after}, before {before}); i16 wrap would shrink"
            );
            assert_eq!(after, crate::state::MAX_SPLIT_RATIO);

            let start = FloatRect {
                x: 10.0,
                y: 4.0,
                w: 20.0,
                h: 10.0,
            };
            let mut floating = floating_backend(start);
            floating
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerRight, 0, 0, true))
                .expect("begin float");
            floating
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::LowerRight,
                    0,
                    0,
                    EXTREME,
                    EXTREME,
                    true,
                ))
                .expect("extreme float resize");
            let grown = floating_rect(&mut floating);
            assert!(
                grown.w > start.w && grown.h > start.h,
                "floating LowerRight extreme delta must grow, got {grown:?}"
            );
        });
    }

    #[test]
    fn scrollable_mouse_resize_deltas_do_not_compound() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            backend
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerRight, 10, 10, true))
                .expect("begin");
            for step in [5u16, 10, 15, 20] {
                backend
                    .dispatch(Msg::ResizePane(
                        1,
                        ResizeCorner::LowerRight,
                        10,
                        10,
                        10 + step,
                        10,
                        true,
                    ))
                    .expect("resize step");
            }
            let width = backend.state().current().workspaces[0].panes[0].scrollable_width;
            let expected = crate::layout::tiling::sanitize_scrollable_width(
                crate::layout::tiling::cell_split_ratio(
                    crate::layout::tiling::scrollable_column_width(
                        100.0,
                        crate::state::DEFAULT_SCROLLABLE_WIDTH,
                    ) + 20.0,
                    100.0,
                ),
            );
            assert!(
                (width - expected).abs() < 1e-5,
                "absolute delta from start, got {width} expected ~{expected}"
            );
        });
    }

    #[test]
    fn scrollable_resize_start_anchor_change_keeps_axis_change_for_siblings() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            let sibling_before = {
                let state = backend.state();
                let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let placements = workspace_target_rects(
                    &state.current().workspaces[0],
                    bounds,
                    state.workspace_top_gap(),
                    state.tile_gap(),
                );
                placement_for(&placements, 1).expect("sibling placement")
            };
            let sibling_width = backend.state().current().workspaces[0].panes[0].scrollable_width;

            backend
                .dispatch(Msg::BeginResize(4, ResizeCorner::LowerRight, 10, 10, true))
                .expect("begin resize on later pane");
            assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
            assert_eq!(backend.state().current().focused_pane, Some(4));
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(4)
            );
            assert!(
                backend
                    .state()
                    .resizing_pane
                    .as_ref()
                    .is_some_and(|session| session.id == 4),
                "active resize session keeps the resized pane on the instant transition gate"
            );

            {
                let state = backend.state();
                let resized = &state.current().workspaces[0].panes[3];
                let sibling = &state.current().workspaces[0].panes[0];
                let resized_cfg =
                    AppRoot::geometry_transition_for_pane(state, resized, false, None);
                let sibling_cfg =
                    AppRoot::geometry_transition_for_pane(state, sibling, false, None);
                assert_eq!(
                    resized_cfg.duration,
                    std::time::Duration::ZERO,
                    "resized pane stays instant via resizing_pane gate"
                );
                assert_eq!(
                    sibling_cfg.duration, state.config.animations.geometry_duration,
                    "non-resized sibling keeps configured AxisChange duration"
                );
            }

            backend
                .dispatch(Msg::ResizePane(
                    4,
                    ResizeCorner::LowerRight,
                    10,
                    10,
                    18,
                    10,
                    true,
                ))
                .expect("drag");
            assert_eq!(
                backend.state().animation,
                GeometryAnimation::AxisChange,
                "scrollable drag must retain the armed axis transition for strip siblings"
            );
            assert_eq!(
                backend.state().current().workspaces[0].panes[0].scrollable_width,
                sibling_width,
                "sibling width is pane-owned and unchanged by a peer resize"
            );
            let sibling_after = {
                let state = backend.state();
                let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let placements = workspace_target_rects(
                    &state.current().workspaces[0],
                    bounds,
                    state.workspace_top_gap(),
                    state.tile_gap(),
                );
                placement_for(&placements, 1).expect("sibling placement after")
            };
            assert!(
                (sibling_after.w - sibling_before.w).abs() < 1e-5,
                "sibling target width unchanged; only strip x should move"
            );
            assert!(
                (sibling_after.x - sibling_before.x).abs() > 0.5,
                "anchor change must shift sibling x (before {} after {})",
                sibling_before.x,
                sibling_after.x
            );
        });
    }

    fn placement_of(state: &crate::state::State, id: PaneId) -> FloatRect {
        let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        let placements = workspace_target_rects(
            &state.current().workspaces[state.current().active_workspace],
            bounds,
            state.workspace_top_gap(),
            state.tile_gap(),
        );
        placement_for(&placements, id).expect("placement")
    }

    #[test]
    fn scrollable_focus_after_resize_rearms_axis_change() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            backend
                .dispatch(Msg::BeginResize(1, ResizeCorner::LowerRight, 10, 10, true))
                .expect("begin");
            backend
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::LowerRight,
                    10,
                    10,
                    30,
                    10,
                    true,
                ))
                .expect("resize");
            assert_eq!(
                backend.state().animation,
                GeometryAnimation::None,
                "same-pane scrollable resize leaves animation None"
            );
            backend
                .dispatch(Msg::EndResize(1))
                .expect("end resize session");
            let before = placement_of(backend.state(), 1);

            for _ in 0..8 {
                backend
                    .dispatch(Msg::RunAction(crate::input::Action::Focus(
                        Direction::Right,
                    )))
                    .expect("directional focus");
                if backend.state().current().focused_pane == Some(4) {
                    break;
                }
            }
            assert_eq!(backend.state().current().focused_pane, Some(4));
            assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(4)
            );
            let after = placement_of(backend.state(), 1);
            assert!(
                (after.x - before.x).abs() > 0.5,
                "focus-scroll must shift placements (before.x={} after.x={})",
                before.x,
                after.x
            );
            let sibling = &backend.state().current().workspaces[0].panes[0];
            let cfg = AppRoot::geometry_transition_for_pane(backend.state(), sibling, false, None);
            assert_eq!(
                cfg.duration,
                backend.state().config.animations.geometry_duration
            );
            assert!(cfg.duration > std::time::Duration::ZERO);
        });
    }

    #[test]
    fn scrollable_cycle_and_click_focus_rearm_axis_change_after_none() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            backend.state_mut().animation = GeometryAnimation::None;
            let before = placement_of(backend.state(), 1);
            backend
                .dispatch(Msg::RunAction(crate::input::Action::CycleFocus(true)))
                .expect("cycle");
            assert_eq!(backend.state().current().focused_pane, Some(2));
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(1),
                "cycle onto a fully visible neighbor must preserve the viewport anchor"
            );
            assert_eq!(backend.state().animation, GeometryAnimation::None);
            assert!((placement_of(backend.state(), 1).x - before.x).abs() < 1e-5);

            backend.state_mut().animation = GeometryAnimation::None;
            let before = placement_of(backend.state(), 1);
            backend.dispatch(Msg::FocusPane(4)).expect("click focus");
            assert_eq!(backend.state().current().focused_pane, Some(4));
            assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(4)
            );
            assert!(
                (placement_of(backend.state(), 1).x - before.x).abs() > 0.5,
                "clicking an off-viewport pane must shift the strip"
            );
        });
    }

    #[test]
    fn scrollable_resize_drag_does_not_rearm_when_anchor_unchanged() {
        in_test_stack(|| {
            let mut backend = scrollable_backend(1);
            backend
                .dispatch(Msg::BeginResize(4, ResizeCorner::LowerRight, 10, 10, true))
                .expect("begin on other pane");
            assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
            backend.state_mut().animation = GeometryAnimation::None;
            backend
                .dispatch(Msg::ResizePane(
                    4,
                    ResizeCorner::LowerRight,
                    10,
                    10,
                    20,
                    10,
                    true,
                ))
                .expect("drag");
            assert_eq!(
                backend.state().animation,
                GeometryAnimation::None,
                "drag must not re-arm once the Scrollable anchor is unchanged"
            );
        });
    }
    /// A tiled pane dragged inside the dropdown follows the pointer as a live preview, clamped to
    /// the dropdown rather than to the whole canvas - the same gesture as in a workspace, measured
    /// against the box the scratch workspace actually occupies.
    #[test]
    fn dragging_a_scratch_pane_previews_inside_the_dropdown() {
        in_test_stack(|| {
            let viewport = Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            {
                let state = backend.state_mut();
                let mut pane = Pane::new(1, 100, FloatRect::default());
                pane.opening = false;
                state.scratch.panes.push(pane);
                crate::layout::tiling::append_tiled_window(&mut state.scratch, 1);
                state.scratch.focused_pane = Some(1);
                state.scratch_visible = true;
            }
            backend.render();
            let dropdown = crate::scratchpad::deployed_rect(backend.state(), viewport);
            let start = FloatRect {
                x: dropdown.x,
                y: dropdown.y,
                w: 20.0,
                h: 5.0,
            };

            backend
                .dispatch(Msg::BeginMove(1, start, 0, 0, 20, 5, true))
                .expect("begin move");
            backend
                .dispatch(Msg::MovePane(1, 6, -20, true))
                .expect("drag");

            let session = backend.state().moving_pane.expect("drag session");
            assert_eq!(session.drag_rect.x, start.x + 6.0, "tracks the pointer");
            assert!(
                session.drag_rect.y >= dropdown.y - 0.5,
                "a drag toward the workspace stops at the dropdown's top edge: {:?} vs {dropdown:?}",
                session.drag_rect
            );
        });
    }
    /// A right-drag grabbing an upper corner of a top-edge scratch pane moves the dropdown's own
    /// border - there is no split above it - while the horizontal half still resizes the pane.
    #[test]
    fn right_dragging_a_top_edge_scratch_pane_resizes_the_dropdown() {
        in_test_stack(|| {
            let viewport = Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            {
                let state = backend.state_mut();
                for id in 1..=2 {
                    let mut pane = Pane::new(id, 100, FloatRect::default());
                    pane.opening = false;
                    state.scratch.panes.push(pane);
                }
                // Stacked, so only pane 1 sits against the dropdown's top edge.
                state.scratch.tile_tree = Some(crate::layout::tiling::DwindleTree::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(crate::layout::tiling::DwindleTree::Leaf(1)),
                    second: Box::new(crate::layout::tiling::DwindleTree::Leaf(2)),
                });
                state.scratch.focused_pane = Some(1);
                state.scratch_visible = true;
            }
            backend.render();
            let start = crate::scratchpad::scratch_height_fraction(backend.state());
            let tree = backend.state().scratch.tile_tree.clone();

            backend
                .dispatch(Msg::BeginResize(1, ResizeCorner::UpperLeft, 10, 20, true))
                .expect("begin resize");
            backend
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::UpperLeft,
                    10,
                    20,
                    10,
                    16,
                    true,
                ))
                .expect("drag up");
            let grown = crate::scratchpad::scratch_height_fraction(backend.state());
            assert!(
                grown > start,
                "dragging up grows the dropdown: {grown} vs {start}"
            );
            assert_eq!(
                backend.state().scratch.tile_tree,
                tree,
                "the inner split belongs to the pane below, not to the dropdown edge"
            );

            // Absolute from the drag origin, so reversing past the start shrinks it again.
            backend
                .dispatch(Msg::ResizePane(
                    1,
                    ResizeCorner::UpperLeft,
                    10,
                    20,
                    10,
                    24,
                    true,
                ))
                .expect("drag back down");
            assert!(crate::scratchpad::scratch_height_fraction(backend.state()) < start);

            // The lower pane has a split above it, so its right-drag stays an ordinary resize.
            backend.state_mut().scratch.focused_pane = Some(2);
            let height = crate::scratchpad::scratch_height_fraction(backend.state());
            backend
                .dispatch(Msg::BeginResize(2, ResizeCorner::UpperLeft, 10, 25, true))
                .expect("begin resize");
            backend
                .dispatch(Msg::ResizePane(
                    2,
                    ResizeCorner::UpperLeft,
                    10,
                    25,
                    10,
                    22,
                    true,
                ))
                .expect("drag");
            assert_eq!(
                crate::scratchpad::scratch_height_fraction(backend.state()),
                height
            );
            assert_ne!(backend.state().scratch.tile_tree, tree, "the split moved");
        });
    }

    /// A dropped pane halves whatever slot it lands in. The drop is a fresh split, so a deliberate
    /// 75/25 on the pair the pane is leaving does not ride along to the pane it lands on.
    #[test]
    fn dropping_a_tiled_pane_halves_the_slot_it_lands_in() {
        in_test_stack(|| {
            let viewport = Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            {
                let state = backend.state_mut();
                let workspace = state.active_workspace_mut();
                workspace.panes.clear();
                for id in [1, 2] {
                    let mut pane = Pane::new(id, 100, FloatRect::default());
                    pane.opening = false;
                    workspace.panes.push(pane);
                }
                workspace.layout_kind = LayoutKind::Dwindle;
                workspace.tile_tree = Some(DwindleTree::Split {
                    axis: SplitAxis::Horizontal,
                    ratio: 0.75,
                    first: Box::new(DwindleTree::Leaf(1)),
                    second: Box::new(DwindleTree::Leaf(2)),
                });
                workspace.focused_pane = Some(1);
                state.current_mut().focused_pane = Some(1);
            }
            backend.render();

            // Aim at the right third of pane 2's slot, so pane 1 docks after it on the horizontal
            // axis - the direction is incidental, the ratio is what the test is about.
            let (x, y) = {
                let state = backend.state_mut();
                let bounds = state.layout_bounds(viewport);
                let top_gap = state.layout_top_gap();
                let tile_gap = state.tile_gap();
                let left_offset = state.terminal_content_left_offset(viewport);
                let top_offset = state.content_top_offset();
                let workspace = state.active_workspace_ref();
                let placements =
                    workspace_target_rects_excluding(workspace, bounds, Some(1), top_gap, tile_gap);
                let slot = placement_for(&placements, 2).expect("pane 2 drop slot");
                (
                    (slot.x + slot.w * 0.8).round() as u16 + left_offset,
                    (slot.y + slot.h * 0.5).round() as u16 + top_offset,
                )
            };
            drop_tiled_pane_at(backend.state_mut(), 1, x, y, viewport);

            let tree = backend.state().active_workspace_ref().tile_tree.clone();
            let Some(DwindleTree::Split { ratio, .. }) = tree else {
                panic!("the drop leaves a two-pane split: {tree:?}");
            };
            assert!(
                (ratio - EVEN_SPLIT_RATIO).abs() < 1e-3,
                "the dropped pane halves its new slot, got {ratio}"
            );
        });
    }
}
