use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use tui_lipan::prelude::TextInput;

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

/// A recording command this UI sent its session server, and so how it shows the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingAction {
    Start(PaneId),
    Stop(PaneId),
    /// A mark on the recordings of this pane, whose dot blinks once it lands.
    Mark(PaneId),
}

/// The Mark pane recording prompt: the label for the recordings of `target`.
pub struct RecordingMarkPrompt {
    pub target: PaneId,
    pub input: TextInput,
}

/// A recording of this UI as it paints, from `record-ui-start` or the Start UI recording command.
///
/// It holds the paint subscription, so the framework captures frames only while one runs, and it
/// writes on this client's machine even when the session it shows is remote.
pub struct UiRecording {
    /// Tells this recording's timers from those of one stopped before it.
    pub id: u64,
    pub subscription: tui_lipan::PaintSubscription,
    pub max_fps: u32,
    pub duration_ms: u64,
    pub max_bytes: u64,
    pub phase: UiRecordingPhase,
    /// When the last frame handed to the writer was painted. The `max_fps` ceiling counts from it.
    pub last_written: Option<Instant>,
    /// The newest frame painted before the ceiling allowed another, written when it does.
    pub pending: Option<tui_lipan::PaintedFrame>,
    /// A timer will write [`Self::pending`]; later frames only replace it.
    pub flush_armed: bool,
    /// What the meta events last reported.
    pub seen: UiRecordingSeen,
    /// Started from the palette, so its start is shown as a toast.
    pub from_action: bool,
}

/// Where a UI recording is in its life.
pub enum UiRecordingPhase {
    /// Waiting for the first frame, whose size and colors the file's header records. The start
    /// repaints the UI, since it puts the recording indicator on screen, so this is one frame.
    Starting {
        output: crate::recording::start::Output,
        force: bool,
        /// The `record-ui-start` waiting to hear where the file is.
        reply: Option<std::sync::mpsc::Sender<crate::control::ControlResponse>>,
        requested: Instant,
    },
    Running(Box<UiRecordingFile>),
}

/// A UI recording's open file.
pub struct UiRecordingFile {
    pub recorder: crate::recording::Recorder,
    pub path: PathBuf,
    /// When the first frame was painted, by the runtime clock frames are stamped with.
    pub first_painted: Instant,
    /// When the first frame arrived, by this machine's clock: what the duration limit counts from.
    pub started: Instant,
    pub started_at_unix_ms: u64,
}

/// The UI state a recording reports as meta events when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiRecordingSeen {
    pub focus: Option<PaneId>,
    pub workspace: usize,
    pub workspace_name: Option<String>,
    pub overlay: Option<&'static str>,
}

/// The blink that confirms a mark landed.
#[derive(Debug, Default)]
pub struct RecordingMarkBlink {
    /// The pane whose dot is blinking, until the blink `revision` names ends.
    pub pane: Option<PaneId>,
    pub revision: u64,
}
