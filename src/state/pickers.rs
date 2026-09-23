use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// A repository picker scoped to the host and project root of the pane that opened it.
pub struct WorktreePickerState {
    pub cwd: String,
    pub target: Option<crate::session::remote::RemoteTarget>,
    pub entries: Vec<crate::git::worktrees::WorktreeInfo>,
    pub sessions: Vec<DiscoveredSession>,
    pub input: TextInput,
    pub selected: usize,
    pub pending_list: Option<u64>,
    pub pending_remove: Option<String>,
    pub form: Option<WorktreeFormState>,
    pub error: Option<String>,
    /// Opened from the sidebar straight into the new-worktree form: there is no list behind the
    /// form, so closing or submitting it closes the whole overlay.
    pub standalone_form: bool,
}

impl WorktreePickerState {
    pub fn new(cwd: String, target: Option<crate::session::remote::RemoteTarget>) -> Self {
        Self {
            cwd,
            target,
            entries: Vec::new(),
            sessions: Vec::new(),
            input: TextInput::new(""),
            selected: 0,
            pending_list: None,
            pending_remove: None,
            form: None,
            error: None,
            standalone_form: false,
        }
    }
}

/// The last checkouts listed for each repository, per host, so the Worktrees picker opens with
/// rows while it refreshes them in the background rather than drawing an empty loading list.
#[derive(Clone, Debug, Default)]
pub struct WorktreeListCache {
    lists: Vec<(
        Option<crate::session::remote::RemoteTarget>,
        String,
        Vec<crate::git::worktrees::WorktreeInfo>,
    )>,
}

impl WorktreeListCache {
    /// Repositories remembered at once; the least recently listed is dropped first.
    const CAPACITY: usize = 8;

    pub fn get(
        &self,
        target: Option<&crate::session::remote::RemoteTarget>,
        cwd: &str,
    ) -> Option<&[crate::git::worktrees::WorktreeInfo]> {
        self.lists
            .iter()
            .find(|(host, repo, _)| host.as_ref() == target && repo == cwd)
            .map(|(_, _, list)| list.as_slice())
    }

    /// The list of the repository `cwd` belongs to: its own entry, or a list that has `cwd` as one
    /// of its checkouts. Every checkout of a repository lists the same worktrees, so moving into a
    /// sibling checkout needs no new request before it can show them.
    pub fn get_repository(
        &self,
        target: Option<&crate::session::remote::RemoteTarget>,
        cwd: &str,
    ) -> Option<&[crate::git::worktrees::WorktreeInfo]> {
        self.get(target, cwd).or_else(|| {
            self.lists
                .iter()
                .find(|(host, _, list)| {
                    host.as_ref() == target && list.iter().any(|tree| tree.path == cwd)
                })
                .map(|(_, _, list)| list.as_slice())
        })
    }

    pub fn put(
        &mut self,
        target: Option<crate::session::remote::RemoteTarget>,
        cwd: String,
        list: Vec<crate::git::worktrees::WorktreeInfo>,
    ) {
        self.forget(target.as_ref(), &cwd);
        self.lists.insert(0, (target, cwd, list));
        self.lists.truncate(Self::CAPACITY);
    }

    pub fn forget(&mut self, target: Option<&crate::session::remote::RemoteTarget>, cwd: &str) {
        self.lists
            .retain(|(host, repo, _)| !(host.as_ref() == target && repo == cwd));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeFormField {
    Branch,
    Base,
    Path,
}

impl WorktreeFormField {
    pub const ORDER: [Self; 3] = [Self::Branch, Self::Base, Self::Path];

    pub fn label(self) -> &'static str {
        match self {
            Self::Branch => "Branch",
            Self::Base => "Base",
            Self::Path => "Path",
        }
    }
}

pub struct WorktreeFormState {
    pub branch: TextInput,
    pub base: TextInput,
    pub path: TextInput,
    pub focus: WorktreeFormField,
    pub path_edited: bool,
    pub preview_revision: u64,
    pub pending_preview: Option<u64>,
    /// The repository's top-level directory the previewed path would add, when Git does not
    /// ignore it.
    pub unignored: Option<String>,
    pub pending_exclude: Option<u64>,
    pub error: Option<String>,
}

impl WorktreeFormState {
    pub fn new() -> Self {
        Self {
            branch: TextInput::new(""),
            base: TextInput::new("HEAD"),
            path: TextInput::new(""),
            focus: WorktreeFormField::Branch,
            path_edited: false,
            preview_revision: 0,
            pending_preview: None,
            unignored: None,
            pending_exclude: None,
            error: None,
        }
    }

    pub fn input(&self, field: WorktreeFormField) -> &TextInput {
        match field {
            WorktreeFormField::Branch => &self.branch,
            WorktreeFormField::Base => &self.base,
            WorktreeFormField::Path => &self.path,
        }
    }

    pub fn input_mut(&mut self, field: WorktreeFormField) -> &mut TextInput {
        match field {
            WorktreeFormField::Branch => &mut self.branch,
            WorktreeFormField::Base => &mut self.base,
            WorktreeFormField::Path => &mut self.path,
        }
    }

    pub fn cycle_focus(&mut self, forward: bool) {
        let position = Self::field_index(self.focus);
        let next = if forward {
            (position + 1) % 3
        } else {
            (position + 2) % 3
        };
        self.focus = WorktreeFormField::ORDER[next];
    }

    fn field_index(field: WorktreeFormField) -> usize {
        WorktreeFormField::ORDER
            .iter()
            .position(|item| *item == field)
            .unwrap()
    }
}

impl Default for WorktreeFormState {
    fn default() -> Self {
        Self::new()
    }
}

/// Git work continues when the picker closes; completion is still delivered as a toast.
pub struct WorktreeOperation {
    pub request_id: u64,
    /// The attachment that sent the request, which need not still be the foreground one.
    pub epoch: u64,
    /// The connection the reply will come back on. Once that attachment is gone or has
    /// reconnected, no reply can arrive.
    pub connection: crate::session::client::ConnectionToken,
    pub cwd: String,
    pub kind: WorktreeOperationKind,
    /// Open the new checkout's session when the create finishes, as long as its attachment is
    /// still in front. Set when the create was submitted from a form nothing else stays open for.
    pub open_when_done: bool,
}

pub enum WorktreeOperationKind {
    Create { branch: String },
    Remove { path: String, force: bool },
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

/// Where one row of the global Agents view points, and therefore what opening it has to do.
///
/// The two variants are the same boundary the host monitor draws everywhere else. The session in
/// front of the user is *here*: its panes are live, and landing on one is a focus change. Anything
/// else is *elsewhere*, known only through its host monitor's semantic summaries — enough to say
/// that something wants attention and where it is, and never enough to show it without attaching
/// to that session first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentLocation {
    Here {
        pane: PaneId,
        /// The published row within the pane, for a program running several agents.
        row: Option<String>,
    },
    Elsewhere {
        target: crate::session::remote::RemoteTarget,
        session: String,
        pane: PaneId,
        row: Option<String>,
    },
}

impl AgentLocation {
    /// The pane this row lands on, once whatever owns it is on screen.
    pub fn pane(&self) -> PaneId {
        match self {
            Self::Here { pane, .. } | Self::Elsewhere { pane, .. } => *pane,
        }
    }

    pub fn row(&self) -> Option<&str> {
        match self {
            Self::Here { row, .. } | Self::Elsewhere { row, .. } => row.as_deref(),
        }
    }
}

/// The global Agents view: every agent this client knows about, wherever it is running.
pub struct AgentPickerState {
    pub input: TextInput,
    /// The highlighted row, held by location rather than by index. Rows are rebuilt from live pane
    /// state and from host-monitor polls that land on their own schedule, so an index would move
    /// the cursor onto a different agent between one frame and the next — which is how a jump ends
    /// up somewhere the user never selected.
    pub selected: Option<AgentLocation>,
}

impl AgentPickerState {
    pub fn new(selected: Option<AgentLocation>) -> Self {
        Self {
            input: TextInput::new(""),
            selected,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemotePickerMode {
    Hosts,
    HostSessions {
        target: crate::session::remote::RemoteTarget,
    },
}

/// Whether the host editor is creating a durable host entry or correcting one that already exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostFormMode {
    Add,
    /// Editing the host that currently has this exact identity. Submitting rewrites it in place.
    Edit(crate::session::remote::RemoteTarget),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostFormField {
    Host,
    User,
    Port,
}

impl HostFormField {
    pub const ORDER: [Self; 3] = [Self::Host, Self::User, Self::Port];

    pub fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::User => "Username",
            Self::Port => "Port",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Host => "host / alias / ssh://…",
            Self::User => "leave empty to let SSH decide",
            Self::Port => "22",
        }
    }
}

/// The *Add host* / *Edit host* form: host, login, port, always all three.
///
/// The host line is allowed to answer more than its own question. `adam@workbox` is how people
/// write an SSH endpoint, and typing it fills the login in and takes that line out of the user's
/// hands rather than leaving two places that disagree about who logs in. Clearing the `user@`
/// hands the line back with whatever they had typed there still in it — the field they own and the
/// value derived from the host are kept apart precisely so switching between them loses nothing.
///
/// No password lives here. SSH asks for one when it needs one, through the askpass modal, and rozi
/// never stores it.
pub struct HostFormState {
    pub mode: HostFormMode,
    pub host: TextInput,
    /// The login the user typed on its own line. Shadowed, never overwritten, while the host line
    /// carries one of its own.
    pub user: TextInput,
    pub port: TextInput,
    pub focus: HostFormField,
    pub error: Option<String>,
}

impl HostFormState {
    pub fn add(initial: impl AsRef<str>) -> Self {
        Self {
            mode: HostFormMode::Add,
            host: TextInput::new(initial.as_ref()),
            user: TextInput::new(""),
            port: TextInput::new(""),
            focus: HostFormField::Host,
            error: None,
        }
    }

    pub fn edit(target: &crate::session::remote::RemoteTarget) -> Self {
        Self {
            mode: HostFormMode::Edit(target.clone()),
            host: TextInput::new(target.host()),
            user: TextInput::new(target.user().unwrap_or_default()),
            port: TextInput::new(
                target
                    .port()
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
            ),
            focus: HostFormField::Host,
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.mode {
            HostFormMode::Add => "Add host",
            HostFormMode::Edit(_) => "Edit host",
        }
    }

    pub fn input(&self, field: HostFormField) -> &TextInput {
        match field {
            HostFormField::Host => &self.host,
            HostFormField::User => &self.user,
            HostFormField::Port => &self.port,
        }
    }

    pub fn input_mut(&mut self, field: HostFormField) -> &mut TextInput {
        match field {
            HostFormField::Host => &mut self.host,
            HostFormField::User => &mut self.user,
            HostFormField::Port => &mut self.port,
        }
    }

    /// The login the host line spells out, if it spells one out.
    ///
    /// Parses first so `ssh://adam@box:22` is understood as well as `adam@box`, and falls back to
    /// the text before `@` for a line still being typed — `adam@` is not a valid target yet, but it
    /// has already said who logs in.
    pub fn host_login(&self) -> Option<&str> {
        let text = self.host.text().trim();
        if crate::session::remote::parse_remote_target(text).is_ok() {
            let (login, _) = text.rsplit_once('@')?;
            let login = login.rsplit_once("//").map_or(login, |(_, rest)| rest);
            return (!login.is_empty()).then_some(login);
        }
        text.split_once('@')
            .map(|(login, _)| login)
            .filter(|login| !login.is_empty())
    }

    /// Whether the *Username* line is the host line's to fill. Read-only while it is, so the two
    /// can never disagree about who logs in.
    pub fn user_is_locked(&self) -> bool {
        self.host_login().is_some()
    }

    /// The lines the cursor can reach, in the order they are drawn. A *Username* the host line
    /// already answered is skipped: there is nothing to type into it.
    pub fn reachable_fields(&self) -> Vec<HostFormField> {
        HostFormField::ORDER
            .into_iter()
            .filter(|field| !(*field == HostFormField::User && self.user_is_locked()))
            .collect()
    }

    /// Move the cursor one line on, wrapping.
    ///
    /// The form moves its own cursor rather than leaving it to `Tab` traversal: the framework's
    /// traversal order is its own, and on this dialog it ran host → port → username, which is not
    /// the order the three lines are read in.
    pub fn cycle_focus(&mut self, forward: bool) {
        let fields = self.reachable_fields();
        let Some(index) = fields.iter().position(|field| *field == self.focus) else {
            self.focus = HostFormField::Host;
            return;
        };
        let len = fields.len();
        let step = if forward { 1 } else { len - 1 };
        self.focus = fields[(index + step) % len];
    }

    /// What the *Username* line shows: the host line's login while that governs, else the user's own.
    pub fn shown_user(&self) -> &str {
        self.host_login().unwrap_or_else(|| self.user.text())
    }

    /// Build the target the form describes, or say what is wrong with it.
    ///
    /// The host line wins wherever it is specific, since that is the line the user just spelled the
    /// endpoint into; the other two fill in what it left open.
    pub fn target(&self) -> Result<crate::session::remote::RemoteTarget, String> {
        let parsed = crate::session::remote::parse_remote_target(self.host.text())?;
        let port = match self.port.text().trim() {
            "" => parsed.port(),
            raw => Some(
                raw.parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| format!("invalid port `{raw}`"))?,
            ),
        };
        let user = parsed
            .user()
            .or_else(|| Some(self.user.text().trim()).filter(|user| !user.is_empty()));
        crate::session::remote::RemoteTarget::from_parts(parsed.host(), user, port)
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
    /// The host the in-flight probe is contacting, which is *not* necessarily the highlighted one:
    /// connecting no longer freezes the list, so the user is free to read the other rows while one
    /// machine is being reached. The result is matched against this rather than against the
    /// selection, or moving the cursor would orphan the answer.
    pub probe_target: Option<crate::session::remote::RemoteTarget>,
    pub host_form: Option<HostFormState>,
    pub pending_forget: Option<crate::session::remote::RemoteTarget>,
    pub pending_kill: Option<RemoteSessionIdentity>,
    pub pending_restart: Option<RemoteSessionIdentity>,
    /// The session `startup = "last"` remembered on this host, waiting on discovery to say whether
    /// the host still has it. Attached when the first successful probe lists it, dropped otherwise —
    /// `last` reopens a session, it never revives one, so a name the host does not report leaves the
    /// user on this picker.
    ///
    /// Consumed by that first probe whatever it finds, so a host reopened by hand later is a plain
    /// browse rather than a second, surprising auto-attach.
    pub startup_resume: Option<String>,
    /// Whether the next successful probe should step straight into `Sessions · <host>` instead of
    /// staying on the host list.
    ///
    /// Set only by a launch that named a machine (`--remote <host>`). Reaching a host and opening it
    /// are two different acts, and an ordinary `Enter` does the first one and stops so the user can
    /// see it worked; a launch already said which machine it wants to work on, so it does both.
    pub auto_open: bool,
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
            probe_target: None,
            host_form: None,
            pending_forget: None,
            pending_kill: None,
            pending_restart: None,
            startup_resume: None,
            auto_open: false,
        }
    }

    /// Whether a probe this picker started is still out at `target`.
    pub fn is_connecting(&self, target: &crate::session::remote::RemoteTarget) -> bool {
        matches!(self.host_probe, super::HostProbe::InFlight)
            && self.probe_target.as_ref() == Some(target)
    }

    pub fn enter_host_sessions(&mut self, target: crate::session::remote::RemoteTarget) {
        self.selected_host = Some(target.clone());
        self.mode = RemotePickerMode::HostSessions { target };
        self.sessions.clear();
        self.selected_session = None;
        self.host_probe = super::HostProbe::Idle;
        self.probe_target = None;
        self.pending_forget = None;
        self.pending_kill = None;
        self.pending_restart = None;
        self.host_form = None;
    }

    pub fn return_to_hosts(&mut self) {
        self.mode = RemotePickerMode::Hosts;
        self.sessions.clear();
        self.selected_session = None;
        self.host_probe = super::HostProbe::Idle;
        self.probe_target = None;
        self.pending_kill = None;
        self.pending_restart = None;
        self.host_form = None;
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

/// The open *Collaborators* dialog: the roster of everyone else on the session. The
/// session-wide controls it sits beside (request control, input lock, takeover) are ordinary
/// command-palette entries, not part of this dialog.
pub struct CollaborationState {
    pub selected: usize,
    /// The client awaiting a second key press to confirm its removal, held by id rather than by
    /// roster position: the roster is server-pushed and can reorder under an armed row, and a
    /// confirmation that lands on whoever slid into that slot is the one mistake this arming exists
    /// to prevent. Cleared by moving the highlight or closing the dialog.
    pub pending_kick: Option<crate::layout::shared::ClientId>,
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
    /// Reconnect attachment that owns this prompt. Used to dismiss a prompt already on screen when
    /// that attempt is cancelled or reaches its deadline.
    pub attach_epoch: Option<u64>,
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
mod host_form_tests {
    use super::*;
    use crate::session::remote::RemoteTarget;

    fn typed(host: &str, user: &str, port: &str) -> HostFormState {
        let mut form = HostFormState::add(host);
        form.user = TextInput::new(user);
        form.port = TextInput::new(port);
        form
    }

    /// The host line is allowed to answer the login question, and while it does the *Username* line
    /// is not the user's to contradict.
    #[test]
    fn a_login_in_the_host_line_fills_and_locks_the_username() {
        let form = typed("adam@10.0.0.5", "", "");
        assert_eq!(form.host_login(), Some("adam"));
        assert!(form.user_is_locked());
        assert_eq!(form.shown_user(), "adam");

        let bare = typed("workbox", "", "");
        assert!(!bare.user_is_locked());
        assert_eq!(bare.host_login(), None);
    }

    /// A line still being typed has already said who logs in, so it locks before it parses.
    #[test]
    fn a_half_typed_login_locks_too() {
        let form = typed("adam@", "", "");
        assert_eq!(form.host_login(), Some("adam"));
        assert!(form.user_is_locked());
    }

    /// The `ssh://` spelling is the same statement, and reads the same way.
    #[test]
    fn an_ssh_url_login_is_recognized_as_one() {
        let form = typed("ssh://adam@workbox:2222", "", "");
        assert_eq!(form.host_login(), Some("adam"));
        assert_eq!(
            form.target().expect("a valid endpoint"),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "workbox".into(),
                port: Some(2222),
            }
        );
    }

    /// Shadowed, never overwritten: clearing the `user@` hands the line back with what was in it.
    #[test]
    fn the_users_own_login_survives_being_shadowed() {
        let mut form = typed("adam@workbox", "bob", "");
        assert_eq!(form.shown_user(), "adam", "the host line governs");
        assert_eq!(
            form.target().expect("valid").user(),
            Some("adam"),
            "and it is what gets saved"
        );

        form.host = TextInput::new("workbox");
        assert!(!form.user_is_locked());
        assert_eq!(form.shown_user(), "bob", "the line comes back as it was");
        assert_eq!(form.target().expect("valid").user(), Some("bob"));
    }

    /// The host line wins where it is specific; the other lines fill in what it left open.
    #[test]
    fn the_host_line_wins_and_the_others_fill_the_gaps() {
        assert_eq!(
            typed("workbox", "adam", "2222").target().expect("valid"),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "workbox".into(),
                port: Some(2222),
            }
        );
        assert_eq!(
            typed("adam@workbox:22", "", "").target().expect("valid"),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "workbox".into(),
                port: Some(22),
            }
        );
        assert_eq!(
            typed("workbox", "", "").target().expect("valid"),
            RemoteTarget::Alias("workbox".into()),
            "nothing overridden stays an ssh_config alias"
        );
        assert!(typed("workbox", "", "nope").target().is_err());
    }

    /// Tab moves down the lines as they are drawn, wrapping, and Shift+Tab back up.
    #[test]
    fn the_cursor_cycles_the_lines_in_the_order_they_are_read() {
        let mut form = typed("workbox", "", "");
        assert_eq!(form.focus, HostFormField::Host);
        for expected in [
            HostFormField::User,
            HostFormField::Port,
            HostFormField::Host,
        ] {
            form.cycle_focus(true);
            assert_eq!(form.focus, expected);
        }
        form.cycle_focus(false);
        assert_eq!(form.focus, HostFormField::Port, "and back the other way");
    }

    /// A *Username* the host line answered is skipped: there is nothing to type into it.
    #[test]
    fn the_cursor_skips_a_username_the_host_line_owns() {
        let mut form = typed("adam@workbox", "", "");
        assert_eq!(
            form.reachable_fields(),
            vec![HostFormField::Host, HostFormField::Port]
        );
        form.cycle_focus(true);
        assert_eq!(form.focus, HostFormField::Port);
        form.cycle_focus(true);
        assert_eq!(form.focus, HostFormField::Host);
    }

    /// Editing shows what is stored, split back across the three lines.
    #[test]
    fn editing_fills_every_line_from_the_stored_target() {
        let target = RemoteTarget::Url {
            user: Some("adam".into()),
            host: "workbox".into(),
            port: Some(2222),
        };
        let form = HostFormState::edit(&target);
        assert_eq!(form.host.text(), "workbox");
        assert_eq!(form.user.text(), "adam");
        assert_eq!(form.port.text(), "2222");
        assert!(
            !form.user_is_locked(),
            "the host line holds only the host, so the login line is the user's"
        );
        assert_eq!(form.target().expect("valid"), target);
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
    pub query: String,
}

impl LayoutPickerState {
    pub fn new(selected: usize, original: super::LayoutKind) -> Self {
        Self {
            selected,
            original,
            query: String::new(),
        }
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

/// One tab a `pick` caller declares up front. Rows arrive per tab afterwards.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickTab {
    /// Names the tab in row snapshots, and rides back as `tab` on selections, actions, and tab
    /// switches. Row ids only need to be unique within their tab.
    pub id: String,
    /// Strip text. Defaults to `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One extra key the caller offers alongside select and cancel.
///
/// Actions are what turn a picker from a menu into a working surface: delete the branch under the
/// cursor and push a fresh list, or open a prompt and create one. They are declared up front so the
/// footer can advertise them the way every built-in picker advertises its own chords.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickAction {
    /// Returned to the caller as `action`.
    pub id: String,
    /// Chord that fires it, in the same spelling `[keys]` uses (`ctrl-n`).
    pub key: String,
    /// Footer text, e.g. `new branch`.
    pub label: String,
    /// When set, the key opens a text prompt and the entered text rides back as `input`.
    /// A string is the title. An object may also set placeholder, a seed value, and masking.
    /// Cancelling the prompt returns to the picker without reporting anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PickPromptSpec>,
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

/// How a pick action describes the stacked text prompt.
///
/// The string form is the title. The object form is additive: every existing `"prompt":"Title"`
/// caller stays valid.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum PickPromptSpec {
    Title(String),
    Fields(PickPromptFields),
}

/// Object form of [`PickPromptSpec`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickPromptFields {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Hide typed characters on screen. The submitted `input` is still plaintext on the stream.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub masked: bool,
}

impl PickPromptSpec {
    pub fn title(&self) -> &str {
        match self {
            Self::Title(title) => title,
            Self::Fields(fields) => &fields.title,
        }
    }

    pub fn placeholder(&self) -> &str {
        match self {
            Self::Title(_) => "",
            Self::Fields(fields) => fields.placeholder.as_deref().unwrap_or(""),
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Self::Title(_) => "",
            Self::Fields(fields) => fields.value.as_deref().unwrap_or(""),
        }
    }

    pub fn masked(&self) -> bool {
        match self {
            Self::Title(_) => false,
            Self::Fields(fields) => fields.masked,
        }
    }
}

/// An open prompt raised by a [`PickAction`], holding the picker underneath it.
pub struct PickPrompt {
    /// Index into [`PickState::actions`].
    pub action: usize,
    pub title: String,
    pub placeholder: String,
    pub masked: bool,
    pub input: TextInput,
}

/// A tab of the Extensions manager.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtensionsTab {
    #[default]
    Installed,
    Discover,
}

impl ExtensionsTab {
    /// The tab strip's order, shared by the strip, the keyboard cycle, and `ExtensionsTabSelected`.
    pub const ORDER: [Self; 2] = [Self::Installed, Self::Discover];

    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Discover => "Discover",
        }
    }

    pub fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ORDER.get(index).copied().unwrap_or_default()
    }

    /// The tab `steps` places along the strip, wrapping at both ends.
    pub fn stepped(self, steps: isize) -> Self {
        let count = Self::ORDER.len();
        Self::from_index((self.index() + count.wrapping_add_signed(steps)) % count)
    }
}

pub struct ExtensionsState {
    pub tab: ExtensionsTab,
    pub entries: Vec<crate::config::ExtensionInfo>,
    pub merged: BTreeMap<String, crate::config::ExtensionSettings>,
    /// Selected row of the Installed tab, as an index into `entries`.
    pub selected: usize,
    /// Selected row of the Discover tab, as an index into `catalog_entries`.
    pub catalog_selected: Option<usize>,
    /// Filters the active tab. Kept here rather than in the widget, so it survives the report and
    /// prompts that replace the picker.
    pub query: TextInput,
    /// Installation path awaiting a second Ctrl+K. The path survives rescans that reorder rows
    /// and distinguishes duplicate manifest ids.
    pub pending_remove: Option<String>,
    pub detail: Option<ExtensionDetailState>,
    pub install_prompt: Option<ExtensionInstallPromptState>,
    pub catalog_entries: Vec<crate::extension_catalog::CatalogEntry>,
    pub catalog_error: Option<String>,
    pub catalog_epoch: u64,
    /// A fetch of the index is running. `catalog_entries` may meanwhile hold the cached index.
    pub catalog_loading: bool,
    pub catalog_detail: Option<CatalogExtensionDetailState>,
    pub(crate) installation_kinds: BTreeMap<String, crate::extension_installation::InstallKind>,
    /// Background update checks of Git installations, by extension id. An id without an entry
    /// has not been checked since the picker opened.
    pub update_checks: BTreeMap<String, ExtensionUpdateCheck>,
    pub update_check_epoch: u64,
    pub(crate) updating_id: Option<String>,
    pub(crate) manifest_entries: BTreeSet<String>,
    pub(crate) removable_entries: BTreeSet<String>,
}

/// Where one Git installation's update check stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionUpdateCheck {
    Checking,
    Current,
    Available {
        revision: String,
        /// The version the remote manifest declares, when it declares a usable one.
        version: Option<String>,
    },
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtensionPickerRow {
    Installed(usize),
    Catalog(usize),
}

/// A snapshot of the discovery entry being reviewed. A refresh may reorder or drop catalog rows
/// while the report is open, so the report never refers back to them by position.
pub struct CatalogExtensionDetailState {
    pub entry: crate::extension_catalog::CatalogEntry,
    pub error: Option<String>,
}

pub struct ExtensionInstallPromptState {
    pub input: TextInput,
    pub error: Option<String>,
    pub error_scroll_offset: usize,
    pub error_scroll_max: Option<usize>,
}

/// An extension installation in flight, from a discovery report or the install prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionInstall {
    /// The discovery repository being installed, or `None` for a source typed into the prompt.
    pub repository: Option<String>,
    /// What the progress modal names: the entry's title, or the typed source.
    pub label: String,
    /// A line under the label, such as the repository and commit being installed.
    pub detail: Option<String>,
    /// The user hid the progress modal. The installation continues and reports when it is done.
    pub hidden: bool,
}

pub struct ExtensionDetailState {
    pub path: String,
    pub(crate) sections: Vec<crate::config::ReportSection>,
}

pub struct PickState {
    pub id: u64,
    pub extension: Option<crate::config::ExtensionProvenance>,
    pub title: String,
    pub placeholder: String,
    /// Producer copy when the row list is empty and the filter is empty. Filter misses use
    /// Rozi's `"No matches"` instead. Omitted leaves the list's ordinary empty appearance.
    pub empty: Option<String>,
    /// Caller-chosen modal width in columns, clamped on the way in.
    pub width: u16,
    pub actions: Vec<PickAction>,
    pub prompt: Option<PickPrompt>,
    /// The `confirm` action awaiting its second press, with the row it was armed on. Held by row
    /// id rather than position: the caller can push a new list under an armed row, and a
    /// confirmation landing on whoever slid into that slot is the mistake arming exists to stop.
    /// Always about the active page; switching tabs disarms it.
    pub pending_action: Option<(usize, String)>,
    /// One page per declared tab, or a single untitled page when the caller declared none. Never
    /// empty, so an untabbed picker is just the one-page case rather than a separate mode.
    pub pages: Vec<PickPage>,
    /// Index into `pages`.
    pub active: usize,
    pub reply: PickReply,
}

/// Reply lines a picker may queue while the stream's writer waits on a slow reader. Actions and
/// tab switches report without closing, so a burst of them has to fit. Past this the newest
/// non-terminal line is dropped; the terminal line never is, see [`PickReply`].
pub const PICK_REPLY_BACKLOG: usize = 64;

/// Most tabs one picker keeps. A strip past this is unusable anyway, and tabs are held on the UI
/// thread, so a malformed producer must not get to declare thousands.
pub const MAX_PICK_TABS: usize = 32;

/// The tab ids a picker actually keeps, in order: nonempty, first occurrence only, at most
/// [`MAX_PICK_TABS`]. Shared by the UI and the CLI bridge so both agree on where opening rows go.
pub fn usable_pick_tabs(tabs: &[PickTab]) -> Vec<&PickTab> {
    let mut seen = std::collections::HashSet::new();
    tabs.iter()
        .filter(|tab| !tab.id.is_empty() && seen.insert(tab.id.as_str()))
        .take(MAX_PICK_TABS)
        .collect()
}

/// Where a picker writes to its caller.
///
/// Events that keep the picker open go through a bounded queue and may be dropped when the caller
/// stops reading. The one terminal line - a selection, a cancellation, or a closing action - has
/// a slot of its own, so it is never lost to a full queue: once rozi closes a picker, the caller
/// hears why. The UI thread never blocks on either.
///
/// `Clone` only because it travels inside `Msg`; the one held by `PickState` is the one that
/// reports.
#[derive(Clone)]
pub struct PickReply {
    events: std::sync::mpsc::SyncSender<String>,
    terminal: std::sync::mpsc::SyncSender<String>,
}

/// The stream side of a [`PickReply`], yielding lines in the order they were sent.
pub struct PickReplyReceiver {
    events: std::sync::mpsc::Receiver<String>,
    terminal: std::sync::mpsc::Receiver<String>,
}

impl PickReply {
    pub fn channel() -> (Self, PickReplyReceiver) {
        let (events, events_rx) = std::sync::mpsc::sync_channel(PICK_REPLY_BACKLOG);
        let (terminal, terminal_rx) = std::sync::mpsc::sync_channel(1);
        (
            Self { events, terminal },
            PickReplyReceiver {
                events: events_rx,
                terminal: terminal_rx,
            },
        )
    }

    /// Report something that leaves the picker open. Dropped if the queue is full.
    pub fn event(&self, payload: &serde_json::Value) {
        let _ = self.events.try_send(format!("{payload}\n"));
    }

    /// Report how the picker ended. Consumes the reply: nothing can follow a terminal line.
    pub fn finish(self, payload: &serde_json::Value) {
        // Only ever sent once into an empty slot of one, so this cannot find it full.
        let _ = self.terminal.try_send(format!("{payload}\n"));
    }
}

impl PickReplyReceiver {
    /// The next line, blocking. Events drain first; the terminal line follows once the picker has
    /// dropped its [`PickReply`], which is after every event it sent. `None` once both are done.
    pub fn recv(&self) -> Option<String> {
        self.events
            .recv()
            .ok()
            .or_else(|| self.terminal.recv().ok())
    }

    /// The next line already sent, without blocking, in the same order as [`Self::recv`].
    pub fn try_recv(&self) -> Option<String> {
        self.events
            .try_recv()
            .ok()
            .or_else(|| self.terminal.try_recv().ok())
    }
}

/// One tab of a picker: its own rows, filter, and highlight.
///
/// The filter is per page on purpose. Tabs are related views rather than one list sorted into
/// buckets, so text typed into Branches is still there after a look at Worktrees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickPage {
    /// The caller's tab id, echoed back on selections and actions. `None` only for the implicit
    /// page of a picker that declared no tabs.
    pub tab: Option<String>,
    pub label: String,
    pub rows: Vec<PickRow>,
    pub selected: usize,
    /// The live filter text, mirrored out of the palette so a rebuild can restore it.
    pub query: String,
    /// What the rebuilt palette is seeded with. Updated only when the page is about to be
    /// remounted (a prompt covering it, or a tab switch back to it), so it stays stable while
    /// typing - feeding the live mirror back as `initial_query` would change that prop on every
    /// keystroke and risk re-seeding the field mid-edit.
    pub restore_query: String,
}

impl PickState {
    pub fn page(&self) -> &PickPage {
        &self.pages[self.active]
    }

    pub fn page_mut(&mut self) -> &mut PickPage {
        &mut self.pages[self.active]
    }

    /// Whether the caller declared tabs, which is what shows the strip. A single declared tab
    /// still gets one, so a producer that adds pages later does not change the picker's shape.
    pub fn tabbed(&self) -> bool {
        self.pages.first().is_some_and(|page| page.tab.is_some())
    }
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

    #[test]
    fn pick_prompt_spec_accepts_a_title_string_or_an_object() {
        let string: PickAction = serde_json::from_str(
            r#"{"id":"create","key":"ctrl-n","label":"new","prompt":"Command"}"#,
        )
        .expect("string prompt");
        assert_eq!(string.prompt, Some(PickPromptSpec::Title("Command".into())));

        let object: PickAction = serde_json::from_str(
            r#"{"id":"edit","key":"ctrl-e","label":"edit","prompt":{"title":"Edit command","placeholder":"git status","value":"git status --short","masked":true}}"#,
        )
        .expect("object prompt");
        let spec = object.prompt.expect("prompt");
        assert_eq!(spec.title(), "Edit command");
        assert_eq!(spec.placeholder(), "git status");
        assert_eq!(spec.value(), "git status --short");
        assert!(spec.masked());
    }
}

#[cfg(test)]
mod worktree_cache_tests {
    use super::WorktreeListCache;

    fn tree(path: &str) -> crate::git::worktrees::WorktreeInfo {
        crate::git::worktrees::WorktreeInfo {
            path: path.into(),
            branch: None,
            detached: true,
            bare: false,
            prunable: false,
            linked: true,
            locked: false,
        }
    }

    #[test]
    fn the_cache_keeps_recent_repositories_per_host() {
        let mut cache = WorktreeListCache::default();
        let host = crate::session::remote::RemoteTarget::Alias("box".into());
        cache.put(None, "/repo".into(), vec![tree("/repo")]);
        cache.put(Some(host.clone()), "/repo".into(), vec![tree("/remote")]);
        assert_eq!(cache.get(None, "/repo").unwrap()[0].path, "/repo");
        assert_eq!(cache.get(Some(&host), "/repo").unwrap()[0].path, "/remote");

        cache.put(None, "/repo".into(), vec![tree("/repo"), tree("/wt")]);
        assert_eq!(cache.get(None, "/repo").unwrap().len(), 2);
        for index in 0..WorktreeListCache::CAPACITY {
            cache.put(None, format!("/other{index}"), Vec::new());
        }
        assert!(
            cache.get(None, "/repo").is_none(),
            "the oldest repository is dropped"
        );
        cache.forget(None, "/other0");
        assert!(cache.get(None, "/other0").is_none());
    }

    #[test]
    fn a_sibling_checkout_finds_its_repositorys_list() {
        let mut cache = WorktreeListCache::default();
        let host = crate::session::remote::RemoteTarget::Alias("box".into());
        cache.put(
            None,
            "/repo".into(),
            vec![tree("/repo"), tree("/repo-wt/feat")],
        );
        assert_eq!(
            cache.get(None, "/repo-wt/feat"),
            None,
            "not filed under the checkout"
        );
        assert_eq!(
            cache.get_repository(None, "/repo-wt/feat").map(<[_]>::len),
            Some(2)
        );
        assert_eq!(
            cache.get_repository(Some(&host), "/repo-wt/feat"),
            None,
            "another host"
        );
        assert_eq!(cache.get_repository(None, "/elsewhere"), None);
    }
}
