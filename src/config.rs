use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
use crate::state::{
    HyprmuxClipboardConfig, HyprmuxConfig, HyprmuxThemeConfig, InputConfig, ThemePreset, WmModifier,
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
    clipboard: ClipboardFileConfig,
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

    let mut input = config.input;
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
    if let Some(enable_osc52) = parsed.clipboard.enable_osc52 {
        config.clipboard.enable_osc52 = enable_osc52;
    }

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
        match parse_key(&prefix) {
            Some(parsed) => input.prefix = parsed,
            None => warnings.push(format!(
                "Could not parse prefix `{prefix}`; try e.g. `ctrl-a`"
            )),
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

fn expand_path(path: impl AsRef<Path>) -> PathBuf {
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

fn parse_key(value: &str) -> Option<KeyEvent> {
    let normalized = value.trim().replace('+', "-");
    if normalized.is_empty() {
        return None;
    }
    let mut mods = KeyMods::NONE;
    let mut code = None;
    for part in normalized
        .split('-')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "super" | "meta" => mods.super_key = true,
            "shift" => mods.shift = true,
            "enter" | "return" => code = Some(KeyCode::Enter),
            "esc" | "escape" => code = Some(KeyCode::Esc),
            "space" => code = Some(KeyCode::Char(' ')),
            "tab" => code = Some(KeyCode::Tab),
            "backspace" => code = Some(KeyCode::Backspace),
            "left" => code = Some(KeyCode::Left),
            "right" => code = Some(KeyCode::Right),
            "up" => code = Some(KeyCode::Up),
            "down" => code = Some(KeyCode::Down),
            other => {
                let mut chars = other.chars();
                let first = chars.next()?;
                if chars.next().is_none() {
                    code = Some(KeyCode::Char(first));
                } else {
                    return None;
                }
            }
        }
    }
    code.map(|code| KeyEvent { code, mods })
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
