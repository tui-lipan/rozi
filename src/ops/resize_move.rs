use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{
    canvas_local_point_from_mouse, clamp_float_rect, clamp_floating_rect, directional_score,
    grabbed_edge_on_outer_border, lift_off_float_rect, resize_float_rect_from_corner,
    workspace_tile_bounds,
};
use crate::layout::{
    self, insert_tiled_pane_at_point, placement_for, target_tiled_pane_for_drop,
    workspace_target_rects, workspace_target_rects_excluding,
};
use crate::ops::focus::{
    active_pane_is_fullscreen, active_pane_mut, focus_pane, request_pane_focus,
};
use crate::state::{
    self, Direction, LayoutKind, MoveSession, MoveSwapHint, PaneId, RATIO_STEP, ResizeCorner,
    ResizeSession, SplitDragKind, SplitDragSession, State, TILE_GAP, TileGap, Workspace,
};
use crate::tiling::{
    SplitEdge, adjust_ratio_value, adjust_tree_split_for_focused, allocate_dwindle,
    append_tiled_window, flip_tree_split_for_focused, focused_is_first_in_nearest_axis_split,
    move_tiled_window_around_target, nearest_split_available, ratio_at, remove_tiled_window,
    resize_tiled_split, resize_tiled_split_for_edge, split_available_for_edge, swap_tree_leaves,
};

/// Whether tree-based split resizing applies to this layout. Grid and monocle place panes
/// purely by formula - there is no ratio to adjust, and writing into the dwindle tree
/// would silently rearrange the layouts that do read it.
fn layout_has_resizable_splits(kind: LayoutKind) -> bool {
    kind == LayoutKind::Dwindle
}

fn ensure_tile_tree(workspace: &mut Workspace) {
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_move(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    current_rect: FloatRect,
    _from_local_x: u16,
    _from_local_y: u16,
    _target_w: u16,
    _target_h: u16,
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
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
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
    let bounds = state.canvas_bounds(viewport);
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
        let bounds = state.canvas_bounds(viewport);
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
    let bounds = state.canvas_bounds(viewport);
    let top_gap = state.workspace_top_gap();
    let tile_gap = state.tile_gap();
    let drop_point = canvas_local_point_from_mouse(x, y, bounds, state.content_top_offset());
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

pub(crate) fn toggle_tiling(ctx: &mut Context<HyprmuxApp>) {
    let Some(id) = ctx.state.focused_pane else {
        return;
    };
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let current_rect = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        placement_for(
            &workspace_target_rects(workspace, bounds, top_gap, tile_gap),
            id,
        )
    };

    let mut insert_tiled_at = None;
    let mut remove_from_tiling = false;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        pane.fullscreen = false;
        if pane.floating {
            pane.floating_rect = clamp_float_rect(pane.floating_rect, bounds);
            insert_tiled_at = Some(crate::geometry::rect_center(pane.floating_rect));
            pane.floating = false;
            ctx.state.animation = GeometryAnimation::TileFloat;
        } else {
            pane.floating_rect = match current_rect {
                Some(tile) => lift_off_float_rect(tile, pane.floating_rect, bounds),
                None => clamp_float_rect(pane.floating_rect, bounds),
            };
            pane.floating = true;
            remove_from_tiling = true;
            ctx.state.animation = GeometryAnimation::TileFloat;
        }
    }

    if insert_tiled_at.is_some() || remove_from_tiling {
        let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
        if let Some(point) = insert_tiled_at {
            if insert_tiled_pane_at_point(workspace, id, point, bounds, top_gap, tile_gap).is_none()
            {
                append_tiled_window(workspace, id);
            }
        } else if remove_from_tiling {
            remove_tiled_window(workspace, id);
        }
    }
    request_pane_focus(ctx, id);
}

pub(crate) fn toggle_fullscreen(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds, top_gap, tile_gap)
    };

    let mut toggled = false;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        if !pane.fullscreen && pane.floating {
            pane.floating_rect = placement_for(&placements, id).unwrap_or(pane.floating_rect);
        }
        pane.fullscreen = !pane.fullscreen;
        toggled = true;
    }
    if toggled {
        ctx.state.animation = GeometryAnimation::Fullscreen;
        request_pane_focus(ctx, id);
    }
    Update::full()
}

pub(crate) fn toggle_focused_split_axis(state: &mut State) {
    let Some(focused) = state.focused_pane else {
        return;
    };
    let workspace = &mut state.workspaces[state.active_workspace];
    // Only dwindle renders the stored split axes: master/grid/monocle place panes by
    // formula, so flipping would change nothing on screen while still scrambling the tree
    // dwindle falls back to.
    if workspace.layout_kind != LayoutKind::Dwindle {
        return;
    }
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&focused)
    {
        return;
    }
    workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return;
    };
    if flip_tree_split_for_focused(tree, focused, 0).is_some() {
        workspace.last_move_swap = None;
        state.animation = GeometryAnimation::AxisChange;
    }
}

pub(crate) fn adjust_focused_split_ratio(state: &mut State, delta: f32) {
    let Some(focused) = state.focused_pane else {
        return;
    };
    let workspace = &mut state.workspaces[state.active_workspace];
    if workspace.layout_kind == LayoutKind::Master {
        if adjust_master_split_for_focused(workspace, focused, delta) {
            state.animation = GeometryAnimation::None;
        }
        return;
    }
    if !layout_has_resizable_splits(workspace.layout_kind) {
        return;
    }
    ensure_tile_tree(workspace);
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return;
    };
    if adjust_tree_split_for_focused(tree, focused, delta, 0).is_some() {
        state.animation = GeometryAnimation::None;
    }
}

pub(crate) fn toggle_layout(ctx: &mut Context<HyprmuxApp>, show_toast: bool) {
    let workspace_index = ctx.state.active_workspace;
    let layout_label = {
        let workspace = &mut ctx.state.workspaces[workspace_index];
        workspace.layout_kind = workspace.layout_kind.toggled();
        workspace.last_move_swap = None;
        workspace.layout_kind.label()
    };
    ctx.state.animation = GeometryAnimation::AxisChange;
    if show_toast {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            format!("Layout mode: {layout_label}"),
        ));
    }
}

fn adjust_master_split_for_focused(workspace: &mut Workspace, focused: PaneId, delta: f32) -> bool {
    let ids = workspace.tiled_ids();
    if ids.len() < 2 || !ids.contains(&focused) {
        return false;
    }
    let signed_delta = if ids.first() == Some(&focused) {
        delta
    } else {
        -delta
    };
    if workspace.split_ratios.is_empty() {
        workspace.split_ratios.push(crate::state::DEFAULT_RATIO);
    }
    workspace.split_ratios[0] =
        adjust_ratio_value(ratio_at(&workspace.split_ratios, 0), signed_delta);
    true
}

fn resize_master_split_by_pixels(
    workspace: &mut Workspace,
    focused: PaneId,
    pixels: f32,
    available: f32,
) -> bool {
    if pixels == 0.0 || available <= 0.0 {
        return false;
    }
    adjust_master_split_for_focused(workspace, focused, pixels / available.max(1.0))
}

fn master_available_width(tile_bounds: FloatRect) -> f32 {
    let gap = if tile_bounds.w > TILE_GAP {
        TILE_GAP
    } else {
        0.0
    };
    (tile_bounds.w - gap).max(1.0)
}

pub(crate) fn move_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    reorder_focused_in_direction(ctx, direction);
}

/// Exchange the focused pane with its directional neighbor, keeping focus on the moved pane.
/// This trades the two panes' slots in place. No-op for a floating/fullscreen focus or when
/// there is no neighbor in that direction.
pub(crate) fn swap_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    reorder_focused_in_direction(ctx, direction);
}

fn reorder_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let workspace_index = ctx.state.active_workspace;
    let Some(focused) = ctx.state.focused_pane else {
        return;
    };
    if active_pane_is_fullscreen(&ctx.state, focused) {
        return;
    }

    let workspace = &mut ctx.state.workspaces[workspace_index];
    if swap_tiled_neighbor_in_direction(workspace, bounds, top_gap, tile_gap, focused, direction) {
        workspace.focused_pane = Some(focused);
        ctx.state.focused_pane = Some(focused);
        ctx.state.animation = GeometryAnimation::AxisChange;
    }
}

fn swap_tiled_neighbor_in_direction(
    workspace: &mut Workspace,
    bounds: FloatRect,
    top_gap: f32,
    tile_gap: TileGap,
    focused: PaneId,
    direction: Direction,
) -> bool {
    let target_id = {
        let tiled_ids = workspace.active_tiled_ids_by_pane_order();
        if !tiled_ids.contains(&focused) {
            return false;
        }
        let placements: Vec<_> = workspace_target_rects(workspace, bounds, top_gap, tile_gap)
            .into_iter()
            .filter(|placement| tiled_ids.contains(&placement.id))
            .collect();
        remembered_swap_target(workspace, &placements, focused, direction)
            .or_else(|| strict_directional_neighbor(&placements, focused, direction))
    };

    let Some(target_id) = target_id else {
        return false;
    };
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return false;
    };
    let swapped = swap_tree_leaves(tree, focused, target_id);
    if swapped {
        workspace.last_move_swap = Some(MoveSwapHint {
            pane: focused,
            return_direction: opposite_direction(direction),
            target: target_id,
        });
    }
    swapped
}

fn remembered_swap_target(
    workspace: &Workspace,
    placements: &[crate::tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let hint = workspace.last_move_swap?;
    if hint.pane != focused || hint.return_direction != direction {
        return None;
    }

    let current = placements
        .iter()
        .find(|placement| placement.id == focused)?;
    let target = placements
        .iter()
        .find(|placement| placement.id == hint.target)?;
    is_strict_directional_candidate(current.rect, target.rect, direction).then_some(hint.target)
}

fn strict_directional_neighbor(
    placements: &[crate::tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let current = placements
        .iter()
        .find(|candidate| candidate.id == focused)?;
    placements
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            is_strict_directional_candidate(current.rect, candidate.rect, direction).then(|| {
                directional_score(current.rect, candidate.rect, direction).map(|score| {
                    let cross_offset =
                        cross_axis_center_offset(current.rect, candidate.rect, direction);
                    (candidate.id, score, cross_offset)
                })
            })?
        })
        .min_by(|(_, a_score, a_cross), (_, b_score, b_cross)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| a_cross.total_cmp(b_cross))
        })
        .map(|(id, _, _)| id)
}

fn is_strict_directional_candidate(
    current: FloatRect,
    candidate: FloatRect,
    direction: Direction,
) -> bool {
    directional_score(current, candidate, direction).is_some()
        && cross_axis_overlap(current, candidate, direction) > 0.0
}

fn cross_axis_overlap(current: FloatRect, candidate: FloatRect, direction: Direction) -> f32 {
    match direction {
        Direction::Left | Direction::Right => interval_overlap(
            current.y,
            current.y + current.h,
            candidate.y,
            candidate.y + candidate.h,
        ),
        Direction::Up | Direction::Down => interval_overlap(
            current.x,
            current.x + current.w,
            candidate.x,
            candidate.x + candidate.w,
        ),
    }
}

fn interval_overlap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

fn cross_axis_center_offset(current: FloatRect, candidate: FloatRect, direction: Direction) -> f32 {
    match direction {
        Direction::Left | Direction::Right => {
            ((current.y + current.h / 2.0) - (candidate.y + candidate.h / 2.0)).abs()
        }
        Direction::Up | Direction::Down => {
            ((current.x + current.w / 2.0) - (candidate.x + candidate.w / 2.0)).abs()
        }
    }
}

fn opposite_direction(direction: Direction) -> Direction {
    match direction {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
    }
}

pub(crate) fn begin_resize_split_drag(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    x: u16,
    y: u16,
) -> Update {
    begin_split_drag(
        ctx,
        SplitDragKind::Single {
            pane_id,
            horizontal_split,
        },
        x,
        y,
    )
}

pub(crate) fn begin_resize_split_junction_drag(
    ctx: &mut Context<HyprmuxApp>,
    left_id: PaneId,
    top_id: PaneId,
    x: u16,
    y: u16,
) -> Update {
    begin_split_drag(ctx, SplitDragKind::Junction { left_id, top_id }, x, y)
}

fn begin_split_drag(ctx: &mut Context<HyprmuxApp>, kind: SplitDragKind, x: u16, y: u16) -> Update {
    if crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    let workspace_index = ctx.state.active_workspace;
    let workspace = &mut ctx.state.workspaces[workspace_index];
    ensure_tile_tree(workspace);
    ctx.state.split_drag = Some(SplitDragSession {
        kind,
        workspace: workspace_index,
        start_x: x,
        start_y: y,
        start_tile_tree: workspace.tile_tree.clone(),
        start_split_ratios: workspace.split_ratios.clone(),
    });
    Update::none()
}

fn ensure_split_drag(ctx: &mut Context<HyprmuxApp>, kind: SplitDragKind, from_x: u16, from_y: u16) {
    // Followers are nudged once at drag start; do not re-nudge (via `begin_split_drag`) on every
    // subsequent drag event, which would stack a toast per pointer move. A follower never has a
    // session, so `restore_split_drag` bails the resize regardless.
    if !ctx.state.is_controller() {
        return;
    }
    let workspace = ctx.state.active_workspace;
    let matches = ctx.state.split_drag.as_ref().is_some_and(|session| {
        session.kind == kind
            && session.workspace == workspace
            && session.start_x == from_x
            && session.start_y == from_y
    });
    if !matches {
        begin_split_drag(ctx, kind, from_x, from_y);
    }
}

fn restore_split_drag(ctx: &mut Context<HyprmuxApp>, kind: SplitDragKind) -> bool {
    let Some(session) =
        ctx.state.split_drag.as_ref().filter(|session| {
            session.kind == kind && session.workspace == ctx.state.active_workspace
        })
    else {
        return false;
    };
    let workspace = &mut ctx.state.workspaces[session.workspace];
    workspace.tile_tree = session.start_tile_tree.clone();
    workspace.split_ratios = session.start_split_ratios.clone();
    true
}

/// Adjust the split on a tiled boundary by a mouse drag. `pane_id` is the pane on the
/// left/top side of the dragged gap; `horizontal_split` is true for a vertical gap (a
/// left|right split). Used by the draggable gap strips in the view. Dwindle and master only.
pub(crate) fn resize_split_by_drag(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let kind = SplitDragKind::Single {
        pane_id,
        horizontal_split,
    };
    ensure_split_drag(ctx, kind, from_x, from_y);
    if !restore_split_drag(ctx, kind) {
        return Update::none();
    }
    let pixels = if horizontal_split {
        x as i32 - from_x as i32
    } else {
        y as i32 - from_y as i32
    } as f32;
    apply_resize_split_pixels(ctx, pane_id, horizontal_split, pixels);
    Update::full()
}

fn apply_resize_split_pixels(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    pixels: f32,
) -> bool {
    if active_pane_is_fullscreen(&ctx.state, pane_id) {
        return false;
    }
    let axis = if horizontal_split {
        state::SplitAxis::Horizontal
    } else {
        state::SplitAxis::Vertical
    };

    let workspace_index = ctx.state.active_workspace;
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let tile_bounds = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap());
    let workspace = &mut ctx.state.workspaces[workspace_index];
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&pane_id)
    {
        return false;
    }

    if workspace.layout_kind == LayoutKind::Master {
        if axis != state::SplitAxis::Horizontal {
            return false;
        }
        let available = master_available_width(tile_bounds);
        if resize_master_split_by_pixels(workspace, pane_id, pixels, available) {
            ctx.state.animation = GeometryAnimation::None;
            return true;
        }
        return false;
    }
    if !layout_has_resizable_splits(workspace.layout_kind) {
        return false;
    }

    ensure_tile_tree(workspace);
    let Some(tree) = workspace.tile_tree.as_ref() else {
        return false;
    };
    let Some(available) =
        nearest_split_available(tree, tile_bounds, TileGap::DEFAULT, pane_id, axis)
    else {
        return false;
    };
    if resize_tiled_split(workspace, pane_id, axis, available, pixels) {
        ctx.state.animation = GeometryAnimation::None;
        return true;
    }
    false
}

pub(crate) fn resize_split_junction_by_drag(
    ctx: &mut Context<HyprmuxApp>,
    left_id: PaneId,
    top_id: PaneId,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let kind = SplitDragKind::Junction { left_id, top_id };
    ensure_split_drag(ctx, kind, from_x, from_y);
    if !restore_split_drag(ctx, kind) {
        return Update::none();
    }
    let dx = (x as i32 - from_x as i32) as f32;
    let dy = (y as i32 - from_y as i32) as f32;
    apply_resize_split_pixels(ctx, left_id, true, dx);
    apply_resize_split_pixels(ctx, top_id, false, dy);
    Update::full()
}

pub(crate) fn resize_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    let Some(focused) = ctx.state.focused_pane else {
        return;
    };
    if active_pane_is_fullscreen(&ctx.state, focused) {
        return;
    }
    let workspace_index = ctx.state.active_workspace;
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let tile_bounds = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap());
    let workspace = &mut ctx.state.workspaces[workspace_index];
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&focused)
    {
        return;
    }

    if workspace.layout_kind == LayoutKind::Master {
        let axis = crate::ops::focus::split_axis_for_direction(direction);
        if axis != state::SplitAxis::Horizontal {
            return;
        }
        let available = master_available_width(tile_bounds);
        let ids = workspace.tiled_ids();
        let focused_is_first = ids.first() == Some(&focused);
        let pixels = keyboard_resize_pixels(direction, focused_is_first, available);
        if resize_master_split_by_pixels(workspace, focused, pixels, available) {
            ctx.state.animation = GeometryAnimation::None;
        }
        return;
    }
    if !layout_has_resizable_splits(workspace.layout_kind) {
        return;
    }

    ensure_tile_tree(workspace);
    let Some(tree) = workspace.tile_tree.as_ref() else {
        return;
    };

    let axis = crate::ops::focus::split_axis_for_direction(direction);
    let Some(available) =
        nearest_split_available(tree, tile_bounds, TileGap::DEFAULT, focused, axis)
    else {
        return;
    };
    let Some(focused_is_first) = focused_is_first_in_nearest_axis_split(tree, focused, axis) else {
        return;
    };
    let pixels = keyboard_resize_pixels(direction, focused_is_first, available);
    if resize_tiled_split(workspace, focused, axis, available, pixels) {
        ctx.state.animation = GeometryAnimation::None;
    }
}

fn keyboard_resize_pixels(direction: Direction, focused_is_first: bool, available: f32) -> f32 {
    let grows_focused = match direction {
        Direction::Left | Direction::Up => !focused_is_first,
        Direction::Right | Direction::Down => focused_is_first,
    };
    let pixels = RATIO_STEP * available;
    if grows_focused { pixels } else { -pixels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Pane, SplitAxis};
    use crate::tiling::DwindleTree;

    #[test]
    fn keyboard_resize_directions_grow_toward_the_nearest_split() {
        let available = 100.0;
        let step = RATIO_STEP * available;

        assert_eq!(
            keyboard_resize_pixels(Direction::Right, true, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Left, true, available),
            -step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Left, false, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Right, false, available),
            -step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Down, true, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Up, false, available),
            step
        );
    }

    #[test]
    fn absolute_split_drag_preserves_clamp_overshoot_until_cursor_returns_to_handle() {
        let start_tree = DwindleTree::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(DwindleTree::Leaf(1)),
            second: Box::new(DwindleTree::Leaf(2)),
        };
        let workspace_for_delta = |pixels: f32| {
            let mut workspace = Workspace::new(0);
            workspace
                .panes
                .push(Pane::new(1, 100, FloatRect::default()));
            workspace
                .panes
                .push(Pane::new(2, 100, FloatRect::default()));
            workspace.tile_tree = Some(start_tree.clone());
            resize_tiled_split(&mut workspace, 1, SplitAxis::Horizontal, 100.0, pixels);
            workspace
        };

        assert_eq!(root_ratio(&workspace_for_delta(60.0)), 0.8);
        assert_eq!(root_ratio(&workspace_for_delta(50.0)), 0.8);
        assert_eq!(root_ratio(&workspace_for_delta(20.0)), 0.7);
    }

    #[test]
    fn directional_move_swaps_with_neighbor_instead_of_splitting_target() {
        let (bounds, mut workspace) = three_pane_stack_workspace();

        assert!(swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            3,
            Direction::Left,
        ));

        assert_eq!(
            workspace.tile_tree,
            Some(DwindleTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(3)),
                second: Box::new(DwindleTree::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(DwindleTree::Leaf(2)),
                    second: Box::new(DwindleTree::Leaf(1)),
                }),
            })
        );
    }

    #[test]
    fn directional_move_returns_to_the_previous_stacked_slot() {
        let (bounds, mut workspace) = three_pane_stack_workspace();

        assert!(swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            3,
            Direction::Left,
        ));
        assert!(swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            3,
            Direction::Right,
        ));

        assert_eq!(workspace.tile_tree, three_pane_stack_tree());
    }

    #[test]
    fn vertical_directional_move_requires_horizontal_overlap() {
        let (bounds, mut workspace) = three_pane_stack_workspace();

        assert!(!swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            1,
            Direction::Down,
        ));
        assert!(!swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            1,
            Direction::Up,
        ));
        assert!(swap_tiled_neighbor_in_direction(
            &mut workspace,
            bounds,
            0.0,
            TileGap::DEFAULT,
            2,
            Direction::Down,
        ));
    }

    #[test]
    fn follower_mouse_resize_leaves_the_layout_untouched() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
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
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn three_pane_stack_workspace() -> (FloatRect, Workspace) {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 80.0,
        };
        let mut workspace = Workspace::new(0);
        for id in 1..=3 {
            workspace.panes.push(Pane::new(id, 100, bounds));
        }
        workspace.tile_tree = three_pane_stack_tree();
        (bounds, workspace)
    }

    fn three_pane_stack_tree() -> Option<DwindleTree> {
        Some(DwindleTree::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(DwindleTree::Leaf(1)),
            second: Box::new(DwindleTree::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(2)),
                second: Box::new(DwindleTree::Leaf(3)),
            }),
        })
    }

    fn root_ratio(workspace: &Workspace) -> f32 {
        match workspace.tile_tree.as_ref().unwrap() {
            DwindleTree::Split { ratio, .. } => *ratio,
            DwindleTree::Leaf(_) => panic!("expected split"),
        }
    }
}
