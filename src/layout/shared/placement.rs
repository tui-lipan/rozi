//! Reading placements out of a shared layout document: the tile source the layout engine measures
//! it through, canvas bounds and scripted float rects. Depends on the document alone.

use super::*;

/// Rebuild a [`DwindleTree`] from a shared tree, keeping only leaves whose pane id is present
/// locally (`known`). Returns `None` if nothing survives, so the caller can rebuild from order.
pub(crate) fn dwindle_from_shared(
    tree: &SharedTree,
    known: &std::collections::HashSet<PaneId>,
) -> Option<DwindleTree> {
    to_dwindle(tree, &|pane| known.contains(pane).then_some(*pane), true)
}

/// One [`SharedWorkspace`] read as tiling input, measured against a canonical canvas.
///
/// The shared document is what a session server has to go on when nobody is attached, so this is
/// how the server places panes: the same allocators, fed from the document instead of a client's
/// live `Workspace`. Client-local view state the document deliberately omits falls back to what a
/// freshly attached client would use - a Scrollable strip scrolled to its first column.
pub(crate) struct SharedTileSource<'a> {
    workspace: &'a SharedWorkspace,
    tree: Option<DwindleTree>,
    canvas: (u16, u16),
}

impl<'a> SharedTileSource<'a> {
    pub(crate) fn new(workspace: &'a SharedWorkspace, canvas: (u16, u16)) -> Self {
        let known = workspace
            .panes
            .iter()
            .map(|pane| pane.pane_id)
            .collect::<std::collections::HashSet<_>>();
        Self {
            workspace,
            tree: workspace
                .tree
                .as_ref()
                .and_then(|tree| dwindle_from_shared(tree, &known)),
            canvas,
        }
    }
}

impl crate::layout::TileSource for SharedTileSource<'_> {
    fn layout_kind(&self) -> crate::layout::model::LayoutKind {
        self.workspace.layout.into()
    }

    fn start_axis(&self) -> crate::layout::model::SplitAxis {
        self.workspace.start_axis.into()
    }

    fn split_ratios(&self) -> &[f32] {
        &self.workspace.split_ratios
    }

    fn stored_tile_tree(&self) -> Option<&DwindleTree> {
        self.tree.as_ref()
    }

    fn tiled_ids_by_pane_order(&self) -> Vec<PaneId> {
        self.workspace
            .panes
            .iter()
            .filter(|pane| !pane.floating)
            .map(|pane| pane.pane_id)
            .collect()
    }

    fn scrollable_width(&self, id: PaneId) -> f32 {
        self.workspace
            .panes
            .iter()
            .find(|pane| pane.pane_id == id)
            .map_or(crate::layout::model::DEFAULT_SCROLLABLE_WIDTH, |pane| {
                pane.scrollable_width
            })
    }

    fn scrollable_viewport(
        &self,
        tiled_ids: &[PaneId],
    ) -> (Option<PaneId>, crate::layout::model::ScrollableRevealEdge) {
        (
            tiled_ids.first().copied(),
            crate::layout::model::ScrollableRevealEdge::Left,
        )
    }

    fn for_each_floating(&self, visit: &mut dyn FnMut(PaneId, FloatRect)) {
        let (cols, rows) = self.canvas;
        for pane in self.workspace.panes.iter().filter(|pane| pane.floating) {
            if let Some(rect) = pane.rect {
                visit(pane.pane_id, frac_rect_to_float(rect, cols, rows));
            }
        }
    }
}

/// Where each pane of `workspace` sits on the document's canonical canvas, in cells.
///
/// Gap-free and chrome-free on purpose. This is the canonical arrangement automation edits;
/// rendering adds the controller's shared pane geometry. The arrangement stays the same
/// whichever client - or none - is looking at it. Scrollable columns may extend past the canvas
/// edge, because the strip is wider than the viewport that scrolls over it.
pub(crate) fn shared_workspace_placements(
    workspace: &SharedWorkspace,
    canvas: (u16, u16),
) -> Vec<crate::layout::tiling::PanePlacement> {
    let bounds = FloatRect {
        x: 0.0,
        y: 0.0,
        w: f32::from(canvas.0.max(1)),
        h: f32::from(canvas.1.max(1)),
    };
    crate::layout::workspace_target_rects_excluding_with_visible_and_float_bounds(
        &SharedTileSource::new(workspace, canvas),
        bounds,
        bounds,
        None,
        None,
        0.0,
        crate::layout::model::TileGap {
            horizontal: 0.0,
            vertical: 0.0,
        },
    )
}

/// The canonical canvas as a rect, for measuring and clamping against it.
pub(crate) fn canvas_bounds(canvas: (u16, u16)) -> FloatRect {
    FloatRect {
        x: 0.0,
        y: 0.0,
        w: f32::from(canvas.0.max(1)),
        h: f32::from(canvas.1.max(1)),
    }
}

/// Where automation floats `pane_id`, in canonical-canvas cells.
///
/// A requested rect is clamped the way a dragged float is, which lets it hang partly off the canvas
/// as long as a grab margin stays on it. Without one, a pane that already floats stays where it is,
/// and a tiled pane lifts off centred on the tile it leaves, at the default floating size. New
/// rects land on whole cells, since that is all a terminal can draw. Both endpoints resolve the rect here from the shared document, so the same
/// request floats a pane to the same place whether a UI or a session server applied it.
pub(crate) fn automation_float_rect(
    workspace: &SharedWorkspace,
    canvas: (u16, u16),
    pane_id: PaneId,
    requested: Option<FloatRect>,
) -> Option<FloatRect> {
    use crate::layout::geometry::{
        clamp_floating_rect, default_floating_rect, lift_off_float_rect,
    };

    let bounds = canvas_bounds(canvas);
    let whole_cells = |rect: FloatRect| FloatRect {
        x: rect.x.round(),
        y: rect.y.round(),
        w: rect.w.round(),
        h: rect.h.round(),
    };
    if let Some(requested) = requested {
        return Some(whole_cells(clamp_floating_rect(requested, bounds)));
    }
    let pane = workspace
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)?;
    if pane.floating {
        return pane
            .rect
            .map(|rect| frac_rect_to_float(rect, canvas.0, canvas.1));
    }
    let tile =
        crate::layout::placement_for(&shared_workspace_placements(workspace, canvas), pane_id)?;
    Some(whole_cells(lift_off_float_rect(
        tile,
        default_floating_rect(bounds, 0),
        bounds,
    )))
}
