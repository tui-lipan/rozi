use crate::layout::tiling::DwindleTree;

use super::{DEFAULT_RATIO, Direction, LayoutKind, MoveSwapHint, Pane, PaneId, SplitAxis};

pub const WORKSPACE_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectionalFocusHint {
    pub pane: PaneId,
    pub entry_direction: Direction,
    pub target: PaneId,
}

/// Which visible edge a Scrollable strip anchor aligns to when scrolling into view.
/// Local viewport state only — never persisted in profiles or SharedLayout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollableRevealEdge {
    /// Align the anchor pane's left edge with the visible left edge.
    #[default]
    Left,
    /// Align the anchor pane's right edge with the visible right edge.
    Right,
    /// Hold the anchor pane's left edge this many cells right of the visible left edge (negative
    /// when it sits scrolled past it). Reordering panes records the strip's scroll this way so a
    /// pane can land in a visible slot without the columns around it sliding.
    LeftOffset(i32),
}

pub struct Workspace {
    pub panes: Vec<Pane>,
    pub tile_tree: Option<DwindleTree>,
    pub focused_pane: Option<PaneId>,
    pub synchronized: bool,
    pub layout_kind: LayoutKind,
    pub start_axis: SplitAxis,
    pub split_ratios: Vec<f32>,
    pub last_move_swap: Option<MoveSwapHint>,
    pub last_directional_focus: Option<DirectionalFocusHint>,
    /// Last live tiled pane that held focus. Scrollable layout uses this as the strip anchor when
    /// focus is on a floating pane so the underlying columns do not jump. Local view state only —
    /// never persisted in profiles or SharedLayout.
    pub scrollable_anchor: Option<PaneId>,
    /// How the Scrollable allocator aligns [`Self::scrollable_anchor`] into the visible interval.
    /// Local view state only — never persisted in profiles or SharedLayout.
    pub scrollable_reveal_edge: ScrollableRevealEdge,
    /// User-assigned label shown in the workbar in place of (or alongside) the workspace
    /// number. `None` keeps the default numeric display.
    pub name: Option<String>,
}

impl Workspace {
    pub fn new(index: usize) -> Self {
        Self {
            panes: Vec::new(),
            tile_tree: None,
            focused_pane: None,
            synchronized: false,
            layout_kind: LayoutKind::Dwindle,
            start_axis: if index.is_multiple_of(2) {
                SplitAxis::Horizontal
            } else {
                SplitAxis::Vertical
            },
            split_ratios: vec![DEFAULT_RATIO; 16],
            last_move_swap: None,
            last_directional_focus: None,
            scrollable_anchor: None,
            scrollable_reveal_edge: ScrollableRevealEdge::Left,
            name: None,
        }
    }

    /// Set or clear the local Scrollable viewport anchor and its reveal edge together.
    pub fn set_scrollable_viewport(&mut self, anchor: Option<PaneId>, edge: ScrollableRevealEdge) {
        self.scrollable_anchor = anchor;
        self.scrollable_reveal_edge = if anchor.is_some() {
            edge
        } else {
            ScrollableRevealEdge::Left
        };
    }

    pub fn visible_count(&self) -> usize {
        self.panes.iter().filter(|pane| !pane.closing).count()
    }

    pub fn tiled_ids(&self) -> Vec<PaneId> {
        crate::layout::ordered_tiled_ids(self)
    }

    pub fn active_tiled_ids_by_pane_order(&self) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|pane| !pane.floating && !pane.closing)
            .map(|pane| pane.id)
            .collect()
    }
}
