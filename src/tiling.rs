use tui_lipan::prelude::FloatRect;

use crate::state::{
    DEFAULT_RATIO, DEFAULT_SCROLLABLE_WIDTH, MAX_SPLIT_RATIO, MIN_SPLIT_RATIO, PaneId,
    ScrollableRevealEdge, SplitAxis, TileGap, Workspace,
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitEdge {
    Leading,
    Trailing,
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
    workspace.last_move_swap = None;
    workspace.last_directional_focus = None;
}

pub fn remove_tiled_window(workspace: &mut Workspace, id: PaneId) {
    let Some(tree) = workspace.tile_tree.take() else {
        return;
    };
    let (tree, removed) = remove_tree_leaf(tree, id);
    workspace.tile_tree = tree;
    if removed {
        workspace.last_move_swap = None;
        workspace.last_directional_focus = None;
    }
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

/// Exchange the screen positions of two tiled leaves by swapping their pane ids in place.
/// The split structure (axes and ratios) is untouched - only the leaf payloads move - so
/// the two panes trade slots. Returns `true` only when both ids were present.
pub fn swap_tree_leaves(tree: &mut DwindleTree, a: PaneId, b: PaneId) -> bool {
    // Only mutate when both leaves are present, so a missing id leaves the tree untouched.
    if a == b || !tree_contains(tree, a) || !tree_contains(tree, b) {
        return false;
    }
    swap_tree_leaves_inner(tree, a, b);
    true
}

fn swap_tree_leaves_inner(tree: &mut DwindleTree, a: PaneId, b: PaneId) {
    match tree {
        DwindleTree::Leaf(id) => {
            if *id == a {
                *id = b;
            } else if *id == b {
                *id = a;
            }
        }
        DwindleTree::Split { first, second, .. } => {
            swap_tree_leaves_inner(first, a, b);
            swap_tree_leaves_inner(second, a, b);
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
    workspace.last_move_swap = None;
    workspace.last_directional_focus = None;
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
    gap: TileGap,
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
    gap: TileGap,
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
    gap: TileGap,
    placements: &mut Vec<PanePlacement>,
) {
    if ids.is_empty() {
        return;
    }
    let gap = gap.for_axis(SplitAxis::Vertical);
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

/// Monocle: every tiled pane fills the whole tile bounds. `ordered_panes` paints the
/// focused pane last, so it lands on top of the stacked siblings.
pub fn allocate_monocle(ids: &[PaneId], rect: FloatRect, placements: &mut Vec<PanePlacement>) {
    for id in ids {
        placements.push(PanePlacement { id: *id, rect });
    }
}

/// Grid: a near-square `ceil(sqrt(N))`-column arrangement, row-major over `ids`. The last
/// row holds the remainder and stretches its (possibly fewer) cells to fill the width.
pub fn allocate_grid(
    ids: &[PaneId],
    rect: FloatRect,
    gap: TileGap,
    placements: &mut Vec<PanePlacement>,
) {
    let n = ids.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        placements.push(PanePlacement { id: ids[0], rect });
        return;
    }

    let cols = (n as f32).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let row_rects = split_evenly(rect, SplitAxis::Vertical, rows, gap);

    let mut placed = 0;
    for (row_index, row_rect) in row_rects.iter().enumerate() {
        let remaining = n - placed;
        let cells_in_row = if row_index + 1 == rows {
            remaining
        } else {
            cols.min(remaining)
        };
        if cells_in_row == 0 {
            continue;
        }
        let cell_rects = split_evenly(*row_rect, SplitAxis::Horizontal, cells_in_row, gap);
        for cell_rect in cell_rects {
            if placed >= n {
                break;
            }
            placements.push(PanePlacement {
                id: ids[placed],
                rect: cell_rect,
            });
            placed += 1;
        }
    }
}

/// Columns: every tiled pane is a full-height column of equal width filling `rect`.
/// Whole-cell remainder is spread so widths differ by at most one cell (unlike
/// [`split_evenly`], which dumps the remainder into the last segment).
pub fn allocate_columns(
    ids: &[PaneId],
    rect: FloatRect,
    gap: TileGap,
    placements: &mut Vec<PanePlacement>,
) {
    if ids.is_empty() {
        return;
    }
    let rects = split_balanced(rect, SplitAxis::Horizontal, ids.len(), gap);
    for (id, column) in ids.iter().zip(rects) {
        placements.push(PanePlacement {
            id: *id,
            rect: column,
        });
    }
}

/// Split `rect` into `count` gapped segments along `axis` with whole-cell sizes that differ by at
/// most one. The first `remainder` segments receive the extra cell so the union still spans
/// `rect` exactly.
fn split_balanced(rect: FloatRect, axis: SplitAxis, count: usize, gap: TileGap) -> Vec<FloatRect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![rect];
    }

    let gap = gap.for_axis(axis);
    let extent = match axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    };
    let usable_gap = if extent > gap { gap } else { 0.0 };
    let available = (extent - usable_gap * (count - 1) as f32).max(0.0);
    let base = (available / count as f32).floor();
    let mut extras = (available - base * count as f32).round().max(0.0) as usize;

    let mut rects = Vec::with_capacity(count);
    let mut start = match axis {
        SplitAxis::Horizontal => rect.x,
        SplitAxis::Vertical => rect.y,
    };
    for _ in 0..count {
        let size = base
            + if extras > 0 {
                extras -= 1;
                1.0
            } else {
                0.0
            };
        rects.push(match axis {
            SplitAxis::Horizontal => FloatRect {
                x: start,
                y: rect.y,
                w: size,
                h: rect.h,
            },
            SplitAxis::Vertical => FloatRect {
                x: rect.x,
                y: start,
                w: rect.w,
                h: size,
            },
        });
        start += size + usable_gap;
    }
    rects
}

/// Sanitize a Scrollable width fraction: non-finite → default, else clamp to split bounds.
pub fn sanitize_scrollable_width(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
    } else {
        DEFAULT_SCROLLABLE_WIDTH
    }
}

/// Whole-cell column width for a Scrollable pane from its viewport fraction.
pub fn scrollable_column_width(viewport_w: f32, width_frac: f32) -> f32 {
    let viewport_w = viewport_w.max(1.0);
    let ratio = sanitize_scrollable_width(width_frac);
    let lowest = (MIN_SPLIT_RATIO * viewport_w).ceil().max(1.0);
    let highest = (MAX_SPLIT_RATIO * viewport_w).floor().max(lowest);
    (viewport_w * ratio).round().clamp(lowest, highest)
}

/// Scrollable: ordered full-height columns with per-pane widths on a horizontal strip.
/// `panes` is `(id, width_fraction)` in tiled order. `anchor` is the pane the viewport follows;
/// placements may extend outside `rect` (Canvas clips overflow).
///
/// Widths are fractions of `rect.w`. Scrolling is clamped to keep the strip aligned within `rect`
/// itself (local/controller). Prefer [`allocate_scrollable_with_visible`] when the actually visible
/// interval differs from the layout rect (follower letterbox).
pub fn allocate_scrollable(
    panes: &[(PaneId, f32)],
    rect: FloatRect,
    gap: TileGap,
    anchor: Option<PaneId>,
    reveal_edge: ScrollableRevealEdge,
    placements: &mut Vec<PanePlacement>,
) {
    allocate_scrollable_with_visible(panes, rect, rect, gap, anchor, reveal_edge, placements);
}

/// Like [`allocate_scrollable`], but clamps scrolling against `visible` while deriving column
/// widths from canonical `layout.w`.
///
/// `visible` is the horizontal interval that is actually on screen (local tile bounds for a
/// follower; normally equal to `layout`). `reveal_edge` left-aligns the anchor pane to
/// `visible.x` or right-aligns it to `visible.right`, then clamps to the valid scroll range.
/// When the strip fits inside `visible`, scroll is stable (no anchor-dependent drift).
///
/// Stored fractions are flex bases: one pane fills `layout.w`; two panes whose bases plus gap fit
/// share free cells evenly (order-balanced remainder) so they exactly span `layout`; three or more
/// (or an overflowing pair) keep independent preferred widths.
pub fn allocate_scrollable_with_visible(
    panes: &[(PaneId, f32)],
    layout: FloatRect,
    visible: FloatRect,
    gap: TileGap,
    anchor: Option<PaneId>,
    reveal_edge: ScrollableRevealEdge,
    placements: &mut Vec<PanePlacement>,
) {
    let n = panes.len();
    if n == 0 {
        return;
    }
    let gap = gap.for_axis(SplitAxis::Horizontal);
    let usable_gap = if layout.w > gap { gap } else { 0.0 };
    let widths = scrollable_allocated_widths(panes, layout.w, usable_gap);
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0.0);
    for (index, width) in widths.iter().enumerate() {
        prefix.push(prefix[index] + width + if index + 1 < n { usable_gap } else { 0.0 });
    }
    let strip_w = prefix[n];
    let vis_left = visible.x;
    let vis_right = visible.x + visible.w.max(1.0);
    let vis_w = (vis_right - vis_left).max(1.0);

    let scroll = if strip_w <= vis_w + 0.5 {
        // Strip fits: one stable origin (left-align into visible). Equals 0 when layout==visible.
        layout.x - vis_left
    } else {
        // May be negative when layout overhangs left of the local viewport (follower letterbox).
        let scroll_min = layout.x - vis_left;
        let scroll_max = layout.x + strip_w - vis_right;
        let anchor_index = anchor
            .and_then(|id| panes.iter().position(|(pane, _)| *pane == id))
            .unwrap_or(0);
        let desired = match reveal_edge {
            ScrollableRevealEdge::Left => layout.x + prefix[anchor_index] - vis_left,
            ScrollableRevealEdge::Right => {
                let focused_right = prefix[anchor_index] + widths[anchor_index];
                layout.x + focused_right - vis_right
            }
        };
        desired.clamp(scroll_min.min(scroll_max), scroll_max.max(scroll_min))
    };

    for (index, (id, _)) in panes.iter().enumerate() {
        placements.push(PanePlacement {
            id: *id,
            rect: FloatRect {
                x: layout.x + prefix[index] - scroll,
                y: layout.y,
                w: widths[index],
                h: layout.h,
            },
        });
    }
}

/// Whole-cell column widths from preferred fractions of canonical `viewport_w`.
///
/// One pane fills the viewport. Two panes whose bases plus `usable_gap` fit share free cells
/// evenly (first gets `floor(free/2)`, second the remainder) so widths + gap equal `viewport_w`.
/// Overflowing pairs and 3+ panes keep independent preferred widths.
fn scrollable_allocated_widths(
    panes: &[(PaneId, f32)],
    viewport_w: f32,
    usable_gap: f32,
) -> Vec<f32> {
    let viewport_w = viewport_w.max(1.0);
    let bases: Vec<f32> = panes
        .iter()
        .map(|(_, frac)| scrollable_column_width(viewport_w, *frac))
        .collect();
    match bases.as_slice() {
        [_] => vec![viewport_w],
        [a, b] if a + b + usable_gap <= viewport_w + 0.5 => {
            let free = (viewport_w - usable_gap - a - b).max(0.0);
            let extra0 = (free / 2.0).floor();
            let extra1 = free - extra0;
            vec![a + extra0, b + extra1]
        }
        _ => bases,
    }
}

/// Split `rect` into `count` flush, gapped segments along `axis`, keeping boundaries on
/// whole cells the way [`split_float_rect`] does. The last segment absorbs the rounding
/// remainder so the segments exactly tile `rect`.
pub(crate) fn split_evenly(
    rect: FloatRect,
    axis: SplitAxis,
    count: usize,
    gap: TileGap,
) -> Vec<FloatRect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![rect];
    }

    let gap = gap.for_axis(axis);
    let extent = match axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    };
    let usable_gap = if extent > gap { gap } else { 0.0 };
    let total_gap = usable_gap * (count - 1) as f32;
    let available = (extent - total_gap).max(0.0);
    let base = (available / count as f32).floor();

    let mut rects = Vec::with_capacity(count);
    let mut start = match axis {
        SplitAxis::Horizontal => rect.x,
        SplitAxis::Vertical => rect.y,
    };
    let mut remaining = available;
    for index in 0..count {
        let last = index + 1 == count;
        let size = if last { remaining } else { base.min(remaining) };
        rects.push(match axis {
            SplitAxis::Horizontal => FloatRect {
                x: start,
                y: rect.y,
                w: size,
                h: rect.h,
            },
            SplitAxis::Vertical => FloatRect {
                x: rect.x,
                y: start,
                w: rect.w,
                h: size,
            },
        });
        start += size + usable_gap;
        remaining = (remaining - size).max(0.0);
    }
    rects
}

pub fn split_float_rect(
    rect: FloatRect,
    axis: SplitAxis,
    ratio: f32,
    gap: TileGap,
) -> (FloatRect, FloatRect) {
    let ratio = clamp_split_ratio(ratio);
    match axis {
        SplitAxis::Horizontal => {
            let gap = gap.for_axis(axis);
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
            let gap = gap.for_axis(axis);
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
    gap: TileGap,
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
    let gap = gap.for_axis(target_axis);
    let extent = match target_axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    };
    let usable_gap = if extent > gap { gap } else { 0.0 };
    Some((extent - usable_gap).max(1.0))
}

pub fn split_available_for_edge(
    tree: &DwindleTree,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: SplitEdge,
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
    let (child, child_rect, focused_is_first) = if tree_contains(first, focused) {
        (first.as_ref(), first_rect, true)
    } else if tree_contains(second, focused) {
        (second.as_ref(), second_rect, false)
    } else {
        return None;
    };

    if let Some(deeper) =
        split_available_for_edge(child, child_rect, gap, focused, target_axis, edge)
    {
        return Some(deeper);
    }

    if *axis != target_axis || !split_edge_matches_focused_side(edge, focused_is_first) {
        return None;
    }
    let gap = gap.for_axis(target_axis);
    let extent = match target_axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    };
    let usable_gap = if extent > gap { gap } else { 0.0 };
    Some((extent - usable_gap).max(1.0))
}

/// Axis, divided extent, and side of the innermost split holding `focused` - the divider between
/// it and its immediate sibling, whichever way that one happens to be split.
pub fn innermost_split_for(
    tree: &DwindleTree,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
) -> Option<(SplitAxis, f32, bool)> {
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
    let (child, child_rect, focused_is_first) = if tree_contains(first, focused) {
        (first.as_ref(), first_rect, true)
    } else if tree_contains(second, focused) {
        (second.as_ref(), second_rect, false)
    } else {
        return None;
    };

    if let Some(deeper) = innermost_split_for(child, child_rect, gap, focused) {
        return Some(deeper);
    }
    let usable = usable_axis_extent(axis_extent(rect, *axis), *axis, gap).max(1.0);
    Some((*axis, usable, focused_is_first))
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

/// Tree path to the nearest split on `target_axis` containing `focused`. Equal paths identify the
/// same split, allowing a geometric junction to deduplicate representatives from adjacent panes.
pub fn nearest_axis_split_path(
    tree: &DwindleTree,
    focused: PaneId,
    target_axis: SplitAxis,
) -> Option<Vec<bool>> {
    axis_split_path(tree, focused, target_axis, None)
}

/// Tree path to the nearest split on `target_axis` that faces the requested pane edge.
/// Equal paths identify the same grabbed divider at a geometric junction.
pub fn axis_split_path_for_edge(
    tree: &DwindleTree,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: SplitEdge,
) -> Option<Vec<bool>> {
    axis_split_path(tree, focused, target_axis, Some(edge))
}

fn axis_split_path(
    tree: &DwindleTree,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: Option<SplitEdge>,
) -> Option<Vec<bool>> {
    fn visit(
        tree: &DwindleTree,
        focused: PaneId,
        target_axis: SplitAxis,
        edge: Option<SplitEdge>,
        path: &mut Vec<bool>,
    ) -> Option<Vec<bool>> {
        let DwindleTree::Split {
            axis,
            first,
            second,
            ..
        } = tree
        else {
            return None;
        };
        let (child, second_side) = if tree_contains(first, focused) {
            (first.as_ref(), false)
        } else if tree_contains(second, focused) {
            (second.as_ref(), true)
        } else {
            return None;
        };

        path.push(second_side);
        if let Some(deeper) = visit(child, focused, target_axis, edge, path) {
            return Some(deeper);
        }
        path.pop();
        (*axis == target_axis
            && edge.is_none_or(|edge| split_edge_matches_focused_side(edge, !second_side)))
        .then(|| path.clone())
    }

    visit(tree, focused, target_axis, edge, &mut Vec::new())
}

/// Move the divider between `focused`'s split neighbours by `pixels` along `target_axis`.
///
/// `rect` is the tile area the tree is laid out in. `pixels` is signed toward the *first* side of
/// the split, so a caller that grabs a boundary from its trailing side passes the raw pointer
/// delta while one working from the leading side flips it.
pub fn resize_tiled_split(
    workspace: &mut Workspace,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
    target_axis: SplitAxis,
    pixels: f32,
) -> bool {
    resize_workspace_split(workspace, rect, gap, focused, target_axis, None, pixels)
}

/// As `resize_tiled_split`, but only the split facing the requested pane edge qualifies, so a pane
/// with splits on both sides of the same axis resizes the one that was actually grabbed.
pub fn resize_tiled_split_for_edge(
    workspace: &mut Workspace,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: SplitEdge,
    pixels: f32,
) -> bool {
    resize_workspace_split(
        workspace,
        rect,
        gap,
        focused,
        target_axis,
        Some(edge),
        pixels,
    )
}

fn resize_workspace_split(
    workspace: &mut Workspace,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: Option<SplitEdge>,
    pixels: f32,
) -> bool {
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = crate::layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return false;
    };
    resize_split_in_tree(tree, rect, gap, focused, target_axis, edge, pixels)
}

fn resize_split_in_tree(
    tree: &mut DwindleTree,
    rect: FloatRect,
    gap: TileGap,
    focused: PaneId,
    target_axis: SplitAxis,
    edge: Option<SplitEdge>,
    pixels: f32,
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
    let axis = *axis;
    let (first_rect, second_rect) = split_float_rect(rect, axis, *ratio, gap);
    let focused_is_first = if tree_contains(first, focused) {
        true
    } else if tree_contains(second, focused) {
        false
    } else {
        return false;
    };

    let (child, child_rect) = if focused_is_first {
        (first.as_mut(), first_rect)
    } else {
        (second.as_mut(), second_rect)
    };
    if resize_split_in_tree(child, child_rect, gap, focused, target_axis, edge, pixels) {
        return true;
    }
    if axis != target_axis
        || !edge.is_none_or(|edge| split_edge_matches_focused_side(edge, focused_is_first))
    {
        return false;
    }

    let usable = usable_axis_extent(axis_extent(rect, axis), axis, gap);
    let before = axis_extent(first_rect, axis);
    let signed = if focused_is_first { pixels } else { -pixels };
    *ratio = cell_split_ratio(before + signed, usable);
    let (moved_first, moved_second) = split_float_rect(rect, axis, *ratio, gap);
    // Ratios are proportional, so the regions on either side of this divider would carry their own
    // nested dividers along as they grow and shrink - dragging one boundary would visibly move
    // several. Rewrite those nested ratios to pin each nested divider where it already is, leaving
    // the two panes that actually touch this boundary to absorb the whole change.
    let moved = axis_extent(moved_first, axis) - before;
    hold_trailing_dividers(first, moved_first, gap, axis, moved);
    hold_leading_dividers(second, moved_second, gap, axis, -moved);
    true
}

fn axis_extent(rect: FloatRect, axis: SplitAxis) -> f32 {
    match axis {
        SplitAxis::Horizontal => rect.w,
        SplitAxis::Vertical => rect.h,
    }
}

/// The ratio that puts a divider exactly `cells` into a `usable`-cell region.
///
/// A ratio is how a split is stored, but the cell is what it means: `split_float_rect` renders a
/// divider by rounding `usable * ratio`, so a ratio landing mid-cell leaves the boundary on a
/// rounding tie. With an odd `usable` and a ratio near 0.5 *every* whole-cell offset lands on one,
/// and f32 representation error alone decides whether a one-cell nudge moves the divider by 0, 1,
/// or 2. Committing the cell count and deriving the ratio from it keeps that product whole, so
/// `round(usable * ratio) == cells` for anything a resize produces.
///
/// The ratio clamp is applied in cells for the same reason: clamping afterwards would put the
/// divider back on a fraction at the two extremes.
pub fn cell_split_ratio(cells: f32, usable: f32) -> f32 {
    let usable = usable.max(1.0);
    let lowest = (MIN_SPLIT_RATIO * usable).ceil();
    let highest = (MAX_SPLIT_RATIO * usable).floor().max(lowest);
    clamp_split_ratio(cells.round().clamp(lowest, highest) / usable)
}

/// The part of `extent` that a split actually divides: the gap between the two sides is taken off
/// the top, exactly as `split_float_rect` does.
pub fn usable_axis_extent(extent: f32, axis: SplitAxis, gap: TileGap) -> f32 {
    let gap = gap.for_axis(axis);
    let usable_gap = if extent > gap { gap } else { 0.0 };
    (extent - usable_gap).max(0.0)
}

/// `rect` is a region whose extent along `axis` just changed by `delta` at its *trailing* edge; its
/// leading edge did not move. Rewrite the ratios of the splits touching that edge so their own
/// dividers keep their absolute position.
fn hold_trailing_dividers(
    tree: &mut DwindleTree,
    rect: FloatRect,
    gap: TileGap,
    axis: SplitAxis,
    delta: f32,
) {
    if delta == 0.0 {
        return;
    }
    let DwindleTree::Split {
        axis: split_axis,
        ratio,
        first,
        second,
    } = tree
    else {
        return;
    };
    let split_axis = *split_axis;
    let (first_rect, second_rect) = split_float_rect(rect, split_axis, *ratio, gap);
    if split_axis != axis {
        // A perpendicular split does not divide `axis`: both children span the whole extent, so
        // the edge that moved belongs to both of them.
        hold_trailing_dividers(first, first_rect, gap, axis, delta);
        hold_trailing_dividers(second, second_rect, gap, axis, delta);
        return;
    }

    let extent = axis_extent(rect, axis);
    let usable = usable_axis_extent(extent, axis, gap);
    let before =
        (usable_axis_extent(extent - delta, axis, gap) * clamp_split_ratio(*ratio)).round();
    *ratio = clamp_split_ratio(before / usable.max(1.0));
    let (first_rect, second_rect) = split_float_rect(rect, split_axis, *ratio, gap);
    // Non-zero only when the rewritten ratio hit its clamp and the first side had to move after
    // all; whatever it could not hold is passed on to the side beyond it.
    let held = axis_extent(first_rect, axis) - before;
    hold_trailing_dividers(first, first_rect, gap, axis, held);
    hold_trailing_dividers(second, second_rect, gap, axis, delta - held);
}

/// Mirror of `hold_trailing_dividers` for a region whose *leading* edge moved while its trailing
/// edge stayed put. `delta` is the change in extent, so the region's start moved by `-delta`.
fn hold_leading_dividers(
    tree: &mut DwindleTree,
    rect: FloatRect,
    gap: TileGap,
    axis: SplitAxis,
    delta: f32,
) {
    if delta == 0.0 {
        return;
    }
    let DwindleTree::Split {
        axis: split_axis,
        ratio,
        first,
        second,
    } = tree
    else {
        return;
    };
    let split_axis = *split_axis;
    let (first_rect, second_rect) = split_float_rect(rect, split_axis, *ratio, gap);
    if split_axis != axis {
        hold_leading_dividers(first, first_rect, gap, axis, delta);
        hold_leading_dividers(second, second_rect, gap, axis, delta);
        return;
    }

    let extent = axis_extent(rect, axis);
    let usable = usable_axis_extent(extent, axis, gap);
    let before =
        (usable_axis_extent(extent - delta, axis, gap) * clamp_split_ratio(*ratio)).round();
    // The start moved by `-delta`, so the first side has to take `delta` more to leave this
    // divider where it was.
    *ratio = clamp_split_ratio((before + delta) / usable.max(1.0));
    let (first_rect, second_rect) = split_float_rect(rect, split_axis, *ratio, gap);
    let held = axis_extent(first_rect, axis) - (before + delta);
    hold_leading_dividers(first, first_rect, gap, axis, delta + held);
    hold_leading_dividers(second, second_rect, gap, axis, -held);
}

fn split_edge_matches_focused_side(edge: SplitEdge, focused_is_first: bool) -> bool {
    matches!(
        (edge, focused_is_first),
        (SplitEdge::Leading, false) | (SplitEdge::Trailing, true)
    )
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
        let (first, second) = split_float_rect(rect, SplitAxis::Horizontal, 0.5, TileGap::DEFAULT);
        assert_close(first.x + first.w + 1.0, second.x);
        assert_close(second.x + second.w, 101.0);
    }

    #[test]
    fn split_float_rect_removes_vertical_row_gap() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 41.0,
        };
        let (first, second) = split_float_rect(rect, SplitAxis::Vertical, 0.5, TileGap::DEFAULT);
        assert_close(first.y + first.h, second.y);
        assert_close(second.y + second.h, 41.0);
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
            TileGap::DEFAULT,
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
            TileGap::DEFAULT,
            0.5,
            &mut placements,
        );

        assert_eq!(placements.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
        assert_close(placements[0].rect.x, 0.0);
        assert_close(placements[0].rect.w, 50.0);
        assert_close(placements[1].rect.x, 51.0);
        assert_close(
            placements[1].rect.y + placements[1].rect.h,
            placements[2].rect.y,
        );
        assert_close(placements[2].rect.y + placements[2].rect.h, 41.0);
    }

    #[test]
    fn swap_tree_leaves_exchanges_payloads_only() {
        let mut tree = build_dwindle_tree(&[1, 2, 3], SplitAxis::Horizontal, &[0.5, 0.5]).unwrap();
        let before = tree.clone();
        assert!(swap_tree_leaves(&mut tree, 1, 3));

        let mut leaves = Vec::new();
        collect_tree_leaves(&tree, &mut leaves);
        assert_eq!(leaves, [3, 2, 1]);

        // Structure (axes/ratios) is unchanged: same split shape, only leaf ids moved.
        let axes_before = split_axes(&before);
        let axes_after = split_axes(&tree);
        assert_eq!(axes_before, axes_after);

        // Swapping an absent id reports failure and leaves the tree untouched.
        let snapshot = tree.clone();
        assert!(!swap_tree_leaves(&mut tree, 1, 99));
        assert_eq!(tree, snapshot);
    }

    fn split_axes(tree: &DwindleTree) -> Vec<SplitAxis> {
        let mut out = Vec::new();
        fn walk(tree: &DwindleTree, out: &mut Vec<SplitAxis>) {
            if let DwindleTree::Split {
                axis,
                first,
                second,
                ..
            } = tree
            {
                out.push(*axis);
                walk(first, out);
                walk(second, out);
            }
        }
        walk(tree, &mut out);
        out
    }

    #[test]
    fn grid_allocates_two_by_two_for_four_panes() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let mut placements = Vec::new();
        allocate_grid(&[1, 2, 3, 4], rect, TileGap::DEFAULT, &mut placements);

        assert_eq!(placements.len(), 4);
        assert_eq!(
            placements.iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        // Top-left flush with the rect origin; bottom-right flush with the far edge.
        assert_close(placements[0].rect.x, 0.0);
        assert_close(placements[0].rect.y, 0.0);
        assert_close(placements[3].rect.x + placements[3].rect.w, 100.0);
        assert_close(placements[3].rect.y + placements[3].rect.h, 40.0);
        // Two rows: first row top, last row bottom.
        assert_close(
            placements[0].rect.y + placements[0].rect.h,
            placements[2].rect.y,
        );
    }

    #[test]
    fn grid_last_row_spans_when_not_full() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let mut placements = Vec::new();
        allocate_grid(&[1, 2, 3], rect, TileGap::DEFAULT, &mut placements);

        assert_eq!(placements.len(), 3);
        // Third pane is alone on the last row and spans the full width.
        assert_close(placements[2].rect.x, 0.0);
        assert_close(placements[2].rect.w, 100.0);
    }

    #[test]
    fn monocle_gives_every_pane_the_full_rect() {
        let rect = FloatRect {
            x: 2.0,
            y: 3.0,
            w: 80.0,
            h: 24.0,
        };
        let mut placements = Vec::new();
        allocate_monocle(&[1, 2, 3], rect, &mut placements);

        assert_eq!(placements.len(), 3);
        for placement in placements {
            assert_eq!(placement.rect, rect);
        }
    }

    #[test]
    fn columns_split_full_height_equal_widths_and_span_bounds() {
        // 100 wide with two unit gaps leaves 98 cells for 3 columns. `split_evenly` would yield
        // 32/32/34; Columns must spread the remainder so widths differ by at most one cell.
        let rect = FloatRect {
            x: 0.0,
            y: 1.0,
            w: 100.0,
            h: 40.0,
        };
        let mut placements = Vec::new();
        allocate_columns(&[1, 2, 3], rect, TileGap::DEFAULT, &mut placements);

        assert_eq!(placements.len(), 3);
        for placement in &placements {
            assert_close(placement.rect.y, rect.y);
            assert_close(placement.rect.h, rect.h);
        }
        assert_close(placements[0].rect.x, 0.0);
        assert_close(placements[2].rect.x + placements[2].rect.w, rect.w);
        assert_close(
            placements[1].rect.x,
            placements[0].rect.x + placements[0].rect.w + 1.0,
        );
        assert_close(
            placements[2].rect.x,
            placements[1].rect.x + placements[1].rect.w + 1.0,
        );
        let widths: Vec<f32> = placements.iter().map(|p| p.rect.w).collect();
        let max_w = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let min_w = widths.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            max_w - min_w <= 1.0 + f32::EPSILON,
            "column widths must differ by at most one cell, got {widths:?}"
        );
        assert_eq!(widths, vec![33.0, 33.0, 32.0]);
    }

    #[test]
    fn columns_respect_positive_and_merged_horizontal_gaps() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 20.0,
        };

        let mut positive = Vec::new();
        allocate_columns(&[1, 2], rect, TileGap::DEFAULT, &mut positive);
        assert_close(
            positive[1].rect.x,
            positive[0].rect.x + positive[0].rect.w + 1.0,
        );
        assert_close(positive[1].rect.x + positive[1].rect.w, rect.w);

        let mut merged = Vec::new();
        allocate_columns(
            &[1, 2],
            rect,
            TileGap {
                horizontal: -1.0,
                vertical: 0.0,
            },
            &mut merged,
        );
        assert_close(merged[1].rect.x, merged[0].rect.x + merged[0].rect.w - 1.0);
        assert_close(merged[1].rect.x + merged[1].rect.w, rect.w);
    }

    #[test]
    fn scrollable_default_width_is_forty_five_percent() {
        assert_close(DEFAULT_SCROLLABLE_WIDTH, 0.45);
        let cells = scrollable_column_width(100.0, DEFAULT_SCROLLABLE_WIDTH);
        assert_close(cells, 45.0);
        assert_close(
            sanitize_scrollable_width(f32::NAN),
            DEFAULT_SCROLLABLE_WIDTH,
        );
        assert_close(sanitize_scrollable_width(0.05), MIN_SPLIT_RATIO);
        assert_close(sanitize_scrollable_width(0.95), MAX_SPLIT_RATIO);
    }

    #[test]
    fn scrollable_one_and_two_panes_flex_to_fill_canonical_width() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let mut one = Vec::new();
        allocate_scrollable(
            &[(1, 0.45)],
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut one,
        );
        assert_close(one[0].rect.w, rect.w);
        assert_close(one[0].rect.x, rect.x);

        let mut two = Vec::new();
        allocate_scrollable(
            &[(1, 0.45), (2, 0.30)],
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut two,
        );
        let gap = TileGap::DEFAULT.for_axis(SplitAxis::Horizontal);
        assert_close(two[0].rect.w + gap + two[1].rect.w, rect.w);
        assert_close(two[0].rect.w - two[1].rect.w, 45.0 - 30.0);
        assert!(two[0].rect.w >= 45.0 - f32::EPSILON);
        assert!(two[1].rect.w >= 30.0 - f32::EPSILON);

        let mut overflow = Vec::new();
        allocate_scrollable(
            &[(1, 0.80), (2, 0.80)],
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut overflow,
        );
        assert_close(overflow[0].rect.w, 80.0);
        assert_close(overflow[1].rect.w, 80.0);
    }

    #[test]
    fn scrollable_widths_are_heterogeneous_and_stable_across_pane_count() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let panes = [(1, 0.45), (2, 0.30), (3, 0.60)];
        let mut before = Vec::new();
        allocate_scrollable(
            &panes,
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut before,
        );
        assert_eq!(
            before.iter().map(|p| p.rect.w).collect::<Vec<_>>(),
            vec![45.0, 30.0, 60.0]
        );

        let mut after = Vec::new();
        let mut appended = panes.to_vec();
        appended.push((99, 0.50));
        allocate_scrollable(
            &appended,
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut after,
        );
        for (id, frac) in panes {
            let w = after.iter().find(|p| p.id == id).expect("survivor").rect.w;
            assert_close(w, scrollable_column_width(rect.w, frac));
        }
    }

    #[test]
    fn scrollable_anchor_translation_keeps_focused_pane_visible() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let panes = [(1, 0.45), (2, 0.45), (3, 0.45), (4, 0.45)];

        let mut first = Vec::new();
        allocate_scrollable(
            &panes,
            rect,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut first,
        );
        assert_close(first[0].rect.x, 0.0);
        assert!(first[0].rect.x + first[0].rect.w <= rect.x + rect.w + f32::EPSILON);

        let mut middle_right = Vec::new();
        allocate_scrollable(
            &panes,
            rect,
            TileGap::DEFAULT,
            Some(3),
            ScrollableRevealEdge::Right,
            &mut middle_right,
        );
        let mid = middle_right.iter().find(|p| p.id == 3).unwrap();
        assert!(mid.rect.x >= rect.x - f32::EPSILON);
        assert_close(mid.rect.x + mid.rect.w, rect.x + rect.w);

        // Pane 2 can left-align without hitting scroll_max (pane 3 cannot).
        let mut early_left = Vec::new();
        allocate_scrollable(
            &panes,
            rect,
            TileGap::DEFAULT,
            Some(2),
            ScrollableRevealEdge::Left,
            &mut early_left,
        );
        let early = early_left.iter().find(|p| p.id == 2).unwrap();
        assert_close(early.rect.x, rect.x);
        assert!(early.rect.x + early.rect.w <= rect.x + rect.w + f32::EPSILON);

        let mut last = Vec::new();
        allocate_scrollable(
            &panes,
            rect,
            TileGap::DEFAULT,
            Some(4),
            ScrollableRevealEdge::Right,
            &mut last,
        );
        let end = last.iter().find(|p| p.id == 4).unwrap();
        assert!(end.rect.x >= rect.x - f32::EPSILON);
        assert_close(end.rect.x + end.rect.w, rect.x + rect.w);
        assert!(
            last[0].rect.x < rect.x,
            "earlier columns may leave the viewport"
        );
        assert_close(end.rect.w, 45.0);
    }

    #[test]
    fn scrollable_visible_interval_allows_negative_scroll_for_letterbox() {
        let layout = FloatRect {
            x: -25.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let visible = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 24.0,
        };
        let panes = [(1, 0.45), (2, 0.45)];

        let mut first = Vec::new();
        allocate_scrollable_with_visible(
            &panes,
            layout,
            visible,
            TileGap::DEFAULT,
            Some(1),
            ScrollableRevealEdge::Left,
            &mut first,
        );
        assert_close(first[0].rect.x, 0.0);
        assert!(first[0].rect.x + first[0].rect.w <= visible.x + visible.w + 0.5);

        let mut second = Vec::new();
        allocate_scrollable_with_visible(
            &panes,
            layout,
            visible,
            TileGap::DEFAULT,
            Some(2),
            ScrollableRevealEdge::Right,
            &mut second,
        );
        let pane2 = second.iter().find(|p| p.id == 2).unwrap();
        assert!(pane2.rect.x >= visible.x - 0.5);
        assert_close(pane2.rect.x + pane2.rect.w, visible.x + visible.w);
        assert!(
            (pane2.rect.x - first[1].rect.x).abs() > 0.5,
            "changing anchor must move the strip when visible is narrower than canonical"
        );
    }

    #[test]
    fn scrollable_visible_equals_layout_matches_allocate_scrollable() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let panes = [(1, 0.45), (2, 0.45), (3, 0.45), (4, 0.45)];
        for anchor in [1, 2, 3, 4] {
            for edge in [ScrollableRevealEdge::Left, ScrollableRevealEdge::Right] {
                let mut a = Vec::new();
                let mut b = Vec::new();
                allocate_scrollable(&panes, rect, TileGap::DEFAULT, Some(anchor), edge, &mut a);
                allocate_scrollable_with_visible(
                    &panes,
                    rect,
                    rect,
                    TileGap::DEFAULT,
                    Some(anchor),
                    edge,
                    &mut b,
                );
                assert_eq!(a.len(), b.len());
                for (left, right) in a.iter().zip(b.iter()) {
                    assert_eq!(left.id, right.id);
                    assert_close(left.rect.x, right.rect.x);
                    assert_close(left.rect.w, right.rect.w);
                }
            }
        }
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

    #[test]
    fn edge_resize_targets_grabbed_left_boundary_not_deeper_right_boundary() {
        let mut workspace = Workspace::new(0);
        workspace.tile_tree =
            build_dwindle_tree(&[1, 2, 3, 4], SplitAxis::Horizontal, &[0.25, 0.5, 0.5]);
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let available = split_available_for_edge(
            workspace.tile_tree.as_ref().unwrap(),
            rect,
            TileGap::DEFAULT,
            3,
            SplitAxis::Horizontal,
            SplitEdge::Leading,
        )
        .unwrap();

        assert_close(available, 99.0);
        assert!(resize_tiled_split_for_edge(
            &mut workspace,
            rect,
            TileGap::DEFAULT,
            3,
            SplitAxis::Horizontal,
            SplitEdge::Leading,
            2.475,
        ));

        // Only the grabbed root boundary moves: the nested 3|4 divider keeps its column.
        // The boundary starts on column 25 - where `0.25` of 99 columns renders - so a 2.475
        // column pull leaves it on 23, not on the 22 a purely proportional ratio would give.
        assert_eq!(
            dwindle_columns(&workspace, rect),
            vec![
                (1, 0.0, 23.0),
                (2, 24.0, 100.0),
                (3, 24.0, 63.0),
                (4, 64.0, 100.0)
            ]
        );
    }

    #[test]
    fn edge_resize_targets_grabbed_right_boundary_when_deepest() {
        let mut workspace = Workspace::new(0);
        workspace.tile_tree =
            build_dwindle_tree(&[1, 2, 3, 4], SplitAxis::Horizontal, &[0.25, 0.5, 0.5]);
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let available = split_available_for_edge(
            workspace.tile_tree.as_ref().unwrap(),
            rect,
            TileGap::DEFAULT,
            3,
            SplitAxis::Horizontal,
            SplitEdge::Trailing,
        )
        .unwrap();

        assert_close(available, 73.0);
        assert!(resize_tiled_split_for_edge(
            &mut workspace,
            rect,
            TileGap::DEFAULT,
            3,
            SplitAxis::Horizontal,
            SplitEdge::Trailing,
            7.3,
        ));

        // The deepest boundary on the grabbed side moves; the root boundary stays put.
        assert_eq!(
            dwindle_columns(&workspace, rect),
            vec![
                (1, 0.0, 25.0),
                (2, 26.0, 100.0),
                (3, 26.0, 70.0),
                (4, 71.0, 100.0)
            ]
        );
    }

    fn dwindle_columns(workspace: &Workspace, rect: FloatRect) -> Vec<(PaneId, f32, f32)> {
        let mut placements = Vec::new();
        allocate_dwindle(
            workspace.tile_tree.as_ref().unwrap(),
            rect,
            TileGap::DEFAULT,
            &mut placements,
        );
        placements
            .iter()
            .map(|p| (p.id, p.rect.x, p.rect.x + p.rect.w))
            .collect()
    }
}
