use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{clamp_float_rect, directional_score, lift_off_float_rect};
use crate::layout::{self, insert_tiled_pane_at_point, placement_for, workspace_target_rects};
use crate::ops::focus::{active_pane_is_fullscreen, active_pane_mut, request_pane_focus};
use crate::state::{
    Direction, LayoutKind, MoveSwapHint, PaneId, State, TILE_GAP, TileGap, Workspace,
};
use crate::tiling::{
    adjust_ratio_value, adjust_tree_split_for_focused, append_tiled_window,
    flip_tree_split_for_focused, move_tiled_window_around_target, ratio_at, remove_tiled_window,
    swap_tree_leaves,
};

use super::float::{
    ensure_tile_tree, finish_pointer_layout_interaction, layout_has_resizable_splits,
};

pub(crate) fn toggle_tiling(ctx: &mut Context<HyprmuxApp>) {
    finish_pointer_layout_interaction(ctx);
    let Some(id) = ctx.state.focused_pane else {
        return;
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
    finish_pointer_layout_interaction(ctx);
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
        workspace.last_directional_focus = None;
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
    finish_pointer_layout_interaction(ctx);
    let workspace_index = ctx.state.active_workspace;
    let layout_label = {
        let workspace = &mut ctx.state.workspaces[workspace_index];
        workspace.layout_kind = workspace.layout_kind.toggled();
        workspace.last_move_swap = None;
        workspace.last_directional_focus = None;
        workspace.layout_kind.label()
    };
    ctx.state.animation = GeometryAnimation::AxisChange;
    if show_toast {
        crate::pty_events::replace_toast(
            ctx,
            crate::state::ToastChannel::LayoutMode,
            crate::pty_events::info_toast(&ctx.state.theme, format!("Layout mode: {layout_label}")),
        );
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

pub(super) fn resize_master_split_by_pixels(
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

pub(super) fn master_available_width(tile_bounds: FloatRect) -> f32 {
    let gap = if tile_bounds.w > TILE_GAP {
        TILE_GAP
    } else {
        0.0
    };
    (tile_bounds.w - gap).max(1.0)
}

/// Lift the focused pane out of its slot and re-insert it beside its directional neighbor,
/// reshaping the tree — the keyboard equivalent of dropping a pane onto another with the mouse.
/// A floating pane has no slot to leave, so it slides instead.
pub(crate) fn move_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    if super::float::move_focused_float(ctx, direction) {
        return;
    }
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
    let tiled_ids = workspace.active_tiled_ids_by_pane_order();
    if !tiled_ids.contains(&focused) {
        return;
    }
    let placements: Vec<_> = workspace_target_rects(workspace, bounds, top_gap, tile_gap)
        .into_iter()
        .filter(|placement| tiled_ids.contains(&placement.id))
        .collect();
    // Unlike a swap, this never consults `last_move_swap`: a move reshapes the tree, so there is no
    // slot left to return to. `move_tiled_window_around_target` clears the hint for the same reason.
    let Some(target) = strict_directional_neighbor(&placements, focused, direction) else {
        return;
    };

    // The pane travels past its neighbor and docks on the far side, so moving left/up lands it
    // first (leading) in the new split and right/down lands it second — the same convention
    // `layout::drop_split_for_target` uses for a mouse drop.
    let axis = crate::ops::focus::split_axis_for_direction(direction);
    let moving_first = matches!(direction, Direction::Left | Direction::Up);
    if move_tiled_window_around_target(workspace, focused, target, axis, moving_first) {
        workspace.focused_pane = Some(focused);
        ctx.state.focused_pane = Some(focused);
        ctx.state.animation = GeometryAnimation::AxisChange;
    }
}

/// Exchange the focused pane with its directional neighbor. The two trade slots in place, so the
/// layout keeps its shape. A floating pane has nothing to trade with, so it slides instead.
pub(crate) fn swap_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    if super::float::move_focused_float(ctx, direction) {
        return;
    }
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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
        workspace.last_directional_focus = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Action;
    use crate::layout::workspace_target_rects_excluding;
    use crate::ops::resize_move::test_util::{
        TEST_VIEWPORT, in_test_stack, three_pane_stack_tree, three_pane_stack_workspace,
    };
    use crate::state::{Pane, SplitAxis};
    use crate::tiling::DwindleTree;
    use crate::{HyprmuxApp, Msg};
    use tui_lipan::TestBackend;

    /// The two directional pane actions are different operations on the same neighbor: `Swap`
    /// trades slots and leaves the tree's shape alone, `Move` lifts the pane out and re-inserts it
    /// beside that neighbor. Same start, same direction, different trees.
    #[test]
    fn swap_keeps_the_tree_shape_where_move_reshapes_it() {
        in_test_stack(|| {
            let tree_after = |action: fn(Direction) -> Action| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(TEST_VIEWPORT);
                {
                    let state = backend.state_mut();
                    let (_, workspace) = three_pane_stack_workspace();
                    state.workspaces[state.active_workspace] = workspace;
                    state.focused_pane = Some(1);
                    state.workspaces[state.active_workspace].focused_pane = Some(1);
                }
                backend.render();
                backend
                    .dispatch(Msg::RunAction(action(Direction::Right)))
                    .expect("dispatch directional pane action");
                let state = backend.state_mut();
                state.workspaces[state.active_workspace].tile_tree.clone()
            };

            // Swap: 1 and 2 exchange leaves; every split and ratio is where it was.
            assert_eq!(
                tree_after(Action::Swap),
                Some(DwindleTree::Split {
                    axis: SplitAxis::Horizontal,
                    ratio: 0.5,
                    first: Box::new(DwindleTree::Leaf(2)),
                    second: Box::new(DwindleTree::Split {
                        axis: SplitAxis::Vertical,
                        ratio: 0.5,
                        first: Box::new(DwindleTree::Leaf(1)),
                        second: Box::new(DwindleTree::Leaf(3)),
                    }),
                }),
                "swap must not restructure the tree"
            );

            // Move: 1 vacates the left column (which collapses) and docks to the right of 2.
            assert_eq!(
                tree_after(Action::Move),
                Some(DwindleTree::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(DwindleTree::Split {
                        axis: SplitAxis::Horizontal,
                        ratio: 0.5,
                        first: Box::new(DwindleTree::Leaf(2)),
                        second: Box::new(DwindleTree::Leaf(1)),
                    }),
                    second: Box::new(DwindleTree::Leaf(3)),
                }),
                "move must re-insert the pane beside its neighbor"
            );
        });
    }

    #[test]
    fn directional_swap_trades_slots_instead_of_splitting_target() {
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
    fn directional_swap_returns_to_the_previous_stacked_slot() {
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
    fn vertical_directional_swap_requires_horizontal_overlap() {
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
    fn fullscreen_toggle_finishes_tiled_drag_at_current_pointer() {
        in_test_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(TEST_VIEWPORT);
            let (start_rect, start_pointer, target_pointer, original_tree) = {
                let state = backend.state_mut();
                let (_, workspace) = three_pane_stack_workspace();
                state.workspaces[state.active_workspace] = workspace;
                state.focused_pane = Some(1);
                state.workspaces[state.active_workspace].focused_pane = Some(1);

                let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
                let top_gap = state.workspace_top_gap();
                let tile_gap = state.tile_gap();
                let top_offset = state.content_top_offset();
                let workspace = &state.workspaces[state.active_workspace];
                let placements = workspace_target_rects(workspace, bounds, top_gap, tile_gap);
                let start = placement_for(&placements, 1).expect("pane 1 placement");
                let drop_placements =
                    workspace_target_rects_excluding(workspace, bounds, Some(1), top_gap, tile_gap);
                let target = placement_for(&drop_placements, 3).expect("pane 3 drop placement");
                let start_pointer = crate::geometry::rect_center(start);
                let target_pointer = crate::geometry::rect_center(target);
                (
                    FloatRect {
                        y: start.y + f32::from(top_offset),
                        ..start
                    },
                    (start_pointer.0, start_pointer.1 + f32::from(top_offset)),
                    (target_pointer.0, target_pointer.1 + f32::from(top_offset)),
                    workspace.tile_tree.clone(),
                )
            };
            backend.render();

            let local_x = (start_pointer.0 - start_rect.x).round() as u16;
            let local_y = (start_pointer.1 - start_rect.y).round() as u16;
            backend
                .dispatch(Msg::BeginMove(
                    1,
                    start_rect,
                    local_x,
                    local_y,
                    start_rect.w.round() as u16,
                    start_rect.h.round() as u16,
                    true,
                ))
                .expect("begin drag");
            backend
                .dispatch(Msg::MovePane(
                    1,
                    (target_pointer.0 - start_pointer.0).round() as i16,
                    (target_pointer.1 - start_pointer.1).round() as i16,
                    true,
                ))
                .expect("move drag");
            // Focus-on-hover may briefly select the pane under the pointer. The mode action still
            // belongs to the pane whose drag is active.
            backend.state_mut().focused_pane = Some(3);
            backend.state_mut().workspaces[0].focused_pane = Some(3);
            backend
                .dispatch(Msg::RunAction(Action::ToggleFullscreen))
                .expect("toggle fullscreen");

            let state = backend.state_mut();
            assert!(state.moving_pane.is_none());
            assert!(
                state.workspaces[state.active_workspace]
                    .panes
                    .iter()
                    .find(|pane| pane.id == 1)
                    .is_some_and(|pane| pane.fullscreen)
            );
            assert_ne!(
                state.workspaces[state.active_workspace].tile_tree, original_tree,
                "the tiled pane must be dropped before fullscreen is applied"
            );
            let dropped_tree = state.workspaces[state.active_workspace].tile_tree.clone();

            backend
                .dispatch(Msg::MovePane(1, 5, 5, true))
                .expect("stale drag event");
            assert_eq!(
                backend.state_mut().workspaces[0].tile_tree,
                dropped_tree,
                "stale mouse events must not resume a completed drag"
            );
        });
    }

    #[test]
    fn tiling_toggle_persists_floating_drag_before_insertion() {
        in_test_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(TEST_VIEWPORT);
            let start_rect = FloatRect {
                x: 8.0,
                y: 5.0,
                w: 30.0,
                h: 10.0,
            };
            {
                let state = backend.state_mut();
                let workspace = &mut state.workspaces[state.active_workspace];
                workspace.panes.clear();
                let mut floating = Pane::new(1, 100, start_rect);
                floating.floating = true;
                workspace.panes.push(floating);
                workspace
                    .panes
                    .push(Pane::new(2, 100, FloatRect::default()));
                workspace.tile_tree = Some(DwindleTree::Leaf(2));
                workspace.focused_pane = Some(1);
                state.focused_pane = Some(1);
            }
            backend.render();

            backend
                .dispatch(Msg::BeginMove(1, start_rect, 4, 3, 30, 10, true))
                .expect("begin floating drag");
            backend
                .dispatch(Msg::MovePane(1, 12, 4, true))
                .expect("move floating pane");
            let dragged_rect = backend
                .state_mut()
                .moving_pane
                .expect("active drag")
                .drag_rect;
            backend
                .dispatch(Msg::RunAction(Action::ToggleFloat))
                .expect("toggle tiling");

            let state = backend.state_mut();
            let pane = state.workspaces[state.active_workspace]
                .panes
                .iter()
                .find(|pane| pane.id == 1)
                .expect("pane 1");
            assert!(state.moving_pane.is_none());
            assert!(!pane.floating);
            assert_eq!(pane.floating_rect, dragged_rect);
            assert!(
                state.workspaces[state.active_workspace]
                    .tiled_ids()
                    .contains(&1)
            );
        });
    }
}
