use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::Msg;
use crate::events::{EventHub, EventKind};
use crate::platform::ipc::{EndpointRegistry, IpcConnection, IpcListener};
use crate::state::PaneId;

/// Version of the public control request and response API.
pub const CONTROL_API_VERSION: u32 = 1;

/// Version of the published JSON Schema describing that API.
///
/// Deliberately its own number, separate from [`CONTROL_API_VERSION`] and from the session
/// protocol version. The session wire protocol bumps when two rozi binaries change how they frame
/// messages to each other, which is nobody else's concern; this one tracks the compatibility of
/// the JSON third-party software reads and writes.
pub const API_SCHEMA_VERSION: u32 = 1;

pub const AGENT_WAITS_CAPABILITY: &str = "agent-waits";
pub const PANE_CONTROL_CAPABILITY: &str = "pane-control";
pub const SESSION_CONTROL_CAPABILITY: &str = "session-control";
pub const PUBLISHED_ACTIVITY_CAPABILITY: &str = "published-activity";
/// This binary can both forward a control command to a session on another host and serve one
/// forwarded to it. Advertised by `api describe`, so a caller can check the far host's rozi before
/// relying on it.
pub const REMOTE_CONTROL_CAPABILITY: &str = "remote-control";

/// Features this binary exposes to control clients and extension authors.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct ApiDescription {
    pub api: u32,
    /// Version of the JSON Schema this binary's API is described by, published at
    /// `docs/schema/rozi-control-v1.schema.json`.
    pub schema: u32,
    pub session_protocol: u32,
    pub capabilities: Vec<&'static str>,
}

impl ApiDescription {
    pub fn current() -> Self {
        Self {
            api: CONTROL_API_VERSION,
            schema: API_SCHEMA_VERSION,
            session_protocol: crate::session::protocol::PROTOCOL_VERSION,
            capabilities: vec![
                AGENT_WAITS_CAPABILITY,
                PANE_CONTROL_CAPABILITY,
                PUBLISHED_ACTIVITY_CAPABILITY,
                REMOTE_CONTROL_CAPABILITY,
                SESSION_CONTROL_CAPABILITY,
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct ControlRequest {
    #[serde(flatten)]
    pub command: ControlCommand,
    #[serde(default)]
    pub source_pane: Option<PaneId>,
    /// Automatically attached by the CLI when launched from an extension process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<crate::config::ExtensionProvenance>,
}

/// How many scrollback lines `capture-pane` should include when not using the visible grid.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum CaptureScrollback {
    /// Trailing line count from the retained scrollback + live grid.
    Lines(usize),
    /// Named capture modes (`"full"`, `"last-output"`).
    Named(CaptureScrollbackNamed),
}

/// Named `capture-pane` scrollback modes. Serde maps these to kebab-case strings so
/// validation lives in the type instead of string compares at each call site.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum CaptureScrollbackNamed {
    Full,
    LastOutput,
}

impl CaptureScrollback {
    pub fn parse_cli(value: &str) -> std::result::Result<Self, String> {
        if value.eq_ignore_ascii_case("full") {
            return Ok(Self::Named(CaptureScrollbackNamed::Full));
        }
        if value.eq_ignore_ascii_case("last-output") {
            return Ok(Self::Named(CaptureScrollbackNamed::LastOutput));
        }
        value
            .parse::<usize>()
            .map(Self::Lines)
            .map_err(|_| "--scrollback requires a line count or `full`".to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum ControlCommand {
    ListPanes,
    AgentsList,
    AgentGet {
        target: AgentTarget,
    },
    AgentRead {
        target: AgentTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scrollback: Option<CaptureScrollback>,
    },
    Metrics,
    Focus {
        target: PaneId,
    },
    SendText {
        target: Option<PaneId>,
        text: String,
    },
    /// Send named keys and/or literal text chunks to a pane (tmux-style key names).
    SendKeys {
        #[serde(default)]
        target: Option<PaneId>,
        keys: Vec<String>,
        /// When true, every entry in `keys` is forwarded as literal UTF-8 (no key-name parsing).
        #[serde(default)]
        literal: bool,
    },
    NewPane {
        /// Shell command line interpreted by the configured command runner.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        /// Direct executable and arguments. Mutually exclusive with `command`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argv: Option<Vec<String>>,
        cwd: Option<String>,
        title: Option<String>,
        #[serde(default)]
        keep_open: bool,
        /// Move focus (and the active workspace) to the new pane. Defaults to `false`: the control
        /// endpoint is an automation surface, and a pane spawned by an agent must not pull the
        /// cursor out from under whoever is typing. Overrides a matched `[[rules]]` `focus`; the
        /// rule still decides workspace, float, and fullscreen.
        #[serde(default)]
        focus: bool,
        /// One-based workspace to spawn into, instead of the caller's. Overrides a matched
        /// `[[rules]]` workspace, which is a default for panes a person opens.
        ///
        /// What this is for: a script that opens panes into the workspace someone is working in
        /// re-tiles their layout on every spawn. Naming a workspace of its own keeps the spawn
        /// entirely out of the way, and gives each pane the same geometry as the last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<usize>,
    },
    /// Run any keybindable `Action` by its stable id (see `Action::id`/`Action::from_id`).
    RunAction {
        action: String,
    },
    /// Capture pane text. Without `scrollback`, returns the current visible snapshot grid.
    /// With `scrollback`, returns scrollback history (`"full"` or a trailing line count).
    CapturePane {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        scrollback: Option<CaptureScrollback>,
    },
    /// Switch the active workspace. `index` is 1-based (1-9), matching the on-screen tabs.
    SwitchWorkspace {
        index: usize,
    },
    /// Move the focused pane to another workspace. `index` is 1-based (1-9).
    MoveToWorkspace {
        index: usize,
    },
    Popup {
        command: String,
        cwd: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        /// Hold the popup open after the command exits, matching the `[keys]` `popup` default; set
        /// `false` for a program that owns the popup for its whole life.
        #[serde(default)]
        keep_open: Option<bool>,
    },
    /// Publish the logical agents or activities running inside the calling pane, and receive
    /// activations for them. Unlike every other command this connection stays open in both
    /// directions; closing it withdraws the pane's rows. Reached through `rozi publish`.
    Publish,
    Subscribe {
        #[serde(default)]
        events: Vec<String>,
    },
    PaneLogging {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    SetStatus {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    AgentWait {
        target: AgentTarget,
        until: AgentWaitCondition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    AgentPrompt {
        target: AgentTarget,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<AgentWaitCondition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(default)]
        allow_working: bool,
    },
    AgentReport {
        #[serde(default)]
        target: Option<PaneId>,
        agent: String,
        integration: String,
        state: crate::session::protocol::AgentState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_session: Option<String>,
        seq: u64,
    },
    AgentRelease {
        #[serde(default)]
        target: Option<PaneId>,
        integration: String,
        seq: u64,
    },
    /// Raise a toast from a script.
    ///
    /// The automation surface can act but not report: a command that closes its own picker, or
    /// runs under `[keys] exec` with no pane, finishes invisibly. This is the "useful off-screen
    /// result" toasts are reserved for - not a receipt for something already on screen.
    Notify {
        message: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        level: NotifyLevel,
    },
    Pick {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        /// Copy shown when the row list is empty and the filter is empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        empty: Option<String>,
        /// Modal width in columns, clamped to a readable range. Omitted uses the shared default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u16>,
        /// Extra chords offered beside select and cancel, advertised in the footer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<crate::state::PickAction>,
        /// Pages shown as a tab strip, each with its own rows, filter, and highlight. Omitted is
        /// one untitled page and no strip.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tabs: Vec<crate::state::PickTab>,
        /// Id of the tab to open on. Omitted or unknown opens the first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum AgentTarget {
    Pane(PaneId),
    Ref(crate::session::protocol::AgentRef),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum AgentWaitCondition {
    Working,
    Blocked,
    Idle,
    Done,
    Quiescent,
    Gone,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentInfo {
    pub session: String,
    pub pane: PaneId,
    pub workspace: usize,
    pub agent: String,
    pub label: String,
    pub state: crate::session::protocol::AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session: Option<String>,
    #[serde(rename = "ref")]
    pub reference: crate::session::protocol::AgentRef,
    pub source: crate::session::protocol::AgentAuthority,
}

/// One pane as `list-panes` reports it.
///
/// Both control surfaces fill this same type. A UI endpoint reads it out of the client's `State`
/// and a session server reads it out of its own authoritative runtime, but the document a script
/// parses is the contract, not either implementation - so there is one type describing it rather
/// than two that have to be kept in step by hand.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PaneInfo {
    /// Session that answered. A remote one is qualified with its host, so two same-name sessions
    /// do not look interchangeable.
    pub session: String,
    pub id: PaneId,
    /// Absent only from a UI endpoint that has no session behind it.
    pub reference: Option<crate::session::protocol::PaneRef>,
    pub agent_ref: Option<crate::session::protocol::AgentRef>,
    pub title: String,
    /// One-based workspace, or `0` when the session has no layout document yet - nothing has
    /// placed the pane, so there is no workspace to name.
    pub workspace: usize,
    /// Initial launch intent, retained for automation and profile diagnostics.
    pub command: Option<String>,
    pub argv: Option<Vec<String>>,
    /// Live foreground process: what the pane is running now, rather than what launched it.
    pub foreground_program: Option<String>,
    pub foreground_programs: Vec<String>,
    pub foreground_arguments: Vec<String>,
    pub cwd: Option<String>,
    /// Lifecycle text in the vocabulary a client's terminal reports: `ready`, or `exited (N)` for
    /// a pane whose process is gone but whose screen is still readable.
    pub status: String,
    pub reported_status: Option<String>,
    pub status_reason: Option<String>,
    /// The agent detection recognized behind this pane, by definition id, and what it reads the
    /// pane as doing. Both absent when no definition matched. This is detection's own answer, not
    /// the pane's `reported_status` - a script capturing screens to test the rules against needs
    /// to see what the rules currently say about the screen it just took.
    pub agent: Option<String>,
    pub agent_state: Option<String>,
}

/// The whole `data` value of a `list-panes` reply.
///
/// A named type for the array rather than for its element alone, because `ControlResponse::data`
/// is untyped on the wire and the schema is where a consumer finds out what a given command's
/// `data` is. Without this, the published schema describes one pane and leaves the shape actually
/// returned - a list of them - unnamed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PaneListPayload(pub Vec<PaneInfo>);

/// The whole `data` value of an `agents list` reply. See [`PaneListPayload`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentListPayload(pub Vec<AgentInfo>);

/// One pane's captured screen, as `capture-pane` and `agents read` report it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PaneCapture {
    pub id: PaneId,
    pub text: String,
    /// The terminal title, which several detection rules match instead of the screen. A capture
    /// without it cannot stand in for what the detector saw.
    pub title: Option<String>,
}

/// What `split` answers with once the pane exists.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct NewPaneAccepted {
    pub id: PaneId,
    pub accepted: bool,
    pub pty_ready: bool,
}

/// What `pane-logging` answers with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PaneLoggingState {
    pub id: PaneId,
    pub enabled: bool,
    /// Where the log is being written, absent once logging is off.
    pub path: Option<String>,
}

/// What `agents prompt` answers with when it was asked not to wait.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentPromptAccepted {
    pub accepted: bool,
    #[serde(rename = "ref")]
    pub reference: crate::session::protocol::AgentRef,
}

/// What a resolved `agents wait` - or an `agents prompt --wait` - answers with.
///
/// `agent` is absent when the condition that resolved the wait is the agent no longer being there
/// (`gone`), which is the one outcome with nothing left to describe.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentWaitResult {
    pub condition: AgentWaitCondition,
    pub agent: Option<crate::session::protocol::AgentRuntime>,
}

/// What `metrics` answers with from a session server.
///
/// Deliberately not the same document a UI endpoint returns
/// ([`RuntimeMetrics`](crate::runtime_metrics::RuntimeMetrics)): a session server has no client
/// queues, no piped remote, and no orphan-output buffer to describe, so it reports the half of the
/// picture it actually owns rather than padding the other half with nulls.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SessionMetricsReport {
    pub sampled_at_unix_ms: u64,
    pub server: crate::runtime_metrics::CachedServerRuntimeMetrics,
}

/// How prominent a [`ControlCommand::Notify`] toast is.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum NotifyLevel {
    #[default]
    Info,
    Error,
}

impl NotifyLevel {
    pub fn parse_cli(value: &str) -> std::result::Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "error" => Ok(Self::Error),
            other => Err(format!(
                "unknown level `{other}`; expected `info` or `error`"
            )),
        }
    }
}

/// The answer to one control request, in the `{ok, data, error}` shape every control surface
/// speaks.
///
/// `Deserialize` because this crosses the session wire as well as the UI endpoint: a headless
/// [`ClientMessage::SessionControl`](crate::session::protocol::ClientMessage::SessionControl)
/// reply carries one of these, and the CLI reads it back as the same type the UI produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct ControlResponse {
    pub ok: bool,
    /// Stable failure category for scripts. Absent on successful replies and on replies from
    /// older Rozi versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ControlErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            code: None,
            data: Some(serde_json::to_value(data).unwrap_or(serde_json::Value::Null)),
            error: None,
        }
    }
    pub fn empty() -> Self {
        Self {
            ok: true,
            code: None,
            data: None,
            error: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self::error_with(ControlErrorCode::RequestFailed, message)
    }

    pub fn error_with(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: Some(code),
            data: None,
            error: Some(message.into()),
        }
    }
}

/// Machine-readable control failure categories.
///
/// Human text may gain context or be reworded. These kebab-case values are the contract scripts
/// should branch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum ControlErrorCode {
    InvalidRequest,
    RequestTimeout,
    MessageTooLarge,
    ExtensionInactive,
    UnknownEvent,
    PaneNotFound,
    TargetRequired,
    PaneNotRunning,
    SessionNotAttached,
    SessionNotConnected,
    InputLocked,
    ReadOnly,
    NotController,
    Unsupported,
    InvalidArgument,
    SpawnFailed,
    Conflict,
    Unavailable,
    AgentGone,
    AgentBlocked,
    AgentReplaced,
    StaleReference,
    Timeout,
    RequestFailed,
}

#[derive(Clone, Debug)]
pub struct ControlEnvelope {
    pub request: ControlRequest,
    pub reply: mpsc::Sender<ControlResponse>,
}

#[derive(Debug)]
pub struct ControlSocketGuard {
    path: PathBuf,
}

impl ControlSocketGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for ControlSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Runtime endpoint directory. Delegates to [`crate::platform::paths::runtime_dir`] (cross-
/// platform plan Phase 3); the private-directory creation/validation policy itself lives in
/// [`crate::platform::fs_security`].
pub fn runtime_dir() -> std::io::Result<PathBuf> {
    crate::platform::paths::runtime_dir(&crate::platform::paths::PlatformEnv::from_process())
}

#[cfg(all(test, unix))]
fn runtime_dir_with_base(base: Option<PathBuf>) -> std::io::Result<PathBuf> {
    let env = crate::platform::paths::PlatformEnv {
        xdg_runtime_dir: base.filter(|path| path.is_absolute()),
        ..Default::default()
    };
    crate::platform::paths::runtime_dir(&env)
}

pub fn socket_path_for_pid(pid: u32) -> std::io::Result<PathBuf> {
    Ok(EndpointRegistry::control_endpoint(&runtime_dir()?, pid)
        .path()
        .to_path_buf())
}

pub fn bind_control_socket() -> std::io::Result<(IpcListener, ControlSocketGuard)> {
    let path = socket_path_for_pid(std::process::id())?;
    let bound = crate::platform::ipc::IpcEndpoint::at_path(&path).bind()?;
    Ok((bound.into_listener(), ControlSocketGuard { path }))
}

/// Largest UTF-8 JSON line a client may send to Rozi, including the trailing newline.
///
/// This bound is for incoming request and stream-update lines. Replies Rozi writes, including
/// `capture-pane --scrollback full`, are read without it.
pub const MAX_CONTROL_MESSAGE: usize = 1024 * 1024;

const OVERSIZED_CONTROL_MESSAGE: &str = "control message exceeds maximum size";
pub(crate) const MAX_PICK_ROWS: usize = 512;

pub(crate) fn read_control_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    read_delimited_line(reader, Some(MAX_CONTROL_MESSAGE))
}

/// Read one control line Rozi wrote. Capture replies can exceed [`MAX_CONTROL_MESSAGE`].
pub(crate) fn read_control_reply_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    read_delimited_line(reader, None)
}

fn read_delimited_line<R: BufRead>(
    reader: &mut R,
    limit: Option<usize>,
) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let found_newline = append_until_newline(reader, &mut buf, limit)?;
    if buf.is_empty() && !found_newline {
        return Ok(None);
    }
    if found_newline {
        trim_trailing_newline(&mut buf);
    }
    let line = String::from_utf8(buf).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "control message is not valid UTF-8",
        )
    })?;
    Ok(Some(line))
}

fn append_until_newline<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    limit: Option<usize>,
) -> io::Result<bool> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(false);
        }
        if let Some(newline) = available.iter().position(|&byte| byte == b'\n') {
            let take = newline + 1;
            reject_if_over_limit(buf.len() + take, limit)?;
            buf.extend_from_slice(&available[..take]);
            reader.consume(take);
            return Ok(true);
        }
        reject_if_over_limit(buf.len() + available.len(), limit)?;
        buf.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

fn reject_if_over_limit(size: usize, limit: Option<usize>) -> io::Result<()> {
    match limit {
        Some(max) if size > max => Err(oversized_control_line()),
        _ => Ok(()),
    }
}

fn trim_trailing_newline(buf: &mut Vec<u8>) {
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
}

fn oversized_control_line() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, OVERSIZED_CONTROL_MESSAGE)
}

pub(crate) fn is_oversized_control_line(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData && error.to_string() == OVERSIZED_CONTROL_MESSAGE
}

pub fn run_listener(listener: IpcListener, link: CommandLink<Msg>, event_hub: EventHub) {
    listener
        .set_nonblocking(false)
        .expect("control listener supports blocking accept");
    loop {
        match listener.accept() {
            Ok(stream) => {
                let link = link.clone();
                let event_hub = event_hub.clone();
                std::thread::spawn(move || handle_connection(stream, link, event_hub));
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("rozi: control endpoint accept failed: {err}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// How many unread activations a publisher may accumulate before rozi stops keeping them.
///
/// Activations are user clicks, so this is generous relative to how fast anyone can produce them;
/// a publisher that has stopped reading is wedged rather than busy.
const PUBLISH_ACTIVATION_BACKLOG: usize = 32;

/// Serve one pane's `publish` stream until its publisher goes away.
///
/// The only bidirectional command: after the acknowledgement, the publisher writes one row list
/// per line and rozi writes one activation per line back. Both directions run for the life of
/// the connection, so a writer thread carries activations while this thread reads.
fn run_publish_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    stream_id: u64,
    requested_pane: Option<PaneId>,
    extension: Option<crate::config::ExtensionProvenance>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel::<String>(PUBLISH_ACTIVATION_BACKLOG);
    let (ack_tx, ack_rx) = mpsc::channel();
    link.send(Msg::PublishStreamOpen {
        stream_id,
        requested_pane,
        extension,
        sender: tx,
        ack: ack_tx,
    });
    let pane_id = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(pane_id)) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::empty()).unwrap()
            );
            pane_id
        }
        Ok(Err(error)) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error(error)).unwrap()
            );
            return;
        }
        Err(_) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error_with(
                    ControlErrorCode::RequestTimeout,
                    "publish request timed out",
                ))
                .unwrap()
            );
            return;
        }
    };
    // A publisher is silent between state changes, and those can be minutes apart.
    let _ = stream.set_read_timeout(None);
    let writer = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                return;
            }
        }
    });

    for line in control_lines(reader_stream) {
        let line = match line {
            Ok(line) => line,
            Err(error) if is_oversized_control_line(&error) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ControlResponse::error_with(
                        ControlErrorCode::MessageTooLarge,
                        OVERSIZED_CONTROL_MESSAGE,
                    ))
                    .unwrap()
                );
                break;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        // A malformed line is the publisher's bug, not a reason to drop its rows; skip it and keep
        // the stream open so the next good list still lands.
        if let Ok(report) = serde_json::from_str::<PublishReport>(&line) {
            link.send(Msg::PublishedRowsReported {
                stream_id,
                pane_id,
                rows: report.rows,
            });
        }
    }

    // Reaching here means EOF or a read error: the publisher is gone, so its rows go with it.
    link.send(Msg::PublishStreamClosed { stream_id, pane_id });
    drop(stream);
    let _ = writer.join();
}

/// Serve one `pick` stream until the user makes a choice, cancels, or the caller disconnects.
///
/// After acknowledging the request on the UI thread, the caller may write row updates, each
/// replacing the previous set. When the picker closes, rozi writes exactly one terminal JSON line
/// (`{"selected":"…"}` or `{"cancelled":true}`).
///
/// Note: `IpcConnection` exposes no `shutdown()`, so after writing the terminal line rozi cannot
/// force the connection closed from this end; the reader thread reaps once the client closes.
#[allow(clippy::too_many_arguments)]
fn run_pick_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    id: u64,
    title: Option<String>,
    placeholder: Option<String>,
    empty: Option<String>,
    width: Option<u16>,
    actions: Vec<crate::state::PickAction>,
    tabs: Vec<crate::state::PickTab>,
    tab: Option<String>,
    extension: Option<crate::config::ExtensionProvenance>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };

    let (ack_tx, ack_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = crate::state::PickReply::channel();

    link.send(Msg::PickStreamOpen {
        id,
        title,
        placeholder,
        empty,
        width,
        actions,
        tabs,
        tab,
        extension,
        sender: reply_tx,
        ack: ack_tx,
    });

    let ack_response = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => res,
        Err(_) => {
            ControlResponse::error_with(ControlErrorCode::RequestTimeout, "pick request timed out")
        }
    };

    let _ = writeln!(stream, "{}", serde_json::to_string(&ack_response).unwrap());

    if !ack_response.ok {
        return;
    }

    let _ = stream.set_read_timeout(None);

    // Every reply line goes out, not just the first: actions and tab switches keep the picker
    // open and report again later. The loop ends after the terminal line, or when the picker is
    // dropped without one.
    let writer = std::thread::spawn(move || {
        while let Some(line) = reply_rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
    });

    for line in control_lines(reader_stream) {
        let line = match line {
            Ok(line) => line,
            Err(error) if is_oversized_control_line(&error) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ControlResponse::error_with(
                        ControlErrorCode::MessageTooLarge,
                        OVERSIZED_CONTROL_MESSAGE,
                    ))
                    .unwrap()
                );
                break;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<PickReport>(&line) {
            let mut rows = report.rows;
            rows.truncate(MAX_PICK_ROWS);
            link.send(Msg::PickRowsReported {
                id,
                tab: report.tab,
                rows,
            });
        }
    }

    link.send(Msg::PickStreamClosed { id });
    drop(stream);
    let _ = writer.join();
}

/// One line written by a `publish` publisher.
#[derive(Debug, serde::Deserialize)]
struct PublishReport {
    #[serde(default)]
    rows: Vec<crate::session::protocol::PublishedRow>,
}

/// One line written by a `pick` publisher.
#[derive(Debug, serde::Deserialize)]
struct PickReport {
    /// The tab these rows fill. Required when the picker declared tabs, absent otherwise.
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    rows: Vec<crate::state::PickRow>,
}

fn write_control_response(stream: &mut IpcConnection, response: &ControlResponse) {
    let _ = writeln!(stream, "{}", serde_json::to_string(response).unwrap());
}

fn authorize_extension_request(
    stream: &mut IpcConnection,
    link: &CommandLink<Msg>,
    provenance: Option<&crate::config::ExtensionProvenance>,
) -> bool {
    let Some(provenance) = provenance else {
        return true;
    };
    let (reply, authorized) = mpsc::channel();
    link.send(Msg::AuthorizeExtensionControl {
        provenance: provenance.clone(),
        reply,
    });
    if authorized.recv_timeout(Duration::from_secs(10)) == Ok(true) {
        return true;
    }
    write_control_response(
        stream,
        &ControlResponse::error_with(
            ControlErrorCode::ExtensionInactive,
            "extension generation is not active",
        ),
    );
    false
}

fn run_subscription(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    event_hub: EventHub,
    events: &[String],
    extension: Option<crate::config::ExtensionProvenance>,
) {
    let mut kinds = std::collections::HashSet::new();
    for id in events {
        let Some(kind) = EventKind::parse(id) else {
            write_control_response(
                &mut stream,
                &ControlResponse::error_with(
                    ControlErrorCode::UnknownEvent,
                    format!("unknown event `{id}`"),
                ),
            );
            return;
        };
        kinds.insert(kind);
    }

    static NEXT_SUBSCRIPTION_ID: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1);
    let owned_subscription = extension.map(|provenance| {
        let id = NEXT_SUBSCRIPTION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (cancel, cancelled) = mpsc::sync_channel(1);
        let (reply, opened) = mpsc::channel();
        link.send(Msg::ExtensionSubscriptionOpen {
            id,
            provenance,
            cancel,
            reply,
        });
        (id, cancelled, opened)
    });
    if let Some((_, _, opened)) = &owned_subscription
        && opened.recv_timeout(Duration::from_secs(10)) != Ok(true)
    {
        write_control_response(
            &mut stream,
            &ControlResponse::error_with(
                ControlErrorCode::ExtensionInactive,
                "extension generation is not active",
            ),
        );
        return;
    }

    let rx = event_hub.subscribe((!kinds.is_empty()).then_some(kinds));
    write_control_response(&mut stream, &ControlResponse::empty());
    let _ = stream.set_read_timeout(None);
    loop {
        if owned_subscription
            .as_ref()
            .is_some_and(|(_, cancelled, _)| cancelled.try_recv().is_ok())
        {
            break;
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                if writeln!(stream, "{event}").is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Subscribers send nothing after the request. Probe for EOF while idle.
                let mut probe = [0u8; 8];
                let _ = stream.set_nonblocking(true);
                let disconnected = match std::io::Read::read(&mut stream, &mut probe) {
                    Ok(0) => true,
                    Ok(_) => false,
                    Err(err) => err.kind() != std::io::ErrorKind::WouldBlock,
                };
                let _ = stream.set_nonblocking(false);
                if disconnected {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some((id, _, _)) = owned_subscription {
        link.send(Msg::ExtensionSubscriptionClosed { id });
    }
}

fn control_lines(stream: IpcConnection) -> impl Iterator<Item = io::Result<String>> {
    ControlLines {
        reader: BufReader::new(stream),
    }
}

struct ControlLines<R> {
    reader: BufReader<R>,
}

impl<R: io::Read> Iterator for ControlLines<R> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        match read_control_line(&mut self.reader) {
            Ok(Some(line)) => Some(Ok(line)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

fn handle_connection(mut stream: IpcConnection, link: CommandLink<Msg>, event_hub: EventHub) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let line = match read_control_line(&mut reader) {
        Ok(Some(line)) => line,
        Ok(None) => return,
        Err(error) => {
            if error.kind() == io::ErrorKind::InvalidData {
                let code = if is_oversized_control_line(&error) {
                    ControlErrorCode::MessageTooLarge
                } else {
                    ControlErrorCode::InvalidRequest
                };
                write_control_response(
                    &mut stream,
                    &ControlResponse::error_with(code, error.to_string()),
                );
            }
            return;
        }
    };
    let request = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(request) => request,
        Err(err) => {
            write_control_response(
                &mut stream,
                &ControlResponse::error_with(
                    ControlErrorCode::InvalidRequest,
                    format!("invalid request: {err}"),
                ),
            );
            return;
        }
    };
    if !authorize_extension_request(&mut stream, &link, request.extension.as_ref()) {
        return;
    }
    if let ControlCommand::Subscribe { events } = &request.command {
        run_subscription(stream, link, event_hub, events, request.extension.clone());
        return;
    }
    if let ControlCommand::Publish = &request.command {
        static NEXT_PUBLISH_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let stream_id = NEXT_PUBLISH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_publish_stream(
            stream,
            link,
            stream_id,
            request.source_pane,
            request.extension.clone(),
        );
        return;
    }
    if let ControlCommand::Pick {
        title,
        placeholder,
        empty,
        width,
        actions,
        tabs,
        tab,
    } = &request.command
    {
        static NEXT_PICK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_PICK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_pick_stream(
            stream,
            link,
            id,
            title.clone(),
            placeholder.clone(),
            empty.clone(),
            *width,
            actions.clone(),
            tabs.clone(),
            tab.clone(),
            request.extension.clone(),
        );
        return;
    }
    let (tx, rx) = mpsc::channel();
    link.send(Msg::ControlRequest(ControlEnvelope { request, reply: tx }));
    let response = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| {
            ControlResponse::error_with(
                ControlErrorCode::RequestTimeout,
                "control request timed out",
            )
        });
    write_control_response(&mut stream, &response);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[cfg(unix)]
    fn temp_base(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rozi-test-{name}-{}", std::process::id()))
    }

    /// Per-uid, in the temp directory - unless the temp directory has no room for an endpoint, the
    /// macOS case `paths::fallback_runtime_dir_leaves_a_temp_dir_too_small_for_an_endpoint` covers.
    /// Whichever it picks, the point of the directory is that a session can be served from it.
    #[test]
    #[cfg(unix)]
    fn runtime_dir_uses_per_user_temp_fallback_without_xdg() {
        let dir = crate::platform::paths::fallback_runtime_dir_path();
        let name = format!("rozi-{}", crate::platform::fs_security::current_uid());
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), name);

        let longest_endpoint = dir.join(format!(
            "session-{}.sock",
            "a".repeat(crate::session::discovery::MAX_SESSION_NAME_LEN)
        ));
        assert!(
            longest_endpoint.as_os_str().len() <= crate::platform::ipc::MAX_ENDPOINT_PATH_LEN,
            "the fallback runtime directory cannot hold a session endpoint: {}",
            longest_endpoint.display()
        );

        // Where the temp directory has the room - every ordinary Linux and Windows machine - that
        // is still where it goes, rather than a short path nobody asked for.
        let in_temp_dir = std::env::temp_dir().join(&name);
        let temp_dir_has_room = in_temp_dir.as_os_str().len() + longest_endpoint.as_os_str().len()
            - dir.as_os_str().len()
            <= crate::platform::ipc::MAX_ENDPOINT_PATH_LEN;
        if temp_dir_has_room {
            assert_eq!(dir, in_temp_dir);
        }
    }

    #[test]
    #[cfg(unix)]
    fn runtime_dir_rejects_unsafe_existing_directory_without_chmod() {
        let base = temp_base("unsafe");
        let dir = base.join("rozi");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let mode = fs::symlink_metadata(&dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o777);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn run_action_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::RunAction {
                action: "toggle-float".to_string(),
            },
            source_pane: Some(3),
            extension: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"cmd":"run-action","action":"toggle-float","source_pane":3}"#
        );
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);
    }

    #[test]
    fn metrics_command_has_deterministic_json_shape() {
        let request = ControlRequest {
            command: ControlCommand::Metrics,
            source_pane: None,
            extension: None,
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"cmd":"metrics","source_pane":null}"#
        );
        assert_eq!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"metrics"}"#).unwrap(),
            request
        );
    }

    /// `list-panes` is one document whichever endpoint answered it. Both surfaces now fill this
    /// type, so the field set is pinned here rather than drifting apart in two modules - renaming
    /// or dropping one of these is an API change, not an implementation detail.
    #[test]
    fn a_pane_record_has_one_field_set() {
        let pane = PaneInfo {
            session: "dev".to_string(),
            id: 3,
            reference: None,
            agent_ref: None,
            title: "pane 3".to_string(),
            workspace: 1,
            command: None,
            argv: None,
            foreground_program: None,
            foreground_programs: Vec::new(),
            foreground_arguments: Vec::new(),
            cwd: None,
            status: "ready".to_string(),
            reported_status: None,
            status_reason: None,
            agent: None,
            agent_state: None,
        };

        let value = serde_json::to_value(&pane).expect("a pane record serializes");
        let mut keys = value
            .as_object()
            .expect("a pane record is an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();

        assert_eq!(
            keys,
            [
                "agent",
                "agent_ref",
                "agent_state",
                "argv",
                "command",
                "cwd",
                "foreground_arguments",
                "foreground_program",
                "foreground_programs",
                "id",
                "reference",
                "reported_status",
                "session",
                "status",
                "status_reason",
                "title",
                "workspace",
            ]
        );
    }

    #[test]
    fn api_description_is_sorted_and_names_the_session_protocol() {
        let description = ApiDescription::current();
        assert_eq!(description.api, CONTROL_API_VERSION);
        assert_eq!(
            description.session_protocol,
            crate::session::protocol::PROTOCOL_VERSION
        );
        assert!(
            description
                .capabilities
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
    }

    #[test]
    fn error_response_carries_a_stable_code_and_success_does_not() {
        assert_eq!(
            serde_json::to_string(&ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                "pane 3 not found",
            ))
            .unwrap(),
            r#"{"ok":false,"code":"pane-not-found","error":"pane 3 not found"}"#
        );
        assert_eq!(
            serde_json::to_string(&ControlResponse::empty()).unwrap(),
            r#"{"ok":true}"#
        );

        let old: ControlResponse =
            serde_json::from_str(r#"{"ok":false,"error":"old server"}"#).unwrap();
        assert_eq!(old.code, None);
    }

    #[test]
    fn capture_pane_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::CapturePane {
                target: Some(5),
                scrollback: None,
            },
            source_pane: None,
            extension: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);

        let defaulted: ControlRequest = serde_json::from_str(r#"{"cmd":"capture-pane"}"#).unwrap();
        assert_eq!(
            defaulted.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: None
            }
        );

        let with_scrollback: ControlRequest =
            serde_json::from_str(r#"{"cmd":"capture-pane","scrollback":"full"}"#).unwrap();
        assert_eq!(
            with_scrollback.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: Some(CaptureScrollback::Named(CaptureScrollbackNamed::Full))
            }
        );

        let with_last_output: ControlRequest =
            serde_json::from_str(r#"{"cmd":"capture-pane","scrollback":"last-output"}"#).unwrap();
        assert_eq!(
            with_last_output.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: Some(CaptureScrollback::Named(CaptureScrollbackNamed::LastOutput))
            }
        );

        assert!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"capture-pane","scrollback":"100"}"#)
                .is_err()
        );

        let send_keys: ControlRequest =
            serde_json::from_str(r#"{"cmd":"send-keys","keys":["C-c","Enter"]}"#).unwrap();
        assert_eq!(
            send_keys.command,
            ControlCommand::SendKeys {
                target: None,
                keys: vec!["C-c".into(), "Enter".into()],
                literal: false,
            }
        );
    }

    #[test]
    fn switch_and_move_workspace_commands_round_trip_through_json() {
        let switch = ControlRequest {
            command: ControlCommand::SwitchWorkspace { index: 3 },
            source_pane: None,
            extension: None,
        };
        let json = serde_json::to_string(&switch).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            switch
        );

        let move_to = ControlRequest {
            command: ControlCommand::MoveToWorkspace { index: 4 },
            source_pane: None,
            extension: None,
        };
        let json = serde_json::to_string(&move_to).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            move_to
        );
    }

    #[test]
    fn popup_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::Popup {
                command: "fzf".into(),
                cwd: Some("/tmp".into()),
                width: Some(0.7),
                height: Some(0.5),
                title: Some("pick".into()),
                keep_open: Some(false),
            },
            source_pane: None,
            extension: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            request
        );
    }

    #[test]
    fn set_status_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::SetStatus {
                target: Some(7),
                status: Some("blocked".into()),
                reason: Some("needs approval".into()),
            },
            source_pane: Some(3),
            extension: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            request
        );
        assert_eq!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"set-status"}"#)
                .unwrap()
                .command,
            ControlCommand::SetStatus {
                target: None,
                status: None,
                reason: None,
            }
        );
    }

    #[test]
    fn pick_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::Pick {
                title: Some("Branch".into()),
                placeholder: Some("Search branches…".into()),
                empty: None,
                width: None,
                actions: Vec::new(),
                tabs: Vec::new(),
                tab: None,
            },
            source_pane: None,
            extension: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"cmd":"pick","title":"Branch","placeholder":"Search branches…","source_pane":null}"#
        );
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_dir_rejects_symlink() {
        let base = temp_base("symlink");
        let dir = base.join("rozi");
        let target = temp_base("symlink-target");
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(target);
    }

    #[test]
    fn read_control_line_accepts_a_json_line() {
        let mut reader = io::Cursor::new(b"{\"cmd\":\"metrics\"}\n");
        assert_eq!(
            read_control_line(&mut reader).unwrap().as_deref(),
            Some("{\"cmd\":\"metrics\"}")
        );
        assert_eq!(read_control_line(&mut reader).unwrap(), None);
    }

    #[test]
    fn read_control_line_rejects_an_oversized_line() {
        let mut payload = vec![b'x'; MAX_CONTROL_MESSAGE];
        payload.push(b'\n');
        let mut reader = io::Cursor::new(payload);
        let error = read_control_line(&mut reader).unwrap_err();
        assert!(is_oversized_control_line(&error));
    }

    #[test]
    fn capture_pane_reply_larger_than_the_request_limit_still_reads() {
        let text = "x".repeat(MAX_CONTROL_MESSAGE);
        let mut line = serde_json::to_string(&serde_json::json!({
            "ok": true,
            "data": { "id": 1, "text": text, "title": null }
        }))
        .unwrap();
        assert!(
            line.len() > MAX_CONTROL_MESSAGE,
            "the encoded capture must exceed the incoming request cap"
        );
        line.push('\n');
        let bytes = line.into_bytes();

        let error = read_control_line(&mut io::Cursor::new(bytes.clone())).unwrap_err();
        assert!(is_oversized_control_line(&error));

        let got = read_control_reply_line(&mut io::Cursor::new(bytes))
            .unwrap()
            .expect("reply line");
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();
        assert_eq!(
            value["data"]["text"].as_str().unwrap().len(),
            MAX_CONTROL_MESSAGE
        );
    }
}
