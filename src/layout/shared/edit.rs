//! Absolute edits to a shared layout document, shared by the server and the live-workspace bridge.

use super::*;

/// Why a shared-document edit was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SharedEditError {
    PaneNotPlaced(PaneId),
    /// A swap needs two different tiled panes in one workspace.
    NotSwappable(PaneId, PaneId),
    InvalidDocument(SharedLayoutValidationError),
}

impl std::fmt::Display for SharedEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PaneNotPlaced(id) => write!(f, "pane {id} is not in the layout"),
            Self::NotSwappable(a, b) => write!(
                f,
                "panes {a} and {b} cannot swap; a swap exchanges two different tiled panes of one workspace"
            ),
            Self::InvalidDocument(error) => write!(f, "{error}"),
        }
    }
}

impl SharedLayout {
    /// The workspace holding `pane_id`, by position in [`Self::workspaces`].
    pub(crate) fn workspace_position_of(&self, pane_id: PaneId) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| workspace.panes.iter().any(|pane| pane.pane_id == pane_id))
    }

    /// Set a workspace's layout kind; `index` is zero-based. A workspace the document does not
    /// hold yet is added, empty, so the choice is recorded for when panes arrive. Returns whether
    /// the document changed.
    pub(crate) fn set_layout_kind(
        &mut self,
        index: usize,
        kind: crate::layout::model::LayoutKind,
    ) -> bool {
        let kind = SharedLayoutKind::from(kind);
        let exists = self
            .workspaces
            .iter()
            .any(|workspace| workspace.index == index);
        let workspace = self.workspace_mut_or_insert(index);
        if exists && workspace.layout == kind {
            return false;
        }
        workspace.layout = kind;
        true
    }

    /// The layout kind of the workspace at zero-based `index`; a workspace the document does not
    /// hold yet reads as the kind it would be added with.
    pub(crate) fn layout_kind_of(&self, index: usize) -> crate::layout::model::LayoutKind {
        self.workspaces
            .iter()
            .find(|workspace| workspace.index == index)
            .map_or_else(
                || crate::layout::model::LayoutKind::DEFAULT,
                |workspace| workspace.layout.into(),
            )
    }

    /// Set the master pane's share of the workspace at zero-based `index`. Returns whether the
    /// document changed.
    pub(crate) fn set_master_ratio(&mut self, index: usize, ratio: f32) -> bool {
        set_master_share(&mut self.workspace_mut_or_insert(index).split_ratios, ratio)
    }

    /// The workspace at zero-based `index`, added empty with a fresh workspace's defaults when the
    /// document does not hold it yet.
    fn workspace_mut_or_insert(&mut self, index: usize) -> &mut SharedWorkspace {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.index == index)
        {
            self.workspaces.push(SharedWorkspace {
                index,
                name: None,
                synchronized: false,
                layout: crate::layout::model::LayoutKind::DEFAULT.into(),
                start_axis: crate::layout::model::SplitAxis::start_for_workspace(index).into(),
                split_ratios: crate::layout::model::LayoutKind::default_split_ratios(),
                tree: None,
                panes: Vec::new(),
            });
            self.workspaces.sort_by_key(|workspace| workspace.index);
        }
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.index == index)
            .expect("inserted above")
    }

    /// Store the tree the layout engine would settle this workspace's live tiled panes into.
    ///
    /// The one normalization every structural edit ends with, on both endpoints: a client applying
    /// the same edit settles its live tree the same way before committing, which is what makes the
    /// two documents equal.
    fn settle_tree(&mut self, position: usize) {
        let canvas = (self.canvas_cols.max(1), self.canvas_rows.max(1));
        let workspace = &self.workspaces[position];
        let tree =
            crate::layout::effective_tile_tree(&SharedTileSource::new(workspace, canvas), None)
                .as_ref()
                .and_then(|tree| from_dwindle(tree, &|id| Some(id)));
        self.workspaces[position].tree = tree;
    }

    /// Move `pane_id` to the workspace at zero-based `target`, at the end of its pane list and,
    /// when tiled, at the end of its tiling order. A floating pane keeps its rect and a pinned one
    /// is unpinned. Returns whether the document changed.
    pub(crate) fn move_pane(
        &mut self,
        pane_id: PaneId,
        target: usize,
    ) -> std::result::Result<bool, SharedEditError> {
        // Sending a pinned pane to another workspace settles it there.
        let leaves = self
            .workspace_position_of(pane_id)
            .is_some_and(|position| self.workspaces[position].index != target);
        let unpinned = leaves && self.set_pinned(pane_id, false);
        let changed = self.move_pane_fields(pane_id, target)? || unpinned;
        self.validate().map_err(SharedEditError::InvalidDocument)?;
        Ok(changed)
    }

    pub(super) fn move_pane_fields(
        &mut self,
        pane_id: PaneId,
        target: usize,
    ) -> std::result::Result<bool, SharedEditError> {
        let source = self
            .workspace_position_of(pane_id)
            .ok_or(SharedEditError::PaneNotPlaced(pane_id))?;
        if self.workspaces[source].index == target {
            return Ok(false);
        }
        let panes = &mut self.workspaces[source].panes;
        let pane = panes.remove(
            panes
                .iter()
                .position(|pane| pane.pane_id == pane_id)
                .expect("located above"),
        );
        self.settle_tree(source);
        let floating = pane.floating;
        self.workspace_mut_or_insert(target);
        let destination = self
            .workspaces
            .iter()
            .position(|workspace| workspace.index == target)
            .expect("inserted above");
        if !floating {
            // Settled before the pane arrives, then extended by it: the order a client's
            // `append_tiled_window` onto its settled tree produces.
            self.settle_tree(destination);
            let canvas = (self.canvas_cols.max(1), self.canvas_rows.max(1));
            let workspace = &self.workspaces[destination];
            let settled = SharedTileSource::new(workspace, canvas);
            let tree = crate::layout::tiling::append_tiled_leaf(
                crate::layout::TileSource::stored_tile_tree(&settled).cloned(),
                pane_id,
                workspace.start_axis.into(),
            );
            self.workspaces[destination].tree = from_dwindle(&tree, &|id| Some(id));
        }
        let fullscreen = pane.fullscreen;
        self.workspaces[destination].panes.push(pane);
        if fullscreen {
            clear_other_shared_fullscreen(&mut self.workspaces[destination], pane_id);
        }
        Ok(true)
    }

    /// Exchange the places of two tiled panes in one workspace.
    pub(crate) fn swap_panes(
        &mut self,
        pane_id: PaneId,
        other: PaneId,
    ) -> std::result::Result<bool, SharedEditError> {
        let changed = self.swap_pane_fields(pane_id, other)?;
        self.validate().map_err(SharedEditError::InvalidDocument)?;
        Ok(changed)
    }

    pub(super) fn swap_pane_fields(
        &mut self,
        pane_id: PaneId,
        other: PaneId,
    ) -> std::result::Result<bool, SharedEditError> {
        let position = self
            .workspace_position_of(pane_id)
            .ok_or(SharedEditError::PaneNotPlaced(pane_id))?;
        let other_position = self
            .workspace_position_of(other)
            .ok_or(SharedEditError::PaneNotPlaced(other))?;
        let tiled = |id: PaneId| {
            self.workspaces[position]
                .panes
                .iter()
                .any(|pane| pane.pane_id == id && !pane.floating)
        };
        if pane_id == other || position != other_position || !tiled(pane_id) || !tiled(other) {
            return Err(SharedEditError::NotSwappable(pane_id, other));
        }
        self.settle_tree(position);
        let known = self.workspaces[position]
            .panes
            .iter()
            .map(|pane| pane.pane_id)
            .collect();
        let Some(mut tree) = self.workspaces[position]
            .tree
            .as_ref()
            .and_then(|tree| dwindle_from_shared(tree, &known))
        else {
            return Err(SharedEditError::NotSwappable(pane_id, other));
        };
        crate::layout::tiling::swap_tree_leaves(&mut tree, pane_id, other);
        self.workspaces[position].tree = from_dwindle(&tree, &|id| Some(id));
        Ok(true)
    }

    /// Restrict a startup publication to panes whose spawn has been submitted.
    pub(crate) fn retain_panes(&mut self, known: &std::collections::HashSet<PaneId>) {
        for position in 0..self.workspaces.len() {
            let workspace = &mut self.workspaces[position];
            let count = workspace.panes.len();
            workspace.panes.retain(|pane| known.contains(&pane.pane_id));
            if count != workspace.panes.len() {
                self.settle_tree(position);
            }
        }
    }

    /// Drop `pane_id` from the document, re-tiling what it leaves behind. Returns the zero-based
    /// index of the workspace it left, or `None` when the document never placed it.
    pub(crate) fn remove_pane(&mut self, pane_id: PaneId) -> Option<usize> {
        let position = self.workspace_position_of(pane_id)?;
        self.workspaces[position]
            .panes
            .retain(|pane| pane.pane_id != pane_id);
        self.settle_tree(position);
        Some(self.workspaces[position].index)
    }

    /// Set an explicit float placement, or append a returning pane to the tiling. Geometry
    /// selection belongs to the caller, so this also serves interactive pointer operations.
    pub(crate) fn set_float_placement(&mut self, id: PaneId, rect: Option<FloatRect>) -> bool {
        let Some(position) = self.workspace_position_of(id) else {
            return false;
        };
        let pane = self.workspaces[position]
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == id)
            .expect("located above");
        let floating = rect.is_some();
        let rect = rect.map(|rect| float_rect_to_frac(rect, self.canvas_cols, self.canvas_rows));
        let changed = pane.floating != floating || pane.rect != rect;
        let retile = pane.floating != floating;
        pane.floating = floating;
        // A pin only means something for a float, so returning to the tiling drops it.
        pane.pinned &= floating;
        pane.rect = rect;
        if retile {
            self.settle_tree(position);
        }
        changed
    }

    /// Pin or unpin pane `id`. Only a floating pane can be pinned, and pinning ends fullscreen.
    /// Returns whether it changed.
    pub(crate) fn set_pinned(&mut self, id: PaneId, pinned: bool) -> bool {
        let Some(pane) = self
            .workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.panes.iter_mut())
            .find(|pane| pane.pane_id == id)
        else {
            return false;
        };
        let pinned = pinned && pane.floating;
        let changed = pane.pinned != pinned || (pinned && pane.fullscreen);
        pane.pinned = pinned;
        // A pinned pane rides over every workspace, so it never covers one.
        pane.fullscreen &= !pinned;
        changed
    }

    /// Apply a validated `pane set` to the document. Returns whether it changed.
    ///
    /// Edits in place and validates afterwards, so a caller that must not be left holding a
    /// refused edit applies it to a copy - which is what committing a new revision does anyway.
    ///
    /// Floating removes the pane from the tiling tree; returning to the tiling appends it at the
    /// end of the tiling order - exactly what [`effective_tile_tree`](crate::layout::effective_tile_tree)
    /// does for a pane the tree does not name, which is how a client applying the same edit to its
    /// live workspace arrives at the same tree.
    pub(crate) fn edit_pane(
        &mut self,
        pane_id: PaneId,
        edit: crate::control::PaneEdit,
    ) -> std::result::Result<bool, SharedEditError> {
        let changed = self.edit_pane_fields(pane_id, edit)?;
        self.validate().map_err(SharedEditError::InvalidDocument)?;
        Ok(changed)
    }

    pub(super) fn edit_pane_fields(
        &mut self,
        pane_id: PaneId,
        edit: crate::control::PaneEdit,
    ) -> std::result::Result<bool, SharedEditError> {
        let canvas = (self.canvas_cols.max(1), self.canvas_rows.max(1));
        let position = self
            .workspace_position_of(pane_id)
            .ok_or(SharedEditError::PaneNotPlaced(pane_id))?;
        let before = self.workspaces[position].clone();
        let floating_now = before
            .panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .is_some_and(|pane| pane.floating);
        let floats = edit.floats(floating_now);
        // Resolved against the document as it stands, so a lifting pane is centred on the tile it
        // is leaving rather than on wherever the reflowed tiling puts its neighbours.
        let float_rect = if floats {
            automation_float_rect(
                &before,
                canvas,
                pane_id,
                edit.rect.map(|rect| rect.to_cells(canvas)),
            )
        } else {
            None
        };

        if !floats {
            self.set_float_placement(pane_id, None);
        } else if (edit.rect.is_some() || !floating_now)
            && let Some(rect) = float_rect
        {
            self.set_float_placement(pane_id, Some(rect));
        }
        let workspace = &mut self.workspaces[position];
        let pane = workspace
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == pane_id)
            .expect("located above");
        if let Some(fullscreen) = edit.fullscreen {
            pane.fullscreen = fullscreen;
            pane.pinned &= !fullscreen;
        }
        if edit.fullscreen == Some(true) {
            clear_other_shared_fullscreen(workspace, pane_id);
        }
        apply_shared_pane_sizes(workspace, canvas, pane_id, edit);
        let changed = *workspace != before;
        Ok(changed)
    }

    /// Whether `pane_id` shares a split in its workspace's settled tree - whether a split ratio
    /// has anything to size.
    pub(crate) fn pane_in_split(&self, pane_id: PaneId) -> bool {
        let canvas = (self.canvas_cols.max(1), self.canvas_rows.max(1));
        self.workspace_position_of(pane_id).is_some_and(|position| {
            crate::layout::effective_tile_tree(
                &SharedTileSource::new(&self.workspaces[position], canvas),
                None,
            )
            .is_some_and(|tree| crate::layout::tiling::leaf_share(&tree, pane_id).is_some())
        })
    }
}

fn apply_shared_pane_sizes(
    workspace: &mut SharedWorkspace,
    canvas: (u16, u16),
    pane_id: PaneId,
    edit: crate::control::PaneEdit,
) {
    if let Some(width) = edit.width_ratio
        && let Some(pane) = workspace
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == pane_id)
    {
        pane.scrollable_width = width;
    }
    if let Some(share) = edit.split_ratio {
        let tree =
            crate::layout::effective_tile_tree(&SharedTileSource::new(workspace, canvas), None)
                .map(|mut tree| {
                    crate::layout::tiling::set_leaf_share(&mut tree, pane_id, share);
                    tree
                });
        workspace.tree = tree
            .as_ref()
            .and_then(|tree| from_dwindle(tree, &|id| Some(id)));
    }
}

/// Restore every pane of `workspace` but `keep` from fullscreen: at most one pane per workspace is
/// fullscreen. The document side of `clear_other_fullscreen`, so both endpoints keep the rule.
fn clear_other_shared_fullscreen(workspace: &mut SharedWorkspace, keep: PaneId) {
    for other in workspace
        .panes
        .iter_mut()
        .filter(|pane| pane.pane_id != keep)
    {
        other.fullscreen = false;
    }
}

/// Set a workspace's master share, which `allocate_master` reads from the first split ratio. Shared
/// by the document edit and the live one, so both store the same ratios.
pub(crate) fn set_master_share(split_ratios: &mut Vec<f32>, ratio: f32) -> bool {
    if split_ratios
        .first()
        .copied()
        .unwrap_or(crate::layout::model::DEFAULT_RATIO)
        == ratio
    {
        return false;
    }
    if let Some(stored) = split_ratios.first_mut() {
        *stored = ratio;
    } else {
        split_ratios.push(ratio);
    }
    true
}

/// Express a canvas-cell rect as the canvas fractions a [`SharedPane`] stores.
///
/// The inverse of [`frac_rect_to_float`]. Both directions are needed by more than one caller now:
/// a client converts its own float on the way onto the wire, and the session server converts the
/// rect a `[[rules]]` float resolves to when it places a headless pane.
pub(crate) fn float_rect_to_frac(rect: FloatRect, canvas_cols: u16, canvas_rows: u16) -> FracRect {
    let cols = f32::from(canvas_cols.max(1));
    let rows = f32::from(canvas_rows.max(1));
    FracRect {
        x: rect.x / cols,
        y: rect.y / rows,
        w: rect.w / cols,
        h: rect.h / rows,
    }
}

pub(crate) fn frac_rect_to_float(rect: FracRect, canvas_cols: u16, canvas_rows: u16) -> FloatRect {
    FloatRect {
        x: rect.x * f32::from(canvas_cols.max(1)),
        y: rect.y * f32::from(canvas_rows.max(1)),
        w: rect.w * f32::from(canvas_cols.max(1)),
        h: rect.h * f32::from(canvas_rows.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::layout::model::LayoutKind;
    use crate::state::{Pane, State};

    /// Three tiled panes in workspace 1 of a 100×30 document.
    fn three_tiled() -> SharedLayout {
        let mut state = State::new(Config::default(), Theme::default());
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        workspace.tile_tree = None;
        for id in 1..=3 {
            let mut pane = Pane::new(id, 100, FloatRect::default());
            pane.pty_generation = 1;
            workspace.panes.push(pane);
            crate::layout::tiling::append_tiled_window(workspace, id);
        }
        shared_layout_from_state(&state, (100, 30))
    }

    fn edit(
        floating: Option<bool>,
        fullscreen: Option<bool>,
        rect: Option<crate::control::CellRect>,
    ) -> crate::control::PaneEdit {
        crate::control::PaneEdit::validate(floating, fullscreen, rect, None, None, None)
            .expect("valid edit")
    }

    fn placement(layout: &SharedLayout, id: PaneId) -> FloatRect {
        crate::layout::placement_for(
            &shared_workspace_placements(&layout.workspaces[0], (100, 30)),
            id,
        )
        .expect("placed")
    }

    #[test]
    fn a_pinned_pane_is_never_fullscreen_and_a_scripted_move_unpins_it() {
        let pane = |layout: &SharedLayout, id: PaneId| {
            layout
                .workspaces
                .iter()
                .flat_map(|workspace| &workspace.panes)
                .find(|pane| pane.pane_id == id)
                .cloned()
                .expect("placed")
        };
        let mut layout = three_tiled();
        assert!(!layout.set_pinned(2, true), "a tiled pane cannot be pinned");

        layout
            .edit_pane(2, edit(Some(true), Some(true), None))
            .unwrap();
        assert!(layout.set_pinned(2, true));
        assert!(pane(&layout, 2).pinned);
        assert!(!pane(&layout, 2).fullscreen, "pinning ends fullscreen");

        layout.edit_pane(2, edit(None, Some(true), None)).unwrap();
        assert!(pane(&layout, 2).fullscreen);
        assert!(!pane(&layout, 2).pinned, "fullscreen unpins");

        layout.edit_pane(2, edit(None, Some(false), None)).unwrap();
        layout.set_pinned(2, true);
        assert_eq!(layout.move_pane(2, 3), Ok(true));
        assert!(!pane(&layout, 2).pinned, "a scripted move settles the pane");
        assert!(pane(&layout, 2).floating);
    }

    #[test]
    fn floating_a_tiled_pane_lifts_it_off_its_tile_and_repeating_it_changes_nothing() {
        let mut layout = three_tiled();
        let tile = placement(&layout, 2);

        assert_eq!(layout.edit_pane(2, edit(Some(true), None, None)), Ok(true));
        let pane = &layout.workspaces[0].panes[1];
        assert!(pane.floating);
        let rect = placement(&layout, 2);
        // The default float size, centred on the tile it left.
        assert_eq!(
            (rect.w, rect.h),
            (42.0, 13.0),
            "42% of the canvas, in whole cells"
        );
        assert!((rect.x + rect.w / 2.0 - (tile.x + tile.w / 2.0)).abs() <= 0.5);
        assert!((rect.y + rect.h / 2.0 - (tile.y + tile.h / 2.0)).abs() <= 0.5);
        // Out of the tree; the other two re-tile across the space.
        let mut leaves = Vec::new();
        let tree = dwindle_from_shared(
            layout.workspaces[0].tree.as_ref().expect("tree"),
            &[1, 3].into_iter().collect(),
        )
        .expect("tree");
        crate::layout::tiling::collect_tree_leaves(&tree, &mut leaves);
        assert_eq!(leaves, vec![1, 3]);

        let settled = layout.clone();
        assert_eq!(layout.edit_pane(2, edit(Some(true), None, None)), Ok(false));
        assert_eq!(
            layout, settled,
            "a float that stays put keeps its exact fractions"
        );
    }

    #[test]
    fn a_requested_rect_is_clamped_and_retiling_appends_to_the_tiling_order() {
        let mut layout = three_tiled();
        let rect = crate::control::CellRect {
            x: 400,
            y: 5,
            width: 40,
            height: 10,
        };
        assert_eq!(
            layout.edit_pane(1, edit(Some(true), None, Some(rect))),
            Ok(true)
        );
        let placed = placement(&layout, 1);
        assert_eq!((placed.w, placed.h), (40.0, 10.0));
        assert!(
            placed.x < 100.0,
            "clamped so part of the float stays on the canvas to grab"
        );
        assert_eq!(
            layout.edit_pane(1, edit(None, None, Some(rect))),
            Ok(false),
            "the same rect again is no change"
        );

        assert_eq!(layout.edit_pane(1, edit(Some(false), None, None)), Ok(true));
        assert!(layout.workspaces[0].panes[0].rect.is_none());
        let order = crate::layout::ordered_tiled_ids(&SharedTileSource::new(
            &layout.workspaces[0],
            (100, 30),
        ));
        assert_eq!(order, vec![2, 3, 1], "a re-tiled pane joins at the end");
        layout.validate().expect("the edited document is valid");
    }

    #[test]
    fn fullscreen_and_layout_kind_are_set_absolutely() {
        let mut layout = three_tiled();
        assert_eq!(layout.edit_pane(3, edit(None, Some(true), None)), Ok(true));
        assert_eq!(layout.edit_pane(3, edit(None, Some(true), None)), Ok(false));
        assert!(layout.workspaces[0].panes[2].fullscreen);

        assert!(layout.set_layout_kind(0, LayoutKind::Grid));
        assert!(!layout.set_layout_kind(0, LayoutKind::Grid));

        // A workspace the document lacks is added, so the choice survives until panes arrive.
        let mut partial = three_tiled();
        partial.workspaces.retain(|workspace| workspace.index == 0);
        assert!(partial.set_layout_kind(4, LayoutKind::Monocle));
        assert_eq!(
            partial
                .workspaces
                .iter()
                .map(|workspace| workspace.index)
                .collect::<Vec<_>>(),
            vec![0, 4]
        );
        partial.validate().expect("valid");

        assert_eq!(
            layout.edit_pane(42, edit(None, Some(true), None)),
            Err(SharedEditError::PaneNotPlaced(42))
        );
    }

    fn tiling_order(layout: &SharedLayout, index: usize) -> Vec<PaneId> {
        let workspace = layout
            .workspaces
            .iter()
            .find(|workspace| workspace.index == index)
            .expect("workspace");
        crate::layout::ordered_tiled_ids(&SharedTileSource::new(workspace, (100, 30)))
    }

    #[test]
    fn moving_a_pane_appends_it_to_the_target_and_retiles_what_it_left() {
        let mut layout = three_tiled();
        assert_eq!(layout.move_pane(2, 0), Ok(false), "already there");

        assert_eq!(layout.move_pane(2, 3), Ok(true));
        assert_eq!(tiling_order(&layout, 0), vec![1, 3]);
        assert_eq!(tiling_order(&layout, 3), vec![2]);
        assert_eq!(layout.move_pane(3, 3), Ok(true));
        assert_eq!(tiling_order(&layout, 3), vec![2, 3], "joins the end");

        // A float travels with its rect and stays out of the target's tiling.
        assert_eq!(layout.edit_pane(1, edit(Some(true), None, None)), Ok(true));
        let rect = layout.workspaces[0].panes[0].rect;
        assert_eq!(layout.move_pane(1, 3), Ok(true));
        let moved = layout.workspaces[3]
            .panes
            .iter()
            .find(|pane| pane.pane_id == 1)
            .expect("moved");
        assert_eq!(moved.rect, rect);
        assert_eq!(tiling_order(&layout, 3), vec![2, 3]);

        // A workspace the document lacks is created for the pane.
        let mut partial = three_tiled();
        partial.workspaces.retain(|workspace| workspace.index == 0);
        assert_eq!(partial.move_pane(3, 6), Ok(true));
        assert_eq!(tiling_order(&partial, 6), vec![3]);
        partial.validate().expect("valid");
        assert_eq!(
            partial.move_pane(9, 1),
            Err(SharedEditError::PaneNotPlaced(9))
        );
    }

    #[test]
    fn swapping_exchanges_two_tiled_panes_and_refuses_anything_else() {
        let mut layout = three_tiled();
        let before = (placement(&layout, 1), placement(&layout, 3));
        assert_eq!(layout.swap_panes(1, 3), Ok(true));
        assert_eq!(tiling_order(&layout, 0), vec![3, 2, 1]);
        assert_eq!((placement(&layout, 3), placement(&layout, 1)), before);

        for (a, b) in [(1, 1), (1, 42)] {
            assert!(layout.swap_panes(a, b).is_err(), "{a} with {b}");
        }
        assert_eq!(layout.edit_pane(2, edit(Some(true), None, None)), Ok(true));
        assert_eq!(
            layout.swap_panes(1, 2),
            Err(SharedEditError::NotSwappable(1, 2)),
            "a floating pane has no tile to trade"
        );
        assert_eq!(layout.move_pane(3, 1), Ok(true));
        assert_eq!(
            layout.swap_panes(1, 3),
            Err(SharedEditError::NotSwappable(1, 3)),
            "panes in different workspaces"
        );
    }

    fn sizes(split_ratio: Option<f64>, width_ratio: Option<f64>) -> crate::control::PaneEdit {
        crate::control::PaneEdit::validate(None, None, None, None, split_ratio, width_ratio)
            .expect("valid edit")
    }

    #[test]
    fn ratios_are_set_absolutely_on_the_document() {
        let mut layout = three_tiled();
        assert!(layout.pane_in_split(3));
        assert_eq!(layout.edit_pane(3, sizes(Some(0.7), None)), Ok(true));
        assert_eq!(layout.edit_pane(3, sizes(Some(0.7), None)), Ok(false));
        let known = [1, 2, 3].into_iter().collect();
        let tree =
            dwindle_from_shared(layout.workspaces[0].tree.as_ref().unwrap(), &known).unwrap();
        assert!((crate::layout::tiling::leaf_share(&tree, 3).unwrap() - 0.7).abs() < 1e-6);

        assert_eq!(layout.edit_pane(2, sizes(None, Some(0.35))), Ok(true));
        assert_eq!(layout.workspaces[0].panes[1].scrollable_width, 0.35);

        assert!(layout.set_master_ratio(0, 0.65));
        assert!(!layout.set_master_ratio(0, 0.65));
        assert_eq!(layout.workspaces[0].split_ratios[0], 0.65);
        layout.validate().expect("valid");
    }

    fn fullscreen_panes(layout: &SharedLayout) -> Vec<(usize, PaneId)> {
        layout
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace
                    .panes
                    .iter()
                    .filter(|pane| pane.fullscreen)
                    .map(|pane| (workspace.index, pane.pane_id))
            })
            .collect()
    }

    #[test]
    fn a_workspace_keeps_at_most_one_fullscreen_pane() {
        let mut layout = three_tiled();
        assert_eq!(layout.edit_pane(1, edit(None, Some(true), None)), Ok(true));
        assert_eq!(layout.edit_pane(2, edit(None, Some(true), None)), Ok(true));
        assert_eq!(
            fullscreen_panes(&layout),
            vec![(0, 2)],
            "the second takes over"
        );

        // A fullscreen pane arriving in a workspace with one of its own takes over there too.
        assert_eq!(layout.move_pane(3, 4), Ok(true));
        assert_eq!(layout.edit_pane(3, edit(None, Some(true), None)), Ok(true));
        assert_eq!(layout.move_pane(2, 4), Ok(true));
        assert_eq!(fullscreen_panes(&layout), vec![(4, 2)]);
    }

    #[test]
    fn removing_a_pane_retiles_its_workspace() {
        let mut layout = three_tiled();
        assert_eq!(layout.remove_pane(2), Some(0));
        assert_eq!(tiling_order(&layout, 0), vec![1, 3]);
        assert_eq!(layout.remove_pane(2), None);
        layout.validate().expect("valid");
    }
}
