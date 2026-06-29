use std::cell::Cell;
use std::path::PathBuf;
use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::anim::{GeometryAnimation, WindowAnimationConfig};
use crate::pane::TerminalPane;
use crate::tiling::{DwindleTree, append_tiled_window, collect_tree_leaves};

pub type PaneId = u32;

pub const WORKSPACE_COUNT: usize = 9;
/// Reserved id for the scratchpad pane. Workspace panes start at 1 (see `State::new`), so 0
/// can never collide with an allocated `next_pane_id`.
pub const SCRATCH_PANE_ID: PaneId = 0;
pub const TOP_BAR_HEIGHT: u16 = 1;
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
    Spiral,
    Monocle,
}

impl LayoutKind {
    /// Every layout in cycle order. `toggled` walks this list, so the order here
    /// is the order `Action::ToggleLayout` rotates through.
    pub fn all() -> &'static [LayoutKind] {
        &[
            Self::Dwindle,
            Self::Master,
            Self::Grid,
            Self::Spiral,
            Self::Monocle,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Grid => "grid",
            Self::Spiral => "spiral",
            Self::Monocle => "monocle",
        }
    }

    pub fn toggled(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|kind| *kind == self).unwrap_or(0);
        all[(index + 1) % all.len()]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveSwapHint {
    pub pane: PaneId,
    pub return_direction: Direction,
    pub target: PaneId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prefix,
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
pub enum WmModifier {
    Super,
    Alt,
}

impl WmModifier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Super => "Super",
            Self::Alt => "Alt",
        }
    }

    pub fn key_mods(self) -> KeyMods {
        match self {
            Self::Super => KeyMods {
                super_key: true,
                ..KeyMods::NONE
            },
            Self::Alt => KeyMods::ALT,
        }
    }

    pub fn matches(self, key: KeyEvent) -> bool {
        match self {
            Self::Super => key.mods.super_key,
            Self::Alt => key.mods.alt,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputConfig {
    pub prefix: KeyBinding,
    pub modifier: WmModifier,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            prefix: KeyBinding::from_str("ctrl-a").expect("default prefix key parses"),
            modifier: WmModifier::Alt,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
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
    pub fn all() -> [Self; 9] {
        [
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

    pub fn index(self) -> usize {
        Self::all()
            .iter()
            .position(|preset| *preset == self)
            .unwrap_or(0)
    }

    pub fn id(self) -> &'static str {
        match self {
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

#[derive(Clone, Debug)]
pub struct HyprmuxThemeConfig {
    pub preset: ThemePreset,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxProfileConfig {
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxSessionConfig {
    /// Persist the live layout on quit and restore it on next launch.
    pub autosave: bool,
    /// Override the session file location; defaults to `$XDG_STATE_HOME/hyprmux/session.toml`.
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxPaneConfig {
    /// Whether the focused pane uses the theme's panel background instead of the normal
    /// workspace backdrop. Disabled by default so hover focus does not repaint terminal bg.
    pub highlight_focused_background: bool,
    /// Whether moving the mouse over a pane focuses it.
    pub focus_on_hover: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxClipboardConfig {
    pub enable_osc52: bool,
}

/// Default fraction of the viewport height the dropdown scratchpad occupies.
pub const SCRATCHPAD_DEFAULT_HEIGHT: f32 = 0.4;
pub const SCRATCHPAD_MIN_HEIGHT: f32 = 0.1;
pub const SCRATCHPAD_MAX_HEIGHT: f32 = 0.9;

#[derive(Clone, Debug)]
pub struct HyprmuxScratchpadConfig {
    /// Command to run instead of the normal shell (e.g. `btop`); `None` uses the shell.
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// Height as a fraction of the viewport (clamped to `0.1..=0.9`).
    pub height: f32,
}

impl Default for HyprmuxScratchpadConfig {
    fn default() -> Self {
        Self {
            command: None,
            cwd: None,
            height: SCRATCHPAD_DEFAULT_HEIGHT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HyprmuxConfig {
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub scrollback: usize,
    pub input: InputConfig,
    pub animations: WindowAnimationConfig,
    pub theme: HyprmuxThemeConfig,
    pub profile: HyprmuxProfileConfig,
    pub session: HyprmuxSessionConfig,
    pub pane: HyprmuxPaneConfig,
    pub clipboard: HyprmuxClipboardConfig,
    pub scratchpad: HyprmuxScratchpadConfig,
    pub bar: BarConfig,
    pub keymap: crate::keymap::Keymap,
}

impl Default for HyprmuxConfig {
    fn default() -> Self {
        Self {
            shell: None,
            cwd: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
            scrollback: 5000,
            input: InputConfig::default(),
            animations: WindowAnimationConfig::default(),
            theme: HyprmuxThemeConfig::default(),
            profile: HyprmuxProfileConfig::default(),
            session: HyprmuxSessionConfig::default(),
            pane: HyprmuxPaneConfig::default(),
            clipboard: HyprmuxClipboardConfig::default(),
            scratchpad: HyprmuxScratchpadConfig::default(),
            bar: BarConfig::default(),
            keymap: crate::keymap::Keymap::default(),
        }
    }
}

/// One segment of the configurable top bar. `Workspaces` is the workspace tab strip;
/// `Text` is a literal with `{host}`/`{workspace}`/`{layout}`/`{session}` placeholders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarSegment {
    Title,
    Workspaces,
    Session,
    Clock,
    Layout,
    Text(String),
}

impl BarSegment {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(literal) = value.strip_prefix("text:") {
            return Some(Self::Text(literal.to_string()));
        }
        match value.to_ascii_lowercase().as_str() {
            "title" => Some(Self::Title),
            "workspaces" => Some(Self::Workspaces),
            "session" => Some(Self::Session),
            "clock" => Some(Self::Clock),
            "layout" => Some(Self::Layout),
            _ => None,
        }
    }

    pub fn is_clock(&self) -> bool {
        matches!(self, Self::Clock)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarConfig {
    pub left: Vec<BarSegment>,
    pub right: Vec<BarSegment>,
    pub clock_format: String,
}

impl Default for BarConfig {
    fn default() -> Self {
        // Matches today's bar: the badge then the workspace tabs, nothing on the right.
        Self {
            left: vec![BarSegment::Title, BarSegment::Workspaces],
            right: Vec::new(),
            clock_format: "%H:%M".to_string(),
        }
    }
}

impl BarConfig {
    pub fn has_clock(&self) -> bool {
        self.left
            .iter()
            .chain(self.right.iter())
            .any(BarSegment::is_clock)
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
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneIdentity {
    pub custom_title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
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

pub struct Pane {
    pub id: PaneId,
    pub title: String,
    pub identity: PaneIdentity,
    pub floating: bool,
    pub fullscreen: bool,
    pub floating_rect: FloatRect,
    pub opening: bool,
    pub closing: bool,
    pub terminal: TerminalPane,
}

impl Pane {
    pub fn new(id: PaneId, scrollback: usize, floating_rect: FloatRect) -> Self {
        Self {
            id,
            title: "shell".to_string(),
            identity: PaneIdentity::default(),
            floating: false,
            fullscreen: false,
            floating_rect,
            opening: true,
            closing: false,
            terminal: TerminalPane::new(scrollback),
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

    /// The shell's current working directory if it can be discovered live, else `None`.
    pub fn live_cwd(&self) -> Option<String> {
        self.terminal.working_directory()
    }
}

pub struct Workspace {
    pub panes: Vec<Pane>,
    pub tile_tree: Option<DwindleTree>,
    pub focused_pane: Option<PaneId>,
    pub layout_kind: LayoutKind,
    pub start_axis: SplitAxis,
    pub split_ratios: Vec<f32>,
    pub last_move_swap: Option<MoveSwapHint>,
}

impl Workspace {
    pub fn new(index: usize) -> Self {
        Self {
            panes: Vec::new(),
            tile_tree: None,
            focused_pane: None,
            layout_kind: LayoutKind::Dwindle,
            start_axis: if index % 2 == 0 {
                SplitAxis::Horizontal
            } else {
                SplitAxis::Vertical
            },
            split_ratios: vec![DEFAULT_RATIO; 16],
            last_move_swap: None,
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
    pub mode: Mode,
    pub moving_pane: Option<MoveSession>,
    pub resizing_pane: Option<ResizeSession>,
    pub animation: GeometryAnimation,
    pub last_viewport: Cell<Option<Rect>>,
    pub show_palette: bool,
    pub show_help: bool,
    pub show_titles: bool,
    pub show_theme_picker: bool,
    pub theme_picker_preview: Option<ThemePickerPreview>,
    pub theme: Theme,
    pub theme_watcher: Option<ThemeWatcher>,
    pub search: Option<ScrollbackSearchState>,
    pub rename: Option<PaneRenameState>,
    pub copy_mode: Option<CopyModeState>,
    pub scratch: Option<Pane>,
    pub scratch_visible: bool,
    /// Focus to restore when the scratchpad is hidden again.
    pub scratch_return_focus: Option<PaneId>,
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
            mode: Mode::Normal,
            moving_pane: None,
            resizing_pane: None,
            animation: GeometryAnimation::None,
            last_viewport: Cell::new(None),
            show_palette: false,
            show_help: false,
            show_titles: true,
            show_theme_picker: false,
            theme_picker_preview: None,
            theme,
            theme_watcher: None,
            search: None,
            rename: None,
            copy_mode: None,
            scratch: None,
            scratch_visible: false,
            scratch_return_focus: None,
        }
    }

    pub fn from_profile(
        config: HyprmuxConfig,
        theme: Theme,
        profile: crate::profiles::HyprmuxProfile,
    ) -> Self {
        crate::profiles::restore_state_from_profile(config, theme, profile)
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
    fn bar_segment_parses_builtins_and_text_literals() {
        assert_eq!(BarSegment::parse("clock"), Some(BarSegment::Clock));
        assert_eq!(
            BarSegment::parse("Workspaces"),
            Some(BarSegment::Workspaces)
        );
        assert_eq!(
            BarSegment::parse("text:hi {host}"),
            Some(BarSegment::Text("hi {host}".to_string()))
        );
        assert_eq!(BarSegment::parse("bogus"), None);
    }

    #[test]
    fn bar_config_default_matches_current_layout() {
        let bar = BarConfig::default();
        assert_eq!(bar.left, vec![BarSegment::Title, BarSegment::Workspaces]);
        assert!(bar.right.is_empty());
        assert!(!bar.has_clock());
        assert_eq!(bar.clock_format, "%H:%M");
    }

    #[test]
    fn layout_kind_cycles_through_every_layout() {
        assert_eq!(LayoutKind::Dwindle.toggled(), LayoutKind::Master);
        assert_eq!(LayoutKind::Master.toggled(), LayoutKind::Grid);
        assert_eq!(LayoutKind::Grid.toggled(), LayoutKind::Spiral);
        assert_eq!(LayoutKind::Spiral.toggled(), LayoutKind::Monocle);
        assert_eq!(LayoutKind::Monocle.toggled(), LayoutKind::Dwindle);
        assert_eq!(LayoutKind::all().len(), 5);
    }

    #[test]
    fn layout_kind_labels_are_distinct() {
        let labels: Vec<&str> = LayoutKind::all().iter().map(|k| k.label()).collect();
        assert_eq!(labels, ["dwindle", "master", "grid", "spiral", "monocle"]);
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
}
