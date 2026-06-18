use std::cell::Cell;
use std::path::PathBuf;

use tui_lipan::prelude::*;

use crate::anim::{GeometryAnimation, WindowAnimationConfig};
use crate::pane::TerminalPane;
use crate::tiling::{DwindleTree, append_tiled_window, collect_tree_leaves};

pub type PaneId = u32;

pub const WORKSPACE_COUNT: usize = 9;
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
}

impl LayoutKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dwindle => Self::Master,
            Self::Master => Self::Dwindle,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prefix,
    Resize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputConfig {
    pub prefix: KeyEvent,
    pub modifier: WmModifier,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            prefix: KeyEvent {
                code: KeyCode::Char('a'),
                mods: KeyMods::CTRL,
            },
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

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxClipboardConfig {
    pub enable_osc52: bool,
}

#[derive(Clone, Debug)]
pub struct HyprmuxConfig {
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub scrollback: usize,
    pub input: InputConfig,
    pub animations: WindowAnimationConfig,
    pub theme: HyprmuxThemeConfig,
    pub clipboard: HyprmuxClipboardConfig,
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
            clipboard: HyprmuxClipboardConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackMatch {
    pub offset: usize,
    pub line: usize,
}

pub struct ScrollbackSearchState {
    pub target: PaneId,
    pub input: TextInput,
    pub matches: Vec<ScrollbackMatch>,
    pub current: usize,
    pub status: String,
}

impl ScrollbackSearchState {
    pub fn new(target: PaneId) -> Self {
        Self {
            target,
            input: TextInput::new(""),
            matches: Vec::new(),
            current: 0,
            status: "Type to search scrollback".to_string(),
        }
    }
}

pub struct Pane {
    pub id: PaneId,
    pub title: String,
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
            floating: false,
            fullscreen: false,
            floating_rect,
            opening: true,
            closing: false,
            terminal: TerminalPane::new(scrollback),
        }
    }
}

pub struct Workspace {
    pub panes: Vec<Pane>,
    pub tile_tree: Option<DwindleTree>,
    pub focused_pane: Option<PaneId>,
    pub layout_kind: LayoutKind,
    pub start_axis: SplitAxis,
    pub split_ratios: Vec<f32>,
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
    pub theme_picker_selected: usize,
    pub theme: Theme,
    pub theme_watcher: Option<ThemeWatcher>,
    pub search: Option<ScrollbackSearchState>,
}

impl State {
    pub fn new(config: HyprmuxConfig, theme: Theme) -> Self {
        let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
        let theme_picker_selected = config.theme.preset.index();
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
            theme_picker_selected,
            theme,
            theme_watcher: None,
            search: None,
        }
    }
}
