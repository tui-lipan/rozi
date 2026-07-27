use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::state::{
    CapStyle, DEFAULT_SPLIT_WIDTH_MULTIPLIER, PaneBorderStyle, PaneTitlebarMode, ThemePreset,
};

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

/// A resolved theme source: a built-in preset, the host-derived system theme, or a named custom
/// theme file in the themes directory. Some sources, such as the ANSI fallback, are resolvable
/// without appearing in the picker.
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

/// What a bare launch (no target/`--session`) does before opening the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionStartup {
    /// Silently attach to this process's ephemeral session.
    Ephemeral,
    /// Show the session picker first (when any candidate exists), so the user can reattach to a
    /// named session or start a fresh ephemeral one, without creating a session until they choose.
    /// Equivalent to passing `--pick`. With no candidate to pick, an ephemeral starts directly.
    #[default]
    Picker,
    /// Reopen the exact most recently used named session. When it is not available, open the
    /// picker with it highlighted rather than silently attaching an unrelated session.
    Last,
}

impl SessionStartup {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ephemeral" | "default" | "attach" => Some(Self::Ephemeral),
            "picker" | "pick" | "choose" => Some(Self::Picker),
            "last" => Some(Self::Last),
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
    /// Whether a bare launch opens the session picker (the default), attaches to an ephemeral
    /// session, or reopens the last named session.
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

/// When `--remote` finds no compatible binary on the far side (Phase 4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteInstallPolicy {
    /// Prompt interactively; never mutate in non-interactive mode.
    #[default]
    Prompt,
    Never,
    Always,
}

impl RemoteInstallPolicy {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "prompt" => Some(Self::Prompt),
            "never" => Some(Self::Never),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Per-alias `[remote.hosts.<name>]` settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHostConfig {
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub ssh_args: Vec<String>,
    pub binary_path: Option<String>,
}

/// `[remote]` configuration for SSH session attach.
#[derive(Clone, Debug)]
pub struct HyprmuxRemoteConfig {
    pub default_host: Option<String>,
    pub hosts: HashMap<String, RemoteHostConfig>,
    pub connection_timeout_secs: u64,
    pub server_alive_interval_secs: u64,
    pub server_alive_count_max: u64,
    pub install: RemoteInstallPolicy,
    /// Pass `BatchMode=yes` to ssh, refusing every interactive prompt.
    ///
    /// On by default: the attach transport hands ssh's stdin to the session protocol, so a
    /// password prompt there cannot be answered and would hang instead. Turning it off lets ssh
    /// prompt on the controlling terminal, which is useful for the CLI helpers
    /// (`list-sessions --remote`, `kill-session --remote`) and for passphrase-protected keys with
    /// no agent — at the cost of a prompt that can land on top of a running TUI.
    pub batch_mode: bool,
}

impl Default for HyprmuxRemoteConfig {
    fn default() -> Self {
        Self {
            default_host: None,
            hosts: HashMap::new(),
            connection_timeout_secs: 15,
            server_alive_interval_secs: 15,
            server_alive_count_max: 3,
            install: RemoteInstallPolicy::default(),
            batch_mode: true,
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
    /// Whether the focused pane uses the focused titlebar colors and emphasis.
    pub highlight_focused_titlebar: bool,
    /// Whether moving the mouse over a pane focuses it.
    pub focus_on_hover: bool,
    /// Whether the workbar (workspace tabs, mode chips, etc.) is shown.
    pub show_workbar: bool,
    /// Whether there is a 1-line gap between the workbar and the panes area.
    pub workbar_gap: bool,
    /// Whether the workbar is drawn on the last row (below the panes) instead of the first row.
    pub workbar_at_bottom: bool,
    /// Whether tiled/floating panes render their selected titlebar layout.
    pub show_titles: bool,
    /// Structural presentation for tiled/floating pane titles.
    pub titlebar: PaneTitlebarMode,
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
    /// End-cap style for workspace and sidebar tabs.
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
            highlight_focused_titlebar: true,
            focus_on_hover: true,
            show_workbar: true,
            workbar_gap: true,
            workbar_at_bottom: false,
            show_titles: true,
            titlebar: PaneTitlebarMode::Bar,
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
    pub pane_blocked: bool,
    pub pane_done: bool,
}

impl Default for HyprmuxNotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pane_exit: true,
            bell: true,
            pane_blocked: true,
            pane_done: false,
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
    /// Confirm before closing a temporary session at the leave prompt — the second `Enter` on an
    /// empty name, which shuts its server down and kills its PTYs. With this off, leaving without
    /// naming closes it on the first press. Named sessions are never closed by leaving, so they are
    /// unaffected either way.
    pub quit_ephemeral: bool,
    /// Confirm before discarding the current ephemeral session to start a fresh one (its panes
    /// are killed). Named sessions are detached safely and do not need confirmation.
    pub new_temporary_session: bool,
    /// Confirm before replacing a live disposable session from the profile picker.
    pub load_profile: bool,
}

impl Default for HyprmuxConfirmConfig {
    fn default() -> Self {
        Self {
            close_pane: false,
            kill_workspace: true,
            kill_session: true,
            quit_ephemeral: true,
            new_temporary_session: true,
            load_profile: true,
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
    pub remote: HyprmuxRemoteConfig,
    pub layout: HyprmuxLayoutConfig,
    pub pane: HyprmuxPaneConfig,
    pub clipboard: HyprmuxClipboardConfig,
    pub notifications: HyprmuxNotificationsConfig,
    pub navigation: HyprmuxNavigationConfig,
    pub confirm: HyprmuxConfirmConfig,
    pub scratchpad: HyprmuxScratchpadConfig,
    pub sidebar: SidebarConfig,
    pub rules: Vec<HyprmuxRuleConfig>,
    pub hints: Vec<HyprmuxHintConfig>,
    pub hooks: Vec<HyprmuxHookConfig>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyprmuxHookConfig {
    pub event: crate::events::EventKind,
    pub run: String,
}

#[derive(Clone, Debug, Default)]
pub struct HyprmuxLoggingConfig {
    pub dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum RuleMatcher {
    Substring(String),
    Regex(regex_lite::Regex),
}

impl PartialEq for RuleMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Substring(a), Self::Substring(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for RuleMatcher {}

impl RuleMatcher {
    pub fn matches(&self, command: &str) -> bool {
        match self {
            Self::Substring(needle) => command.contains(needle),
            Self::Regex(regex) => regex.is_match(command),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Substring(needle) => needle.clone(),
            Self::Regex(regex) => regex.as_str().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HyprmuxRuleConfig {
    pub matcher: RuleMatcher,
    pub float: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Zero-based workspace index.
    pub workspace: Option<usize>,
    pub focus: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Debug)]
pub struct HyprmuxHintConfig {
    pub pattern: regex_lite::Regex,
    pub open: bool,
}

impl PartialEq for HyprmuxHintConfig {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open && self.pattern.as_str() == other.pattern.as_str()
    }
}

/// What a user-defined keybinding does: `Run` spawns a new pane running the shell command;
/// `Send` writes literal text to the focused pane's PTY (TOML escapes like `\n` already work);
/// `Popup` runs the command in a centered floating pane.
///
/// `keep_open` preserves command output after exit instead of tearing the pane down. A `Run` pane
/// then becomes an interactive shell; a `Popup` remains as a read-only result. It defaults to
/// `true` because a user command names a specific thing to run, and its output is the reason it was
/// run at all. Long-lived programs that own the pane for their whole life (`nvim`, `lazygit`) are
/// the case worth setting `keep_open = false` on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserCommandAction {
    Run { command: String, keep_open: bool },
    Send(String),
    Popup { command: String, keep_open: bool },
}

impl UserCommandAction {
    /// A `run` action with the default hold-after-exit behavior.
    pub fn run(command: impl Into<String>) -> Self {
        Self::Run {
            command: command.into(),
            keep_open: true,
        }
    }

    /// A `popup` action with the default hold-after-exit behavior.
    pub fn popup(command: impl Into<String>) -> Self {
        Self::Popup {
            command: command.into(),
            keep_open: true,
        }
    }

    /// The command line or literal text the action carries, for labels and detail lines.
    pub fn target(&self) -> &str {
        match self {
            Self::Run { command, .. } | Self::Popup { command, .. } => command,
            Self::Send(text) => text,
        }
    }
}

pub const SIDEBAR_MIN_WIDTH: u16 = 16;
pub const SIDEBAR_MAX_WIDTH: u16 = 80;
pub const SIDEBAR_MIN_COMMAND_INTERVAL_SECS: u64 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarPosition {
    #[default]
    Left,
    Right,
}

impl SidebarPosition {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SidebarTabId(String);

impl SidebarTabId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarLauncherEntry {
    pub label: String,
    pub action: UserCommandAction,
}

/// Which projection a file-tree sidebar tab shows. The two tabs are one integration over the same
/// widget: `Files` browses the tree, `Changes` shows only paths git reports as changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarTreeView {
    Files,
    Changes,
}

impl SidebarTreeView {
    pub fn id(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Changes => "git",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Changes => "Git",
        }
    }
}

/// Where a file-tree tab is rooted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarTreeRoot {
    /// The focused pane's working directory: browse where you are.
    #[default]
    Cwd,
    /// The git repository containing that directory, so changes elsewhere in the repo are still
    /// visible from a subdirectory. Falls back to the working directory outside a repository.
    Repo,
}

impl SidebarTreeRoot {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cwd" | "pane" => Some(Self::Cwd),
            "repo" | "repository" | "root" => Some(Self::Repo),
            _ => None,
        }
    }
}

pub const SIDEBAR_TREE_MAX_ENTRIES_LIMIT: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarTreeConfig {
    pub root: SidebarTreeRoot,
    pub show_hidden: bool,
    /// Show file-kind icons. Off by default: the glyphs assume a Nerd Font, and the rest of the
    /// sidebar is text-only.
    pub icons: bool,
    /// Show the fuzzy-find input above the tree.
    pub explorer: bool,
    /// Show `+N -M` diff stats beside change markers.
    pub diff_stats: bool,
    pub max_entries: usize,
    /// What activating a row does. `{path}` is replaced with the activated path.
    pub on_click: Option<UserCommandAction>,
}

impl SidebarTreeConfig {
    /// Defaults per view: browsing wants the pane's directory and no change noise, while the
    /// changes view is only useful repo-wide and with diff stats on.
    pub fn for_view(view: SidebarTreeView) -> Self {
        Self {
            root: match view {
                SidebarTreeView::Files => SidebarTreeRoot::Cwd,
                SidebarTreeView::Changes => SidebarTreeRoot::Repo,
            },
            show_hidden: false,
            icons: false,
            explorer: false,
            diff_stats: view == SidebarTreeView::Changes,
            max_entries: 2_000,
            on_click: Some(UserCommandAction::Send("{path}".to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarTab {
    Agents,
    Panes,
    Sessions,
    Tree {
        view: SidebarTreeView,
        config: SidebarTreeConfig,
    },
    Launcher {
        name: SidebarTabId,
        label: String,
        entries: Vec<SidebarLauncherEntry>,
    },
    Command {
        name: SidebarTabId,
        label: String,
        command: String,
        interval_secs: u64,
        on_click: Option<UserCommandAction>,
    },
}

impl SidebarTab {
    pub fn id(&self) -> SidebarTabId {
        match self {
            Self::Agents => SidebarTabId::new("agents"),
            Self::Panes => SidebarTabId::new("panes"),
            Self::Sessions => SidebarTabId::new("sessions"),
            Self::Tree { view, .. } => SidebarTabId::new(view.id()),
            Self::Launcher { name, .. } | Self::Command { name, .. } => name.clone(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Agents => "Agents",
            Self::Panes => "Panes",
            Self::Sessions => "Sessions",
            Self::Tree { view, .. } => view.label(),
            Self::Launcher { label, .. } | Self::Command { label, .. } => label,
        }
    }

    /// Whether the tab body manages its own scrolling. The file tree scrolls internally, so the
    /// sidebar must not wrap it in a second scroll view.
    pub fn scrolls_itself(&self) -> bool {
        matches!(self, Self::Tree { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarConfig {
    pub visible: bool,
    pub width: u16,
    pub position: SidebarPosition,
    pub tabs: Vec<SidebarTab>,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            visible: false,
            width: 32,
            position: SidebarPosition::Left,
            tabs: vec![SidebarTab::Agents, SidebarTab::Panes, SidebarTab::Sessions],
        }
    }
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
            UserCommandAction::Run { command, .. } => {
                format!("Run: {}", truncate_for_label(command))
            }
            UserCommandAction::Send(text) => {
                format!("Send: {}", truncate_for_label(&escape_for_label(text)))
            }
            UserCommandAction::Popup { command, .. } => {
                format!("Popup: {}", truncate_for_label(command))
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
            remote: HyprmuxRemoteConfig::default(),
            layout: HyprmuxLayoutConfig::default(),
            pane: HyprmuxPaneConfig::default(),
            clipboard: HyprmuxClipboardConfig::default(),
            notifications: HyprmuxNotificationsConfig::default(),
            navigation: HyprmuxNavigationConfig::default(),
            confirm: HyprmuxConfirmConfig::default(),
            scratchpad: HyprmuxScratchpadConfig::default(),
            sidebar: SidebarConfig::default(),
            rules: Vec::new(),
            hints: Vec::new(),
            hooks: Vec::new(),
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
/// `Location` identifies the active remote host or retained remote count; `Session` is the live
/// attach-connection badge (invisible until attached to a named session);
/// `Text` is a literal with `{host}`/`{workspace}`/`{layout}`/`{session}` placeholders;
/// `Command` runs a shell command on a timer and shows the first line of its stdout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkbarSegment {
    Title,
    Workspaces,
    Location,
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
            "location" => Some(Self::Location),
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
        if let Some((secs, command)) = rest.split_once(':')
            && let Ok(interval_secs) = secs.trim().parse::<u64>()
        {
            return Self::Command {
                command: command.to_string(),
                interval_secs: interval_secs.max(1),
            };
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
            right: vec![
                WorkbarItem::new(WorkbarSegment::Location),
                WorkbarItem::new(WorkbarSegment::Session),
            ],
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
    fn session_startup_parses_known_values_and_defaults_to_picker() {
        assert_eq!(SessionStartup::default(), SessionStartup::Picker);
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
        assert_eq!(SessionStartup::parse("last"), Some(SessionStartup::Last));
        assert_eq!(SessionStartup::parse("nonsense"), None);
    }

    #[test]
    fn user_command_label_describes_run_and_send() {
        let run = UserCommand {
            action: UserCommandAction::run("lazygit".to_string()),
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
            action: UserCommandAction::run("x".repeat(60)),
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
            vec![
                WorkbarItem {
                    segment: WorkbarSegment::Location,
                    color: None,
                },
                WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                },
            ]
        );
        assert!(HyprmuxPaneConfig::default().workbar_powerline);
        assert!(HyprmuxPaneConfig::default().show_titles);
        assert!(HyprmuxPaneConfig::default().highlight_focused_titlebar);
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
    fn pane_titlebar_modes_parse_and_cycle() {
        assert_eq!(PaneTitlebarMode::parse("bar"), Some(PaneTitlebarMode::Bar));
        assert_eq!(
            PaneTitlebarMode::parse("frame"),
            Some(PaneTitlebarMode::Border)
        );
        assert_eq!(
            PaneTitlebarMode::parse("compact"),
            Some(PaneTitlebarMode::Integrated)
        );
        assert_eq!(PaneTitlebarMode::parse("off"), None);
        assert_eq!(PaneTitlebarMode::parse("nonsense"), None);
        assert!(PaneTitlebarMode::Bar.takes_title_row());
        assert!(!PaneTitlebarMode::Integrated.takes_title_row());
        assert_eq!(PaneTitlebarMode::Integrated.next(), PaneTitlebarMode::Bar);
    }

    #[test]
    fn workbar_segment_parses_builtins_and_text_literals() {
        assert_eq!(WorkbarSegment::parse("clock"), Some(WorkbarSegment::Clock));
        assert_eq!(
            WorkbarSegment::parse("location"),
            Some(WorkbarSegment::Location)
        );
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
            vec![
                WorkbarItem::new(WorkbarSegment::Location),
                WorkbarItem::new(WorkbarSegment::Session),
            ]
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
