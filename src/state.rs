use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::config::{HyprmuxConfig, ProfileEntry};
use crate::pane::TerminalPane;
use crate::session::discovery::DiscoveredSession;
use crate::tiling::{DwindleTree, append_tiled_window, collect_tree_leaves};

pub type PaneId = u32;

pub const WORKSPACE_COUNT: usize = 9;
/// Reserved id for the scratchpad pane. Workspace panes start at 1 (see `State::new`), so 0
/// can never collide with an allocated `next_pane_id`.
pub const SCRATCH_PANE_ID: PaneId = 0;
pub const WORKBAR_HEIGHT: u16 = 1;
pub const TILE_GAP: f32 = 1.0;
pub const OUTER_GAP: f32 = 1.0;
pub const OFFSCREEN_MIN_VISIBLE: f32 = 6.0;
pub const DEFAULT_RATIO: f32 = 0.58;
pub const MIN_SPLIT_RATIO: f32 = 0.20;
pub const MAX_SPLIT_RATIO: f32 = 0.80;
pub const RATIO_STEP: f32 = 0.04;
/// Weights tile width against height when choosing a dwindle split direction (Hyprland's
/// `split_width_multiplier`). 2.0 corrects for terminal cells being ~twice as tall as wide.
pub const SPLIT_WIDTH_MULTIPLIER: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

impl SplitAxis {
    pub fn flipped(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    pub fn at_depth(self, depth: usize) -> Self {
        if depth % 2 == 0 { self } else { self.flipped() }
    }
}

/// Per-axis gap between tiled panes. Split apart because the two axes differ: left|right splits
/// carry a visible column gap, while top|bottom splits sit flush (their titlebars separate
/// stacked panes). Border merging drives both negative (a one-cell overlap so shared borders
/// fuse), except the vertical overlap is suppressed when titlebars are shown - otherwise a lower
/// pane's title row would land on the border of the pane above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGap {
    pub horizontal: f32,
    pub vertical: f32,
}

impl TileGap {
    /// The default un-merged gaps: a column between left|right splits, none between stacked panes.
    pub const DEFAULT: TileGap = TileGap {
        horizontal: TILE_GAP,
        vertical: 0.0,
    };

    pub fn for_axis(self, axis: SplitAxis) -> f32 {
        match axis {
            SplitAxis::Horizontal => self.horizontal,
            SplitAxis::Vertical => self.vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Dwindle,
    Master,
    Grid,
    Monocle,
}

impl LayoutKind {
    /// Every layout in cycle order. `toggled` walks this list, so the order here
    /// is the order `Action::ToggleLayout` rotates through.
    pub fn all() -> &'static [LayoutKind] {
        &[Self::Dwindle, Self::Master, Self::Grid, Self::Monocle]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Grid => "grid",
            Self::Monocle => "monocle",
        }
    }

    pub fn toggled(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|kind| *kind == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }
}

/// The border glyphs tiled panes draw. A single app-wide setting (`Action::CycleBorderStyle`),
/// not per-pane. Floating panes keep their own `Double` border so they stay visually distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneBorderStyle {
    Rounded,
    Plain,
    Double,
    Thick,
}

impl PaneBorderStyle {
    /// Cycle order for `Action::CycleBorderStyle`.
    pub fn all() -> &'static [PaneBorderStyle] {
        &[Self::Rounded, Self::Plain, Self::Double, Self::Thick]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Plain => "plain",
            Self::Double => "double",
            Self::Thick => "thick",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rounded => "Rounded",
            Self::Plain => "Plain",
            Self::Double => "Double",
            Self::Thick => "Thick",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "rounded" | "round" => Some(Self::Rounded),
            "plain" | "single" | "square" => Some(Self::Plain),
            "double" => Some(Self::Double),
            "thick" | "heavy" | "bold" => Some(Self::Thick),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    pub fn to_border_style(self) -> BorderStyle {
        match self {
            Self::Rounded => BorderStyle::Rounded,
            Self::Plain => BorderStyle::Plain,
            Self::Double => BorderStyle::Double,
            Self::Thick => BorderStyle::Thick,
        }
    }
}

/// End-cap glyphs for a colored chip drawn over a background (pane titlebars via
/// `Action::CycleTitleStyle`, workbar badges via `Action::CycleWorkbarBadgeStyle`). `Padded`
/// keeps a flush bar with blank side padding; the others draw the chip's ends in the chip color
/// over whatever is behind it, so it reads as a rounded/pointed pill. The cap glyphs (except
/// `Half`) are powerline separators and need a patched/Nerd font, like the titlebar's mode icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapStyle {
    Padded,
    Half,
    Round,
    Arrow,
}

impl CapStyle {
    /// Cycle order for `Action::CycleTitleStyle`.
    pub fn all() -> &'static [CapStyle] {
        &[Self::Padded, Self::Half, Self::Round, Self::Arrow]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Padded => "padded",
            Self::Half => "half",
            Self::Round => "round",
            Self::Arrow => "arrow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Padded => "Padded",
            Self::Half => "Half block",
            Self::Round => "Round",
            Self::Arrow => "Arrow",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "padded" | "pad" | "plain" | "none" => Some(Self::Padded),
            "half" | "half-block" | "block" => Some(Self::Half),
            "round" | "rounded" | "pill" => Some(Self::Round),
            "arrow" | "pointed" | "slant" | "powerline" => Some(Self::Arrow),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    /// Cycle order for `Action::CycleWorkbarBadgeStyle`—same as [`all`] except `Half` is excluded.
    pub fn badge_styles() -> &'static [CapStyle] {
        &[Self::Padded, Self::Round, Self::Arrow]
    }

    pub fn next_badge(self) -> Self {
        let all = Self::badge_styles();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    /// The (left, right) cap glyphs, or `None` for `Padded` (blank side padding, no glyphs). The
    /// caps paint in the titlebar color over the backdrop, so a left cap fills toward its right
    /// and a right cap toward its left.
    pub fn caps(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Padded => None,
            Self::Half => Some(("\u{2590}", "\u{258c}")),
            Self::Round => Some(("\u{e0b6}", "\u{e0b4}")),
            Self::Arrow => Some(("\u{e0b2}", "\u{e0b0}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeCorner {
    UpperLeft,
    UpperRight,
    LowerLeft,
    LowerRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeSession {
    pub id: PaneId,
    pub corner: ResizeCorner,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveSession {
    pub id: PaneId,
    pub was_floating: bool,
    pub drag_rect: FloatRect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitDragSession {
    pub kind: SplitDragKind,
    pub workspace: usize,
    pub start_x: u16,
    pub start_y: u16,
    pub start_tile_tree: Option<DwindleTree>,
    pub start_split_ratios: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDragKind {
    Single {
        pane_id: PaneId,
        horizontal_split: bool,
    },
    Junction {
        left_id: PaneId,
        top_id: PaneId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearanceAction {
    Theme,
    ToggleTitles,
    ToggleWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarPosition,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleBorderMerge,
    CycleBorderStyle,
    CycleTitleStyle,
    CycleWorkbarBadgeStyle,
}

impl AppearanceAction {
    /// Whether this row configures a feature that is currently switched off, so the row is inert:
    /// it still renders (greyed) but activating it does nothing. Keeps the appearance list stable
    /// instead of hiding dependent rows as their parent toggles.
    pub fn disabled_reason(self, pane: &crate::config::HyprmuxPaneConfig) -> Option<&'static str> {
        match self {
            Self::CycleTitleStyle if !pane.show_titles => Some("Needs titlebar"),
            Self::ToggleWorkbarGap | Self::ToggleWorkbarPosition | Self::CycleWorkbarBadgeStyle
                if !pane.show_workbar =>
            {
                Some("Needs workbar")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveSwapHint {
    pub pane: PaneId,
    pub return_direction: Direction,
    pub target: PaneId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Resize,
    Copy,
}

/// State for keyboard copy mode: a cursor and optional selection anchor in the target
/// pane's snapshot grid (viewport coordinates, which already reflect `offset`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyModeState {
    pub target: PaneId,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Selection start, or `None` until the user presses `v`/`Space`.
    pub anchor: Option<(usize, usize)>,
    /// Scrollback offset the pane is parked at while in copy mode.
    pub offset: usize,
}

impl CopyModeState {
    pub fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        self.anchor
            .map(|anchor| (anchor, (self.cursor_row, self.cursor_col)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyFlashState {
    pub id: u64,
    pub target: PaneId,
    pub selection: ((usize, usize), (usize, usize)),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    Lipan,
    OneDark,
    Dracula,
    Nord,
    Gruvbox,
    Catppuccin,
    TokyoNight,
    SolarizedDark,
    Monokai,
    Ansi,
}

impl ThemePreset {
    pub fn all() -> [Self; 10] {
        [
            Self::Lipan,
            Self::OneDark,
            Self::Dracula,
            Self::Nord,
            Self::Gruvbox,
            Self::Catppuccin,
            Self::TokyoNight,
            Self::SolarizedDark,
            Self::Monokai,
            Self::Ansi,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Lipan => "lipan",
            Self::OneDark => "one-dark",
            Self::Dracula => "dracula",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
            Self::Catppuccin => "catppuccin",
            Self::TokyoNight => "tokyo-night",
            Self::SolarizedDark => "solarized-dark",
            Self::Monokai => "monokai",
            Self::Ansi => "ansi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lipan => "Lipan",
            Self::OneDark => "One Dark",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox",
            Self::Catppuccin => "Catppuccin",
            Self::TokyoNight => "Tokyo Night",
            Self::SolarizedDark => "Solarized Dark",
            Self::Monokai => "Monokai",
            Self::Ansi => "ANSI",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "lipan" | "tui-lipan" | "tuilipan" | "default" => Some(Self::Lipan),
            "one-dark" | "onedark" => Some(Self::OneDark),
            "dracula" => Some(Self::Dracula),
            "nord" => Some(Self::Nord),
            "gruvbox" => Some(Self::Gruvbox),
            "catppuccin" => Some(Self::Catppuccin),
            "tokyo-night" | "tokyonight" => Some(Self::TokyoNight),
            "solarized-dark" | "solarized" => Some(Self::SolarizedDark),
            "monokai" => Some(Self::Monokai),
            "ansi" => Some(Self::Ansi),
            _ => None,
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            Self::Lipan => Theme::lipan(),
            Self::OneDark => Theme::one_dark(),
            Self::Dracula => Theme::dracula(),
            Self::Nord => Theme::nord(),
            Self::Gruvbox => Theme::gruvbox(),
            Self::Catppuccin => Theme::catppuccin(),
            Self::TokyoNight => Theme::tokyo_night(),
            Self::SolarizedDark => Theme::solarized_dark(),
            Self::Monokai => Theme::monokai(),
            Self::Ansi => Theme::ansi(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
    pub pane: PaneId,
}

/// Which panes a scrollback search scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    FocusedPane,
    Workspace,
    All,
}

impl SearchScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::FocusedPane => "pane",
            Self::Workspace => "workspace",
            Self::All => "all panes",
        }
    }

    /// Cycle pane → workspace → all → pane (bound to `Tab` in the search overlay).
    pub fn cycled(self) -> Self {
        match self {
            Self::FocusedPane => Self::Workspace,
            Self::Workspace => Self::All,
            Self::All => Self::FocusedPane,
        }
    }
}

pub struct ScrollbackSearchState {
    pub target: PaneId,
    pub scope: SearchScope,
    pub input: TextInput,
    pub matches: Vec<ScrollbackMatch>,
    pub current: usize,
    pub status: String,
    pub pending_server_requests: Vec<u64>,
}

impl ScrollbackSearchState {
    pub fn new(target: PaneId) -> Self {
        Self {
            target,
            scope: SearchScope::FocusedPane,
            input: TextInput::new(""),
            matches: Vec::new(),
            current: 0,
            status: "Type to search scrollback".to_string(),
            pending_server_requests: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneIdentity {
    pub custom_title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub keep_open: bool,
}

impl PaneIdentity {
    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        let title = title.as_ref().trim();
        if title.is_empty() {
            self.custom_title = None;
            self.profile_name = None;
        } else {
            self.custom_title = Some(title.to_string());
        }
    }
}

pub struct PaneRenameState {
    pub target: PaneId,
    pub input: TextInput,
}

impl PaneRenameState {
    pub fn new(target: PaneId, initial: impl AsRef<str>) -> Self {
        Self {
            target,
            input: TextInput::new(initial.as_ref()),
        }
    }
}

/// Rename prompt state for a workspace, keyed by its index rather than a `PaneId`.
pub struct WorkspaceRenameState {
    pub target: usize,
    pub input: TextInput,
}

impl WorkspaceRenameState {
    pub fn new(target: usize, initial: impl AsRef<str>) -> Self {
        Self {
            target,
            input: TextInput::new(initial.as_ref()),
        }
    }
}

pub struct ProfilePickerState {
    pub entries: Vec<ProfileEntry>,
    pub input: TextInput,
    /// Index into [`Self::entries`] for the highlighted profile.
    pub selected: usize,
    /// Entry index awaiting a second Ctrl+D to confirm deletion.
    pub pending_delete: Option<usize>,
}

pub struct SessionPickerState {
    pub entries: Vec<DiscoveredSession>,
    pub input: TextInput,
    pub selected: usize,
    pub pending_kill: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingDestructive {
    ClosePane(PaneId),
    KillWorkspace(usize),
    KillSession,
}

pub struct PendingDestructiveConfirmation {
    pub action: PendingDestructive,
    pub armed_at: Instant,
    pub toast_id: OverlayId,
}

impl SessionPickerState {
    pub fn new(entries: Vec<DiscoveredSession>) -> Self {
        Self {
            entries,
            input: TextInput::new(""),
            selected: 0,
            pending_kill: None,
        }
    }
}

impl ProfilePickerState {
    pub fn new(entries: Vec<ProfileEntry>) -> Self {
        Self {
            entries,
            input: TextInput::new(""),
            selected: 0,
            pending_delete: None,
        }
    }
}

pub struct Pane {
    pub id: PaneId,
    pub pty_generation: u64,
    pub title: String,
    pub identity: PaneIdentity,
    pub floating: bool,
    pub fullscreen: bool,
    pub floating_rect: FloatRect,
    pub opening: bool,
    pub terminal_active: bool,
    pub closing: bool,
    pub activity: PaneActivity,
    pub terminal: TerminalPane,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneActivity {
    pub last_activity: Option<Instant>,
    pub has_unseen_output: bool,
}

impl Pane {
    pub fn new(id: PaneId, scrollback: usize, floating_rect: FloatRect) -> Self {
        Self {
            id,
            pty_generation: 0,
            title: "shell".to_string(),
            identity: PaneIdentity::default(),
            floating: false,
            fullscreen: false,
            floating_rect,
            opening: true,
            terminal_active: false,
            closing: false,
            activity: PaneActivity::default(),
            terminal: {
                let mut terminal = TerminalPane::new(scrollback);
                terminal.bind_session(id, 0);
                terminal
            },
        }
    }

    pub fn display_title(&self, terminal_title: Option<String>) -> String {
        self.identity
            .custom_title
            .clone()
            .or(terminal_title)
            .unwrap_or_else(|| self.title.clone())
    }

    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        self.identity.set_custom_title(title);
    }

    pub fn clear_custom_title(&mut self) {
        self.identity.custom_title = None;
    }

    pub fn subtitle(&self) -> Option<&str> {
        self.identity
            .command
            .as_deref()
            .or(self.identity.cwd.as_deref())
    }

    pub fn subtitle_for_title(&self, title: &str) -> Option<&str> {
        if let Some(command) = self.identity.command.as_deref() {
            return Some(command);
        }

        let cwd = self.identity.cwd.as_deref()?;
        if title_contains_cwd(title, cwd) {
            None
        } else {
            Some(cwd)
        }
    }

    /// The shell's current working directory if it can be discovered live, else `None`.
    pub fn live_cwd(&self) -> Option<String> {
        self.terminal.working_directory()
    }
}

fn title_contains_cwd(title: &str, cwd: &str) -> bool {
    if cwd.is_empty() || title.contains(cwd) {
        return !cwd.is_empty();
    }

    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = home.trim_end_matches('/');
    if home.is_empty() || !cwd.starts_with(home) {
        return false;
    }

    let rest = cwd[home.len()..].trim_start_matches('/');
    let tilde_cwd = if rest.is_empty() {
        "~".to_string()
    } else {
        format!("~/{rest}")
    };
    title.contains(&tilde_cwd)
}

pub struct Workspace {
    pub panes: Vec<Pane>,
    pub tile_tree: Option<DwindleTree>,
    pub focused_pane: Option<PaneId>,
    pub synchronized: bool,
    pub layout_kind: LayoutKind,
    pub start_axis: SplitAxis,
    pub split_ratios: Vec<f32>,
    pub last_move_swap: Option<MoveSwapHint>,
    /// User-assigned label shown in the workbar in place of (or alongside) the workspace
    /// number. `None` keeps the default numeric display.
    pub name: Option<String>,
}

impl Workspace {
    pub fn new(index: usize) -> Self {
        Self {
            panes: Vec::new(),
            tile_tree: None,
            focused_pane: None,
            synchronized: false,
            layout_kind: LayoutKind::Dwindle,
            start_axis: if index % 2 == 0 {
                SplitAxis::Horizontal
            } else {
                SplitAxis::Vertical
            },
            split_ratios: vec![DEFAULT_RATIO; 16],
            last_move_swap: None,
            name: None,
        }
    }

    pub fn visible_count(&self) -> usize {
        self.panes.iter().filter(|pane| !pane.closing).count()
    }

    pub fn tiled_ids(&self) -> Vec<PaneId> {
        let active = self.active_tiled_ids_by_pane_order();
        let mut ordered = Vec::new();
        if let Some(tree) = self.tile_tree.as_ref() {
            collect_tree_leaves(tree, &mut ordered);
            ordered.retain(|id| active.contains(id));
            for id in &active {
                if !ordered.contains(id) {
                    ordered.push(*id);
                }
            }
        }

        if ordered.is_empty() { active } else { ordered }
    }

    pub fn active_tiled_ids_by_pane_order(&self) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|pane| !pane.floating && !pane.closing)
            .map(|pane| pane.id)
            .collect()
    }
}

pub struct State {
    pub config: HyprmuxConfig,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub focused_pane: Option<PaneId>,
    pub next_pane_id: PaneId,
    pub next_pty_generation: u64,
    pub runtime_epoch: u64,
    pub mode: Mode,
    pub moving_pane: Option<MoveSession>,
    pub resizing_pane: Option<ResizeSession>,
    pub split_drag: Option<SplitDragSession>,
    pub animation: GeometryAnimation,
    pub last_viewport: Cell<Option<Rect>>,
    pub show_palette: bool,
    pub show_help: bool,
    pub show_appearance: bool,
    pub show_theme_picker: bool,
    pub theme_picker_preview: Option<ThemePickerPreview>,
    pub theme: Theme,
    pub system_theme: Option<Theme>,
    pub theme_watcher: Option<ThemeWatcher>,
    pub search: Option<ScrollbackSearchState>,
    pub rename: Option<PaneRenameState>,
    pub rename_workspace: Option<WorkspaceRenameState>,
    pub save_profile_prompt: Option<PaneRenameState>,
    pub show_profile_picker: bool,
    pub profile_picker: Option<ProfilePickerState>,
    pub show_session_picker: bool,
    pub session_picker: Option<SessionPickerState>,
    pub copy_mode: Option<CopyModeState>,
    pub copy_flash: Option<CopyFlashState>,
    pub next_copy_flash_id: u64,
    pub scratch: Option<Pane>,
    pub scratch_visible: bool,
    /// Focus to restore when the scratchpad is hidden again.
    pub scratch_return_focus: Option<PaneId>,
    pub control_socket_path: Option<PathBuf>,
    pub session_client: Option<crate::session::client::SessionClient>,
    pub session_name: Option<String>,
    pub session_attached: bool,
    pub pending_session_attach: Option<PendingSessionAttach>,
    /// A destructive action armed by its first press; the second press only fires while the arm
    /// time is within [`crate::exit_ops::CONFIRM_WINDOW_SECS`].
    pub pending_destructive: Option<PendingDestructiveConfirmation>,
    pub last_pushed_layout: Option<String>,
    /// Cached first-line stdout for each configured `BarSegment::Command`, keyed by the raw
    /// command string. Refreshed on a background timer per command; empty until the first run
    /// completes.
    pub bar_command_output: HashMap<String, String>,
    /// Commands that already have a background poller thread running (see
    /// [`crate::pane_lifecycle::spawn_bar_command_pollers`]). A config reload spawns pollers
    /// only for commands newly added by the reload, since existing pollers never stop.
    pub bar_commands_running: HashSet<String>,
    /// Set whenever something `crate::commands::sync` needs to see (shortcuts, dynamic labels,
    /// or the `commands_active` gate) may have changed. Checked once per message at the tail of
    /// `update::handle_msg` rather than resyncing unconditionally, since high-frequency messages
    /// (PTY output, keystrokes forwarded to a pane) never affect it.
    pub commands_dirty: bool,
}

pub struct PendingSessionAttach {
    pub epoch: u64,
    pub name: String,
    pub client: Option<crate::session::client::SessionClient>,
    pub migrate_local_panes: bool,
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
        workspaces[0]
            .panes
            .push(Pane::new(initial_id, config.scrollback, initial_rect));
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
            mode: Mode::Normal,
            moving_pane: None,
            resizing_pane: None,
            split_drag: None,
            animation: GeometryAnimation::None,
            last_viewport: Cell::new(None),
            show_palette: false,
            show_help: false,
            show_appearance: false,
            show_theme_picker: false,
            theme_picker_preview: None,
            theme,
            system_theme: None,
            theme_watcher: None,
            search: None,
            rename: None,
            rename_workspace: None,
            save_profile_prompt: None,
            show_profile_picker: false,
            profile_picker: None,
            show_session_picker: false,
            session_picker: None,
            copy_mode: None,
            copy_flash: None,
            next_copy_flash_id: 1,
            scratch: None,
            scratch_visible: false,
            scratch_return_focus: None,
            control_socket_path: None,
            session_client: None,
            session_name: None,
            session_attached: false,
            pending_session_attach: None,
            pending_destructive: None,
            last_pushed_layout: None,
            bar_command_output: HashMap::new(),
            bar_commands_running: HashSet::new(),
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

    /// Vertical space (in rows) the workbar removes from the panes area. Independent of whether
    /// the bar sits at the top or the bottom - either way it consumes the same one row.
    pub fn top_chrome_height(&self) -> u16 {
        if self.config.pane.show_workbar {
            WORKBAR_HEIGHT
        } else {
            0
        }
    }

    /// Row offset of the panes area from the top of the viewport: the workbar height when the bar
    /// sits above the panes, and 0 when it sits below them (the panes start at the first row and
    /// the bar is drawn on the last row). Used to translate between root and canvas-local space.
    pub fn content_top_offset(&self) -> u16 {
        if self.config.pane.show_workbar && !self.config.pane.workbar_at_bottom {
            WORKBAR_HEIGHT
        } else {
            0
        }
    }

    /// Signed inset (in cells) that keeps the panes clear of the workbar. Positive insets the top
    /// edge of the tile area (bar above the panes); negative insets the bottom edge (bar below the
    /// panes), so the gap always lands between the panes and the bar. Zero when there is no gap.
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

pub struct ThemePickerPreview {
    pub theme: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        )
    }

    #[test]
    fn layout_kind_cycles_through_every_layout() {
        assert_eq!(LayoutKind::Dwindle.toggled(), LayoutKind::Master);
        assert_eq!(LayoutKind::Master.toggled(), LayoutKind::Grid);
        assert_eq!(LayoutKind::Grid.toggled(), LayoutKind::Monocle);
        assert_eq!(LayoutKind::Monocle.toggled(), LayoutKind::Dwindle);
        assert_eq!(LayoutKind::all().len(), 4);
    }

    #[test]
    fn layout_kind_labels_are_distinct() {
        let labels: Vec<&str> = LayoutKind::all().iter().map(|k| k.label()).collect();
        assert_eq!(labels, ["dwindle", "master", "grid", "monocle"]);
    }

    #[test]
    fn pane_display_title_prefers_custom_title() {
        let mut pane = pane();
        pane.title = "terminal title".to_string();
        pane.set_custom_title("custom title");

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "custom title"
        );
    }

    #[test]
    fn pane_display_title_uses_terminal_title_before_fallback() {
        let mut pane = pane();
        pane.title = "fallback title".to_string();

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "terminal title"
        );
    }

    #[test]
    fn empty_custom_title_is_cleared() {
        let mut pane = pane();
        pane.set_custom_title("custom title");
        pane.set_custom_title("   ");

        assert_eq!(pane.identity.custom_title, None);
        assert_eq!(pane.display_title(None), "shell");
    }

    #[test]
    fn pane_subtitle_prefers_command_before_cwd() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());
        pane.identity.command = Some("vim src/main.rs".to_string());

        assert_eq!(pane.subtitle(), Some("vim src/main.rs"));
    }

    #[test]
    fn pane_subtitle_hides_cwd_already_in_terminal_title() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());

        assert_eq!(pane.subtitle_for_title("razuer@host:/tmp/project"), None);
    }

    #[test]
    fn pane_subtitle_hides_home_relative_cwd_in_terminal_title() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let home = home.trim_end_matches('/');
        if home.is_empty() {
            return;
        }

        let mut pane = pane();
        pane.identity.cwd = Some(format!("{home}/Work/Projects/opencode-tui"));

        assert_eq!(
            pane.subtitle_for_title("razuer@host:~/Work/Projects/opencode-tui"),
            None
        );
    }

    #[test]
    fn pane_subtitle_keeps_command_even_when_title_contains_cwd() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());
        pane.identity.command = Some("cargo run".to_string());

        assert_eq!(
            pane.subtitle_for_title("razuer@host:/tmp/project"),
            Some("cargo run")
        );
    }
}
