use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::config::HyprmuxConfig;
use crate::tiling::append_tiled_window;

mod appearance;
mod attachment;
mod drag;
mod identity;
mod layout;
mod mode;
mod pane;
mod pickers;
mod search;
mod session;
mod sessions_view;
mod sidebar;
mod workspace;

pub use appearance::*;
pub use attachment::*;
pub use drag::*;
pub use identity::*;
pub use layout::*;
pub use mode::*;
pub use pane::*;
pub use pickers::*;
pub use search::*;
pub use session::*;
pub use sessions_view::*;
pub use sidebar::*;
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
    pub runtime_epoch: u64,
    /// Next candidate attachment id. Allocation also checks current/background ids so restored
    /// sessions can never cause an id to be reused.
    pub(crate) next_attachment_id: AttachmentId,
    pub command_link: Option<tui_lipan::CommandLink<crate::Msg>>,
    pub mode: Mode,
    pub moving_pane: Option<MoveSession>,
    pub resizing_pane: Option<ResizeSession>,
    pub split_drag: Option<SplitDragSession>,
    pub animation: GeometryAnimation,
    /// Last terminal/root viewport, used by geometry helpers outside a render context.
    pub last_viewport: Cell<Option<Rect>>,
    /// Last app-content viewport, used to snap geometry when sidebar reservation changes.
    pub last_content_viewport: Cell<Option<Rect>>,
    /// Clock text the workbar last actually rendered. The clock ticks every second, but the default
    /// `clock_format` has minute resolution, so this lets the tick skip the ~59 of 60 frames that
    /// would redraw an identical string. Recorded by the view (rather than reformatted in the
    /// handler) so the comparison is against what is really on screen, with no sampling skew.
    pub last_clock_text: RefCell<Option<String>>,
    pub sidebar_visible: bool,
    pub sidebar: SidebarState,
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
    /// The client's connection to the current session (client handle, identity, shared-layout
    /// lease, spawn/replay buffers, and its window-manager state). Reached through [`Self::current`]
    /// / [`Self::current_mut`].
    pub attachment: Attachment,
    /// Attachments retained in the background after switching away from them: their session client
    /// stays attached and their screens stay live (server output routes to them by epoch), so
    /// switching back is instant. Keyed by [`Attachment::epoch`]. Empty until a switch parks one.
    pub background: HashMap<AttachmentId, Attachment>,
    /// Known remote hosts for the unified Sessions view: configured aliases, recent ad-hoc targets,
    /// and hosts a live attachment targets. Seeded when the Sessions view opens; carries the
    /// per-host expand/collapse and error state that must survive the recurring session sweep.
    pub hosts: HostRegistry,
    /// Last-seen sessions per remote host, loaded from disk when the Sessions view is seeded and
    /// refreshed on each successful probe. Lets an offline or unreachable host still list the
    /// workplaces it had, rather than reading as empty. Convenience only — never authoritative, and
    /// it holds no credentials.
    pub host_session_cache: crate::session::HostSessionCache,
    /// A destructive action armed by its first press; the second press only fires while the arm
    /// time is within [`crate::ops::exit::CONFIRM_WINDOW_SECS`].
    pub pending_destructive: Option<PendingDestructiveConfirmation>,
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

/// The default single-pane attachment a fresh launch, a fresh ephemeral session, or a killed
/// session hops onto. Its one pane inherits the launch directory so the shell starts where hyprmux
/// was invoked. Factored out so every "start a blank session" path builds the same thing without
/// rebuilding the whole [`State`].
pub fn fresh_default_attachment(config: &HyprmuxConfig) -> Attachment {
    let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
    let initial_id = 1;
    let initial_rect = FloatRect {
        x: 4.0,
        y: 3.0,
        w: 80.0,
        h: 24.0,
    };
    let mut initial_pane = Pane::new(initial_id, config.scrollback, initial_rect);
    // Launch the first pane in the directory hyprmux was started from; without this it spawns with
    // no cwd and the PTY falls back to the shell's home directory.
    initial_pane.identity.cwd = config.cwd.clone();
    workspaces[0].panes.push(initial_pane);
    append_tiled_window(&mut workspaces[0], initial_id);
    workspaces[0].focused_pane = Some(initial_id);

    let mut attachment = Attachment::new();
    attachment.workspaces = workspaces;
    attachment.focused_pane = Some(initial_id);
    attachment.next_pane_id = initial_id + 1;
    attachment
}

impl State {
    pub fn new(config: HyprmuxConfig, theme: Theme) -> Self {
        let sidebar_visible = config.sidebar.visible;
        let sidebar = SidebarState::new(&config.sidebar);
        let attachment = fresh_default_attachment(&config);

        Self {
            config,
            runtime_epoch: 0,
            next_attachment_id: 1,
            command_link: None,
            mode: Mode::Normal,
            moving_pane: None,
            resizing_pane: None,
            split_drag: None,
            animation: GeometryAnimation::None,
            last_viewport: Cell::new(None),
            last_content_viewport: Cell::new(None),
            last_clock_text: RefCell::new(None),
            sidebar_visible,
            sidebar,
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
            attachment,
            background: HashMap::new(),
            hosts: HostRegistry::default(),
            host_session_cache: crate::session::HostSessionCache::new(),
            pending_destructive: None,
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

    /// The current session attachment.
    pub fn current(&self) -> &Attachment {
        &self.attachment
    }

    /// Mutable access to the [current attachment](Self::current).
    pub fn current_mut(&mut self) -> &mut Attachment {
        &mut self.attachment
    }

    /// The live attachment a server frame at `epoch` belongs to: the current attachment when the
    /// epoch matches [`Self::runtime_epoch`], otherwise a background attachment retained under that
    /// epoch. `None` for a stale epoch (a torn-down attachment).
    pub fn attachment_for_epoch_mut(&mut self, epoch: AttachmentId) -> Option<&mut Attachment> {
        if epoch == self.runtime_epoch {
            Some(&mut self.attachment)
        } else {
            self.background.get_mut(&epoch)
        }
    }

    pub fn attachment_for_epoch(&self, epoch: AttachmentId) -> Option<&Attachment> {
        if epoch == self.runtime_epoch {
            Some(&self.attachment)
        } else {
            self.background.get(&epoch)
        }
    }

    /// Mint an id newer than every current or retained attachment id.
    pub fn mint_attachment_id(&mut self) -> AttachmentId {
        let highest_live = self
            .background
            .keys()
            .copied()
            .chain(std::iter::once(self.runtime_epoch))
            .max()
            .unwrap_or(0);
        let id = self.next_attachment_id.max(highest_live.saturating_add(1));
        self.next_attachment_id = id.saturating_add(1);
        id
    }

    /// The id of a background attachment matching `name` and its resolved remote target, if one is
    /// retained. Used to switch back instantly instead of reconnecting.
    pub fn parked_attachment_id(
        &self,
        name: &str,
        remote_target: Option<&crate::session::remote::RemoteTarget>,
    ) -> Option<AttachmentId> {
        self.background.iter().find_map(|(id, attachment)| {
            (attachment.session_name.as_deref() == Some(name)
                && attachment.remote_target.as_ref() == remote_target)
                .then_some(*id)
        })
    }

    pub fn attachment_by_identity(
        &self,
        name: &str,
        remote_target: Option<&crate::session::remote::RemoteTarget>,
    ) -> Option<&Attachment> {
        std::iter::once(&self.attachment)
            .chain(self.background.values())
            .find(|attachment| {
                attachment.session_name.as_deref() == Some(name)
                    && attachment.remote_target.as_ref() == remote_target
            })
    }

    /// Park the current attachment into the background under `current_epoch`, installing
    /// `replacement` as the new current attachment. The parked attachment keeps its live session
    /// client and screens; it is only torn down on quit (or when explicitly closed).
    pub fn park_current(&mut self, current_epoch: AttachmentId, replacement: Attachment) {
        let mut parked = std::mem::replace(&mut self.attachment, replacement);
        parked.epoch = current_epoch;
        self.background.insert(current_epoch, parked);
    }

    /// Bring the background attachment `id` to the foreground, parking the current one under
    /// `current_epoch`. The restored attachment's screens are already live, so the caller only needs
    /// to re-seed the view. Returns the restored attachment's epoch (the new `runtime_epoch`).
    pub fn unpark(
        &mut self,
        id: AttachmentId,
        current_epoch: AttachmentId,
    ) -> Option<AttachmentId> {
        let restored = self.background.remove(&id)?;
        let restored_epoch = restored.epoch;
        let mut parked = std::mem::replace(&mut self.attachment, restored);
        parked.epoch = current_epoch;
        self.background.insert(current_epoch, parked);
        Some(restored_epoch)
    }

    /// Drop queued replay inputs whose spawn can no longer complete (see
    /// [`Attachment::prune_replay_inputs_to_pending_spawns`]).
    pub fn prune_replay_inputs_to_pending_spawns(&mut self) {
        self.current_mut().prune_replay_inputs_to_pending_spawns();
    }

    /// Whether the currently attached session is an auto-managed ephemeral session.
    pub fn is_ephemeral_session(&self) -> bool {
        self.current().is_ephemeral_session()
    }

    /// Whether a pane's contents reach the screen on the next frame.
    ///
    /// `view::render` only builds panes from the active workspace, plus the scratchpad and popup
    /// (which live outside the workspace lists). Output for anything else — a build running on
    /// another workspace — changes state that nothing currently draws, so the frame it would cost
    /// is pure waste. Skipping the frame never loses content: the snapshot is still updated in
    /// place, and whatever makes the pane visible (workspace switch, scratchpad toggle) renders
    /// from the current snapshot.
    ///
    /// Deliberately conservative — the scratchpad counts as rendered even while hidden, because it
    /// animates in and out and a stale frame there is worse than a redundant one.
    pub fn pane_is_rendered(&self, id: PaneId) -> bool {
        if self.scratch.as_ref().is_some_and(|pane| pane.id == id)
            || self.popup.as_ref().is_some_and(|pane| pane.id == id)
        {
            return true;
        }
        self.current()
            .active_workspace_ref()
            .panes
            .iter()
            .any(|pane| pane.id == id)
    }

    /// The active workspace of the current attachment. Single-borrow accessors so callers avoid the
    /// `workspaces[active_workspace]` double index.
    pub fn active_workspace_ref(&self) -> &Workspace {
        self.current().active_workspace_ref()
    }

    /// Mutable [active workspace](Self::active_workspace_ref).
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        self.current_mut().active_workspace_mut()
    }

    /// Whether this client may mutate the shared layout: always true when purely local (no shared
    /// session), otherwise true only while it holds the layout-control lease.
    pub fn is_controller(&self) -> bool {
        self.current().is_controller()
    }

    /// The number of clients attached to the shared session (1 when local/unshared).
    pub fn attached_client_count(&self) -> u32 {
        self.current().attached_client_count()
    }

    pub fn pane_input_block_reason(&self) -> Option<&'static str> {
        self.current().pane_input_block_reason()
    }

    /// The canonical pane canvas the controller publishes, if this client is a follower that
    /// should letterbox to it. `None` for the controller or a local session (renders to its own
    /// viewport).
    pub fn follower_canonical_canvas(&self) -> Option<(u16, u16)> {
        self.current().follower_canonical_canvas()
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

    pub fn canvas_bounds_from_terminal_viewport(&self, viewport: Rect) -> FloatRect {
        crate::geometry::canvas_bounds_from_content_viewport(
            self.content_viewport(viewport),
            self.top_chrome_height(),
        )
    }

    /// App-content viewport local to the non-sidebar child of the outer HStack.
    pub fn content_viewport(&self, terminal_viewport: Rect) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: terminal_viewport
                .w
                .saturating_sub(self.effective_sidebar_width(terminal_viewport)),
            h: terminal_viewport.h,
        }
    }

    pub fn effective_sidebar_width(&self, terminal_viewport: Rect) -> u16 {
        crate::geometry::effective_sidebar_width(
            terminal_viewport.w,
            self.config.sidebar.width,
            self.sidebar_visible,
        )
    }

    /// Terminal-space x offset of app content. Nested Canvas placements remain content-local.
    pub fn terminal_content_left_offset(&self, terminal_viewport: Rect) -> u16 {
        if self.config.sidebar.position == crate::config::SidebarPosition::Left {
            self.effective_sidebar_width(terminal_viewport)
        } else {
            0
        }
    }
}

#[cfg(test)]
mod render_visibility_tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use tui_lipan::prelude::Theme;

    fn state_with_two_workspaces() -> State {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(1, 100, rect));
        state.current_mut().workspaces[1].panes.clear();
        state.current_mut().workspaces[1]
            .panes
            .push(Pane::new(2, 100, rect));
        state.current_mut().active_workspace = 0;
        state
    }

    #[test]
    fn only_the_active_workspaces_panes_are_rendered() {
        let state = state_with_two_workspaces();
        assert!(state.pane_is_rendered(1));
        // A build running here still updates its screen; it just must not cost a frame.
        assert!(!state.pane_is_rendered(2));
    }

    #[test]
    fn switching_workspace_moves_which_panes_are_rendered() {
        let mut state = state_with_two_workspaces();
        state.current_mut().active_workspace = 1;
        assert!(!state.pane_is_rendered(1));
        assert!(state.pane_is_rendered(2));
    }

    #[test]
    fn scratchpad_and_popup_are_rendered_from_outside_the_workspace_lists() {
        let mut state = state_with_two_workspaces();
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        state.scratch = Some(Pane::new(7, 100, rect));
        state.popup = Some(Pane::new(8, 100, rect));
        // Both animate in and out, so they count as rendered even while hidden: a stale frame
        // there is worse than a redundant one.
        assert!(state.pane_is_rendered(7));
        assert!(state.pane_is_rendered(8));
    }

    #[test]
    fn an_unknown_pane_is_not_rendered() {
        let state = state_with_two_workspaces();
        assert!(!state.pane_is_rendered(999));
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use tui_lipan::prelude::Theme;

    fn state() -> State {
        State::new(HyprmuxConfig::default(), Theme::default())
    }

    #[test]
    fn park_and_unpark_round_trips_the_current_attachment() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".to_string());
        state.current_mut().session_attached = true;
        state.runtime_epoch = 5;

        // Park the current session; a fresh empty attachment takes its place.
        state.park_current(state.runtime_epoch, Attachment::new());
        assert!(state.background.contains_key(&5));
        assert_eq!(state.current().session_name, None);
        assert_eq!(state.parked_attachment_id("dev", None), Some(5));

        // Switching to a new session advances the epoch.
        state.runtime_epoch = 6;

        // Switch back: the parked "dev" returns as current, the fresh one parks under epoch 6.
        assert_eq!(state.unpark(5, state.runtime_epoch), Some(5));
        assert_eq!(state.current().session_name.as_deref(), Some("dev"));
        assert!(state.background.contains_key(&6));
        assert!(!state.background.contains_key(&5));
    }

    #[test]
    fn parked_lookup_distinguishes_remote_target() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".to_string());
        state.current_mut().remote_host = Some("winvm".to_string());
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        state.current_mut().remote_target = Some(target.clone());
        state.park_current(state.runtime_epoch, Attachment::new());

        assert_eq!(state.parked_attachment_id("dev", Some(&target)), Some(0));
        // A local `dev` is a different session from `dev` on `winvm`.
        assert_eq!(state.parked_attachment_id("dev", None), None);
    }

    #[test]
    fn parked_lookup_does_not_confuse_same_host_with_different_credentials() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".to_string());
        let alice = crate::session::remote::RemoteTarget::Url {
            user: Some("alice".to_string()),
            host: "example.com".to_string(),
            port: Some(22),
        };
        let bob = crate::session::remote::RemoteTarget::Url {
            user: Some("bob".to_string()),
            host: "example.com".to_string(),
            port: Some(2222),
        };
        state.current_mut().remote_host = Some("example.com".to_string());
        state.current_mut().remote_target = Some(alice.clone());
        state.park_current(state.runtime_epoch, Attachment::new());

        assert_eq!(state.parked_attachment_id("dev", Some(&alice)), Some(0));
        assert_eq!(state.parked_attachment_id("dev", Some(&bob)), None);
    }

    #[test]
    fn attachment_ids_remain_monotonic_after_restoring_an_old_attachment() {
        let mut state = state();
        state.runtime_epoch = 5;
        state.park_current(5, Attachment::new());
        state.runtime_epoch = 6;
        state.unpark(5, 6).expect("restore old attachment");
        state.runtime_epoch = 5;

        assert_eq!(state.mint_attachment_id(), 7);
    }

    #[test]
    fn attachment_for_epoch_routes_current_and_background() {
        let mut state = state();
        state.runtime_epoch = 9;
        state.park_current(state.runtime_epoch, Attachment::new());
        state.runtime_epoch = 10;

        assert!(state.attachment_for_epoch_mut(10).is_some(), "current");
        assert!(state.attachment_for_epoch_mut(9).is_some(), "background");
        assert!(state.attachment_for_epoch_mut(99).is_none(), "stale");
    }

    #[test]
    fn background_output_updates_only_the_parked_screen() {
        let mut state = state();
        // The initial pane (id 1) lives in the current attachment.
        let generation = state.current().active_workspace_ref().panes[0].pty_generation;
        state.runtime_epoch = 4;
        state.park_current(state.runtime_epoch, Attachment::new());
        state.runtime_epoch = 5;

        let parked = state.attachment_for_epoch_mut(4).expect("parked");
        parked.apply_background_output(1, generation, b"hi");

        assert!(
            state.background.get(&4).unwrap().workspaces[0].panes[0]
                .activity
                .has_unseen_output,
            "the parked pane records background activity"
        );
        // The fresh current attachment never had pane 1.
        assert!(state.current_mut().find_pane_mut(1).is_none());
    }

    #[test]
    fn background_output_ignores_a_stale_generation() {
        let mut state = state();
        let generation = state.current().active_workspace_ref().panes[0].pty_generation;
        state.runtime_epoch = 7;
        state.park_current(state.runtime_epoch, Attachment::new());
        state.runtime_epoch = 8;

        let parked = state.attachment_for_epoch_mut(7).expect("parked");
        parked.apply_background_output(1, generation.wrapping_add(1), b"hi");

        assert!(
            !state.background.get(&7).unwrap().workspaces[0].panes[0]
                .activity
                .has_unseen_output,
            "output for a stale generation is dropped"
        );
    }
}
