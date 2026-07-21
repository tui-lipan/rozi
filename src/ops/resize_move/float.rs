use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{
    canvas_local_point_from_mouse, clamp_floating_rect, grabbed_edge_on_outer_border,
    resize_float_rect_from_corner, workspace_tile_bounds,
};
use crate::layout::{
    self, placement_for, target_tiled_pane_for_drop, workspace_target_rects,
    workspace_target_rects_excluding,
};
use crate::ops::focus::{active_pane_mut, focus_pane, request_pane_focus};
use crate::state::{
    self, Direction, LayoutKind, MoveSession, PaneId, ResizeCorner, ResizeSession, State, TileGap,
    Workspace,
};
use crate::tiling::{
    SplitEdge, allocate_dwindle, move_tiled_window_around_target, resize_tiled_split_for_edge,
    split_available_for_edge,
};

use super::tiling::{master_available_width, resize_master_split_by_pixels};

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

/// One keyboard step along a workspace axis of `available` cells. Proportional so it feels like
/// the tiled `RATIO_STEP`, but snapped to whole cells: a floating pane's PTY is sized in cells, so
/// a fractional step would drift the border without ever changing the terminal.
fn float_keyboard_step(available: f32) -> f32 {
    (state::RATIO_STEP * available).round().max(1.0)
}

fn float_keyboard_delta(direction: Direction, bounds: FloatRect) -> (f32, f32) {
    match direction {
        Direction::Left => (-float_keyboard_step(bounds.w), 0.0),
        Direction::Right => (float_keyboard_step(bounds.w), 0.0),
        Direction::Up => (0.0, -float_keyboard_step(bounds.h)),
        Direction::Down => (0.0, float_keyboard_step(bounds.h)),
    }
}

/// Translate the focused floating pane one step. Returns whether the focus was floating at all, so
/// `super::tiling::reorder_focused_in_direction` can fall through to the tiled reorder when it was
/// not.
pub(super) fn move_focused_float(ctx: &mut Context<HyprmuxApp>, direction: Direction) -> bool {
    let Some(id) = ctx.state.focused_pane else {
        return false;
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
pub(super) fn resize_focused_float(ctx: &mut Context<HyprmuxApp>, direction: Direction) -> bool {
    let Some(id) = ctx.state.focused_pane else {
        return false;
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
    ctx: &mut Context<HyprmuxApp>,
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
    if crate::ops::session::nudge_if_follower(ctx) {
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
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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

pub(crate) fn end_move(ctx: &mut Context<HyprmuxApp>, id: PaneId, x: u16, y: u16) -> Update {
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
pub(super) fn finish_pointer_layout_interaction(ctx: &mut Context<HyprmuxApp>) {
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
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    if crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    ctx.state.animation = GeometryAnimation::None;
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    let workspace = ctx.state.active_workspace;
    ensure_tile_tree(&mut ctx.state.workspaces[workspace]);
    let start_floating_rect = active_pane_mut(&mut ctx.state, id)
        .filter(|pane| pane.floating)
        .map(|pane| pane.floating_rect);
    ctx.state.resizing_pane = Some(ResizeSession {
        id,
        corner,
        workspace,
        start_x: x,
        start_y: y,
        start_tile_tree: ctx.state.workspaces[workspace].tile_tree.clone(),
        start_split_ratios: ctx.state.workspaces[workspace].split_ratios.clone(),
        start_floating_rect,
    });
    Update::full()
}

pub(crate) fn resize_pane(
    ctx: &mut Context<HyprmuxApp>,
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
    ctx.state.animation = GeometryAnimation::None;
    let corner = session.corner;
    let workspace = &mut ctx.state.workspaces[session.workspace];
    workspace.tile_tree = session.start_tile_tree.clone();
    workspace.split_ratios = session.start_split_ratios.clone();
    if let Some(rect) = session.start_floating_rect
        && let Some(pane) = active_pane_mut(&mut ctx.state, id)
    {
        pane.floating_rect = rect;
    }
    let dx = (current.0 as i32 - from.0 as i32) as i16;
    let dy = (current.1 as i32 - from.1 as i32) as i16;
    let viewport = ctx.viewport();
    resize_pane_state(&mut ctx.state, id, corner, dx, dy, viewport);
    Update::full()
}

fn resize_pane_state(
    state: &mut State,
    id: PaneId,
    corner: ResizeCorner,
    dx: i16,
    dy: i16,
    viewport: Rect,
) {
    focus_pane(state, id);
    let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
    let Some(pane) = active_pane_mut(state, id) else {
        return;
    };

    if pane.fullscreen {
        return;
    }

    if pane.floating {
        pane.floating_rect = resize_float_rect_from_corner(
            pane.floating_rect,
            corner,
            f32::from(dx),
            f32::from(dy),
            bounds,
        );
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

    let layout_kind = state.workspaces[state.active_workspace].layout_kind;
    if layout_kind == LayoutKind::Master {
        let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
        let tile_bounds = workspace_tile_bounds(bounds, state.workspace_top_gap());
        let focused_rect = {
            let placements = workspace_target_rects(
                &state.workspaces[state.active_workspace],
                bounds,
                state.workspace_top_gap(),
                state.tile_gap(),
            );
            placement_for(&placements, id)
        };
        if focused_rect.is_some_and(|rect| {
            grabbed_edge_on_outer_border(rect, tile_bounds, corner, state::SplitAxis::Horizontal)
        }) {
            return;
        }
        resize_master_split_by_pixels(
            &mut state.workspaces[state.active_workspace],
            id,
            f32::from(effective_dx),
            master_available_width(tile_bounds),
        );
        state.animation = GeometryAnimation::None;
        return;
    }
    if !layout_has_resizable_splits(layout_kind) {
        return;
    }

    let tile_bounds = workspace_tile_bounds(bounds, state.workspace_top_gap());
    ensure_tile_tree(&mut state.workspaces[state.active_workspace]);
    let Some(tree) = layout::effective_tile_tree(&state.workspaces[state.active_workspace], None)
    else {
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
        (state::SplitAxis::Horizontal, f32::from(effective_dx)),
        (state::SplitAxis::Vertical, f32::from(effective_dy)),
    ] {
        if pixels == 0.0 {
            continue;
        }
        if focused_rect.is_some_and(|r| grabbed_edge_on_outer_border(r, tile_bounds, corner, axis))
        {
            continue;
        }
        let edge = split_edge_for_corner(axis, corner);
        if let Some(available) =
            split_available_for_edge(&tree, tile_bounds, TileGap::DEFAULT, id, axis, edge)
        {
            resize_tiled_split_for_edge(
                &mut state.workspaces[state.active_workspace],
                id,
                axis,
                edge,
                available,
                pixels,
            );
        }
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
    let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
    let top_gap = state.workspace_top_gap();
    let tile_gap = state.tile_gap();
    let drop_point = canvas_local_point_from_mouse(
        x,
        y,
        bounds,
        state.terminal_content_left_offset(viewport),
        state.content_top_offset(),
    );
    let target = {
        let workspace = &state.workspaces[state.active_workspace];
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
    let workspace = &mut state.workspaces[state.active_workspace];
    move_tiled_window_around_target(workspace, id, target_id, axis, moving_first);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::resize_move::test_util::in_test_stack;
    use crate::state::{Pane, SharedSessionState, SplitAxis};
    use crate::tiling::DwindleTree;
    use crate::{HyprmuxApp, Msg};
    use tui_lipan::TestBackend;

    /// A backend whose active workspace holds one focused floating pane at `rect`, in a 100x30
    /// viewport. `RATIO_STEP` (4%) of that canvas rounds to a 4-column / 1-row keyboard step.
    fn floating_backend(rect: FloatRect) -> TestBackend<HyprmuxApp> {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        {
            let state = backend.state_mut();
            let workspace = &mut state.workspaces[state.active_workspace];
            workspace.panes.clear();
            let mut pane = Pane::new(1, 100, rect);
            pane.floating = true;
            pane.opening = false;
            workspace.panes.push(pane);
            workspace.focused_pane = Some(1);
            state.focused_pane = Some(1);
        }
        backend.render();
        backend
    }

    fn floating_rect(backend: &mut TestBackend<HyprmuxApp>) -> FloatRect {
        let state = backend.state_mut();
        state.workspaces[state.active_workspace].panes[0].floating_rect
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
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                // A follower: attached, but another client holds the layout lease.
                state.session_attached = true;
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(2);
                state.shared = Some(shared);

                let bounds = FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 30.0,
                };
                let workspace = &mut state.workspaces[state.active_workspace];
                workspace.panes.clear();
                workspace.panes.push(Pane::new(1, 100, bounds));
                workspace.panes.push(Pane::new(2, 100, bounds));
                workspace.tile_tree = Some(split.clone());
                state.focused_pane = Some(1);
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
                state.workspaces[state.active_workspace].tile_tree,
                Some(split),
                "a follower's mouse resize must not mutate the layout"
            );
            assert!(
                state.resizing_pane.is_none(),
                "no resize session should be opened for a follower"
            );
        });
    }
}
