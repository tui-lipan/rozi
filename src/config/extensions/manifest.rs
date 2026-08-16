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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionCommandFile {
    pub(super) id: Option<String>,
    pub(super) label: Option<String>,
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

#[derive(Debug, Default, Deserialize)]
pub(super) struct ExtensionSettingsOnly {
    #[serde(default)]
    pub(super) extensions: ExtensionDisabledOnly,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ExtensionDisabledOnly {
    #[serde(default)]
    pub(super) disabled: Vec<String>,
}
