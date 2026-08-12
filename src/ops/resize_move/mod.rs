mod float;
mod keyboard;
mod split_drag;
mod tiling;

/// One keyboard move/resize step across an extent of `available` cells.
///
/// Proportional, so a step feels the same fraction of a big split as of a small one, but committed
/// in whole cells: a pane's PTY is sized in cells, so a fractional step would land on the same cell
/// for some presses and skip one on others, which is what makes a run of presses look uneven.
fn keyboard_step_cells(available: f32) -> f32 {
    (crate::state::RATIO_STEP * available).round().max(1.0)
}

pub(crate) use float::{begin_move, begin_resize, end_move, move_pane, resize_pane};
pub(crate) use keyboard::resize_focused_in_direction;
pub(crate) use split_drag::{
    begin_resize_split_drag, begin_resize_split_junction_drag, resize_split_by_drag,
    resize_split_junction_by_drag,
};
pub(crate) use tiling::{
    adjust_focused_split_ratio, move_focused_in_direction, set_layout, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};

#[cfg(test)]
pub(super) mod test_util {
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::*;

    use crate::AppRoot;
    use crate::state::{Pane, SplitAxis, Workspace};
    use crate::tiling::DwindleTree;

    pub(super) const TEST_VIEWPORT: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    };

    pub(super) fn in_test_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    /// A rendered backend holding two panes split along `axis`, pane 1 focused.
    ///
    /// The two axes are deliberately different shapes at [`TEST_VIEWPORT`]: the columns a
    /// left|right split divides come to an odd number, the rows a stacked split divides to an
    /// even one. Only the odd extent can put a divider on a half-cell, so a resize that is exact
    /// on one axis and not the other shows up here.
    pub(super) fn two_pane_backend(axis: SplitAxis) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(TEST_VIEWPORT);
        {
            let state = backend.state_mut();
            let workspace = state.active_workspace_mut();
            workspace.panes.clear();
            for id in 1..=2 {
                workspace
                    .panes
                    .push(Pane::new(id, 100, FloatRect::default()));
            }
            workspace.tile_tree = Some(DwindleTree::Split {
                axis,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(1)),
                second: Box::new(DwindleTree::Leaf(2)),
            });
            workspace.focused_pane = Some(1);
            state.current_mut().focused_pane = Some(1);
        }
        backend.render();
        backend
    }

    /// Pane 1's extent along `axis`, in cells - the visible half of a resize.
    pub(super) fn first_pane_extent(backend: &mut TestBackend<AppRoot>, axis: SplitAxis) -> f32 {
        let state = backend.state_mut();
        let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
        let top_gap = state.workspace_top_gap();
        let gap = state.tile_gap();
        let index = state.current().active_workspace;
        let rect = crate::layout::workspace_target_rects(
            &state.current().workspaces[index],
            bounds,
            top_gap,
            gap,
        )
        .iter()
        .find(|placement| placement.id == 1)
        .expect("pane 1 placement")
        .rect;
        match axis {
            SplitAxis::Horizontal => rect.w,
            SplitAxis::Vertical => rect.h,
        }
    }

    /// Cell deltas between consecutive samples.
    pub(super) fn steps(extents: &[f32]) -> Vec<f32> {
        extents.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    pub(super) fn three_pane_stack_workspace() -> (FloatRect, Workspace) {
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

    pub(super) fn three_pane_stack_tree() -> Option<DwindleTree> {
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

    pub(super) fn balanced_grid_tree() -> DwindleTree {
        DwindleTree::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(DwindleTree::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(1)),
                second: Box::new(DwindleTree::Leaf(2)),
            }),
            second: Box::new(DwindleTree::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(3)),
                second: Box::new(DwindleTree::Leaf(4)),
            }),
        }
    }

    pub(super) fn balanced_grid_ratios(tree: &DwindleTree) -> (f32, f32, f32) {
        let DwindleTree::Split {
            ratio,
            first,
            second,
            ..
        } = tree
        else {
            panic!("expected root split");
        };
        let DwindleTree::Split {
            ratio: first_ratio, ..
        } = first.as_ref()
        else {
            panic!("expected first child split");
        };
        let DwindleTree::Split {
            ratio: second_ratio,
            ..
        } = second.as_ref()
        else {
            panic!("expected second child split");
        };
        (*ratio, *first_ratio, *second_ratio)
    }

    pub(super) fn assert_ratio_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
    }

    /// The column/row a split's divider renders on, which is what a resize commits and what
    /// `split_float_rect` reads back out of the stored ratio.
    pub(super) fn divider_cell(ratio: f32, available: f32) -> f32 {
        (available * ratio).round()
    }

    pub(super) fn root_ratio(workspace: &Workspace) -> f32 {
        match workspace.tile_tree.as_ref().unwrap() {
            DwindleTree::Split { ratio, .. } => *ratio,
            DwindleTree::Leaf(_) => panic!("expected split"),
        }
    }
}
