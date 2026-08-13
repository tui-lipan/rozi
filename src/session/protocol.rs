//! Session wire protocol.
//!
//! # Version negotiation
//!
//! [`PROTOCOL_VERSION`] is this build's maximum supported wire version.
//! [`MIN_SUPPORTED_PROTOCOL`] is the oldest version this build can still speak.
//!
//! Clients advertise both on [`ClientMessage::Attach`] / [`ClientMessage::Query`]. The server
//! chooses `effective = min(client_max, server_max)` and accepts only when that value sits in both
//! sides' supported ranges. The negotiated value is echoed as `effective_protocol` on
//! [`ServerMessage::Attached`] / [`ServerMessage::SessionInfo`].
//!
//! The two bounds are equal, so a peer either speaks this exact version or is turned away. The
//! negotiation machinery stays regardless: the day a range is worth supporting is the day it has to
//! already be on the wire.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::runtime_metrics::ServerRuntimeMetrics;
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

/// Maximum wire protocol version this build speaks.
///
/// Every peer is built from this same tree, so there is one version and no skew to straddle. Bump
/// this for any wire change, additive or not; a mismatched peer is rejected at the handshake rather
/// than shimmed. Per-message capability gates are not worth keeping while that holds — a gate below
/// the floor is a branch that cannot be taken.
pub const PROTOCOL_VERSION: u32 = 1;
/// Oldest wire protocol version this build can still speak.
pub const MIN_SUPPORTED_PROTOCOL: u32 = PROTOCOL_VERSION;
/// `code` on the [`ServerMessage::Error`] a client receives just before the server closes it for
/// being evicted. Distinguishes a removal from a dropped connection, which the client would
/// otherwise answer by reconnecting straight back into the session it was removed from.
pub const EVICTED_ERROR_CODE: &str = "evicted";
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
pub const PANE_STATUS_MAX_LEN: usize = 64;
pub const PANE_STATUS_REASON_MAX_LEN: usize = 256;
/// On-wire size of a pane byte frame besides the payload: 4-byte length, 1-byte kind, 13-byte
/// header (`pane_id` + `generation` + locality).
pub const PANE_FRAME_OVERHEAD: usize = 18;
const PANE_FRAME_HEADER_LEN: usize = 13;
const FRAME_KIND_CONTROL_JSON: u8 = 1;
const FRAME_KIND_PANE_OUTPUT: u8 = 2;
const FRAME_KIND_PANE_INPUT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePalette {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub ansi: [Color; 16],
}

impl From<TerminalColorPalette> for WirePalette {
    fn from(palette: TerminalColorPalette) -> Self {
        Self {
            foreground: palette.foreground,
            background: palette.background,
            ansi: palette.ansi,
        }
    }
}

impl From<WirePalette> for TerminalColorPalette {
    fn from(palette: WirePalette) -> Self {
        Self {
            foreground: palette.foreground,
            background: palette.background,
            ansi: palette.ansi,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneMeta {
    pub pane_id: PaneId,
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub pid: Option<u32>,
    pub title: Option<String>,
    /// Account that owns the session server and launched the pane's original shell. Additive and
    /// optional so protocol-12/13 peers that predate this field remain compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_user: Option<String>,
    pub exited: Option<i32>,
    pub logging: bool,
    /// Authoritative pane runtime state: CWD, command lifecycle,
    /// and foreground executable, as tracked server-side from shell-integration OSC reports and
    /// native process-inspection fallbacks. Present in every `Attached`/`SpawnResult`-adjacent
    /// pane listing so a freshly attached client starts with current state rather than waiting for
    /// the next [`ServerMessage::PaneRuntimeChanged`].
    pub runtime: PaneRuntimeState,
}

/// Which precedence tier produced [`PaneRuntimeState::cwd`].
///
/// Tier order, most to least authoritative: [`ShellReport`](Self::ShellReport) (a local or remote
/// `OSC 7`/`OSC 9;9` report) -> [`ProcessInspector`](Self::ProcessInspector) (native `/proc` or
/// `libproc` inspection) -> [`LaunchDirectory`](Self::LaunchDirectory) (the directory the pane was
/// spawned with). A configured-fallback fourth tier from the plan's precedence table collapses
/// into `LaunchDirectory` here: the launch cwd a pane starts with is already itself resolved from
/// config/profile defaults, so there is no separate value a fourth tier could contribute once
/// launch has happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneCwdSource {
    #[default]
    Unknown,
    ShellReport,
    ProcessInspector,
    LaunchDirectory,
}

/// Mirrors [`TerminalCommandPhase`] on the wire (that framework type has no `serde` impls, since
/// it is UI-agnostic runtime state rather than persisted/transmitted data).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum PaneCommandPhase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Executing,
    Completed {
        exit_status: Option<i32>,
    },
}

pub mod pane_status {
    pub const WORKING: &str = "working";
    pub const BLOCKED: &str = "blocked";
    pub const DONE: &str = "done";
    pub const IDLE: &str = "idle";
}

/// Whether a status describes a quiescent agent rather than an active run.
pub fn status_is_quiescent(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.trim().eq_ignore_ascii_case(pane_status::IDLE)
            || value.trim().eq_ignore_ascii_case(pane_status::DONE)
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneStatus {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub set_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    Mastracode,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    QoderCli,
    Maki,
    Aider,
    Goose,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pi => "Pi",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::Cursor => "Cursor Agent",
            Self::Devin => "Devin CLI",
            Self::Antigravity => "Antigravity",
            Self::Cline => "Cline",
            Self::Omp => "OMP",
            Self::Mastracode => "Mastra Code",
            Self::OpenCode => "OpenCode",
            Self::GithubCopilot => "GitHub Copilot",
            Self::Kimi => "Kimi Code",
            Self::Kiro => "Kiro CLI",
            Self::Droid => "Droid",
            Self::Amp => "Amp",
            Self::Grok => "Grok",
            Self::Hermes => "Hermes",
            Self::Kilo => "Kilo Code",
            Self::QoderCli => "Qoder CLI",
            Self::Maki => "Maki",
            Self::Aider => "Aider",
            Self::Goose => "Goose",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetectedAgentState {
    Idle,
    Working,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedAgent {
    pub kind: AgentKind,
    pub state: DetectedAgentState,
}

fn detected_agent_status(detected: &DetectedAgent) -> &'static str {
    match detected.state {
        DetectedAgentState::Idle => pane_status::IDLE,
        DetectedAgentState::Working => pane_status::WORKING,
        DetectedAgentState::Blocked => pane_status::BLOCKED,
    }
}

/// One logical agent inside a pane that publishes several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSlot {
    /// Publisher-chosen identity, opaque to rozi and stable across updates.
    ///
    /// Never a position: tabs are reordered and closed, and an index-keyed run clock would hand
    /// one tab's elapsed time to whichever tab slid into its place.
    pub id: String,
    pub title: String,
    /// Same vocabulary and same sanitization as [`PaneStatus::value`].
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The slot the publisher is currently displaying; at most one is true.
    ///
    /// This is what lets a finish on a *background* slot raise an alert even while the pane is
    /// focused - looking at the pane only ever acknowledges the slot on screen.
    #[serde(default)]
    pub active: bool,
    /// Server-owned run start, mirroring [`PaneRuntimeState::work_started_at`] per slot. Whatever
    /// a publisher sends here is overwritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_started_at: Option<u64>,
}

/// The single state a pane shows for a set of published slots.
///
/// Severity order rather than recency: pane chrome answers "is anything in here demanding
/// attention", so one blocked slot outranks any number of working ones, and any working slot
/// outranks the ones that have finished. Returns `None` for an empty set, which is a pane that
/// publishes nothing rather than a pane whose agents are all idle.
pub fn aggregate_slot_state(slots: &[AgentSlot]) -> Option<DetectedAgentState> {
    let mut state = None;
    for slot in slots {
        let value = slot.status.trim();
        if value.eq_ignore_ascii_case(pane_status::BLOCKED) {
            return Some(DetectedAgentState::Blocked);
        }
        // A custom status is an active run by the same rule `status_is_quiescent` uses elsewhere.
        if !status_is_quiescent(Some(value)) {
            state = Some(DetectedAgentState::Working);
        } else if state.is_none() {
            state = Some(DetectedAgentState::Idle);
        }
    }
    state
}

/// The agent status shared by clients and the session server.
///
/// Explicit active reports remain authoritative, but a detected blocked prompt elevates over a
/// stale quiescent `idle`/`done` report. This prevents OpenCode permission prompts from reading as
/// completed while preserving reported-only status consumers.
pub fn effective_agent_status<'a>(
    reported: Option<&'a PaneStatus>,
    detected: Option<&'a DetectedAgent>,
) -> Option<&'a str> {
    let reported = reported.map(|status| status.value.as_str());
    if detected.is_some_and(|agent| agent.state == DetectedAgentState::Blocked)
        && status_is_quiescent(reported)
    {
        return Some(pane_status::BLOCKED);
    }
    reported.or_else(|| detected.map(detected_agent_status))
}

impl From<TerminalCommandPhase> for PaneCommandPhase {
    fn from(phase: TerminalCommandPhase) -> Self {
        match phase {
            TerminalCommandPhase::Unknown => Self::Unknown,
            TerminalCommandPhase::Prompt => Self::Prompt,
            TerminalCommandPhase::Input => Self::Input,
            TerminalCommandPhase::Executing => Self::Executing,
            TerminalCommandPhase::Completed { exit_status } => Self::Completed { exit_status },
        }
    }
}

/// Authoritative pane runtime state: the server-owned `TerminalPty`/
/// `TerminalScreen` pair is the source of truth for all of this, combining shell-integration OSC
/// reports with native process-inspection fallbacks per [`PaneCwdSource`]'s precedence order.
///
/// `cwd` is always the *best available displayable* path regardless of source; `cwd_host` is set
/// only when that path names a location on a different host than the **session server** (a remote
/// `OSC 7` report over SSH into the server's machine, for instance). Under `--remote`, the server
/// still owns this comparison — paths on the remote session host have `cwd_host = None` even though
/// they are not on the client's machine. Client-local filesystem consumers (file tree, profile
/// save of paths) must therefore also check whether the client is remote-attached before treating
/// a `cwd_host`-free path as local. Spawn requests sent to the session server may still carry the
/// server-relative cwd.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneRuntimeState {
    pub cwd: Option<String>,
    pub cwd_host: Option<String>,
    /// Server-computed compact cwd for pane chrome: project-qualified when inside a detected Git
    /// project, otherwise home-relative or absolute. Optional for compatibility with older peers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_path: Option<String>,
    /// Absolute path of the Git project containing `cwd`, as the session server sees it. Set only
    /// for a server-local `cwd` — a `cwd_host` path names a filesystem the server cannot probe.
    /// This, not `cwd`, is what the sidebar groups agents by, so a pane in `repo/src` lands in the
    /// same project as one in `repo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    /// Checked-out branch of `project_root`, or a short commit id when `HEAD` is detached. Read
    /// from `HEAD` on a slow tick rather than derived from `cwd`, since a checkout changes it
    /// without the directory moving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub cwd_source: PaneCwdSource,
    pub command_phase: PaneCommandPhase,
    /// Normalized executable basename only - never a full command line (shell integrations are
    /// expected to emit just the name; the process-inspector fallbacks read a `comm`/`proc_name`
    /// value that is inherently already just a basename).
    pub foreground_program: Option<String>,
    /// The most recently observed exit status, from either an `OSC 133;D` report or the PTY child
    /// itself exiting. Sticky across the next command's `Prompt`/`Input` phases so callers can
    /// still show "last command exited N" at a fresh prompt.
    pub last_exit_status: Option<i32>,
    #[serde(default)]
    pub status: Option<PaneStatus>,
    #[serde(default)]
    pub detected_agent: Option<DetectedAgent>,
    /// Unix timestamp for the current active agent run. It is retained while an agent is blocked
    /// or reports another non-quiescent status, so clients can show one continuous run age after a
    /// block/resume transition and after detach/reattach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_started_at: Option<u64>,
    /// Logical agents the pane's own program published for itself, empty for every pane that
    /// publishes nothing - which is nearly all of them, and the path that stays unchanged.
    ///
    /// A pane is one PTY but need not be one agent. A client with its own tab bar runs several
    /// sessions behind one terminal and can only ever *draw* the one in view, so screen detection
    /// sees a single state for several runs and cannot tell which of them it belongs to. A program
    /// that knows all of them reports them here instead, and the server stops scraping that pane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<AgentSlot>,
    /// Monotonic per-pane counter, bumped only when some other field in this struct actually
    /// changed. [`ServerMessage::PaneRuntimeChanged`] carries this so a client that received
    /// updates out of order (should not happen on a single ordered connection, but is cheap
    /// insurance) can detect and ignore a stale one.
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: ClientId,
    pub label: String,
    pub read_only: bool,
    /// True while this client has an outstanding request for the layout-control lease that the
    /// controller has not yet granted or declined. Broadcast in the roster so every client can badge
    /// the pending request; cleared when control moves to it or the controller declines.
    #[serde(default)]
    pub requesting_control: bool,
    /// True while this client holds the session open in the background rather than displaying it.
    /// A parked client is still attached — its screens stay live and switching back is instant —
    /// but it is not an occupant: it never holds the layout-control lease and is skipped when the
    /// lease has to move, so a background connection cannot push the next client into follower
    /// mode. Broadcast in the roster so the clients view can say which clients are only parked.
    #[serde(default)]
    pub parked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Attach {
        session: String,
        /// Client's maximum supported protocol version.
        protocol_version: u32,
        /// Client's minimum supported protocol version. Missing on pre-negotiation peers; treat a
        /// default of `0` as "exactly `protocol_version`".
        #[serde(default)]
        min_protocol_version: u32,
        label: String,
        read_only: bool,
    },
    /// Record the reusable profile that supplied this session's initial panes. Sent only after the
    /// profile seed requests have been queued successfully.
    SetSessionOrigin {
        profile: String,
    },
    /// Picker probe: report session status without registering the connection as a client and
    /// without any replay seeding. Cheap enough to run against many sockets concurrently.
    Query {
        session: String,
        /// Client's maximum supported protocol version.
        protocol_version: u32,
        /// Client's minimum supported protocol version. Missing on pre-negotiation peers; treat a
        /// default of `0` as "exactly `protocol_version`".
        #[serde(default)]
        min_protocol_version: u32,
    },
    SpawnPane {
        pane_id: PaneId,
        /// Owner-scoped transient pane. Local panes may reuse another client's pane id, are
        /// operable by their owner without the layout lease, and die with that client.
        local: bool,
        generation: u64,
        command: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        keep_open: bool,
        env: Vec<(String, String)>,
        title: Option<String>,
        /// Terminal color palette seeded onto the server screen *before* the PTY spawns, so the
        /// child's startup OSC 4/10/11 color queries are answered with the theme palette rather
        /// than the screen default. Sending it out-of-band via `SetPalette` races the child's
        /// query, so it must ride along with the spawn request.
        palette: WirePalette,
        /// Resolved interactive-shell argv (non-empty; program then fixed args), resolved
        /// client-side via `platform::command::resolve_interactive_shell` against the live
        /// config. Used verbatim when `command` is `None`; also used to `exec` into after
        /// `command` completes when `keep_open` is set (see [`ServerPane`] doc comment).
        shell: Vec<String>,
        /// Resolved command-runner argv (non-empty; program then fixed args), resolved
        /// client-side via `platform::command::resolve_command_shell`. Only used when `command`
        /// is `Some`; the command string becomes its final argument.
        command_shell: Vec<String>,
        /// Host cell size in pixels, so the PTY reports pixel dimensions a child that draws
        /// images can use. Zero (the pre-17 default) leaves the PTY's own default in place.
        #[serde(default)]
        cell_width: u16,
        /// See `cell_width`.
        #[serde(default)]
        cell_height: u16,
    },
    Resize {
        pane_id: PaneId,
        /// Address the owner-local namespace when true; the shared session namespace when false.
        local: bool,
        generation: u64,
        cols: u16,
        rows: u16,
        /// Host cell width in pixels, from the controller. Zero (the pre-17 default) means the
        /// client did not report one; the server keeps whatever the PTY already had.
        #[serde(default)]
        cell_width: u16,
        /// Host cell height in pixels. See `cell_width`.
        #[serde(default)]
        cell_height: u16,
    },
    Kill {
        pane_id: PaneId,
        local: bool,
        generation: u64,
    },
    SetPalette {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        palette: WirePalette,
    },
    ConfigurePane {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        palette: Option<WirePalette>,
        title: Option<String>,
        cwd: Option<String>,
    },
    SetPaneLogging {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        enabled: bool,
    },
    SetPaneStatus {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        status: Option<String>,
        reason: Option<String>,
    },
    /// Replace the pane's published agent slots. An empty list withdraws them, and the pane falls
    /// back to screen detection.
    ReportPaneSlots {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        slots: Vec<AgentSlot>,
    },
    /// Commit a new shared layout. Accepted only from the controller and only when `base_rev`
    /// equals the server's current revision; otherwise the server replies [`ServerMessage::LayoutRejected`].
    CommitLayout {
        base_rev: u64,
        layout: SharedLayout,
    },
    /// Request the layout-control lease. The server grants immediately when there is no controller
    /// or takeover is enabled; otherwise it flags this client as requesting and notifies the
    /// controller (see [`ServerMessage::ControlRequested`]).
    RequestControl,
    /// Controller-only: allow or forbid writable followers from taking the lease immediately when
    /// they send [`ClientMessage::RequestControl`].
    SetControlTakeover {
        allowed: bool,
    },
    /// Declare whether this client is parked: still attached, with its screens kept live, but not
    /// displaying or driving the session. Parking releases the layout-control lease if this client
    /// held it and excludes it from promotion, so a client keeping several sessions open in the
    /// background does not make each of them look occupied to the next client that attaches.
    /// Unparking asks for the lease back, which the server auto-grants when nobody holds it.
    SetParked {
        parked: bool,
    },
    /// Controller-only: grant the lease to `to`, which also clears `to`'s pending request.
    GrantControl {
        to: ClientId,
    },
    /// Controller-only: reject `to`'s pending control request, clearing its flag and notifying it
    /// (see [`ServerMessage::ControlDeclined`]).
    DeclineControl {
        to: ClientId,
    },
    /// Controller-only: remove `target` from the session. The server sends the
    /// target an [`ServerMessage::Error`] carrying [`EVICTED_ERROR_CODE`] and then closes its
    /// connection; the session itself and every other client keep running.
    EvictClient {
        target: ClientId,
    },
    SetInputLock {
        locked: bool,
    },
    /// Heartbeat reply to a [`ServerMessage::Ping`].
    Pong {
        seq: u64,
    },
    Rename {
        name: String,
    },
    /// List one directory **on the server's host** for the sidebar file tree.
    ///
    /// Answered by [`ServerMessage::DirectoryListing`].
    ListDirectory {
        path: String,
        /// Include dotfiles. Mirrors `[sidebar] tree.show_hidden` on the client.
        show_hidden: bool,
    },
    /// Scan a repository **on the server's host** for changed paths.
    ///
    /// Answered by [`ServerMessage::ChangeListing`]. Backs the file tree's `Changes` projection,
    /// which cannot use local git discovery when the files live on another machine.
    ListChanges {
        root: String,
    },
    /// Ask for an immediate server-owned resource sample.
    RequestRuntimeMetrics,
    Detach,
    Shutdown,
}

/// One child entry in a [`ServerMessage::DirectoryListing`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDirEntry {
    /// Name relative to the listed directory, never a path.
    pub name: String,
    pub is_dir: bool,
    #[serde(default)]
    pub is_symlink: bool,
    /// Whether the server's ignore rules exclude this entry.
    #[serde(default)]
    pub ignored: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_staged: Option<WireChangeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_unstaged: Option<WireChangeState>,
}

/// Serde-stable mirror of the framework's git change states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireChangeState {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One changed path in a [`ServerMessage::ChangeListing`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireChange {
    /// Path relative to the scanned root.
    pub path: String,
    pub state: WireChangeState,
    #[serde(default)]
    pub staged: bool,
}

/// Why the layout-control lease moved. Carried by [`ServerMessage::ControllerChanged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControllerChangeReason {
    /// The controller detached or dropped cleanly.
    Released,
    /// The controller missed heartbeats and was disconnected.
    Expired,
    /// The lease was granted: the first attacher, promotion of the oldest survivor, a controller's
    /// explicit grant, or an auto-grant to a requester when no controller held the lease.
    Granted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerMessage {
    Error {
        code: String,
        message: String,
    },
    Attached {
        /// Server's maximum supported protocol version.
        protocol_version: u32,
        /// Negotiated wire version for this connection. Missing (`0`) on pre-negotiation peers.
        #[serde(default)]
        effective_protocol: u32,
        session: String,
        client_id: ClientId,
        panes: Vec<PaneMeta>,
        layout_rev: u64,
        layout: Option<SharedLayout>,
        controller: Option<ClientId>,
        clients: Vec<ClientInfo>,
        input_locked: bool,
        #[serde(default)]
        allow_takeover: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_from_profile: Option<String>,
    },
    /// Reply to a [`ClientMessage::Query`] probe.
    SessionInfo {
        session: String,
        panes: usize,
        clients: u32,
        has_layout: bool,
        /// Negotiated wire version for this probe. Missing (`0`) on pre-negotiation peers.
        #[serde(default)]
        effective_protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_from_profile: Option<String>,
    },
    SessionOriginSet {
        created_from_profile: String,
    },
    Resized {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    Exited {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        code: i32,
    },
    SpawnResult {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        pid: Option<u32>,
        ok: bool,
        error: Option<String>,
    },
    PaneLoggingChanged {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        enabled: bool,
        path: Option<String>,
        error: Option<String>,
    },
    /// A pane's [`PaneRuntimeState`] changed: broadcast after raw
    /// pane output for the same event, once the server has both processed the bytes into the
    /// screen and re-derived runtime state from them. `generation` guards against a message that
    /// raced a respawn; `state.sequence` guards against out-of-order delivery.
    PaneRuntimeChanged {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        state: PaneRuntimeState,
    },
    Renamed {
        session: String,
    },
    /// A new layout revision was accepted; broadcast to every client including its author (so the
    /// author confirms its own rev from the echo and all clients see one identical rev sequence).
    LayoutCommitted {
        rev: u64,
        author: ClientId,
        layout: SharedLayout,
    },
    /// A commit was rejected (stale base rev or non-controller). Sent to the committer only, with
    /// the authoritative layout so the rejection self-heals.
    LayoutRejected {
        current_rev: u64,
        layout: Option<SharedLayout>,
    },
    ControllerChanged {
        controller: Option<ClientId>,
        reason: ControllerChangeReason,
    },
    /// Sent only to the current controller when `from` requests the lease. Debounced per requester so
    /// repeated requests cannot spam the controller; the sticky badge lives in the roster instead.
    ControlRequested {
        from: ClientId,
    },
    /// Sent only to a requester whose pending control request the controller declined.
    ControlDeclined,
    ClientsChanged {
        clients: Vec<ClientInfo>,
        input_locked: bool,
        #[serde(default)]
        allow_takeover: bool,
    },
    Ping {
        seq: u64,
    },
    /// Reply to [`ClientMessage::ListDirectory`]. `error` is set instead of `entries` when the
    /// server could not read the directory; the client renders it in the tree row.
    DirectoryListing {
        path: String,
        entries: Vec<WireDirEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to [`ClientMessage::ListChanges`]. An empty list with no `error` means a clean tree or
    /// a root that is not a repository.
    ChangeListing {
        root: String,
        changes: Vec<WireChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Server-owned resource sample, requested by a protocol-18 client.
    RuntimeMetrics {
        metrics: ServerRuntimeMetrics,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame<C> {
    Control(C),
    PaneBytes {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        bytes: Vec<u8>,
    },
}

/// Normalize a peer-advertised minimum: missing/`0` means "exactly `max`" (pre-negotiation peers).
pub fn normalize_min_protocol(max: u32, min: u32) -> u32 {
    if min == 0 { max } else { min }
}

/// Error from [`negotiate_protocol`] when the advertised ranges do not overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolMismatch {
    pub client_max: u32,
    pub client_min: u32,
    pub server_max: u32,
    pub server_min: u32,
    pub older_side: ProtocolSide,
}

/// Which peer is too old for the other side's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolSide {
    Client,
    Server,
}

impl ProtocolMismatch {
    pub fn message(&self) -> String {
        let older = match self.older_side {
            ProtocolSide::Client => "client",
            ProtocolSide::Server => "server",
        };
        format!(
            "client protocol {}-{} is incompatible with server protocol {}-{} ({older} is older)",
            self.client_min, self.client_max, self.server_min, self.server_max
        )
    }
}

/// Choose the effective wire version for a client/server pair.
///
/// `effective = min(client_max, server_max)`, accepted only when it falls within both sides'
/// supported ranges. A `client_min` of `0` is treated as "exactly `client_max`".
pub fn negotiate_protocol(
    client_max: u32,
    client_min: u32,
    server_max: u32,
    server_min: u32,
) -> std::result::Result<u32, ProtocolMismatch> {
    let client_min = normalize_min_protocol(client_max, client_min);
    let effective = client_max.min(server_max);
    if effective < client_min || effective < server_min {
        let older_side = if client_max < server_min {
            ProtocolSide::Client
        } else {
            ProtocolSide::Server
        };
        return Err(ProtocolMismatch {
            client_max,
            client_min,
            server_max,
            server_min,
            older_side,
        });
    }
    Ok(effective)
}

/// Attach message using this build's supported protocol range.
pub fn attach_message(
    session: impl Into<String>,
    label: impl Into<String>,
    read_only: bool,
) -> ClientMessage {
    ClientMessage::Attach {
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: label.into(),
        read_only,
    }
}

/// Query message using this build's supported protocol range.
pub fn query_message(session: impl Into<String>) -> ClientMessage {
    ClientMessage::Query {
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
    }
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> std::io::Result<()> {
    write_control_frame(writer, value)
}

pub fn write_control_frame<W: Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_frame_body(writer, FRAME_KIND_CONTROL_JSON, &body)
}

pub fn write_pane_output_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_frame(
        writer,
        FRAME_KIND_PANE_OUTPUT,
        pane_id,
        generation,
        local,
        bytes,
    )
}

pub fn write_pane_input_frame<W: Write>(
    writer: &mut W,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_pane_frame(
        writer,
        FRAME_KIND_PANE_INPUT,
        pane_id,
        generation,
        local,
        bytes,
    )
}

fn write_pane_frame<W: Write>(
    writer: &mut W,
    kind: u8,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut body = Vec::with_capacity(PANE_FRAME_HEADER_LEN + bytes.len());
    body.extend_from_slice(&pane_id.to_be_bytes());
    body.extend_from_slice(&generation.to_be_bytes());
    body.push(u8::from(local));
    body.extend_from_slice(bytes);
    write_frame_body(writer, kind, &body)
}

fn write_frame_body<W: Write>(writer: &mut W, kind: u8, body: &[u8]) -> std::io::Result<()> {
    let frame_len = body.len().saturating_add(1);
    if frame_len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "frame length {} exceeds maximum {MAX_FRAME_SIZE}",
                frame_len
            ),
        ));
    }
    let len: u32 = frame_len
        .try_into()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(body)
}

pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> std::io::Result<T> {
    read_frame_with_limit(reader, MAX_FRAME_SIZE)
}

pub fn read_frame_with_limit<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
    max_size: usize,
) -> std::io::Result<T> {
    let mut len = [0; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds maximum {max_size}"),
        ));
    }
    if len == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty frame",
        ));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let (kind, payload) = body.split_first().expect("non-empty frame");
    if *kind != FRAME_KIND_CONTROL_JSON {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected control frame, got kind {kind}"),
        ));
    }
    serde_json::from_slice(payload).map_err(std::io::Error::other)
}

#[derive(Debug)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    max_size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameReadStatus {
    Read(usize),
    WouldBlock,
    Eof,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new(MAX_FRAME_SIZE)
    }
}

impl FrameDecoder {
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_size,
        }
    }

    pub fn read_from_status<R: Read>(
        &mut self,
        reader: &mut R,
    ) -> std::io::Result<FrameReadStatus> {
        let mut chunk = [0_u8; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => Ok(FrameReadStatus::Eof),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                Ok(FrameReadStatus::Read(n))
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(FrameReadStatus::WouldBlock)
            }
            Err(err) => Err(err),
        }
    }

    pub fn next_frame<T: for<'de> Deserialize<'de>>(
        &mut self,
    ) -> std::io::Result<Option<Frame<T>>> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buffer[..4].try_into().expect("slice length")) as usize;
        if len > self.max_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("frame length {len} exceeds maximum {}", self.max_size),
            ));
        }
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buffer[4..4 + len].to_vec();
        self.buffer.drain(..4 + len);
        decode_frame(&body).map(Some)
    }
}

fn decode_frame<T: for<'de> Deserialize<'de>>(body: &[u8]) -> std::io::Result<Frame<T>> {
    let Some((&kind, payload)) = body.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty frame",
        ));
    };
    match kind {
        FRAME_KIND_CONTROL_JSON => serde_json::from_slice(payload)
            .map(Frame::Control)
            .map_err(std::io::Error::other),
        FRAME_KIND_PANE_OUTPUT | FRAME_KIND_PANE_INPUT => decode_pane_frame(payload),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown frame kind {kind}"),
        )),
    }
}

fn decode_pane_frame<T>(payload: &[u8]) -> std::io::Result<Frame<T>> {
    if payload.len() < PANE_FRAME_HEADER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pane byte frame missing header",
        ));
    }
    let pane_id = u32::from_be_bytes(payload[..4].try_into().expect("slice length"));
    let generation = u64::from_be_bytes(payload[4..12].try_into().expect("slice length"));
    let local = match payload[12] {
        0 => false,
        1 => true,
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("pane byte frame has invalid locality {other}"),
            ));
        }
    };
    Ok(Frame::PaneBytes {
        pane_id,
        local,
        generation,
        bytes: payload[PANE_FRAME_HEADER_LEN..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_blocked_elevates_over_quiescent_reports_but_not_working() {
        let detected = DetectedAgent {
            kind: AgentKind::OpenCode,
            state: DetectedAgentState::Blocked,
        };
        for value in [pane_status::IDLE, pane_status::DONE] {
            let reported = PaneStatus {
                value: value.into(),
                reason: None,
                set_at: 0,
            };
            assert_eq!(
                effective_agent_status(Some(&reported), Some(&detected)),
                Some(pane_status::BLOCKED)
            );
        }
        let working = PaneStatus {
            value: pane_status::WORKING.into(),
            reason: None,
            set_at: 0,
        };
        assert_eq!(
            effective_agent_status(Some(&working), Some(&detected)),
            Some(pane_status::WORKING)
        );
        assert_eq!(
            effective_agent_status(Some(&working), None),
            Some(pane_status::WORKING)
        );
    }
    use crate::shared_layout::{
        SHARED_LAYOUT_VERSION, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
        SharedWorkspace,
    };

    #[test]
    fn golden_layout_commit_json_shape() {
        let layout = SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: 120,
            canvas_rows: 40,
            workspaces: vec![SharedWorkspace {
                index: 0,
                name: Some("dev".to_string()),
                synchronized: true,
                layout: SharedLayoutKind::Master,
                start_axis: SharedSplitAxis::Vertical,
                split_ratios: vec![0.4],
                tree: Some(SharedTree::Split {
                    axis: SharedSplitAxis::Vertical,
                    ratio: 0.375,
                    first: Box::new(SharedTree::Leaf { pane: 2 }),
                    second: Box::new(SharedTree::Leaf { pane: 9 }),
                }),
                panes: vec![SharedPane {
                    pane_id: 2,
                    generation: 7,
                    title: Some("editor".to_string()),
                    profile_name: None,
                    cwd: Some("/repo".to_string()),
                    command: Some("nvim".to_string()),
                    replay: false,
                    keep_open: false,
                    floating: false,
                    fullscreen: false,
                    rect: None,
                    scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                }],
            }],
        };

        assert_eq!(
            serde_json::to_value(ServerMessage::LayoutCommitted {
                rev: 4,
                author: 3,
                layout,
            })
            .unwrap(),
            serde_json::json!({
                "type": "layout-committed",
                "rev": 4,
                "author": 3,
                "layout": {
                    "version": 2,
                    "canvas_cols": 120,
                    "canvas_rows": 40,
                    "workspaces": [{
                        "index": 0,
                        "name": "dev",
                        "synchronized": true,
                        "layout": "master",
                        "start_axis": "vertical",
                        "split_ratios": [0.4000000059604645],
                        "tree": {
                            "kind": "split",
                            "axis": "vertical",
                            "ratio": 0.375,
                            "first": {"kind": "leaf", "pane": 2},
                            "second": {"kind": "leaf", "pane": 9}
                        },
                        "panes": [{
                            "pane_id": 2,
                            "generation": 7,
                            "title": "editor",
                            "profile_name": null,
                            "cwd": "/repo",
                            "command": "nvim",
                            "replay": false,
                            "keep_open": false,
                            "floating": false,
                            "fullscreen": false,
                            "rect": null,
                            "scrollable_width": 0.44999998807907104
                        }]
                    }]
                }
            })
        );
    }

    #[test]
    fn protocol_frame_round_trips() {
        let msg = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            label: "alice".into(),
            read_only: false,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn pane_meta_from_older_peer_defaults_original_user() {
        let mut value = serde_json::to_value(PaneMeta {
            pane_id: 1,
            generation: 2,
            cols: 80,
            rows: 24,
            pid: Some(42),
            title: Some("user@host:~".to_string()),
            original_user: Some("user".to_string()),
            exited: None,
            logging: false,
            runtime: PaneRuntimeState::default(),
        })
        .unwrap();
        value.as_object_mut().unwrap().remove("original_user");

        let decoded: PaneMeta = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.original_user, None);
    }

    #[test]
    fn session_origin_shape_round_trips() {
        let msg = ClientMessage::SetSessionOrigin {
            profile: "work".into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn file_tree_messages_round_trip() {
        let request = ClientMessage::ListDirectory {
            path: "/srv/project".into(),
            show_hidden: true,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &request).unwrap();
        assert_eq!(
            read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(),
            request
        );

        let listing = ServerMessage::DirectoryListing {
            path: "/srv/project".into(),
            entries: vec![WireDirEntry {
                name: "src".into(),
                is_dir: true,
                is_symlink: false,
                ignored: false,
                git_staged: None,
                git_unstaged: Some(WireChangeState::Modified),
            }],
            error: None,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &listing).unwrap();
        assert_eq!(
            read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
            listing
        );

        let changes = ServerMessage::ChangeListing {
            root: "/srv/project".into(),
            changes: vec![WireChange {
                path: "src/lib.rs".into(),
                state: WireChangeState::Modified,
                staged: false,
            }],
            error: None,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &changes).unwrap();
        assert_eq!(
            read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
            changes
        );
    }

    #[test]
    fn only_the_exact_version_negotiates() {
        const { assert!(MIN_SUPPORTED_PROTOCOL == PROTOCOL_VERSION) };

        assert_eq!(
            negotiate_protocol(
                PROTOCOL_VERSION,
                MIN_SUPPORTED_PROTOCOL,
                PROTOCOL_VERSION,
                MIN_SUPPORTED_PROTOCOL,
            )
            .expect("same-version peers negotiate"),
            PROTOCOL_VERSION
        );
        assert!(
            negotiate_protocol(
                PROTOCOL_VERSION,
                MIN_SUPPORTED_PROTOCOL,
                PROTOCOL_VERSION + 1,
                PROTOCOL_VERSION + 1,
            )
            .is_err(),
            "a newer-only peer is rejected"
        );
        assert!(
            negotiate_protocol(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1, 1, 1).is_err(),
            "an older-only peer is rejected"
        );

        let request = ClientMessage::RequestRuntimeMetrics;
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        assert_eq!(
            read_frame::<_, ClientMessage>(&mut &bytes[..]).unwrap(),
            request
        );

        let sample = ServerMessage::RuntimeMetrics {
            metrics: ServerRuntimeMetrics {
                sampled_at_unix_ms: 42,
                ..ServerRuntimeMetrics::default()
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &sample).unwrap();
        assert_eq!(
            read_frame::<_, ServerMessage>(&mut &bytes[..]).unwrap(),
            sample
        );
    }

    #[test]
    fn pane_status_message_round_trips() {
        let msg = ClientMessage::SetPaneStatus {
            pane_id: 7,
            local: false,
            generation: 9,
            status: Some("blocked".into()),
            reason: Some("needs approval".into()),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "set-pane-status",
                "pane_id": 7,
                "local": false,
                "generation": 9,
                "status": "blocked",
                "reason": "needs approval"
            })
        );
    }

    #[test]
    fn pane_slots_message_round_trips() {
        let msg = ClientMessage::ReportPaneSlots {
            pane_id: 3,
            local: false,
            generation: 2,
            slots: vec![
                AgentSlot {
                    id: "ses_abc".into(),
                    title: "audit the widget layer".into(),
                    status: "working".into(),
                    reason: None,
                    active: true,
                    work_started_at: Some(120),
                },
                AgentSlot {
                    id: "ses_def".into(),
                    title: "fix the flaky test".into(),
                    status: "blocked".into(),
                    reason: Some("permission required".into()),
                    active: false,
                    work_started_at: None,
                },
            ],
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
    }

    /// The overwhelming majority of panes publish nothing, and must not pay for the field.
    #[test]
    fn a_pane_without_slots_does_not_serialize_the_key() {
        let json = serde_json::to_string(&PaneRuntimeState::default()).unwrap();
        assert!(!json.contains("slots"), "{json}");
    }

    #[test]
    fn slot_aggregation_is_by_severity_not_recency() {
        let slot = |status: &str| AgentSlot {
            id: status.into(),
            title: status.into(),
            status: status.into(),
            reason: None,
            active: false,
            work_started_at: None,
        };
        assert_eq!(aggregate_slot_state(&[]), None);
        assert_eq!(
            aggregate_slot_state(&[slot("idle"), slot("done")]),
            Some(DetectedAgentState::Idle)
        );
        assert_eq!(
            aggregate_slot_state(&[slot("idle"), slot("working")]),
            Some(DetectedAgentState::Working),
            "one running session keeps the pane working"
        );
        assert_eq!(
            aggregate_slot_state(&[slot("working"), slot("blocked")]),
            Some(DetectedAgentState::Blocked),
            "a prompt outranks work happening beside it"
        );
        assert_eq!(
            aggregate_slot_state(&[slot("idle"), slot("compacting")]),
            Some(DetectedAgentState::Working),
            "a custom status is an active run, matching status_is_quiescent"
        );
    }

    #[test]
    fn pane_runtime_status_is_optional_for_serde_compatibility() {
        let old_shape = serde_json::json!({
            "cwd": null,
            "cwd_host": null,
            "cwd_source": "unknown",
            "command_phase": {"phase": "unknown"},
            "foreground_program": null,
            "last_exit_status": null,
            "sequence": 4
        });
        let decoded: PaneRuntimeState = serde_json::from_value(old_shape).unwrap();
        assert_eq!(decoded.display_path, None);
        assert_eq!(decoded.status, None);
        assert_eq!(decoded.detected_agent, None);
        assert_eq!(decoded.work_started_at, None);

        let state = PaneRuntimeState {
            status: Some(PaneStatus {
                value: "working".into(),
                reason: None,
                set_at: 123,
            }),
            work_started_at: Some(120),
            sequence: 5,
            ..PaneRuntimeState::default()
        };
        assert_eq!(
            serde_json::from_value::<PaneRuntimeState>(serde_json::to_value(&state).unwrap())
                .unwrap(),
            state
        );
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(9_u32).to_be_bytes());
        buf.extend_from_slice(b"{}");
        let err = read_frame_with_limit::<_, ClientMessage>(&mut &buf[..], 8).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn oversized_write_frame_is_rejected() {
        let msg = ClientMessage::SpawnPane {
            pane_id: 1,
            local: false,
            generation: 1,
            command: Some("x".repeat(MAX_FRAME_SIZE)),
            cwd: None,
            cols: 80,
            rows: 24,
            keep_open: false,
            env: Vec::new(),
            title: None,
            palette: WirePalette {
                foreground: None,
                background: None,
                ansi: [Color::Black; 16],
            },
            cell_width: 0,
            cell_height: 0,
            shell: vec!["/bin/sh".to_string()],
            command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
        };
        let err = write_frame(&mut Vec::new(), &msg).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn frame_decoder_preserves_partial_bytes_until_complete() {
        let msg = ClientMessage::Detach;
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &msg).unwrap();
        let split = 6.min(encoded.len() - 1);
        let mut decoder = FrameDecoder::default();
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[..split]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert!(decoder.next_frame::<ClientMessage>().unwrap().is_none());
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[split..]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::Control(msg))
        );
    }

    #[test]
    fn golden_client_attach_json_shape() {
        let value = serde_json::to_value(ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            label: "alice".into(),
            read_only: true,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type":"attach",
                "session":"dev",
                "protocol_version":PROTOCOL_VERSION,
                "min_protocol_version":MIN_SUPPORTED_PROTOCOL,
                "label":"alice",
                "read_only":true
            })
        );
        assert_eq!(
            serde_json::to_value(ClientMessage::SetSessionOrigin {
                profile: "work".into()
            })
            .unwrap(),
            serde_json::json!({"type":"set-session-origin","profile":"work"})
        );
    }

    #[test]
    fn golden_query_json_shape() {
        let value = serde_json::to_value(ClientMessage::Query {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type":"query",
                "session":"dev",
                "protocol_version":PROTOCOL_VERSION,
                "min_protocol_version":MIN_SUPPORTED_PROTOCOL
            })
        );
    }

    #[test]
    fn golden_request_control_and_pong_json_shape() {
        assert_eq!(
            serde_json::to_value(ClientMessage::RequestControl).unwrap(),
            serde_json::json!({"type":"request-control"})
        );
        assert_eq!(
            serde_json::to_value(ClientMessage::Pong { seq: 5 }).unwrap(),
            serde_json::json!({"type":"pong","seq":5})
        );
    }

    #[test]
    fn grant_control_and_input_lock_round_trip() {
        for message in [
            ClientMessage::GrantControl { to: 7 },
            ClientMessage::DeclineControl { to: 7 },
            ClientMessage::EvictClient { target: 7 },
            ClientMessage::RequestControl,
            ClientMessage::SetControlTakeover { allowed: true },
            ClientMessage::SetInputLock { locked: true },
        ] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &message).unwrap();
            assert_eq!(
                read_frame::<_, ClientMessage>(&mut &bytes[..]).unwrap(),
                message
            );
        }
    }

    #[test]
    fn golden_controller_changed_and_clients_changed_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::ControllerChanged {
                controller: Some(3),
                reason: ControllerChangeReason::Granted,
            })
            .unwrap(),
            serde_json::json!({"type":"controller-changed","controller":3,"reason":"granted"})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::ClientsChanged {
                clients: vec![ClientInfo {
                    id: 1,
                    label: "alice".into(),
                    read_only: false,
                    requesting_control: true,
                    parked: false,
                }],
                input_locked: true,
                allow_takeover: false,
            })
            .unwrap(),
            serde_json::json!({"type":"clients-changed","clients":[{"id":1,"label":"alice","read_only":false,"requesting_control":true,"parked":false}],"input_locked":true,"allow_takeover":false})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::Ping { seq: 9 }).unwrap(),
            serde_json::json!({"type":"ping","seq":9})
        );
    }

    #[test]
    fn golden_session_info_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::SessionInfo {
                session: "dev".into(),
                panes: 2,
                clients: 1,
                has_layout: true,
                effective_protocol: PROTOCOL_VERSION,
                created_from_profile: Some("work".into()),
            })
            .unwrap(),
            serde_json::json!({
                "type":"session-info",
                "session":"dev",
                "panes":2,
                "clients":1,
                "has_layout":true,
                "effective_protocol":PROTOCOL_VERSION,
                "created_from_profile":"work"
            })
        );
    }

    #[test]
    fn binary_pane_frame_has_golden_shape() {
        let mut buf = Vec::new();
        write_pane_output_frame(&mut buf, 7, 9, false, b"abc").unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&17_u32.to_be_bytes());
        expected.push(FRAME_KIND_PANE_OUTPUT);
        expected.extend_from_slice(&7_u32.to_be_bytes());
        expected.extend_from_slice(&9_u64.to_be_bytes());
        expected.push(0);
        expected.extend_from_slice(b"abc");

        assert_eq!(buf, expected);

        let mut local = Vec::new();
        write_pane_input_frame(&mut local, 7, 9, true, b"abc").unwrap();
        let mut expected_local = Vec::new();
        expected_local.extend_from_slice(&17_u32.to_be_bytes());
        expected_local.push(FRAME_KIND_PANE_INPUT);
        expected_local.extend_from_slice(&7_u32.to_be_bytes());
        expected_local.extend_from_slice(&9_u64.to_be_bytes());
        expected_local.push(1);
        expected_local.extend_from_slice(b"abc");
        assert_eq!(local, expected_local);
    }

    #[test]
    fn frame_decoder_decodes_interleaved_control_and_binary_frames() {
        let attach = ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            label: "alice".into(),
            read_only: false,
        };
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &attach).unwrap();
        write_pane_input_frame(&mut encoded, 7, 9, false, b"abc").unwrap();

        let mut decoder = FrameDecoder::default();
        assert!(matches!(
            decoder.read_from_status(&mut &encoded[..]).unwrap(),
            FrameReadStatus::Read(_)
        ));
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::Control(attach))
        );
        assert_eq!(
            decoder.next_frame::<ClientMessage>().unwrap(),
            Some(Frame::PaneBytes {
                pane_id: 7,
                local: false,
                generation: 9,
                bytes: b"abc".to_vec(),
            })
        );
        assert_eq!(decoder.next_frame::<ClientMessage>().unwrap(), None);
    }

    #[test]
    fn golden_client_spawn_json_shape() {
        let palette = WirePalette {
            foreground: Some(Color::White),
            background: Some(Color::Black),
            ansi: [Color::Black; 16],
        };
        let value = serde_json::to_value(ClientMessage::SpawnPane {
            pane_id: 7,
            local: false,
            generation: 9,
            command: Some("bash".into()),
            cwd: Some("/repo".into()),
            cols: 80,
            rows: 24,
            keep_open: true,
            env: vec![("A".into(), "B".into())],
            title: Some("shell".into()),
            palette,
            shell: vec!["/bin/zsh".to_string()],
            command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
            cell_width: 9,
            cell_height: 18,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"spawn-pane","pane_id":7,"local":false,"generation":9,"command":"bash","cwd":"/repo","cols":80,"rows":24,"keep_open":true,"env":[["A","B"]],"title":"shell","palette":serde_json::to_value(palette).unwrap(),"shell":["/bin/zsh"],"command_shell":["/bin/sh","-c"],"cell_width":9,"cell_height":18})
        );
    }

    #[test]
    fn golden_server_messages_json_shape() {
        assert_eq!(
            serde_json::to_value(ServerMessage::Resized {
                pane_id: 1,
                local: false,
                generation: 2,
                cols: 80,
                rows: 24,
            })
            .unwrap(),
            serde_json::json!({"type":"resized","pane_id":1,"local":false,"generation":2,"cols":80,"rows":24})
        );
        assert_eq!(
            serde_json::to_value(ServerMessage::Error {
                code: "bad".into(),
                message: "no".into()
            })
            .unwrap(),
            serde_json::json!({"type":"error","code":"bad","message":"no"})
        );
    }

    #[test]
    fn negotiate_protocol_table() {
        // (client_max, client_min, server_max, server_min, expected Ok(effective) or Err(older))
        type Case = (u32, u32, u32, u32, std::result::Result<u32, ProtocolSide>);
        let cases: &[Case] = &[
            (12, 12, 12, 12, Ok(12)),
            (13, 12, 12, 12, Ok(12)),
            (12, 12, 13, 12, Ok(12)),
            (14, 12, 13, 12, Ok(13)),
            (11, 11, 12, 12, Err(ProtocolSide::Client)),
            (13, 13, 12, 12, Err(ProtocolSide::Server)),
            (12, 0, 12, 12, Ok(12)), // missing min => exactly max
            (11, 0, 12, 12, Err(ProtocolSide::Client)),
        ];
        for &(client_max, client_min, server_max, server_min, expected) in cases {
            let result = negotiate_protocol(client_max, client_min, server_max, server_min);
            match expected {
                Ok(effective) => {
                    assert_eq!(
                        result,
                        Ok(effective),
                        "client {client_min}-{client_max} vs server {server_min}-{server_max}"
                    );
                }
                Err(older) => {
                    let err = result.expect_err("expected mismatch");
                    assert_eq!(err.older_side, older);
                    let message = err.message();
                    assert!(message.contains("incompatible"), "{message}");
                    assert!(
                        message.contains("client") && message.contains("server"),
                        "mismatch must name both sides: {message}"
                    );
                    assert!(
                        message.contains(match older {
                            ProtocolSide::Client => "client is older",
                            ProtocolSide::Server => "server is older",
                        }),
                        "{message}"
                    );
                }
            }
        }
    }

    #[test]
    fn attach_without_min_protocol_deserializes_as_legacy_exact() {
        let value = serde_json::json!({
            "type": "attach",
            "session": "dev",
            "protocol_version": 12,
            "label": "alice",
            "read_only": false
        });
        let decoded: ClientMessage = serde_json::from_value(value).unwrap();
        assert_eq!(
            decoded,
            ClientMessage::Attach {
                session: "dev".into(),
                protocol_version: 12,
                min_protocol_version: 0,
                label: "alice".into(),
                read_only: false,
            }
        );
    }

    #[test]
    fn attached_without_effective_protocol_deserializes_as_zero() {
        let value = serde_json::json!({
            "type": "attached",
            "protocol_version": 12,
            "session": "dev",
            "client_id": 1,
            "panes": [],
            "layout_rev": 0,
            "layout": null,
            "controller": null,
            "clients": [],
            "input_locked": false
        });
        let decoded: ServerMessage = serde_json::from_value(value).unwrap();
        let ServerMessage::Attached {
            effective_protocol, ..
        } = decoded
        else {
            panic!("expected Attached");
        };
        assert_eq!(effective_protocol, 0);
    }
}
