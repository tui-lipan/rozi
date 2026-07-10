use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::state::{CapStyle, PaneBorderStyle, ThemePreset};

// === Config schema ===

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

    /// Spelling of this modifier in tui-lipan `KeyBinding` strings (e.g. `alt-c`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Super => "super",
            Self::Alt => "alt",
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
    /// When true (the default), every built-in default key is also registered as a held
    /// WM-modifier chord (`<modifier>-<key>`, e.g. `Alt+q`) alongside its `<prefix> <key>`
    /// leader chord. Set to false to drop the modifier layer entirely, leaving prefix-only
    /// bindings so held `Alt`/`Super` chords pass straight through to the focused pane.
    pub modifier_shortcuts: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            prefix: KeyBinding::from_str("ctrl-a").expect("default prefix key parses"),
            modifier: WmModifier::Alt,
            modifier_shortcuts: true,
        }
    }
}

/// Expand one key step into its scheme-generated shortcuts: the leader chord
/// (`<prefix> <key>`) plus, when `modifier_shortcuts` is enabled, the held WM-modifier chord
/// (`<modifier>-<key>`). A step that fails to parse in either form is simply dropped, so a
/// malformed step yields an empty result rather than a panic.
pub fn scheme_shortcuts(input: &InputConfig, key: &str) -> Vec<KeyBinding> {
    let prefix = input.prefix.canonical_lowercase();
    let mut out = Vec::new();
    if let Ok(chord) = KeyBinding::from_str(&format!("{prefix} {key}")) {
        out.push(chord);
    }
    if input.modifier_shortcuts
        && let Ok(held) = KeyBinding::from_str(&format!("{}-{key}", input.modifier.token()))
    {
        out.push(held);
    }
    out
}

#[derive(Clone, Debug)]
pub struct HyprmuxThemeConfig {
    /// Name of the active theme: a built-in preset id, the reserved name `system` (derive
    /// colors from the host terminal), or the file stem of a custom theme in [`themes_dir`].
    /// A custom file shadows a built-in of the same name.
    pub name: String,
}

impl Default for HyprmuxThemeConfig {
    fn default() -> Self {
        Self {
            name: ThemePreset::Lipan.id().to_string(),
        }
    }
}

/// A selectable theme: a built-in preset, the host-derived system theme, or a named custom
/// theme file in the themes directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Builtin(ThemePreset),
    Custom { name: String, path: PathBuf },
}

impl ThemeChoice {
    /// Human-facing name shown in the theme picker.
    pub fn label(&self) -> String {
        match self {
            Self::System => "System".to_string(),
            Self::Builtin(preset) => preset.label().to_string(),
            Self::Custom { name, .. } => name.clone(),
        }
    }

    /// Config-facing id persisted as `[theme].name`.
    pub fn id(&self) -> String {
        match self {
            Self::System => "system".to_string(),
            Self::Builtin(preset) => preset.id().to_string(),
            Self::Custom { name, .. } => name.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxProfileConfig {
    /// Name of a profile in [`profiles_dir`] to load on startup when no CLI profile is given.
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileEntry {
    pub name: String,
    pub path: PathBuf,
}

/// What a bare launch (no `--attach`/`--session`) does before opening the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionStartup {
    /// Silently attach to this process's ephemeral session (the historical behavior).
    #[default]
    Ephemeral,
    /// Show the session picker first (when any named session exists), so the user can reattach to a
    /// named session or start a fresh ephemeral one. Equivalent to passing `--pick`.
    Picker,
}

impl SessionStartup {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ephemeral" | "default" | "attach" => Some(Self::Ephemeral),
            "picker" | "pick" | "choose" => Some(Self::Picker),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxSessionConfig {
    /// Persist the live layout on quit and restore it on next launch.
    pub autosave: bool,
    /// Override the session file location; defaults to `$XDG_STATE_HOME/hyprmux/session.toml`.
    pub path: Option<PathBuf>,
    /// Whether a bare launch attaches to an ephemeral session or opens the session picker first.
    pub startup: SessionStartup,
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxPaneConfig {
    /// Whether the focused pane uses the theme's panel background instead of the normal
    /// workspace backdrop. Disabled by default so hover focus does not repaint terminal bg.
    pub highlight_focused_background: bool,
    /// Whether the focused pane uses the theme's active border color instead of the normal border.
    pub highlight_focused_border: bool,
    /// Whether moving the mouse over a pane focuses it.
    pub focus_on_hover: bool,
    /// Whether the workbar (workspace tabs, mode chips, etc.) is shown.
    pub show_workbar: bool,
    /// Whether there is a 1-line gap between the workbar and the panes area.
    pub workbar_gap: bool,
    /// Whether the workbar is drawn on the last row (below the panes) instead of the first row.
    pub workbar_at_bottom: bool,
    /// Whether tiled/floating panes render their titlebars.
    pub show_titles: bool,
    /// Whether adjacent tiled panes overlap by a cell so their borders fuse into a shared seam
    /// instead of drawing a gap between separate boxes.
    pub merge_borders: bool,
    /// App-wide border glyphs for tiled panes.
    pub border_style: PaneBorderStyle,
    /// Blank cells inserted between a pane's border and its terminal grid, as
    /// `(top, right, bottom, left)`. Purely cosmetic: each cell of padding costs a column/row of
    /// usable terminal space, so this stays off by default. Painted with the pane's frame
    /// background. Configured with CSS-style shorthand (see [`PaddingSpec`]).
    pub padding: (u16, u16, u16, u16),
    /// App-wide end-cap style for pane titlebars.
    pub title_style: CapStyle,
    /// End-cap style for the workbar's colored badges (the title chip and mode chips).
    pub workbar_badge_style: CapStyle,
    /// Whether trailing workbar badges chain into a powerline (no gap between chips, each cap drawn
    /// over its left neighbor's color) instead of standing apart with a gap. Independent of
    /// `workbar_badge_style`, which only controls the pill shape.
    pub workbar_powerline: bool,
    /// End-cap style for workspace tabs in the workbar.
    pub workbar_tab_style: CapStyle,
    /// End-cap style for the workbar itself: the whole panel bar reads as a pill/point over the
    /// backdrop rather than a flush edge-to-edge bar.
    pub workbar_style: CapStyle,
}

impl Default for HyprmuxPaneConfig {
    fn default() -> Self {
        Self {
            highlight_focused_background: false,
            highlight_focused_border: true,
            focus_on_hover: true,
            show_workbar: true,
            workbar_gap: true,
            workbar_at_bottom: false,
            show_titles: true,
            merge_borders: false,
            border_style: PaneBorderStyle::Rounded,
            padding: (0, 0, 0, 0),
            title_style: CapStyle::Padded,
            workbar_badge_style: CapStyle::Padded,
            workbar_powerline: true,
            workbar_tab_style: CapStyle::Padded,
            workbar_style: CapStyle::Padded,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxClipboardConfig {
    pub enable_osc52: bool,
}

impl Default for HyprmuxClipboardConfig {
    fn default() -> Self {
        Self { enable_osc52: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyprmuxNotificationsConfig {
    pub enabled: bool,
    pub pane_exit: bool,
}

impl Default for HyprmuxNotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pane_exit: true,
        }
    }
}

/// Seamless-navigation policy for the `smart-focus-*` actions: the set of foreground programs
/// that manage their own splits and should receive `Ctrl-h/j/k/l` themselves instead of having
/// hyprmux move pane focus. Modeled on vim-tmux-navigator's `is_vim` check (see
/// [docs/keybindings.md]); matching is case-insensitive against the pane's foreground process
/// name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyprmuxNavigationConfig {
    pub editors: Vec<String>,
}

impl Default for HyprmuxNavigationConfig {
    fn default() -> Self {
        Self {
            editors: DEFAULT_SPLIT_EDITORS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        }
    }
}

impl HyprmuxNavigationConfig {
    /// Whether `command` (a pane's foreground process name) is a program that handles its own
    /// splits, so `smart-focus-*` should forward the navigation key to it rather than move focus.
    pub fn is_split_editor(&self, command: &str) -> bool {
        self.editors
            .iter()
            .any(|name| name.eq_ignore_ascii_case(command))
    }
}

/// Foreground programs that `smart-focus-*` forwards navigation keys to by default. Names match
/// `/proc/<pid>/comm` (the truncated executable basename), covering the common vim family plus a
/// few other split-aware TUIs; extend or replace via `[navigation] editors`.
const DEFAULT_SPLIT_EDITORS: &[&str] = &[
    "vim",
    "nvim",
    "vi",
    "view",
    "vimdiff",
    "nvim-wrapped",
    "hx",
    "helix",
    "kak",
    "emacs",
    "emacsclient",
    "fzf",
];

/// Which destructive actions require a confirming second press within the confirm window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyprmuxConfirmConfig {
    /// Confirm before closing a pane whose process is still running.
    pub close_pane: bool,
    /// Confirm before killing every pane on the active workspace.
    pub kill_workspace: bool,
    /// Confirm before shutting down the attached session server.
    pub kill_session: bool,
    /// Confirm before quitting an ephemeral session that still has a live pane (quitting it
    /// shuts the server down and kills those PTYs). Named-session quits are unaffected.
    pub quit_ephemeral: bool,
    /// Confirm before discarding the current ephemeral session to start a fresh one (its panes
    /// are killed). No effect on named sessions or when there is no live pane.
    pub new_temporary_session: bool,
}

impl Default for HyprmuxConfirmConfig {
    fn default() -> Self {
        Self {
            close_pane: false,
            kill_workspace: true,
            kill_session: true,
            quit_ephemeral: true,
            new_temporary_session: true,
        }
    }
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
    pub notifications: HyprmuxNotificationsConfig,
    pub navigation: HyprmuxNavigationConfig,
    pub confirm: HyprmuxConfirmConfig,
    pub scratchpad: HyprmuxScratchpadConfig,
    pub workbar: WorkbarConfig,
    /// Explicit `[keys]` overrides: command id -> native `KeyBinding` shortcuts. A command id
    /// present with an empty list is an explicit unbind; an id absent here uses the built-in
    /// defaults (see `crate::commands`).
    pub key_overrides: HashMap<String, Vec<KeyBinding>>,
    /// User-defined `[keys]` entries keyed by a literal trigger binding (rather than a built-in
    /// action id): each becomes its own generated command (see `crate::commands`).
    pub user_commands: Vec<UserCommand>,
}

/// What a user-defined keybinding does: `Run` spawns a new pane running the shell command;
/// `Send` writes literal text to the focused pane's PTY (TOML escapes like `\n` already work).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserCommandAction {
    Run(String),
    Send(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserCommand {
    pub action: UserCommandAction,
    pub binding: KeyBinding,
}

impl UserCommand {
    /// Human-facing description for the help overlay and command palette, since these have no
    /// static label of their own the way a built-in command does.
    pub fn label(&self) -> String {
        match &self.action {
            UserCommandAction::Run(command) => format!("Run: {}", truncate_for_label(command)),
            UserCommandAction::Send(text) => {
                format!("Send: {}", truncate_for_label(&escape_for_label(text)))
            }
        }
    }
}

/// Renders control characters visibly (e.g. a trailing `\n` reads as `\n`, not a line break)
/// so a `send` command's label stays on one line.
fn escape_for_label(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' => "\\n".to_string(),
            '\t' => "\\t".to_string(),
            '\r' => "\\r".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn truncate_for_label(text: &str) -> String {
    const MAX_LEN: usize = 40;
    if text.chars().count() <= MAX_LEN {
        text.to_string()
    } else {
        let mut truncated: String = text.chars().take(MAX_LEN).collect();
        truncated.push('…');
        truncated
    }
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
            notifications: HyprmuxNotificationsConfig::default(),
            navigation: HyprmuxNavigationConfig::default(),
            confirm: HyprmuxConfirmConfig::default(),
            scratchpad: HyprmuxScratchpadConfig::default(),
            workbar: WorkbarConfig::default(),
            key_overrides: HashMap::new(),
            user_commands: Vec::new(),
        }
    }
}

/// Default refresh interval for a `command:` workbar segment that doesn't specify one.
pub const DEFAULT_WORKBAR_COMMAND_INTERVAL_SECS: u64 = 60;

/// One segment of the configurable workbar. `Workspaces` is the workspace tab strip;
/// `Session` is the live attach-connection badge (invisible until attached to a named session);
/// `Text` is a literal with `{host}`/`{workspace}`/`{layout}`/`{session}` placeholders;
/// `Command` runs a shell command on a timer and shows the first line of its stdout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkbarSegment {
    Title,
    Workspaces,
    Session,
    Clock,
    Layout,
    Activity,
    Text(String),
    Command { command: String, interval_secs: u64 },
}

impl WorkbarSegment {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(literal) = value.strip_prefix("text:") {
            return Some(Self::Text(literal.to_string()));
        }
        if let Some(rest) = value.strip_prefix("command:") {
            return Some(Self::parse_command(rest));
        }
        match value.to_ascii_lowercase().as_str() {
            "title" => Some(Self::Title),
            "workspaces" => Some(Self::Workspaces),
            "session" => Some(Self::Session),
            "clock" => Some(Self::Clock),
            "layout" => Some(Self::Layout),
            "activity" => Some(Self::Activity),
            _ => None,
        }
    }

    /// Parses the part of a `command:` segment after the prefix: either `<command>` (default
    /// interval) or `<interval_secs>:<command>` when the part before the first colon is a
    /// plain integer.
    fn parse_command(rest: &str) -> Self {
        if let Some((secs, command)) = rest.split_once(':') {
            if let Ok(interval_secs) = secs.trim().parse::<u64>() {
                return Self::Command {
                    command: command.to_string(),
                    interval_secs: interval_secs.max(1),
                };
            }
        }
        Self::Command {
            command: rest.to_string(),
            interval_secs: DEFAULT_WORKBAR_COMMAND_INTERVAL_SECS,
        }
    }

    pub fn is_clock(&self) -> bool {
        matches!(self, Self::Clock)
    }
}

/// A workbar badge color chosen by theme role name (not a literal color) so a segment's badge
/// tracks the active theme. Resolved to concrete `(bg, fg)` colors at render time by the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeColor {
    Accent,
    Info,
    Success,
    Warning,
    Error,
    Neutral,
    Panel,
}

impl BadgeColor {
    /// Accepted role names for the `color` field of a `[workbar]` segment table.
    pub const NAMES: &'static str = "accent, info, success, warning, error, neutral, panel";

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "accent" => Some(Self::Accent),
            "info" => Some(Self::Info),
            "success" => Some(Self::Success),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            "neutral" => Some(Self::Neutral),
            "panel" => Some(Self::Panel),
            _ => None,
        }
    }
}

/// A configured workbar segment plus an optional badge color override. `color: None` uses the
/// segment's curated default color (see the view's `curated_color`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkbarItem {
    pub segment: WorkbarSegment,
    pub color: Option<BadgeColor>,
}

impl WorkbarItem {
    fn new(segment: WorkbarSegment) -> Self {
        Self {
            segment,
            color: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkbarConfig {
    pub left: Vec<WorkbarItem>,
    pub right: Vec<WorkbarItem>,
    pub clock_format: String,
}

impl Default for WorkbarConfig {
    fn default() -> Self {
        // Badge + workspace tabs on the left; the session badge sits on the right but stays
        // invisible until an attach connection exists, so local mode looks unchanged.
        Self {
            left: vec![
                WorkbarItem::new(WorkbarSegment::Title),
                WorkbarItem::new(WorkbarSegment::Workspaces),
            ],
            right: vec![WorkbarItem::new(WorkbarSegment::Session)],
            clock_format: "%H:%M".to_string(),
        }
    }
}

impl WorkbarConfig {
    pub fn has_clock(&self) -> bool {
        self.left
            .iter()
            .chain(self.right.iter())
            .any(|item| item.segment.is_clock())
    }

    /// Unique `(command, interval_secs)` pairs across both workbar sides, one background poller
    /// per distinct command string even if it appears in multiple segments.
    pub fn command_specs(&self) -> Vec<(String, u64)> {
        let mut seen = std::collections::HashSet::new();
        self.left
            .iter()
            .chain(self.right.iter())
            .filter_map(|item| match &item.segment {
                WorkbarSegment::Command {
                    command,
                    interval_secs,
                } => Some((command.clone(), *interval_secs)),
                _ => None,
            })
            .filter(|(command, _)| seen.insert(command.clone()))
            .collect()
    }
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    shell: Option<String>,
    cwd: Option<String>,
    scrollback: Option<usize>,
    input: InputFileConfig,
    animations: AnimationFileConfig,
    theme: ThemeFileConfig,
    profile: ProfileFileConfig,
    session: SessionFileConfig,
    pane: PaneFileConfig,
    clipboard: ClipboardFileConfig,
    notifications: NotificationsFileConfig,
    navigation: NavigationFileConfig,
    confirm: ConfirmFileConfig,
    scratchpad: ScratchpadFileConfig,
    workbar: WorkbarFileConfig,
    keys: HashMap<String, KeyBindingSpec>,
}

/// A `[keys]` value: replacement bindings, an additive binding table, or a user command table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyBindingSpec {
    One(String),
    Many(Vec<String>),
    Add(AddKeyBindingSpec),
    UserCommand(UserCommandTableSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddKeyBindingSpec {
    add: KeyBindingCandidates,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyBindingCandidates {
    One(String),
    Many(Vec<String>),
}

impl KeyBindingCandidates {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct UserCommandTableSpec {
    run: Option<String>,
    send: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ConfirmFileConfig {
    close_pane: Option<bool>,
    kill_workspace: Option<bool>,
    kill_session: Option<bool>,
    quit_ephemeral: Option<bool>,
    new_temporary_session: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ScratchpadFileConfig {
    command: Option<String>,
    cwd: Option<String>,
    height: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WorkbarFileConfig {
    left: Option<Vec<WorkbarSegmentSpec>>,
    right: Option<Vec<WorkbarSegmentSpec>>,
    clock_format: Option<String>,
}

/// A `[workbar]` list entry: either a bare segment name (`"clock"`, `"text:.."`, `"command:.."`)
/// or a table `{ segment = "..", color = "info" }` that overrides the badge color by theme role.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkbarSegmentSpec {
    Name(String),
    Table {
        segment: String,
        #[serde(default)]
        color: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProfileFileConfig {
    default: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SessionFileConfig {
    autosave: Option<bool>,
    path: Option<String>,
    startup: Option<String>,
}

/// `[pane] padding` value: a single number applies to all four sides, or a CSS-style array of
/// `[vertical, horizontal]` (2 values) or `[top, right, bottom, left]` (4 values).
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
enum PaddingSpec {
    All(u16),
    Sides(Vec<u16>),
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PaneFileConfig {
    highlight_focused_background: Option<bool>,
    highlight_focused_border: Option<bool>,
    focus_on_hover: Option<bool>,
    show_workbar: Option<bool>,
    workbar_gap: Option<bool>,
    workbar_at_bottom: Option<bool>,
    show_titles: Option<bool>,
    merge_borders: Option<bool>,
    border_style: Option<String>,
    padding: Option<PaddingSpec>,
    title_style: Option<String>,
    workbar_badge_style: Option<String>,
    workbar_powerline: Option<bool>,
    workbar_tab_style: Option<String>,
    workbar_style: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_section_overrides_defaults() {
        let defaults = HyprmuxConfirmConfig::default();
        assert!(!defaults.close_pane);
        assert!(defaults.kill_workspace);
        assert!(defaults.kill_session);
        assert!(defaults.quit_ephemeral);
        assert!(defaults.new_temporary_session);

        let parsed: FileConfig = toml::from_str(
            r#"
            [confirm]
            close_pane = true
            kill_workspace = false
            quit_ephemeral = false
            new_temporary_session = false
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.confirm.close_pane, Some(true));
        assert_eq!(parsed.confirm.kill_workspace, Some(false));
        assert_eq!(parsed.confirm.kill_session, None);
        assert_eq!(parsed.confirm.quit_ephemeral, Some(false));
        assert_eq!(parsed.confirm.new_temporary_session, Some(false));
    }

    #[test]
    fn session_startup_parses_known_values_and_defaults_to_ephemeral() {
        assert_eq!(SessionStartup::default(), SessionStartup::Ephemeral);
        assert_eq!(
            SessionStartup::parse("ephemeral"),
            Some(SessionStartup::Ephemeral)
        );
        assert_eq!(
            SessionStartup::parse("PICKER"),
            Some(SessionStartup::Picker)
        );
        assert_eq!(
            SessionStartup::parse(" pick "),
            Some(SessionStartup::Picker)
        );
        assert_eq!(SessionStartup::parse("nonsense"), None);
    }

    #[test]
    fn session_section_parses_startup() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [session]
            startup = "picker"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.session.startup.as_deref(), Some("picker"));
    }

    #[test]
    fn key_overrides_parse_native_bindings_and_warn_on_unknown_action() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = ["alt-enter"]
            close = "ctrl-b q"
            notanaction = "x"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(
            overrides.get("spawn"),
            Some(&vec![KeyBinding::from_str("alt-enter").unwrap()])
        );
        assert_eq!(
            overrides.get("close"),
            Some(&vec![KeyBinding::from_str("ctrl-b q").unwrap()])
        );

        // Unknown action id yields exactly one warning and is skipped.
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("notanaction"));
    }

    #[test]
    fn keys_table_value_registers_a_run_user_command() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = { run = "lazygit" }
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(user_commands.len(), 1);
        assert_eq!(
            user_commands[0].action,
            UserCommandAction::Run("lazygit".to_string())
        );
        assert_eq!(
            user_commands[0].binding,
            KeyBinding::from_str("ctrl-a g").unwrap()
        );
    }

    #[test]
    fn keys_table_value_registers_a_send_user_command() {
        let parsed: FileConfig = toml::from_str(
            "
            [keys]
            alt-g = { send = \"echo hi\\n\" }
            ",
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(user_commands.len(), 1);
        assert_eq!(
            user_commands[0].action,
            UserCommandAction::Send("echo hi\n".to_string())
        );
    }

    #[test]
    fn keys_table_value_without_run_or_send_warns_and_is_skipped() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = {}
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(user_commands.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("run"));
    }

    #[test]
    fn keys_table_value_with_both_run_and_send_warns_and_is_skipped() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = { run = "lazygit", send = "hi" }
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(user_commands.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("both"));
    }

    #[test]
    fn user_command_label_describes_run_and_send() {
        let run = UserCommand {
            action: UserCommandAction::Run("lazygit".to_string()),
            binding: KeyBinding::from_str("ctrl-a g").unwrap(),
        };
        assert_eq!(run.label(), "Run: lazygit");

        let send = UserCommand {
            action: UserCommandAction::Send("ls -la\n".to_string()),
            binding: KeyBinding::from_str("ctrl-a g").unwrap(),
        };
        assert_eq!(send.label(), "Send: ls -la\\n");
    }

    #[test]
    fn user_command_label_truncates_long_commands() {
        let run = UserCommand {
            action: UserCommandAction::Run("x".repeat(60)),
            binding: KeyBinding::from_str("ctrl-a g").unwrap(),
        };
        let label = run.label();
        assert!(label.starts_with("Run: "));
        assert!(label.ends_with('…'));
        assert!(label.chars().count() < 60);
    }

    #[test]
    fn bare_key_override_expands_through_the_input_scheme() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "b"
            close = "shift-w"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a b").unwrap(),
                KeyBinding::from_str("alt-b").unwrap(),
            ])
        );
        // Shift-only steps count as bare keys too, matching the built-in default-key grammar.
        assert_eq!(
            overrides.get("close"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a shift-w").unwrap(),
                KeyBinding::from_str("alt-shift-w").unwrap(),
            ])
        );
    }

    #[test]
    fn bare_key_override_follows_custom_prefix_modifier_and_mirror_toggle() {
        let keys = || {
            toml::from_str::<FileConfig>(
                r#"
                [keys]
                copy-mode = "b"
                "#,
            )
            .expect("config parses")
            .keys
        };

        let mut input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };
        let mut warnings = Vec::new();
        let overrides = build_key_overrides(keys(), &input, &mut Vec::new(), &mut warnings);
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-b b").unwrap(),
                KeyBinding::from_str("super-b").unwrap(),
            ])
        );

        input.modifier_shortcuts = false;
        let overrides = build_key_overrides(keys(), &input, &mut Vec::new(), &mut warnings);
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![KeyBinding::from_str("ctrl-b b").unwrap()])
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn bare_keys_and_literal_bindings_mix_in_one_override() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = ["n", "super-enter"]
            focus-left = "ctrl-h"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("spawn"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a n").unwrap(),
                KeyBinding::from_str("alt-n").unwrap(),
                KeyBinding::from_str("super-enter").unwrap(),
            ])
        );
        // A step with a real modifier stays a literal binding rather than expanding.
        assert_eq!(
            overrides.get("focus-left"),
            Some(&vec![KeyBinding::from_str("ctrl-h").unwrap()])
        );
    }

    #[test]
    fn additive_key_override_keeps_defaults_and_accepts_bare_or_literal_bindings() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = { add = ["n", "super-enter", "enter"] }
            "#,
        )
        .expect("config parses");

        let input = InputConfig::default();
        let defaults = crate::commands::default_shortcuts_for_action(&input, "spawn").unwrap();
        let mut warnings = Vec::new();
        let overrides = build_key_overrides(parsed.keys, &input, &mut Vec::new(), &mut warnings);
        let bindings = overrides.get("spawn").unwrap();

        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(bindings.starts_with(&defaults));
        assert!(bindings.contains(&KeyBinding::from_str("ctrl-a n").unwrap()));
        assert!(bindings.contains(&KeyBinding::from_str("alt-n").unwrap()));
        assert!(bindings.contains(&KeyBinding::from_str("super-enter").unwrap()));
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| **binding == KeyBinding::from_str("ctrl-a enter").unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn scheme_marker_expands_modified_keys_in_replacements_and_additions() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "scheme:ctrl-t"
            spawn = { add = ["scheme:ctrl-n"] }
            "#,
        )
        .expect("config parses");
        let input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(parsed.keys, &input, &mut Vec::new(), &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-b ctrl-t").unwrap(),
                KeyBinding::from_str("super-ctrl-t").unwrap(),
            ])
        );
        let spawn = overrides.get("spawn").unwrap();
        assert!(spawn.contains(&KeyBinding::from_str("ctrl-b ctrl-n").unwrap()));
        assert!(spawn.contains(&KeyBinding::from_str("super-ctrl-n").unwrap()));
    }

    #[test]
    fn scheme_marker_rejects_multiple_key_steps() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "scheme:ctrl-t x"
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();

        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(!overrides.contains_key("copy-mode"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("one key step"))
        );
    }

    #[test]
    fn top_level_input_aliases_are_rejected() {
        let error = toml::from_str::<FileConfig>(
            r#"
            prefix = "ctrl-b"
            modifier = "super"
            "#,
        )
        .expect_err("top-level input aliases should not parse");

        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn empty_key_specs_record_an_explicit_unbind() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = []
            spawn = ""
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(overrides.get("scratchpad"), Some(&Vec::new()));
        assert_eq!(overrides.get("spawn"), Some(&Vec::new()));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn malformed_key_specs_keep_defaults_and_warn() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = "not-a-real-key"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        // A fully-unparseable spec is a config error, not an intentional unbind - no override
        // is recorded, so `resolve_shortcuts` falls back to `scratchpad`'s default shortcuts
        // instead of leaving it unreachable.
        assert_eq!(overrides.get("scratchpad"), None);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("not-a-real-key"));
        assert!(warnings[0].contains("scratchpad"));
        assert!(warnings[1].contains("scratchpad"));
        assert!(warnings[1].contains("default"));
    }

    #[test]
    fn no_default_actions_are_bindable() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            toggle-pane-synchronization = "ctrl-a y"
            rename-session = "alt-y"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(
            overrides.get("toggle-pane-synchronization"),
            Some(&vec![KeyBinding::from_str("ctrl-a y").unwrap()])
        );
        assert_eq!(
            overrides.get("rename-session"),
            Some(&vec![KeyBinding::from_str("alt-y").unwrap()])
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn file_config_parses_profile_default() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [profile]
            default = "dev"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.profile.default.as_deref(), Some("dev"));
    }

    #[test]
    fn list_profiles_reads_sorted_toml_stems() {
        let temp =
            std::env::temp_dir().join(format!("hyprmux-profiles-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).expect("tempdir");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &temp);
        }

        let profiles = temp.join("hyprmux/profiles");
        std::fs::create_dir_all(&profiles).expect("profiles dir");
        std::fs::write(profiles.join("beta.toml"), "version = 1\n").expect("beta");
        std::fs::write(profiles.join("alpha.toml"), "version = 1\n").expect("alpha");
        std::fs::write(profiles.join("notes.txt"), "skip").expect("txt");

        let listed = list_profiles();
        assert_eq!(
            listed
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn profile_upsert_adds_missing_section() {
        assert_eq!(
            upsert_default_profile("scrollback = 100\n", "dev"),
            "scrollback = 100\n\n[profile]\ndefault = \"dev\"\n"
        );
    }

    #[test]
    fn profile_upsert_replaces_default_and_removes_path() {
        let updated = upsert_default_profile(
            "[profile]\npath = \"~/old.toml\"\ndefault = \"old\"\n\n[session]\nautosave = true\n",
            "dev",
        );
        assert_eq!(
            updated,
            "[profile]\ndefault = \"dev\"\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn remove_default_profile_strips_matching_entry() {
        let text = "[profile]\ndefault = \"dev\"\n\n[session]\nautosave = true\n";
        assert_eq!(
            remove_default_profile(text, "dev"),
            "[profile]\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn remove_default_profile_leaves_other_defaults() {
        let text = "[profile]\ndefault = \"work\"\n";
        assert_eq!(remove_default_profile(text, "dev"), text);
    }

    #[test]
    fn delete_profile_file_treats_missing_as_success() {
        let path = std::env::temp_dir().join(format!(
            "hyprmux-missing-profile-{}.toml",
            std::process::id()
        ));
        delete_profile_file(&path).expect("missing profile delete succeeds");
    }

    #[test]
    fn file_config_parses_pane_options() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            highlight_focused_background = true
            highlight_focused_border = true
            focus_on_hover = false
            show_workbar = false
            workbar_gap = false
            workbar_at_bottom = true
            show_titles = false
            padding = 2
            title_style = "round"
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            workbar_style = "half"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.pane.highlight_focused_background, Some(true));
        assert_eq!(parsed.pane.highlight_focused_border, Some(true));
        assert_eq!(parsed.pane.focus_on_hover, Some(false));
        assert_eq!(parsed.pane.show_workbar, Some(false));
        assert_eq!(parsed.pane.workbar_gap, Some(false));
        assert_eq!(parsed.pane.workbar_at_bottom, Some(true));
        assert_eq!(parsed.pane.show_titles, Some(false));
        assert_eq!(parsed.pane.padding, Some(PaddingSpec::All(2)));
        assert_eq!(parsed.pane.title_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_badge_style.as_deref(), Some("arrow"));
        assert_eq!(parsed.pane.workbar_tab_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_style.as_deref(), Some("half"));
    }

    #[test]
    fn pane_padding_accepts_scalar_and_array_forms() {
        let scalar: PaneFileConfig = toml::from_str("padding = 3").expect("scalar parses");
        assert_eq!(scalar.padding, Some(PaddingSpec::All(3)));

        let pair: PaneFileConfig = toml::from_str("padding = [0, 1]").expect("pair parses");
        assert_eq!(pair.padding, Some(PaddingSpec::Sides(vec![0, 1])));

        let quad: PaneFileConfig = toml::from_str("padding = [1, 2, 3, 4]").expect("quad parses");
        assert_eq!(quad.padding, Some(PaddingSpec::Sides(vec![1, 2, 3, 4])));
    }

    #[test]
    fn resolve_pane_padding_maps_css_shorthand() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(2), &mut warnings),
            Some((2, 2, 2, 2))
        );
        // Two values are [vertical, horizontal].
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![0, 1]), &mut warnings),
            Some((0, 1, 0, 1))
        );
        // Four values are [top, right, bottom, left].
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3, 4]), &mut warnings),
            Some((1, 2, 3, 4))
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn resolve_pane_padding_clamps_and_rejects_bad_lengths() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(99), &mut warnings),
            Some((8, 8, 8, 8))
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3]), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(Vec::new()), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn workbar_defaults_match_documented_values() {
        let workbar = WorkbarConfig::default();
        assert_eq!(
            workbar.right,
            vec![WorkbarItem {
                segment: WorkbarSegment::Session,
                color: None,
            }]
        );
        assert!(HyprmuxPaneConfig::default().workbar_powerline);
    }

    #[test]
    fn workbar_segment_table_form_overrides_color() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [workbar]
            right = [{ segment = "clock", color = "info" }, "session"]
            "#,
        )
        .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, parsed.workbar, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            workbar.right,
            vec![
                WorkbarItem {
                    segment: WorkbarSegment::Clock,
                    color: Some(BadgeColor::Info),
                },
                WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                },
            ]
        );
    }

    #[test]
    fn workbar_unknown_color_warns_and_falls_back_to_default() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [workbar]
            right = [{ segment = "clock", color = "chartreuse" }]
            "#,
        )
        .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, parsed.workbar, &mut warnings);
        assert_eq!(
            workbar.right,
            vec![WorkbarItem {
                segment: WorkbarSegment::Clock,
                color: None,
            }]
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn workbar_powerline_parses_and_applies() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_powerline = false
            "#,
        )
        .expect("config parses");
        assert_eq!(parsed.pane.workbar_powerline, Some(false));
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);
        assert!(warnings.is_empty());
        assert!(!pane.workbar_powerline);
    }

    #[test]
    fn workbar_badge_style_backfills_workbar_tabs_when_tabs_are_unset() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_badge_style = "arrow"
            "#,
        )
        .expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, CapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, CapStyle::Arrow);
    }

    #[test]
    fn explicit_workbar_tab_style_overrides_only_tabs() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            "#,
        )
        .expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, CapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, CapStyle::Round);
    }

    #[test]
    fn pane_title_style_parses_aliases_and_cycles() {
        assert_eq!(CapStyle::parse("padded"), Some(CapStyle::Padded));
        assert_eq!(CapStyle::parse("Half Block"), Some(CapStyle::Half));
        assert_eq!(CapStyle::parse("pill"), Some(CapStyle::Round));
        assert_eq!(CapStyle::parse("powerline"), Some(CapStyle::Arrow));
        assert_eq!(CapStyle::parse("nonsense"), None);
        assert_eq!(CapStyle::Padded.caps(), None);
        assert!(CapStyle::Round.caps().is_some());
        assert_eq!(CapStyle::Arrow.next(), CapStyle::Padded);
    }

    #[test]
    fn upsert_bool_in_section_replaces_and_preserves_comments() {
        let text = "# chrome prefs\n[pane]\nfocus_on_hover = true\n# keep\n";
        let updated = upsert_bool_in_section(text, "pane", "focus_on_hover", false);
        assert!(updated.contains("# chrome prefs"));
        assert!(updated.contains("focus_on_hover = false"));
        assert!(updated.contains("# keep"));
        assert!(!updated.contains("focus_on_hover = true"));
    }

    #[test]
    fn upsert_bool_in_section_appends_missing_section() {
        let updated = upsert_bool_in_section("", "pane", "show_workbar", true);
        assert_eq!(updated, "[pane]\nshow_workbar = true\n");
    }

    #[test]
    fn file_config_parses_notifications() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [notifications]
            enabled = true
            pane_exit = false
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.notifications.enabled, Some(true));
        assert_eq!(parsed.notifications.pane_exit, Some(false));
    }

    #[test]
    fn file_config_parses_navigation_editors() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [navigation]
            editors = ["nvim", "hx"]
            "#,
        )
        .expect("config parses");

        assert_eq!(
            parsed.navigation.editors,
            Some(vec!["nvim".to_string(), "hx".to_string()])
        );
    }

    #[test]
    fn default_navigation_recognizes_vim_family_case_insensitively() {
        let nav = HyprmuxNavigationConfig::default();
        assert!(nav.is_split_editor("nvim"));
        assert!(nav.is_split_editor("VIM"));
        assert!(nav.is_split_editor("vimdiff"));
        assert!(!nav.is_split_editor("bash"));
        assert!(!nav.is_split_editor("less"));
    }

    #[test]
    fn theme_choices_lead_with_system_then_builtins() {
        let choices = build_theme_choices(Vec::new());
        assert_eq!(choices.first(), Some(&ThemeChoice::System));
        assert_eq!(choices.len(), 1 + ThemePreset::all().len());
        assert!(choices.contains(&ThemeChoice::Builtin(ThemePreset::Dracula)));
    }

    #[test]
    fn custom_theme_shadows_same_named_builtin() {
        let custom = vec![("dracula".to_string(), PathBuf::from("/themes/dracula.toml"))];
        let choices = build_theme_choices(custom);
        // The built-in dracula is dropped in favour of the custom file of the same name.
        assert!(!choices.contains(&ThemeChoice::Builtin(ThemePreset::Dracula)));
        assert_eq!(
            choices.last(),
            Some(&ThemeChoice::Custom {
                name: "dracula".to_string(),
                path: PathBuf::from("/themes/dracula.toml"),
            })
        );
        assert_eq!(choices.iter().filter(|c| c.label() == "dracula").count(), 1);
    }

    #[test]
    fn resolve_theme_falls_back_to_lipan_for_unknown_name() {
        let resolved = resolve_theme("definitely-not-a-real-theme-xyz", None);
        assert!(!resolved.warnings.is_empty());
        assert!(resolved.watch_path.is_none());
    }

    #[test]
    fn theme_upsert_adds_missing_section() {
        assert_eq!(
            upsert_theme_name("scrollback = 100\n", "lipan"),
            "scrollback = 100\n\n[theme]\nname = \"lipan\"\n"
        );
    }

    #[test]
    fn theme_upsert_replaces_name_and_removes_legacy_keys() {
        let updated = upsert_theme_name(
            "[theme]\npreset = \"dracula\"\npath = \"~/theme.toml\"\n\n[session]\nautosave = true\n",
            "my-nord",
        );
        assert_eq!(
            updated,
            "[theme]\nname = \"my-nord\"\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn workbar_segment_parses_builtins_and_text_literals() {
        assert_eq!(WorkbarSegment::parse("clock"), Some(WorkbarSegment::Clock));
        assert_eq!(
            WorkbarSegment::parse("Workspaces"),
            Some(WorkbarSegment::Workspaces)
        );
        assert_eq!(
            WorkbarSegment::parse("text:hi {host}"),
            Some(WorkbarSegment::Text("hi {host}".to_string()))
        );
        assert_eq!(WorkbarSegment::parse("bogus"), None);
    }

    #[test]
    fn workbar_segment_parses_command_with_and_without_interval() {
        assert_eq!(
            WorkbarSegment::parse("command:uptime -p"),
            Some(WorkbarSegment::Command {
                command: "uptime -p".to_string(),
                interval_secs: DEFAULT_WORKBAR_COMMAND_INTERVAL_SECS,
            })
        );
        assert_eq!(
            WorkbarSegment::parse("command:5:uptime -p"),
            Some(WorkbarSegment::Command {
                command: "uptime -p".to_string(),
                interval_secs: 5,
            })
        );
        // A command containing colons but no valid leading integer keeps the default interval
        // and the whole remainder as the command.
        assert_eq!(
            WorkbarSegment::parse("command:echo 12:34"),
            Some(WorkbarSegment::Command {
                command: "echo 12:34".to_string(),
                interval_secs: DEFAULT_WORKBAR_COMMAND_INTERVAL_SECS,
            })
        );
    }

    #[test]
    fn workbar_config_default_matches_current_layout() {
        let workbar = WorkbarConfig::default();
        assert_eq!(
            workbar.left,
            vec![
                WorkbarItem::new(WorkbarSegment::Title),
                WorkbarItem::new(WorkbarSegment::Workspaces),
            ]
        );
        assert_eq!(
            workbar.right,
            vec![WorkbarItem::new(WorkbarSegment::Session)]
        );
        assert!(!workbar.has_clock());
        assert_eq!(workbar.clock_format, "%H:%M");
        assert!(workbar.command_specs().is_empty());
    }

    #[test]
    fn workbar_config_command_specs_dedups_by_command_string() {
        let mut workbar = WorkbarConfig::default();
        workbar.left.push(WorkbarItem::new(WorkbarSegment::Command {
            command: "uptime -p".to_string(),
            interval_secs: 10,
        }));
        workbar
            .right
            .push(WorkbarItem::new(WorkbarSegment::Command {
                command: "uptime -p".to_string(),
                interval_secs: 30,
            }));
        workbar
            .right
            .push(WorkbarItem::new(WorkbarSegment::Command {
                command: "whoami".to_string(),
                interval_secs: 5,
            }));
        let specs = workbar.command_specs();
        assert_eq!(
            specs,
            vec![("uptime -p".to_string(), 10), ("whoami".to_string(), 5),]
        );
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InputFileConfig {
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ThemeFileConfig {
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ClipboardFileConfig {
    enable_osc52: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct NotificationsFileConfig {
    enabled: Option<bool>,
    pane_exit: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct NavigationFileConfig {
    editors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct AnimationFileConfig {
    enabled: Option<bool>,
    spawn: Option<bool>,
    close: Option<bool>,
    fullscreen: Option<bool>,
    tile_float: Option<bool>,
    axis_change: Option<bool>,
    focus_chrome: Option<bool>,
    geometry_ms: Option<u64>,
    close_ms: Option<u64>,
    focus_chrome_ms: Option<u64>,
    open_delay_ms: Option<u64>,
}

/// The config text most recently read or written by this process. Lets the live-reload
/// watcher distinguish external edits from hyprmux's own persistence writes (theme selection,
/// appearance toggles, default profile) and skip event bursts that left the content unchanged.
static LAST_SEEN_CONFIG: Mutex<Option<String>> = Mutex::new(None);

fn note_config_text(text: Option<String>) {
    *LAST_SEEN_CONFIG.lock().unwrap() = text;
}

/// True when the on-disk config no longer matches the text hyprmux last read or wrote.
pub fn config_text_changed_on_disk() -> bool {
    let current = std::fs::read_to_string(config_path()).ok();
    *LAST_SEEN_CONFIG.lock().unwrap() != current
}

pub fn load_config() -> LoadedConfig {
    let path = config_path();
    let mut warnings = Vec::new();
    let mut config = HyprmuxConfig::default();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            note_config_text(None);
            return LoadedConfig { config, warnings };
        }
        Err(err) => {
            note_config_text(None);
            warnings.push(format!("Config read failed for {}: {err}", path.display()));
            return LoadedConfig { config, warnings };
        }
    };
    note_config_text(Some(text.clone()));

    let parsed = match toml::from_str::<FileConfig>(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(format!("Config parse failed for {}: {err}", path.display()));
            return LoadedConfig { config, warnings };
        }
    };

    if let Some(shell) = non_empty(parsed.shell) {
        config.shell = Some(shell);
    }
    if let Some(cwd) = non_empty(parsed.cwd) {
        config.cwd = Some(expand_path(cwd).to_string_lossy().to_string());
    }
    if let Some(scrollback) = parsed.scrollback {
        config.scrollback = scrollback.max(1);
    }

    let mut input = config.input.clone();
    apply_input_config(
        &mut input,
        parsed.input.modifier,
        parsed.input.prefix,
        parsed.input.modifier_shortcuts,
        &mut warnings,
    );
    config.input = input;
    apply_animations(&mut config.animations, parsed.animations);

    if let Some(name) = non_empty(parsed.theme.name) {
        config.theme.name = name;
    }
    if let Some(name) = non_empty(parsed.profile.default) {
        config.profile.default = Some(name);
    }
    if let Some(autosave) = parsed.session.autosave {
        config.session.autosave = autosave;
    }
    if let Some(path) = non_empty(parsed.session.path) {
        config.session.path = Some(expand_path(path));
    }
    if let Some(startup) = non_empty(parsed.session.startup) {
        match SessionStartup::parse(&startup) {
            Some(value) => config.session.startup = value,
            None => warnings.push(format!(
                "Ignored unknown session.startup \"{startup}\" (expected `ephemeral` or `picker`)"
            )),
        }
    }
    if let Some(highlight_focused_background) = parsed.pane.highlight_focused_background {
        config.pane.highlight_focused_background = highlight_focused_background;
    }
    if let Some(highlight_focused_border) = parsed.pane.highlight_focused_border {
        config.pane.highlight_focused_border = highlight_focused_border;
    }
    if let Some(focus_on_hover) = parsed.pane.focus_on_hover {
        config.pane.focus_on_hover = focus_on_hover;
    }
    if let Some(show_workbar) = parsed.pane.show_workbar {
        config.pane.show_workbar = show_workbar;
    }
    if let Some(workbar_gap) = parsed.pane.workbar_gap {
        config.pane.workbar_gap = workbar_gap;
    }
    if let Some(workbar_at_bottom) = parsed.pane.workbar_at_bottom {
        config.pane.workbar_at_bottom = workbar_at_bottom;
    }
    if let Some(show_titles) = parsed.pane.show_titles {
        config.pane.show_titles = show_titles;
    }
    if let Some(merge_borders) = parsed.pane.merge_borders {
        config.pane.merge_borders = merge_borders;
    }
    if let Some(border_style) = parsed.pane.border_style.as_deref() {
        match PaneBorderStyle::parse(border_style) {
            Some(style) => config.pane.border_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.border_style \"{border_style}\" (expected one of: rounded, plain, double, thick)"
            )),
        }
    }
    if let Some(padding) = parsed.pane.padding.clone() {
        if let Some(resolved) = resolve_pane_padding(padding, &mut warnings) {
            config.pane.padding = resolved;
        }
    }
    if let Some(title_style) = parsed.pane.title_style.as_deref() {
        match CapStyle::parse(title_style) {
            Some(style) => config.pane.title_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.title_style \"{title_style}\" (expected one of: padded, half, round, arrow)"
            )),
        }
    }
    apply_workbar_style_config(&mut config.pane, &parsed.pane, &mut warnings);
    if let Some(enable_osc52) = parsed.clipboard.enable_osc52 {
        config.clipboard.enable_osc52 = enable_osc52;
    }
    if let Some(enabled) = parsed.notifications.enabled {
        config.notifications.enabled = enabled;
    }
    if let Some(pane_exit) = parsed.notifications.pane_exit {
        config.notifications.pane_exit = pane_exit;
    }
    if let Some(editors) = parsed.navigation.editors {
        config.navigation.editors = editors
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
    }
    if let Some(close_pane) = parsed.confirm.close_pane {
        config.confirm.close_pane = close_pane;
    }
    if let Some(kill_workspace) = parsed.confirm.kill_workspace {
        config.confirm.kill_workspace = kill_workspace;
    }
    if let Some(kill_session) = parsed.confirm.kill_session {
        config.confirm.kill_session = kill_session;
    }
    if let Some(quit_ephemeral) = parsed.confirm.quit_ephemeral {
        config.confirm.quit_ephemeral = quit_ephemeral;
    }
    if let Some(new_temporary_session) = parsed.confirm.new_temporary_session {
        config.confirm.new_temporary_session = new_temporary_session;
    }

    config.scratchpad.command = non_empty(parsed.scratchpad.command);
    config.scratchpad.cwd =
        non_empty(parsed.scratchpad.cwd).map(|cwd| expand_path(cwd).to_string_lossy().to_string());
    if let Some(height) = parsed.scratchpad.height {
        let clamped = height.clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
        if (clamped - height).abs() > f32::EPSILON {
            warnings.push(format!(
                "Scratchpad height {height} out of range; clamped to {clamped}"
            ));
        }
        config.scratchpad.height = clamped;
    }

    apply_workbar_config(&mut config.workbar, parsed.workbar, &mut warnings);
    let mut user_commands = Vec::new();
    config.key_overrides = build_key_overrides(
        parsed.keys,
        &config.input,
        &mut user_commands,
        &mut warnings,
    );
    config.user_commands = user_commands;

    LoadedConfig { config, warnings }
}

/// Directory holding custom theme files. Each `*.toml` file is a theme named by its stem.
pub fn themes_dir() -> PathBuf {
    config_home().join("hyprmux/themes")
}

/// Path a custom theme named `name` would live at (whether or not it exists).
pub fn custom_theme_path(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.toml"))
}

/// Every custom theme file in [`themes_dir`], as `(name, path)`, sorted by name.
pub fn list_custom_themes() -> Vec<(String, PathBuf)> {
    let Ok(read_dir) = fs::read_dir(themes_dir()) else {
        return Vec::new();
    };
    let mut entries = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some((name, path))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// The ordered set of selectable themes: `System`, every built-in preset not shadowed by a
/// same-named custom file, then every custom theme in [`themes_dir`].
pub fn theme_choices() -> Vec<ThemeChoice> {
    build_theme_choices(list_custom_themes())
}

fn build_theme_choices(custom: Vec<(String, PathBuf)>) -> Vec<ThemeChoice> {
    let mut choices = vec![ThemeChoice::System];
    for preset in ThemePreset::all() {
        if !custom.iter().any(|(name, _)| name == preset.id()) {
            choices.push(ThemeChoice::Builtin(preset));
        }
    }
    for (name, path) in custom {
        choices.push(ThemeChoice::Custom { name, path });
    }
    choices
}

/// Resolve a `[theme].name` to its choice. A custom file shadows the reserved `system` name
/// and any built-in preset. Returns `None` when the name matches nothing.
pub fn resolve_choice(name: &str) -> Option<ThemeChoice> {
    let path = custom_theme_path(name);
    if path.is_file() {
        return Some(ThemeChoice::Custom {
            name: name.to_string(),
            path,
        });
    }
    if name.eq_ignore_ascii_case("system") {
        return Some(ThemeChoice::System);
    }
    ThemePreset::parse(name).map(ThemeChoice::Builtin)
}

#[derive(Debug)]
pub struct ResolvedTheme {
    pub theme: Theme,
    /// The file to hot-reload while this theme is active (custom themes only).
    pub watch_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// Resolve a `[theme].name` to a concrete theme. `system_theme` supplies the host-derived
/// theme for the reserved `system` name; unknown names and load failures fall back to Lipan
/// with a warning.
pub fn resolve_theme(name: &str, system_theme: Option<&Theme>) -> ResolvedTheme {
    let fallback = ThemePreset::Lipan.theme();
    let mut warnings = Vec::new();
    let choice = match resolve_choice(name) {
        Some(choice) => choice,
        None => {
            warnings.push(format!("Unknown theme `{name}`; using lipan"));
            ThemeChoice::Builtin(ThemePreset::Lipan)
        }
    };
    match choice {
        ThemeChoice::System => ResolvedTheme {
            theme: system_theme.cloned().unwrap_or(fallback),
            watch_path: None,
            warnings,
        },
        ThemeChoice::Builtin(preset) => ResolvedTheme {
            theme: preset.theme(),
            watch_path: None,
            warnings,
        },
        ThemeChoice::Custom { path, .. } => {
            let theme = match load_theme_from_toml(&path, fallback.clone()) {
                Ok(theme) => theme,
                Err(err) => {
                    warnings.push(format!("Theme load failed for {}: {err}", path.display()));
                    fallback
                }
            };
            ResolvedTheme {
                theme,
                watch_path: Some(path),
                warnings,
            }
        }
    }
}

/// Writes an updated config text, creating the config directory when needed, and records the
/// text as last-seen so the live-reload watcher does not treat our own write as an edit.
fn write_config_text(path: &Path, updated: String) -> std::result::Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Could not create config directory {}: {err}",
                parent.display()
            )
        })?;
    }
    fs::write(path, &updated)
        .map_err(|err| format!("Could not write config {}: {err}", path.display()))?;
    note_config_text(Some(updated));
    Ok(())
}

pub fn persist_theme_name(name: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_theme_name(&text, name);
    write_config_text(&path, updated)?;
    Ok(path)
}

fn upsert_theme_name(text: &str, name: &str) -> String {
    let mut output = String::new();
    let mut in_theme = false;
    let mut saw_theme = false;
    let mut wrote_name = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_theme && !wrote_name {
                output.push_str(&format!("name = \"{name}\"\n"));
                wrote_name = true;
            }
            in_theme = trimmed == "[theme]";
            saw_theme |= in_theme;
        }

        if in_theme
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "name" | "preset" | "path"))
        {
            if !wrote_name {
                output.push_str(&format!("name = \"{name}\"\n"));
                wrote_name = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_theme && !wrote_name {
        output.push_str(&format!("name = \"{name}\"\n"));
    } else if !saw_theme {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[theme]\n");
        output.push_str(&format!("name = \"{name}\"\n"));
    }

    output
}

pub fn persist_pane_flag(key: &str, value: bool) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_bool_in_section(&text, "pane", key, value);
    write_config_text(&path, updated)?;
    Ok(path)
}

/// Persist the compact CSS-style vertical/horizontal pane padding form.
pub fn persist_pane_padding(
    vertical: u16,
    horizontal: u16,
) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };
    let updated = upsert_pane_padding(&text, vertical, horizontal);
    write_config_text(&path, updated)?;
    Ok(path)
}

/// Serialize the Appearance editor's explicit vertical/horizontal form. Kept separate from I/O
/// so all accepted source forms are covered by a deterministic persistence test.
fn upsert_pane_padding(text: &str, vertical: u16, horizontal: u16) -> String {
    upsert_value_in_section(
        text,
        "pane",
        "padding",
        &format!("[{vertical}, {horizontal}]"),
    )
}

pub fn persist_animation_flag(key: &str, value: bool) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_bool_in_section(&text, "animations", key, value);
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn persist_pane_string(key: &str, value: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_value_in_section(&text, "pane", key, &format!("\"{value}\""));
    write_config_text(&path, updated)?;
    Ok(path)
}

fn upsert_bool_in_section(text: &str, section: &str, key: &str, value: bool) -> String {
    upsert_value_in_section(text, section, key, if value { "true" } else { "false" })
}

/// Insert or replace `key = <line_value>` inside `[section]`, creating the section at the end
/// of the file when it does not exist yet. `line_value` is written verbatim (already quoted for
/// strings, bare for bools/numbers).
fn upsert_value_in_section(text: &str, section: &str, key: &str, line_value: &str) -> String {
    let section_header = format!("[{section}]");
    let mut output = String::new();
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote_key = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_section && !wrote_key {
                output.push_str(&format!("{key} = {line_value}\n"));
                wrote_key = true;
            }
            in_section = trimmed == section_header;
            saw_section |= in_section;
        }

        if in_section
            && trimmed
                .split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        {
            if !wrote_key {
                output.push_str(&format!("{key} = {line_value}\n"));
                wrote_key = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_section && !wrote_key {
        output.push_str(&format!("{key} = {line_value}\n"));
    } else if !saw_section {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str(&section_header);
        output.push('\n');
        output.push_str(&format!("{key} = {line_value}\n"));
    }

    output
}

#[cfg(test)]
mod padding_persistence_tests {
    use super::*;

    #[test]
    fn pane_padding_upsert_replaces_every_source_form_and_creates_pane_section() {
        for source in [
            "[pane]\npadding = 2\n",
            "[pane]\npadding = [1, 2]\n",
            "[pane]\npadding = [1, 2, 3, 4]\n",
        ] {
            assert_eq!(
                upsert_pane_padding(source, 3, 4),
                "[pane]\npadding = [3, 4]\n"
            );
        }
        assert_eq!(
            upsert_pane_padding("[theme]\nname = \"dark\"\n", 3, 4),
            "[theme]\nname = \"dark\"\n\n[pane]\npadding = [3, 4]\n"
        );
    }
}

pub fn profiles_dir() -> PathBuf {
    config_home().join("hyprmux/profiles")
}

pub fn profile_path_for_name(name: &str) -> PathBuf {
    profiles_dir().join(format!("{name}.toml"))
}

pub fn list_profiles() -> Vec<ProfileEntry> {
    let dir = profiles_dir();
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(ProfileEntry { name, path })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

pub fn persist_default_profile(name: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_default_profile(&text, name);
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn delete_profile_file(path: &Path) -> std::result::Result<(), String> {
    match fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Err(format!("Not a profile file: {}", path.display()));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Could not inspect profile {}: {err}",
                path.display()
            ));
        }
        Ok(_) => {}
    }

    fs::remove_file(path)
        .map_err(|err| format!("Could not delete profile {}: {err}", path.display()))
}

pub fn clear_default_profile(name: &str) -> std::result::Result<Option<PathBuf>, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = remove_default_profile(&text, name);
    if updated == text {
        return Ok(None);
    }

    write_config_text(&path, updated)?;
    Ok(Some(path))
}

fn remove_default_profile(text: &str, name: &str) -> String {
    let target = format!("default = \"{name}\"");
    let mut output = String::new();
    let mut in_profile = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            in_profile = trimmed == "[profile]";
        }

        if in_profile && trimmed == target {
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    output
}

fn upsert_default_profile(text: &str, name: &str) -> String {
    let mut output = String::new();
    let mut in_profile = false;
    let mut saw_profile = false;
    let mut wrote_default = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_profile && !wrote_default {
                output.push_str(&format!("default = \"{name}\"\n"));
                wrote_default = true;
            }
            in_profile = trimmed == "[profile]";
            saw_profile |= in_profile;
        }

        if in_profile
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "default" | "path"))
        {
            if trimmed.starts_with("default") && !wrote_default {
                output.push_str(&format!("default = \"{name}\"\n"));
                wrote_default = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_profile && !wrote_default {
        output.push_str(&format!("default = \"{name}\"\n"));
    } else if !saw_profile {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[profile]\n");
        output.push_str(&format!("default = \"{name}\"\n"));
    }

    output
}

pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPRMUX_CONFIG") {
        return expand_path(path);
    }
    config_home().join("hyprmux/hyprmux.toml")
}

fn apply_input_config(
    input: &mut InputConfig,
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
    warnings: &mut Vec<String>,
) {
    if let Some(modifier_shortcuts) = modifier_shortcuts {
        input.modifier_shortcuts = modifier_shortcuts;
    }
    if let Some(modifier) = modifier {
        match parse_modifier(&modifier) {
            Some(parsed) => input.modifier = parsed,
            None => warnings.push(format!(
                "Unknown modifier `{modifier}`; expected `alt` or `super`"
            )),
        }
    }
    if let Some(prefix) = prefix {
        match KeyBinding::from_str(&prefix) {
            Ok(parsed) if parsed.step_count() == 1 => input.prefix = parsed,
            Ok(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; prefix must be a single key"
            )),
            Err(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; try e.g. `ctrl-a`"
            )),
        }
    }
}

/// True when a `[keys]` candidate is a bare key step: a single chord step carrying at most
/// `shift` (e.g. `"b"`, `"shift-w"`, `"tab"`). A literal global binding on such a step would
/// steal plain typing from the focused terminal, so it is treated as a default-key replacement
/// and expanded through the `[input]` prefix/modifier scheme instead. Steps with a real
/// modifier (`ctrl-h`, `alt-b`) and multi-step chords stay literal.
fn is_bare_key_step(candidate: &str) -> bool {
    let mut steps = candidate.split_whitespace();
    let (Some(step), None) = (steps.next(), steps.next()) else {
        return false;
    };
    // Non-shift modifier spellings accepted by tui-lipan's binding grammar.
    const MODIFIERS: &[&str] = &[
        "ctrl", "control", "alt", "option", "super", "cmd", "command", "meta", "win", "windows",
    ];
    !step
        .split(['-', '+'])
        .any(|token| MODIFIERS.contains(&token.to_ascii_lowercase().as_str()))
}

/// Build `[keys]` overrides. Strings/lists replace an action's defaults, `{ add = ... }` extends
/// its generated defaults, and `{ run = ... }` / `{ send = ... }` defines a user command.
fn build_key_overrides(
    keys: HashMap<String, KeyBindingSpec>,
    input: &InputConfig,
    user_commands: &mut Vec<UserCommand>,
    warnings: &mut Vec<String>,
) -> HashMap<String, Vec<KeyBinding>> {
    let mut overrides = HashMap::new();
    for (key, spec) in keys {
        // A table value (`{ run = ".." }` / `{ send = ".." }`) defines a brand new user
        // command rather than rebinding a built-in action; here the map *key* is the literal
        // trigger binding (e.g. `"ctrl-a g"`), not a command id.
        if let KeyBindingSpec::UserCommand(table) = spec {
            bind_user_command(user_commands, &key, table, warnings);
            continue;
        }

        let Some(default_bindings) = crate::commands::default_shortcuts_for_action(input, &key)
        else {
            warnings.push(format!("Unknown key action `{key}`; skipped"));
            continue;
        };

        let (bindings, additive) = match spec {
            KeyBindingSpec::One(value) => (vec![value], false),
            KeyBindingSpec::Many(values) => (values, false),
            KeyBindingSpec::Add(table) => (table.add.into_vec(), true),
            KeyBindingSpec::UserCommand(_) => unreachable!(),
        };

        let mut parsed_bindings = if additive {
            default_bindings
        } else {
            Vec::new()
        };
        let mut candidate_count = 0;
        let initial_binding_count = parsed_bindings.len();
        for binding in bindings {
            for candidate in binding
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                candidate_count += 1;
                let scheme_key = if let Some(key_step) = candidate.strip_prefix("scheme:") {
                    match KeyBinding::from_str(key_step) {
                        Ok(binding) if binding.step_count() == 1 => Some(key_step),
                        _ => {
                            warnings.push(format!(
                                "Could not parse scheme binding `{candidate}` for `{key}`; expected one key step"
                            ));
                            continue;
                        }
                    }
                } else {
                    is_bare_key_step(candidate).then_some(candidate)
                };
                if let Some(key_step) = scheme_key {
                    let expanded = scheme_shortcuts(input, key_step);
                    if expanded.is_empty() {
                        warnings.push(format!(
                            "Could not parse binding `{candidate}` for `{key}`; skipped"
                        ));
                    } else {
                        for binding in expanded {
                            if !parsed_bindings.contains(&binding) {
                                parsed_bindings.push(binding);
                            }
                        }
                    }
                    continue;
                }
                match KeyBinding::from_str(candidate) {
                    Ok(parsed) => {
                        if !parsed_bindings.contains(&parsed) {
                            parsed_bindings.push(parsed);
                        }
                    }
                    Err(_) => warnings.push(format!(
                        "Could not parse binding `{candidate}` for `{key}`; skipped"
                    )),
                }
            }
        }
        // Only an explicit `= []` (no candidates at all) means "unbind". If every candidate
        // was present but failed to parse (e.g. the pre-tui-lipan `"prefix c"` grammar), that's
        // a config error, not intent to unbind - keep the default shortcuts rather than
        // silently making the action unreachable, and say so plainly.
        if candidate_count > 0 && parsed_bindings.len() == initial_binding_count && !additive {
            warnings.push(format!(
                "No valid bindings for `{key}`; keeping its default shortcuts"
            ));
            continue;
        }
        overrides.insert(key, parsed_bindings);
    }
    overrides
}

fn bind_user_command(
    user_commands: &mut Vec<UserCommand>,
    key: &str,
    table: UserCommandTableSpec,
    warnings: &mut Vec<String>,
) {
    // Trim `run` (a shell command) but keep `send` byte-for-byte: trailing `\n`/whitespace is
    // often exactly the point (e.g. `send = "ls -la\n"` to submit the command).
    let run = table.run.map(|value| value.trim().to_string());
    let send = table.send.filter(|value| !value.is_empty());
    let action = match (run.filter(|value| !value.is_empty()), send) {
        (Some(run), None) => UserCommandAction::Run(run),
        (None, Some(send)) => UserCommandAction::Send(send),
        (None, None) => {
            warnings.push(format!(
                "User command `{key}` needs a `run` or `send` value; skipped"
            ));
            return;
        }
        (Some(_), Some(_)) => {
            warnings.push(format!(
                "User command `{key}` has both `run` and `send`; skipped"
            ));
            return;
        }
    };
    let Ok(binding) = KeyBinding::from_str(key) else {
        warnings.push(format!(
            "Could not parse binding `{key}` for a user command; skipped"
        ));
        return;
    };
    user_commands.push(UserCommand { action, binding });
}

fn apply_workbar_config(
    workbar: &mut WorkbarConfig,
    raw: WorkbarFileConfig,
    warnings: &mut Vec<String>,
) {
    fn parse_segments(
        raw: Vec<WorkbarSegmentSpec>,
        region: &str,
        warnings: &mut Vec<String>,
    ) -> Vec<WorkbarItem> {
        raw.into_iter()
            .filter_map(|spec| {
                let (name, color_name) = match spec {
                    WorkbarSegmentSpec::Name(name) => (name, None),
                    WorkbarSegmentSpec::Table { segment, color } => (segment, color),
                };
                let segment = match WorkbarSegment::parse(&name) {
                    Some(segment) => segment,
                    None => {
                        warnings.push(format!(
                            "Unknown {region} workbar segment `{name}`; skipped"
                        ));
                        return None;
                    }
                };
                // An unknown color role name falls back to the segment's curated default rather than
                // dropping the whole segment.
                let color = match color_name {
                    Some(color_name) => match BadgeColor::parse(&color_name) {
                        Some(color) => Some(color),
                        None => {
                            warnings.push(format!(
                                "Unknown {region} workbar color `{color_name}` for `{name}` (expected one of: {}); using default",
                                BadgeColor::NAMES
                            ));
                            None
                        }
                    },
                    None => None,
                };
                Some(WorkbarItem { segment, color })
            })
            .collect()
    }

    if let Some(left) = raw.left {
        workbar.left = parse_segments(left, "left", warnings);
    }
    if let Some(right) = raw.right {
        workbar.right = parse_segments(right, "right", warnings);
    }
    if let Some(format) = non_empty(raw.clock_format) {
        // Reject invalid strftime so a clock segment can't panic at render time.
        if chrono::format::StrftimeItems::new(&format).parse().is_ok() {
            workbar.clock_format = format;
        } else {
            warnings.push(format!(
                "Invalid clock_format `{format}`; keeping `{}`",
                workbar.clock_format
            ));
        }
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn expand_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(text.as_ref()));
    }
    PathBuf::from(text.as_ref())
}

fn apply_workbar_style_config(
    config: &mut HyprmuxPaneConfig,
    parsed: &PaneFileConfig,
    warnings: &mut Vec<String>,
) {
    if let Some(workbar_badge_style) = parsed.workbar_badge_style.as_deref() {
        match CapStyle::parse(workbar_badge_style) {
            Some(CapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_badge_style \"{workbar_badge_style}\" (half block is not available for workbar badges)"
            )),
            Some(style) => {
                config.workbar_badge_style = style;
                if parsed.workbar_tab_style.is_none() {
                    config.workbar_tab_style = style;
                }
            }
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_badge_style \"{workbar_badge_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_tab_style) = parsed.workbar_tab_style.as_deref() {
        match CapStyle::parse(workbar_tab_style) {
            Some(CapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_tab_style \"{workbar_tab_style}\" (half block is not available for workspace tabs)"
            )),
            Some(style) => config.workbar_tab_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_tab_style \"{workbar_tab_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_style) = parsed.workbar_style.as_deref() {
        match CapStyle::parse(workbar_style) {
            Some(style) => config.workbar_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_style \"{workbar_style}\" (expected one of: padded, half, round, arrow)"
            )),
        }
    }
    if let Some(workbar_powerline) = parsed.workbar_powerline {
        config.workbar_powerline = workbar_powerline;
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Cap defensively: padding eats terminal grid on every side, so a large value would leave no
/// usable pane. 8 cells is already generous for a cosmetic inset.
pub const MAX_PANE_PADDING: u16 = 8;

fn clamp_pane_padding(value: u16, warnings: &mut Vec<String>) -> u16 {
    if value > MAX_PANE_PADDING {
        warnings.push(format!(
            "Clamped pane.padding {value} to the maximum of {MAX_PANE_PADDING}"
        ));
        MAX_PANE_PADDING
    } else {
        value
    }
}

/// Resolve a `[pane] padding` spec into `(top, right, bottom, left)` cells using CSS shorthand: one
/// value applies to all sides, two are `[vertical, horizontal]`, four are `[top, right, bottom,
/// left]`. Other array lengths are rejected with a warning and leave the default untouched.
fn resolve_pane_padding(
    spec: PaddingSpec,
    warnings: &mut Vec<String>,
) -> Option<(u16, u16, u16, u16)> {
    let sides = match spec {
        PaddingSpec::All(value) => vec![value],
        PaddingSpec::Sides(values) => values,
    };
    match sides.as_slice() {
        [all] => {
            let all = clamp_pane_padding(*all, warnings);
            Some((all, all, all, all))
        }
        [vertical, horizontal] => {
            let vertical = clamp_pane_padding(*vertical, warnings);
            let horizontal = clamp_pane_padding(*horizontal, warnings);
            Some((vertical, horizontal, vertical, horizontal))
        }
        [top, right, bottom, left] => Some((
            clamp_pane_padding(*top, warnings),
            clamp_pane_padding(*right, warnings),
            clamp_pane_padding(*bottom, warnings),
            clamp_pane_padding(*left, warnings),
        )),
        other => {
            warnings.push(format!(
                "Ignored pane.padding with {} value(s) (expected 1, 2, or 4)",
                other.len()
            ));
            None
        }
    }
}

fn parse_modifier(value: &str) -> Option<WmModifier> {
    match value.trim().to_ascii_lowercase().as_str() {
        "alt" | "mod" => Some(WmModifier::Alt),
        "super" | "meta" | "logo" | "win" | "windows" => Some(WmModifier::Super),
        _ => None,
    }
}

fn apply_animations(target: &mut WindowAnimationConfig, raw: AnimationFileConfig) {
    if let Some(value) = raw.enabled {
        target.enabled = value;
    }
    if let Some(value) = raw.spawn {
        target.spawn = value;
    }
    if let Some(value) = raw.close {
        target.close = value;
    }
    if let Some(value) = raw.fullscreen {
        target.fullscreen = value;
    }
    if let Some(value) = raw.tile_float {
        target.tile_float = value;
    }
    if let Some(value) = raw.axis_change {
        target.axis_change = value;
    }
    if let Some(value) = raw.focus_chrome {
        target.focus_chrome = value;
    }
    if let Some(value) = raw.geometry_ms {
        target.geometry_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.close_ms {
        target.close_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.focus_chrome_ms {
        target.focus_chrome_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.open_delay_ms {
        target.open_delay = Duration::from_millis(value);
    }
}
