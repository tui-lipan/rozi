use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::active_pane_is_fullscreen;
use crate::state::{self, Direction, LayoutKind, RATIO_STEP, TileGap};
use crate::tiling::{
    focused_is_first_in_nearest_axis_split, nearest_split_available, resize_tiled_split,
};

use super::float::{ensure_tile_tree, layout_has_resizable_splits};
use super::tiling::{master_available_width, resize_master_split_by_pixels};

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
}
