use std::collections::HashMap;
use std::path::PathBuf;

use tui_lipan::prelude::TerminalColorPalette;

use crate::config::UserCommandAction;

use super::PaneId;

/// A PTY-creating action deferred until an ephemeral session finishes attaching.
///
/// The launcher (and any other no-client resting state) has nowhere to run a pane. Callers stash
/// one of these, start [`crate::ops::session::attach_startup_ephemeral`], and
/// [`crate::ops::session::run_pending_session_action`] replays it from `SessionAttached`.
#[derive(Clone, Debug, PartialEq)]
pub enum PendingSessionAction {
    OpenConfigFile,
    ToggleScratchpad,
    UserCommand {
        action: UserCommandAction,
        env: Vec<(String, String)>,
    },
    NewPane {
        source: Option<PaneId>,
        command: Option<String>,
        cwd: Option<String>,
        title: Option<String>,
        keep_open: bool,
    },
    Popup {
        command: String,
        cwd: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        keep_open: bool,
    },
}

pub struct PendingSessionAttach {
    pub epoch: u64,
    pub name: String,
    pub client: Option<crate::session::client::SessionClient>,
    /// Whether a failed connect should autostart a `--server` process. Ephemeral sessions
    /// autostart; a dead named session surfaces as an error instead of a silent resurrection.
    pub autostart: bool,
    pub read_only: bool,
    /// This attach repairs an existing retained attachment rather than seeding a replacement.
    pub reconnect: bool,
    /// Remote host label when attaching via `--remote`.
    pub remote_host: Option<String>,
    pub intent: AttachIntent,
    pub left: Option<LeftSession>,
    /// The id of a session parked in the background to start this attach, restored if the attach
    /// fails. Without this, a failed remote connect would fall back to a fresh local ephemeral that
    /// re-attaches to this process's own `eph-<pid>` server — the one the parked client still
    /// controls — and join as a follower of itself. `None` when nothing was parked (a reconnect, or
    /// an attach that released rather than parked the previous session).
    pub parked_epoch: Option<super::AttachmentId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachIntent {
    Plain,
    ProfileSeed { profile: String, path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftSession {
    pub name: String,
    pub was_ephemeral_shutdown: bool,
}

/// Per-run maximum orphan bytes buffered per pane before oldest data is dropped (see
/// [`SharedSessionState::orphan_output`]).
pub const ORPHAN_OUTPUT_CAP: usize = 256 * 1024;

/// Client-side state for an attached shared session: the layout-control lease, revision
/// bookkeeping for optimistic commits, the controller's canonical canvas, and the buffers the
/// reconciler needs. Present whenever [`super::State::session_attached`] is true under protocol v6.
pub struct SharedSessionState {
    /// This client's server-assigned id.
    pub client_id: crate::shared_layout::ClientId,
    /// The last layout revision this client has applied.
    pub layout_rev: u64,
    /// Optimistic base for the next commit: bumped locally on each commit so pipelined commits
    /// carry increasing base revs without waiting for each echo.
    pub assumed_rev: u64,
    /// The current layout controller, or `None` between promotions.
    pub controller: Option<crate::shared_layout::ClientId>,
    /// How many clients are attached to the session (including this one).
    pub clients: Vec<crate::session::protocol::ClientInfo>,
    pub input_locked: bool,
    /// Whether writable followers may immediately take the layout-control lease.
    pub allow_takeover: bool,
    pub read_only: bool,
    /// The controller's canonical pane canvas in cells (excluding the workbar). Followers letterbox
    /// to this; `None` until the first layout with a canvas is seen.
    pub canonical_canvas: Option<(u16, u16)>,
    /// The last layout this client committed/applied, used as the dirty detector for the commit
    /// chokepoint (cheaper than re-serializing).
    pub last_committed_layout: Option<crate::shared_layout::SharedLayout>,
    /// Pane output that arrived before the pane's `LayoutCommitted` created it locally, keyed by
    /// `(pane_id, generation)`; drained into the pane once the reconciler adds it. Capped per pane.
    pub orphan_output: HashMap<(PaneId, u64), Vec<u8>>,
    /// Latest pending resize per pane while the controller debounces resize storms.
    pub pending_resizes: HashMap<PaneId, (u16, u16)>,
    /// Whether a trailing-edge `Msg::FlushPaneResizes` is already in flight, so a burst of resizes
    /// schedules only one flush timer.
    pub resize_flush_scheduled: bool,
    /// Whether a trailing-edge `Msg::FlushLayoutCommit` is already in flight.
    pub layout_commit_scheduled: bool,
}

impl SharedSessionState {
    pub fn new(client_id: crate::shared_layout::ClientId) -> Self {
        Self {
            client_id,
            layout_rev: 0,
            assumed_rev: 0,
            controller: None,
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            read_only: false,
            canonical_canvas: None,
            last_committed_layout: None,
            orphan_output: HashMap::new(),
            pending_resizes: HashMap::new(),
            resize_flush_scheduled: false,
            layout_commit_scheduled: false,
        }
    }

    /// True when this client currently holds the layout-control lease.
    pub fn is_controller(&self) -> bool {
        self.controller == Some(self.client_id)
    }

    /// How many attached clients are actually using the session, ignoring the ones parked in the
    /// background. This is the count that decides whether the session is shared in any sense the
    /// user should be told about.
    pub fn active_clients(&self) -> usize {
        self.clients.iter().filter(|client| !client.parked).count()
    }

    /// Whether any other client has an outstanding request for the control lease (badge fodder for
    /// the controller's workbar and the session-clients view).
    pub fn has_pending_control_requests(&self) -> bool {
        self.clients
            .iter()
            .any(|client| client.requesting_control && Some(client.id) != self.controller)
    }

    /// Buffer pane output that arrived before its pane exists locally, enforcing the per-pane cap
    /// by dropping the oldest bytes.
    pub fn buffer_orphan_output(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        let buffer = self.orphan_output.entry((pane_id, generation)).or_default();
        buffer.extend_from_slice(bytes);
        if buffer.len() > ORPHAN_OUTPUT_CAP {
            let overflow = buffer.len() - ORPHAN_OUTPUT_CAP;
            buffer.drain(..overflow);
        }
    }
}

/// A pane spawn deferred until a session client is available (see [`super::State::pending_spawns`]).
#[derive(Clone, Debug)]
pub struct PendingPaneSpawn {
    pub pane_id: PaneId,
    pub generation: u64,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub keep_open: bool,
    pub env: Vec<(String, String)>,
    pub title: Option<String>,
    pub palette: TerminalColorPalette,
    pub shell: Vec<String>,
    pub command_shell: Vec<String>,
}

/// The prefix that marks an auto-named ephemeral session. Ephemeral servers shut down on a clean
/// quit but survive a UI crash for reattach; user-typed names may not use this prefix.
pub const EPHEMERAL_SESSION_PREFIX: &str = "eph-";

/// Whether `name` denotes an auto-managed ephemeral session.
pub fn is_ephemeral_session_name(name: &str) -> bool {
    name.starts_with(EPHEMERAL_SESSION_PREFIX)
}

/// The ephemeral session name for this UI process (`eph-<pid>`).
pub fn ephemeral_session_name() -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}", std::process::id())
}

/// A fresh ephemeral name that will not collide with a still-running ephemeral server left behind
/// by a prior detach (`eph-<pid>-<salt>`).
pub fn fresh_ephemeral_session_name(salt: u64) -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}-{salt}", std::process::id())
}

/// Ephemeral name qualified by a stable per-client identifier (`eph-<host>-<pid>`), for `--remote`.
///
/// A bare `eph-<pid>` names a session that lives on the *remote* host, where two clients on
/// different machines can plausibly share a pid and would silently land on the same session. The
/// hostname disambiguates them; it stays constant for the process lifetime, so a dropped link
/// reconnects to the same ephemeral name.
pub fn remote_ephemeral_session_name() -> String {
    let host = crate::platform::user::hostname()
        .map(|host| {
            // Session names permit only `[A-Za-z0-9_-]`; keep the alphanumerics and cap the length
            // so an odd or very long hostname cannot produce an invalid or unwieldy name.
            host.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(24)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "host".to_string());
    format!("{EPHEMERAL_SESSION_PREFIX}{host}-{}", std::process::id())
}
