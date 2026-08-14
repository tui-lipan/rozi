use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::state::PaneId;

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
    /// the next [`super::ServerMessage::PaneRuntimeChanged`].
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

pub(crate) fn detected_agent_status(detected: &DetectedAgent) -> &'static str {
    match detected.state {
        DetectedAgentState::Idle => pane_status::IDLE,
        DetectedAgentState::Working => pane_status::WORKING,
        DetectedAgentState::Blocked => pane_status::BLOCKED,
    }
}

/// One logical agent or activity inside a pane that publishes several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedRow {
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
    /// The row the publisher is currently displaying; at most one is true.
    ///
    /// This is what lets a finish on a *background* row raise an alert even while the pane is
    /// focused - looking at the pane only ever acknowledges the row on screen.
    #[serde(default)]
    pub active: bool,
    /// Server-owned run start, mirroring [`PaneRuntimeState::work_started_at`] per row. Whatever
    /// a publisher sends here is overwritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_started_at: Option<u64>,
}

/// The single state a pane shows for a set of published rows.
///
/// Severity order rather than recency: pane chrome answers "is anything in here demanding
/// attention", so one blocked row outranks any number of working ones, and any working row
/// outranks the ones that have finished. Returns `None` for an empty set, which is a pane that
/// publishes nothing rather than a pane whose agents are all idle.
pub fn aggregate_row_state(rows: &[PublishedRow]) -> Option<DetectedAgentState> {
    let mut state = None;
    for row in rows {
        let value = row.status.trim();
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
    /// Logical agents or activities this pane's program published for itself; empty for a pane that
    /// publishes nothing - which is nearly all of them, and the path that stays unchanged.
    ///
    /// A pane is one PTY but need not be one agent. A client with its own tab bar runs several
    /// sessions behind one terminal and can only ever *draw* the one in view, so screen detection
    /// sees a single state for several runs and cannot tell which of them it belongs to. A program
    /// that knows all of them reports them here instead, and the server stops scraping that pane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<PublishedRow>,
    /// Monotonic per-pane counter, bumped only when some other field in this struct actually
    /// changed. [`super::ServerMessage::PaneRuntimeChanged`] carries this so a client that received
    /// updates out of order (should not happen on a single ordered connection, but is cheap
    /// insurance) can detect and ignore a stale one.
    pub sequence: u64,
}
