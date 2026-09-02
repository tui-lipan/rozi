use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionManifestFile {
    pub(super) extension: ExtensionMetadataFile,
    #[serde(default)]
    pub(super) commands: Vec<ExtensionCommandFile>,
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
    /// Settings this extension understands, with the value each one takes when the user says
    /// nothing. Declaring them is what makes a user override checkable and what gives
    /// `extensions check` something to show; an undeclared key is not a setting.
    #[serde(default)]
    pub(super) settings: BTreeMap<String, toml::Value>,
    /// Static split-aware foreground-program declarations. Rozi compiles these into core
    /// navigation policy when no explicit `[navigation] editors` list replaces the defaults.
    #[serde(default)]
    pub(super) navigation_targets: Vec<ExtensionNavigationTargetFile>,
}

/// One `[[sidebar_tabs]]` entry. The tree-only options `config.toml` tab tables accept are absent:
/// `files` and `git` are built-in tabs an extension cannot reconfigure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionSidebarTabFile {
    pub(super) name: Option<String>,
    pub(super) label: Option<String>,
    pub(super) entries: Option<Vec<super::super::file::SidebarLauncherEntrySpec>>,
    pub(super) command: Option<String>,
    pub(super) interval: Option<u64>,
    pub(super) on_click: Option<super::super::file::UserCommandTableSpec>,
    pub(super) group_prefix: Option<String>,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionMetadataFile {
    pub(super) id: Option<String>,
    pub(super) title: Option<String>,
    pub(super) description: Option<String>,
    pub(super) version: Option<String>,
    pub(super) api: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionNavigationTargetFile {
    pub(super) name: Option<String>,
    pub(super) programs: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserExtensionConfig {
    #[serde(default)]
    pub(crate) disabled: Vec<String>,
    #[serde(flatten)]
    pub(crate) settings: BTreeMap<String, toml::Value>,
}
