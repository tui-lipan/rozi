use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::active_pane_is_fullscreen;
use crate::state::{self, Direction, LayoutKind};
use crate::tiling::{
    focused_is_first_in_nearest_axis_split, nearest_split_available, resize_tiled_split,
};

use super::float::{ensure_tile_tree, layout_has_resizable_splits, resize_focused_float};
use super::tiling::{master_available_width, resize_master_split_by_pixels};

pub(crate) fn resize_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    let Some(focused) = ctx.state.current().focused_pane else {
        return;
    };
    if active_pane_is_fullscreen(&ctx.state, focused) {
        return;
    }
    // A floating pane has no split to push against; it resizes its own rect instead.
    if resize_focused_float(ctx, direction) {
        return;
    }
    let workspace_index = ctx.state.current().active_workspace;
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let tile_bounds = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap());
    let tile_gap = ctx.state.tile_gap();
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
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
        let available = master_available_width(tile_bounds, tile_gap);
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
    let Some(available) = nearest_split_available(tree, tile_bounds, tile_gap, focused, axis)
    else {
        return;
    };
    let Some(focused_is_first) = focused_is_first_in_nearest_axis_split(tree, focused, axis) else {
        return;
    };
    let pixels = keyboard_resize_pixels(direction, focused_is_first, available);
    if resize_tiled_split(workspace, tile_bounds, tile_gap, focused, axis, pixels) {
        ctx.state.animation = GeometryAnimation::None;
    }
}

fn keyboard_resize_pixels(direction: Direction, focused_is_first: bool, available: f32) -> f32 {
    let grows_focused = match direction {
        Direction::Left | Direction::Up => !focused_is_first,
        Direction::Right | Direction::Down => focused_is_first,
    };
    let step = super::keyboard_step_cells(available);
    if grows_focused { step } else { -step }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Action;
    use crate::ops::resize_move::test_util::{
        first_pane_extent, in_test_stack, steps, two_pane_backend,
    };
    use crate::state::SplitAxis;
    use crate::{HyprmuxApp, Msg};
    use tui_lipan::TestBackend;
    use tui_lipan::core::event::{KeyCode, KeyEvent, KeyMods};

    /// Every press of a resize-mode key must move the divider the same number of cells.
    ///
    /// The step is `RATIO_STEP` of the split, which is 1.12 rows of a 28-row split and 3.96
    /// columns of a 99-column one. Left as a ratio those land on a different cell from press to
    /// press - a run of presses came out 1,1,1,1,2,1,1 and 3,4,4,4,4,4 - so the step is committed
    /// in whole cells instead.
    #[test]
    fn every_resize_mode_press_moves_the_divider_the_same_number_of_cells() {
        in_test_stack(|| {
            for (axis, key) in [
                (SplitAxis::Horizontal, KeyCode::Char('l')),
                (SplitAxis::Vertical, KeyCode::Char('j')),
            ] {
                let mut backend = two_pane_backend(axis);
                let mut extents = vec![first_pane_extent(&mut backend, axis)];
                for _ in 0..5 {
                    backend
                        .dispatch(Msg::RunAction(Action::EnterResizeMode))
                        .expect("enter resize mode");
                    backend
                        .send_key(KeyEvent {
                            code: key,
                            mods: KeyMods::NONE,
                        })
                        .expect("resize key");
                    extents.push(first_pane_extent(&mut backend, axis));
                }

                let steps = steps(&extents);
                let first = steps[0];
                assert!(
                    first >= 1.0,
                    "{axis:?}: a press must move at least one cell"
                );
                assert!(
                    steps.iter().all(|step| *step == first),
                    "{axis:?}: uneven steps {steps:?} from extents {extents:?}"
                );
            }
        });
    }

    /// Growing and then shrinking by the same number of presses returns the divider to its
    /// column. A step that rounds differently on the way back would leave it adrift.
    #[test]
    fn resize_mode_presses_undo_each_other_exactly() {
        in_test_stack(|| {
            for (axis, grow, shrink) in [
                (
                    SplitAxis::Horizontal,
                    KeyCode::Char('l'),
                    KeyCode::Char('h'),
                ),
                (SplitAxis::Vertical, KeyCode::Char('j'), KeyCode::Char('k')),
            ] {
                let mut backend = two_pane_backend(axis);
                let before = first_pane_extent(&mut backend, axis);
                let press = |backend: &mut TestBackend<HyprmuxApp>, code| {
                    backend
                        .dispatch(Msg::RunAction(Action::EnterResizeMode))
                        .expect("enter resize mode");
                    backend
                        .send_key(KeyEvent {
                            code,
                            mods: KeyMods::NONE,
                        })
                        .expect("resize key");
                };
                for _ in 0..3 {
                    press(&mut backend, grow);
                }
                for _ in 0..3 {
                    press(&mut backend, shrink);
                }
                assert_eq!(first_pane_extent(&mut backend, axis), before, "{axis:?}");
            }
        });
    }

    #[test]
    fn keyboard_resize_directions_grow_toward_the_nearest_split() {
        let available = 100.0;
        let step = super::super::keyboard_step_cells(available);

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
