use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::config::HyprmuxConfig;
use crate::tiling::append_tiled_window;

mod appearance;
mod drag;
mod identity;
mod layout;
mod mode;
mod pane;
mod pickers;
mod search;
mod session;
mod workspace;

pub use appearance::*;
pub use drag::*;
pub use identity::*;
pub use layout::*;
pub use mode::*;
pub use pane::*;
pub use pickers::*;
pub use search::*;
pub use session::*;
pub use workspace::*;

pub type PaneId = u32;

/// Reserved id for the scratchpad pane. Workspace panes start at 1 (see `State::new`), so 0
/// can never collide with an allocated `next_pane_id`.
pub const SCRATCH_PANE_ID: PaneId = 0;
pub const POPUP_PANE_ID: PaneId = u32::MAX;
pub const WORKBAR_HEIGHT: u16 = 1;
pub const TILE_GAP: f32 = 1.0;
pub const OUTER_GAP: f32 = 1.0;
pub const OFFSCREEN_MIN_VISIBLE: f32 = 6.0;
pub const DEFAULT_RATIO: f32 = 0.58;
pub const MIN_SPLIT_RATIO: f32 = 0.20;
pub const MAX_SPLIT_RATIO: f32 = 0.80;
pub const RATIO_STEP: f32 = 0.04;
/// Default weight for tile width against height when choosing a dwindle split direction.
pub const DEFAULT_SPLIT_WIDTH_MULTIPLIER: f32 = 2.3;

pub struct State {
    pub config: HyprmuxConfig,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub focused_pane: Option<PaneId>,
    pub next_pane_id: PaneId,
    pub next_pty_generation: u64,
    pub runtime_epoch: u64,
    pub command_link: Option<tui_lipan::CommandLink<crate::Msg>>,
    pub mode: Mode,
    pub moving_pane: Option<MoveSession>,
    pub resizing_pane: Option<ResizeSession>,
    pub split_drag: Option<SplitDragSession>,
    pub animation: GeometryAnimation,
    pub last_viewport: Cell<Option<Rect>>,
    pub show_palette: bool,
    pub show_help: bool,
    pub show_appearance: bool,
    pub pane_padding_editor: Option<PanePaddingEditorState>,
    pub show_theme_picker: bool,
    pub theme_picker_preview: Option<ThemePickerPreview>,
    pub theme: Theme,
    pub system_theme: Option<Theme>,
    pub theme_watcher: Option<ThemeWatcher>,
    pub search: Option<ScrollbackSearchState>,
    pub rename: Option<PaneRenameState>,
    pub rename_session: Option<SessionRenameState>,
    pub save_profile_prompt: Option<SaveProfileState>,
    pub show_profile_picker: bool,
    pub profile_picker: Option<ProfilePickerState>,
    pub show_session_picker: bool,
    pub session_picker: Option<SessionPickerState>,
    pub client_list: Option<ClientListState>,
    pub last_blocked_input_toast: Option<Instant>,
    pub(crate) replaceable_toasts: HashMap<ToastChannel, OverlayId>,
    /// Incremented each time the session picker opens; tags the off-thread auto-refresh watcher so
    /// stale ticks from a previous opening (or after close) are ignored.
    pub session_picker_epoch: u64,
    pub profile_picker_epoch: u64,
    pub copy_mode: Option<CopyModeState>,
    pub hint_mode: Option<HintModeState>,
    pub copy_flash: Option<CopyFlashState>,
    pub next_copy_flash_id: u64,
    pub scratch: Option<Pane>,
    pub scratch_visible: bool,
    /// Focus to restore when the scratchpad is hidden again.
    pub scratch_return_focus: Option<PaneId>,
    /// Runtime height override for the scratchpad as a fraction of the tile height, set by
    /// dragging its top edge. `None` falls back to `config.scratchpad.height`.
    pub scratch_height: Option<f32>,
    /// Height fraction captured at the start of a scratchpad top-edge resize drag, so each drag
    /// move recomputes from the origin (drift-free) rather than accumulating deltas.
    pub scratch_resize_start: Option<f32>,
    pub popup: Option<Pane>,
    pub popup_return_focus: Option<PaneId>,
    pub control_socket_path: Option<PathBuf>,
    pub event_hub: crate::events::EventHub,
    pub session_client: Option<crate::session::client::SessionClient>,
    pub session_name: Option<String>,
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
    /// A destructive action armed by its first press; the second press only fires while the arm
    /// time is within [`crate::ops::exit::CONFIRM_WINDOW_SECS`].
    pub pending_destructive: Option<PendingDestructiveConfirmation>,
    /// Shared-session bookkeeping for the attached named/ephemeral session: the layout lease,
    /// revision counters, canonical canvas, and reconciliation buffers. `None` until the session
    /// handshake completes (and while purely local, pre-attach).
    pub shared: Option<SharedSessionState>,
    /// Cached first-line stdout for each configured `WorkbarSegment::Command`, keyed by the raw
    /// command string. Refreshed on a background timer per command; empty until the first run
    /// completes.
    pub workbar_command_output: HashMap<String, String>,
    /// Commands that already have a background poller thread running (see
    /// [`crate::pane_lifecycle::spawn_workbar_command_pollers`]). A config reload spawns pollers
    /// only for commands newly added by the reload, since existing pollers never stop.
    pub workbar_commands_running: HashSet<String>,
    /// Set whenever something `crate::commands::sync` needs to see (shortcuts, dynamic labels,
    /// or the `commands_active` gate) may have changed. Checked once per message at the tail of
    /// `update::handle_msg` rather than resyncing unconditionally, since high-frequency messages
    /// (PTY output, keystrokes forwarded to a pane) never affect it.
    pub commands_dirty: bool,
}

impl State {
    pub fn new(config: HyprmuxConfig, theme: Theme) -> Self {
        let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
        let initial_id = 1;
        let initial_rect = FloatRect {
            x: 4.0,
            y: 3.0,
            w: 80.0,
            h: 24.0,
        };
        let mut initial_pane = Pane::new(initial_id, config.scrollback, initial_rect);
        // Launch the first pane in the directory hyprmux was started from; without this it
        // spawns with no cwd and the PTY falls back to the shell's home directory.
        initial_pane.identity.cwd = config.cwd.clone();
        workspaces[0].panes.push(initial_pane);
        append_tiled_window(&mut workspaces[0], initial_id);
        workspaces[0].focused_pane = Some(initial_id);

        Self {
            config,
            workspaces,
            active_workspace: 0,
            focused_pane: Some(initial_id),
            next_pane_id: initial_id + 1,
            next_pty_generation: 1,
            runtime_epoch: 0,
            command_link: None,
            mode: Mode::Normal,
            moving_pane: None,
            resizing_pane: None,
            split_drag: None,
            animation: GeometryAnimation::None,
            last_viewport: Cell::new(None),
            show_palette: false,
            show_help: false,
            show_appearance: false,
            pane_padding_editor: None,
            show_theme_picker: false,
            theme_picker_preview: None,
            theme,
            system_theme: None,
            theme_watcher: None,
            search: None,
            rename: None,
            rename_session: None,
            save_profile_prompt: None,
            show_profile_picker: false,
            profile_picker: None,
            show_session_picker: false,
            session_picker: None,
            client_list: None,
            last_blocked_input_toast: None,
            replaceable_toasts: HashMap::new(),
            session_picker_epoch: 0,
            profile_picker_epoch: 0,
            copy_mode: None,
            hint_mode: None,
            copy_flash: None,
            next_copy_flash_id: 1,
            scratch: None,
            scratch_visible: false,
            scratch_return_focus: None,
            scratch_height: None,
            scratch_resize_start: None,
            popup: None,
            popup_return_focus: None,
            control_socket_path: None,
            event_hub: crate::events::EventHub::default(),
            session_client: None,
            session_name: None,
            created_from_profile: None,
            deferred_profile_seed: None,
            pending_profile_loaded: None,
            session_attached: false,
            pending_session_attach: None,
            pending_spawns: Vec::new(),
            pending_replay_inputs: HashMap::new(),
            pending_destructive: None,
            shared: None,
            workbar_command_output: HashMap::new(),
            workbar_commands_running: HashSet::new(),
            commands_dirty: false,
        }
    }

    pub fn from_profile(
        config: HyprmuxConfig,
        theme: Theme,
        profile: crate::profiles::HyprmuxProfile,
    ) -> Self {
        crate::profiles::restore_state_from_profile(config, theme, profile)
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
            Some("Input is locked to the controller")
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

    /// Vertical space (in rows) the workbar removes from the panes area. Independent of whether
    /// the workbar sits at the top or the bottom - either way it consumes the same one row.
    pub fn top_chrome_height(&self) -> u16 {
        if self.config.pane.show_workbar {
            WORKBAR_HEIGHT
        } else {
            0
        }
    }

    /// Row offset of the panes area from the top of the viewport: the workbar height when the
    /// workbar sits above the panes, and 0 when it sits below them (the panes start at the first
    /// row and the workbar is drawn on the last row). Used to translate between root and
    /// canvas-local space.
    pub fn content_top_offset(&self) -> u16 {
        if self.config.pane.show_workbar && !self.config.pane.workbar_at_bottom {
            WORKBAR_HEIGHT
        } else {
            0
        }
    }

    /// Signed inset (in cells) that keeps the panes clear of the workbar. Positive insets the top
    /// edge of the tile area (workbar above the panes); negative insets the bottom edge (workbar
    /// below the panes), so the gap always lands between the panes and the workbar. Zero when
    /// there is no gap.
    pub fn workspace_top_gap(&self) -> f32 {
        if self.config.pane.show_workbar && self.config.pane.workbar_gap {
            if self.config.pane.workbar_at_bottom {
                -OUTER_GAP
            } else {
                OUTER_GAP
            }
        } else {
            0.0
        }
    }

    /// Per-axis gap between adjacent tiled panes. When border merging is on the gap goes negative
    /// so neighboring panes overlap by exactly one cell: their borders land on the same column/row
    /// and the terminal backend fuses the shared glyphs (`┬`/`├`/`┼`/…) with no extra divider. The
    /// vertical overlap is suppressed while titlebars are shown, since a lower pane's title row
    /// would otherwise cover the border of the pane above it.
    pub fn tile_gap(&self) -> TileGap {
        if self.config.pane.merge_borders {
            TileGap {
                horizontal: -1.0,
                vertical: if self.config.pane.show_titles {
                    0.0
                } else {
                    -1.0
                },
            }
        } else {
            TileGap::DEFAULT
        }
    }

    pub fn canvas_bounds(&self, viewport: Rect) -> FloatRect {
        crate::geometry::canvas_bounds_from_viewport(viewport, self.top_chrome_height())
    }
}
