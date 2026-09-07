use serde::{Deserialize, Serialize};
use tui_lipan::prelude::FloatRect;

use crate::layout::shared::FracRect;
use crate::layout::tiling::DwindleTree;

use super::{Direction, LayoutTarget, PaneId};

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
    pub workspace: LayoutTarget,
    pub start_x: u16,
    pub start_y: u16,
    pub start_tile_tree: Option<DwindleTree>,
    pub start_split_ratios: Vec<f32>,
    pub start_floating_rect: Option<FloatRect>,
    /// Snapshot of the pane's Scrollable width fraction at drag start (absolute deltas).
    pub start_scrollable_width: Option<f32>,
    /// Snapshot of the scratchpad height fraction, set only when the grabbed corner is an upper
    /// one on a pane against the dropdown's top edge. That edge is the scratch workspace's outer
    /// border, so the drag's vertical component moves the whole dropdown instead of a split that
    /// does not exist; the horizontal component still resizes the pane.
    pub start_scratch_height: Option<f32>,
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

/// Another client's tiled drag, mirrored here so every attached client lifts the same pane out of
/// the same tiling.
///
/// This is presence state, not layout: it rides its own wire messages instead of
/// [`SharedLayout`](crate::layout::shared::SharedLayout), because a half-lifted pane must never be
/// persisted, resurrected, or inherited by whoever takes the control lease next. It is cleared
/// whenever the controller changes, so a disconnect mid-gesture heals into the authoritative
/// layout rather than leaving a pane floating forever.
///
/// Only *tiled* drags need it. A floating pane's rectangle is already part of the shared document,
/// so dragging one replicates through the ordinary layout commit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteDrag {
    pub pane_id: PaneId,
    /// The lifted rectangle, as fractions of the controller's canonical canvas - the same frame
    /// [`SharedPane::rect`](crate::layout::shared::SharedPane::rect) uses, so a follower with a
    /// different viewport letterboxes it exactly like a floating pane.
    pub rect: FracRect,
}

/// A split-boundary drag in flight. The session is the authority for the whole gesture: it records
/// which boundary was grabbed at drag start and the ratios to measure the pointer delta against.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitDragSession {
    pub kind: SplitDragKind,
    pub workspace: LayoutTarget,
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
