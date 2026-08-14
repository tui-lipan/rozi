use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use tui_lipan::prelude::{FloatRect, Rect, Theme, ThemeWatcher};

use crate::anim::GeometryAnimation;
use crate::config::Config;
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
mod settings;
mod sidebar;
mod workbar;
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
pub use settings::*;
pub use sidebar::*;
pub use workbar::*;
pub use workspace::*;

pub type PaneId = u32;

/// Which workspace a layout edit applies to. The scratchpad is a client-local workspace laid out
/// in the dropdown rect rather than an entry in the attachment's workspace list, so it cannot be
/// named by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutTarget {
    Scratch,
    Workspace(usize),
}

pub const POPUP_PANE_ID: PaneId = u32::MAX;
pub const WORKBAR_HEIGHT: u16 = 1;
pub const TILE_GAP: f32 = 1.0;
pub const OUTER_GAP: f32 = 1.0;
pub const OFFSCREEN_MIN_VISIBLE: f32 = 6.0;
pub const DEFAULT_RATIO: f32 = 0.58;
/// Default Scrollable column width as a fraction of the tile viewport.
pub const DEFAULT_SCROLLABLE_WIDTH: f32 = 0.45;
pub const MIN_SPLIT_RATIO: f32 = 0.20;
pub const MAX_SPLIT_RATIO: f32 = 0.80;
pub const RATIO_STEP: f32 = 0.04;
/// Default weight for tile width against height when choosing a dwindle split direction.
pub const DEFAULT_SPLIT_WIDTH_MULTIPLIER: f32 = 2.3;

pub struct State {
    pub config: Config,
    /// Whether the host terminal/window currently has focus. This is distinct from which pane the
    /// app has selected: a selected pane is only attended while the host window is focused too.
    pub window_focused: bool,
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
    /// Invalidates the framework-owned workspace Canvas when a pane relocates between workspace
    /// collections without a local workspace switch, so relocation is never mistaken for exit.
    pub pane_canvas_epoch: u64,
    /// Last terminal/root viewport, used by geometry helpers outside a render context.
    pub last_viewport: Cell<Option<Rect>>,
    /// Last app-content viewport, used to snap geometry when sidebar reservation changes.
    pub last_content_viewport: Cell<Option<Rect>>,
    /// Last box the scratchpad's panes tiled inside, in root coordinates. Compared each frame for
    /// the same reason as [`Self::last_content_viewport`]: the dropdown's box moves while it grows,
    /// and a pane transition chasing it would settle on its own curve instead. `None` while the
    /// dropdown is off screen, so its next appearance counts as a move.
    pub last_scratch_rect: Cell<Option<Rect>>,
    /// Clock text the workbar last actually rendered. The clock ticks every second, but the default
    /// `clock_format` has minute resolution, so this lets the tick skip the ~59 of 60 frames that
    /// would redraw an identical string. Recorded by the view (rather than reformatted in the
    /// handler) so the comparison is against what is really on screen, with no sampling skew.
    pub last_clock_text: RefCell<Option<String>>,
    /// Shared half-period phase for urgent alert borders and inactive workspace markers.
    pub alert_pulse_phase: bool,
    /// Phase for calm alerts (a finished agent), flipped once per full urgent cycle so it breathes
    /// `anim::ALERT_PULSE_CALM_FACTOR` times slower off the same tick chain.
    pub alert_pulse_calm_phase: bool,
    /// Whether one delayed alert-pulse tick is already queued.
    pub alert_pulse_armed: bool,
    pub sidebar_visible: bool,
    /// How far the sidebar has slid in: `0.0` fully retracted, `1.0` fully deployed. Recorded by the
    /// view each frame, because the transition driving it lives there.
    ///
    /// Layout reads this rather than [`Self::sidebar_visible`], so the columns the sidebar reserves
    /// grow and shrink with the animation and the pane column is genuinely resized to make room -
    /// the same way the tile beside a spawning pane is. Seeded from the config so a `State` that has
    /// never been rendered reports settled geometry.
    pub sidebar_slide: Cell<f32>,
    pub sidebar: SidebarState,
    pub workbar: WorkbarState,
    pub show_palette: bool,
    /// Whether the command palette's trimmed query is exactly `sidebar`. This scopes the primary
    /// Sidebar toggle's result priority to that one broad query without disturbing empty-list order.
    pub command_palette_sidebar_query: bool,
    pub show_help: bool,
    pub show_settings: bool,
    /// Highlighted settings row. Drives Left/Right stepping and
    /// `initial_selected_item_index` while the overlay is open.
    pub settings_selected: Option<SettingsAction>,
    pub do_not_disturb: bool,
    pub(crate) sound_cues: HashMap<crate::platform::sound::Cue, std::time::Instant>,
    pub pane_padding_editor: Option<PanePaddingEditorState>,
    pub show_theme_picker: bool,
    pub theme_picker_preview: Option<ThemePickerPreview>,
    /// The theme picker's highlighted row, index into `theme_choices()`. Drives the palette's
    /// `initial_selected_item_index` so filtering preserves the user's selection (or falls to the
    /// first match) instead of snapping back to the active theme. `None` when the picker is closed.
    pub theme_picker_selected: Option<usize>,
    pub show_layout_picker: bool,
    pub layout_picker: Option<LayoutPickerState>,
    pub theme: Theme,
    pub system_theme: Option<Theme>,
    pub theme_watcher: Option<ThemeWatcher>,
    /// Global search-scan generation; never reset when the search overlay closes.
    pub search_scan_epoch: u64,
    /// Epoch of the one cooperative search chunk currently queued in the runtime.
    pub search_scan_scheduled_epoch: Option<u64>,
    pub search: Option<ScrollbackSearchState>,
    pub rename: Option<PaneRenameState>,
    pub rename_session: Option<SessionRenameState>,
    pub save_profile_prompt: Option<SaveProfileState>,
    pub show_profile_picker: bool,
    pub profile_picker: Option<ProfilePickerState>,
    pub show_session_picker: bool,
    pub session_picker: Option<SessionPickerState>,
    pub collaboration: Option<CollaborationState>,
    /// Raised when an attach lands on a session another client is actively controlling, so
    /// following is something the user chooses rather than something that happens to them.
    pub follow_prompt: Option<FollowPromptState>,
    /// Where the open nested dialog was raised from, so cancelling or finishing it returns there
    /// rather than to the focused pane. Assigned by every child-dialog opener (to `None` when the
    /// child was raised standalone, which is what keeps it from going stale) and consumed by
    /// [`crate::ops::overlay_return::restore`].
    pub overlay_return: Option<OverlayOrigin>,
    /// Toasts still young enough to be de-duplicated against, keyed by the slot they occupy.
    /// Pruned on every notification, so it holds only what was raised in the last few seconds.
    pub(crate) replaceable_toasts:
        HashMap<crate::pty_events::ToastKey, crate::pty_events::TrackedToast>,
    /// Incremented each time the session picker opens; tags the off-thread auto-refresh watcher so
    /// stale ticks from a previous opening (or after close) are ignored.
    pub session_picker_epoch: u64,
    pub profile_picker_epoch: u64,
    pub copy_mode: Option<CopyModeState>,
    /// Pane whose historical viewport is retained until range-based copy feedback finishes.
    pub copy_feedback_target: Option<(AttachmentId, PaneId)>,
    pub copy_feedback_epoch: u64,
    pub hint_mode: Option<HintModeState>,
    /// Client-local workspace rendered in the dropdown. It is deliberately outside every
    /// attachment, so profiles and shared-layout commits cannot serialize it.
    pub scratch: Workspace,
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
    /// Open `agent-slots` streams, keyed by the pane whose program opened one.
    ///
    /// Held so a sidebar click can ask that program to bring a slot on screen. Dropping the sender
    /// closes the stream, which is also how a pane's slots are withdrawn - a publisher that dies
    /// cannot leave rows behind.
    pub agent_slot_streams: std::collections::HashMap<PaneId, std::sync::mpsc::SyncSender<String>>,
    /// The client's connection to the current session (client handle, identity, shared-layout
    /// lease, spawn/replay buffers, and its window-manager state). Reached through [`Self::current`]
    /// / [`Self::current_mut`].
    pub attachment: Attachment,
    /// Attachments retained in the background after switching away from them: their session client
    /// stays attached and their screens stay live (server output routes to them by epoch), so
    /// switching back is instant. Keyed by [`Attachment::epoch`]. Empty until a switch parks one.
    pub background: HashMap<AttachmentId, Attachment>,
    /// The attachment a launcher-state client would hand to the session it starts: the panes a
    /// launch had prepared (the initial shell, or a restored profile/autosave layout) before the
    /// startup picker took the foreground. Taken by the first "start a shell" that follows, so
    /// choosing it after dismissing the picker still lands on the layout the launch intended.
    /// `None` once consumed, and for a launcher reached by killing a session rather than at launch.
    pub launcher_seed: Option<Attachment>,
    /// PTY action waiting for an ephemeral session attach (open-config, user `run`/`popup`,
    /// scratchpad, control `new-pane`). Cleared on attach success or failure.
    pub pending_session_action: Option<PendingSessionAction>,
    /// Control-socket reply held while [`Self::pending_session_action`] waits for attach, so
    /// `new-pane` / `popup` can answer with the real pane id after the session is up.
    pub pending_control_reply: Option<std::sync::mpsc::Sender<crate::control::ControlResponse>>,
    /// Control-socket `new-pane` replies held until the pane's PTY actually reports ready, so the
    /// answer states readiness instead of mere acceptance. Keyed by
    /// `(epoch, local, pane id, generation)` so a client-local pane and a shared pane that share a
    /// numeric id cannot steal each other's reply. The epoch keeps a parked attachment's spawn from
    /// colliding with a fresh one that restarts pane ids and generations from the same counters.
    /// Entries are resolved by the spawn result or by [`crate::Msg::SpawnReplyDeadline`], whichever
    /// lands first.
    pub pending_spawn_replies:
        HashMap<(u64, bool, PaneId, u64), std::sync::mpsc::Sender<crate::control::ControlResponse>>,
    /// Control-socket input (`send-text` / `send-keys`) accepted for a pane whose PTY is still
    /// starting, flushed as type-ahead once it is ready. Keyed by `(local, pane id, generation)` so
    /// a client-local pane and a shared pane that share a numeric id cannot steal each other's
    /// queued input. Matches [`crate::state::Attachment::pending_replay_inputs`] for the shared
    /// namespace, which it always flushes behind so a restored pane runs its own command first.
    pub pending_control_input: HashMap<(bool, PaneId, u64), Vec<u8>>,
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
    /// time is within [`crate::ops::confirm::CONFIRM_WINDOW`].
    pub pending_destructive: Option<PendingDestructiveConfirmation>,
    /// Identifies the currently armed confirmation, whichever surface armed it. Advanced by every
    /// arming, so the expiry scheduled for one arming can recognize that a later one replaced it and
    /// leave that one alone. See [`crate::ops::confirm`].
    pub confirm_epoch: u64,
    /// Source of [`Attachment::parked_seq`] stamps, counting parkings so background attachments can
    /// be ordered by how recently they were used.
    next_parked_seq: u64,
    /// Set whenever something `crate::commands::sync` needs to see (shortcuts, dynamic labels,
    /// or the `commands_active` gate) may have changed. Checked once per message at the tail of
    /// `update::handle_msg` rather than resyncing unconditionally, since high-frequency messages
    /// (PTY output, keystrokes forwarded to a pane) never affect it.
    pub commands_dirty: bool,
}

/// The default single-pane attachment a fresh launch, a fresh ephemeral session, or a killed
/// session hops onto. Its one pane inherits the launch directory so the shell starts where rozi
/// was invoked. Factored out so every "start a blank session" path builds the same thing without
/// rebuilding the whole [`State`].
pub fn fresh_default_attachment(config: &Config) -> Attachment {
    let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
    for workspace in &mut workspaces {
        workspace.layout_kind = config.layout.default;
    }
    let initial_id = 1;
    let initial_rect = FloatRect {
        x: 4.0,
        y: 3.0,
        w: 80.0,
        h: 24.0,
    };
    let mut initial_pane = Pane::new(initial_id, config.scrollback, initial_rect);
    // Launch the first pane in the directory rozi was started from; without this it spawns with
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
    pub fn new(config: Config, theme: Theme) -> Self {
        let sidebar_visible = config.sidebar.visible;
        let sidebar = SidebarState::new(&config.sidebar);
        let attachment = fresh_default_attachment(&config);

        Self {
            config,
            window_focused: true,
            runtime_epoch: 0,
            next_attachment_id: 1,
            command_link: None,
            mode: Mode::Normal,
            moving_pane: None,
            resizing_pane: None,
            split_drag: None,
            animation: GeometryAnimation::None,
            pane_canvas_epoch: 0,
            last_viewport: Cell::new(None),
            last_content_viewport: Cell::new(None),
            last_scratch_rect: Cell::new(None),
            last_clock_text: RefCell::new(None),
            alert_pulse_phase: false,
            alert_pulse_calm_phase: false,
            alert_pulse_armed: false,
            sidebar_visible,
            sidebar_slide: Cell::new(if sidebar_visible { 1.0 } else { 0.0 }),
            sidebar,
            workbar: WorkbarState::default(),
            show_palette: false,
            command_palette_sidebar_query: false,
            show_help: false,
            show_settings: false,
            settings_selected: None,
            do_not_disturb: false,
            sound_cues: HashMap::new(),
            pane_padding_editor: None,
            show_theme_picker: false,
            theme_picker_preview: None,
            theme_picker_selected: None,
            show_layout_picker: false,
            layout_picker: None,
            theme,
            system_theme: None,
            theme_watcher: None,
            search_scan_epoch: 0,
            search_scan_scheduled_epoch: None,
            search: None,
            rename: None,
            rename_session: None,
            save_profile_prompt: None,
            show_profile_picker: false,
            profile_picker: None,
            show_session_picker: false,
            session_picker: None,
            collaboration: None,
            follow_prompt: None,
            overlay_return: None,
            replaceable_toasts: HashMap::new(),
            session_picker_epoch: 0,
            profile_picker_epoch: 0,
            copy_mode: None,
            copy_feedback_target: None,
            copy_feedback_epoch: 0,
            hint_mode: None,
            scratch: Workspace::new(0),
            scratch_visible: false,
            scratch_return_focus: None,
            scratch_height: None,
            scratch_resize_start: None,
            popup: None,
            popup_return_focus: None,
            control_socket_path: None,
            event_hub: crate::events::EventHub::default(),
            agent_slot_streams: std::collections::HashMap::new(),
            attachment,
            background: HashMap::new(),
            launcher_seed: None,
            pending_session_action: None,
            pending_control_reply: None,
            pending_spawn_replies: HashMap::new(),
            pending_control_input: HashMap::new(),
            hosts: HostRegistry::default(),
            host_session_cache: crate::session::HostSessionCache::new(),
            pending_destructive: None,
            confirm_epoch: 0,
            next_parked_seq: 0,
            commands_dirty: false,
        }
    }

    pub fn from_profile(config: Config, theme: Theme, profile: crate::profiles::Profile) -> Self {
        crate::profiles::restore_state_from_profile(config, theme, profile)
    }

    /// The current session attachment.
    pub fn current(&self) -> &Attachment {
        &self.attachment
    }

    /// Whether this client is currently attending `pane_id` in its active attachment.
    pub fn is_pane_attended(&self, pane_id: PaneId) -> bool {
        self.window_focused && self.focused_pane() == Some(pane_id)
    }

    /// The current session's name, but only while it is a *local* session.
    ///
    /// Local discovery scans this machine's runtime directory and skips this name, so the attached
    /// session is not listed twice — it is re-added from live state, which knows more about it than
    /// a probe does. Session names are per-machine, though: `dev` on a remote host and `dev` here
    /// are unrelated sessions that merely share a spelling. Excluding by bare name while attached to
    /// the remote one would hide the local one, which is a session the user can still switch to.
    pub fn local_current_session_name(&self) -> Option<&str> {
        self.current()
            .remote_target
            .is_none()
            .then(|| self.current().session_name.as_deref())
            .flatten()
    }

    /// Mutable access to the [current attachment](Self::current).
    pub fn current_mut(&mut self) -> &mut Attachment {
        &mut self.attachment
    }

    pub fn popup_is_present(&self) -> bool {
        self.popup.is_some()
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
        parked.parked_seq = self.next_parked_seq();
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
        parked.parked_seq = self.next_parked_seq();
        self.background.insert(current_epoch, parked);
        Some(restored_epoch)
    }

    fn next_parked_seq(&mut self) -> u64 {
        self.next_parked_seq = self.next_parked_seq.wrapping_add(1);
        self.next_parked_seq
    }

    /// Parked sessions worth returning to, most recently used first.
    ///
    /// This is the candidate order for landing the client somewhere when the current session is
    /// taken away rather than left — killed, or its host disconnected. The caller walks the list and
    /// takes the first that still switches, so an unusable candidate falls through to the next
    /// instead of stranding the user.
    ///
    /// Attachments still mid-connect are skipped outright: they have nothing on screen to come back
    /// to, so landing on one trades an empty session for another empty session — which is the whole
    /// complaint about defaulting to a fresh ephemeral.
    pub fn parked_by_recency(&self) -> Vec<AttachmentId> {
        let mut candidates: Vec<_> = self
            .background
            .iter()
            .filter(|(_, attachment)| attachment.pending_session_attach.is_none())
            .map(|(id, attachment)| (*id, attachment.parked_seq))
            .collect();
        candidates.sort_by_key(|(_, parked_seq)| std::cmp::Reverse(*parked_seq));
        candidates.into_iter().map(|(id, _)| id).collect()
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

    /// Whether the client currently has no session in the foreground: nothing attached, nothing
    /// connecting, and no panes to draw. This is the launcher — a legitimate resting state, not a
    /// failure — reached by dismissing the startup picker or by killing the last session. Parked
    /// sessions may still be live in the background; a launcher only says the *foreground* is
    /// empty.
    pub fn is_launcher(&self) -> bool {
        let current = self.current();
        current.session_name.is_none()
            && current.session_client.is_none()
            && current.pending_session_attach.is_none()
            && !current
                .workspaces
                .iter()
                .any(|workspace| !workspace.panes.is_empty())
    }

    /// Whether a new PTY spawn would have nowhere to run: no live client and no attach in flight.
    /// Mid-connect may still queue into `pending_spawns` (flushed on attach); this is the resting
    /// no-session case where those spawns would hang forever.
    pub fn needs_session_for_pty(&self) -> bool {
        self.current().session_client.is_none() && self.current().pending_session_attach.is_none()
    }

    /// Whether a pane's contents reach the screen on the next frame.
    ///
    /// `view::render` only builds panes from the active workspace, plus the scratch workspace and
    /// popup (which live outside the attachment workspace lists). Output for anything else — a build running on
    /// another workspace — changes state that nothing currently draws, so the frame it would cost
    /// is pure waste. Skipping the frame never loses content: the snapshot is still updated in
    /// place, and whatever makes the pane visible (workspace switch, scratchpad toggle) renders
    /// from the current snapshot.
    ///
    /// Deliberately conservative — the scratchpad counts as rendered even while hidden, because it
    /// animates in and out and a stale frame there is worse than a redundant one.
    pub fn pane_is_rendered(&self, id: PaneId) -> bool {
        if self.scratch.panes.iter().any(|pane| pane.id == id)
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
        if self.scratch_visible {
            &self.scratch
        } else {
            self.current().active_workspace_ref()
        }
    }

    /// Mutable [active workspace](Self::active_workspace_ref).
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        if self.scratch_visible {
            &mut self.scratch
        } else {
            self.current_mut().active_workspace_mut()
        }
    }

    pub fn focused_pane(&self) -> Option<PaneId> {
        if self.scratch_visible {
            self.scratch.focused_pane
        } else {
            self.current().focused_pane
        }
    }

    /// Record the focused pane in whichever workspace is currently active.
    pub fn set_focused_pane(&mut self, id: Option<PaneId>) {
        if self.scratch_visible {
            self.scratch.focused_pane = id;
        } else {
            self.current_mut().focused_pane = id;
        }
    }

    /// Which workspace layout edits apply to right now. A pointer gesture records this at its
    /// start so a mid-gesture scratchpad toggle cannot redirect it onto the other workspace.
    pub fn layout_target(&self) -> LayoutTarget {
        if self.scratch_visible {
            LayoutTarget::Scratch
        } else {
            LayoutTarget::Workspace(self.current().active_workspace)
        }
    }

    pub fn workspace_for(&self, target: LayoutTarget) -> &Workspace {
        match target {
            LayoutTarget::Scratch => &self.scratch,
            LayoutTarget::Workspace(index) => &self.current().workspaces[index],
        }
    }

    pub fn workspace_for_mut(&mut self, target: LayoutTarget) -> &mut Workspace {
        match target {
            LayoutTarget::Scratch => &mut self.scratch,
            LayoutTarget::Workspace(index) => &mut self.current_mut().workspaces[index],
        }
    }

    /// Canvas-space rect the active workspace tiles inside: the scratchpad's deployed dropdown rect
    /// while it is up, otherwise the whole pane canvas. Every layout computation - placement,
    /// split resize, float clamping, drop targeting - reads its extent from here, which is what
    /// makes the scratchpad an ordinary workspace laid out in a smaller box.
    ///
    /// The *deployed* rect deliberately, not the sliding one: layout math must not follow the
    /// slide, or a drag started mid-animation would compute against a rect that is still moving.
    pub fn layout_bounds(&self, viewport: Rect) -> FloatRect {
        if self.scratch_visible {
            crate::scratchpad::deployed_rect(self, viewport)
        } else {
            self.canvas_bounds_from_terminal_viewport(viewport)
        }
    }

    /// Workbar inset for [`Self::layout_bounds`]. Zero for the scratchpad: its rect already sits
    /// inside the tile area, so insetting again would double the gap.
    pub fn layout_top_gap(&self) -> f32 {
        if self.scratch_visible {
            0.0
        } else {
            self.workspace_top_gap()
        }
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
        if self.scratch_visible {
            self.current()
                .shared
                .as_ref()
                .filter(|shared| shared.read_only)
                .map(|_| "Attached read-only")
        } else {
            self.current().pane_input_block_reason()
        }
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

    /// Per-axis gap between adjacent tiled panes for the selected border presentation.
    pub fn tile_gap(&self) -> TileGap {
        match self.config.pane.border_mode {
            PaneBorderMode::Separate => TileGap::DEFAULT,
            PaneBorderMode::Merged => TileGap {
                horizontal: -1.0,
                vertical: if self.config.pane.show_titles
                    && self.config.pane.titlebar.takes_outer_row()
                {
                    0.0
                } else {
                    -1.0
                },
            },
            PaneBorderMode::None => TileGap {
                horizontal: 0.0,
                vertical: 0.0,
            },
            PaneBorderMode::Dividers => TileGap {
                horizontal: 1.0,
                vertical: 1.0,
            },
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

    /// Layout columns currently reserved for the sidebar: its deployed width scaled by how far its
    /// slide has got.
    ///
    /// Animating the reservation is what makes the pane column *give up* the space rather than be
    /// shoved sideways out of it. Both of its edges stay where they belong the whole way - the near
    /// one travelling with the sidebar, the far one pinned to the far edge of the screen - because
    /// the column is genuinely resized, exactly like the tile beside a spawning pane.
    ///
    /// The price is the price every geometry animation in rozi pays: the panes resize as they move,
    /// so each PTY takes a `pty.resize` per debounce window for the length of the slide. Set
    /// `[animations] sidebar = false` to skip straight to the settled width.
    pub fn effective_sidebar_width(&self, terminal_viewport: Rect) -> u16 {
        let deployed = self.sidebar_slide_width(terminal_viewport);
        let reserved = (f32::from(deployed) * self.sidebar_slide.get().clamp(0.0, 1.0)).round();
        let reserved = reserved as u16;
        // A single column is all splitter handle with no panel behind it, and the splitter's own
        // minimum would quietly hand it a second one - leaving the pane column a column narrower
        // than this says, with its far border pushed off the screen. Skip the width that cannot be
        // honoured rather than report one that is wrong.
        if reserved <= 1 { 0 } else { reserved }
    }

    /// The width the sidebar occupies once deployed, whether or not it is currently visible or
    /// settled.
    ///
    /// The panel is always drawn at this width and clipped, never squeezed into the part of it the
    /// layout has reserved so far - squeezing would reflow its tabs and rows on every frame.
    pub fn sidebar_slide_width(&self, terminal_viewport: Rect) -> u16 {
        crate::geometry::effective_sidebar_width(
            terminal_viewport.w,
            self.sidebar_requested_width(),
            true,
        )
    }

    pub fn sidebar_requested_width(&self) -> u16 {
        self.sidebar
            .width_preview
            .unwrap_or(self.config.sidebar.width)
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
    use crate::config::Config;
    use tui_lipan::prelude::Theme;

    #[test]
    fn fresh_workspaces_adopt_the_configured_default_layout() {
        let mut config = Config::default();
        config.layout.default = LayoutKind::Master;
        let attachment = fresh_default_attachment(&config);
        assert!(
            attachment
                .workspaces
                .iter()
                .all(|workspace| workspace.layout_kind == LayoutKind::Master),
            "every fresh workspace should start in the configured default layout",
        );
    }

    fn state_with_two_workspaces() -> State {
        let mut state = State::new(Config::default(), Theme::default());
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
        state.scratch.panes.push(Pane::new(7, 100, rect));
        state.popup = Some(Pane::new(8, 100, rect));
        // Both animate in and out, so they count as rendered even while hidden: a stale frame
        // there is worse than a redundant one.
        assert!(state.pane_is_rendered(7));
        assert!(state.pane_is_rendered(8));
    }

    #[test]
    fn visible_scratch_workspace_does_not_replace_attachment_workspace_state() {
        let mut state = state_with_two_workspaces();
        state
            .scratch
            .panes
            .push(Pane::new(7, 100, FloatRect::default()));
        state.scratch.focused_pane = Some(7);
        state.scratch_visible = true;

        assert_eq!(state.active_workspace_ref().focused_pane, Some(7));
        assert_eq!(state.focused_pane(), Some(7));
        assert_eq!(state.current().active_workspace, 0);
        assert_eq!(state.current().focused_pane, Some(1));
        assert_eq!(state.current().workspaces[0].panes[0].id, 1);
    }

    #[test]
    fn an_unknown_pane_is_not_rendered() {
        let state = state_with_two_workspaces();
        assert!(!state.pane_is_rendered(999));
    }

    #[test]
    fn pane_attendance_requires_focused_window_and_pane() {
        let mut state = State::new(Config::default(), Theme::default());
        let pane_id = state.current().focused_pane.expect("fresh pane focus");

        assert!(state.is_pane_attended(pane_id));

        state.current_mut().focused_pane = None;
        assert!(!state.is_pane_attended(pane_id));

        state.current_mut().focused_pane = Some(pane_id);
        state.window_focused = false;
        assert!(!state.is_pane_attended(pane_id));

        state.current_mut().focused_pane = None;
        assert!(!state.is_pane_attended(pane_id));
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use crate::config::Config;
    use tui_lipan::prelude::Theme;

    fn state() -> State {
        State::new(Config::default(), Theme::default())
    }

    #[test]
    fn park_and_unpark_round_trips_the_current_attachment() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".to_string());
        state.current_mut().session_attached = true;
        let rect = state.current().workspaces[0].panes[0].floating_rect;
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(2, 100, rect));
        state.current_mut().workspaces[0].focused_pane = Some(2);
        state.current_mut().focused_pane = Some(2);
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
        assert_eq!(state.current().focused_pane, Some(2));
        assert_eq!(state.current().workspaces[0].focused_pane, Some(2));
        assert!(state.background.contains_key(&6));
        assert!(!state.background.contains_key(&5));
    }

    /// The local scan skips the current session so it is not listed twice, but session names are
    /// per-machine: `dev` on a remote host and `dev` here are unrelated. Attached to the remote one,
    /// the local `dev` is still a session the user can switch to and must not be filtered out.
    #[test]
    fn only_a_local_current_session_is_excluded_from_the_local_scan() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".to_string());
        assert_eq!(state.local_current_session_name(), Some("dev"));

        state.current_mut().remote_target = Some(crate::session::remote::RemoteTarget::Alias(
            "winvm".to_string(),
        ));
        assert_eq!(
            state.local_current_session_name(),
            None,
            "attached to `dev` on a host, the local `dev` stays listed"
        );
    }

    /// Landing candidates are ordered by when they were last used, not by id — parking reuses an
    /// attachment's id, so ids say nothing about recency. Mid-connect attachments are skipped: they
    /// have nothing on screen, which is the very thing that makes a fresh ephemeral a bad landing.
    #[test]
    fn parked_candidates_are_ordered_by_recency_and_skip_mid_connect_ones() {
        let mut state = state();
        for (epoch, name) in [(10, "alpha"), (11, "beta"), (12, "gamma")] {
            state.current_mut().session_name = Some(name.to_string());
            state.park_current(epoch, Attachment::new());
        }
        // Most recently parked first.
        assert_eq!(state.parked_by_recency(), vec![12, 11, 10]);

        // Returning to `beta` and parking again makes it the most recent, without its id changing.
        state.runtime_epoch = 20;
        assert_eq!(state.unpark(11, state.runtime_epoch), Some(11));
        state.park_current(11, Attachment::new());
        assert_eq!(state.parked_by_recency().first(), Some(&11));

        // A candidate still mid-connect is no landing spot at all.
        state
            .background
            .get_mut(&12)
            .unwrap()
            .pending_session_attach = Some(crate::state::PendingSessionAttach {
            epoch: 12,
            name: "gamma".to_string(),
            client: None,
            autostart: false,
            read_only: false,
            reconnect: false,
            remote_host: None,
            intent: crate::state::AttachIntent::Plain,
            left: None,
            parked_epoch: None,
        });
        assert!(!state.parked_by_recency().contains(&12));
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
