use std::collections::HashMap;
use std::time::Instant;

use tui_lipan::prelude::{OverlayId, TextInput};

use crate::config::ProfileEntry;
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};

use super::PaneId;

pub struct SaveProfileState {
    pub input: TextInput,
    pub pending_overwrite: bool,
}

pub struct ProfilePickerState {
    pub entries: Vec<ProfileEntry>,
    pub input: TextInput,
    /// Index into [`Self::entries`] for the highlighted profile.
    pub selected: usize,
    /// Entry index awaiting a second Ctrl+D to confirm deletion.
    pub pending_delete: Option<usize>,
    pub pending_open: Option<usize>,
    pub running: HashMap<String, DiscoveredSessionStatus>,
    pub pending_apply: Option<usize>,
    pub apply_mode: bool,
}

pub struct SessionPickerState {
    pub entries: Vec<DiscoveredSession>,
    pub input: TextInput,
    pub selected: usize,
    /// Entry index awaiting a second Ctrl+K to confirm its kill. The armed state is shown inline on
    /// the row itself (struck through in the error color with `again to kill`), so no separate
    /// confirm toast is needed.
    pub pending_kill: Option<usize>,
    /// Entry index awaiting a second Ctrl+E to confirm its restart (warning highlight, no strike).
    pub pending_restart: Option<usize>,
}

/// The open *Manage collaborators* dialog: the roster of everyone else on the session. The
/// session-wide controls it sits beside (request control, input lock, takeover) are ordinary
/// command-palette entries, not part of this dialog.
pub struct CollaborationState {
    pub selected: usize,
    /// The client awaiting a second key press to confirm its removal, held by id rather than by
    /// roster position: the roster is server-pushed and can reorder under an armed row, and a
    /// confirmation that lands on whoever slid into that slot is the one mistake this arming exists
    /// to prevent. Cleared by moving the highlight or closing the dialog.
    pub pending_kick: Option<crate::shared_layout::ClientId>,
    /// The live filter text. Mirrored out of the palette so the dialog can rank the roster the same
    /// way the widget does and refuse to act on a client the filter has hidden — a `ctrl+k` that
    /// removed somebody scrolled off by a query would be indefensible.
    pub query: String,
}

impl CollaborationState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            pending_kick: None,
            query: String::new(),
        }
    }
}

impl Default for CollaborationState {
    fn default() -> Self {
        Self::new()
    }
}

/// What the user can do about attaching to a session another client is actively driving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowChoice {
    /// Stay attached as a follower: same panes and layout, view synchronized to the controller.
    Follow,
    /// Request the layout-control lease. This asks the controller under cooperative policy and
    /// takes the lease immediately when session takeover is enabled.
    AskForControl,
    /// Leave the session alone and go back where we came from.
    Cancel,
}

impl FollowChoice {
    pub const ALL: [Self; 3] = [Self::Follow, Self::AskForControl, Self::Cancel];

    pub fn label(self, allow_takeover: bool) -> &'static str {
        match self {
            Self::Follow => "Follow",
            Self::AskForControl if allow_takeover => "Take control",
            Self::AskForControl => "Ask for control",
            Self::Cancel => "Cancel",
        }
    }

    pub fn description(self, allow_takeover: bool) -> &'static str {
        match self {
            Self::Follow => "no layout control",
            Self::AskForControl if allow_takeover => "control moves to you",
            Self::AskForControl => "send a request",
            Self::Cancel => "go back",
        }
    }
}

/// Raised when an attach lands on a session another client is actively controlling. Following is a
/// deliberate choice, never something that happens silently to a client for being second.
pub struct FollowPromptState {
    pub session: String,
    /// How the controlling client identifies itself in the roster.
    pub controller_label: String,
    pub allow_takeover: bool,
    pub selected: usize,
}

/// The dialog a nested one was raised from, so cancelling (or finishing) the child returns there
/// instead of dropping the user back on the terminal. A picker is rebuilt rather than un-hidden —
/// opening a child drops the picker's state — so its origin carries the query and highlighted row
/// the rebuild has to restore. See [`crate::ops::overlay_return`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayOrigin {
    Settings,
    ProfilePicker {
        query: String,
        selected: usize,
        apply_mode: bool,
    },
    SessionPicker {
        query: String,
        selected: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingDestructive {
    ClosePane(PaneId),
    KillWorkspace(usize),
    KillSession,
    RestartSession,
    NewTemporarySession,
}

pub struct PendingDestructiveConfirmation {
    pub action: PendingDestructive,
    pub armed_at: Instant,
    pub toast_id: OverlayId,
}

/// A named toast slot, for state whose *newest* value should supersede the previous one even
/// though the text differs. Everything else de-duplicates on its content instead and needs no
/// entry here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ToastChannel {
    InputState,
    LayoutControl,
    /// The active workspace layout, so cycling through the modes reports the newest one in place
    /// rather than leaving a column of superseded names.
    LayoutMode,
    PreferenceSave,
    /// Attach/reconnect progress and its outcome, so `Reconnecting to X…` is replaced by whatever
    /// happened rather than being buried under it.
    SessionLifecycle,
}

impl SessionPickerState {
    pub fn new(entries: Vec<DiscoveredSession>) -> Self {
        Self {
            entries,
            input: TextInput::new(""),
            selected: 0,
            pending_kill: None,
            pending_restart: None,
        }
    }
}

impl ProfilePickerState {
    pub fn new(entries: Vec<ProfileEntry>) -> Self {
        Self {
            entries,
            input: TextInput::new(""),
            selected: 0,
            pending_delete: None,
            pending_open: None,
            running: HashMap::new(),
            pending_apply: None,
            apply_mode: false,
        }
    }
}

impl SaveProfileState {
    pub fn new(initial: &str) -> Self {
        Self {
            input: TextInput::new(initial),
            pending_overwrite: false,
        }
    }
}

/// The open layout-mode picker. The list is the fixed [`crate::state::LayoutKind::all`] set, so this
/// only tracks the highlighted row — the row set is derived at render time. `selected` lets the
/// `set default` key act on whichever mode is highlighted; `original` is the active workspace's
/// layout when the picker opened, restored if the user leaves without committing (live preview).
pub struct LayoutPickerState {
    pub selected: usize,
    pub original: super::LayoutKind,
}

impl LayoutPickerState {
    pub fn new(selected: usize, original: super::LayoutKind) -> Self {
        Self { selected, original }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PickRow {
    #[serde(default)]
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub disabled: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub priority: Option<i32>,
}

/// One extra key the caller offers alongside select and cancel.
///
/// Actions are what turn a picker from a menu into a working surface: delete the branch under the
/// cursor and push a fresh list, or open a prompt and create one. They are declared up front so the
/// footer can advertise them the way every built-in picker advertises its own chords.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PickAction {
    /// Returned to the caller as `action`.
    pub id: String,
    /// Chord that fires it, in the same spelling `[keys]` uses (`ctrl-n`).
    pub key: String,
    /// Footer text, e.g. `new branch`.
    pub label: String,
    /// When set, the key opens a text prompt with this title and the entered text rides back as
    /// `input`. Cancelling the prompt returns to the picker without reporting anything.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Whether firing it ends the picker. Default `false`: the caller usually wants to push an
    /// updated row set and keep going.
    #[serde(default)]
    pub close: bool,
}

/// An open prompt raised by a [`PickAction`], holding the picker underneath it.
pub struct PickPrompt {
    /// Index into [`PickState::actions`].
    pub action: usize,
    pub title: String,
    pub input: TextInput,
}

pub struct PickState {
    pub id: u64,
    pub title: String,
    pub placeholder: String,
    /// Caller-chosen modal width in columns, clamped on the way in.
    pub width: u16,
    pub actions: Vec<PickAction>,
    pub prompt: Option<PickPrompt>,
    pub rows: Vec<PickRow>,
    pub selected: usize,
    pub reply: std::sync::mpsc::SyncSender<String>,
}
