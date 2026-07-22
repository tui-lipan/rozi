use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

#[cfg(test)]
use crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER;
use crate::state::{CapStyle, PaneBorderStyle};

use super::appearance::{apply_animations, resolve_pane_padding};
use super::input::{apply_input_config, build_key_overrides};
use super::rules::{build_hints, build_rules};
use super::schema::*;
use super::sidebar::apply_sidebar_config;
use super::workbar::{apply_workbar_config, apply_workbar_style_config};

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub warnings: Vec<String>,
}

/// `shell`/`command_shell` config value: the historical bare string (a program name with no
/// arguments) or an argument-preserving array, e.g. `shell = ["pwsh.exe", "-NoLogo"]`.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum ShellFileValue {
    One(String),
    Many(Vec<String>),
}

impl ShellFileValue {
    fn into_argv(self) -> Vec<String> {
        match self {
            ShellFileValue::One(program) => vec![program],
            ShellFileValue::Many(argv) => argv,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    shell: Option<ShellFileValue>,
    command_shell: Option<ShellFileValue>,
    shell_integration: ShellIntegrationFileConfig,
    cwd: Option<String>,
    scrollback: Option<usize>,
    input: InputFileConfig,
    animations: AnimationFileConfig,
    theme: ThemeFileConfig,
    profile: ProfileFileConfig,
    session: SessionFileConfig,
    remote: RemoteFileConfig,
    layout: LayoutFileConfig,
    pane: PaneFileConfig,
    clipboard: ClipboardFileConfig,
    notifications: NotificationsFileConfig,
    navigation: NavigationFileConfig,
    confirm: ConfirmFileConfig,
    scratchpad: ScratchpadFileConfig,
    sidebar: SidebarFileConfig,
    workbar: WorkbarFileConfig,
    rules: Vec<RuleFileConfig>,
    hints: Vec<HintFileConfig>,
    hooks: Vec<HookFileConfig>,
    logging: LoggingFileConfig,
    keys: HashMap<String, KeyBindingSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookFileConfig {
    event: String,
    run: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct LoggingFileConfig {
    dir: Option<String>,
}

/// A `[keys]` value: replacement bindings, an additive binding table, or a user command table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum KeyBindingSpec {
    One(String),
    Many(Vec<String>),
    Add(AddKeyBindingSpec),
    UserCommand(UserCommandTableSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddKeyBindingSpec {
    pub(super) add: KeyBindingCandidates,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum KeyBindingCandidates {
    One(String),
    Many(Vec<String>),
}

impl KeyBindingCandidates {
    pub(super) fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct UserCommandTableSpec {
    pub(super) run: Option<String>,
    pub(super) send: Option<String>,
    pub(super) popup: Option<String>,
    pub(super) keep_open: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SidebarFileConfig {
    pub(super) visible: Option<bool>,
    pub(super) width: Option<u16>,
    pub(super) position: Option<String>,
    pub(super) tabs: Option<Vec<SidebarTabSpec>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum SidebarTabSpec {
    Name(String),
    // Boxed: the table form carries every built-in file-tree option, so inlining it would make each
    // bare tab name in the list as large as the fullest table.
    Table(Box<SidebarTabTableSpec>),
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SidebarTabTableSpec {
    pub(super) name: String,
    pub(super) label: String,
    pub(super) entries: Option<Vec<SidebarLauncherEntrySpec>>,
    pub(super) command: Option<String>,
    pub(super) interval: Option<u64>,
    pub(super) on_click: Option<UserCommandTableSpec>,
    // Built-in file-tree options; only meaningful when `name` is `files` or `git`.
    pub(super) root: Option<String>,
    pub(super) show_hidden: Option<bool>,
    pub(super) icons: Option<bool>,
    pub(super) explorer: Option<bool>,
    pub(super) diff_stats: Option<bool>,
    pub(super) max_entries: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SidebarLauncherEntrySpec {
    pub(super) label: String,
    pub(super) run: Option<String>,
    pub(super) send: Option<String>,
    pub(super) popup: Option<String>,
    pub(super) keep_open: Option<bool>,
}

impl SidebarLauncherEntrySpec {
    pub(super) fn action(self) -> UserCommandTableSpec {
        UserCommandTableSpec {
            run: self.run,
            send: self.send,
            popup: self.popup,
            keep_open: self.keep_open,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ConfirmFileConfig {
    close_pane: Option<bool>,
    kill_workspace: Option<bool>,
    kill_session: Option<bool>,
    quit_ephemeral: Option<bool>,
    new_temporary_session: Option<bool>,
    load_profile: Option<bool>,
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
pub(super) struct WorkbarFileConfig {
    pub(super) left: Option<Vec<WorkbarSegmentSpec>>,
    pub(super) right: Option<Vec<WorkbarSegmentSpec>>,
    pub(super) clock_format: Option<String>,
}

/// A `[workbar]` list entry: either a bare segment name (`"clock"`, `"text:.."`, `"command:.."`)
/// or a table `{ segment = "..", color = "info" }` that overrides the badge color by theme role.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum WorkbarSegmentSpec {
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
#[serde(default, deny_unknown_fields)]
struct ShellIntegrationFileConfig {
    mode: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SessionFileConfig {
    autosave: Option<bool>,
    path: Option<String>,
    startup: Option<String>,
    resurrect: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RemoteFileConfig {
    default_host: Option<String>,
    connection_timeout_secs: Option<u64>,
    server_alive_interval_secs: Option<u64>,
    server_alive_count_max: Option<u64>,
    install: Option<String>,
    batch_mode: Option<bool>,
    #[serde(default)]
    hosts: HashMap<String, RemoteHostFileConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RemoteHostFileConfig {
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    #[serde(default)]
    ssh_args: Vec<String>,
    binary_path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct LayoutFileConfig {
    split_width_multiplier: Option<f32>,
}

/// `[pane] padding` value: a single number applies to all four sides, or a CSS-style array of
/// `[vertical, horizontal]` (2 values) or `[top, right, bottom, left]` (4 values).
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub(super) enum PaddingSpec {
    All(u16),
    Sides(Vec<u16>),
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub(super) struct PaneFileConfig {
    hold_on_exit: Option<bool>,
    highlight_focused_background: Option<bool>,
    highlight_focused_border: Option<bool>,
    focus_on_hover: Option<bool>,
    show_workbar: Option<bool>,
    workbar_gap: Option<bool>,
    workbar_at_bottom: Option<bool>,
    show_titles: Option<bool>,
    merge_borders: Option<bool>,
    background_follows_terminal: Option<bool>,
    border_style: Option<String>,
    padding: Option<PaddingSpec>,
    title_style: Option<String>,
    pub(super) workbar_badge_style: Option<String>,
    pub(super) workbar_powerline: Option<bool>,
    pub(super) workbar_tab_style: Option<String>,
    pub(super) workbar_style: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RuleFileConfig {
    #[serde(rename = "match", default)]
    pub(super) matches: String,
    #[serde(default)]
    pub(super) match_regex: Option<String>,
    pub(super) float: bool,
    pub(super) width: Option<f32>,
    pub(super) height: Option<f32>,
    pub(super) workspace: Option<usize>,
    pub(super) focus: bool,
    pub(super) fullscreen: bool,
}

impl Default for RuleFileConfig {
    fn default() -> Self {
        Self {
            matches: String::new(),
            match_regex: None,
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct HintFileConfig {
    pub(super) pattern: String,
    pub(super) open: bool,
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
    bell: Option<bool>,
    pane_blocked: Option<bool>,
    pane_done: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct NavigationFileConfig {
    editors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub(super) struct AnimationFileConfig {
    pub(super) enabled: Option<bool>,
    pub(super) spawn: Option<bool>,
    pub(super) close: Option<bool>,
    pub(super) fullscreen: Option<bool>,
    pub(super) tile_float: Option<bool>,
    pub(super) axis_change: Option<bool>,
    pub(super) focus_chrome: Option<bool>,
    pub(super) geometry_ms: Option<u64>,
    pub(super) close_ms: Option<u64>,
    pub(super) focus_chrome_ms: Option<u64>,
    pub(super) open_delay_ms: Option<u64>,
}

/// The config text most recently read or written by this process. Lets the live-reload
/// watcher distinguish external edits from hyprmux's own persistence writes (theme selection,
/// appearance toggles, default profile) and skip event bursts that left the content unchanged.
static LAST_SEEN_CONFIG: Mutex<Option<String>> = Mutex::new(None);

pub(super) fn note_config_text(text: Option<String>) {
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

    let Some(parsed) = parse_file_config(&text, &path, &mut warnings) else {
        return LoadedConfig { config, warnings };
    };

    if let Some(shell) = non_empty_argv(parsed.shell) {
        config.shell = Some(shell);
    }
    if let Some(command_shell) = non_empty_argv(parsed.command_shell) {
        config.command_shell = Some(command_shell);
    }
    if let Some(mode) = non_empty(parsed.shell_integration.mode) {
        match ShellIntegrationMode::parse(&mode) {
            Some(value) => config.shell_integration.mode = value,
            None => warnings.push(format!(
                "Ignored unknown shell_integration.mode \"{mode}\" (expected `auto` or `off`)"
            )),
        }
    }
    if let Some(cwd) = non_empty(parsed.cwd) {
        config.cwd = Some(expand_path(cwd).to_string_lossy().to_string());
    }
    if let Some(scrollback) = parsed.scrollback {
        config.scrollback = scrollback.max(1);
    }
    if let Some(dir) = non_empty(parsed.logging.dir) {
        config.logging.dir = Some(expand_path(dir));
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
    if let Some(resurrect) = parsed.session.resurrect {
        config.session.resurrect = resurrect;
    }
    if let Some(path) = non_empty(parsed.session.path) {
        config.session.path = Some(expand_path(path));
    }
    if let Some(startup) = non_empty(parsed.session.startup) {
        match SessionStartup::parse(&startup) {
            Some(value) => config.session.startup = value,
            None => warnings.push(format!(
                "Ignored unknown session.startup \"{startup}\" (expected `ephemeral`, `picker`, or `last`)"
            )),
        }
    }
    if let Some(default_host) = non_empty(parsed.remote.default_host) {
        config.remote.default_host = Some(default_host);
    }
    if let Some(secs) = parsed.remote.connection_timeout_secs {
        config.remote.connection_timeout_secs = secs;
    }
    if let Some(secs) = parsed.remote.server_alive_interval_secs {
        config.remote.server_alive_interval_secs = secs.max(1);
    }
    if let Some(count) = parsed.remote.server_alive_count_max {
        config.remote.server_alive_count_max = count.max(1);
    }
    if let Some(batch_mode) = parsed.remote.batch_mode {
        config.remote.batch_mode = batch_mode;
    }
    if let Some(install) = non_empty(parsed.remote.install) {
        match RemoteInstallPolicy::parse(&install) {
            Some(value) => config.remote.install = value,
            None => warnings.push(format!(
                "Ignored unknown remote.install \"{install}\" (expected `prompt`, `never`, or `always`)"
            )),
        }
    }
    for (alias, host) in parsed.remote.hosts {
        config.remote.hosts.insert(
            alias,
            RemoteHostConfig {
                host: non_empty(host.host),
                user: non_empty(host.user),
                port: host.port.filter(|port| *port != 0),
                identity_file: non_empty(host.identity_file),
                ssh_args: host.ssh_args,
                binary_path: non_empty(host.binary_path),
            },
        );
    }
    if let Some(multiplier) = parsed.layout.split_width_multiplier {
        if multiplier.is_finite() && multiplier > 0.0 {
            config.layout.split_width_multiplier = multiplier;
        } else {
            warnings.push(format!(
                "Ignored layout.split_width_multiplier {multiplier} (expected a positive finite number)"
            ));
        }
    }
    if let Some(highlight_focused_background) = parsed.pane.highlight_focused_background {
        config.pane.highlight_focused_background = highlight_focused_background;
    }
    if let Some(hold_on_exit) = parsed.pane.hold_on_exit {
        config.pane.hold_on_exit = hold_on_exit;
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
    if let Some(background_follows_terminal) = parsed.pane.background_follows_terminal {
        config.pane.background_follows_terminal = background_follows_terminal;
    }
    if let Some(border_style) = parsed.pane.border_style.as_deref() {
        match PaneBorderStyle::parse(border_style) {
            Some(style) => config.pane.border_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.border_style \"{border_style}\" (expected one of: rounded, plain, double, thick)"
            )),
        }
    }
    if let Some(padding) = parsed.pane.padding.clone()
        && let Some(resolved) = resolve_pane_padding(padding, &mut warnings)
    {
        config.pane.padding = resolved;
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
    if let Some(bell) = parsed.notifications.bell {
        config.notifications.bell = bell;
    }
    if let Some(pane_blocked) = parsed.notifications.pane_blocked {
        config.notifications.pane_blocked = pane_blocked;
    }
    if let Some(pane_done) = parsed.notifications.pane_done {
        config.notifications.pane_done = pane_done;
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
    if let Some(load_profile) = parsed.confirm.load_profile {
        config.confirm.load_profile = load_profile;
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
    apply_sidebar_config(&mut config.sidebar, parsed.sidebar, &mut warnings);
    config.rules = build_rules(parsed.rules, &mut warnings);
    config.hints = build_hints(parsed.hints, &mut warnings);
    config.hooks = build_hooks(parsed.hooks, &mut warnings);
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

fn parse_file_config(text: &str, path: &Path, warnings: &mut Vec<String>) -> Option<FileConfig> {
    match toml::from_str::<FileConfig>(text) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            let legacy_hooks = toml::from_str::<toml::Value>(text)
                .ok()
                .and_then(|value| value.get("hooks").cloned())
                .is_some_and(|hooks| hooks.is_table());
            if legacy_hooks {
                warnings.push(
                    "Legacy [hooks] is no longer supported; migrate each command to `[[hooks]]` with `event = \"...\"` and `run = \"...\"`"
                        .to_string(),
                );
            } else {
                warnings.push(format!("Config parse failed for {}: {err}", path.display()));
            }
            None
        }
    }
}

fn build_hooks(hooks: Vec<HookFileConfig>, warnings: &mut Vec<String>) -> Vec<HyprmuxHookConfig> {
    hooks
        .into_iter()
        .filter_map(|hook| {
            let Some(event) = crate::events::EventKind::parse(&hook.event) else {
                warnings.push(format!("Ignored unknown hook event `{}`", hook.event));
                return None;
            };
            if hook.run.trim().is_empty() {
                warnings.push(format!("Ignored empty hook for `{}`", hook.event));
                return None;
            }
            Some(HyprmuxHookConfig {
                event,
                run: hook.run,
            })
        })
        .collect()
}

pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPRMUX_CONFIG") {
        return expand_path(path);
    }
    config_home().join("hyprmux.toml")
}

/// The `hyprmux` config directory (already includes the `hyprmux` segment - callers should join
/// filenames directly, e.g. `config_home().join("hyprmux.toml")`).
///
/// Delegates to [`crate::platform::paths::config_dir`]; kept as a thin wrapper here (rather than
/// switching every call site to the platform module directly) so `config_path()`/`profiles_dir()`/
/// `themes_dir()` in this module family don't need to change beyond the path they join onto it.
pub(super) fn config_home() -> PathBuf {
    crate::platform::paths::config_dir(&crate::platform::paths::PlatformEnv::from_process())
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

pub(super) fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Like [`non_empty`], but for the argument-preserving `shell`/`command_shell` form: trims the
/// program (first element) and rejects an empty program, an empty array, or a bare empty string.
fn non_empty_argv(value: Option<ShellFileValue>) -> Option<Vec<String>> {
    let mut argv = value?.into_argv();
    let program = argv.first_mut()?;
    *program = program.trim().to_string();
    if program.is_empty() {
        return None;
    }
    Some(argv)
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn structured_hooks_parse_and_keep_multiple_entries() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [[hooks]]
            event = "pane-exited"
            run = "first"

            [[hooks]]
            event = "pane-exited"
            run = "second"
            "#,
        )
        .expect("config parses");
        let hooks = build_hooks(parsed.hooks, &mut Vec::new());
        assert_eq!(hooks.len(), 2);
        assert!(
            hooks
                .iter()
                .all(|hook| hook.event == crate::events::EventKind::PaneExited)
        );
        assert_eq!(hooks[0].run, "first");
        assert_eq!(hooks[1].run, "second");
    }

    #[test]
    fn unknown_and_empty_hooks_are_dropped_with_warnings() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [[hooks]]
            event = "future-event"
            run = "ignored"

            [[hooks]]
            event = "pane-exited"
            run = "  "

            [[hooks]]
            event = "pane-spawned"
            run = "kept"
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let hooks = build_hooks(parsed.hooks, &mut warnings);
        assert_eq!(
            hooks,
            vec![HyprmuxHookConfig {
                event: crate::events::EventKind::PaneSpawned,
                run: "kept".into(),
            }]
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("future-event"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("empty hook"))
        );
    }

    #[test]
    fn legacy_flat_hooks_report_migration_warning() {
        let mut warnings = Vec::new();
        let parsed = parse_file_config(
            "[hooks]\npane-exited = \"notify-send exited\"",
            Path::new("hyprmux.toml"),
            &mut warnings,
        );
        assert!(parsed.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Legacy [hooks]"));
        assert!(warnings[0].contains("[[hooks]]"));
        assert!(warnings[0].contains("event"));
        assert!(warnings[0].contains("run"));
    }

    #[test]
    fn layout_split_width_multiplier_is_configurable() {
        let parsed: FileConfig =
            toml::from_str("[layout]\nsplit_width_multiplier = 2.28").expect("config parses");
        assert_eq!(parsed.layout.split_width_multiplier, Some(2.28));
        assert_eq!(
            HyprmuxLayoutConfig::default().split_width_multiplier,
            DEFAULT_SPLIT_WIDTH_MULTIPLIER
        );
    }

    #[test]
    fn shell_integration_section_parses_its_mode() {
        let parsed: FileConfig =
            toml::from_str("[shell_integration]\nmode = \"off\"").expect("config parses");
        assert_eq!(parsed.shell_integration.mode.as_deref(), Some("off"));
        assert_eq!(
            ShellIntegrationMode::parse("off"),
            Some(ShellIntegrationMode::Off)
        );
        assert_eq!(ShellIntegrationMode::parse("sometimes"), None);
    }

    #[test]
    fn confirm_section_overrides_defaults() {
        let defaults = HyprmuxConfirmConfig::default();
        assert!(!defaults.close_pane);
        assert!(defaults.kill_workspace);
        assert!(defaults.kill_session);
        assert!(defaults.quit_ephemeral);
        assert!(defaults.new_temporary_session);

        let parsed: FileConfig = toml::from_str(
            "[confirm]\nclose_pane = true\nkill_workspace = false\nquit_ephemeral = false\nnew_temporary_session = false",
        )
        .expect("config parses");
        assert_eq!(parsed.confirm.close_pane, Some(true));
        assert_eq!(parsed.confirm.kill_workspace, Some(false));
        assert_eq!(parsed.confirm.kill_session, None);
        assert_eq!(parsed.confirm.quit_ephemeral, Some(false));
        assert_eq!(parsed.confirm.new_temporary_session, Some(false));
    }

    #[test]
    fn session_section_parses_startup() {
        let parsed: FileConfig =
            toml::from_str("[session]\nstartup = \"picker\"").expect("config parses");
        assert_eq!(parsed.session.startup.as_deref(), Some("picker"));
    }

    #[test]
    fn top_level_input_aliases_are_rejected() {
        let error = toml::from_str::<FileConfig>("prefix = \"ctrl-b\"\nmodifier = \"super\"")
            .expect_err("top-level input aliases should not parse");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn file_config_parses_profile_default() {
        let parsed: FileConfig =
            toml::from_str("[profile]\ndefault = \"dev\"").expect("config parses");
        assert_eq!(parsed.profile.default.as_deref(), Some("dev"));
    }

    #[test]
    fn file_config_parses_pane_options() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            hold_on_exit = true
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
        assert_eq!(parsed.pane.hold_on_exit, Some(true));
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
    fn file_config_parses_notifications_and_navigation() {
        let parsed: FileConfig = toml::from_str(
            "[notifications]\nenabled = true\npane_exit = false\npane_blocked = false\npane_done = true\n[navigation]\neditors = [\"nvim\", \"hx\"]",
        )
        .expect("config parses");
        assert_eq!(parsed.notifications.enabled, Some(true));
        assert_eq!(parsed.notifications.pane_exit, Some(false));
        assert_eq!(parsed.notifications.pane_blocked, Some(false));
        assert_eq!(parsed.notifications.pane_done, Some(true));
        assert_eq!(
            parsed.navigation.editors,
            Some(vec!["nvim".into(), "hx".into()])
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
}
