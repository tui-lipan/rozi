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
    /// the row itself (struck through in the error color), so no separate confirm toast is needed.
    pub pending_kill: Option<usize>,
}

pub struct ClientListState {
    pub selected: usize,
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
    Appearance,
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
