use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control;
use crate::platform::ipc::{EndpointRegistry, IpcConnection, IpcEndpoint, IpcListener};
use crate::runtime_metrics::{
    ByteBufferMetrics, QueueMetrics, ResurrectionMetrics, ServerOutboxMetrics,
    ServerRuntimeMetrics, unix_time_millis,
};
use crate::session::protocol::{
    self, ClientInfo, ClientMessage, ControllerChangeReason, Frame, PROTOCOL_VERSION, PaneMeta,
    ServerMessage, WirePalette,
};
use crate::session::queue::ByteQueue;
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

mod browse;
mod connection;
mod lease;
mod panes;
mod resurrect;
pub(crate) use resurrect::list_snapshot_names_by_recency;
mod runtime;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;
const DEFAULT_SCROLLBACK: usize = 5000;
/// How long an *ephemeral* session server survives with no client attached before it self-reaps,
/// regardless of pane state. This is only a crash/abnormal-exit backstop: a clean quit or normal
/// transition tears an ephemeral server down client-side (`ClientMessage::Shutdown`). A *named*
/// session never self-reaps from client absence - it is durable until explicitly killed.
const EPHEMERAL_NO_CLIENT_GRACE: Duration = Duration::from_secs(45);
/// How often the server pings each attached client.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Default time a client may go without a pong before it is disconnected (and its lease released).
/// A wedged UI loses control deliberately; a merely busy one has ample slack.
const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);
/// Server work below this duration is ordinary scheduling overhead. Longer pauses are excluded from
/// client heartbeat deadlines because the server itself could not exchange heartbeat frames.
const HEARTBEAT_STALL_THRESHOLD: Duration = Duration::from_millis(100);
const MAX_PTY_EVENTS_PER_TICK: usize = 256;
const MAX_PTY_INGRESS_BYTES: usize = 4 * 1024 * 1024;
const MAX_COALESCED_PANE_BYTES: usize = 64 * 1024;
/// Minimum spacing between control-request notifications to the controller from the same requester,
/// so a held `request-control` key raises one toast rather than a stream (the roster badge is sticky
/// regardless).
const REQUEST_NOTIFY_COOLDOWN: Duration = Duration::from_secs(4);
/// Default per-client outbox cap; a client backed up past this is disconnected so it can never
/// stall the broadcast to everyone else.
const DEFAULT_MAX_BACKLOG: usize = 8 * 1024 * 1024;
/// Larger cap while a client is still receiving its initial replay seed.
const SEED_MAX_BACKLOG: usize = 64 * 1024 * 1024;
const SEED_CHUNK: usize = 256 * 1024;

pub struct SessionServer {
    panes: HashMap<PaneId, ServerPane>,
    next_generation: u64,
    layout: Option<SharedLayout>,
    layout_rev: u64,
    /// Immutable origin metadata claimed by the profile client that first seeds an empty session.
    created_from_profile: Option<String>,
    origin_seed_client: Option<ClientId>,
    controller: Option<ClientId>,
    input_locked: bool,
    allow_takeover: bool,
    clients: Vec<ClientConn>,
    next_client_id: ClientId,
    max_backlog: usize,
    events: Arc<ByteQueue<ServerEvent>>,
    /// Aggregate high-water across every client outbox for this server process's lifetime.
    outbox_high_water_bytes: usize,
    resurrection_metrics: ResurrectionMetrics,
    shutdown: bool,
    forget_snapshot: bool,
    /// Bumped by every change a snapshot must capture. Compared against `snapshot_generation`
    /// rather than cleared, so output arriving while a snapshot is being written cannot be
    /// mistaken for output the snapshot already contains.
    dirty_generation: u64,
    /// The generation the last *successful* snapshot persisted.
    snapshot_generation: u64,
    snapshot_worker: Option<resurrect::SnapshotWorker>,
    /// Per-pane `content_generation` captured by the last *successful* snapshot, and therefore the
    /// generation whose replay file the live snapshot directory currently holds. Cleared whenever
    /// a snapshot fails, so a broken or externally removed directory self-heals into a full export.
    persisted_replays: HashMap<PaneId, u64>,
    last_snapshot: Instant,
    last_runtime_poll: Instant,
    last_attached_count: u32,
    session_name: String,
    /// The endpoint this server currently listens on. Set by [`run_named_session`]; a rename
    /// replaces it (see [`SessionServer::rename_session`]).
    endpoint: Option<IpcEndpoint>,
    /// A listener bound to a *new* endpoint by a rename, plus the old endpoint it displaces, waiting
    /// for the accept loop to swap them in one step (see [`SessionServer::rename_session`]). The old
    /// endpoint is retired only at the swap, so it keeps answering right up until the moment the
    /// listener behind it is dropped.
    pending_listener: Option<(IpcListener, Option<IpcEndpoint>)>,
    settings: ServerSettings,
}

#[derive(Clone, Debug)]
pub struct ServerSettings {
    pub log_dir: Option<PathBuf>,
    pub resurrect: bool,
    pub snapshot_dir: Option<PathBuf>,
    pub snapshot_interval: Duration,
    /// Maximum time an attached client may go without a heartbeat pong.
    pub heartbeat_timeout: Duration,
    /// Whether a writable follower's control request immediately transfers the lease. Mirrors
    /// [`crate::config::HyprmuxSessionConfig::allow_takeover`], including its `true` default, so a
    /// server started without a config behaves like one started from the default config.
    pub allow_takeover: bool,
    /// Scrollback retained by each server-side terminal parser.
    pub scrollback: usize,
    /// Resolved interactive-shell/command-runner argv used only for snapshot restore ([`resurrect::restore`]),
    /// which respawns panes with no controlling client yet connected to resolve them - the
    /// server's own config load is the only launch-policy source available at that point. Empty
    /// (the default) falls through to `pty_config`'s own `/bin/sh` fallback.
    ///
    /// Not yet persisted in the snapshot itself; the cross-platform plan calls for bumping the
    /// snapshot format only if resolved launch policy must be persisted, which restoring against
    /// the server's current config (rather than whatever was resolved when the pane was
    /// originally spawned) does not yet require.
    pub shell: Vec<String>,
    pub command_shell: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            log_dir: None,
            resurrect: false,
            snapshot_dir: None,
            snapshot_interval: Duration::from_secs(30),
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            allow_takeover: true,
            scrollback: DEFAULT_SCROLLBACK,
            shell: Vec::new(),
            command_shell: Vec::new(),
        }
    }
}

pub struct PaneLog {
    file: File,
    path: PathBuf,
}

pub struct ServerPane {
    pub generation: u64,
    pub title: Option<String>,
    /// Launch-time working directory (tier 3 of [`protocol::PaneCwdSource`]'s precedence order) -
    /// what the pane was spawned with, before any live OSC report or process-inspector fallback.
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub keep_open: bool,
    /// Set once a `keep_open` pane's command has finished and its PTY has been replaced by the
    /// interactive shell (see [`SessionServer::replace_with_keep_open_shell`]). Without it, the
    /// *shell's* eventual exit would be mistaken for the command's and trigger a second
    /// replacement - a pane you could never close.
    pub command_completed: bool,
    pub palette: WirePalette,
    pub pty: Option<TerminalPty>,
    /// Reached through [`ServerPane::screen_mut`] / [`ServerPane::screen_without_change`] rather
    /// than directly, so a content change cannot silently skip `content_generation`.
    terminal: TerminalScreen,
    /// Bumped by every change to what this pane's replay bytes would contain.
    ///
    /// A snapshot reuses the replay file already on disk when this still matches the generation
    /// that file was written from, which is what lets a session with one busy pane and a dozen
    /// idle ones avoid re-exporting all thirteen.
    content_generation: u64,
    pub cols: u16,
    pub rows: u16,
    /// Host cell size in pixels, as reported by the controller and handed to the PTY.
    ///
    /// The child reads it out of `TIOCGWINSZ` to size images in cells; the client rendering those
    /// images measures against the same value, so what the child reserved and what the pane draws
    /// agree.
    pub cell: tui_lipan::TerminalCellSize,
    pub exited: Option<i32>,
    pub log: Option<PaneLog>,
    /// The resolved interactive shell and spawn environment this pane was launched with, kept so a
    /// `keep_open` replacement can start the same shell the client asked for - resolved from the
    /// client's live config, with shell integration already injected - rather than re-deriving it
    /// from the detached server's own stale environment.
    pub shell: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Cached result of the last [`SessionServer::sync_pane_runtime`] call; kept
    /// per-pane so `pane_meta()` can hand it out without re-deriving it from scratch on every call,
    /// and so change detection has a "previous value" to diff against.
    pub runtime: protocol::PaneRuntimeState,
    /// Foreground identity and time of the last agent detection, so an idle pane can skip it.
    ///
    /// Detection sweeps every process on the host to find this pane's process-group members, which
    /// at the 250 ms poll rate cost ~2% of a core per pane while nothing was happening. The
    /// foreground program and command phase are both already computed cheaply, so an unchanged
    /// pair means the sweep would rediscover exactly what is cached. See
    /// [`AGENT_DETECT_REFRESH`](super::runtime::AGENT_DETECT_REFRESH) for the safety net that
    /// still catches a wrapped process appearing without either changing.
    pub last_agent_probe: Option<AgentProbe>,
    /// When detection last actually swept, for the periodic refresh.
    pub last_agent_detect: Option<std::time::Instant>,
    /// When this pane's project root and branch were last read from disk. A checkout changes the
    /// branch without the working directory moving, so unlike the rest of the runtime state this
    /// cannot be driven by a cwd change alone; see
    /// [`GIT_REFRESH`](super::runtime::GIT_REFRESH).
    pub last_git_read: Option<std::time::Instant>,
    /// The framework answered ConPTY's startup cursor query before spawning. Suppress the parser's
    /// later duplicate cursor report so it cannot leak into child stdin.
    pub initial_cursor_report_primed: bool,
}

/// Cheap foreground fingerprint used to decide whether agent detection can be skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProbe {
    pub foreground_program: Option<String>,
    pub command_phase: protocol::PaneCommandPhase,
}

/// One attached (or connecting) client. The stream is non-blocking; outbound frames are
/// pre-encoded and queued in `outbox`, flushed opportunistically so a slow reader never blocks the
/// server or the other clients.
struct ClientConn {
    id: ClientId,
    stream: IpcConnection,
    decoder: protocol::FrameDecoder,
    outbox: VecDeque<Arc<[u8]>>,
    outbox_bytes: usize,
    /// Bytes of `outbox.front()` already written (non-blocking writes can be partial).
    front_offset: usize,
    attached: bool,
    label: Option<String>,
    read_only: bool,
    /// True while the initial replay seed is still queued; raises the backlog cap.
    seeding: bool,
    /// Close this connection once its outbox drains (query probes, rejected attaches).
    close_after_flush: bool,
    last_pong: Instant,
    last_ping: Instant,
    ping_seq: u64,
    /// Negotiated wire protocol for this attached client. Zero until attach succeeds.
    effective_protocol: u32,
    /// True while this client has an unanswered request for the control lease.
    requesting_control: bool,
    /// True while this client keeps the session open in the background instead of using it. A
    /// parked client stays attached and keeps receiving output, but never holds the control lease
    /// and is skipped when the lease has to move, so it does not occupy the session (protocol 14+;
    /// always false against an older client, which cannot say it is parked).
    parked: bool,
    /// When the controller was last notified of this client's request, for per-requester debounce.
    last_request_notify: Option<Instant>,
}

impl ClientConn {
    fn new(id: ClientId, stream: IpcConnection) -> Self {
        let now = Instant::now();
        Self {
            id,
            stream,
            decoder: protocol::FrameDecoder::default(),
            outbox: VecDeque::new(),
            outbox_bytes: 0,
            front_offset: 0,
            attached: false,
            label: None,
            read_only: false,
            seeding: false,
            close_after_flush: false,
            last_pong: now,
            last_ping: now,
            ping_seq: 0,
            effective_protocol: 0,
            requesting_control: false,
            parked: false,
            last_request_notify: None,
        }
    }

    fn try_push(&mut self, bytes: Arc<[u8]>, default_cap: usize) -> bool {
        if self.outbox_bytes.saturating_add(bytes.len()) > self.backlog_cap(default_cap) {
            return false;
        }
        self.outbox_bytes += bytes.len();
        self.outbox.push_back(bytes);
        true
    }

    fn backlog_cap(&self, default: usize) -> usize {
        if self.seeding {
            SEED_MAX_BACKLOG
        } else {
            default
        }
    }
}

#[derive(Debug)]
enum ServerEvent {
    Pty(PaneId, u64, TerminalPtyEvent),
}

impl ServerEvent {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Pty(_, _, TerminalPtyEvent::Output(bytes)) => bytes.len(),
            Self::Pty(_, _, TerminalPtyEvent::Exited(_) | TerminalPtyEvent::Error(_)) => 0,
        }
    }

    fn coalesce_output(&mut self, next: &Self) -> bool {
        let (
            Self::Pty(id, generation, TerminalPtyEvent::Output(bytes)),
            Self::Pty(next_id, next_generation, TerminalPtyEvent::Output(next_bytes)),
        ) = (self, next)
        else {
            return false;
        };
        if id != next_id
            || generation != next_generation
            || bytes.len().saturating_add(next_bytes.len()) > MAX_COALESCED_PANE_BYTES
        {
            return false;
        }
        let mut combined = Vec::with_capacity(bytes.len() + next_bytes.len());
        combined.extend_from_slice(bytes);
        combined.extend_from_slice(next_bytes);
        *bytes = Arc::from(combined);
        true
    }
}

enum ServerOutbound {
    /// Boxed because `PaneOutput` is the variant that actually flows in volume: an inline control
    /// message would make every byte-carrying move pay the largest control message's footprint.
    Control(Box<ServerMessage>),
    PaneOutput {
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
}

impl ServerOutbound {
    fn control(message: ServerMessage) -> Self {
        Self::Control(Box::new(message))
    }
}

/// Where a response frame should go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Just the client that sent the triggering message.
    Sender,
    /// A specific client by id (e.g. the controller, or a request's originator).
    Client(ClientId),
    /// Every attached client.
    Broadcast,
}

impl SessionServer {
    #[cfg(all(test, unix))]
    pub fn new_named(session_name: impl Into<String>) -> Self {
        Self::new_named_with_settings(session_name, ServerSettings::default())
    }

    pub fn new_named_with_settings(
        session_name: impl Into<String>,
        settings: ServerSettings,
    ) -> Self {
        let session_name = session_name.into();
        let events = Arc::new(ByteQueue::new(MAX_PTY_INGRESS_BYTES));
        Self {
            panes: HashMap::new(),
            next_generation: 1,
            layout: None,
            layout_rev: 0,
            created_from_profile: None,
            origin_seed_client: None,
            controller: None,
            input_locked: false,
            allow_takeover: settings.allow_takeover,
            clients: Vec::new(),
            next_client_id: 1,
            max_backlog: DEFAULT_MAX_BACKLOG,
            events,
            outbox_high_water_bytes: 0,
            resurrection_metrics: ResurrectionMetrics::default(),
            shutdown: false,
            forget_snapshot: false,
            dirty_generation: 0,
            snapshot_generation: 0,
            snapshot_worker: None,
            persisted_replays: HashMap::new(),
            last_snapshot: Instant::now(),
            last_runtime_poll: Instant::now(),
            last_attached_count: 0,
            session_name,
            endpoint: None,
            pending_listener: None,
            settings,
        }
    }

    pub fn run_listener(&mut self, listener: IpcListener) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        let mut listener = listener;
        // Tracks how long an ephemeral session has had no *attached* client. A named session
        // ignores this timer and is durable until explicitly shut down.
        let mut no_client_since: Option<Instant> = None;
        while !self.shutdown {
            let iteration_started = Instant::now();
            // A rename binds the new endpoint before the old listener is retired, so no window
            // exists where the session is discoverable under neither name. Dropping the old
            // listener here does not disturb already-accepted connections: existing clients stay
            // attached across a rename.
            if let Some((next, retired)) = self.pending_listener.take() {
                listener = next;
                if let Some(retired) = retired {
                    retired.remove_stale();
                }
            }
            // A signal (Unix) or console control event (Windows) asking this server to stop routes
            // onto the same graceful teardown the authenticated `Shutdown` message takes, rather
            // than killing the process mid-write and stranding its PTY children.
            if crate::platform::server_lifecycle::shutdown_requested() {
                self.shutdown = true;
                break;
            }
            self.accept_new(&listener)?;

            for _ in 0..MAX_PTY_EVENTS_PER_TICK {
                let Some(event) = self.events.try_pop() else {
                    break;
                };
                if let Some(outbound) = self.handle_event(event) {
                    self.broadcast_outbound(&outbound);
                }
            }

            self.pump_clients();
            self.poll_pane_runtime();
            if let Some((next, retired)) = self.pending_listener.take() {
                listener = next;
                if let Some(retired) = retired {
                    retired.remove_stale();
                }
            }
            if let Err(err) = self.drain_snapshot_results() {
                eprintln!("hyprmux: session snapshot failed: {err}");
            }
            if let Err(err) = self.maybe_snapshot() {
                eprintln!("hyprmux: session snapshot failed: {err}");
            }
            let iteration_elapsed = iteration_started.elapsed();
            self.credit_server_stall(iteration_elapsed);
            self.heartbeat();
            self.flush_clients();

            let attached = self.attached_count();
            if attached == 0 {
                if crate::state::is_ephemeral_session_name(&self.session_name) {
                    let since = no_client_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= EPHEMERAL_NO_CLIENT_GRACE {
                        self.shutdown = true;
                    }
                }
            } else {
                no_client_since = None;
            }

            let idle = if self.clients.is_empty() { 20 } else { 1 };
            std::thread::sleep(Duration::from_millis(idle));
        }
        // A snapshot is most often triggered by the last client detaching, so let an in-flight
        // durable write land before the process exits. Ordered before `forget_snapshot` so a
        // session being forgotten deletes the finished snapshot rather than racing it.
        self.finish_snapshots();
        if self.forget_snapshot
            && let Err(err) = self.delete_snapshot()
        {
            eprintln!("hyprmux: could not delete session snapshot: {err}");
        }
        for pane in self.panes.values() {
            if let Some(pty) = &pane.pty {
                let _ = pty.kill();
            }
        }
        Ok(())
    }

    pub(crate) fn runtime_metrics(&self) -> ServerRuntimeMetrics {
        let ingress = self.events.stats();
        let current_outbox = self
            .clients
            .iter()
            .map(|client| client.outbox_bytes)
            .sum::<usize>();
        let outbox_capacity = self
            .clients
            .iter()
            .map(|client| client.backlog_cap(self.max_backlog))
            .sum::<usize>();
        ServerRuntimeMetrics {
            sampled_at_unix_ms: unix_time_millis(),
            pty_ingress: QueueMetrics {
                bytes: ByteBufferMetrics::new(
                    ingress.bytes,
                    ingress.high_water_bytes,
                    ingress.capacity,
                ),
                queued_items: ingress.len as u64,
            },
            client_outboxes: ServerOutboxMetrics {
                bytes: ByteBufferMetrics::new(
                    current_outbox,
                    self.outbox_high_water_bytes,
                    outbox_capacity,
                ),
                clients: self.clients.len() as u64,
            },
            resurrection: self.resurrection_metrics,
        }
    }

    fn note_outbox_high_water(&mut self) {
        let current = self
            .clients
            .iter()
            .map(|client| client.outbox_bytes)
            .sum::<usize>();
        self.outbox_high_water_bytes = self.outbox_high_water_bytes.max(current);
    }
}

impl Drop for SessionServer {
    fn drop(&mut self) {
        self.events.close();
    }
}

fn encode_control(message: &ServerMessage) -> Option<Arc<[u8]>> {
    let mut buf = Vec::new();
    protocol::write_frame(&mut buf, message)
        .ok()
        .map(|()| Arc::from(buf))
}

fn encode_pane_output(pane_id: PaneId, generation: u64, bytes: &[u8]) -> Option<Arc<[u8]>> {
    let mut buf = Vec::new();
    protocol::write_pane_output_frame(&mut buf, pane_id, generation, bytes)
        .ok()
        .map(|()| Arc::from(buf))
}

struct SpawnRequest {
    pane_id: PaneId,
    generation: u64,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    cols: u16,
    rows: u16,
    keep_open: bool,
    env: Vec<(String, String)>,
    palette: WirePalette,
    shell: Vec<String>,
    command_shell: Vec<String>,
    /// Host cell size, when the client reported one.
    cell: Option<tui_lipan::TerminalCellSize>,
}

/// A reported cell size, or `None` when the peer sent none (pre-17 clients send zeroes).
fn cell_size(width: u16, height: u16) -> Option<tui_lipan::TerminalCellSize> {
    (width > 0 && height > 0).then(|| tui_lipan::TerminalCellSize::new(width, height))
}

impl ServerPane {
    /// The pane's terminal, for reading.
    pub(super) fn screen(&self) -> &TerminalScreen {
        &self.terminal
    }

    /// The pane's terminal, for an operation that changes what a snapshot would persist.
    ///
    /// Every content change goes through here. `content_generation` is the only thing telling a
    /// snapshot that the replay file already on disk is stale, so a change that bypassed it would
    /// be persisted-scrollback loss on the next resurrect.
    pub(super) fn screen_mut(&mut self) -> &mut TerminalScreen {
        self.content_generation = self.content_generation.saturating_add(1);
        &mut self.terminal
    }

    /// The pane's terminal, for an operation that needs `&mut` but leaves persisted content
    /// identical: rendering a snapshot, draining queued responses or semantic events, and
    /// exporting replay bytes - which swaps the alt grid out and back but restores it.
    ///
    /// Prefer [`Self::screen_mut`] whenever there is any doubt; over-exporting a pane costs time,
    /// while under-exporting one loses its scrollback.
    pub(super) fn screen_without_change(&mut self) -> &mut TerminalScreen {
        &mut self.terminal
    }

    fn effective_title(&self) -> Option<String> {
        self.terminal
            .title()
            .and_then(crate::pane::sanitize_terminal_title)
            .or_else(|| self.title.clone())
    }

    /// The best *local* cwd known for this pane, suitable for `Command::current_dir` on a
    /// respawn (session resurrection, keep-open restart): the live-tracked runtime cwd when it
    /// does not name a remote host, else the pane's original launch directory. Never returns a
    /// remote `OSC 7` path - see [`protocol::PaneRuntimeState`]'s doc comment.
    pub(super) fn spawnable_cwd(&self) -> Option<String> {
        if self.runtime.cwd_host.is_none() {
            self.runtime.cwd.clone().or_else(|| self.cwd.clone())
        } else {
            self.cwd.clone()
        }
    }
}

/// Build the PTY spawn config from the client-resolved `shell`/`command_shell` argv (see
/// [`crate::platform::command`]). Empty argv falls back to the current platform's resolved policy.
///
/// A pane with a `command` runs it through the deterministic `command_shell` runner; a pane without
/// one runs the resolved interactive `shell`.
///
/// `keep_open` is deliberately *not* handled here, and must not be: interpolating the shell into the
/// command line (`command; exec <shell>`) would bind this to POSIX shell syntax - `exec` does not
/// exist in cmd.exe or PowerShell, and `;` does not separate commands in cmd - and would swallow the
/// command's exit status into the `exec`, leaving nothing to report. Keep-open is a server-driven
/// PTY replacement after the command exits instead; see
/// [`SessionServer::replace_with_keep_open_shell`].
fn pty_config(
    command: Option<&str>,
    shell: &[String],
    command_shell: &[String],
) -> TerminalPtyConfig {
    use crate::platform::command::ShellCommand;

    let env = crate::platform::command::ShellEnv::from_process();

    if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
        let runner = ShellCommand::from_argv(command_shell)
            .unwrap_or_else(|| crate::platform::command::resolve_command_shell(None, &env));
        let mut config = TerminalPtyConfig::new(runner.program).term("xterm-256color");
        for arg in runner.args {
            config = config.arg(arg);
        }
        config.arg(command.to_string())
    } else {
        let shell = ShellCommand::from_argv(shell)
            .unwrap_or_else(|| crate::platform::command::resolve_interactive_shell(None, &env));
        let mut config = TerminalPtyConfig::new(shell.program).term("xterm-256color");
        for arg in shell.args {
            config = config.arg(arg);
        }
        config
    }
}

pub fn session_socket_path(name: &str) -> io::Result<PathBuf> {
    Ok(session_endpoint(name)?.path().to_path_buf())
}

pub fn session_endpoint(name: &str) -> io::Result<IpcEndpoint> {
    if !crate::session::discovery::valid_attach_target(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    Ok(EndpointRegistry::session_endpoint(
        &control::runtime_dir()?,
        name,
    ))
}

pub fn bind_session_socket(name: &str) -> io::Result<(IpcListener, IpcEndpoint)> {
    let endpoint = session_endpoint(name)?;
    let bound = endpoint.bind()?;
    Ok((bound.into_listener(), endpoint))
}

pub fn run_named_session(name: &str) -> io::Result<()> {
    run_named_session_mode(name, false)
}

pub fn run_named_session_mode(name: &str, fresh: bool) -> io::Result<()> {
    if !crate::session::discovery::valid_attach_target(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    // Before anything can spawn a PTY: contain this process's children so a crashed or forcibly
    // terminated server cannot strand them (Windows Job Object; no-op on Unix), and route a stop
    // signal / console control event onto the same graceful teardown the protocol `Shutdown`
    // message takes (cross-platform plan Phase 5b).
    if let Err(err) = crate::platform::server_lifecycle::contain_children() {
        eprintln!("hyprmux: could not contain server child processes: {err}");
    }
    if let Err(err) = crate::platform::server_lifecycle::install_shutdown_handler() {
        eprintln!("hyprmux: could not install server shutdown handler: {err}");
    }

    let (listener, endpoint) = bind_session_socket(name)?;
    let loaded = crate::config::load_config();
    for warning in loaded.warnings {
        eprintln!("hyprmux: {warning}");
    }
    let (shell, command_shell) = crate::platform::command::resolve_launch_argv(
        loaded.config.shell.as_deref(),
        loaded.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    let mut server = SessionServer::new_named_with_settings(
        name,
        ServerSettings {
            log_dir: loaded.config.logging.dir,
            resurrect: loaded.config.session.resurrect,
            allow_takeover: loaded.config.session.allow_takeover,
            scrollback: loaded.config.scrollback,
            shell,
            command_shell,
            ..ServerSettings::default()
        },
    );
    server.endpoint = Some(endpoint);
    if fresh {
        delete_snapshot(name)?;
    } else if server.settings.resurrect
        && let Err(err) = server.restore()
    {
        eprintln!("hyprmux: could not restore session {name:?}: {err}");
    }
    let result = server.run_listener(listener);
    // A rename replaces the endpoint, so retire the current one rather than the original.
    if let Some(endpoint) = &server.endpoint {
        endpoint.remove_stale();
    }
    result
}

pub fn delete_snapshot(name: &str) -> io::Result<()> {
    resurrect::delete_snapshot_for(name)
}

#[cfg(all(test, unix))]
mod tests;
