use tui_lipan::prelude::FloatRect;

use crate::tiling::DwindleTree;

use super::{Direction, PaneId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeCorner {
    UpperLeft,
    UpperRight,
    LowerLeft,
    LowerRight,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResizeSession {
    pub id: PaneId,
    pub corner: ResizeCorner,
    pub workspace: usize,
    pub start_x: u16,
    pub start_y: u16,
    pub start_tile_tree: Option<DwindleTree>,
    pub start_split_ratios: Vec<f32>,
    pub start_floating_rect: Option<FloatRect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveSession {
    pub id: PaneId,
    pub was_floating: bool,
    pub drag_rect: FloatRect,
    /// Last pointer position in root coordinates, retained so a keyboard action can finish the
    /// drop exactly like a mouse release before changing the pane's mode.
    pub pointer_x: i32,
    pub pointer_y: i32,
}

/// A split-boundary drag in flight. The session is the authority for the whole gesture: it records
/// which boundary was grabbed at drag start and the ratios to measure the pointer delta against.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitDragSession {
    pub kind: SplitDragKind,
    pub workspace: usize,
    pub start_x: u16,
    pub start_y: u16,
    pub start_tile_tree: Option<DwindleTree>,
    pub start_split_ratios: Vec<f32>,
}

/// Which tree split(s) a drag adjusts. Resolved once, from the strip the pointer grabbed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitDragKind {
    Single {
        pane_id: PaneId,
        horizontal_split: bool,
    },
    /// A crossing of two boundaries. Each list holds the pane representatives whose trailing edge
    /// identifies the tree split to move on that axis.
    Junction {
        horizontal_panes: Vec<PaneId>,
        vertical_panes: Vec<PaneId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveSwapHint {
    pub pane: PaneId,
    pub return_direction: Direction,
    pub target: PaneId,
}
