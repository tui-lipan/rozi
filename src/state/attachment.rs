use std::collections::HashMap;
use std::path::PathBuf;

use super::{
    PaneId, PendingPaneSpawn, PendingSessionAttach, SharedSessionState, WORKSPACE_COUNT, Workspace,
    is_ephemeral_session_name,
};

/// The client's connection to one session together with that session's window-manager state: the
/// session client handle and identity (name, remote host), the shared-layout lease, the
/// spawn/replay buffers, and the workspaces/focus/pane-id space scoped to that session.
///
/// Today `State` holds exactly one `Attachment` (see [`super::State::current`]); a session switch
/// still replaces the whole `State`. This type exists so that per-session state lives in one place
/// ahead of the multi-attachment work, where `State` will hold a collection of `Attachment`s with
/// one marked current and background attachments keep their own client, screens, and buffers. Each
/// attachment carries its own `next_pane_id`/`next_pty_generation` so two live sessions can never
/// mint colliding `(pane_id, generation)` routing keys.
pub struct Attachment {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub focused_pane: Option<PaneId>,
    pub next_pane_id: PaneId,
    pub next_pty_generation: u64,
    pub session_client: Option<crate::session::client::SessionClient>,
    pub session_name: Option<String>,
    /// When attached via `--remote`, the remote host alias/URL; `None` for local sessions.
    /// Local-filesystem features (file tree, profile path capture) must treat server cwds as
    /// remote when this is set, even if `cwd_host` is `None` (server-relative).
    pub remote_host: Option<String>,
    /// The resolved `--remote` target for the current session, carried alongside `remote_host` so a
    /// dropped link reconnects to the *same* remote host instead of re-parsing (or, worse, falling
    /// back to a same-named local session). `None` for local sessions.
    pub remote_target: Option<crate::session::remote::RemoteTarget>,
    pub created_from_profile: Option<String>,
    pub deferred_profile_seed: Option<(String, PathBuf)>,
    pub pending_profile_loaded: Option<(String, PathBuf, String)>,
    pub session_attached: bool,
    pub pending_session_attach: Option<PendingSessionAttach>,
    /// Pane spawns requested while no session client was connected yet (e.g. a scratchpad toggle
    /// during the initial attach or a reconnect window). Flushed to the server once
    /// [`Msg::SessionAttached`](crate::Msg::SessionAttached) installs the client.
    pub pending_spawns: Vec<PendingPaneSpawn>,
    /// Replay commands (see [`PaneIdentity::replay`]) waiting for their pane's `SpawnResult`,
    /// keyed by `(pane_id, generation)`. The spawn goes out with `command: None` so the server
    /// launches the interactive shell; once the spawn succeeds the command is sent as pane input
    /// (with a trailing carriage return), where it sits as type-ahead until the shell's first
    /// prompt reads and runs it. Only the client that requested the spawn holds the entry, so a
    /// multi-client session injects it exactly once.
    pub pending_replay_inputs: HashMap<(PaneId, u64), String>,
    /// Shared-session bookkeeping for the attached named/ephemeral session: the layout lease,
    /// revision counters, canonical canvas, and reconciliation buffers. `None` until the session
    /// handshake completes (and while purely local, pre-attach).
    pub shared: Option<SharedSessionState>,
}

impl Attachment {
    /// A fresh, unattached attachment (no session client, purely local, nothing pending) with an
    /// empty set of workspaces. Callers that need an initial pane populate `workspaces` after.
    pub fn new() -> Self {
        Self {
            workspaces: (0..WORKSPACE_COUNT).map(Workspace::new).collect(),
            active_workspace: 0,
            focused_pane: None,
            next_pane_id: 1,
            next_pty_generation: 1,
            session_client: None,
            session_name: None,
            remote_host: None,
            remote_target: None,
            created_from_profile: None,
            deferred_profile_seed: None,
            pending_profile_loaded: None,
            session_attached: false,
            pending_session_attach: None,
            pending_spawns: Vec::new(),
            pending_replay_inputs: HashMap::new(),
            shared: None,
        }
    }

    /// Drop queued replay inputs whose spawn can no longer complete. Called when the session
    /// connection is torn down (disconnect, attach-elsewhere reseed): only a spawn still waiting
    /// in [`Self::pending_spawns`] will ever produce a `SpawnResult` for its key, and a stale
    /// entry must not linger - `reset_state_for_shared_seed` restarts the generation counter, so
    /// a later attachment could mint the same `(pane_id, generation)` key and receive a command
    /// meant for a pane of the previous session.
    pub fn prune_replay_inputs_to_pending_spawns(&mut self) {
        if self.pending_replay_inputs.is_empty() {
            return;
        }
        let queued: std::collections::HashSet<(PaneId, u64)> = self
            .pending_spawns
            .iter()
            .map(|spawn| (spawn.pane_id, spawn.generation))
            .collect();
        self.pending_replay_inputs
            .retain(|key, _| queued.contains(key));
    }

    /// The active workspace. A single-borrow accessor so callers avoid the
    /// `workspaces[active_workspace]` double index, which would borrow the attachment twice.
    pub fn active_workspace_ref(&self) -> &Workspace {
        &self.workspaces[self.active_workspace]
    }

    /// Mutable [active workspace](Self::active_workspace_ref).
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace]
    }

    /// Whether the currently attached session is an auto-managed ephemeral session.
    pub fn is_ephemeral_session(&self) -> bool {
        self.session_name
            .as_deref()
            .is_some_and(is_ephemeral_session_name)
    }

    /// Whether this client may mutate the shared layout: always true when purely local (no shared
    /// session), otherwise true only while it holds the layout-control lease.
    pub fn is_controller(&self) -> bool {
        self.shared
            .as_ref()
            .is_none_or(SharedSessionState::is_controller)
    }

    /// The number of clients attached to the shared session (1 when local/unshared).
    pub fn attached_client_count(&self) -> u32 {
        self.shared
            .as_ref()
            .map_or(1, |shared| shared.clients.len().max(1) as u32)
    }

    pub fn pane_input_block_reason(&self) -> Option<&'static str> {
        let shared = self.shared.as_ref()?;
        if shared.read_only {
            Some("Attached read-only")
        } else if shared.input_locked && !shared.is_controller() {
            Some("Input locked to the controller")
        } else {
            None
        }
    }

    /// The canonical pane canvas the controller publishes, if this client is a follower that
    /// should letterbox to it. `None` for the controller or a local session (renders to its own
    /// viewport).
    pub fn follower_canonical_canvas(&self) -> Option<(u16, u16)> {
        let shared = self.shared.as_ref()?;
        if shared.is_controller() {
            return None;
        }
        shared.canonical_canvas
    }
}

impl Default for Attachment {
    fn default() -> Self {
        Self::new()
    }
}
