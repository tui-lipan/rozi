use tui_lipan::prelude::*;

use crate::input::Action;
use crate::state::{PaneId, ResizeCorner};
#[allow(unused_imports)]
use crate::{config, platform};
use crate::{control, session, shared_layout};

#[derive(Clone)]
pub enum Msg {
    CommandLinkReady(CommandLink<Msg>),
    /// The controlling terminal or console asked this client to go away (Unix `SIGHUP`/`SIGTERM`,
    /// Windows console close/logoff/shutdown). Delivered from
    /// [`platform::server_lifecycle::on_hangup`]'s worker thread so the clean detach path runs
    /// instead of the process dying where it stands.
    Hangup,
    RunAction(Action),
    ClosePalette,
    CloseHelp,
    CloseAppearance,
    AppearanceActivate(crate::state::AppearanceAction),
    ClosePanePaddingEditor,
    PanePaddingVerticalChanged(InputEvent),
    PanePaddingHorizontalChanged(InputEvent),
    AdvancePanePadding,
    SubmitPanePadding,
    CloseThemePicker,
    /// Index into [`config::theme_choices`]: preview the highlighted theme.
    PreviewTheme(usize),
    /// Index into [`config::theme_choices`]: commit the chosen theme.
    SelectTheme(usize),
    ThemeTick,
    WorkbarTick,
    /// The config watcher saw `hyprmux.toml` change on disk; reload it if the content differs.
    ConfigFileChanged,
    /// A `WorkbarSegment::Command` poller produced fresh output: (command string, first output line).
    WorkbarCommandOutput(String, String),
    ThemeError(String),
    CloseSearch,
    SearchQueryChanged(String),
    SearchNext(bool),
    SearchSelect(usize),
    SearchActivate(usize),
    SearchCycleScope,
    CloseRenamePane,
    RenamePaneChanged(InputEvent),
    SubmitRenamePane,
    CloseRenameSession,
    RenameSessionChanged(InputEvent),
    SubmitRenameSession,
    CloseSaveProfile,
    SaveProfileNameChanged(InputEvent),
    SubmitSaveProfile,
    CloseProfilePicker,
    ProfilePickerQueryChanged(String),
    ProfilePickerSelect(usize),
    ProfilePickerSetDefault,
    ProfilePickerDelete,
    ProfilePickerApply,
    ProfilePickerOpenAs,
    ProfilePickerNew,
    SelectProfile(usize),
    ProfileSessionsDiscovered {
        epoch: u64,
        rows: Vec<crate::session::discovery::DiscoveredSession>,
    },
    CloseSessionPicker,
    /// Off-thread auto-refresh results for the open session picker, tagged with the opening's epoch.
    SessionsDiscovered {
        epoch: u64,
        rows: Vec<crate::session::discovery::DiscoveredSession>,
    },
    SessionPickerQueryChanged(String),
    SessionPickerSelect(usize),
    SessionPickerActivate(usize),
    SessionPickerCreateFromQuery,
    SessionPickerDetachCurrent,
    SessionPickerKillSelected,
    SessionPickerNameCurrent,
    CloseClientList,
    ClientListSelect(usize),
    ClientListGrant(usize),
    ClientListDecline(usize),
    FocusPane(PaneId),
    ClosePopup,
    HoverPane(PaneId),
    BeginMove(PaneId, FloatRect, u16, u16, u16, u16, bool),
    MovePane(PaneId, i16, i16, bool),
    EndMove(PaneId, u16, u16),
    BeginResize(PaneId, ResizeCorner, u16, u16, bool),
    ResizePane(PaneId, ResizeCorner, u16, u16, u16, u16, bool),
    EndResize(PaneId),
    BeginResizeSplit(PaneId, bool, u16, u16),
    /// Drag a tiled split boundary: (left/top pane, horizontal_split, from_x, from_y, x, y).
    ResizeSplit(PaneId, bool, u16, u16, u16, u16),
    BeginResizeSplitJunction(u16, u16),
    /// Drag a tiled split junction: pane representatives for horizontal/vertical tree splits,
    /// followed by (from_x, from_y, x, y).
    ResizeSplitJunction(Vec<PaneId>, Vec<PaneId>, u16, u16, u16, u16),
    EndResizeSplit,
    /// Grab the scratchpad's top edge to resize its height: drag origin y (root coordinates).
    BeginScratchResize(u16),
    /// Drag the scratchpad's top edge: (from_y, y) in root coordinates.
    ScratchResize(u16, u16),
    EndScratchResize,
    FinishOpen(u64, PaneId, u64),
    ActivatePane(u64, PaneId, u64),
    PruneClosed(u64, PaneId, u64),
    PaneInput(PaneId, TerminalInputEvent),
    PaneKey(PaneId, KeyEvent),
    PaneMouse(PaneId, Vec<u8>),
    PaneResize(PaneId, u16, u16),
    PaneScroll(PaneId, usize),
    CopyFlashExpired(PaneId, u64),
    ControlRequest(control::ControlEnvelope),
    SessionConnected {
        epoch: u64,
        name: String,
        client: session::client::SessionClient,
    },
    SessionDisconnected {
        epoch: u64,
        name: String,
    },
    SessionAttachFailed {
        epoch: u64,
        message: String,
    },
    SessionAttached {
        epoch: u64,
        session: String,
        client_id: shared_layout::ClientId,
        panes: Vec<session::protocol::PaneMeta>,
        layout_rev: u64,
        layout: Option<shared_layout::SharedLayout>,
        controller: Option<shared_layout::ClientId>,
        clients: Vec<session::protocol::ClientInfo>,
        input_locked: bool,
        read_only: bool,
        created_from_profile: Option<String>,
    },
    SessionOriginSet {
        epoch: u64,
        created_from_profile: String,
    },
    SessionLayoutCommitted {
        epoch: u64,
        rev: u64,
        author: shared_layout::ClientId,
        layout: shared_layout::SharedLayout,
    },
    SessionLayoutRejected {
        epoch: u64,
        current_rev: u64,
        layout: Option<shared_layout::SharedLayout>,
    },
    SessionControllerChanged {
        epoch: u64,
        controller: Option<shared_layout::ClientId>,
        reason: session::protocol::ControllerChangeReason,
    },
    SessionClientsChanged {
        epoch: u64,
        clients: Vec<session::protocol::ClientInfo>,
        input_locked: bool,
    },
    /// Another client asked this (controller) client for the layout-control lease.
    SessionControlRequested {
        epoch: u64,
        from: shared_layout::ClientId,
    },
    /// This client's pending control request was declined by the controller.
    SessionControlDeclined {
        epoch: u64,
    },
    SessionPing {
        epoch: u64,
        seq: u64,
    },
    /// Trailing-edge flush of debounced controller pane resizes (see `pty_events::handle_pane_resize`).
    FlushPaneResizes {
        epoch: u64,
    },
    /// Trailing-edge flush of the controller's shared layout.
    FlushLayoutCommit {
        epoch: u64,
    },
    SessionSpawnResult {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        pid: Option<u32>,
        ok: bool,
        error: Option<String>,
    },
    SessionOutput {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
    SessionResized {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    SessionExited {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        code: i32,
    },
    SessionPaneLoggingChanged {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        enabled: bool,
        path: Option<String>,
        error: Option<String>,
    },
    SessionPaneRuntimeChanged {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        state: crate::session::protocol::PaneRuntimeState,
    },
    SessionError {
        epoch: u64,
        message: String,
    },
    SessionRenamed {
        epoch: u64,
        session: String,
    },
}
