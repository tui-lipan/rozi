use std::time::Instant;

use tui_lipan::prelude::{OverlayId, TextInput};

use crate::config::ProfileEntry;
use crate::session::discovery::DiscoveredSession;

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
}

pub struct SessionPickerState {
    pub entries: Vec<DiscoveredSession>,
    pub input: TextInput,
    pub selected: usize,
    /// Entry index awaiting a second Ctrl+K to confirm its kill. The armed state is shown inline on
    /// the row itself (struck through in the error color), so no separate confirm toast is needed.
    pub pending_kill: Option<usize>,
    /// Entry index awaiting a second Enter to confirm attaching to it while the current session is a
    /// disposable ephemeral one - opening the target shuts that ephemeral server down and kills its
    /// panes, so it warrants the same two-press guard as a kill. Signalled inline on the row (a
    /// warning-colored highlight), so it needs no confirm toast either.
    pub pending_open: Option<usize>,
}

pub struct ClientListState {
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingDestructive {
    ClosePane(PaneId),
    KillWorkspace(usize),
    KillSession,
    /// Quit an ephemeral session that still has a live pane (shuts the server down).
    Quit,
    NewTemporarySession,
}

pub struct PendingDestructiveConfirmation {
    pub action: PendingDestructive,
    pub armed_at: Instant,
    pub toast_id: OverlayId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ToastChannel {
    InputState,
    LayoutControl,
    LayoutMode,
    PaneSynchronization,
    PreferenceSave,
}

impl SessionPickerState {
    pub fn new(entries: Vec<DiscoveredSession>) -> Self {
        Self {
            entries,
            input: TextInput::new(""),
            selected: 0,
            pending_kill: None,
            pending_open: None,
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
