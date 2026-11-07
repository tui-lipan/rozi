//! Server-authoritative shared layout: the structured document that describes a named session's
//! window-manager state (workspace membership + order, tiling trees + ratios, layout kind, start
//! axis, floating/fullscreen geometry, workspace names, sync flag, and pane identity). It is the
//! only layout representation on the wire - profile TOML remains the on-disk format for
//! profiles/autosave, but nothing in the session protocol uses TOML.
//!
//! A controller client commits a [`SharedLayout`] on every layout change; the server bumps a
//! revision and broadcasts it, and every follower reconciles its local `State` toward the document
//! via [`apply_shared_layout`] without touching live terminal screens.

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::layout::tiling::DwindleTree;
use crate::layout::tree_ser::{
    SerializedLayoutKind, SerializedSplitAxis, SerializedTree, from_dwindle, to_dwindle,
};
use crate::pane::id::PaneId;

/// Stable identity for an attached client, assigned by the server on attach.
pub type ClientId = u64;

/// Wire-format version for [`SharedLayout`]. Bumped if the document shape changes.
/// Version 4 includes the controller's pane chrome geometry.
pub const SHARED_LAYOUT_VERSION: u32 = 4;

/// The complete shared window-manager document for a session. Fractions in [`FracRect`] are
/// relative to the controller's canonical pane canvas (`canvas_cols` × `canvas_rows`, excluding
/// the workbar) so followers place floats inside the same centered canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SharedLayout {
    pub version: u32,
    pub canvas_cols: u16,
    pub canvas_rows: u16,
    pub pane_geometry: SharedPaneGeometry,
    pub workspaces: Vec<SharedWorkspace>,
}

/// Geometry-affecting presentation shared with followers so their terminal grids match.
/// Colors, border glyphs, and animation recipes remain client-local.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SharedPaneGeometry {
    pub show_titles: bool,
    pub titlebar: crate::pane::chrome::PaneTitlebarMode,
    pub border_mode: crate::pane::chrome::PaneBorderMode,
    pub keep_special_borders: bool,
    pub padding: (u16, u16, u16, u16),
    pub top_gap: i8,
}

impl Default for SharedPaneGeometry {
    /// The structural defaults of a stock config. `project.rs` tests that they still agree.
    fn default() -> Self {
        Self {
            show_titles: true,
            titlebar: crate::pane::chrome::PaneTitlebarMode::Bar,
            border_mode: crate::pane::chrome::PaneBorderMode::Separate,
            keep_special_borders: true,
            padding: (0, 0, 0, 0),
            top_gap: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SharedWorkspace {
    pub index: usize,
    pub name: Option<String>,
    pub synchronized: bool,
    pub layout: SharedLayoutKind,
    pub start_axis: SharedSplitAxis,
    pub split_ratios: Vec<f32>,
    pub tree: Option<SharedTree>,
    pub panes: Vec<SharedPane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SharedPane {
    pub pane_id: PaneId,
    pub generation: u64,
    pub title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub launch: Option<crate::pane::launch::PaneLaunch>,
    /// See [`crate::state::PaneIdentity::replay`]. Shared so a follower that takes control
    /// respawns an exited profile pane through the interactive shell, not the command runner.
    /// Defaulted so layout documents committed before this field existed still parse.
    #[serde(default)]
    pub replay: bool,
    pub keep_open: bool,
    pub floating: bool,
    /// Carried to whichever workspace the controller switches to. Only a floating pane is pinned.
    #[serde(default)]
    pub pinned: bool,
    pub fullscreen: bool,
    /// Fractions of the canonical canvas; `Some` only for floating panes.
    pub rect: Option<FracRect>,
    /// Scrollable column width as a fraction of the tile viewport.
    #[serde(default = "default_shared_scrollable_width")]
    pub scrollable_width: f32,
}

fn default_shared_scrollable_width() -> f32 {
    crate::layout::model::DEFAULT_SCROLLABLE_WIDTH
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct FracRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
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
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct RemoteDrag {
    pub pane_id: PaneId,
    /// The lifted rectangle, as fractions of the controller's canonical canvas - the same frame
    /// [`SharedPane::rect`](crate::layout::shared::SharedPane::rect) uses, so a follower with a
    /// different viewport letterboxes it exactly like a floating pane.
    pub rect: FracRect,
}

/// The tiling tree with direct pane ids (no positional indirection like profile TOML).
pub type SharedTree = SerializedTree<PaneId>;
pub type SharedLayoutKind = SerializedLayoutKind;
pub type SharedSplitAxis = SerializedSplitAxis;

#[cfg(test)]
pub(super) mod test_support;

mod edit;
mod placement;
mod project;
mod reconcile;
mod validate;

pub(crate) use edit::*;
pub(crate) use placement::*;
pub use project::*;
pub(crate) use reconcile::*;
pub use validate::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_pane_without_replay_field_parses_as_non_replay() {
        // `replay` remains defaulted independently of the launch representation.
        let pane: SharedPane = serde_json::from_value(serde_json::json!({
            "pane_id": 2,
            "generation": 7,
            "title": null,
            "profile_name": null,
            "cwd": null,
            "launch": {"kind": "shell", "command": "nvim"},
            "keep_open": false,
            "floating": false,
            "fullscreen": false,
            "rect": null
        }))
        .expect("pre-replay shared pane parses");
        assert!(!pane.replay);
        assert_eq!(
            pane.scrollable_width,
            crate::layout::model::DEFAULT_SCROLLABLE_WIDTH
        );
    }
}
