use tui_lipan::prelude::FloatRect;

use crate::anim::SlideEdge;
use crate::geometry::{clamp_floating_rect, float_rect_contains_point, workspace_tile_bounds};
use crate::state::{LayoutKind, Pane, PaneId, SplitAxis, TileGap, Workspace};
pub use crate::tiling::effective_tile_tree;
use crate::tiling::{
    PanePlacement, allocate_columns, allocate_dwindle, allocate_grid, allocate_master,
    allocate_monocle, allocate_rows, allocate_scrollable_with_visible, append_tiled_window,
    insert_leaf_around_target, ratio_at,
};

pub fn workspace_target_rects(
    workspace: &Workspace,
    bounds: FloatRect,
    top_gap: f32,
    tile_gap: TileGap,
) -> Vec<PanePlacement> {
    workspace_target_rects_excluding(workspace, bounds, None, top_gap, tile_gap)
}

/// Like [`workspace_target_rects`], but Scrollable scrolling is clamped to `visible_bounds`
/// (local canvas) while column widths still use canonical `bounds`. Non-Scrollable layouts ignore
/// `visible_bounds`.
pub fn workspace_target_rects_with_visible_bounds(
    workspace: &Workspace,
    bounds: FloatRect,
    visible_bounds: FloatRect,
    top_gap: f32,
    tile_gap: TileGap,
) -> Vec<PanePlacement> {
    workspace_target_rects_excluding_with_visible(
        workspace,
        bounds,
        Some(visible_bounds),
        None,
        top_gap,
        tile_gap,
    )
}

pub fn workspace_target_rects_excluding(
    workspace: &Workspace,
    bounds: FloatRect,
    exclude_tiled: Option<PaneId>,
    top_gap: f32,
    tile_gap: TileGap,
) -> Vec<PanePlacement> {
    workspace_target_rects_excluding_with_visible(
        workspace,
        bounds,
        None,
        exclude_tiled,
        top_gap,
        tile_gap,
    )
}

/// `visible_bounds` is the actually on-screen canvas (follower local viewport). When `None`,
/// Scrollable scrolling uses `bounds` alone — the historical local/controller path.
pub fn workspace_target_rects_excluding_with_visible(
    workspace: &Workspace,
    bounds: FloatRect,
    visible_bounds: Option<FloatRect>,
    exclude_tiled: Option<PaneId>,
    top_gap: f32,
    tile_gap: TileGap,
) -> Vec<PanePlacement> {
    let mut placements = Vec::new();
    let tile_bounds = workspace_tile_bounds(bounds, top_gap);
    match workspace.layout_kind {
        LayoutKind::Dwindle => {
            if let Some(tree) = effective_tile_tree(workspace, exclude_tiled) {
                allocate_dwindle(&tree, tile_bounds, tile_gap, &mut placements);
            }
        }
        LayoutKind::Master => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_master(
                &ids,
                tile_bounds,
                tile_gap,
                ratio_at(&workspace.split_ratios, 0),
                &mut placements,
            );
        }
        LayoutKind::Grid => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_grid(&ids, tile_bounds, tile_gap, &mut placements);
        }
        LayoutKind::Columns => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_columns(&ids, tile_bounds, tile_gap, &mut placements);
        }
        LayoutKind::Rows => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_rows(&ids, tile_bounds, tile_gap, &mut placements);
        }
        LayoutKind::Scrollable => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            let panes: Vec<(PaneId, f32)> = ids
                .iter()
                .map(|id| {
                    let width = workspace
                        .panes
                        .iter()
                        .find(|pane| pane.id == *id)
                        .map(|pane| pane.scrollable_width)
                        .unwrap_or(crate::state::DEFAULT_SCROLLABLE_WIDTH);
                    (*id, width)
                })
                .collect();
            let anchor = scrollable_viewport_anchor(workspace, &ids);
            // Scroll against the on-screen intersection of canonical and local tiles — not the
            // whole local tile — so a wider follower viewport keeps letterbox centering.
            let visible_tile = visible_bounds.map_or(tile_bounds, |visible| {
                horizontal_tile_intersection(tile_bounds, workspace_tile_bounds(visible, top_gap))
                    .unwrap_or(tile_bounds)
            });
            allocate_scrollable_with_visible(
                &panes,
                tile_bounds,
                visible_tile,
                tile_gap,
                anchor,
                workspace.scrollable_reveal_edge,
                &mut placements,
            );
        }
        LayoutKind::Monocle => {
            let ids = order_driven_ids(workspace, exclude_tiled);
            allocate_monocle(&ids, tile_bounds, &mut placements);
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

/// Horizontal intersection of canonical/layout and local tile bounds used as the Scrollable
/// visible scroll clamp. `None` when the intervals do not overlap.
fn horizontal_tile_intersection(layout: FloatRect, local: FloatRect) -> Option<FloatRect> {
    let left = layout.x.max(local.x);
    let right = (layout.x + layout.w).min(local.x + local.w);
    if !left.is_finite() || !right.is_finite() {
        return None;
    }
    let width = right - left;
    if width <= 0.0 || !width.is_finite() {
        return None;
    }
    Some(FloatRect {
        x: left,
        y: layout.y,
        w: width,
        h: layout.h,
    })
}

/// Tiled ids for order-driven layouts (master/grid/columns/rows/scrollable/monocle), in tree-leaf
/// order with the optionally moving pane excluded.
fn order_driven_ids(workspace: &Workspace, exclude_tiled: Option<PaneId>) -> Vec<PaneId> {
    workspace
        .tiled_ids()
        .into_iter()
        .filter(|id| Some(*id) != exclude_tiled)
        .collect()
}

/// Scrollable strip anchor: valid local anchor first (so a non-focusing spawn that only updates
/// remembered `focused_pane` cannot steal the viewport), then focused tiled pane, then first tiled.
pub(crate) fn scrollable_viewport_anchor(
    workspace: &Workspace,
    tiled_ids: &[PaneId],
) -> Option<PaneId> {
    if let Some(anchor) = workspace
        .scrollable_anchor
        .filter(|id| tiled_ids.contains(id))
    {
        return Some(anchor);
    }
    if let Some(focused) = workspace.focused_pane.filter(|id| {
        workspace.panes.iter().any(|pane| {
            pane.id == *id && !pane.floating && !pane.closing && tiled_ids.contains(&pane.id)
        })
    }) {
        return Some(focused);
    }
    tiled_ids.first().copied()
}

pub fn placement_for(placements: &[PanePlacement], id: PaneId) -> Option<FloatRect> {
    placements
        .iter()
        .find(|placement| placement.id == id)
        .map(|placement| placement.rect)
}

/// Painter order with a caller-supplied alert predicate, keeping layout independent from view
/// policy. In merged frames the later pane owns a shared seam: quiet < alert < focused.
pub fn ordered_panes(
    workspace: &Workspace,
    focused: Option<PaneId>,
    is_alerting: impl Fn(&Pane) -> bool,
) -> Vec<&Pane> {
    let mut panes: Vec<&Pane> = workspace.panes.iter().collect();
    panes.sort_by_key(|pane| {
        (
            pane_z_group(pane),
            pane.fullscreen,
            focused == Some(pane.id),
            is_alerting(pane),
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
/// `split_width_multiplier` for terminal cell aspect. The new pane takes the second
/// (right/bottom) slot - a fixed side, not the cursor (Hyprland's `force_split = 2`).
pub fn spawn_split_for_rect(rect: FloatRect, split_width_multiplier: f32) -> (SplitAxis, bool) {
    let axis = if rect.w >= rect.h * split_width_multiplier {
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
    top_gap: f32,
    tile_gap: TileGap,
) -> Option<(PaneId, bool)> {
    let target = {
        let placements =
            workspace_target_rects_excluding(workspace, bounds, Some(id), top_gap, tile_gap);
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
    workspace.last_move_swap = None;
    workspace.last_directional_focus = None;
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnPlacement {
    /// Split off the focused tile along `axis`. The new pane always takes the second slot, so a
    /// horizontal split puts it to the right of `target` and a vertical one below.
    Split { target: PaneId, axis: SplitAxis },
    /// Appended to the tree (first tiled pane, or no valid split target).
    Appended,
}

impl SpawnPlacement {
    /// The tile edge a pane placed this way slides in from under
    /// [`PaneAnimationStyle::Slide`](crate::anim::PaneAnimationStyle::Slide).
    ///
    /// A split reads its own axis. Order-driven layouts have no split to read, so the edge follows
    /// where the layout grows as panes are appended.
    pub fn slide_edge(self, layout_kind: LayoutKind) -> SlideEdge {
        match self {
            Self::Split {
                axis: SplitAxis::Horizontal,
                ..
            } => SlideEdge::Right,
            Self::Split {
                axis: SplitAxis::Vertical,
                ..
            } => SlideEdge::Bottom,
            Self::Appended => match layout_kind {
                // Master keeps its master tile on one side and appends to the stack beside it.
                LayoutKind::Master | LayoutKind::Columns | LayoutKind::Scrollable => {
                    SlideEdge::Right
                }
                // Grid fills row-major, so appending extends it downward. Monocle stacks panes on
                // top of each other with no neighbour at all, and Dwindle only lands here for the
                // first pane in a workspace; both take the scratchpad's default.
                LayoutKind::Grid | LayoutKind::Rows | LayoutKind::Monocle | LayoutKind::Dwindle => {
                    SlideEdge::Bottom
                }
            },
        }
    }
}

/// Insert `id` by splitting the focused pane - Hyprland's dwindle behavior: a new pane
/// always splits the currently focused one, never the tile under the cursor. The split
/// axis follows the focused tile's aspect ratio. Falls back to a plain append when there is no
/// valid split target (the first pane, or a floating focus).
pub fn place_spawned_pane(
    workspace: &mut Workspace,
    id: PaneId,
    previous_focused: Option<PaneId>,
    bounds: FloatRect,
    top_gap: f32,
    tile_gap: TileGap,
    split_width_multiplier: f32,
) -> SpawnPlacement {
    // Order-driven layouts read pane order, not split structure, so a new pane simply appends.
    // Dwindle splits the focused tile.
    if matches!(
        workspace.layout_kind,
        LayoutKind::Master
            | LayoutKind::Grid
            | LayoutKind::Columns
            | LayoutKind::Rows
            | LayoutKind::Scrollable
            | LayoutKind::Monocle
    ) {
        append_tiled_window(workspace, id);
        return SpawnPlacement::Appended;
    }

    if let Some(target) = previous_focused.filter(|target| *target != id) {
        let placements =
            workspace_target_rects_excluding(workspace, bounds, Some(id), top_gap, tile_gap);
        if let Some(rect) = placement_for(&placements, target) {
            let axis = spawn_split_for_rect(rect, split_width_multiplier).0;
            let moving_first = false;
            if insert_tiled_pane_around_target(workspace, id, target, axis, moving_first) {
                return SpawnPlacement::Split { target, axis };
            }
        }
    }

    append_tiled_window(workspace, id);
    SpawnPlacement::Appended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiling::DwindleTree;

    #[test]
    fn spawn_split_direction_follows_focused_tile_aspect() {
        // Wider than tall (after the cell-aspect multiplier) → split side by side.
        let wide = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 20.0,
        };
        assert_eq!(spawn_split_for_rect(wide, 2.3).0, SplitAxis::Horizontal);

        // Taller/narrow → split top/bottom.
        let tall = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 30.0,
        };
        assert_eq!(spawn_split_for_rect(tall, 2.3).0, SplitAxis::Vertical);

        let user_terminal = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 56.0,
        };
        assert_eq!(
            spawn_split_for_rect(user_terminal, 2.3).0,
            SplitAxis::Vertical
        );

        // The new pane always takes the second (right/bottom) slot - fixed, not cursor.
        assert!(!spawn_split_for_rect(wide, 2.3).1);
    }

    #[test]
    fn a_dwindle_split_slides_the_new_pane_in_from_the_slot_it_took() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 40.0,
        };
        let mut workspace = Workspace::new(0);
        workspace.panes.push(Pane::new(1, 100, bounds));
        place_spawned_pane(
            &mut workspace,
            1,
            None,
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
            crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER,
        );
        workspace.focused_pane = Some(1);

        workspace.panes.push(Pane::new(2, 100, bounds));
        let placement = place_spawned_pane(
            &mut workspace,
            2,
            Some(1),
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
            crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER,
        );
        // 120x40 splits side by side, and the new pane takes the right slot, so it arrives from the
        // right rather than from wherever the layout happens to grow.
        assert!(matches!(
            placement,
            SpawnPlacement::Split {
                target: 1,
                axis: SplitAxis::Horizontal
            }
        ));
        assert_eq!(
            placement.slide_edge(workspace.layout_kind),
            SlideEdge::Right
        );

        let stacked = SpawnPlacement::Split {
            target: 1,
            axis: SplitAxis::Vertical,
        };
        assert_eq!(stacked.slide_edge(LayoutKind::Dwindle), SlideEdge::Bottom);
    }

    #[test]
    fn an_appended_pane_slides_in_from_wherever_its_layout_grows() {
        for kind in [
            LayoutKind::Master,
            LayoutKind::Columns,
            LayoutKind::Scrollable,
        ] {
            assert_eq!(
                SpawnPlacement::Appended.slide_edge(kind),
                SlideEdge::Right,
                "{kind:?} appends beside the existing tiles"
            );
        }
        for kind in [
            LayoutKind::Grid,
            LayoutKind::Rows,
            LayoutKind::Monocle,
            LayoutKind::Dwindle,
        ] {
            assert_eq!(
                SpawnPlacement::Appended.slide_edge(kind),
                SlideEdge::Bottom,
                "{kind:?} appends below the existing tiles"
            );
        }
    }

    #[test]
    fn first_spawn_split_ignores_workspace_parity() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 40.0,
        };

        for index in [0, 1] {
            let mut workspace = Workspace::new(index);
            for id in 1..=2 {
                let previous_focused = workspace.focused_pane;
                workspace.panes.push(Pane::new(id, 100, bounds));
                place_spawned_pane(
                    &mut workspace,
                    id,
                    previous_focused,
                    bounds,
                    0.0,
                    crate::state::TileGap::DEFAULT,
                    crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER,
                );
                workspace.focused_pane = Some(id);
            }

            let DwindleTree::Split { axis, .. } = workspace.tile_tree.as_ref().unwrap() else {
                panic!("expected split tree");
            };
            assert_eq!(*axis, SplitAxis::Horizontal);
        }
    }

    #[test]
    fn merge_gap_overlaps_adjacent_tiles_by_one_cell() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 20.0,
        };
        let mut workspace = Workspace::new(0);
        for id in 1..=2 {
            let previous_focused = workspace.focused_pane;
            workspace.panes.push(Pane::new(id, 100, bounds));
            place_spawned_pane(
                &mut workspace,
                id,
                previous_focused,
                bounds,
                0.0,
                crate::state::TileGap::DEFAULT,
                crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER,
            );
            workspace.focused_pane = Some(id);
        }

        // The wide bounds split left|right. With the border-merge overlap gap (-1) the right
        // pane's left edge lands one cell left of the left pane's right edge, so their borders
        // share a column and the union still spans the tile bounds exactly.
        let mut rects: Vec<FloatRect> = workspace_target_rects(
            &workspace,
            bounds,
            0.0,
            TileGap {
                horizontal: -1.0,
                vertical: 0.0,
            },
        )
        .into_iter()
        .map(|placement| placement.rect)
        .collect();
        rects.sort_by(|a, b| a.x.total_cmp(&b.x));
        let [left, right] = rects.as_slice() else {
            panic!("expected two tiled placements, got {}", rects.len());
        };
        assert_eq!(right.x, left.x + left.w - 1.0, "panes overlap by one cell");
        assert_eq!(left.x, 0.0);
        assert_eq!(right.x + right.w, bounds.w, "union spans the bounds");
    }

    #[test]
    fn columns_and_scrollable_spawns_append_not_split() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 40.0,
        };
        for kind in [LayoutKind::Columns, LayoutKind::Scrollable] {
            let mut workspace = Workspace::new(0);
            workspace.layout_kind = kind;
            for id in 1..=3 {
                let previous_focused = workspace.focused_pane;
                workspace.panes.push(Pane::new(id, 100, bounds));
                let placement = place_spawned_pane(
                    &mut workspace,
                    id,
                    previous_focused,
                    bounds,
                    0.0,
                    crate::state::TileGap::DEFAULT,
                    crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER,
                );
                assert_eq!(placement, SpawnPlacement::Appended);
                workspace.focused_pane = Some(id);
            }
            assert_eq!(workspace.tiled_ids(), vec![1, 2, 3]);
        }
    }

    #[test]
    fn scrollable_anchor_fallback_uses_local_anchor_when_focus_is_floating() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let mut workspace = Workspace::new(0);
        workspace.layout_kind = LayoutKind::Scrollable;
        for id in 1..=3 {
            workspace.panes.push(Pane::new(id, 100, bounds));
            append_tiled_window(&mut workspace, id);
        }
        workspace.panes.push({
            let mut floating = Pane::new(4, 100, bounds);
            floating.floating = true;
            floating
        });
        workspace.focused_pane = Some(4);
        workspace.scrollable_anchor = Some(3);
        workspace.scrollable_reveal_edge = crate::state::ScrollableRevealEdge::Right;

        let placements =
            workspace_target_rects(&workspace, bounds, 0.0, crate::state::TileGap::DEFAULT);
        let anchored = placements.iter().find(|p| p.id == 3).unwrap();
        assert!(
            anchored.rect.x >= bounds.x - f32::EPSILON
                && anchored.rect.x + anchored.rect.w <= bounds.x + bounds.w + f32::EPSILON,
            "floating focus keeps the tiled scroll anchor fully visible"
        );
        assert!(
            (anchored.rect.x + anchored.rect.w - (bounds.x + bounds.w)).abs() < f32::EPSILON,
            "right reveal edge keeps later tiled anchors right-aligned under floating focus"
        );

        workspace.scrollable_anchor = Some(99);
        let fallback = scrollable_viewport_anchor(&workspace, &workspace.tiled_ids());
        assert_eq!(fallback, Some(1));
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

        let ids: Vec<PaneId> = ordered_panes(&workspace, Some(2), |_| false)
            .into_iter()
            .map(|pane| pane.id)
            .collect();

        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn ordered_panes_paints_alerts_above_quiet_peers_below_focus() {
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        let mut workspace = Workspace::new(0);
        workspace.panes = vec![Pane::new(1, 100, rect), Pane::new(2, 100, rect)];

        let ids: Vec<_> = ordered_panes(&workspace, Some(1), |pane| pane.id == 2)
            .into_iter()
            .map(|pane| pane.id)
            .collect();
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn scrollable_wider_local_visible_keeps_canonical_centering() {
        // Canonical letterbox centered in a wider local viewport: intersection == canonical tile.
        let canonical = FloatRect {
            x: 25.0,
            y: 0.0,
            w: 100.0,
            h: 28.0,
        };
        let local = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 150.0,
            h: 28.0,
        };
        let mut workspace = Workspace::new(0);
        workspace.layout_kind = LayoutKind::Scrollable;
        for id in [1, 2] {
            let mut pane = Pane::new(id, 100, canonical);
            pane.scrollable_width = 0.20;
            workspace.panes.push(pane);
            append_tiled_window(&mut workspace, id);
        }
        workspace.scrollable_anchor = Some(1);
        workspace.focused_pane = Some(1);
        // Three panes keep preferred widths so the strip stays short of the canonical tile.
        workspace.panes.push({
            let mut pane = Pane::new(3, 100, canonical);
            pane.scrollable_width = 0.20;
            pane
        });
        append_tiled_window(&mut workspace, 3);

        let with_visible = workspace_target_rects_with_visible_bounds(
            &workspace,
            canonical,
            local,
            0.0,
            TileGap::DEFAULT,
        );
        let canonical_only = workspace_target_rects(&workspace, canonical, 0.0, TileGap::DEFAULT);
        assert_eq!(with_visible.len(), canonical_only.len());
        for (got, expected) in with_visible.iter().zip(canonical_only.iter()) {
            assert_eq!(got.id, expected.id);
            assert!(
                (got.rect.x - expected.rect.x).abs() < 1e-5,
                "wider local must not shift strip: got {} expected {}",
                got.rect.x,
                expected.rect.x
            );
            assert!(got.rect.x >= canonical.x - 0.5);
            assert!(got.rect.x + got.rect.w <= canonical.x + canonical.w + 0.5);
        }
        assert!(
            (with_visible[0].rect.x - canonical.x).abs() < 1e-5,
            "short strip stays left-aligned to canonical tile, not local edge"
        );

        assert!(
            horizontal_tile_intersection(
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0
                },
                FloatRect {
                    x: 20.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0
                },
            )
            .is_none()
        );
    }
}
