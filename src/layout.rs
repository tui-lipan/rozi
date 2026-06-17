use tui_lipan::prelude::FloatRect;

use crate::geometry::{clamp_floating_rect, float_rect_contains_point, inset_float_rect};
use crate::state::{OUTER_GAP, Pane, PaneId, SPLIT_WIDTH_MULTIPLIER, SplitAxis, TILE_GAP, Workspace};
use crate::tiling::{
    DwindleTree, PanePlacement, allocate_dwindle, append_tiled_window, build_dwindle_tree,
    insert_leaf_around_target, prune_tree_to_ids, tree_contains,
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
    if let Some(tree) = effective_tile_tree(workspace, exclude_tiled) {
        allocate_dwindle(&tree, tile_bounds, TILE_GAP, &mut placements);
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
            pane.closing,
            pane.floating,
            pane.fullscreen,
            focused == Some(pane.id),
            pane.id,
        )
    });
    panes
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
/// axis comes from the focused tile's shape (`spawn_split_for_rect`). Falls back to a
/// plain append when there is no valid split target (the first pane, or a floating focus).
pub fn place_spawned_pane(
    workspace: &mut Workspace,
    id: PaneId,
    previous_focused: Option<PaneId>,
    bounds: FloatRect,
) -> SpawnPlacement {
    if let Some(target) = previous_focused.filter(|target| *target != id) {
        let placements = workspace_target_rects_excluding(workspace, bounds, Some(id));
        if let Some(rect) = placement_for(&placements, target) {
            let (axis, moving_first) = spawn_split_for_rect(rect);
            if insert_tiled_pane_around_target(workspace, id, target, axis, moving_first) {
                return SpawnPlacement::Split(target);
            }
        }
    }

    append_tiled_window(workspace, id);
    SpawnPlacement::Appended
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
}
