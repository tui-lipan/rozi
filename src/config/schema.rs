use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::state::{CapStyle, DEFAULT_SPLIT_WIDTH_MULTIPLIER, PaneBorderStyle, ThemePreset};

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
    /// colors from the host terminal), or the file stem of a custom theme in
    /// [`crate::config::themes_dir`].
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
    /// Name of a profile in [`crate::config::profiles_dir`] to load on startup when no CLI profile
    /// is given.
    pub default: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxLayoutConfig {
    /// Terminal cell height divided by cell width, used to compare tile dimensions visually.
    pub split_width_multiplier: f32,
}

impl Default for HyprmuxLayoutConfig {
    fn default() -> Self {
        Self {
            split_width_multiplier: DEFAULT_SPLIT_WIDTH_MULTIPLIER,
        }
    }
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

#[derive(Clone, Debug)]
pub struct HyprmuxSessionConfig {
    /// Persist the live layout on quit and restore it on next launch.
    pub autosave: bool,
    /// Override the session file location; defaults to `$XDG_STATE_HOME/hyprmux/session.toml`.
    pub path: Option<PathBuf>,
    /// Whether a bare launch attaches to an ephemeral session or opens the session picker first.
    pub startup: SessionStartup,
    /// Persist and restart named sessions after their server disappears.
    pub resurrect: bool,
}

impl Default for HyprmuxSessionConfig {
    fn default() -> Self {
        Self {
            autosave: false,
            path: None,
            startup: SessionStartup::default(),
            resurrect: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HyprmuxPaneConfig {
    /// Keep naturally exited panes in the layout so they can be respawned in place.
    pub hold_on_exit: bool,
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
    /// Whether `surface.backdrop` (canvas gaps, unfocused pane frames) always tracks the host
    /// terminal's own background instead of the active theme's authored value. Overrides any
    /// theme, including a custom file that already sets a concrete `backdrop`.
    pub background_follows_terminal: bool,
    /// App-wide border glyphs for tiled panes.
    pub border_style: PaneBorderStyle,
    /// Blank cells inserted between a pane's border and its terminal grid, as
    /// `(top, right, bottom, left)`. Purely cosmetic: each cell of padding costs a column/row of
    /// usable terminal space, so this stays off by default. Painted with the pane's frame
    /// background. Configured with CSS-style shorthand in the `[pane]` file schema.
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
            hold_on_exit: false,
            highlight_focused_background: false,
            highlight_focused_border: true,
            focus_on_hover: true,
            show_workbar: true,
            workbar_gap: true,
            workbar_at_bottom: false,
            show_titles: true,
            merge_borders: false,
            background_follows_terminal: false,
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
    pub bell: bool,
}

impl Default for HyprmuxNotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pane_exit: true,
            bell: true,
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
    /// are killed). Named sessions are detached safely and do not need confirmation.
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

/// `[shell_integration] mode` (cross-platform plan Phase 8): whether hyprmux injects its
/// OSC 7/133 shell-integration scripts into resolved-interactive-shell spawns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellIntegrationMode {
    /// Inject for a recognized shell (bash/zsh/fish today; PowerShell/cmd.exe stay documented
    /// opt-in per the plan even once Milestone 2 lands), unless an existing hyprmux or
    /// terminal-native integration is already loaded.
    #[default]
    Auto,
    /// Never inject; panes get whatever shell config/integration the user already has.
    Off,
}

impl ShellIntegrationMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxShellIntegrationConfig {
    pub mode: ShellIntegrationMode,
}

#[derive(Clone, Debug)]
pub struct HyprmuxConfig {
    /// Interactive-shell override (argument-preserving; first element is the program). `None`
    /// falls through to `$SHELL`/`/bin/sh` on Unix or `pwsh.exe`/`powershell.exe`/`%COMSPEC%`/
    /// `cmd.exe` on Windows - see [`crate::platform::command::resolve_interactive_shell`].
    pub shell: Option<Vec<String>>,
    /// Command-runner override (argument-preserving) used for one-off command lines: pane/popup
    /// commands, hooks, workbar `command:` segments, `[keys] run`, profile commands, and
    /// control-socket run requests. `None` falls through to a fixed, non-detection-based default
    /// (`["/bin/sh", "-c"]` on Unix, `[%COMSPEC%, "/D", "/S", "/C"]` on Windows) - see
    /// [`crate::platform::command::resolve_command_shell`].
    pub command_shell: Option<Vec<String>>,
    pub shell_integration: HyprmuxShellIntegrationConfig,
    pub cwd: Option<String>,
    pub scrollback: usize,
    pub input: InputConfig,
    pub animations: WindowAnimationConfig,
    pub theme: HyprmuxThemeConfig,
    pub profile: HyprmuxProfileConfig,
    pub session: HyprmuxSessionConfig,
    pub layout: HyprmuxLayoutConfig,
    pub pane: HyprmuxPaneConfig,
    pub clipboard: HyprmuxClipboardConfig,
    pub notifications: HyprmuxNotificationsConfig,
    pub navigation: HyprmuxNavigationConfig,
    pub confirm: HyprmuxConfirmConfig,
    pub scratchpad: HyprmuxScratchpadConfig,
    pub rules: Vec<HyprmuxRuleConfig>,
    pub hooks: HashMap<String, String>,
    pub logging: HyprmuxLoggingConfig,
    pub workbar: WorkbarConfig,
    /// Explicit `[keys]` overrides: command id -> native `KeyBinding` shortcuts. A command id
    /// present with an empty list is an explicit unbind; an id absent here uses the built-in
    /// defaults (see `crate::commands`).
    pub key_overrides: HashMap<String, Vec<KeyBinding>>,
    /// User-defined `[keys]` entries keyed by a literal trigger binding (rather than a built-in
    /// action id): each becomes its own generated command (see `crate::commands`).
    pub user_commands: Vec<UserCommand>,
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxLoggingConfig {
    pub dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HyprmuxRuleConfig {
    pub matches: String,
    pub float: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Zero-based workspace index.
    pub workspace: Option<usize>,
    pub focus: bool,
    pub fullscreen: bool,
}

/// What a user-defined keybinding does: `Run` spawns a new pane running the shell command;
/// `Send` writes literal text to the focused pane's PTY (TOML escapes like `\n` already work).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserCommandAction {
    Run(String),
    Send(String),
    Popup(String),
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
            UserCommandAction::Popup(command) => format!("Popup: {}", truncate_for_label(command)),
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
            command_shell: None,
            shell_integration: HyprmuxShellIntegrationConfig::default(),
            cwd: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
            scrollback: 5000,
            input: InputConfig::default(),
            animations: WindowAnimationConfig::default(),
            theme: HyprmuxThemeConfig::default(),
            profile: HyprmuxProfileConfig::default(),
            session: HyprmuxSessionConfig::default(),
            layout: HyprmuxLayoutConfig::default(),
            pane: HyprmuxPaneConfig::default(),
            clipboard: HyprmuxClipboardConfig::default(),
            notifications: HyprmuxNotificationsConfig::default(),
            navigation: HyprmuxNavigationConfig::default(),
            confirm: HyprmuxConfirmConfig::default(),
            scratchpad: HyprmuxScratchpadConfig::default(),
            rules: Vec::new(),
            hooks: HashMap::new(),
            logging: HyprmuxLoggingConfig::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
