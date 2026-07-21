use crate::tiling::{DwindleTree, collect_tree_leaves};

use super::{DEFAULT_RATIO, Direction, LayoutKind, MoveSwapHint, Pane, PaneId, SplitAxis};

pub const WORKSPACE_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectionalFocusHint {
    pub pane: PaneId,
    pub entry_direction: Direction,
    pub target: PaneId,
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
            name: None,
        }
    }

    pub fn visible_count(&self) -> usize {
        self.panes.iter().filter(|pane| !pane.closing).count()
    }

    pub fn pane_display_number(&self, id: PaneId) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.id == id)
            .map(|index| index + 1)
    }

    pub fn tiled_ids(&self) -> Vec<PaneId> {
        let active = self.active_tiled_ids_by_pane_order();
        let mut ordered = Vec::new();
        if let Some(tree) = self.tile_tree.as_ref() {
            collect_tree_leaves(tree, &mut ordered);
            ordered.retain(|id| active.contains(id));
            for id in &active {
                if !ordered.contains(id) {
                    ordered.push(*id);
                }
            }
        }

        if ordered.is_empty() { active } else { ordered }
    }

    pub fn active_tiled_ids_by_pane_order(&self) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|pane| !pane.floating && !pane.closing)
            .map(|pane| pane.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use tui_lipan::prelude::FloatRect;

    use super::*;

    fn pane() -> Pane {
        Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        )
    }

    #[test]
    fn pane_display_number_uses_workspace_position_not_internal_id() {
        let mut workspace = Workspace::new(0);
        let mut first = pane();
        first.id = 7;
        let mut second = pane();
        second.id = 42;
        workspace.panes = vec![first, second];

        assert_eq!(workspace.pane_display_number(42), Some(2));
        assert_eq!(workspace.pane_display_number(99), None);
    }
}
