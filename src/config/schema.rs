use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use tui_lipan::prelude::*;

use crate::layout::anim::WindowAnimationConfig;
use crate::state::{
    AlertMode, AlertPaint, DEFAULT_SPLIT_WIDTH_MULTIPLIER, PaneBorderMode, PaneBorderStyle,
    PaneTitlebarMode, ThemePreset,
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
    /// How reluctant the which-key strip is: off, or how long the prefix is held before the strip
    /// listing what the next key can be appears. Held WM-modifier chords resolve in one key and
    /// never go pending, so this only ever affects the leader scheme.
    pub which_key: WhichKey,
}

/// Whether the which-key strip appears at all, and how long the prefix is held before it does.
///
/// One ladder rather than a bool plus a delay: the only decision anyone actually makes here is how
/// reluctant the strip should be, from beating their muscle memory to never showing up. Named steps
/// rather than a millisecond field, because a free number invites tuning a value whose exact size
/// nobody can perceive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhichKey {
    /// The strip never appears. The `PREFIX` badge and the withheld pane caret are unaffected.
    Off,
    /// No delay - the strip appears with the `PREFIX` badge. Best while learning the keys, at the
    /// cost of a flash on every chord finished from muscle memory.
    Instant,
    /// Long enough that a chord typed without thinking never shows the strip, short enough that it
    /// is already there when you pause to think.
    #[default]
    Short,
    /// Only appears once you have visibly stopped, so the strip stays out of the way until asked.
    Long,
}

impl WhichKey {
    /// Cycle order for the Settings row, ascending in how long the strip waits. `Off` leads so it
    /// sits one step from `Instant` in one direction and one from `Long` in the other.
    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Instant, Self::Short, Self::Long]
    }

    /// One spelling per step; an unrecognized value warns and leaves the default in place.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "instant" => Some(Self::Instant),
            "short" => Some(Self::Short),
            "long" => Some(Self::Long),
            _ => None,
        }
    }

    /// The config spelling, so `parse(which_key.id())` round-trips.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Instant => "instant",
            Self::Short => "short",
            Self::Long => "long",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Instant => "Instant",
            Self::Short => "Short",
            Self::Long => "Long",
        }
    }

    /// Whether the strip is drawn at all. The view checks this before ever reading the runtime's
    /// revealed flag, which is what lets [`Self::reveal_delay`] stay a plain `Duration`.
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// `Short` sits near a typed chord's own duration, so it reads as "you hesitated" rather than
    /// "you were slow". `Long` is roughly double that: past any hesitation, into a deliberate stop.
    ///
    /// `Off` has no meaningful delay - nothing waits on the revealed flag - so it reports zero
    /// rather than parking a timer whose result is discarded.
    pub fn reveal_delay(self) -> std::time::Duration {
        std::time::Duration::from_millis(match self {
            Self::Off | Self::Instant => 0,
            Self::Short => 300,
            Self::Long => 750,
        })
    }

    pub fn step(self, reverse: bool) -> Self {
        let choices = Self::all();
        let index = choices
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or_default();
        let offset = if reverse { choices.len() - 1 } else { 1 };
        choices[(index + offset) % choices.len()]
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            prefix: KeyBinding::from_str("ctrl-a").expect("default prefix key parses"),
            modifier: WmModifier::Alt,
            modifier_shortcuts: true,
            which_key: WhichKey::default(),
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
pub struct ThemeConfig {
    /// Name of the active theme: a built-in preset id, the reserved name `system` (derive
    /// colors from the host terminal), or the file stem of a custom theme in
    /// [`crate::config::themes_dir`].
    /// A custom file shadows a built-in of the same name.
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: ThemePreset::Rozi.id().to_string(),
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
pub struct ProfileConfig {
    /// Name of a profile in [`crate::config::profiles_dir`] to load on startup when no CLI profile
    /// is given.
    pub default: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutConfig {
    /// Terminal cell height divided by cell width, used to compare tile dimensions visually.
    pub split_width_multiplier: f32,
    /// Layout mode every fresh workspace starts in. Profiles override this per workspace.
    pub default: crate::state::LayoutKind,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            split_width_multiplier: DEFAULT_SPLIT_WIDTH_MULTIPLIER,
            default: crate::state::LayoutKind::Dwindle,
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
    /// Open the session named after `[profile] default`, exactly as `rozi <that name>` would:
    /// attach when it is running, otherwise create it from its canonical same-name profile. With no
    /// default configured, or nothing to open under that name, fall through to the picker.
    Profile,
}

impl SessionStartup {
    /// Cycle order for the Settings row, ascending in how much it decides for you: ask, scratch,
    /// wherever you were, one fixed workplace.
    pub fn all() -> &'static [Self] {
        &[Self::Picker, Self::Ephemeral, Self::Last, Self::Profile]
    }

    /// The modes the Settings row offers. `Profile` is withheld while no default profile is set,
    /// since it has nothing to open then; starring one in Profiles brings it back. A row is a single
    /// visible value rather than a list, so there is nothing to grey out the way a dependent row is.
    pub fn choices(default_profile_set: bool) -> Vec<Self> {
        Self::all()
            .iter()
            .copied()
            .filter(|mode| default_profile_set || *mode != Self::Profile)
            .collect()
    }

    /// One spelling per mode: an unrecognized value warns and leaves the default in place rather
    /// than resolving through aliases that describe a different mode than they name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ephemeral" => Some(Self::Ephemeral),
            "picker" => Some(Self::Picker),
            "last" => Some(Self::Last),
            "profile" => Some(Self::Profile),
            _ => None,
        }
    }

    /// The config spelling, so `parse(startup.id())` round-trips.
    pub fn id(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::Picker => "picker",
            Self::Last => "last",
            Self::Profile => "profile",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ephemeral => "Ephemeral",
            Self::Picker => "Picker",
            Self::Last => "Last",
            Self::Profile => "Profile",
        }
    }

    pub fn next(self) -> Self {
        self.step_in(Self::all(), false)
    }

    pub fn prev(self) -> Self {
        self.step_in(Self::all(), true)
    }

    /// Step within an offered subset. A value the subset no longer contains (a config that says
    /// `profile` after its default profile went away) is not a position to move from, so either
    /// direction lands on the nearest end instead of silently staying put.
    pub fn step_in(self, choices: &[Self], reverse: bool) -> Self {
        let Some(first) = choices.first().copied() else {
            return self;
        };
        let Some(index) = choices.iter().position(|mode| *mode == self) else {
            return if reverse {
                choices[choices.len() - 1]
            } else {
                first
            };
        };
        let offset = if reverse { choices.len() - 1 } else { 1 };
        choices[(index + offset) % choices.len()]
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Persist the live layout on quit and restore it on next launch.
    pub autosave: bool,
    /// Override the session file location; defaults to `$XDG_STATE_HOME/rozi/session.toml`.
    pub path: Option<PathBuf>,
    /// Whether a bare launch opens the session picker (the default), attaches to an ephemeral
    /// session, or reopens the last named session.
    pub startup: SessionStartup,
    /// Persist and restart named sessions after their server disappears.
    pub resurrect: bool,
    /// Let a writable follower take layout control immediately instead of waiting for the current
    /// controller to grant its request.
    ///
    /// Defaults to `true`. Every client that can attach at all is already the same OS account (the
    /// endpoint is per-user), so this is a politeness policy between equally trusted clients rather
    /// than a permission boundary — and the common multi-client case is one person on two machines,
    /// where waiting to be granted means walking to the other keyboard. Taking control is
    /// symmetric and instantly reversible; set `false` for a session shared with another person,
    /// or attach that person with `--read-only`.
    pub allow_takeover: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            autosave: false,
            path: None,
            startup: SessionStartup::default(),
            resurrect: true,
            allow_takeover: true,
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
pub struct RemoteConfig {
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

impl Default for RemoteConfig {
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
pub struct PaneConfig {
    /// Minimum interval between PTY resize batches. Zero forwards geometry reports immediately.
    pub resize_debounce_ms: u64,
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
    /// Whether panes use separate frames, merged frames, no borders, or internal dividers.
    pub border_mode: PaneBorderMode,
    /// Whether configured pane alert colors are drawn on pane borders.
    pub alert_border: AlertMode,
    /// Per-state theme roles for pane-alert borders. `None` disables that state.
    pub alert_colors: PaneAlertColors,
    /// Keep double frames around floating panes, popups, and the scratchpad when the selected
    /// border mode otherwise disables per-pane frames. On by default: a floating pane, popup, or
    /// scratchpad is a layer above the tiles, and in `none`/`dividers` its frame is the only thing
    /// that says where it ends. Config-file only.
    pub keep_special_borders: bool,
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
    /// How opaque a toast's background is over the pane content it covers, in `[0.0, 1.0]`.
    ///
    /// The default `0.8` reads as tinted glass: the theme's panel color blended per cell with
    /// whatever is behind, so content underneath stays visible rather than being replaced. `1.0`
    /// paints the panel color solid.
    ///
    /// Below `1.0` the text contrast depends on what the toast covers, and themes differ widely in
    /// how much headroom their panel/text pair has. Measured over white, yellow, red, and dark
    /// panes across the 30 bundled themes, the worst case sits under the 4.5:1 readability floor
    /// on 17 of them at `0.8`, 7 at `0.9`, and 2 at `1.0` — and those last 2 are at their own
    /// theme's ceiling either way. Raise it on a theme whose toasts read poorly.
    pub toast_opacity: f32,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            resize_debounce_ms: 16,
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
            border_mode: PaneBorderMode::Separate,
            alert_border: AlertMode::Pulse,
            alert_colors: PaneAlertColors::default(),
            keep_special_borders: true,
            background_follows_terminal: false,
            border_style: PaneBorderStyle::Rounded,
            padding: (0, 0, 0, 0),
            title_style: CapStyle::Padded,
            workbar_badge_style: CapStyle::Padded,
            workbar_powerline: true,
            workbar_tab_style: CapStyle::Padded,
            workbar_style: CapStyle::Padded,
            toast_opacity: 0.8,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ClipboardConfig {
    pub enable_osc52: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self { enable_osc52: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationsConfig {
    pub enabled: bool,
    pub pane_exit: bool,
    pub pane_exit_error: bool,
    pub bell: bool,
    pub pane_blocked: bool,
    pub pane_done: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            // A clean natural exit is the one pane-exit edge that is not attendance-gated: the
            // pane is gone, so there is no pane left to attend. Announcing it tells the user what
            // they just watched happen, which is the same thing the toast policy forbids.
            pane_exit: false,
            pane_exit_error: true,
            bell: true,
            pane_blocked: true,
            pane_done: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundsConfig {
    pub enabled: bool,
    pub bell: bool,
    pub blocked: bool,
    pub done: bool,
    pub error: bool,
    pub throttle_ms: u64,
    pub bell_file: Option<PathBuf>,
    pub blocked_file: Option<PathBuf>,
    pub done_file: Option<PathBuf>,
    pub error_file: Option<PathBuf>,
    pub player: Option<String>,
}
impl Default for SoundsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bell: true,
            blocked: true,
            done: true,
            error: true,
            throttle_ms: 2000,
            bell_file: None,
            blocked_file: None,
            done_file: None,
            error_file: None,
            player: None,
        }
    }
}
impl SoundsConfig {
    pub fn enabled_for(&self, cue: crate::platform::sound::Cue) -> bool {
        match cue {
            crate::platform::sound::Cue::Bell => self.bell,
            crate::platform::sound::Cue::Blocked => self.blocked,
            crate::platform::sound::Cue::Done => self.done,
            crate::platform::sound::Cue::Error => self.error,
        }
    }
    pub fn file_for(&self, cue: crate::platform::sound::Cue) -> Option<&PathBuf> {
        match cue {
            crate::platform::sound::Cue::Bell => self.bell_file.as_ref(),
            crate::platform::sound::Cue::Blocked => self.blocked_file.as_ref(),
            crate::platform::sound::Cue::Done => self.done_file.as_ref(),
            crate::platform::sound::Cue::Error => self.error_file.as_ref(),
        }
    }
}

/// Seamless-navigation policy for the `smart-focus-*` actions: the set of foreground programs
/// that manage their own splits and should receive `Ctrl-h/j/k/l` themselves instead of having
/// rozi move pane focus. Modeled on vim-tmux-navigator's `is_vim` check (see
/// [docs/keybindings.md]); matching is case-insensitive against the pane's foreground process
/// name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationConfig {
    pub editors: Vec<String>,
}

impl Default for NavigationConfig {
    fn default() -> Self {
        Self {
            editors: DEFAULT_SPLIT_EDITORS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        }
    }
}

impl NavigationConfig {
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
pub struct ConfirmConfig {
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

impl Default for ConfirmConfig {
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
pub struct ScratchpadConfig {
    /// Command to run instead of the normal shell (e.g. `btop`); `None` uses the shell.
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// Height as a fraction of the viewport (clamped to `0.1..=0.9`).
    pub height: f32,
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            command: None,
            cwd: None,
            height: SCRATCHPAD_DEFAULT_HEIGHT,
        }
    }
}

/// `[shell_integration] mode` (cross-platform plan Phase 8): whether rozi injects its
/// OSC 7/133 shell-integration scripts into resolved-interactive-shell spawns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellIntegrationMode {
    /// Inject for a recognized shell (bash/zsh/fish today; PowerShell/cmd.exe stay documented
    /// opt-in per the plan even once Milestone 2 lands), unless an existing rozi or
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
pub struct ShellIntegrationConfig {
    pub mode: ShellIntegrationMode,
}

#[derive(Clone, Debug, Default)]
pub struct EnvironmentConfig {
    /// Additional client-process variables to copy into newly created local panes.
    pub forward: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
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
    pub shell_integration: ShellIntegrationConfig,
    pub environment: EnvironmentConfig,
    pub cwd: Option<String>,
    pub scrollback: usize,
    /// Ceiling on frames the client draws for content nothing asked it to redraw: every live pane
    /// is polled on this cadence, and animations advance on it. Lowering it trades smoothness for
    /// client CPU, which is the trade worth making over a slow link or on battery. The server's PTY
    /// reads are unaffected - output is coalesced into fewer repaints, never dropped.
    pub frame_rate: u16,
    /// Use Nerd Font private-use glyphs for decorative UI chrome: pane title icons, sidebar
    /// directory chevrons, and `round`/`arrow` end caps. File-kind icons in the sidebar still need
    /// the per-tab `icons` flag as well. On by default so existing installs keep their current look.
    pub nerd_icons: bool,
    pub input: InputConfig,
    pub animations: WindowAnimationConfig,
    pub theme: ThemeConfig,
    pub profile: ProfileConfig,
    pub session: SessionConfig,
    pub remote: RemoteConfig,
    pub layout: LayoutConfig,
    pub pane: PaneConfig,
    pub clipboard: ClipboardConfig,
    pub notifications: NotificationsConfig,
    pub sounds: SoundsConfig,
    pub navigation: NavigationConfig,
    pub confirm: ConfirmConfig,
    pub scratchpad: ScratchpadConfig,
    pub sidebar: SidebarConfig,
    pub rules: Vec<RuleConfig>,
    pub hints: Vec<HintConfig>,
    pub hooks: Vec<HookConfig>,
    pub commands: Vec<NamedCommand>,
    pub services: Vec<ServiceConfig>,
    /// Agent definitions from `[[agents]]` and from installed extensions, ahead of the built-in
    /// catalog. Detection happens in the session server, so this is what the controller hands it;
    /// see [`crate::agent_detection::AgentCatalog`].
    pub agents: Vec<crate::agent_detection::AgentDefinition>,
    /// Stable IDs of extensions that are valid, compatible, unique, and enabled for this load.
    pub active_extensions: HashSet<String>,
    /// Chords extensions asked for and got: command id -> bindings. Separate from
    /// [`Self::key_overrides`], which means "the user bound this by hand" and outranks these.
    pub extension_key_defaults: HashMap<String, Vec<KeyBinding>>,
    /// Stable IDs of every extension present on disk, whatever its status. Wider than
    /// [`Self::active_extensions`] on purpose: a durable sidebar placement naming a tab from a
    /// disabled or currently broken extension is kept, and only one that is gone gets pruned.
    pub installed_extensions: HashSet<String>,
    /// Process-facing definitions used to preserve or rotate opaque runtime fencing tokens.
    pub(crate) extension_runtime:
        std::collections::BTreeMap<String, super::extensions::ExtensionRuntimeFingerprint>,
    pub logging: LoggingConfig,
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
pub struct HookConfig {
    pub event: crate::events::EventKind,
    pub run: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ServiceRestart {
    #[default]
    OnFailure,
    Always,
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceConfig {
    pub name: String,
    pub launch: ServiceLaunch,
    pub cwd: Option<String>,
    pub restart: ServiceRestart,
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceLaunch {
    Direct(Vec<String>),
    Shell(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedCommand {
    pub id: String,
    pub label: Option<String>,
    pub action: UserCommandAction,
    pub category: String,
    pub env: Vec<(String, String)>,
    /// Key steps an extension suggests after the leader prefix. A suggestion only: it is resolved
    /// into [`Config::extension_key_defaults`] at load, and dropped there if anything already
    /// answers to that chord. Always `None` for a `config.toml` command, which has `[keys]`.
    pub default_key: Option<String>,
}

impl NamedCommand {
    pub fn label(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        user_command_action_label(&self.action)
    }
}

/// Default ceiling on one pane log file. Generous enough that an ordinary logged session never
/// reaches it, small enough that a runaway pane cannot quietly fill the state directory.
pub const DEFAULT_LOG_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LoggingConfig {
    pub dir: Option<PathBuf>,
    /// Size ceiling for one pane log file. `0` disables the cap and restores unbounded growth.
    pub max_bytes: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            dir: None,
            max_bytes: DEFAULT_LOG_MAX_BYTES,
        }
    }
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
pub struct RuleConfig {
    pub matcher: RuleMatcher,
    pub float: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Where a floating pane is placed. Ignored unless [`Self::float`] is set.
    pub position: FloatPosition,
    /// Zero-based workspace index.
    pub workspace: Option<usize>,
    pub focus: bool,
    pub fullscreen: bool,
}

/// Where a rule-spawned (or `spawn-float`) floating pane sits on the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatPosition {
    #[default]
    Center,
    /// Center of the pane at the last mouse pointer. Falls back to canvas center when no pointer
    /// has been seen this run.
    Cursor,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl FloatPosition {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "center" => Self::Center,
            "cursor" => Self::Cursor,
            "top-left" => Self::TopLeft,
            "top" => Self::Top,
            "top-right" => Self::TopRight,
            "left" => Self::Left,
            "right" => Self::Right,
            "bottom-left" => Self::BottomLeft,
            "bottom" => Self::Bottom,
            "bottom-right" => Self::BottomRight,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Cursor => "cursor",
            Self::TopLeft => "top-left",
            Self::Top => "top",
            Self::TopRight => "top-right",
            Self::Left => "left",
            Self::Right => "right",
            Self::BottomLeft => "bottom-left",
            Self::Bottom => "bottom",
            Self::BottomRight => "bottom-right",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HintConfig {
    pub pattern: regex_lite::Regex,
    pub open: bool,
}

impl PartialEq for HintConfig {
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
    Run {
        command: String,
        keep_open: bool,
    },
    Send(String),
    Popup {
        command: String,
        keep_open: bool,
    },
    /// Run detached with no pane and no popup, output discarded.
    ///
    /// For a command whose whole result is a side effect - it drives rozi over the control socket,
    /// or hands off to another program - a pane is pure cost: the layout opens and closes around
    /// output nobody reads. `keep_open` has no meaning here because nothing is held open. A
    /// non-zero exit still raises an error toast, so this is quiet rather than silent.
    Exec {
        command: String,
    },
    /// Run an argv vector directly without a command shell.
    ExecDirect {
        argv: Vec<String>,
    },
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
    pub fn target(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Run { command, .. } | Self::Popup { command, .. } | Self::Exec { command } => {
                std::borrow::Cow::Borrowed(command)
            }
            Self::Send(text) => std::borrow::Cow::Borrowed(text),
            Self::ExecDirect { argv } => std::borrow::Cow::Owned(argv.join(" ")),
        }
    }
}

pub const SIDEBAR_MIN_WIDTH: u16 = 16;
pub const SIDEBAR_MAX_WIDTH: u16 = 80;
pub const SIDEBAR_MIN_SPLIT_RATIO: f32 = 0.15;
pub const SIDEBAR_MAX_SPLIT_RATIO: f32 = 0.85;
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
    /// Section this entry belongs under, or `None` for the unheaded run at the top of the tab.
    /// Entries are clustered by it at parse time, so the stored order is already display order.
    pub group: Option<String>,
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
    /// Show file-kind icons. Off by default: the glyphs assume a Nerd Font. Also requires the
    /// global `nerd_icons` switch; with that off, this flag has no effect.
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
            show_hidden: true,
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
    Activity,
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
        /// Environment an extension's tab passes to whatever its rows launch. Empty for a
        /// `config.toml` tab, which runs as the user with nothing added.
        env: Vec<(String, String)>,
    },
    Command {
        name: SidebarTabId,
        label: String,
        command: String,
        interval_secs: u64,
        on_click: Option<UserCommandAction>,
        /// Marks which output lines are section headers rather than rows. A line starting with it
        /// renders as a header with the prefix stripped; without it every line is an ordinary row,
        /// which is what a command that knows nothing about rozi produces.
        group_prefix: Option<String>,
        /// Environment for the polling process and for `on_click`. Empty for a `config.toml` tab.
        env: Vec<(String, String)>,
    },
}

impl SidebarTab {
    /// A file-tree tab at that view's own defaults.
    pub fn tree(view: SidebarTreeView) -> Self {
        Self::Tree {
            view,
            config: SidebarTreeConfig::for_view(view),
        }
    }

    pub fn id(&self) -> SidebarTabId {
        match self {
            Self::Activity => SidebarTabId::new("activity"),
            Self::Panes => SidebarTabId::new("panes"),
            Self::Sessions => SidebarTabId::new("sessions"),
            Self::Tree { view, .. } => SidebarTabId::new(view.id()),
            Self::Launcher { name, .. } | Self::Command { name, .. } => name.clone(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Activity => "Activity",
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

    /// Environment the tab's own processes receive. Only an extension's tab carries any: it is how
    /// a contributed tab is told where it is installed and what the user configured.
    pub fn env(&self) -> &[(String, String)] {
        match self {
            Self::Launcher { env, .. } | Self::Command { env, .. } => env,
            _ => &[],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarConfig {
    pub visible: bool,
    pub width: u16,
    pub position: SidebarPosition,
    pub tabs: Vec<SidebarTab>,
    /// Durable tab placement for the top and optional bottom panel.
    pub panels: Vec<Vec<SidebarTabId>>,
    /// Whether the saved panel placement is rendered as a vertical split.
    pub split: bool,
    /// Fraction of the split sidebar height assigned to the top panel.
    pub split_ratio: f32,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        // Two panels out of the box: the session's own state on top, the repository below. The
        // trees cost nothing until their tab is the active one, so carrying them by default only
        // spends sidebar rows, not work.
        let tabs = vec![
            SidebarTab::Activity,
            SidebarTab::Panes,
            SidebarTab::Sessions,
            SidebarTab::tree(SidebarTreeView::Files),
            SidebarTab::tree(SidebarTreeView::Changes),
        ];
        Self {
            visible: false,
            width: 32,
            position: SidebarPosition::Left,
            panels: vec![
                vec![
                    SidebarTab::Activity.id(),
                    SidebarTab::Panes.id(),
                    SidebarTab::Sessions.id(),
                ],
                vec![
                    SidebarTabId::new(SidebarTreeView::Files.id()),
                    SidebarTabId::new(SidebarTreeView::Changes.id()),
                ],
            ],
            tabs,
            split: true,
            // The bottom panel starts larger: a repository has far more rows to show than a
            // session has agents and panes.
            split_ratio: 0.4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserCommand {
    pub action: UserCommandAction,
    /// Every chord this command answers to. A bare key step expands through the input scheme the
    /// same way a built-in action's default does, so one entry yields both the prefix chord and
    /// the held-modifier chord; an explicitly-written chord binds only itself.
    pub bindings: Vec<KeyBinding>,
    /// What the palette and help overlay print for the shortcut. A scheme-expanded command shows
    /// the bare key, matching how a built-in renders, rather than spelling out one of its forms.
    pub hint: String,
    /// Operator-supplied name from `[keys]`, used instead of the generated `Run: <command>`.
    pub label: Option<String>,
}

impl UserCommand {
    /// Human-facing description for the help overlay and command palette, since these have no
    /// static label of their own the way a built-in command does.
    pub fn label(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        user_command_action_label(&self.action)
    }
}

fn user_command_action_label(action: &UserCommandAction) -> String {
    match action {
        UserCommandAction::Run { command, .. } => {
            format!("Run: {}", truncate_for_label(command))
        }
        UserCommandAction::Send(text) => {
            format!("Send: {}", truncate_for_label(&escape_for_label(text)))
        }
        UserCommandAction::Popup { command, .. } => {
            format!("Popup: {}", truncate_for_label(command))
        }
        UserCommandAction::Exec { command } => {
            format!("Exec: {}", truncate_for_label(command))
        }
        UserCommandAction::ExecDirect { argv } => {
            format!("Exec: {}", truncate_for_label(&argv.join(" ")))
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

pub fn truncate_for_label(text: &str) -> String {
    const MAX_LEN: usize = 40;
    if text.chars().count() <= MAX_LEN {
        text.to_string()
    } else {
        let mut truncated: String = text.chars().take(MAX_LEN).collect();
        truncated.push('…');
        truncated
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            command_shell: None,
            shell_integration: ShellIntegrationConfig::default(),
            environment: EnvironmentConfig::default(),
            cwd: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
            scrollback: 5000,
            frame_rate: DEFAULT_FRAME_RATE,
            nerd_icons: true,
            input: InputConfig::default(),
            animations: WindowAnimationConfig::default(),
            theme: ThemeConfig::default(),
            profile: ProfileConfig::default(),
            session: SessionConfig::default(),
            remote: RemoteConfig::default(),
            layout: LayoutConfig::default(),
            pane: PaneConfig::default(),
            clipboard: ClipboardConfig::default(),
            notifications: NotificationsConfig::default(),
            sounds: SoundsConfig::default(),
            navigation: NavigationConfig::default(),
            confirm: ConfirmConfig::default(),
            scratchpad: ScratchpadConfig::default(),
            sidebar: SidebarConfig::default(),
            rules: Vec::new(),
            hints: Vec::new(),
            hooks: Vec::new(),
            commands: Vec::new(),
            services: Vec::new(),
            agents: Vec::new(),
            active_extensions: HashSet::new(),
            installed_extensions: HashSet::new(),
            extension_key_defaults: HashMap::new(),
            extension_runtime: std::collections::BTreeMap::new(),
            logging: LoggingConfig::default(),
            workbar: WorkbarConfig::default(),
            key_overrides: HashMap::new(),
            user_commands: Vec::new(),
        }
    }
}

impl Config {
    /// Round and arrow caps need a Nerd Font; with [`Self::nerd_icons`] off they render as padded.
    /// Half-block caps stay as they are: they use standard Unicode, not the Nerd Font PUA.
    pub fn effective_cap_style(&self, style: CapStyle) -> CapStyle {
        match style {
            CapStyle::Round | CapStyle::Arrow if !self.nerd_icons => CapStyle::Padded,
            other => other,
        }
    }

    /// Pane title icon for tiled / floating / fullscreen chrome. Empty when nerd icons are off so
    /// the title does not spend columns on a missing glyph or a substitute badge.
    pub fn pane_title_icon(&self, fullscreen: bool, floating: bool) -> &'static str {
        if !self.nerd_icons {
            return "";
        }
        if fullscreen {
            "󰊓"
        } else if floating {
            "󰹙"
        } else {
            "󰖲"
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

/// Per-state theme roles for pane-alert borders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneAlertColors {
    pub blocked: Option<BadgeColor>,
    pub finished: Option<BadgeColor>,
    pub working: Option<BadgeColor>,
    pub idle: Option<BadgeColor>,
}

impl Default for PaneAlertColors {
    fn default() -> Self {
        Self {
            blocked: Some(BadgeColor::Error),
            finished: Some(BadgeColor::Success),
            working: None,
            idle: None,
        }
    }
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
    pub alert: WorkbarAlertConfig,
}

/// Workspace tab alerts. The flags say *which* states mark a tab, `mode` whether the mark holds
/// still or breathes, and `paint` what it colors — three independent axes, mirroring `[pane.alert]`
/// colors plus `[pane] alert_border` on the pane side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkbarAlertConfig {
    pub bell: bool,
    pub blocked: bool,
    pub finished: bool,
    pub working: bool,
    pub idle: bool,
    pub mode: AlertMode,
    pub paint: AlertPaint,
}

impl Default for WorkbarAlertConfig {
    fn default() -> Self {
        Self {
            bell: true,
            blocked: true,
            finished: true,
            working: false,
            idle: false,
            mode: AlertMode::Pulse,
            paint: AlertPaint::Background,
        }
    }
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
            alert: WorkbarAlertConfig::default(),
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

    /// Unique `(command, interval_secs)` pairs across both workbar sides, one scheduled run per
    /// distinct command string even if it appears in multiple segments.
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
    use crate::state::{next_cap_style, parse_cap_style};

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
            SessionStartup::parse(" picker "),
            Some(SessionStartup::Picker)
        );
        assert_eq!(SessionStartup::parse("last"), Some(SessionStartup::Last));
        assert_eq!(
            SessionStartup::parse("profile"),
            Some(SessionStartup::Profile)
        );
        assert_eq!(SessionStartup::parse("nonsense"), None);
        // Each mode has exactly one spelling: `attach` named a mode that attaches nothing and
        // `default` named the mode that is no longer the default, so both are rejected.
        assert_eq!(SessionStartup::parse("default"), None);
        assert_eq!(SessionStartup::parse("attach"), None);
        assert_eq!(SessionStartup::parse("pick"), None);
        assert_eq!(SessionStartup::parse("choose"), None);
    }

    /// The Settings row steps through this enum and writes `id()` back, so the ring has to be
    /// closed in both directions and every spelling has to parse back to the same mode.
    #[test]
    fn session_startup_cycles_through_every_mode_and_round_trips_its_id() {
        let mut mode = SessionStartup::Picker;
        for _ in 0..SessionStartup::all().len() {
            assert_eq!(SessionStartup::parse(mode.id()), Some(mode));
            assert_eq!(mode.next().prev(), mode);
            mode = mode.next();
        }
        assert_eq!(mode, SessionStartup::Picker, "next() must close the ring");
        assert_eq!(
            SessionStartup::Picker.prev(),
            SessionStartup::Profile,
            "prev() must wrap the other way"
        );
    }

    /// Without a default profile there is nothing for `profile` mode to open, so the Settings row
    /// does not offer it. A config that already selected it still has to be steppable out of.
    #[test]
    fn session_startup_offers_profile_mode_only_with_a_default_profile() {
        let with = SessionStartup::choices(true);
        let without = SessionStartup::choices(false);
        assert!(with.contains(&SessionStartup::Profile));
        assert!(!without.contains(&SessionStartup::Profile));
        assert_eq!(without.len(), SessionStartup::all().len() - 1);

        // Ring of three: the last offered mode wraps to the first, skipping `profile` entirely.
        assert_eq!(
            SessionStartup::Last.step_in(&without, false),
            SessionStartup::Picker
        );
        assert_eq!(
            SessionStartup::Picker.step_in(&without, true),
            SessionStartup::Last
        );

        // Stranded on a mode that is no longer offered: both directions move to an offered one.
        assert_eq!(
            SessionStartup::Profile.step_in(&without, false),
            SessionStartup::Picker
        );
        assert_eq!(
            SessionStartup::Profile.step_in(&without, true),
            SessionStartup::Last
        );
    }

    #[test]
    fn user_command_label_describes_run_and_send() {
        let run = UserCommand {
            action: UserCommandAction::run("lazygit".to_string()),
            bindings: vec![KeyBinding::from_str("ctrl-a g").unwrap()],
            hint: "ctrl+a g".to_string(),
            label: None,
        };
        assert_eq!(run.label(), "Run: lazygit");

        let send = UserCommand {
            action: UserCommandAction::Send("ls -la\n".to_string()),
            bindings: vec![KeyBinding::from_str("ctrl-a g").unwrap()],
            hint: "ctrl+a g".to_string(),
            label: None,
        };
        assert_eq!(send.label(), "Send: ls -la\\n");
    }

    #[test]
    fn user_command_label_truncates_long_commands() {
        let run = UserCommand {
            action: UserCommandAction::run("x".repeat(60)),
            bindings: vec![KeyBinding::from_str("ctrl-a g").unwrap()],
            hint: "ctrl+a g".to_string(),
            label: None,
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
        assert!(PaneConfig::default().workbar_powerline);
        assert_eq!(PaneConfig::default().resize_debounce_ms, 16);
        assert!(PaneConfig::default().show_titles);
        assert_eq!(PaneConfig::default().border_mode, PaneBorderMode::Separate);
        assert!(PaneConfig::default().keep_special_borders);
        assert!(PaneConfig::default().highlight_focused_titlebar);
    }

    #[test]
    fn pane_title_style_parses_aliases_and_cycles() {
        assert_eq!(parse_cap_style("padded"), Some(CapStyle::Padded));
        assert_eq!(parse_cap_style("Half Block"), Some(CapStyle::Half));
        assert_eq!(parse_cap_style("pill"), Some(CapStyle::Round));
        assert_eq!(parse_cap_style("powerline"), Some(CapStyle::Arrow));
        assert_eq!(parse_cap_style("nonsense"), None);
        assert_eq!(CapStyle::Padded.glyphs(), None);
        assert!(CapStyle::Round.glyphs().is_some());
        assert_eq!(next_cap_style(CapStyle::Arrow), CapStyle::Padded);
    }

    #[test]
    fn nerd_icons_default_on_and_force_round_arrow_caps_to_padded() {
        let mut config = Config::default();
        assert!(config.nerd_icons);
        assert_eq!(config.effective_cap_style(CapStyle::Round), CapStyle::Round);
        assert_eq!(config.effective_cap_style(CapStyle::Arrow), CapStyle::Arrow);
        assert_eq!(config.effective_cap_style(CapStyle::Half), CapStyle::Half);
        assert_eq!(config.pane_title_icon(false, false), "󰖲");
        assert_eq!(config.pane_title_icon(false, true), "󰹙");
        assert_eq!(config.pane_title_icon(true, false), "󰊓");

        config.nerd_icons = false;
        assert_eq!(
            config.effective_cap_style(CapStyle::Round),
            CapStyle::Padded
        );
        assert_eq!(
            config.effective_cap_style(CapStyle::Arrow),
            CapStyle::Padded
        );
        assert_eq!(config.effective_cap_style(CapStyle::Half), CapStyle::Half);
        assert_eq!(
            config.effective_cap_style(CapStyle::Padded),
            CapStyle::Padded
        );
        assert_eq!(config.pane_title_icon(false, false), "");
        assert_eq!(config.pane_title_icon(true, true), "");
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
        assert_eq!(
            PaneTitlebarMode::parse("inside"),
            Some(PaneTitlebarMode::Inset)
        );
        assert_eq!(PaneTitlebarMode::parse("off"), None);
        assert_eq!(PaneTitlebarMode::parse("nonsense"), None);
        assert!(PaneTitlebarMode::Bar.takes_outer_row());
        assert!(!PaneTitlebarMode::Integrated.takes_outer_row());
        // The inset strip lives inside the frame, so it never displaces the top border row.
        assert!(!PaneTitlebarMode::Inset.takes_outer_row());
        assert!(PaneTitlebarMode::Bar.fills_strip());
        // Both draw plain text over the pane, so neither has a strip for `title_style` to cap.
        assert!(!PaneTitlebarMode::Border.fills_strip());
        assert!(!PaneTitlebarMode::Inset.fills_strip());
        assert_eq!(PaneTitlebarMode::Integrated.next(), PaneTitlebarMode::Inset);
        assert_eq!(PaneTitlebarMode::Inset.next(), PaneTitlebarMode::Bar);
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
