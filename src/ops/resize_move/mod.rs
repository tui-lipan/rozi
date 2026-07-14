mod float;
mod keyboard;
mod split_drag;
mod tiling;

pub(crate) use float::{begin_move, begin_resize, end_move, move_pane, resize_pane};
pub(crate) use keyboard::resize_focused_in_direction;
pub(crate) use split_drag::{
    begin_resize_split_drag, begin_resize_split_junction_drag, resize_split_by_drag,
    resize_split_junction_by_drag,
};
pub(crate) use tiling::{
    adjust_focused_split_ratio, move_focused_in_direction, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};

#[cfg(test)]
pub(super) mod test_util {
    use tui_lipan::prelude::*;

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

    pub(super) fn root_ratio(workspace: &Workspace) -> f32 {
        match workspace.tile_tree.as_ref().unwrap() {
            DwindleTree::Split { ratio, .. } => *ratio,
            DwindleTree::Leaf(_) => panic!("expected split"),
        }
    }
}
