use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Deserialize;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::input::Action;
use crate::keymap::{Keymap, Trigger};
use crate::state::ThemePreset;

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

impl Default for HyprmuxPaneConfig {
    fn default() -> Self {
        Self {
            highlight_focused_background: false,
            focus_on_hover: true,
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
    pub scratchpad: HyprmuxScratchpadConfig,
    pub bar: BarConfig,
    pub keymap: Keymap,
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
            scratchpad: HyprmuxScratchpadConfig::default(),
            bar: BarConfig::default(),
            keymap: Keymap::default(),
        }
    }
}

/// One segment of the configurable top bar. `Workspaces` is the workspace tab strip;
/// `Session` is the live attach-connection badge (invisible until attached to a named session);
/// `Text` is a literal with `{host}`/`{workspace}`/`{layout}`/`{session}` placeholders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarSegment {
    Title,
    Workspaces,
    Session,
    Clock,
    Layout,
    Activity,
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
            "activity" => Some(Self::Activity),
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
        // Badge + workspace tabs on the left; the session badge sits on the right but stays
        // invisible until an attach connection exists, so local mode looks unchanged.
        Self {
            left: vec![BarSegment::Title, BarSegment::Workspaces],
            right: vec![BarSegment::Session],
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

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct FileConfig {
    shell: Option<String>,
    cwd: Option<String>,
    scrollback: Option<usize>,
    modifier: Option<String>,
    prefix: Option<String>,
    input: InputFileConfig,
    animations: AnimationFileConfig,
    theme: ThemeFileConfig,
    profile: ProfileFileConfig,
    session: SessionFileConfig,
    pane: PaneFileConfig,
    clipboard: ClipboardFileConfig,
    notifications: NotificationsFileConfig,
    scratchpad: ScratchpadFileConfig,
    bar: BarFileConfig,
    keys: HashMap<String, KeyBindingSpec>,
}

/// A `[keys]` value: one binding string or a list of them.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyBindingSpec {
    One(String),
    Many(Vec<String>),
}

impl KeyBindingSpec {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
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
struct BarFileConfig {
    left: Option<Vec<String>>,
    right: Option<Vec<String>>,
    clock_format: Option<String>,
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
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PaneFileConfig {
    highlight_focused_background: Option<bool>,
    focus_on_hover: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_overlay_parses_held_and_prefix_bindings() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = ["alt-enter"]
            close = "prefix q"
            notanaction = "x"
            "#,
        )
        .expect("config parses");

        let prefix = InputConfig::default().prefix;
        let mut warnings = Vec::new();
        let keymap = build_keymap(parsed.keys, &prefix, &mut warnings);

        let alt_enter = KeyEvent {
            code: KeyCode::Enter,
            mods: KeyMods::ALT,
        };
        assert_eq!(keymap.held_action(alt_enter), Some(Action::Spawn));

        let q = KeyEvent {
            code: KeyCode::Char('q'),
            mods: KeyMods::NONE,
        };
        assert_eq!(keymap.prefix_action(q), Some(Action::Close));

        // Unknown action id yields exactly one warning and is skipped.
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("notanaction"));
    }

    #[test]
    fn empty_key_specs_clear_default_bindings() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = []
            spawn = ""
            "#,
        )
        .expect("config parses");

        let prefix = InputConfig::default().prefix;
        let mut warnings = Vec::new();
        let keymap = build_keymap(parsed.keys, &prefix, &mut warnings);

        assert_eq!(
            keymap.keys_for(Action::ToggleScratchpad).as_deref(),
            Some("")
        );
        assert_eq!(keymap.keys_for(Action::Spawn).as_deref(), Some(""));
        assert_eq!(
            keymap.prefix_action(KeyEvent {
                code: KeyCode::Char('`'),
                mods: KeyMods::NONE
            }),
            None
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn malformed_key_specs_clear_defaults_and_warn() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = "not-a-real-key"
            "#,
        )
        .expect("config parses");

        let prefix = InputConfig::default().prefix;
        let mut warnings = Vec::new();
        let keymap = build_keymap(parsed.keys, &prefix, &mut warnings);

        assert_eq!(
            keymap.keys_for(Action::ToggleScratchpad).as_deref(),
            Some("")
        );
        assert_eq!(
            keymap.prefix_action(KeyEvent {
                code: KeyCode::Char('`'),
                mods: KeyMods::NONE
            }),
            None
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("not-a-real-key"));
        assert!(warnings[0].contains("scratchpad"));
    }

    #[test]
    fn no_default_actions_are_bindable() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            toggle-pane-synchronization = "prefix s"
            save-profile = "alt-s"
            "#,
        )
        .expect("config parses");

        let prefix = InputConfig::default().prefix;
        let mut warnings = Vec::new();
        let keymap = build_keymap(parsed.keys, &prefix, &mut warnings);

        assert_eq!(
            keymap
                .keys_for(Action::TogglePaneSynchronization)
                .as_deref(),
            Some("Ctrl+A S")
        );
        assert_eq!(
            keymap.keys_for(Action::SaveProfile).as_deref(),
            Some("Alt+S")
        );
        assert_eq!(
            keymap.prefix_action(KeyEvent {
                code: KeyCode::Char('s'),
                mods: KeyMods::NONE
            }),
            Some(Action::TogglePaneSynchronization)
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
            focus_on_hover = false
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.pane.highlight_focused_background, Some(true));
        assert_eq!(parsed.pane.focus_on_hover, Some(false));
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
        assert_eq!(bar.right, vec![BarSegment::Session]);
        assert!(!bar.has_clock());
        assert_eq!(bar.clock_format, "%H:%M");
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InputFileConfig {
    modifier: Option<String>,
    prefix: Option<String>,
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

pub fn load_config() -> LoadedConfig {
    let path = config_path();
    let mut warnings = Vec::new();
    let mut config = HyprmuxConfig::default();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return LoadedConfig { config, warnings };
        }
        Err(err) => {
            warnings.push(format!("Config read failed for {}: {err}", path.display()));
            return LoadedConfig { config, warnings };
        }
    };

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
        parsed.modifier.or(parsed.input.modifier),
        parsed.prefix.or(parsed.input.prefix),
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
    if let Some(highlight_focused_background) = parsed.pane.highlight_focused_background {
        config.pane.highlight_focused_background = highlight_focused_background;
    }
    if let Some(focus_on_hover) = parsed.pane.focus_on_hover {
        config.pane.focus_on_hover = focus_on_hover;
    }
    if let Some(enable_osc52) = parsed.clipboard.enable_osc52 {
        config.clipboard.enable_osc52 = enable_osc52;
    }
    if let Some(enabled) = parsed.notifications.enabled {
        config.notifications.enabled = enabled;
    }
    if let Some(pane_exit) = parsed.notifications.pane_exit {
        config.notifications.pane_exit = pane_exit;
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

    apply_bar_config(&mut config.bar, parsed.bar, &mut warnings);
    config.keymap = build_keymap(parsed.keys, &config.input.prefix, &mut warnings);

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

pub fn persist_theme_name(name: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_theme_name(&text, name);
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
    fs::write(&path, updated)
        .map_err(|err| format!("Could not write config {}: {err}", path.display()))?;
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
    fs::write(&path, updated)
        .map_err(|err| format!("Could not write config {}: {err}", path.display()))?;
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
    fs::write(&path, updated)
        .map_err(|err| format!("Could not write config {}: {err}", path.display()))?;
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
    warnings: &mut Vec<String>,
) {
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

fn build_keymap(
    keys: HashMap<String, KeyBindingSpec>,
    prefix: &KeyBinding,
    warnings: &mut Vec<String>,
) -> Keymap {
    let mut keymap = Keymap::default();
    for (action_name, spec) in keys {
        let Some(action) = Action::from_id(&action_name) else {
            warnings.push(format!("Unknown key action `{action_name}`; skipped"));
            continue;
        };
        keymap.clear_action(action);
        keymap.mark_configured(action);
        let mut parsed_bindings = Vec::new();
        for binding in spec.into_vec() {
            for candidate in binding
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                match parse_binding(candidate, prefix) {
                    Some(parsed) => parsed_bindings.push(parsed),
                    None => warnings.push(format!(
                        "Could not parse binding `{candidate}` for `{action_name}`; skipped"
                    )),
                }
            }
        }
        for (trigger, display) in parsed_bindings {
            keymap.bind(action, trigger, display);
        }
    }
    keymap
}

/// Parse a binding string into a [`Trigger`] and its display text. A binding is a prefix
/// sequence when it starts with the literal `prefix` keyword or the configured prefix key
/// (e.g. `prefix c`, `ctrl-a c`); otherwise it is a held-modifier chord (e.g. `alt-enter`).
fn parse_binding(spec: &str, prefix: &KeyBinding) -> Option<(Trigger, String)> {
    let parts: Vec<&str> = spec.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    if parts.len() >= 2 {
        let starts_with_prefix = parts[0].eq_ignore_ascii_case("prefix")
            || KeyBinding::from_str(parts[0]).is_ok_and(|key| key == *prefix);
        if starts_with_prefix {
            let raw_key = parts[1..].join(" ");
            let key = KeyBinding::from_str(&raw_key).ok()?;
            if key.step_count() != 1 {
                return None;
            }
            let display = format!("{prefix} {key}");
            return Some((Trigger::Prefix(key), display));
        }
    }

    let key = KeyBinding::from_str(spec).ok()?;
    if key.step_count() != 1 {
        return None;
    }
    let display = key.to_string();
    Some((Trigger::Held(key), display))
}

fn apply_bar_config(bar: &mut BarConfig, raw: BarFileConfig, warnings: &mut Vec<String>) {
    fn parse_segments(
        raw: Vec<String>,
        region: &str,
        warnings: &mut Vec<String>,
    ) -> Vec<BarSegment> {
        raw.into_iter()
            .filter_map(|name| match BarSegment::parse(&name) {
                Some(segment) => Some(segment),
                None => {
                    warnings.push(format!("Unknown {region} bar segment `{name}`; skipped"));
                    None
                }
            })
            .collect()
    }

    if let Some(left) = raw.left {
        bar.left = parse_segments(left, "left", warnings);
    }
    if let Some(right) = raw.right {
        bar.right = parse_segments(right, "right", warnings);
    }
    if let Some(format) = non_empty(raw.clock_format) {
        // Reject invalid strftime so a clock segment can't panic at render time.
        if chrono::format::StrftimeItems::new(&format).parse().is_ok() {
            bar.clock_format = format;
        } else {
            warnings.push(format!(
                "Invalid clock_format `{format}`; keeping `{}`",
                bar.clock_format
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

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
