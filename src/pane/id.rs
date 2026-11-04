//! Pane identity shared by the layout, session and app layers.

/// Identifies one pane within a session.
pub type PaneId = u32;

/// Reserved id of the popup pane, which is never part of a layout.
pub const POPUP_PANE_ID: PaneId = u32::MAX;
