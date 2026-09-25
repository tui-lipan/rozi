use std::cell::Cell;

use super::PaneId;

/// What a screenshot action photographed, and so what its flash covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotTarget {
    Pane(PaneId),
    Ui,
}

/// The screenshot actions' client-local state.
#[derive(Debug, Default)]
pub struct ScreenshotState {
    /// A Screenshot UI is waiting for the frame it will save. While it is, the view settles the
    /// dialog dim at once instead of fading it, so the frame shows the UI at rest rather than the
    /// palette that ran the action still lifting off it.
    pub ui_waiting: bool,
    /// The latest flash. Each screenshot takes a new `revision`, which restarts the flash rather
    /// than stacking a second one on it; a finished flash is left here at rest.
    pub flash: Option<ScreenshotFlash>,
    pub next_revision: u64,
    /// The flash revision the view last started, so it restarts once per screenshot.
    pub flash_seen: Cell<Option<u64>>,
    /// What the root view sampled for this frame: the target and the tint's current strength.
    /// `None` at rest. Read by the pane view, which is built later in the same frame.
    pub flash_frame: Cell<Option<(ScreenshotTarget, f32)>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenshotFlash {
    pub target: ScreenshotTarget,
    pub revision: u64,
}
