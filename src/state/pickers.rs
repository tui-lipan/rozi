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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSessionIdentity {
    pub target: crate::session::remote::RemoteTarget,
    pub name: String,
}

impl RemoteSessionIdentity {
    pub fn of(session: &DiscoveredSession) -> Option<Self> {
        Some(Self {
            target: session.remote_target.clone()?,
            name: session.name.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemotePickerMode {
    Hosts,
    HostSessions {
        target: crate::session::remote::RemoteTarget,
    },
}

pub struct RemoteTargetPromptState {
    pub input: TextInput,
    pub error: Option<String>,
}

impl RemoteTargetPromptState {
    pub fn new(initial: impl AsRef<str>) -> Self {
        Self {
            input: TextInput::new(initial.as_ref()),
            error: None,
        }
    }
}

/// The dedicated remote-host browser. Host and session navigation retain independent text fields
/// and identity-based selections so returning from a host never loses the user's place and a
/// reordered async result cannot redirect a destructive confirmation.
pub struct RemotePickerState {
    pub mode: RemotePickerMode,
    pub host_input: TextInput,
    pub selected_host: Option<crate::session::remote::RemoteTarget>,
    pub session_input: TextInput,
    pub selected_session: Option<RemoteSessionIdentity>,
    pub sessions: Vec<DiscoveredSession>,
    pub probe_epoch: u64,
    pub host_probe: super::HostProbe,
    pub target_prompt: Option<RemoteTargetPromptState>,
    pub pending_forget: Option<crate::session::remote::RemoteTarget>,
    pub pending_kill: Option<RemoteSessionIdentity>,
    pub pending_restart: Option<RemoteSessionIdentity>,
    /// Bumped to remount the hosts palette while a probe is in flight, so navigation cannot
    /// move the highlight off the connecting row.
    pub interaction_epoch: u64,
}

impl RemotePickerState {
    pub fn new(selected_host: Option<crate::session::remote::RemoteTarget>) -> Self {
        Self {
            mode: RemotePickerMode::Hosts,
            host_input: TextInput::new(""),
            selected_host,
            session_input: TextInput::new(""),
            selected_session: None,
            sessions: Vec::new(),
            probe_epoch: 0,
            host_probe: super::HostProbe::Idle,
            target_prompt: None,
            pending_forget: None,
            pending_kill: None,
            pending_restart: None,
            interaction_epoch: 0,
        }
    }

    pub fn enter_host_sessions(&mut self, target: crate::session::remote::RemoteTarget) {
        self.selected_host = Some(target.clone());
        self.mode = RemotePickerMode::HostSessions { target };
        self.sessions.clear();
        self.selected_session = None;
        self.host_probe = super::HostProbe::Idle;
        self.pending_forget = None;
        self.pending_kill = None;
        self.pending_restart = None;
        self.target_prompt = None;
    }

    pub fn return_to_hosts(&mut self) {
        self.mode = RemotePickerMode::Hosts;
        self.sessions.clear();
        self.selected_session = None;
        self.host_probe = super::HostProbe::Idle;
        self.pending_kill = None;
        self.pending_restart = None;
        self.target_prompt = None;
    }

    pub fn replace_sessions(&mut self, sessions: Vec<DiscoveredSession>) {
        let changed = self.sessions != sessions;
        self.sessions = sessions;
        let selected = self.selected_session.take().filter(|selected| {
            self.sessions
                .iter()
                .filter_map(RemoteSessionIdentity::of)
                .any(|identity| &identity == selected)
        });
        self.selected_session =
            selected.or_else(|| self.sessions.first().and_then(RemoteSessionIdentity::of));
        let identity_exists = |pending: &RemoteSessionIdentity| {
            self.sessions
                .iter()
                .filter_map(RemoteSessionIdentity::of)
                .any(|identity| &identity == pending)
        };
        if changed
            || self
                .pending_kill
                .as_ref()
                .is_some_and(|pending| !identity_exists(pending))
            || self
                .pending_restart
                .as_ref()
                .is_some_and(|pending| !identity_exists(pending))
        {
            self.pending_kill = None;
            self.pending_restart = None;
        }
    }
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

/// One prompt OpenSSH raised on a connection this client owns, waiting on the user.
pub struct AskpassPrompt {
    /// Identifies the waiting helper process. The answer is routed by id, so a reply can never
    /// reach the ssh that asked a different question.
    pub id: u64,
    /// The ssh invocation asking. All three of one connection's retries share it; the next
    /// connection carries a different one.
    pub session: String,
    pub kind: crate::session::remote::AskpassKind,
    /// Verbatim prompt text from ssh, multi-line for host-key verification.
    pub prompt: String,
    /// Why the previous answer to this same question was not accepted. `ssh` re-asks in silence,
    /// so without this the modal reappears looking like it was never submitted.
    pub error: Option<String>,
}

/// What became of the last ssh prompt, which is the only way to read the next one.
///
/// OpenSSH re-asks the *same question* three times after a wrong answer **and** after a refusal —
/// declining is not "no" to ssh, it is "that answer was wrong" — and it says nothing about which
/// it is. A probe runs several ssh invocations back to back, each with its own three. What
/// separates them is the connection asking: a repeat from the same connection means the answer was
/// rejected, and a prompt from a connection whose prompt was refused is the refusal being ignored.
///
/// Keying on the connection rather than on elapsed time is what keeps a fresh attempt at the same
/// host from inheriting the last one's verdict — the user retrying a host they just cancelled must
/// get a prompt, not a silent refusal.
#[derive(Default)]
pub struct AskpassHistory {
    /// The connection whose prompts are declined unasked, because one of them already was.
    refused: Option<String>,
    /// The connection and question last answered, so its repeat can be recognized.
    answered: Option<(String, String)>,
}

impl AskpassHistory {
    /// Whether this connection has already been refused, so the prompt should be declined unasked.
    pub fn refuses(&self, session: &str) -> bool {
        self.refused.as_deref() == Some(session)
    }

    pub fn refused(&mut self, session: &str) {
        self.refused = Some(session.to_string());
        self.answered = None;
    }

    pub fn answered(&mut self, session: &str, prompt: &str) {
        self.refused = None;
        self.answered = Some((session.to_string(), prompt.to_string()));
    }

    /// Whether `prompt` is one connection asking again for something already answered — that is,
    /// whether the answer was rejected.
    pub fn is_retry_of(&self, session: &str, prompt: &str) -> bool {
        self.answered
            .as_ref()
            .is_some_and(|(last_session, last_prompt)| {
                last_session == session && last_prompt == prompt
            })
    }
}

/// The modal standing in for the terminal prompt `ssh` would otherwise write over the UI.
///
/// Two ssh processes can be in flight at once (a host probe alongside an attach), so a second
/// prompt queues behind the one on screen rather than replacing it — replacing would leave the
/// first ssh waiting on an answer nobody can give any more.
pub struct AskpassState {
    pub current: AskpassPrompt,
    pub input: TextInput,
    pub queued: std::collections::VecDeque<AskpassPrompt>,
}

impl AskpassState {
    pub fn new(current: AskpassPrompt) -> Self {
        Self {
            current,
            input: TextInput::new(""),
            queued: std::collections::VecDeque::new(),
        }
    }

    /// Move to the next queued prompt, if any, with a cleared field. Returns `false` when the
    /// queue is empty and the modal should close.
    pub fn advance(&mut self) -> bool {
        match self.queued.pop_front() {
            Some(next) => {
                self.current = next;
                self.input = TextInput::new("");
                true
            }
            None => false,
        }
    }

    /// Drop `id` wherever it sits. Returns `true` when it was the prompt on screen, so the caller
    /// knows to [`Self::advance`].
    pub fn discard(&mut self, id: u64) -> bool {
        if self.current.id == id {
            return true;
        }
        self.queued.retain(|prompt| prompt.id != id);
        false
    }
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
    RemoteHosts {
        query: String,
        selected_target: Option<crate::session::remote::RemoteTarget>,
    },
    RemoteHostSessions {
        target: crate::session::remote::RemoteTarget,
        query: String,
        selected_session: Option<RemoteSessionIdentity>,
        /// The picker below Remote hosts, retained while a naming prompt temporarily replaces the
        /// remote picker.
        parent: Option<Box<OverlayOrigin>>,
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
    /// Session attach/reconnect outcomes, so repeated lifecycle failures replace one another.
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

#[cfg(test)]
mod remote_picker_tests {
    use super::*;

    #[test]
    fn mode_transitions_clear_incompatible_confirmations() {
        let first = crate::session::remote::RemoteTarget::Alias("first".into());
        let second = crate::session::remote::RemoteTarget::Alias("second".into());
        let session = RemoteSessionIdentity {
            target: second.clone(),
            name: "dev".into(),
        };
        let mut picker = RemotePickerState::new(Some(first.clone()));
        picker.pending_forget = Some(first);
        picker.enter_host_sessions(second);
        assert!(picker.pending_forget.is_none());

        picker.pending_kill = Some(session.clone());
        picker.pending_restart = Some(session);
        picker.return_to_hosts();
        assert!(picker.pending_kill.is_none());
        assert!(picker.pending_restart.is_none());
    }

    #[test]
    fn replacing_sessions_disarms_a_stale_confirmation() {
        let target = crate::session::remote::RemoteTarget::Alias("workbox".into());
        let identity = RemoteSessionIdentity {
            target: target.clone(),
            name: "dev".into(),
        };
        let mut picker = RemotePickerState::new(Some(target.clone()));
        picker.enter_host_sessions(target);
        picker.pending_kill = Some(identity.clone());
        picker.pending_restart = Some(identity);
        picker.replace_sessions(Vec::new());
        assert!(picker.pending_kill.is_none());
        assert!(picker.pending_restart.is_none());
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
    /// Require a second press to fire, the way the session and profile pickers arm a kill or a
    /// delete. The armed row is struck through in the error colour with an `again to <label>` cue,
    /// and moving the highlight disarms it.
    #[serde(default)]
    pub confirm: bool,
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
    pub extension: Option<crate::config::ExtensionProvenance>,
    pub title: String,
    pub placeholder: String,
    /// Caller-chosen modal width in columns, clamped on the way in.
    pub width: u16,
    pub actions: Vec<PickAction>,
    pub prompt: Option<PickPrompt>,
    /// The live filter text, mirrored out of the palette so a rebuild can restore it.
    pub query: String,
    /// What the rebuilt palette is seeded with. Updated only when the picker is about to unmount,
    /// so it stays stable while typing - feeding the live mirror back as `initial_query` would
    /// change that prop on every keystroke and risk re-seeding the field mid-edit.
    pub restore_query: String,
    /// The `confirm` action awaiting its second press, with the row it was armed on. Held by row
    /// id rather than position: the caller can push a new list under an armed row, and a
    /// confirmation landing on whoever slid into that slot is the mistake arming exists to stop.
    pub pending_action: Option<(usize, String)>,
    pub rows: Vec<PickRow>,
    pub selected: usize,
    pub reply: std::sync::mpsc::SyncSender<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = "dev@workbox's password: ";

    /// One Esc has to cover the whole connection: ssh re-raises the same question three times
    /// whatever the helper answers.
    #[test]
    fn a_refusal_covers_every_later_prompt_from_the_same_connection() {
        let mut history = AskpassHistory::default();
        history.refused("ssh-1");

        assert!(history.refuses("ssh-1"));
        assert!(
            !history.refuses("ssh-2"),
            "a fresh connection is asked, not refused"
        );
    }

    /// The bug this replaced a timer to fix: cancelling a host and immediately trying it again is
    /// a new connection, and it has to get a prompt rather than inherit the refusal.
    #[test]
    fn retrying_the_host_after_a_refusal_is_asked_again() {
        let mut history = AskpassHistory::default();
        history.refused("probe-1");
        assert!(!history.refuses("probe-2"));
        assert!(!history.is_retry_of("probe-2", PROMPT));
    }

    /// A repeat of a question already answered is ssh saying the answer was wrong — the only
    /// signal it gives, since the prompt text is identical either way.
    #[test]
    fn a_repeat_of_an_answered_prompt_reads_as_a_rejection() {
        let mut history = AskpassHistory::default();
        history.answered("ssh-1", PROMPT);

        assert!(history.is_retry_of("ssh-1", PROMPT));
        assert!(
            !history.is_retry_of("ssh-1", "dev@other's password: "),
            "a different question is a different question"
        );
        assert!(
            !history.is_retry_of("ssh-2", PROMPT),
            "and the same question from a new connection is a first ask"
        );
    }

    /// The two records are exclusive. Answering ends a refusal (the user changed their mind), and
    /// refusing drops the answered record, so a later dialog cannot open pre-accusing a password
    /// nobody typed.
    #[test]
    fn answering_and_refusing_each_clear_the_other() {
        let mut history = AskpassHistory::default();

        history.refused("ssh-1");
        history.answered("ssh-1", PROMPT);
        assert!(!history.refuses("ssh-1"));

        history.refused("ssh-1");
        assert!(!history.is_retry_of("ssh-1", PROMPT));
    }
}
