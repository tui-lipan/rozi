use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

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
    OpenFile(PathBuf),
    UserCommand {
        action: UserCommandAction,
        env: Vec<(String, String)>,
    },
    NewPane {
        source: Option<PaneId>,
        launch: Option<crate::pane::launch::PaneLaunch>,
        cwd: Option<String>,
        title: Option<String>,
        keep_open: bool,
        focus: bool,
        /// Zero-based, already validated by the control command that asked for it.
        workspace: Option<usize>,
        /// Already validated by the control command that asked for it.
        size: Option<crate::control::PaneSize>,
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

/// The pane the global Agents view aimed at, held while the session that owns it attaches.
///
/// Opening a remote agent is one gesture with two halves that cannot happen in one turn: the
/// attach is a round trip over ssh, and the pane does not exist locally until its snapshot lands.
/// The destination is recorded by session identity rather than by attachment epoch so that a
/// switch to an already-parked session — which never goes through an attach at all — resolves it
/// on exactly the same rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAgentJump {
    pub target: Option<crate::session::remote::RemoteTarget>,
    pub session: String,
    pub pane: PaneId,
    pub row: Option<crate::session::protocol::RowKey>,
}

/// One attempt to bring a session onto this client: everything that decides how it is reached and
/// what happens when it lands.
///
/// A request is built once, where the attach is asked for, and travels with the attempt in
/// [`PendingSessionAttach`] until the session lands, the attempt fails, or another attach replaces
/// it. Nothing about an attempt is kept anywhere else, so ending the attempt forgets all of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AttachRequest {
    pub name: String,
    /// The host the session lives on, `None` for this machine.
    pub remote: Option<crate::session::remote::RemoteTarget>,
    pub mode: AttachMode,
    pub access: AttachAccess,
    /// What an empty server is seeded with, and what origin it records.
    pub intent: AttachIntent,
    pub prompting: AttachPrompting,
}

impl AttachRequest {
    /// A writable, unseeded attach to `name` on this machine.
    pub fn new(name: impl Into<String>, mode: AttachMode) -> Self {
        Self {
            name: name.into(),
            remote: None,
            mode,
            access: AttachAccess::WRITABLE,
            intent: AttachIntent::Plain,
            prompting: AttachPrompting::Allowed,
        }
    }

    pub fn on_host(mut self, remote: Option<crate::session::remote::RemoteTarget>) -> Self {
        self.remote = remote;
        self
    }

    pub fn with_access(mut self, access: AttachAccess) -> Self {
        self.access = access;
        self
    }

    pub fn with_intent(mut self, intent: AttachIntent) -> Self {
        self.intent = intent;
        self
    }

    pub fn with_prompting(mut self, prompting: AttachPrompting) -> Self {
        self.prompting = prompting;
        self
    }

    /// Whether this attempt restores the retained attachment rather than bringing a new one.
    pub fn is_reconnect(&self) -> bool {
        matches!(self.mode, AttachMode::Reconnect | AttachMode::Recreate)
    }

    /// The host label the workbar and toasts show for this attach.
    pub fn remote_label(&self) -> Option<String> {
        self.remote
            .as_ref()
            .map(crate::session::remote::RemoteTarget::display_label)
    }

    /// How this attach joins a session another client controls, with `configured` as the
    /// `[session] on_attach` in effect when it lands. `None` when the question does not arise: a
    /// reconnect returns to the role it had, and a viewer never takes part in layout control.
    pub fn occupied_policy(
        &self,
        configured: crate::config::SessionOnAttach,
    ) -> Option<crate::config::SessionOnAttach> {
        if self.is_reconnect() {
            return None;
        }
        match self.access {
            AttachAccess::Writable { on_attach } => Some(on_attach.unwrap_or(configured)),
            AttachAccess::ReadOnly => None,
        }
    }
}

/// How an attach finds or starts its session server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachMode {
    /// Join a running server; a missing one fails the attach.
    Join,
    /// Join the server, starting it when none is running.
    JoinOrStart,
    /// Start the server; one already running fails the attach.
    Create,
    /// Restore the retained attachment's transport without replacing its server.
    Reconnect,
    /// Seed a replacement remote server from the retained attachment after the original is gone.
    Recreate,
}

/// What an attach may do in the session, and how a writable one joins a session another client
/// controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachAccess {
    /// Types and may hold layout control. `on_attach` overrides `[session] on_attach` for this
    /// attach alone.
    Writable {
        on_attach: Option<crate::config::SessionOnAttach>,
    },
    /// Watches without input or layout authority.
    ReadOnly,
}

impl AttachAccess {
    pub const WRITABLE: Self = Self::Writable { on_attach: None };

    pub fn is_read_only(self) -> bool {
        self == Self::ReadOnly
    }
}

impl Default for AttachAccess {
    fn default() -> Self {
        Self::WRITABLE
    }
}

/// Whether ssh may raise a password or host-key dialog for this attempt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachPrompting {
    /// Someone is waiting on this attempt and can answer.
    #[default]
    Allowed,
    /// Nobody asked for this attempt: one that would need a dialog fails instead.
    Forbidden,
}

impl AttachPrompting {
    pub fn remote_config(
        self,
        config: &crate::config::RemoteConfig,
    ) -> crate::config::RemoteConfig {
        match self {
            Self::Allowed => config.clone(),
            Self::Forbidden => config.unattended(),
        }
    }
}

/// An [`AttachRequest`] in flight on an attachment.
pub struct PendingSessionAttach {
    pub epoch: u64,
    pub request: AttachRequest,
    pub client: Option<crate::session::client::SessionClient>,
    pub left: Option<LeftSession>,
    /// The id of a session parked in the background to start this attach, restored if the attach
    /// fails. Without this, a failed remote connect would fall back to a fresh local ephemeral that
    /// re-attaches to this process's own `eph-<pid>` server — the one the parked client still
    /// controls — and join as a follower of itself. `None` when nothing was parked (a reconnect, or
    /// an attach that released rather than parked the previous session).
    pub parked_epoch: Option<super::AttachmentId>,
}

impl PendingSessionAttach {
    pub fn new(epoch: u64, request: AttachRequest) -> Self {
        Self {
            epoch,
            request,
            client: None,
            left: None,
            parked_epoch: None,
        }
    }

    /// Record what this attach moved away from: the session it released (`left`) or parked.
    pub fn leaving(
        mut self,
        left: Option<LeftSession>,
        parked_epoch: Option<super::AttachmentId>,
    ) -> Self {
        self.left = left;
        self.parked_epoch = parked_epoch;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachIntent {
    Plain,
    ProfileSeed {
        profile: String,
        path: PathBuf,
    },
    /// A new session for the checkout at `path`, seeded from `profile` (name, file) when one was
    /// applied.
    WorktreeSeed {
        path: String,
        profile: Option<(String, PathBuf)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftSession {
    pub name: String,
    pub was_ephemeral_shutdown: bool,
}

/// Maximum orphan bytes retained for one `(pane, generation)` key.
pub const ORPHAN_OUTPUT_CAP: usize = 256 * 1024;
/// Maximum orphan bytes retained across all panes and generations.
pub const ORPHAN_OUTPUT_GLOBAL_CAP: usize = 4 * 1024 * 1024;
/// Maximum acquired media bytes retained for one orphan `(pane, generation)` key.
pub const ORPHAN_OUTPUT_MEDIA_CAP: usize = 64 * 1024 * 1024;
/// Maximum acquired media bytes retained across all orphan panes and generations.
pub const ORPHAN_OUTPUT_MEDIA_GLOBAL_CAP: usize = 128 * 1024 * 1024;
/// Maximum acquired media files retained for one orphan `(pane, generation)` key.
pub const ORPHAN_OUTPUT_MEDIA_FILE_CAP: usize = 64;
/// Maximum acquired media files retained across all orphan panes and generations.
pub const ORPHAN_OUTPUT_MEDIA_GLOBAL_FILE_CAP: usize = 128;
/// Maximum distinct `(pane, generation)` buffers retained by one attachment.
///
/// Four thousand pending identities is already far beyond a plausible layout race, while keeping
/// tiny frames from turning the byte-bounded store into an effectively unbounded hash table.
pub const ORPHAN_OUTPUT_KEY_CAP: usize = 4 * 1024;

type OrphanOutputKey = (PaneId, u64);

fn orphan_media_bytes(media: &[tui_lipan::TerminalOutputMedia]) -> usize {
    media
        .iter()
        .fold(0usize, |acc, item| acc.saturating_add(item.file.byte_len()))
}

/// Bounded output received before an authoritative layout creates its pane locally.
pub struct OrphanOutputStore {
    buffers: HashMap<OrphanOutputKey, Vec<u8>>,
    media: HashMap<OrphanOutputKey, Vec<tui_lipan::TerminalOutputMedia>>,
    order: VecDeque<OrphanOutputKey>,
    retained: usize,
    high_water: usize,
    retained_media_bytes: usize,
    high_water_media_bytes: usize,
    retained_media_files: usize,
    high_water_media_files: usize,
    media_cap: usize,
    media_global_cap: usize,
    media_file_cap: usize,
    media_global_file_cap: usize,
}

impl Default for OrphanOutputStore {
    fn default() -> Self {
        Self::with_media_limits(
            ORPHAN_OUTPUT_MEDIA_CAP,
            ORPHAN_OUTPUT_MEDIA_GLOBAL_CAP,
            ORPHAN_OUTPUT_MEDIA_FILE_CAP,
            ORPHAN_OUTPUT_MEDIA_GLOBAL_FILE_CAP,
        )
    }
}

/// Cheap accounting snapshot for orphan output retained by an attachment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrphanOutputStats {
    pub retained: usize,
    pub high_water: usize,
    pub keys: usize,
}

impl OrphanOutputStore {
    pub fn with_media_limits(
        media_cap: usize,
        media_global_cap: usize,
        media_file_cap: usize,
        media_global_file_cap: usize,
    ) -> Self {
        Self {
            buffers: HashMap::new(),
            media: HashMap::new(),
            order: VecDeque::new(),
            retained: 0,
            high_water: 0,
            retained_media_bytes: 0,
            high_water_media_bytes: 0,
            retained_media_files: 0,
            high_water_media_files: 0,
            media_cap,
            media_global_cap,
            media_file_cap,
            media_global_file_cap,
        }
    }

    pub fn insert(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) -> bool {
        self.insert_media(pane_id, generation, bytes, Vec::new())
    }

    pub fn insert_media(
        &mut self,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
        media: Vec<tui_lipan::TerminalOutputMedia>,
    ) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let key = (pane_id, generation);
        let offset = self.buffers.get(&key).map_or(0, Vec::len);
        let existing_media = self.media.get(&key).map_or(&[][..], Vec::as_slice);
        let existing_media_bytes = orphan_media_bytes(existing_media);
        let existing_media_files = existing_media.len();
        let added_media_bytes = orphan_media_bytes(&media);
        let added_media_files = media.len();
        if offset.saturating_add(bytes.len()) > ORPHAN_OUTPUT_CAP
            || self.retained.saturating_add(bytes.len()) > ORPHAN_OUTPUT_GLOBAL_CAP
            || existing_media_bytes.saturating_add(added_media_bytes) > self.media_cap
            || self.retained_media_bytes.saturating_add(added_media_bytes) > self.media_global_cap
            || existing_media_files.saturating_add(added_media_files) > self.media_file_cap
            || self.retained_media_files.saturating_add(added_media_files)
                > self.media_global_file_cap
            || (!self.buffers.contains_key(&key) && self.buffers.len() >= ORPHAN_OUTPUT_KEY_CAP)
        {
            // A byte tail may start inside an escape or refer to pixels already discarded.
            // The caller must end the connection and obtain a complete replay.
            return false;
        }
        if !self.buffers.contains_key(&key) {
            self.order.push_back(key);
        }
        self.buffers
            .entry(key)
            .or_default()
            .extend_from_slice(bytes);
        if !media.is_empty() {
            self.media
                .entry(key)
                .or_default()
                .extend(media.into_iter().map(|mut m| {
                    m.command_range.start += offset;
                    m.command_range.end += offset;
                    m
                }));
            self.retained_media_bytes += added_media_bytes;
            self.high_water_media_bytes =
                self.high_water_media_bytes.max(self.retained_media_bytes);
            self.retained_media_files += added_media_files;
            self.high_water_media_files =
                self.high_water_media_files.max(self.retained_media_files);
        }
        self.retained += bytes.len();
        self.high_water = self.high_water.max(self.retained);
        true
    }

    fn remove_media_for_key(
        &mut self,
        key: &OrphanOutputKey,
    ) -> Vec<tui_lipan::TerminalOutputMedia> {
        let Some(removed) = self.media.remove(key) else {
            return Vec::new();
        };
        self.retained_media_bytes -= orphan_media_bytes(&removed);
        self.retained_media_files -= removed.len();
        removed
    }

    pub fn take_owned_for_generation(
        &mut self,
        pane_id: PaneId,
        generation: u64,
    ) -> Option<(Vec<u8>, Vec<tui_lipan::TerminalOutputMedia>)> {
        let media = self.remove_media_for_key(&(pane_id, generation));
        self.take_for_generation(pane_id, generation)
            .map(|bytes| (bytes, media))
    }

    pub fn take(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        let key = (pane_id, generation);
        let bytes = self.buffers.remove(&key)?;
        let _ = self.remove_media_for_key(&key);
        self.retained -= bytes.len();
        self.order.retain(|candidate| *candidate != key);
        debug_assert_eq!(self.order.len(), self.buffers.len());
        Some(bytes)
    }

    /// Drop stale generations for this pane, then return only the generation in the layout.
    pub fn take_for_generation(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        let buffers = &mut self.buffers;
        let media = &mut self.media;
        let retained = &mut self.retained;
        let retained_media_bytes = &mut self.retained_media_bytes;
        let retained_media_files = &mut self.retained_media_files;
        self.order.retain(|key| {
            if key.0 == pane_id && key.1 < generation {
                let removed = buffers.remove(key).expect("orphan order contains live key");
                *retained -= removed.len();
                if let Some(removed_media) = media.remove(key) {
                    *retained_media_bytes -= orphan_media_bytes(&removed_media);
                    *retained_media_files -= removed_media.len();
                }
                false
            } else {
                true
            }
        });
        self.take(pane_id, generation)
    }

    pub fn stats(&self) -> OrphanOutputStats {
        OrphanOutputStats {
            retained: self.retained,
            high_water: self.high_water,
            keys: self.buffers.len(),
        }
    }
}

/// One shared creation waiting for its authoritative publication or rejection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SharedSpawnInFlight {
    pub pane_id: PaneId,
    pub generation: u64,
    pub base_rev: u64,
    pub failed: bool,
}

/// Client-side state for an attached shared session: the layout-control lease, revision
/// bookkeeping, serialized shared creation, canonical canvas, and reconciliation buffers.
pub struct SharedSessionState {
    /// This client's server-assigned id.
    pub client_id: crate::layout::shared::ClientId,
    /// The last layout revision this client has applied.
    pub layout_rev: u64,
    /// Optimistic base for the next commit: bumped locally on each commit so pipelined commits
    /// carry increasing base revs without waiting for each echo.
    pub assumed_rev: u64,
    /// The current layout controller, or `None` between promotions.
    pub controller: Option<crate::layout::shared::ClientId>,
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
    pub last_committed_layout: Option<crate::layout::shared::SharedLayout>,
    /// Pane output that arrived before the pane's `LayoutCommitted` created it locally, keyed by
    /// `(pane_id, generation)`; drained into the pane once the reconciler adds it.
    orphan_output: OrphanOutputStore,
    pub(crate) orphan_replay_required: bool,
    /// The latest server geometry for a pane the layout has not created yet, keyed like
    /// `orphan_output` and applied before that output is parsed. One entry per pane: a newer
    /// generation replaces an older one.
    orphan_geometry: HashMap<PaneId, (u64, PaneGeometry)>,
    /// Whether a trailing-edge `Msg::FlushLayoutCommit` is already in flight.
    pub layout_commit_scheduled: bool,
    pub pane_scrollback: HashMap<(PaneId, u64), usize>,
    pub(crate) spawn_in_flight: Option<SharedSpawnInFlight>,
}

impl SharedSessionState {
    pub fn new(client_id: crate::layout::shared::ClientId) -> Self {
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
            orphan_output: OrphanOutputStore::default(),
            orphan_replay_required: false,
            orphan_geometry: HashMap::new(),
            layout_commit_scheduled: false,
            pane_scrollback: HashMap::new(),
            spawn_in_flight: None,
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
    /// without splitting commands. A false result requires reconnect and replay.
    pub fn buffer_orphan_output(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) -> bool {
        self.orphan_output.insert(pane_id, generation, bytes)
    }

    pub fn buffer_orphan_media(
        &mut self,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
        media: Vec<tui_lipan::TerminalOutputMedia>,
    ) -> bool {
        self.orphan_output
            .insert_media(pane_id, generation, bytes, media)
    }

    pub fn take_owned_orphan_output(
        &mut self,
        pane_id: PaneId,
        generation: u64,
    ) -> Option<(Vec<u8>, Vec<tui_lipan::TerminalOutputMedia>)> {
        self.orphan_output
            .take_owned_for_generation(pane_id, generation)
    }

    pub fn take_orphan_output(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        self.orphan_output.take_for_generation(pane_id, generation)
    }

    /// Forget output buffered for a pane the server is about to replay from scratch.
    pub fn discard_orphan_output(&mut self, pane_id: PaneId, generation: u64) {
        self.orphan_output.take_for_generation(pane_id, generation);
    }

    pub fn orphan_output_stats(&self) -> OrphanOutputStats {
        self.orphan_output.stats()
    }

    /// Hold the server geometry of a pane the layout has not created yet.
    pub fn hold_orphan_geometry(
        &mut self,
        pane_id: PaneId,
        generation: u64,
        geometry: PaneGeometry,
    ) {
        if self
            .orphan_geometry
            .get(&pane_id)
            .is_some_and(|(held, _)| *held > generation)
        {
            return;
        }
        self.orphan_geometry.insert(pane_id, (generation, geometry));
    }

    /// The held geometry for this pane generation, if any. An older generation's is dropped and a
    /// newer one's kept, like `orphan_output`.
    pub fn take_orphan_geometry(
        &mut self,
        pane_id: PaneId,
        generation: u64,
    ) -> Option<PaneGeometry> {
        let (held, _) = *self.orphan_geometry.get(&pane_id)?;
        if held > generation {
            return None;
        }
        let (_, geometry) = self.orphan_geometry.remove(&pane_id)?;
        (held == generation).then_some(geometry)
    }
}

/// A pane's size and cell size as the server reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneGeometry {
    pub cols: u16,
    pub rows: u16,
    pub cell: Option<tui_lipan::TerminalCellSize>,
}

/// A pane spawn deferred until a session client is available (see [`super::State::pending_spawns`]).
pub type PendingPaneSpawn = crate::session::client::SpawnPaneRequest;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attach_request_resolves_its_occupied_session_policy() {
        use crate::config::SessionOnAttach::{Ask, Follow, Take};

        let join = AttachRequest::new("dev", AttachMode::Join);
        assert_eq!(join.occupied_policy(Ask), Some(Ask));
        assert_eq!(join.occupied_policy(Take), Some(Take));
        let take = join.clone().with_access(AttachAccess::Writable {
            on_attach: Some(Take),
        });
        assert_eq!(take.occupied_policy(Follow), Some(Take));
        assert_eq!(
            join.clone()
                .with_access(AttachAccess::ReadOnly)
                .occupied_policy(Take),
            None,
            "a viewer never asks for control"
        );
        for mode in [AttachMode::Reconnect, AttachMode::Recreate] {
            let reconnect = AttachRequest {
                mode,
                ..take.clone()
            };
            assert!(reconnect.is_reconnect());
            assert_eq!(
                reconnect.occupied_policy(Ask),
                None,
                "a reconnect resumes the session it lost without asking again"
            );
        }
        for mode in [
            AttachMode::Join,
            AttachMode::JoinOrStart,
            AttachMode::Create,
        ] {
            assert!(!AttachRequest::new("dev", mode).is_reconnect());
        }
    }

    #[test]
    fn orphan_store_preserves_complete_prefix_and_rejects_overflow() {
        let mut store = OrphanOutputStore::default();
        store.insert(1, 7, &[]);
        assert_eq!(store.stats(), OrphanOutputStats::default());

        let first = vec![1; ORPHAN_OUTPUT_CAP - 2];
        store.insert(1, 7, &first);
        assert!(!store.insert(1, 7, &[2, 3, 4, 5]));

        let retained = store.take(1, 7).expect("buffered generation");
        assert_eq!(retained.len(), ORPHAN_OUTPUT_CAP - 2);
        assert_eq!(&retained[..2], &[1, 1]);
        assert!(retained.iter().all(|b| *b == 1));
        assert_eq!(store.stats().retained, 0);
        assert_eq!(store.stats().keys, 0);
        assert_eq!(store.order.len(), 0);
    }

    #[test]
    fn orphan_store_rejects_new_buffer_at_global_budget() {
        let mut store = OrphanOutputStore::default();
        for pane_id in 0..=16 {
            store.insert(pane_id, 1, &vec![pane_id as u8; ORPHAN_OUTPUT_CAP]);
        }

        assert!(store.buffers.contains_key(&(0, 1)));
        assert!(store.buffers.contains_key(&(1, 1)));
        assert!(!store.buffers.contains_key(&(16, 1)));
        assert_eq!(
            store.stats(),
            OrphanOutputStats {
                retained: ORPHAN_OUTPUT_GLOBAL_CAP,
                high_water: ORPHAN_OUTPUT_GLOBAL_CAP,
                keys: 16,
            }
        );
        assert_eq!(store.order.len(), store.buffers.len());
    }

    #[test]
    fn orphan_store_rejects_excess_keys_during_a_tiny_frame_flood() {
        let mut store = OrphanOutputStore::default();
        for pane_id in 0..ORPHAN_OUTPUT_KEY_CAP as PaneId + 3 {
            store.insert(pane_id, 1, &[pane_id as u8]);
        }

        assert_eq!(store.stats().keys, ORPHAN_OUTPUT_KEY_CAP);
        assert_eq!(store.stats().retained, ORPHAN_OUTPUT_KEY_CAP);
        assert_eq!(store.stats().high_water, ORPHAN_OUTPUT_KEY_CAP);
        assert!(store.buffers.contains_key(&(0, 1)));
        assert!(store.buffers.contains_key(&(1, 1)));
        assert!(store.buffers.contains_key(&(2, 1)));
        assert_eq!(store.order.front(), Some(&(0, 1)));
        assert_eq!(
            store.order.back(),
            Some(&(ORPHAN_OUTPUT_KEY_CAP as PaneId - 1, 1))
        );
        assert_eq!(store.order.len(), store.buffers.len());
    }

    #[test]
    fn orphan_store_take_updates_accounting_and_order() {
        let mut store = OrphanOutputStore::default();
        store.insert(1, 1, b"abc");
        store.insert(2, 1, b"defgh");

        assert_eq!(store.take(1, 1), Some(b"abc".to_vec()));
        assert_eq!(store.stats().retained, 5);
        assert_eq!(store.stats().high_water, 8);
        assert_eq!(store.stats().keys, 1);
        assert_eq!(store.order.iter().copied().collect::<Vec<_>>(), [(2, 1)]);
        assert_eq!(store.take(1, 1), None);
    }

    #[test]
    fn orphan_store_discards_only_superseded_generations() {
        let mut store = OrphanOutputStore::default();
        store.insert(4, 2, b"old");
        store.insert(4, 3, b"exact");
        store.insert(4, 4, b"future");
        store.insert(5, 1, b"other");

        assert_eq!(store.take_for_generation(4, 3), Some(b"exact".to_vec()));
        assert!(!store.buffers.contains_key(&(4, 2)));
        assert_eq!(
            store.buffers.get(&(4, 4)).map(Vec::as_slice),
            Some(&b"future"[..])
        );
        assert_eq!(
            store.buffers.get(&(5, 1)).map(Vec::as_slice),
            Some(&b"other"[..])
        );
        assert_eq!(
            store.stats(),
            OrphanOutputStats {
                retained: b"future".len() + b"other".len(),
                high_water: b"old".len() + b"exact".len() + b"future".len() + b"other".len(),
                keys: 2,
            }
        );
        assert_eq!(
            store.order.iter().copied().collect::<Vec<_>>(),
            [(4, 4), (5, 1)]
        );
    }

    fn test_media_output(seed: u8) -> (Vec<u8>, Vec<tui_lipan::TerminalOutputMedia>, PathBuf) {
        use base64::Engine as _;

        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), [seed, 2, 3, 255]).unwrap();
        let name = base64::engine::general_purpose::STANDARD
            .encode(source.path().to_string_lossy().as_bytes());
        let mut server = tui_lipan::prelude::TerminalScreen::new(5, 20, 100);
        server.set_image_file_storage_enabled(true);
        server.set_output_transaction_recording(true);
        server.set_image_relay_policy(tui_lipan::GraphicsRelayPolicy::NamedAsFiles);
        server.process_bytes(
            format!("\x1b_Ga=T,t=f,f=32,s=1,v=1,i={seed},C=1,q=2;{name}\x1b\\").as_bytes(),
        );
        let transaction = server.take_output_transactions().remove(0);
        let path = transaction.media[0].file.path().to_owned();
        (transaction.bytes, transaction.media, path)
    }

    #[test]
    fn orphan_store_enforces_per_key_and_global_media_bounds_and_releases_files() {
        // Per-key cap: 8 media bytes / 2 files; global cap: 12 media bytes / 3 files.
        let mut store = OrphanOutputStore::with_media_limits(8, 12, 2, 3);

        let (b1, m1, p1) = test_media_output(1);
        let (b2, m2, p2) = test_media_output(2);
        let (b3, m3, p3) = test_media_output(3);
        assert!(store.insert_media(1, 2, &b1, m1));
        assert!(store.insert_media(1, 2, &b2, m2));
        assert_eq!(store.retained_media_bytes, 8);
        assert_eq!(store.retained_media_files, 2);

        // Exceeds per-key media limit; rejected media is dropped immediately.
        assert!(!store.insert_media(1, 2, &b3, m3));
        assert!(!p3.exists());
        assert_eq!(store.retained_media_bytes, 8);
        assert_eq!(store.retained_media_files, 2);

        // Second key fits within global limit (12 bytes / 3 files).
        let (b4, m4, p4) = test_media_output(4);
        assert!(store.insert_media(2, 1, &b4, m4));
        assert_eq!(store.retained_media_bytes, 12);
        assert_eq!(store.retained_media_files, 3);

        // Third key exceeds global media limit.
        let (b5, m5, p5) = test_media_output(5);
        assert!(!store.insert_media(3, 1, &b5, m5));
        assert!(!p5.exists());

        // Superseding generation 2 on pane 1 with generation 3 drops pane 1's older media files.
        let (b6, m6, p6) = test_media_output(6);
        // Taking generation 3 first evicts stale generation 2 on pane 1.
        assert_eq!(store.take_owned_for_generation(1, 3), None);
        assert!(!p1.exists());
        assert!(!p2.exists());
        assert_eq!(store.retained_media_bytes, 4);
        assert_eq!(store.retained_media_files, 1);

        assert!(store.insert_media(1, 3, &b6, m6));
        assert_eq!(store.retained_media_bytes, 8);
        assert_eq!(store.retained_media_files, 2);
        assert_eq!(store.high_water_media_bytes, 12);
        assert_eq!(store.high_water_media_files, 3);

        let (taken_bytes, taken_media) = store.take_owned_for_generation(1, 3).unwrap();
        assert_eq!(taken_bytes, b6);
        assert_eq!(taken_media.len(), 1);
        assert!(p6.exists());
        drop(taken_media);
        assert!(!p6.exists());
        assert_eq!(store.retained_media_bytes, 4);
        assert_eq!(store.retained_media_files, 1);

        assert_eq!(store.take(2, 1), Some(b4));
        assert!(!p4.exists());
        assert_eq!(store.retained_media_bytes, 0);
        assert_eq!(store.retained_media_files, 0);
    }
}
