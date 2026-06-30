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
    pub preset: ThemePreset,
    pub path: Option<PathBuf>,
}

impl Default for HyprmuxThemeConfig {
    fn default() -> Self {
        Self {
            preset: ThemePreset::Lipan,
            path: None,
        }
    }
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
            scratchpad: HyprmuxScratchpadConfig::default(),
            bar: BarConfig::default(),
            keymap: Keymap::default(),
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

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct LoadedTheme {
    pub theme: Theme,
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
    path: Option<String>,
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
    fn file_config_parses_profile_path() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [profile]
            path = "~/code/hyprmux/dev.toml"
            "#,
        )
        .expect("config parses");

        assert_eq!(
            parsed.profile.path.as_deref(),
            Some("~/code/hyprmux/dev.toml")
        );
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
    fn theme_upsert_adds_missing_section() {
        assert_eq!(
            upsert_theme_preset("scrollback = 100\n", "lipan"),
            "scrollback = 100\n\n[theme]\npreset = \"lipan\"\n"
        );
    }

    #[test]
    fn theme_upsert_replaces_preset_and_removes_custom_path() {
        let updated = upsert_theme_preset(
            "[theme]\npreset = \"dracula\"\npath = \"~/theme.toml\"\n\n[session]\nautosave = true\n",
            "system",
        );
        assert_eq!(
            updated,
            "[theme]\npreset = \"system\"\n\n[session]\nautosave = true\n"
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
        assert!(bar.right.is_empty());
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
    preset: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ClipboardFileConfig {
    enable_osc52: Option<bool>,
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

    if let Some(preset) = parsed.theme.preset {
        match ThemePreset::parse(&preset) {
            Some(preset) => config.theme.preset = preset,
            None => warnings.push(format!("Unknown theme preset `{preset}`; using lipan")),
        }
    }
    if let Some(path) = non_empty(parsed.theme.path) {
        config.theme.path = Some(expand_path(path));
    }
    if let Some(path) = non_empty(parsed.profile.path) {
        config.profile.path = Some(expand_path(path));
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

pub fn load_initial_theme(config: &HyprmuxConfig) -> LoadedTheme {
    let fallback = theme_for_preset(config.theme.preset);
    let mut warnings = Vec::new();
    let theme = if let Some(path) = &config.theme.path {
        match load_theme_from_toml(path, fallback.clone()) {
            Ok(theme) => theme,
            Err(err) => {
                warnings.push(format!("Theme load failed for {}: {err}", path.display()));
                fallback
            }
        }
    } else {
        fallback
    };
    LoadedTheme { theme, warnings }
}

pub fn theme_for_preset(preset: ThemePreset) -> Theme {
    if preset == ThemePreset::System {
        ThemePreset::Lipan.theme()
    } else {
        preset.theme()
    }
}

pub fn persist_theme_selection(preset: ThemePreset) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_theme_preset(&text, preset.id());
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

fn upsert_theme_preset(text: &str, preset_id: &str) -> String {
    let mut output = String::new();
    let mut in_theme = false;
    let mut saw_theme = false;
    let mut wrote_preset = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_theme && !wrote_preset {
                output.push_str(&format!("preset = \"{preset_id}\"\n"));
                wrote_preset = true;
            }
            in_theme = trimmed == "[theme]";
            saw_theme |= in_theme;
        }

        if in_theme
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "preset" | "path"))
        {
            if trimmed.starts_with("preset") && !wrote_preset {
                output.push_str(&format!("preset = \"{preset_id}\"\n"));
                wrote_preset = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_theme && !wrote_preset {
        output.push_str(&format!("preset = \"{preset_id}\"\n"));
    } else if !saw_theme {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[theme]\n");
        output.push_str(&format!("preset = \"{preset_id}\"\n"));
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
        if !parsed_bindings.is_empty() {
            keymap.clear_action(action);
            for (trigger, display) in parsed_bindings {
                keymap.bind(action, trigger, display);
            }
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
