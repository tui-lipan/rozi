use tui_lipan::prelude::FloatRect;

use crate::state::{DEFAULT_RATIO, MAX_SPLIT_RATIO, MIN_SPLIT_RATIO, PaneId, SplitAxis, Workspace};

#[derive(Clone, Debug, PartialEq)]
pub enum DwindleTree {
    Leaf(PaneId),
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<DwindleTree>,
        second: Box<DwindleTree>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanePlacement {
    pub id: PaneId,
    pub rect: FloatRect,
}

pub fn append_tiled_window(workspace: &mut Workspace, id: PaneId) {
    if workspace
        .tile_tree
        .as_ref()
        .is_some_and(|tree| tree_contains(tree, id))
    {
        return;
    }
    workspace.tile_tree = Some(append_tiled_leaf(
        workspace.tile_tree.take(),
        id,
        workspace.start_axis,
    ));
}

pub fn remove_tiled_window(workspace: &mut Workspace, id: PaneId) {
    workspace.tile_tree = workspace
        .tile_tree
        .take()
        .and_then(|tree| remove_tree_leaf(tree, id).0);
}

pub fn build_dwindle_tree(
    ids: &[PaneId],
    start_axis: SplitAxis,
    ratios: &[f32],
) -> Option<DwindleTree> {
    build_dwindle_tree_at(ids, start_axis, ratios, 0)
}

fn build_dwindle_tree_at(
    ids: &[PaneId],
    start_axis: SplitAxis,
    ratios: &[f32],
    depth: usize,
) -> Option<DwindleTree> {
    match ids {
        [] => None,
        [id] => Some(DwindleTree::Leaf(*id)),
        [first, rest @ ..] => Some(DwindleTree::Split {
            axis: start_axis.at_depth(depth),
            ratio: ratio_at(ratios, depth),
            first: Box::new(DwindleTree::Leaf(*first)),
            second: Box::new(build_dwindle_tree_at(rest, start_axis, ratios, depth + 1)?),
        }),
    }
}

pub fn collect_tree_leaves(tree: &DwindleTree, out: &mut Vec<PaneId>) {
    match tree {
        DwindleTree::Leaf(id) => out.push(*id),
        DwindleTree::Split { first, second, .. } => {
            collect_tree_leaves(first, out);
            collect_tree_leaves(second, out);
        }
    }
}

pub fn tree_contains(tree: &DwindleTree, id: PaneId) -> bool {
    match tree {
        DwindleTree::Leaf(leaf) => *leaf == id,
        DwindleTree::Split { first, second, .. } => {
            tree_contains(first, id) || tree_contains(second, id)
        }
    }
}

pub fn append_tiled_leaf(
    tree: Option<DwindleTree>,
    id: PaneId,
    start_axis: SplitAxis,
) -> DwindleTree {
    match tree {
        Some(tree) => append_tiled_leaf_at(tree, id, start_axis, 0),
        None => DwindleTree::Leaf(id),
    }
}

fn append_tiled_leaf_at(
    tree: DwindleTree,
    id: PaneId,
    start_axis: SplitAxis,
    depth: usize,
) -> DwindleTree {
    match tree {
        DwindleTree::Leaf(existing) => DwindleTree::Split {
            axis: start_axis.at_depth(depth),
            ratio: DEFAULT_RATIO,
            first: Box::new(DwindleTree::Leaf(existing)),
            second: Box::new(DwindleTree::Leaf(id)),
        },
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => DwindleTree::Split {
            axis,
            ratio,
            first,
            second: Box::new(append_tiled_leaf_at(*second, id, start_axis, depth + 1)),
        },
    }
}

pub fn prune_tree_to_ids(tree: DwindleTree, active_ids: &[PaneId]) -> Option<DwindleTree> {
    match tree {
        DwindleTree::Leaf(id) => active_ids.contains(&id).then_some(DwindleTree::Leaf(id)),
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => match (
            prune_tree_to_ids(*first, active_ids),
            prune_tree_to_ids(*second, active_ids),
        ) {
            (Some(first), Some(second)) => Some(DwindleTree::Split {
                axis,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
    }
}

pub fn remove_tree_leaf(tree: DwindleTree, id: PaneId) -> (Option<DwindleTree>, bool) {
    match tree {
        DwindleTree::Leaf(leaf) if leaf == id => (None, true),
        DwindleTree::Leaf(leaf) => (Some(DwindleTree::Leaf(leaf)), false),
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first, removed_first) = remove_tree_leaf(*first, id);
            let (second, removed_second) = remove_tree_leaf(*second, id);
            let removed = removed_first || removed_second;
            let tree = match (first, second) {
                (Some(first), Some(second)) => Some(DwindleTree::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            };
            (tree, removed)
        }
    }
}

pub fn insert_leaf_around_target(
    tree: DwindleTree,
    target: PaneId,
    moving: PaneId,
    axis: SplitAxis,
    moving_first: bool,
) -> Option<DwindleTree> {
    match tree {
        DwindleTree::Leaf(id) if id == target => {
            let moving = DwindleTree::Leaf(moving);
            let target = DwindleTree::Leaf(target);
            let (first, second) = if moving_first {
                (moving, target)
            } else {
                (target, moving)
            };
            Some(DwindleTree::Split {
                axis,
                ratio: 0.5,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
        DwindleTree::Leaf(_) => None,
        DwindleTree::Split {
            axis: split_axis,
            ratio,
            first,
            second,
        } => {
            let first = *first;
            let second = *second;
            if tree_contains(&first, target) {
                insert_leaf_around_target(first, target, moving, axis, moving_first).map(
                    |inserted| DwindleTree::Split {
                        axis: split_axis,
                        ratio,
                        first: Box::new(inserted),
                        second: Box::new(second),
                    },
                )
            } else if tree_contains(&second, target) {
                insert_leaf_around_target(second, target, moving, axis, moving_first).map(
                    |inserted| DwindleTree::Split {
                        axis: split_axis,
                        ratio,
                        first: Box::new(first),
                        second: Box::new(inserted),
                    },
                )
            } else {
                None
            }
        }
    }
}

pub fn move_tiled_window_around_target(
    workspace: &mut Workspace,
    moving: PaneId,
    target: PaneId,
    axis: SplitAxis,
    moving_first: bool,
) -> bool {
    if moving == target {
        return false;
    }
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = crate::layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.take() else {
        return false;
    };
    let original = tree.clone();
    let (Some(without_moving), true) = remove_tree_leaf(tree, moving) else {
        workspace.tile_tree = Some(original);
        return false;
    };
    let Some(inserted) =
        insert_leaf_around_target(without_moving, target, moving, axis, moving_first)
    else {
        workspace.tile_tree = Some(original);
        return false;
    };
    workspace.tile_tree = Some(inserted);
    true
}

pub fn adjust_tree_split_for_focused(
    tree: &mut DwindleTree,
    focused: PaneId,
    delta: f32,
    depth: usize,
) -> Option<usize> {
    match tree {
        DwindleTree::Leaf(_) => None,
        DwindleTree::Split {
            ratio,
            first,
            second,
            ..
        } => {
            if tree_contains(first.as_ref(), focused) {
                if let Some(index) = adjust_tree_split_for_focused(first, focused, delta, depth + 1)
                {
                    return Some(index);
                }
                *ratio = adjust_ratio_value(*ratio, delta);
                Some(depth)
            } else if tree_contains(second.as_ref(), focused) {
                if let Some(index) =
                    adjust_tree_split_for_focused(second, focused, delta, depth + 1)
                {
                    return Some(index);
                }
                *ratio = adjust_ratio_value(*ratio, -delta);
                Some(depth)
            } else {
                None
            }
        }
    }
}

pub fn flip_tree_split_for_focused(
    tree: &mut DwindleTree,
    focused: PaneId,
    depth: usize,
) -> Option<(usize, SplitAxis)> {
    match tree {
        DwindleTree::Leaf(_) => None,
        DwindleTree::Split {
            axis,
            first,
            second,
            ..
        } => {
            if tree_contains(first.as_ref(), focused) {
                if let Some(result) = flip_tree_split_for_focused(first, focused, depth + 1) {
                    return Some(result);
                }
            } else if tree_contains(second.as_ref(), focused) {
                if let Some(result) = flip_tree_split_for_focused(second, focused, depth + 1) {
                    return Some(result);
                }
            } else {
                return None;
            }

            *axis = axis.flipped();
            Some((depth, *axis))
        }
    }
}

pub fn allocate_dwindle(
    tree: &DwindleTree,
    rect: FloatRect,
    gap: f32,
    placements: &mut Vec<PanePlacement>,
) {
    match tree {
        DwindleTree::Leaf(id) => placements.push(PanePlacement { id: *id, rect }),
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first_rect, second_rect) = split_float_rect(rect, *axis, *ratio, gap);
            allocate_dwindle(first, first_rect, gap, placements);
            allocate_dwindle(second, second_rect, gap, placements);
        }
    }
}

pub fn allocate_master(
    ids: &[PaneId],
    rect: FloatRect,
    gap: f32,
    ratio: f32,
    placements: &mut Vec<PanePlacement>,
) {
    match ids {
        [] => {}
        [id] => placements.push(PanePlacement { id: *id, rect }),
        [master, stack @ ..] => {
            let (master_rect, stack_rect) =
                split_float_rect(rect, SplitAxis::Horizontal, ratio, gap);
            placements.push(PanePlacement {
                id: *master,
                rect: master_rect,
            });
            allocate_master_stack(stack, stack_rect, gap, placements);
        }
    }
}

fn allocate_master_stack(
    ids: &[PaneId],
    rect: FloatRect,
    gap: f32,
    placements: &mut Vec<PanePlacement>,
) {
    if ids.is_empty() {
        return;
    }
    let usable_gap = if rect.h > gap { gap } else { 0.0 };
    let available = (rect.h - usable_gap * ids.len().saturating_sub(1) as f32).max(0.0);
    let base_h = (available / ids.len() as f32).floor();
    let mut y = rect.y;
    let mut remaining_h = available;
    for (index, id) in ids.iter().enumerate() {
        let last = index + 1 == ids.len();
        let h = if last {
            remaining_h
        } else {
            base_h.min(remaining_h)
        };
        placements.push(PanePlacement {
            id: *id,
            rect: FloatRect {
                x: rect.x,
                y,
                w: rect.w,
                h,
            },
        });
        y += h + usable_gap;
        remaining_h = (remaining_h - h).max(0.0);
    }
}

pub fn split_float_rect(
    rect: FloatRect,
    axis: SplitAxis,
    ratio: f32,
    gap: f32,
) -> (FloatRect, FloatRect) {
    let ratio = clamp_split_ratio(ratio);
    match axis {
        SplitAxis::Horizontal => {
            let gap = if rect.w > gap { gap } else { 0.0 };
            let available = (rect.w - gap).max(0.0);
            let first_w = (available * ratio).round();
            let second_w = available - first_w;
            (
                FloatRect {
                    x: rect.x,
                    y: rect.y,
                    w: first_w,
                    h: rect.h,
                },
                FloatRect {
                    x: rect.x + first_w + gap,
                    y: rect.y,
                    w: second_w,
                    h: rect.h,
                },
            )
        }
        SplitAxis::Vertical => {
            let gap = if rect.h > gap { gap } else { 0.0 };
            let available = (rect.h - gap).max(0.0);
            let first_h = (available * ratio).round();
            let second_h = available - first_h;
            (
                FloatRect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: first_h,
                },
                FloatRect {
                    x: rect.x,
                    y: rect.y + first_h + gap,
                    w: rect.w,
                    h: second_h,
                },
            )
        }
    }
}

pub fn ratio_at(ratios: &[f32], index: usize) -> f32 {
    ratios
        .get(index)
        .copied()
        .map(clamp_split_ratio)
        .unwrap_or(DEFAULT_RATIO)
}

pub fn clamp_split_ratio(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
    } else {
        DEFAULT_RATIO
    }
}

pub fn adjust_ratio_value(value: f32, delta: f32) -> f32 {
    clamp_split_ratio(value + delta)
}

pub fn nearest_split_available(
    tree: &DwindleTree,
    rect: FloatRect,
    gap: f32,
    focused: PaneId,
    target_axis: SplitAxis,
) -> Option<f32> {
    let DwindleTree::Split {
        axis,
        ratio,
        first,
        second,
    } = tree
    else {
        return None;
    };

    let (first_rect, second_rect) = split_float_rect(rect, *axis, *ratio, gap);
    let (child, child_rect) = if tree_contains(first, focused) {
        (first.as_ref(), first_rect)
    } else if tree_contains(second, focused) {
        (second.as_ref(), second_rect)
    } else {
        return None;
    };

    if let Some(deeper) = nearest_split_available(child, child_rect, gap, focused, target_axis) {
        return Some(deeper);
    }

    if *axis != target_axis {
        return None;
    }
    let extent = match target_axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    };
    let usable_gap = if extent > gap { gap } else { 0.0 };
    Some((extent - usable_gap).max(1.0))
}

pub fn focused_is_first_in_nearest_axis_split(
    tree: &DwindleTree,
    focused: PaneId,
    target_axis: SplitAxis,
) -> Option<bool> {
    let DwindleTree::Split {
        axis,
        first,
        second,
        ..
    } = tree
    else {
        return None;
    };

    if tree_contains(first, focused) {
        focused_is_first_in_nearest_axis_split(first, focused, target_axis)
            .or((*axis == target_axis).then_some(true))
    } else if tree_contains(second, focused) {
        focused_is_first_in_nearest_axis_split(second, focused, target_axis)
            .or((*axis == target_axis).then_some(false))
    } else {
        None
    }
}

pub fn resize_tiled_split(
    workspace: &mut Workspace,
    focused: PaneId,
    target_axis: SplitAxis,
    available: f32,
    pixels: f32,
) -> bool {
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = crate::layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return false;
    };
    let ratio_delta = pixels / available.max(1.0);
    adjust_nearest_axis_split(tree, focused, target_axis, ratio_delta)
}

fn adjust_nearest_axis_split(
    tree: &mut DwindleTree,
    focused: PaneId,
    target_axis: SplitAxis,
    delta: f32,
) -> bool {
    let DwindleTree::Split {
        axis,
        ratio,
        first,
        second,
    } = tree
    else {
        return false;
    };

    if tree_contains(first, focused) {
        if adjust_nearest_axis_split(first, focused, target_axis, delta) {
            return true;
        }
        if *axis == target_axis {
            *ratio = adjust_ratio_value(*ratio, delta);
            return true;
        }
        false
    } else if tree_contains(second, focused) {
        if adjust_nearest_axis_split(second, focused, target_axis, delta) {
            return true;
        }
        if *axis == target_axis {
            *ratio = adjust_ratio_value(*ratio, -delta);
            return true;
        }
        false
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.001;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < EPSILON,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn split_float_rect_keeps_whole_cell_boundary_flush() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 20.0,
        };
        let (first, second) = split_float_rect(rect, SplitAxis::Horizontal, 0.5, 1.0);
        assert_close(first.x + first.w + 1.0, second.x);
        assert_close(second.x + second.w, 101.0);
    }

    #[test]
    fn dwindle_allocates_all_leaves() {
        let ids = [1, 2, 3];
        let tree = build_dwindle_tree(&ids, SplitAxis::Horizontal, &[0.5, 0.5]).unwrap();
        let mut placements = Vec::new();
        allocate_dwindle(
            &tree,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 40.0,
            },
            1.0,
            &mut placements,
        );
        assert_eq!(placements.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn ratio_is_clamped() {
        assert_close(clamp_split_ratio(0.01), MIN_SPLIT_RATIO);
        assert_close(clamp_split_ratio(0.99), MAX_SPLIT_RATIO);
        assert_close(clamp_split_ratio(f32::NAN), DEFAULT_RATIO);
    }

    #[test]
    fn master_allocates_first_pane_as_left_master() {
        let ids = [1, 2, 3];
        let mut placements = Vec::new();
        allocate_master(
            &ids,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 101.0,
                h: 41.0,
            },
            1.0,
            0.5,
            &mut placements,
        );

        assert_eq!(placements.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
        assert_close(placements[0].rect.x, 0.0);
        assert_close(placements[0].rect.w, 50.0);
        assert_close(placements[1].rect.x, 51.0);
        assert_close(
            placements[1].rect.y + placements[1].rect.h + 1.0,
            placements[2].rect.y,
        );
        assert_close(placements[2].rect.y + placements[2].rect.h, 41.0);
    }

    #[test]
    fn nearest_axis_split_reports_focused_side() {
        let tree = build_dwindle_tree(&[1, 2, 3], SplitAxis::Horizontal, &[0.5, 0.5]).unwrap();

        assert_eq!(
            focused_is_first_in_nearest_axis_split(&tree, 1, SplitAxis::Horizontal),
            Some(true)
        );
        assert_eq!(
            focused_is_first_in_nearest_axis_split(&tree, 2, SplitAxis::Horizontal),
            Some(false)
        );
        assert_eq!(
            focused_is_first_in_nearest_axis_split(&tree, 2, SplitAxis::Vertical),
            Some(true)
        );
        assert_eq!(
            focused_is_first_in_nearest_axis_split(&tree, 3, SplitAxis::Vertical),
            Some(false)
        );
        assert_eq!(
            focused_is_first_in_nearest_axis_split(&tree, 99, SplitAxis::Horizontal),
            None
        );
    }
}
