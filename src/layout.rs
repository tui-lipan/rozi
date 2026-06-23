use tui_lipan::prelude::FloatRect;

use crate::geometry::{clamp_floating_rect, float_rect_contains_point, inset_float_rect};
use crate::state::{
    LayoutKind, OUTER_GAP, Pane, PaneId, SPLIT_WIDTH_MULTIPLIER, SplitAxis, TILE_GAP, Workspace,
};
use crate::tiling::{
    DwindleTree, PanePlacement, allocate_dwindle, allocate_grid, allocate_master, allocate_monocle,
    allocate_spiral, append_tiled_window, build_dwindle_tree, insert_leaf_around_target,
    leaf_depth, prune_tree_to_ids, ratio_at, tree_contains,
};

pub fn workspace_target_rects(workspace: &Workspace, bounds: FloatRect) -> Vec<PanePlacement> {
    workspace_target_rects_excluding(workspace, bounds, None)
}

pub fn workspace_target_rects_excluding(
    workspace: &Workspace,
    bounds: FloatRect,
    exclude_tiled: Option<PaneId>,
) -> Vec<PanePlacement> {
    let mut placements = Vec::new();
    let tile_bounds = inset_float_rect(bounds, OUTER_GAP);
    match workspace.layout_kind {
        LayoutKind::Dwindle => {
            if let Some(tree) = effective_tile_tree(workspace, exclude_tiled) {
                allocate_dwindle(&tree, tile_bounds, TILE_GAP, &mut placements);
            }
        }
        LayoutKind::Master => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_master(
                &ids,
                tile_bounds,
                TILE_GAP,
                ratio_at(&workspace.split_ratios, 0),
                &mut placements,
            );
        }
        LayoutKind::Grid => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_grid(&ids, tile_bounds, TILE_GAP, &mut placements);
        }
        LayoutKind::Monocle => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_monocle(&ids, tile_bounds, &mut placements);
        }
        LayoutKind::Spiral => {
            if let Some(tree) = effective_tile_tree(workspace, exclude_tiled) {
                allocate_spiral(&tree, tile_bounds, TILE_GAP, &mut placements);
            }
        }
    }

    for pane in workspace
        .panes
        .iter()
        .filter(|pane| pane.floating && !pane.closing)
    {
        placements.push(PanePlacement {
            id: pane.id,
            rect: clamp_floating_rect(pane.floating_rect, bounds),
        });
    }

    placements
}

/// Tiled ids for order-driven layouts (master/grid/monocle), in tree-leaf order with the
/// optionally moving pane excluded.
fn order_driven_ids(workspace: &Workspace, exclude_tiled: Option<PaneId>) -> Vec<PaneId> {
    workspace
        .tiled_ids()
        .into_iter()
        .filter(|id| Some(*id) != exclude_tiled)
        .collect()
}

pub fn effective_tile_tree(
    workspace: &Workspace,
    exclude_tiled: Option<PaneId>,
) -> Option<DwindleTree> {
    let active_ids: Vec<PaneId> = workspace
        .active_tiled_ids_by_pane_order()
        .into_iter()
        .filter(|id| Some(*id) != exclude_tiled)
        .collect();
    if active_ids.is_empty() {
        return None;
    }

    let mut tree = workspace
        .tile_tree
        .clone()
        .and_then(|tree| prune_tree_to_ids(tree, &active_ids))
        .or_else(|| build_dwindle_tree(&active_ids, workspace.start_axis, &workspace.split_ratios));

    for id in active_ids {
        if !tree.as_ref().is_some_and(|tree| tree_contains(tree, id)) {
            tree = Some(crate::tiling::append_tiled_leaf(
                tree,
                id,
                workspace.start_axis,
            ));
        }
    }

    tree
}

pub fn placement_for(placements: &[PanePlacement], id: PaneId) -> Option<FloatRect> {
    placements
        .iter()
        .find(|placement| placement.id == id)
        .map(|placement| placement.rect)
}

pub fn ordered_panes(workspace: &Workspace, focused: Option<PaneId>) -> Vec<&Pane> {
    let mut panes: Vec<&Pane> = workspace.panes.iter().collect();
    panes.sort_by_key(|pane| {
        (
            pane_z_group(pane),
            pane.fullscreen,
            focused == Some(pane.id),
            pane.id,
        )
    });
    panes
}

fn pane_z_group(pane: &Pane) -> u8 {
    match (pane.closing, pane.floating) {
        // Tiled close animations should not cover the panes expanding into their space.
        (true, false) => 0,
        (false, false) => 1,
        (false, true) => 2,
        // Floating windows do not resize the tile layout, so keep their fade-out above it.
        (true, true) => 3,
    }
}

/// Dwindle split direction for the focused tile: split the longer side so the two halves
/// stay roughly square (Hyprland compares the node's width vs height). Width is weighted by
/// [`SPLIT_WIDTH_MULTIPLIER`] for terminal cell aspect. The new pane takes the second
/// (right/bottom) slot — a fixed side, not the cursor (Hyprland's `force_split = 2`).
pub fn spawn_split_for_rect(rect: FloatRect) -> (SplitAxis, bool) {
    let axis = if rect.w >= rect.h * SPLIT_WIDTH_MULTIPLIER {
        SplitAxis::Horizontal
    } else {
        SplitAxis::Vertical
    };
    (axis, false)
}

pub fn drop_split_for_target(target_rect: FloatRect, point: (f32, f32)) -> (SplitAxis, bool) {
    let local_x = ((point.0 - target_rect.x) / target_rect.w.max(1.0)).clamp(0.0, 1.0);
    let local_y = ((point.1 - target_rect.y) / target_rect.h.max(1.0)).clamp(0.0, 1.0);
    let from_center_x = local_x - 0.5;
    let from_center_y = local_y - 0.5;

    if from_center_x.abs() >= from_center_y.abs() {
        (SplitAxis::Horizontal, from_center_x < 0.0)
    } else {
        (SplitAxis::Vertical, from_center_y < 0.0)
    }
}

pub fn target_tiled_pane_for_drop(
    placements: &[PanePlacement],
    tiled_ids: &[PaneId],
    point: (f32, f32),
) -> Option<PaneId> {
    placements
        .iter()
        .rev()
        .find(|placement| float_rect_contains_point(placement.rect, point))
        .and_then(|placement| tiled_ids.contains(&placement.id).then_some(placement.id))
}

pub fn insert_tiled_pane_at_point(
    workspace: &mut Workspace,
    id: PaneId,
    point: (f32, f32),
    bounds: FloatRect,
) -> Option<(PaneId, bool)> {
    let target = {
        let placements = workspace_target_rects_excluding(workspace, bounds, Some(id));
        let tiled_ids: Vec<PaneId> = workspace
            .tiled_ids()
            .into_iter()
            .filter(|target_id| *target_id != id)
            .collect();
        target_tiled_pane_for_drop(&placements, &tiled_ids, point).and_then(|target_id| {
            placement_for(&placements, target_id).map(|rect| (target_id, rect))
        })
    };

    let (target_id, target_rect) = target?;
    let (axis, moving_first) = drop_split_for_target(target_rect, point);
    insert_tiled_pane_around_target(workspace, id, target_id, axis, moving_first)
        .then_some((target_id, moving_first))
}

pub fn insert_tiled_pane_around_target(
    workspace: &mut Workspace,
    id: PaneId,
    target: PaneId,
    axis: SplitAxis,
    moving_first: bool,
) -> bool {
    if id == target {
        return false;
    }
    let Some(tree) = effective_tile_tree(workspace, Some(id)) else {
        return false;
    };
    let Some(inserted) = insert_leaf_around_target(tree, target, id, axis, moving_first) else {
        return false;
    };
    workspace.tile_tree = Some(inserted);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnPlacement {
    /// Split off the focused tile (the given pane).
    Split(PaneId),
    /// Appended to the tree (first tiled pane, or no valid split target).
    Appended,
}

/// Insert `id` by splitting the focused pane — Hyprland's dwindle behavior: a new pane
/// always splits the currently focused one, never the tile under the cursor. The split
/// axis alternates by the focused leaf's depth, matching dwindle's regular V/H/V/H growth.
/// Falls back to a plain append when there is no valid split target (the first pane, or a
/// floating focus).
pub fn place_spawned_pane(
    workspace: &mut Workspace,
    id: PaneId,
    previous_focused: Option<PaneId>,
    bounds: FloatRect,
) -> SpawnPlacement {
    // Order-driven layouts (master/grid/monocle) read pane order, not split structure, so a
    // new pane simply appends to the end. Dwindle and spiral split the focused tile.
    if matches!(
        workspace.layout_kind,
        LayoutKind::Master | LayoutKind::Grid | LayoutKind::Monocle
    ) {
        append_tiled_window(workspace, id);
        return SpawnPlacement::Appended;
    }

    if let Some(target) = previous_focused.filter(|target| *target != id) {
        let placements = workspace_target_rects_excluding(workspace, bounds, Some(id));
        if let Some(rect) = placement_for(&placements, target) {
            let axis = spawn_split_for_target(workspace, id, target)
                .unwrap_or_else(|| spawn_split_for_rect(rect).0);
            let moving_first = false;
            if insert_tiled_pane_around_target(workspace, id, target, axis, moving_first) {
                return SpawnPlacement::Split(target);
            }
        }
    }

    append_tiled_window(workspace, id);
    SpawnPlacement::Appended
}

fn spawn_split_for_target(
    workspace: &Workspace,
    moving: PaneId,
    target: PaneId,
) -> Option<SplitAxis> {
    let tree = effective_tile_tree(workspace, Some(moving))?;
    leaf_depth(&tree, target).map(|depth| workspace.start_axis.at_depth(depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_split_direction_follows_focused_tile_aspect() {
        // Wider than tall (after the cell-aspect multiplier) → split side by side.
        let wide = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 20.0,
        };
        assert_eq!(spawn_split_for_rect(wide).0, SplitAxis::Horizontal);

        // Taller/narrow → split top/bottom.
        let tall = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 30.0,
        };
        assert_eq!(spawn_split_for_rect(tall).0, SplitAxis::Vertical);

        // The new pane always takes the second (right/bottom) slot — fixed, not cursor.
        assert!(!spawn_split_for_rect(wide).1);
    }

    #[test]
    fn spawned_panes_alternate_split_axis_by_focused_depth() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 40.0,
        };
        let mut workspace = Workspace::new(0);

        for id in 1..=5 {
            let previous_focused = workspace.focused_pane;
            workspace.panes.push(Pane::new(id, 100, bounds));
            place_spawned_pane(&mut workspace, id, previous_focused, bounds);
            workspace.focused_pane = Some(id);
        }

        let mut axes = Vec::new();
        collect_axes(workspace.tile_tree.as_ref().unwrap(), &mut axes);
        assert_eq!(
            axes,
            vec![
                SplitAxis::Horizontal,
                SplitAxis::Vertical,
                SplitAxis::Horizontal,
                SplitAxis::Vertical,
            ]
        );
    }

    fn collect_axes(tree: &DwindleTree, out: &mut Vec<SplitAxis>) {
        if let DwindleTree::Split {
            axis,
            first,
            second,
            ..
        } = tree
        {
            out.push(*axis);
            collect_axes(first, out);
            collect_axes(second, out);
        }
    }

    #[test]
    fn ordered_panes_draws_tiled_closing_panes_under_expanding_panes() {
        fn pane(id: PaneId) -> Pane {
            Pane::new(
                id,
                100,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 80.0,
                    h: 24.0,
                },
            )
        }

        let mut closing_tiled = pane(1);
        closing_tiled.closing = true;

        let active_tiled = pane(2);

        let mut active_floating = pane(3);
        active_floating.floating = true;

        let mut closing_floating = pane(4);
        closing_floating.floating = true;
        closing_floating.closing = true;

        let mut workspace = Workspace::new(0);
        workspace.panes = vec![
            active_tiled,
            closing_floating,
            closing_tiled,
            active_floating,
        ];

        let ids: Vec<PaneId> = ordered_panes(&workspace, Some(2))
            .into_iter()
            .map(|pane| pane.id)
            .collect();

        assert_eq!(ids, vec![1, 2, 3, 4]);
    }
}
