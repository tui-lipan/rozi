use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control;
use crate::session::protocol::{
    self, ClientInfo, ClientMessage, ControllerChangeReason, Frame, PROTOCOL_VERSION, PaneMeta,
    ServerMessage, WirePalette,
};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

mod connection;
mod lease;
mod panes;
mod resurrect;

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
/// How long a client may go without a pong before it is disconnected (and its lease released). A
/// wedged UI loses control deliberately; a merely busy one has ample slack.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);
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
    controller: Option<ClientId>,
    input_locked: bool,
    clients: Vec<ClientConn>,
    next_client_id: ClientId,
    max_backlog: usize,
    event_rx: mpsc::Receiver<ServerEvent>,
    event_tx: mpsc::Sender<ServerEvent>,
    shutdown: bool,
    forget_snapshot: bool,
    dirty: bool,
    last_snapshot: Instant,
    last_attached_count: u32,
    session_name: String,
    /// The socket file this server currently listens on. Set by [`run_named_session`]; a rename
    /// moves this file in place so the running listener keeps serving under the new name.
    socket_path: Option<PathBuf>,
    settings: ServerSettings,
}

#[derive(Clone, Debug)]
pub struct ServerSettings {
    pub log_dir: Option<PathBuf>,
    pub resurrect: bool,
    pub snapshot_dir: Option<PathBuf>,
    pub snapshot_interval: Duration,
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
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub keep_open: bool,
    pub palette: WirePalette,
    pub pty: Option<TerminalPty>,
    pub screen: TerminalScreen,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
    pub log: Option<PaneLog>,
}

/// One attached (or connecting) client. The stream is non-blocking; outbound frames are
/// pre-encoded and queued in `outbox`, flushed opportunistically so a slow reader never blocks the
/// server or the other clients.
struct ClientConn {
    id: ClientId,
    stream: UnixStream,
    decoder: protocol::FrameDecoder,
    outbox: VecDeque<Vec<u8>>,
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
    /// True while this client has an unanswered request for the control lease.
    requesting_control: bool,
    /// When the controller was last notified of this client's request, for per-requester debounce.
    last_request_notify: Option<Instant>,
}

impl ClientConn {
    fn new(id: ClientId, stream: UnixStream) -> Self {
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
            requesting_control: false,
            last_request_notify: None,
        }
    }

    fn push(&mut self, bytes: Vec<u8>) {
        self.outbox_bytes += bytes.len();
        self.outbox.push_back(bytes);
    }

    fn backlog_cap(&self, default: usize) -> usize {
        if self.seeding {
            SEED_MAX_BACKLOG
        } else {
            default
        }
    }
}

enum ServerEvent {
    Pty(PaneId, u64, TerminalPtyEvent),
}

enum ServerOutbound {
    Control(ServerMessage),
    PaneOutput {
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
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
    #[cfg(test)]
    pub fn new_named(session_name: impl Into<String>) -> Self {
        Self::new_named_with_settings(session_name, ServerSettings::default())
    }

    pub fn new_named_with_settings(
        session_name: impl Into<String>,
        settings: ServerSettings,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            panes: HashMap::new(),
            next_generation: 1,
            layout: None,
            layout_rev: 0,
            controller: None,
            input_locked: false,
            clients: Vec::new(),
            next_client_id: 1,
            max_backlog: DEFAULT_MAX_BACKLOG,
            event_rx,
            event_tx,
            shutdown: false,
            forget_snapshot: false,
            dirty: false,
            last_snapshot: Instant::now(),
            last_attached_count: 0,
            session_name: session_name.into(),
            socket_path: None,
            settings,
        }
    }

    pub fn run_listener(&mut self, listener: UnixListener) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        // Tracks how long an ephemeral session has had no *attached* client. A named session
        // ignores this timer and is durable until explicitly shut down.
        let mut no_client_since: Option<Instant> = None;
        while !self.shutdown {
            self.accept_new(&listener)?;

            while let Ok(event) = self.event_rx.try_recv() {
                if let Some(outbound) = self.handle_event(event) {
                    self.broadcast_outbound(&outbound);
                }
            }

            self.pump_clients();
            if let Err(err) = self.maybe_snapshot() {
                eprintln!("hyprmux: session snapshot failed: {err}");
            }
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

            let idle = if self.clients.is_empty() { 20 } else { 6 };
            std::thread::sleep(Duration::from_millis(idle));
        }
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
}

fn encode_control(message: &ServerMessage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    protocol::write_frame(&mut buf, message).ok().map(|()| buf)
}

fn encode_pane_output(pane_id: PaneId, generation: u64, bytes: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    protocol::write_pane_output_frame(&mut buf, pane_id, generation, bytes)
        .ok()
        .map(|()| buf)
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
}

impl ServerPane {
    fn effective_title(&self) -> Option<String> {
        self.screen.title().or_else(|| self.title.clone())
    }

    fn effective_cwd(&self) -> Option<String> {
        self.pty
            .as_ref()
            .and_then(|pty| pty.pid())
            .and_then(cwd_for_pid)
            .or_else(|| self.cwd.clone())
    }
}

#[cfg(target_os = "linux")]
fn cwd_for_pid(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

#[cfg(not(target_os = "linux"))]
fn cwd_for_pid(_pid: u32) -> Option<String> {
    None
}

/// Build the PTY spawn config from the client-resolved `shell`/`command_shell` argv (see
/// [`crate::platform::command`]). Falls back to a bare `/bin/sh` if a caller ever sends an empty
/// argv (should not happen - the client always resolves a non-empty one - but a spawn config
/// needs *some* program either way).
///
/// `keep_open` with a `command` runs the command through `command_shell`, then `exec`s into the
/// resolved interactive `shell` on completion so the pane stays open - the historical
/// `command; exec shell` behavior, now correctly using the deterministic command runner rather
/// than the interactive shell to run the one-off command. This is an interim, string-interpolated
/// implementation preserved as-is from before this module existed; it does not yet give the
/// keep-open replacement its own exit-status/scrollback-preserving PTY swap (cross-platform plan
/// Phase 4's "server-driven PTY replacement" is not implemented - deferred to land alongside the
/// Phase 6/7 runtime-state work it naturally overlaps with).
fn pty_config(
    command: Option<&str>,
    keep_open: bool,
    shell: &[String],
    command_shell: &[String],
) -> TerminalPtyConfig {
    use crate::platform::command::ShellCommand;

    let shell = ShellCommand::from_argv(shell).unwrap_or_else(|| ShellCommand::new("/bin/sh"));
    if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
        let runner = ShellCommand::from_argv(command_shell)
            .unwrap_or_else(|| ShellCommand::new("/bin/sh").arg("-c"));
        let command = if keep_open {
            let shell_argv = shell.as_argv().join(" ");
            format!("{command}; exec {shell_argv}")
        } else {
            command.to_string()
        };
        let mut config = TerminalPtyConfig::new(runner.program).term("xterm-256color");
        for arg in runner.args {
            config = config.arg(arg);
        }
        config.arg(command)
    } else {
        let mut config = TerminalPtyConfig::new(shell.program).term("xterm-256color");
        for arg in shell.args {
            config = config.arg(arg);
        }
        config
    }
}

pub fn session_socket_path(name: &str) -> io::Result<PathBuf> {
    if !crate::session::discovery::valid_attach_target(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    Ok(control::runtime_dir()?.join(format!("session-{name}.sock")))
}

pub fn bind_session_socket(name: &str) -> io::Result<(UnixListener, PathBuf)> {
    let path = session_socket_path(name)?;
    bind_unix_socket(&path)?;
    let listener = UnixListener::bind(&path)?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok((listener, path))
}

fn bind_unix_socket(path: &Path) -> io::Result<()> {
    if path.exists() && UnixStream::connect(path).is_err() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn run_named_session(name: &str) -> io::Result<()> {
    if !crate::session::discovery::valid_attach_target(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    let (listener, path) = bind_session_socket(name)?;
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
            shell,
            command_shell,
            ..ServerSettings::default()
        },
    );
    server.socket_path = Some(path);
    if server.settings.resurrect
        && let Err(err) = server.restore()
    {
        eprintln!("hyprmux: could not restore session {name:?}: {err}");
    }
    let result = server.run_listener(listener);
    // A rename moves the socket file, so unlink the current path rather than the original one.
    if let Some(path) = &server.socket_path {
        let _ = fs::remove_file(path);
    }
    result
}

pub fn delete_snapshot(name: &str) -> io::Result<()> {
    resurrect::delete_snapshot_for(name)
}

#[cfg(test)]
mod tests;
