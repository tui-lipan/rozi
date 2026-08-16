use serde::{Deserialize, Serialize};

use crate::runtime_metrics::ServerRuntimeMetrics;
use crate::session::protocol::pane_runtime::{
    PaneMeta, PaneRuntimeState, PublishedRow, WirePalette,
};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

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
        launch: Option<crate::pane_launch::PaneLaunch>,
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
        /// config. Used verbatim when `launch` is `None`; also used to `exec` into after a shell
        /// command or direct process completes when `keep_open` is set (see [`ServerPane`] docs).
        shell: Vec<String>,
        /// Resolved command-runner argv (non-empty; program then fixed args), resolved
        /// client-side via `platform::command::resolve_command_shell`. Only used for
        /// [`PaneLaunch::Shell`](crate::pane_launch::PaneLaunch::Shell).
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
    /// Replace the pane's published rows. An empty list withdraws them, and the pane falls
    /// back to screen detection.
    ReportPaneRows {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        rows: Vec<PublishedRow>,
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
    /// target an [`ServerMessage::Error`] carrying [`super::constants::EVICTED_ERROR_CODE`] and then closes its
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
