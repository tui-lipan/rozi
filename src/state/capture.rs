use std::cell::Cell;

use super::{AttachmentId, PaneId};

/// What a screenshot action photographed, and so what its flash covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotTarget {
    /// A pane, named with its namespace: a popup or scratch pane and a shared pane may carry the
    /// same numeric id, and a session switch reuses ids from another attachment.
    Pane {
        id: PaneId,
        /// The attachment epoch a shared pane belongs to; `None` for a local popup or scratch pane.
        attachment: Option<AttachmentId>,
    },
    Ui,
}

/// The screenshot actions' client-local state.
#[derive(Debug, Default)]
pub struct ScreenshotState {
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
