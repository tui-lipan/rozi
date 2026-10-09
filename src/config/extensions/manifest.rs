use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionManifestFile {
    pub(super) extension: ExtensionMetadataFile,
    #[serde(default)]
    pub(super) commands: Vec<ExtensionCommandFile>,
    #[serde(default)]
    pub(super) link_handlers: Vec<ExtensionLinkHandlerFile>,
    #[serde(default)]
    pub(super) services: Vec<ExtensionServiceFile>,
    /// Agent definitions this extension teaches Rozi. Same format as `config.toml`'s
    /// `[[agents]]`; ids are namespaced `<extension>.<id>` like commands and services are.
    #[serde(default)]
    pub(super) agents: Vec<crate::agent_detection::AgentSpec>,
    /// Sidebar tabs this extension contributes. Same launcher and command forms `config.toml`
    /// accepts, under namespaced ids so an extension can only ever add a tab.
    #[serde(default)]
    pub(super) sidebar_tabs: Vec<ExtensionSidebarTabFile>,
    #[serde(default)]
    pub(super) sidebar_presets: Vec<crate::config::SidebarLayoutPreset>,
    /// Settings this extension understands, with the value each one takes when the user says
    /// nothing. Declaring them is what makes a user override checkable and what gives
    /// `extensions check` something to show; an undeclared key is not a setting.
    #[serde(default)]
    pub(super) settings: BTreeMap<String, toml::Value>,
    /// Static split-aware foreground-program declarations. Rozi compiles these into core
    /// navigation policy when no explicit `[navigation] editors` list replaces the defaults.
    #[serde(default)]
    pub(super) navigation_targets: Vec<ExtensionNavigationTargetFile>,
    /// Best-effort bindings for explicitly extension-bindable core actions. These are resolved
    /// after user configuration and built-in defaults; they never intercept input at runtime.
    #[serde(default)]
    pub(super) suggested_keybindings: Vec<ExtensionSuggestedKeybindingFile>,
}

/// One `[[sidebar_tabs]]` entry. The tree-only options `config.toml` tab tables accept are absent:
/// `files` and `git` are built-in tabs an extension cannot reconfigure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionSidebarTabFile {
    pub(super) suggested_location: Option<crate::config::SidebarTabLocation>,
    pub(super) name: Option<String>,
    pub(super) label: Option<String>,
    pub(super) entries: Option<Vec<super::super::file::SidebarLauncherEntrySpec>>,
    pub(super) command: Option<String>,
    pub(super) interval: Option<u64>,
    pub(super) on_click: Option<super::super::file::UserCommandTableSpec>,
    pub(super) group_prefix: Option<String>,
    /// Where the listing command and an `on_click` `exec` run. See [`super::placement`].
    pub(super) placement: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionCommandFile {
    pub(super) id: Option<String>,
    pub(super) label: Option<String>,
    /// Suggested chord, written as the key steps that follow the user's leader prefix (`"g b"`
    /// means `<prefix> g b`). Never a bare key and never the held-modifier layer: an extension
    /// proposes a shortcut inside the prefix space, it does not take one.
    pub(super) key: Option<String>,
    pub(super) exec: Option<Vec<String>>,
    pub(super) shell: Option<String>,
    pub(super) send: Option<String>,
    /// Start out of the command palette until the extension shows it with `command-visibility`.
    pub(super) hidden: Option<bool>,
    /// Where the command's process runs. See [`super::placement`].
    pub(super) placement: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionServiceFile {
    pub(super) name: Option<String>,
    pub(super) exec: Option<Vec<String>>,
    pub(super) shell: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) restart: Option<String>,
    #[serde(default)]
    pub(super) env: BTreeMap<String, String>,
    /// Where, and how many, instances run. See [`super::placement`].
    pub(super) placement: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionMetadataFile {
    pub(super) id: Option<String>,
    pub(super) title: Option<String>,
    pub(super) description: Option<String>,
    pub(super) version: Option<String>,
    pub(super) api: Option<u32>,
    /// Oldest Rozi this extension works with, as a semver version. Unlike `api`, which is a single
    /// number that either matches or does not, this expresses "anything from here up" - the usual
    /// shape of a dependency on a feature that was added once and stayed.
    pub(super) min_rozi: Option<String>,
    /// Operating systems this extension runs on, named as Rust names them (`linux`, `macos`,
    /// `windows`, `freebsd`, `netbsd`). Empty means every platform, which is the common case and
    /// stays the default.
    #[serde(default)]
    pub(super) platforms: Vec<String>,
    /// Where to read more. Discovery metadata only: Rozi never fetches it.
    pub(super) homepage: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionNavigationTargetFile {
    pub(super) name: Option<String>,
    pub(super) programs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionSuggestedKeybindingFile {
    pub(super) action: Option<String>,
    pub(super) key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserExtensionConfig {
    #[serde(default)]
    pub(crate) disabled: Vec<String>,
    #[serde(flatten)]
    pub(crate) settings: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionLinkHandlerFile {
    pub(super) schemes: Vec<String>,
    pub(super) command: String,
}
