use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Deserialize;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::input::Action;
use crate::keymap::{Keymap, Trigger};
use crate::state::{
    BarConfig, BarSegment, HyprmuxClipboardConfig, HyprmuxConfig, HyprmuxThemeConfig, InputConfig,
    SCRATCHPAD_MAX_HEIGHT, SCRATCHPAD_MIN_HEIGHT, ThemePreset, WmModifier,
};

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub path: PathBuf,
    pub found: bool,
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
            return LoadedConfig {
                config,
                path,
                found: false,
                warnings,
            };
        }
        Err(err) => {
            warnings.push(format!("Config read failed for {}: {err}", path.display()));
            return LoadedConfig {
                config,
                path,
                found: false,
                warnings,
            };
        }
    };

    let parsed = match toml::from_str::<FileConfig>(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(format!("Config parse failed for {}: {err}", path.display()));
            return LoadedConfig {
                config,
                path,
                found: false,
                warnings,
            };
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
            None => warnings.push(format!("Unknown theme preset `{preset}`; using one-dark")),
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

    LoadedConfig {
        config,
        path,
        found: true,
        warnings,
    }
}

pub fn load_initial_theme(config: &HyprmuxConfig) -> LoadedTheme {
    let fallback = config.theme.preset.theme();
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

impl Default for HyprmuxThemeConfig {
    fn default() -> Self {
        Self {
            preset: ThemePreset::OneDark,
            path: None,
        }
    }
}

impl Default for HyprmuxClipboardConfig {
    fn default() -> Self {
        Self { enable_osc52: true }
    }
}
