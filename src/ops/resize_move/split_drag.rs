use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::active_pane_is_fullscreen;
use crate::state::{self, LayoutKind, PaneId, SplitDragKind, SplitDragSession, TileGap, Workspace};
use crate::tiling::{nearest_axis_split_path, nearest_split_available, resize_tiled_split};

use super::float::{ensure_tile_tree, layout_has_resizable_splits};
use super::tiling::{master_available_width, resize_master_split_by_pixels};

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
    x: u16,
    y: u16,
) -> Update {
    begin_split_drag(ctx, SplitDragKind::Junction, x, y)
}

fn begin_split_drag(ctx: &mut Context<HyprmuxApp>, kind: SplitDragKind, x: u16, y: u16) -> Update {
    if crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    let workspace_index = ctx.state.current().active_workspace;
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
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
    let workspace = ctx.state.current().active_workspace;
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
    let Some(session) = ctx.state.split_drag.as_ref().filter(|session| {
        session.kind == kind && session.workspace == ctx.state.current().active_workspace
    }) else {
        return false;
    };
    let ws_index = session.workspace;
    let start_tile_tree = session.start_tile_tree.clone();
    let start_split_ratios = session.start_split_ratios.clone();
    let workspace = &mut ctx.state.current_mut().workspaces[ws_index];
    workspace.tile_tree = start_tile_tree;
    workspace.split_ratios = start_split_ratios;
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

    let workspace_index = ctx.state.current().active_workspace;
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let tile_bounds = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap());
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
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
    horizontal_panes: &[PaneId],
    vertical_panes: &[PaneId],
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let kind = SplitDragKind::Junction;
    ensure_split_drag(ctx, kind, from_x, from_y);
    if !restore_split_drag(ctx, kind) {
        return Update::none();
    }
    let dx = (x as i32 - from_x as i32) as f32;
    let dy = (y as i32 - from_y as i32) as f32;
    let horizontal_panes = distinct_split_representatives(
        &ctx.state.current().workspaces[ctx.state.current().active_workspace],
        horizontal_panes,
        state::SplitAxis::Horizontal,
    );
    let vertical_panes = distinct_split_representatives(
        &ctx.state.current().workspaces[ctx.state.current().active_workspace],
        vertical_panes,
        state::SplitAxis::Vertical,
    );
    for pane_id in horizontal_panes {
        apply_resize_split_pixels(ctx, pane_id, true, dx);
    }
    for pane_id in vertical_panes {
        apply_resize_split_pixels(ctx, pane_id, false, dy);
    }
    Update::full()
}

fn distinct_split_representatives(
    workspace: &Workspace,
    panes: &[PaneId],
    axis: state::SplitAxis,
) -> Vec<PaneId> {
    let Some(tree) = workspace.tile_tree.as_ref() else {
        return Vec::new();
    };
    let mut paths = std::collections::HashSet::new();
    panes
        .iter()
        .copied()
        .filter(|pane_id| {
            nearest_axis_split_path(tree, *pane_id, axis).is_some_and(|path| paths.insert(path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::resize_move::test_util::{
        TEST_VIEWPORT, assert_ratio_close, balanced_grid_ratios, balanced_grid_tree, in_test_stack,
        root_ratio,
    };
    use crate::state::{Pane, SplitAxis, Workspace};
    use crate::tiling::{DwindleTree, resize_tiled_split};
    use crate::{HyprmuxApp, Msg};
    use tui_lipan::TestBackend;

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
    fn four_pane_junction_tracks_absolute_pointer_once_per_split() {
        in_test_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(TEST_VIEWPORT);
            let (root_available, left_available, right_available) = {
                let state = backend.state_mut();
                let workspace = state.active_workspace_mut();
                workspace.panes.clear();
                for id in 1..=4 {
                    workspace
                        .panes
                        .push(Pane::new(id, 100, FloatRect::default()));
                }
                workspace.tile_tree = Some(balanced_grid_tree());

                let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
                let tile_bounds = workspace_tile_bounds(bounds, state.workspace_top_gap());
                let tree = state.current().workspaces[state.current().active_workspace]
                    .tile_tree
                    .as_ref()
                    .unwrap();
                (
                    nearest_split_available(
                        tree,
                        tile_bounds,
                        TileGap::DEFAULT,
                        1,
                        SplitAxis::Horizontal,
                    )
                    .unwrap(),
                    nearest_split_available(
                        tree,
                        tile_bounds,
                        TileGap::DEFAULT,
                        1,
                        SplitAxis::Vertical,
                    )
                    .unwrap(),
                    nearest_split_available(
                        tree,
                        tile_bounds,
                        TileGap::DEFAULT,
                        3,
                        SplitAxis::Vertical,
                    )
                    .unwrap(),
                )
            };
            backend.render();
            backend
                .dispatch(Msg::BeginResizeSplitJunction(50, 15))
                .expect("begin junction drag");
            backend
                .dispatch(Msg::ResizeSplitJunction(
                    vec![1, 2],
                    vec![1, 3],
                    50,
                    15,
                    60,
                    18,
                ))
                .expect("resize junction");

            let ratios = balanced_grid_ratios(
                backend.state_mut().current_mut().workspaces[0]
                    .tile_tree
                    .as_ref()
                    .unwrap(),
            );
            assert_ratio_close(ratios.0, 0.5 + 10.0 / root_available);
            assert_ratio_close(ratios.1, 0.5 + 3.0 / left_available);
            assert_ratio_close(ratios.2, 0.5 + 3.0 / right_available);

            backend
                .dispatch(Msg::ResizeSplitJunction(
                    vec![1, 2],
                    vec![1, 3],
                    50,
                    15,
                    61,
                    19,
                ))
                .expect("continue junction drag");
            let ratios = balanced_grid_ratios(
                backend.state_mut().current_mut().workspaces[0]
                    .tile_tree
                    .as_ref()
                    .unwrap(),
            );
            assert_ratio_close(ratios.0, 0.5 + 11.0 / root_available);
            assert_ratio_close(ratios.1, 0.5 + 4.0 / left_available);
            assert_ratio_close(ratios.2, 0.5 + 4.0 / right_available);
        });
    }
}
