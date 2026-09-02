use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;

use serde::Deserialize;
use tui_lipan::input::KeyBinding;

#[cfg(test)]
use crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER;
use crate::state::{AlertMode, PaneBorderMode, PaneBorderStyle, PaneTitlebarMode, parse_cap_style};

use super::appearance::{apply_animations, resolve_pane_padding};
use super::commands::build_named_commands;
use super::input::{apply_input_config, build_key_overrides};
use super::rules::{build_hints, build_rules};
use super::schema::*;
use super::sidebar::apply_sidebar_config;
use super::workbar::{apply_pane_alert_colors, apply_workbar_config, apply_workbar_style_config};

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
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
    environment: EnvironmentFileConfig,
    cwd: Option<String>,
    scrollback: Option<usize>,
    frame_rate: Option<u16>,
    nerd_icons: Option<bool>,
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
    sounds: SoundsFileConfig,
    navigation: NavigationFileConfig,
    confirm: ConfirmFileConfig,
    scratchpad: ScratchpadFileConfig,
    sidebar: SidebarFileConfig,
    workbar: WorkbarFileConfig,
    agents: Vec<crate::agent_detection::AgentSpec>,
    rules: Vec<RuleFileConfig>,
    hints: Vec<HintFileConfig>,
    hooks: Vec<HookFileConfig>,
    commands: Vec<NamedCommandFileConfig>,
    services: Vec<ServiceFileConfig>,
    extensions: ExtensionsFileConfig,
    logging: LoggingFileConfig,
    keys: HashMap<String, KeyBindingSpec>,
}

pub(crate) fn parse_extensions_config(text: &str) -> Result<ExtensionsFileConfig, toml::de::Error> {
    toml::from_str::<FileConfig>(text).map(|config| config.extensions)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceFileConfig {
    pub(crate) name: Option<String>,
    pub(crate) run: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) restart: Option<String>,
    #[serde(default)]
    pub(crate) env: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct NamedCommandFileConfig {
    pub(crate) id: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) run: Option<String>,
    pub(crate) send: Option<String>,
    pub(crate) popup: Option<String>,
    pub(crate) exec: Option<String>,
    pub(crate) keep_open: Option<bool>,
}

impl NamedCommandFileConfig {
    pub(super) fn action(self) -> UserCommandTableSpec {
        UserCommandTableSpec {
            label: None,
            run: self.run,
            send: self.send,
            popup: self.popup,
            exec: self.exec,
            keep_open: self.keep_open,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(default)]
pub(crate) struct ExtensionsFileConfig {
    pub(crate) disabled: Vec<String>,
    /// `[extensions.<id>]` settings tables, kept raw until the extension that declares them is
    /// known. `deny_unknown_fields` is absent here because these keys are open by design; a table
    /// naming no installed extension is reported when the two are matched up.
    #[serde(flatten)]
    pub(crate) settings: std::collections::BTreeMap<String, toml::Value>,
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
    max_bytes: Option<u64>,
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
    /// Name shown in the command palette and help overlay instead of the generated
    /// `Run: <command>`, which truncates a pipeline into something unreadable.
    pub(super) label: Option<String>,
    pub(super) run: Option<String>,
    pub(super) send: Option<String>,
    pub(super) popup: Option<String>,
    pub(super) exec: Option<String>,
    pub(super) keep_open: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SidebarFileConfig {
    pub(super) visible: Option<bool>,
    pub(super) width: Option<u16>,
    pub(super) position: Option<String>,
    pub(super) tabs: Option<Vec<SidebarTabSpec>>,
    pub(super) panels: Option<Vec<Vec<String>>>,
    pub(super) split: Option<bool>,
    pub(super) split_ratio: Option<f32>,
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
    /// Command tabs only: the marker that turns an output line into a section header.
    pub(super) group_prefix: Option<String>,
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
    pub(super) group: Option<String>,
    pub(super) run: Option<String>,
    pub(super) send: Option<String>,
    pub(super) popup: Option<String>,
    pub(super) keep_open: Option<bool>,
}

impl SidebarLauncherEntrySpec {
    pub(super) fn action(self) -> UserCommandTableSpec {
        UserCommandTableSpec {
            // A launcher entry carries its own label in the sidebar row, so it never needs the
            // palette-facing one.
            label: None,
            run: self.run,
            send: self.send,
            popup: self.popup,
            // A launcher row is a visible affordance; running its command invisibly would leave
            // the click with no feedback at all.
            exec: None,
            keep_open: self.keep_open,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ConfirmFileConfig {
    close_pane: Option<bool>,
    kill_workspace: Option<bool>,
    kill_session: Option<bool>,
    quit_ephemeral: Option<bool>,
    new_temporary_session: Option<bool>,
    load_profile: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ScratchpadFileConfig {
    command: Option<String>,
    cwd: Option<String>,
    height: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct WorkbarFileConfig {
    pub(super) left: Option<Vec<WorkbarSegmentSpec>>,
    pub(super) right: Option<Vec<WorkbarSegmentSpec>>,
    pub(super) clock_format: Option<String>,
    pub(super) alert: WorkbarAlertFileConfig,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct WorkbarAlertFileConfig {
    pub(super) bell: Option<bool>,
    pub(super) blocked: Option<bool>,
    pub(super) finished: Option<bool>,
    pub(super) working: Option<bool>,
    pub(super) idle: Option<bool>,
    pub(super) mode: Option<String>,
    pub(super) paint: Option<String>,
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
#[serde(default, deny_unknown_fields)]
struct ProfileFileConfig {
    default: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ShellIntegrationFileConfig {
    mode: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct EnvironmentFileConfig {
    forward: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SessionFileConfig {
    autosave: Option<bool>,
    path: Option<String>,
    startup: Option<String>,
    resurrect: Option<bool>,
    allow_takeover: Option<bool>,
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
    default: Option<String>,
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
#[serde(default, deny_unknown_fields)]
pub(super) struct PaneFileConfig {
    resize_debounce_ms: Option<u64>,
    hold_on_exit: Option<bool>,
    highlight_focused_background: Option<bool>,
    highlight_focused_border: Option<bool>,
    highlight_focused_titlebar: Option<bool>,
    focus_on_hover: Option<bool>,
    show_workbar: Option<bool>,
    workbar_gap: Option<bool>,
    workbar_at_bottom: Option<bool>,
    show_titles: Option<bool>,
    border_mode: Option<String>,
    alert_border: Option<String>,
    pub(super) alert: PaneAlertFileConfig,
    keep_special_borders: Option<bool>,
    background_follows_terminal: Option<bool>,
    border_style: Option<String>,
    padding: Option<PaddingSpec>,
    titlebar: Option<String>,
    title_style: Option<String>,
    pub(super) workbar_badge_style: Option<String>,
    pub(super) workbar_powerline: Option<bool>,
    pub(super) workbar_tab_style: Option<String>,
    pub(super) workbar_style: Option<String>,
    pub(super) toast_opacity: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PaneAlertFileConfig {
    pub(super) blocked: Option<String>,
    pub(super) finished: Option<String>,
    pub(super) working: Option<String>,
    pub(super) idle: Option<String>,
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
    pub(super) position: Option<String>,
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
            position: None,
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
#[serde(default, deny_unknown_fields)]
struct InputFileConfig {
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
    which_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ThemeFileConfig {
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ClipboardFileConfig {
    enable_osc52: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct NotificationsFileConfig {
    enabled: Option<bool>,
    pane_exit: Option<bool>,
    pane_exit_error: Option<bool>,
    bell: Option<bool>,
    pane_blocked: Option<bool>,
    pane_done: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SoundsFileConfig {
    enabled: Option<bool>,
    bell: Option<bool>,
    blocked: Option<bool>,
    done: Option<bool>,
    error: Option<bool>,
    throttle_ms: Option<u64>,
    bell_file: Option<String>,
    blocked_file: Option<String>,
    done_file: Option<String>,
    error_file: Option<String>,
    player: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct NavigationFileConfig {
    editors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct AnimationFileConfig {
    pub(super) enabled: Option<bool>,
    pub(super) spawn: Option<bool>,
    pub(super) close: Option<bool>,
    pub(super) fullscreen: Option<bool>,
    pub(super) tile_float: Option<bool>,
    pub(super) axis_change: Option<bool>,
    pub(super) sidebar: Option<bool>,
    pub(super) focus_chrome: Option<bool>,
    pub(super) pane_style: Option<String>,
    pub(super) geometry_ms: Option<u64>,
    pub(super) close_ms: Option<u64>,
    pub(super) focus_chrome_ms: Option<u64>,
    pub(super) alert_pulse_ms: Option<u64>,
    pub(super) open_delay_ms: Option<u64>,
}

/// The config text most recently read or written by this process. Lets the live-reload
/// watcher distinguish external edits from rozi's own persistence writes (theme selection,
/// appearance toggles, default profile) and skip event bursts that left the content unchanged.
static LAST_SEEN_CONFIG: Mutex<Option<String>> = Mutex::new(None);

pub(super) fn note_config_text(text: Option<String>) {
    *LAST_SEEN_CONFIG.lock().unwrap() = text;
}

/// True when the on-disk config no longer matches the text rozi last read or wrote.
pub fn config_text_changed_on_disk() -> bool {
    let current = std::fs::read_to_string(config_path()).ok();
    *LAST_SEEN_CONFIG.lock().unwrap() != current
}

pub fn load_config() -> LoadedConfig {
    let path = config_path();
    let extension_scan = super::extensions::scan_extensions();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            note_config_text(None);
            return load_config_from_text_with_extensions("", &path, extension_scan, Vec::new());
        }
        Err(err) => {
            note_config_text(None);
            let mut warnings = Vec::new();
            warnings.push(format!("Config read failed for {}: {err}", path.display()));
            return load_config_from_text_with_extensions("", &path, extension_scan, warnings);
        }
    };
    note_config_text(Some(text.clone()));

    load_config_from_text_with_extensions(&text, &path, extension_scan, Vec::new())
}

/// Applies one config document over the defaults. `path` only names the source in warnings, so
/// this is the whole load pipeline minus the filesystem.
#[cfg(test)]
fn load_config_from_text(text: &str, path: &Path) -> LoadedConfig {
    load_config_from_text_with_extensions(
        text,
        path,
        super::extensions::ExtensionScan::default(),
        Vec::new(),
    )
}

fn load_config_from_text_with_extensions(
    text: &str,
    path: &Path,
    extensions: super::extensions::ExtensionScan,
    mut warnings: Vec<String>,
) -> LoadedConfig {
    let mut config = Config::default();

    let Some(parsed) = parse_file_config(text, path, &mut warnings) else {
        let contributions = extensions.into_contributions(&[], &Default::default());
        let suggested_keybindings = contributions.suggested_keybindings;
        warnings.extend(contributions.warnings);
        config.commands = contributions.commands;
        config.active_extensions = contributions.active_ids;
        config.installed_extensions = contributions.installed_ids;
        config.extension_problems = contributions.problem_count;
        config.extension_runtime = contributions.runtime;
        config.services = contributions.services;
        config.agents = contributions.agents;
        config
            .navigation
            .add_extension_targets(&contributions.navigation_targets);
        // An unreadable config.toml is not a reason to lose an extension's tab: the defaults still
        // apply, and this takes the same merge the normal path does.
        apply_sidebar_config(
            &mut config.sidebar,
            SidebarFileConfig::default(),
            contributions.sidebar_tabs,
            &mut warnings,
        );
        let resolved = super::extensions::resolve_suggested_keybindings(
            &config,
            suggested_keybindings,
            &HashSet::new(),
            &mut warnings,
        );
        config.extension_action_key_defaults = resolved.active;
        config.suggested_keybinding_resolutions = resolved.diagnostics;
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
    let mut forwarded = std::collections::HashSet::new();
    config.environment.forward = parsed
        .environment
        .forward
        .into_iter()
        .filter_map(|name| non_empty(Some(name)))
        .filter(|name| forwarded.insert(name.clone()))
        .collect();
    if let Some(cwd) = non_empty(parsed.cwd) {
        config.cwd = Some(expand_path(cwd).to_string_lossy().to_string());
    }
    if let Some(scrollback) = parsed.scrollback {
        config.scrollback = scrollback.max(1);
    }
    if let Some(frame_rate) = parsed.frame_rate {
        config.frame_rate = clamp_frame_rate(frame_rate, &mut warnings);
    }
    if let Some(nerd_icons) = parsed.nerd_icons {
        config.nerd_icons = nerd_icons;
    }
    if let Some(dir) = non_empty(parsed.logging.dir) {
        config.logging.dir = Some(expand_path(dir));
    }
    if let Some(max_bytes) = parsed.logging.max_bytes {
        config.logging.max_bytes = max_bytes;
    }

    let mut input = config.input.clone();
    apply_input_config(
        &mut input,
        parsed.input.modifier,
        parsed.input.prefix,
        parsed.input.modifier_shortcuts,
        parsed.input.which_key,
        &mut warnings,
    );
    config.input = input;
    apply_animations(&mut config.animations, parsed.animations, &mut warnings);

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
    if let Some(allow_takeover) = parsed.session.allow_takeover {
        config.session.allow_takeover = allow_takeover;
    }
    if let Some(path) = non_empty(parsed.session.path) {
        config.session.path = Some(expand_path(path));
    }
    if let Some(startup) = non_empty(parsed.session.startup) {
        match SessionStartup::parse(&startup) {
            Some(value) => config.session.startup = value,
            None => warnings.push(format!(
                "Ignored unknown session.startup \"{startup}\" (expected `picker`, `ephemeral`, `last`, or `profile`)"
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
    if let Some(name) = parsed.layout.default.as_deref() {
        match crate::state::LayoutKind::from_label(name) {
            Some(kind) => config.layout.default = kind,
            None => {
                let expected = crate::state::LayoutKind::all()
                    .iter()
                    .map(|kind| kind.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                warnings.push(format!(
                    "Ignored layout.default `{name}` (expected one of {expected})"
                ))
            }
        }
    }
    if let Some(highlight_focused_background) = parsed.pane.highlight_focused_background {
        config.pane.highlight_focused_background = highlight_focused_background;
    }
    if let Some(resize_debounce_ms) = parsed.pane.resize_debounce_ms {
        config.pane.resize_debounce_ms = resize_debounce_ms;
    }
    if let Some(hold_on_exit) = parsed.pane.hold_on_exit {
        config.pane.hold_on_exit = hold_on_exit;
    }
    if let Some(highlight_focused_border) = parsed.pane.highlight_focused_border {
        config.pane.highlight_focused_border = highlight_focused_border;
    }
    if let Some(highlight_focused_titlebar) = parsed.pane.highlight_focused_titlebar {
        config.pane.highlight_focused_titlebar = highlight_focused_titlebar;
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
    if let Some(keep_special_borders) = parsed.pane.keep_special_borders {
        config.pane.keep_special_borders = keep_special_borders;
    }
    if let Some(border_mode) = parsed.pane.border_mode.as_deref() {
        match PaneBorderMode::parse(border_mode) {
            Some(mode) => config.pane.border_mode = mode,
            None => warnings.push(format!(
                "Ignored unknown pane.border_mode \"{border_mode}\" (expected one of: separate, merged, none, dividers)"
            )),
        }
    }
    if let Some(alert_border) = parsed.pane.alert_border.as_deref() {
        match AlertMode::parse(alert_border) {
            Some(mode) => config.pane.alert_border = mode,
            None => warnings.push(format!(
                "Ignored unknown pane.alert_border \"{alert_border}\" (expected one of: off, static, pulse)"
            )),
        }
    }
    apply_pane_alert_colors(
        &mut config.pane.alert_colors,
        parsed.pane.alert.clone(),
        &mut warnings,
    );
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
    if let Some(titlebar) = parsed.pane.titlebar.as_deref() {
        match PaneTitlebarMode::parse(titlebar) {
            Some(mode) => config.pane.titlebar = mode,
            None => warnings.push(format!(
                "Ignored unknown pane.titlebar \"{titlebar}\" (expected one of: bar, border, integrated, inset)"
            )),
        }
    }
    if let Some(title_style) = parsed.pane.title_style.as_deref() {
        match parse_cap_style(title_style) {
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
    if let Some(pane_exit_error) = parsed.notifications.pane_exit_error {
        config.notifications.pane_exit_error = pane_exit_error;
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
    if let Some(enabled) = parsed.sounds.enabled {
        config.sounds.enabled = enabled;
    }
    if let Some(bell) = parsed.sounds.bell {
        config.sounds.bell = bell;
    }
    if let Some(blocked) = parsed.sounds.blocked {
        config.sounds.blocked = blocked;
    }
    if let Some(done) = parsed.sounds.done {
        config.sounds.done = done;
    }
    if let Some(error) = parsed.sounds.error {
        config.sounds.error = error;
    }
    if let Some(throttle_ms) = parsed.sounds.throttle_ms {
        let clamped = throttle_ms.clamp(100, 60_000);
        if clamped != throttle_ms {
            warnings.push(format!(
                "Sound throttle {throttle_ms}ms out of range; clamped to {clamped}ms"
            ));
        }
        config.sounds.throttle_ms = clamped;
    }
    config.sounds.bell_file = non_empty(parsed.sounds.bell_file).map(expand_path);
    config.sounds.blocked_file = non_empty(parsed.sounds.blocked_file).map(expand_path);
    config.sounds.done_file = non_empty(parsed.sounds.done_file).map(expand_path);
    config.sounds.error_file = non_empty(parsed.sounds.error_file).map(expand_path);
    config.sounds.player = non_empty(parsed.sounds.player);
    let has_explicit_navigation_editors = parsed.navigation.editors.is_some();
    if let Some(editors) = parsed.navigation.editors {
        config.navigation.replace_with_user_editors(
            editors
                .into_iter()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect(),
        );
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
    let contributions =
        extensions.into_contributions(&parsed.extensions.disabled, &parsed.extensions.settings);
    let suggested_keybindings = contributions.suggested_keybindings;
    if !has_explicit_navigation_editors {
        config
            .navigation
            .add_extension_targets(&contributions.navigation_targets);
    }
    config.extension_problems = contributions.problem_count;
    warnings.extend(contributions.warnings);
    // Sidebar tabs are merged inside the sidebar pass rather than appended after it: placement is
    // resolved there, and a tab that arrived too late to be placed would be unreachable.
    apply_sidebar_config(
        &mut config.sidebar,
        parsed.sidebar,
        contributions.sidebar_tabs,
        &mut warnings,
    );
    config.rules = build_rules(parsed.rules, &mut warnings);
    config.hints = build_hints(parsed.hints, &mut warnings);
    config.hooks = build_hooks(parsed.hooks, &mut warnings);
    config.commands = build_named_commands(parsed.commands, &mut warnings);
    config.active_extensions = contributions.active_ids;
    config.installed_extensions = contributions.installed_ids;
    config.extension_runtime = contributions.runtime;
    config.commands.extend(contributions.commands);
    // Config entries first: an id shared with a built-in replaces it, and an extension's ids are
    // namespaced so they can only ever add.
    config.agents = crate::agent_detection::build_definitions(
        parsed.agents,
        crate::agent_detection::AgentOrigin::Config,
        &crate::agent_detection::AgentCatalog::builtin_definitions(),
        &mut warnings,
    );
    config.agents.extend(contributions.agents);
    let named_ids: HashSet<_> = config
        .commands
        .iter()
        .map(|command| command.id.clone())
        .collect();
    config.services = crate::config::services::build_services(parsed.services, &mut warnings);
    config.services.extend(contributions.services);
    let explicitly_configured_actions = parsed
        .keys
        .iter()
        .filter(|(id, spec)| {
            crate::commands::is_extension_bindable_action(id)
                && !matches!(spec, KeyBindingSpec::UserCommand(_))
        })
        .map(|(id, _)| id.clone())
        .collect::<HashSet<_>>();
    let mut user_commands = Vec::new();
    config.key_overrides = build_key_overrides(
        parsed.keys,
        &config.input,
        &named_ids,
        &mut user_commands,
        &mut warnings,
    );
    config.user_commands = user_commands;
    config.extension_key_defaults = resolve_extension_key_defaults(&config, &mut warnings);
    let resolved = super::extensions::resolve_suggested_keybindings(
        &config,
        suggested_keybindings,
        &explicitly_configured_actions,
        &mut warnings,
    );
    config.extension_action_key_defaults = resolved.active;
    config.suggested_keybinding_resolutions = resolved.diagnostics;

    LoadedConfig { config, warnings }
}

/// Turn each extension's suggested chord into a real binding, unless something already answers to
/// it.
///
/// A suggestion is the weakest claim in the system. It loses to a built-in default, to anything the
/// user wrote in `[keys]`, and to an extension that asked first — and it loses to a chord that
/// merely *starts* with an existing one, because typing that prefix would fire the other command
/// before the second step arrived. Losing is reported and costs nothing else: the command is still
/// in the palette and the user can bind it by hand.
fn resolve_extension_key_defaults(
    config: &Config,
    warnings: &mut Vec<String>,
) -> HashMap<String, Vec<KeyBinding>> {
    let mut claimed: Vec<(String, String)> = crate::commands::core_default_shortcuts(&config.input)
        .into_iter()
        .filter(|(id, _)| !config.key_overrides.contains_key(id))
        .map(|(id, binding)| (binding.canonical_lowercase(), format!("`{id}`")))
        .collect();
    for (id, bindings) in &config.key_overrides {
        for binding in bindings {
            claimed.push((binding.canonical_lowercase(), format!("`{id}`")));
        }
    }
    for command in &config.user_commands {
        for binding in &command.bindings {
            claimed.push((
                binding.canonical_lowercase(),
                "a `[keys]` command".to_string(),
            ));
        }
    }

    let prefix = format!(
        "{} {}",
        config.input.prefix.canonical_lowercase(),
        crate::commands::EXTENSION_KEY_LEADER
    );
    let mut defaults: HashMap<String, Vec<KeyBinding>> = HashMap::new();
    for command in &config.commands {
        let Some(steps) = command.default_key.as_deref() else {
            continue;
        };
        // An explicit `[keys]` entry for this very command already decided the question.
        if config.key_overrides.contains_key(&command.id) {
            continue;
        }
        let Ok(binding) = KeyBinding::from_str(&format!("{prefix} {steps}")) else {
            continue;
        };
        let canonical = binding.canonical_lowercase();
        let conflict = claimed.iter().find(|(existing, _)| {
            existing == &canonical
                || canonical.starts_with(&format!("{existing} "))
                || existing.starts_with(&format!("{canonical} "))
        });
        match conflict {
            Some((_, owner)) => warnings.push(format!(
                "Extension command `{}` suggests `{canonical}`, which {owner} already uses; bind it in `[keys]` to use it anyway",
                command.id
            )),
            None => {
                claimed.push((canonical, format!("`{}`", command.id)));
                defaults.insert(command.id.clone(), vec![binding]);
            }
        }
    }
    defaults
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

fn build_hooks(hooks: Vec<HookFileConfig>, warnings: &mut Vec<String>) -> Vec<HookConfig> {
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
            Some(HookConfig {
                event,
                run: hook.run,
            })
        })
        .collect()
}

pub fn config_path() -> PathBuf {
    // An isolated test process ignores the explicit override: it exists to keep every write inside
    // a scratch root, and a `ROZI_CONFIG` inherited from the developer's shell points out of it.
    if !crate::platform::paths::user_dirs_are_isolated()
        && let Ok(path) = std::env::var("ROZI_CONFIG")
    {
        return expand_path(path);
    }
    config_home().join("config.toml")
}

/// The `rozi` config directory (already includes the `rozi` segment - callers should join
/// filenames directly, e.g. `config_home().join("config.toml")`).
///
/// Delegates to [`crate::platform::paths::config_dir`]; kept as a thin wrapper here (rather than
/// switching every call site to the platform module directly) so `config_path()`/`profiles_dir()`/
/// `themes_dir()` in this module family don't need to change beyond the path they join onto it.
pub(super) fn config_home() -> PathBuf {
    crate::platform::paths::config_dir(&crate::platform::paths::PlatformEnv::from_process())
}

fn home_dir() -> Option<PathBuf> {
    crate::platform::paths::PlatformEnv::from_process().home
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

/// Bounds `tui-lipan`'s `App::frame_rate` accepts. Rozi clamps against them itself rather than
/// letting the framework do it silently, so a typo (`frame_rate = 6`) warns instead of leaving the
/// user at an unexplained 15.
pub const MIN_FRAME_RATE: u16 = 15;
/// Upper bound of the range described by [`MIN_FRAME_RATE`].
pub const MAX_FRAME_RATE: u16 = 480;

fn clamp_frame_rate(value: u16, warnings: &mut Vec<String>) -> u16 {
    let clamped = value.clamp(MIN_FRAME_RATE, MAX_FRAME_RATE);
    if clamped != value {
        warnings.push(format!(
            "Clamped frame_rate {value} to {clamped} (supported range {MIN_FRAME_RATE}-{MAX_FRAME_RATE})"
        ));
    }
    clamped
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
            vec![HookConfig {
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
            Path::new("config.toml"),
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
            LayoutConfig::default().split_width_multiplier,
            DEFAULT_SPLIT_WIDTH_MULTIPLIER
        );
    }

    #[test]
    fn layout_default_mode_parses_and_maps() {
        let parsed: FileConfig =
            toml::from_str("[layout]\ndefault = \"master\"").expect("config parses");
        assert_eq!(parsed.layout.default.as_deref(), Some("master"));
        assert_eq!(
            crate::state::LayoutKind::from_label(parsed.layout.default.as_deref().unwrap()),
            Some(crate::state::LayoutKind::Master)
        );
        // The built-in fallback stays dwindle when the key is absent.
        assert_eq!(
            LayoutConfig::default().default,
            crate::state::LayoutKind::Dwindle
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
        let defaults = ConfirmConfig::default();
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
    fn session_section_parses_startup_and_takeover() {
        let parsed: FileConfig =
            toml::from_str("[session]\nstartup = \"picker\"\nallow_takeover = false")
                .expect("config parses");
        assert_eq!(parsed.session.startup.as_deref(), Some("picker"));
        assert_eq!(parsed.session.allow_takeover, Some(false));
    }

    /// Takeover is on unless a config turns it off, and the server's own settings default agrees —
    /// a server started without a config must not behave differently from one started with the
    /// default config.
    #[test]
    fn takeover_is_enabled_by_default_on_both_sides() {
        assert!(crate::config::Config::default().session.allow_takeover);
        assert!(crate::session::server::ServerSettings::default().allow_takeover);
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
            resize_debounce_ms = 0
            hold_on_exit = true
            highlight_focused_background = true
            highlight_focused_border = true
            highlight_focused_titlebar = false
            focus_on_hover = false
            show_workbar = false
            workbar_gap = false
            workbar_at_bottom = true
            show_titles = false
            border_mode = "dividers"
            keep_special_borders = false
            padding = 2
            titlebar = "integrated"
            title_style = "round"
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            workbar_style = "half"
            "#,
        )
        .expect("config parses");
        assert_eq!(parsed.pane.resize_debounce_ms, Some(0));
        assert_eq!(parsed.pane.highlight_focused_background, Some(true));
        assert_eq!(parsed.pane.hold_on_exit, Some(true));
        assert_eq!(parsed.pane.highlight_focused_border, Some(true));
        assert_eq!(parsed.pane.highlight_focused_titlebar, Some(false));
        assert_eq!(parsed.pane.focus_on_hover, Some(false));
        assert_eq!(parsed.pane.show_workbar, Some(false));
        assert_eq!(parsed.pane.workbar_gap, Some(false));
        assert_eq!(parsed.pane.workbar_at_bottom, Some(true));
        assert_eq!(parsed.pane.show_titles, Some(false));
        assert_eq!(parsed.pane.border_mode.as_deref(), Some("dividers"));
        assert_eq!(parsed.pane.keep_special_borders, Some(false));
        assert_eq!(parsed.pane.padding, Some(PaddingSpec::All(2)));
        assert_eq!(parsed.pane.titlebar.as_deref(), Some("integrated"));
        assert_eq!(parsed.pane.title_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_badge_style.as_deref(), Some("arrow"));
        assert_eq!(parsed.pane.workbar_tab_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_style.as_deref(), Some("half"));
    }

    #[test]
    fn pane_resize_debounce_applies_and_allows_immediate_forwarding() {
        let loaded =
            load_config_from_text("[pane]\nresize_debounce_ms = 0\n", Path::new("config.toml"));
        assert_eq!(loaded.config.pane.resize_debounce_ms, 0);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn pane_alert_border_applies_known_values_and_warns_on_unknown_mode() {
        let loaded = load_config_from_text(
            r#"
            [pane]
            alert_border = "pulse"
            [pane.alert]
            blocked = "warning"
            finished = "off"
            "#,
            Path::new("test.toml"),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.config.pane.alert_border, AlertMode::Pulse);
        assert_eq!(
            loaded.config.pane.alert_colors.blocked,
            Some(BadgeColor::Warning)
        );
        assert_eq!(loaded.config.pane.alert_colors.finished, None);

        let loaded =
            load_config_from_text("[pane]\nalert_border = \"flash\"", Path::new("test.toml"));
        assert_eq!(loaded.config.pane.alert_border, AlertMode::Pulse);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(
            loaded.warnings[0].contains("off, static, pulse"),
            "{:?}",
            loaded.warnings
        );
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
    fn environment_forward_trims_drops_empty_and_deduplicates_names() {
        let loaded = load_config_from_text(
            "[environment]\nforward = [\" CURSOR_API_KEY \", \"\", \"CURSOR_API_KEY\", \"SOME_CUSTOM_VAR\"]",
            Path::new("test.toml"),
        );
        assert_eq!(
            loaded.config.environment.forward,
            vec!["CURSOR_API_KEY", "SOME_CUSTOM_VAR"]
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn file_config_parses_sounds() {
        let loaded = load_config_from_text(
            "[sounds]\nenabled = true\nblocked = false\nthrottle_ms = 10\nbell_file = \"~/bell.wav\"\nplayer = \"play\"",
            Path::new("test.toml"),
        );
        assert!(loaded.config.sounds.enabled);
        assert!(!loaded.config.sounds.blocked);
        assert_eq!(loaded.config.sounds.throttle_ms, 100);
        assert!(loaded.config.sounds.bell_file.is_some());
        assert_eq!(loaded.config.sounds.player.as_deref(), Some("play"));
    }

    const REFERENCE_EXAMPLE: &str = include_str!("../../examples/config.toml");

    /// Strips the comment marker from `examples/config.toml` setting lines, which are written as
    /// a hash immediately followed by the key. Prose lines use a hash and a space, so alternative
    /// spellings mentioned in the surrounding text never become a second live copy of a key.
    fn activate_reference_example(text: &str) -> String {
        text.lines()
            .map(|line| match line.strip_prefix('#') {
                Some(setting) if !setting.starts_with([' ', '#']) => setting,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// True once any leaf value survives: a table header alone sets nothing, but a key does.
    fn carries_a_value(value: &toml::Value) -> bool {
        match value {
            toml::Value::Table(table) => table.values().any(carries_a_value),
            toml::Value::Array(array) => array.iter().any(carries_a_value),
            _ => true,
        }
    }

    #[test]
    fn reference_example_ships_fully_commented_out() {
        let shipped: toml::Table =
            toml::from_str(REFERENCE_EXAMPLE).expect("reference example parses");
        let live: Vec<&str> = shipped
            .iter()
            .filter(|(_, value)| carries_a_value(value))
            .map(|(key, _)| key.as_str())
            .collect();
        assert!(
            live.is_empty(),
            "reference example must ship inert; these carry live values: {live:?}"
        );
    }

    #[test]
    fn agents_load_from_config_and_can_replace_a_builtin() {
        let loaded = load_config_from_text(
            r#"
            [[agents]]
            id = "mycoolagent"
            label = "My Cool Agent"
            match = { names = ["mca"] }

            [[agents.states]]
            state = "working"
            scope = "footer"
            screen = { any_of = ["thinking…"] }

            [[agents]]
            id = "claude"
            label = "Claude (mine)"
            "#,
            Path::new("test.toml"),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let ids: Vec<_> = loaded.config.agents.iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["mycoolagent", "claude"]);
        // The `claude` entry declared no `match`, so it inherited the built-in's process vocabulary
        // rather than being dropped as unmatchable.
        let claude = &loaded.config.agents[1];
        assert!(claude.names.contains(&"claude".to_string()));
        assert_eq!(claude.label(), "Claude (mine)");

        let catalog = crate::agent_detection::AgentCatalog::with_definitions(loaded.config.agents);
        assert_eq!(
            catalog.by_name("mca").map(|agent| agent.label()),
            Some("My Cool Agent")
        );
        assert_eq!(
            catalog.by_name("claude").map(|agent| agent.label()),
            Some("Claude (mine)"),
            "the config entry replaces the built-in rather than sitting behind it"
        );
        assert_eq!(
            catalog.by_name("codex").map(|agent| agent.label()),
            Some("Codex"),
            "untouched built-ins survive"
        );
    }

    #[test]
    fn frame_rate_clamps_out_of_range_values_with_a_warning() {
        let path = Path::new("test.toml");

        let loaded = load_config_from_text("", path);
        assert_eq!(
            loaded.config.frame_rate,
            tui_lipan::prelude::DEFAULT_FRAME_RATE
        );

        let loaded = load_config_from_text("frame_rate = 60\n", path);
        assert_eq!(loaded.config.frame_rate, 60);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

        let loaded = load_config_from_text("frame_rate = 6\n", path);
        assert_eq!(loaded.config.frame_rate, MIN_FRAME_RATE);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("frame_rate"));

        let loaded = load_config_from_text("frame_rate = 1000\n", path);
        assert_eq!(loaded.config.frame_rate, MAX_FRAME_RATE);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    }

    #[test]
    fn nerd_icons_parses_at_the_top_level_and_defaults_on() {
        assert!(crate::config::Config::default().nerd_icons);
        let loaded = load_config_from_text("nerd_icons = false\n", Path::new("config.toml"));
        assert!(!loaded.config.nerd_icons);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let loaded = load_config_from_text("", Path::new("config.toml"));
        assert!(loaded.config.nerd_icons);
    }

    /// Guards `examples/config.toml` against silent drift. Every struct in the file model denies
    /// unknown fields, so a renamed or dropped key fails to parse here, and a renamed value token
    /// (a cap style, a layout name, a hook event) shows up as a warning.
    #[test]
    fn reference_example_is_a_valid_warning_free_config_once_uncommented() {
        let text = activate_reference_example(REFERENCE_EXAMPLE);
        let loaded = load_config_from_text(&text, Path::new("examples/config.toml"));

        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    fn scan_with(temp: &Path, manifest: &str) -> super::super::extensions::ExtensionScan {
        let directory = temp.join("tasks");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("extension.toml"), manifest).unwrap();
        super::super::extensions::scan_extensions_in(temp)
    }

    const NAVIGATION_MANIFEST: &str = "[extension]\nid = \"vim-rozi\"\napi = 1\n\
         [[navigation_targets]]\nname = \"vim\"\nprograms = [\"nvim\", \"zed\"]\n";

    #[test]
    fn extension_navigation_targets_augment_defaults_when_user_config_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "",
            Path::new("config.toml"),
            scan_with(temp.path(), NAVIGATION_MANIFEST),
            Vec::new(),
        );

        assert!(loaded.config.navigation.is_split_editor("zed"));
        assert_eq!(
            loaded
                .config
                .navigation
                .editors
                .iter()
                .filter(|program| program.eq_ignore_ascii_case("nvim"))
                .count(),
            1
        );
        let nvim = loaded
            .config
            .navigation
            .registrations
            .iter()
            .find(|registration| registration.program == "nvim")
            .unwrap();
        assert_eq!(
            nvim.sources,
            [
                NavigationTargetSource::Builtin,
                NavigationTargetSource::Extension {
                    id: "vim-rozi".to_string(),
                    target: "vim".to_string(),
                },
            ]
        );
    }

    #[test]
    fn explicit_navigation_editors_completely_replace_extension_targets() {
        let temp = tempfile::tempdir().unwrap();
        for (text, expected) in [
            ("[navigation]\neditors = []\n", Vec::<String>::new()),
            ("[navigation]\neditors = [\"hx\"]\n", vec!["hx".to_string()]),
        ] {
            let loaded = load_config_from_text_with_extensions(
                text,
                Path::new("config.toml"),
                scan_with(temp.path(), NAVIGATION_MANIFEST),
                Vec::new(),
            );
            assert_eq!(loaded.config.navigation.editors, expected);
            assert!(!loaded.config.navigation.is_split_editor("zed"));
            assert!(
                loaded
                    .config
                    .navigation
                    .registrations
                    .iter()
                    .all(|registration| registration.sources == [NavigationTargetSource::User])
            );
        }
    }

    #[test]
    fn disabled_extensions_do_not_contribute_navigation_targets() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "[extensions]\ndisabled = [\"vim-rozi\"]\n",
            Path::new("config.toml"),
            scan_with(temp.path(), NAVIGATION_MANIFEST),
            Vec::new(),
        );

        assert!(!loaded.config.navigation.is_split_editor("zed"));
    }

    const KEY_MANIFEST: &str = "[extension]\nid = \"tasks\"\napi = 1\n\
         [[commands]]\nid = \"run\"\nsend = \"run\"\nkey = \"g r\"\n";
    const SUGGESTED_BINDING_MANIFEST: &str = "[extension]\nid = \"vim-rozi\"\napi = 1\n\
         [[suggested_keybindings]]\naction = \"smart-focus-left\"\nkey = \"ctrl-h\"\n\
         [[suggested_keybindings]]\naction = \"smart-focus-down\"\nkey = \"ctrl-j\"\n";

    #[test]
    fn extension_problem_count_survives_a_config_parse_failure() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "[",
            Path::new("config.toml"),
            scan_with(temp.path(), "[extension]\nid = \"invalid.id\"\napi = 1\n"),
            Vec::new(),
        );
        assert_eq!(loaded.config.extension_problems, 1);
    }

    /// A suggested chord is taken when nothing answers to it, and it is reachable as the command's
    /// shortcut rather than only from the palette.
    #[test]
    fn an_extension_chord_is_granted_when_the_prefix_space_is_free() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "",
            Path::new("config.toml"),
            scan_with(temp.path(), KEY_MANIFEST),
            Vec::new(),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let granted = &loaded.config.extension_key_defaults["tasks.run"];
        assert_eq!(granted[0].canonical_lowercase(), "ctrl+a x g r");
    }

    /// The weakest claim in the system: anything that already answers to the chord keeps it, and the
    /// extension is told rather than silently ignored.
    #[test]
    fn an_extension_chord_yields_to_builtins_and_to_the_user() {
        // A user binding inside the extension space still outranks a suggestion.
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "[keys]\ntoggle-sidebar = \"ctrl-a x g r\"\n",
            Path::new("config.toml"),
            scan_with(temp.path(), KEY_MANIFEST),
            Vec::new(),
        );
        assert!(loaded.config.extension_key_defaults.is_empty());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("already uses")),
            "{:?}",
            loaded.warnings
        );

        // A `[keys]` entry for the same command decides it outright: no suggestion, no warning.
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "[keys]\n\"tasks.run\" = \"ctrl-a z\"\n",
            Path::new("config.toml"),
            scan_with(temp.path(), KEY_MANIFEST),
            Vec::new(),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert!(loaded.config.extension_key_defaults.is_empty());
        assert_eq!(
            loaded.config.key_overrides["tasks.run"][0].canonical_lowercase(),
            "ctrl+a z"
        );
    }

    /// A chord that merely starts with an existing one is still a conflict: the first step would
    /// fire the other command before the second arrived.
    #[test]
    fn an_extension_chord_yields_to_a_shorter_chord_it_extends() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "[keys]\ntoggle-sidebar = \"ctrl-a x g\"\n",
            Path::new("config.toml"),
            scan_with(temp.path(), KEY_MANIFEST),
            Vec::new(),
        );
        assert!(loaded.config.extension_key_defaults.is_empty());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("already uses")),
            "{:?}",
            loaded.warnings
        );
    }

    #[test]
    fn suggested_action_bindings_apply_and_explicit_action_config_suppresses_them() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_config_from_text_with_extensions(
            "",
            Path::new("config.toml"),
            scan_with(temp.path(), SUGGESTED_BINDING_MANIFEST),
            Vec::new(),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(
            loaded.config.extension_action_key_defaults["smart-focus-left"][0]
                .canonical_lowercase(),
            "ctrl+h"
        );
        assert!(
            loaded
                .config
                .suggested_keybinding_resolutions
                .iter()
                .all(|binding| binding.status
                    == crate::config::ExtensionSuggestedKeybindingStatus::Active)
        );

        let loaded = load_config_from_text_with_extensions(
            "[keys]\nsmart-focus-left = []\n",
            Path::new("config.toml"),
            scan_with(temp.path(), SUGGESTED_BINDING_MANIFEST),
            Vec::new(),
        );
        assert!(
            !loaded
                .config
                .extension_action_key_defaults
                .contains_key("smart-focus-left")
        );
        assert_eq!(
            loaded
                .config
                .suggested_keybinding_resolutions
                .iter()
                .find(|binding| binding.action == "smart-focus-left")
                .unwrap()
                .status,
            crate::config::ExtensionSuggestedKeybindingStatus::Suppressed
        );
        assert!(
            loaded
                .config
                .extension_action_key_defaults
                .contains_key("smart-focus-down")
        );
    }

    /// The whole path an installed extension travels: scanned from disk, merged into the tab
    /// catalog, and placed in a panel so it is reachable without the user editing anything. The
    /// order matters — placement is resolved during the sidebar pass, so a tab merged after it
    /// would exist but belong to no panel.
    #[test]
    fn an_installed_extension_tab_is_merged_and_placed_by_the_loader() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("git-tools");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("extension.toml"),
            "[extension]\nid = \"git-tools\"\napi = 1\n\
             [[sidebar_tabs]]\nname = \"agents\"\nlabel = \"Agents\"\n\
             entries = [{ label = \"rozi\", run = \"claude\" }]\n",
        )
        .unwrap();

        let loaded = load_config_from_text_with_extensions(
            "",
            Path::new("config.toml"),
            super::super::extensions::scan_extensions_in(temp.path()),
            Vec::new(),
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let id = SidebarTabId::new("git-tools.agents");
        assert!(loaded.config.sidebar.tabs.iter().any(|tab| tab.id() == id));
        assert!(
            loaded
                .config
                .sidebar
                .panels
                .iter()
                .flatten()
                .any(|placed| *placed == id)
        );
        assert!(loaded.config.installed_extensions.contains("git-tools"));
    }

    #[test]
    fn sidebar_example_is_a_valid_warning_free_config() {
        let parsed: FileConfig = toml::from_str(include_str!("../../examples/sidebar.toml"))
            .expect("sidebar example parses");
        let mut sidebar = SidebarConfig::default();
        let mut warnings = Vec::new();
        apply_sidebar_config(&mut sidebar, parsed.sidebar, Vec::new(), &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(sidebar.tabs.len(), 9);
        assert_eq!(sidebar.panels.len(), 2);
    }

    #[test]
    fn default_navigation_recognizes_vim_family_case_insensitively() {
        let nav = NavigationConfig::default();
        assert!(nav.is_split_editor("nvim"));
        assert!(nav.is_split_editor("NVIM.EXE"));
        assert!(nav.is_split_editor("VIM"));
        assert!(nav.is_split_editor("vimdiff"));
        assert!(!nav.is_split_editor("bash"));
        assert!(!nav.is_split_editor("less"));
    }
}
