use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::{ExitQueue, GraphicsMediaPolicy};

use super::{
    Pane, PaneId, PendingPaneSpawn, PendingSessionAttach, SharedSessionState, WORKSPACE_COUNT,
    Workspace, is_ephemeral_session_name,
};

/// Identity of an attachment. Equal to the `runtime_epoch` the attachment was committed with, which
/// is also the epoch its server frames carry, so a background attachment's output routes to it while
/// the current attachment keeps [`super::State::runtime_epoch`].
pub type AttachmentId = u64;

/// What leaving a session behind should do with it. Produced by [`Attachment::disposition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDisposition {
    /// Leave it running. Named sessions, sessions shared with another client, and anything this
    /// client does not solely own all land here — none of them are ours to close.
    Keep,
    /// Close it without asking. A temporary session the client created for the user and that they
    /// never worked in holds nothing, and nothing can reattach to it by name.
    Discard,
    /// Ours and temporary, but the user worked in it and something is still running. Closing it
    /// would lose real work, and naming it is the only way to keep it, so the user decides.
    AskBeforeClosing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    AuthRequired,
    Unreachable,
    Incompatible,
}

/// The client's connection to one session together with that session's window-manager state: the
/// session client handle and identity (name, remote host), the shared-layout lease, the
/// spawn/replay buffers, and the workspaces/focus/pane-id space scoped to that session.
///
/// `State` keeps one current attachment plus a keyed set of background attachments. Each attachment
/// carries its own `next_pane_id`/`next_pty_generation` so two live sessions can never mint
/// colliding `(pane_id, generation)` routing keys.
pub struct Attachment {
    /// This attachment's identity, matching its server frames' epoch. `0` until committed (the
    /// current attachment's live epoch is [`super::State::runtime_epoch`]); stamped when the
    /// attachment is parked into [`super::State::background`].
    pub epoch: AttachmentId,
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
    pub connection: ConnectionState,
    /// Attach mode to restore after a dropped link. This remains available after shared-session
    /// bookkeeping is cleared during disconnect.
    pub reconnect_read_only: bool,
    pub session_attached: bool,
    pub pending_session_attach: Option<PendingSessionAttach>,
    /// Pane spawns requested while no session client was connected yet (e.g. a scratchpad toggle
    /// during the initial attach or a reconnect window). Flushed to the server once
    /// [`Msg::SessionAttached`](crate::Msg::SessionAttached) installs the client.
    pub pending_spawns: Vec<PendingPaneSpawn>,
    /// Replay commands (see [`PaneIdentity::replay`]) waiting for their pane's `SpawnResult`,
    /// keyed by `(pane_id, generation)`. The spawn goes out with `launch: None` so the server
    /// launches the interactive shell; once the spawn succeeds the command is sent as pane input
    /// (with a trailing carriage return), where it sits as type-ahead until the shell's first
    /// prompt reads and runs it. Only the client that requested the spawn holds the entry, so a
    /// multi-client session injects it exactly once.
    pub pending_replay_inputs: HashMap<(PaneId, u64), String>,
    /// Latest authoritative layout received while this attachment was in the background. Applied
    /// when it becomes current so background protocol traffic never mutates the visible session.
    pub pending_background_layout: Option<(u64, crate::shared_layout::SharedLayout)>,
    /// Structural closes deferred while this attachment is parked. Applied after it returns to the
    /// foreground and regains layout control.
    pub pending_background_closes: Vec<(PaneId, u64)>,
    /// Latest pending terminal size per `(local, pane)`. This outlives a transport reconnect so an
    /// unchanged viewport is still delivered after the new client is installed.
    pub pending_resizes: HashMap<(bool, PaneId), (u16, u16)>,
    /// Whether a batched `Msg::FlushPaneResizes` timer is in flight.
    pub resize_flush_scheduled: bool,
    /// Recently removed authoritative panes retained only so a same-generation layout correction
    /// can restore the client-side terminal screen while Canvas owns the visual exit subtree.
    pub retired_panes: ExitQueue<(PaneId, u64), Pane>,
    /// Shared-session bookkeeping for the attached named/ephemeral session: the layout lease,
    /// revision counters, canonical canvas, and reconciliation buffers. `None` until the session
    /// handshake completes (and while purely local, pre-attach).
    pub shared: Option<SharedSessionState>,
    /// Whether this session was created for the user rather than by the user: the startup
    /// ephemeral, or the one a fallback path had to invent. Only these are discarded on switch-away
    /// when untouched — a session the user explicitly asked for is theirs to keep.
    pub auto_created: bool,
    /// Whether the user has actually worked in this session: typed into a pane, pasted, sent input,
    /// or spawned a pane by hand. An ephemeral that never got this far holds nothing worth
    /// preserving, so it is neither retained across a switch nor worth warning about on quit.
    pub engaged: bool,
    /// When this attachment was last parked, as a monotonic counter; `0` while it has never been.
    /// Attachment ids are stable identities rather than a use order — parking one reuses its id —
    /// so recency needs its own stamp. Read when something has to land the user on the session they
    /// were most recently on, such as disconnecting the host the current one lives on.
    pub parked_seq: u64,
}

impl Attachment {
    /// A fresh, unattached attachment (no session client, purely local, nothing pending) with an
    /// empty set of workspaces. Callers that need an initial pane populate `workspaces` after.
    pub fn new() -> Self {
        Self {
            epoch: 0,
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
            connection: ConnectionState::Disconnected,
            reconnect_read_only: false,
            session_attached: false,
            pending_session_attach: None,
            pending_spawns: Vec::new(),
            pending_replay_inputs: HashMap::new(),
            pending_background_layout: None,
            pending_background_closes: Vec::new(),
            pending_resizes: HashMap::new(),
            resize_flush_scheduled: false,
            retired_panes: ExitQueue::with_exit_timeout(crate::anim::retained_pane_timeout(
                crate::anim::WindowAnimationConfig::default(),
            )),
            shared: None,
            auto_created: false,
            engaged: false,
            parked_seq: 0,
        }
    }

    pub fn retire_pane(&mut self, pane: Pane, timeout: Duration) {
        let key = (pane.id, pane.pty_generation);
        let keys = self
            .retired_panes
            .iter()
            .map(|(key, _, _)| *key)
            .collect::<Vec<_>>();
        let mut replacement = ExitQueue::with_exit_timeout(timeout);
        for key in keys {
            if let Some(transfer) = self.retired_panes.transfer_out(&key) {
                replacement.adopt(transfer);
            }
        }
        self.retired_panes = replacement;
        self.retired_panes.sync([(key, pane)]);
        self.retired_panes
            .sync(std::iter::empty::<((PaneId, u64), Pane)>());
    }

    pub fn take_retired_pane(&mut self, id: PaneId, generation: u64) -> Option<Pane> {
        self.retired_panes.expire();
        self.retired_panes
            .transfer_out(&(id, generation))
            .map(|transfer| transfer.into_parts().1)
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

    /// Find a workspace pane by id within this attachment. Unlike [`crate::pane_lifecycle::find_pane_mut`]
    /// it does not consult the scratchpad/popup, which are client-local overlays on `State` that only
    /// ever belong to the current attachment - so this is the finder used to apply a *background*
    /// attachment's server output to its own screens.
    pub fn find_pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.panes.iter_mut())
            .find(|pane| pane.id == id)
    }

    /// Which out-of-band graphics media the panes of this attachment may read.
    ///
    /// A pane hands over a picture by naming a file only when the process that wrote it and the
    /// client that draws it share a filesystem. Over `--remote` they do not, so nothing named is
    /// opened - a path arriving from the far side would resolve against this machine's files.
    /// Locally the re-readable medium is allowed and the consuming ones are not, because several
    /// clients can be attached to one session and the first reader would take the frame from the
    /// others.
    pub fn image_media_policy(&self) -> GraphicsMediaPolicy {
        if self.remote_target.is_some() {
            GraphicsMediaPolicy::NONE
        } else {
            GraphicsMediaPolicy::SHARED
        }
    }

    /// Apply server output to a *background* attachment: update the pane's screen (or buffer it as
    /// orphan output when the pane's layout commit has not arrived yet) and mark unseen activity.
    /// None of the current-attachment view side effects (bell notifications, focus, rendering) run,
    /// since a background attachment is never drawn - only its screen is kept live for an instant
    /// switch-back.
    pub fn apply_background_output(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        let policy = self.image_media_policy();
        if let Some(pane) = self.find_pane_mut(pane_id) {
            if pane.pty_generation == generation {
                pane.terminal.set_media_policy(policy);
                pane.terminal.process_server_output(bytes);
                pane.terminal.take_bell();
                pane.activity.last_activity = Some(std::time::Instant::now());
                pane.activity.has_unseen_output = true;
            }
        } else if let Some(shared) = self.shared.as_mut() {
            shared.buffer_orphan_output(pane_id, generation, bytes);
        }
    }

    /// Preserve the last rendered screens while dropping transport-specific state after a link
    /// ends. A background attachment can then be restored immediately and reconnect in place.
    pub fn mark_disconnected(&mut self) {
        self.connection = ConnectionState::Disconnected;
        self.session_attached = false;
        self.session_client = None;
        self.shared = None;
        self.prune_replay_inputs_to_pending_spawns();
        for pane in self
            .workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.panes.iter_mut())
        {
            pane.terminal.status =
                tui_lipan::prelude::ManagedTerminalStatus::Error("session disconnected".into());
        }
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

    /// What should happen to this session when the client stops showing it — switching away, or
    /// leaving altogether.
    ///
    /// This is the single answer to "is this session disposable", asked once so the switch path,
    /// the exit path, and the leave prompt cannot drift apart on it. Each of them reads the same
    /// verdict and only decides what to *do* about it.
    pub fn disposition(&self) -> SessionDisposition {
        // Anything we do not solely own is somebody else's to close: a named session, one another
        // client is sharing, one we do not lead, or a read-only attachment.
        if !self.solely_owns_temporary_server() {
            return SessionDisposition::Keep;
        }
        if self.auto_created && !self.engaged {
            // Created for the user, never used: closing it loses nothing, and keeping it is what
            // made "temporary" sessions accumulate.
            return SessionDisposition::Discard;
        }
        if self.any_pane_live() {
            SessionDisposition::AskBeforeClosing
        } else {
            // Worked in once, but nothing is running any more. There is no work to lose and no
            // name to come back by, so it needs no ceremony — but nothing is gained by closing it
            // either, and its server self-reaps once no client is attached.
            SessionDisposition::Keep
        }
    }

    /// Whether this client alone owns this session's disposable server: a temporary session it
    /// leads, that nobody else is attached to, and that it may write to. Only then is closing the
    /// server this client's call at all — the question of *whether* it should is
    /// [`Self::disposition`].
    pub fn solely_owns_temporary_server(&self) -> bool {
        self.is_ephemeral_session()
            && self.is_controller()
            && self.attached_client_count() == 1
            && self.shared.as_ref().is_none_or(|shared| !shared.read_only)
    }

    pub fn any_pane_live(&self) -> bool {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .any(|pane| !pane.closing && pane.terminal.is_running())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary session this client solely owns, in whatever state the test needs.
    fn temporary(auto_created: bool, engaged: bool) -> Attachment {
        let mut attachment = Attachment::new();
        attachment.session_name = Some("eph-1".to_string());
        attachment.auto_created = auto_created;
        attachment.engaged = engaged;
        attachment
    }

    /// The three call sites that used to ask their own version of "is this disposable" now read one
    /// verdict, so these cases pin the whole model rather than one path through it.
    #[test]
    fn a_named_session_is_never_ours_to_close() {
        let mut named = temporary(true, false);
        named.session_name = Some("dev".to_string());
        assert_eq!(named.disposition(), SessionDisposition::Keep);
    }

    #[test]
    fn an_untouched_startup_session_is_discarded_without_asking() {
        assert_eq!(
            temporary(true, false).disposition(),
            SessionDisposition::Discard
        );
    }

    /// Worked in, but nothing is running: there is no work to lose, so it needs no prompt — and
    /// nothing is gained by closing it either.
    #[test]
    fn a_used_session_with_nothing_running_is_left_alone() {
        assert_eq!(
            temporary(true, true).disposition(),
            SessionDisposition::Keep
        );
    }

    /// A session the user explicitly asked for is theirs even before they touch it: it was never
    /// the client's to invent, so it is never the client's to quietly discard.
    #[test]
    fn a_user_created_temporary_session_is_not_discarded_untouched() {
        assert_ne!(
            temporary(false, false).disposition(),
            SessionDisposition::Discard
        );
    }

    /// Sharing changes the answer regardless of everything else: another client is attached, so
    /// closing the server would take the session out from under them.
    #[test]
    fn a_shared_session_is_never_closed_by_this_client() {
        let mut shared_session = temporary(true, false);
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.clients = vec![
            crate::session::protocol::ClientInfo {
                id: 1,
                label: "me".to_string(),
                read_only: false,
                requesting_control: false,
                parked: false,
            },
            crate::session::protocol::ClientInfo {
                id: 2,
                label: "them".to_string(),
                read_only: false,
                requesting_control: false,
                parked: false,
            },
        ];
        shared_session.shared = Some(shared);
        assert_eq!(shared_session.disposition(), SessionDisposition::Keep);
    }
}
