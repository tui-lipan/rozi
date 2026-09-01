use std::collections::{BTreeMap, HashSet};

use crate::config::{NamedCommand, ServiceConfig, SidebarTab};

use super::{
    EXTENSION_API_VERSION, ExtensionRuntimeFingerprint, ExtensionScan, ExtensionStatus,
    SETTINGS_ENV, fingerprint, fingerprints_by_id, settings,
};

/// Everything a scan of the extension directory contributes to a loaded [`crate::config::Config`].
#[derive(Debug, Default)]
pub(crate) struct ExtensionContributions {
    pub(crate) commands: Vec<NamedCommand>,
    pub(crate) services: Vec<ServiceConfig>,
    pub(crate) agents: Vec<crate::agent_detection::AgentDefinition>,
    pub(crate) sidebar_tabs: Vec<SidebarTab>,
    pub(crate) active_ids: HashSet<String>,
    /// Every extension present on disk, whatever its status. A sidebar placement naming a tab from
    /// one of these is kept rather than pruned: the extension is here, it just is not contributing
    /// right now, and the user's arrangement should survive disabling or a broken update.
    pub(crate) installed_ids: HashSet<String>,
    pub(crate) runtime: BTreeMap<String, ExtensionRuntimeFingerprint>,
    pub(crate) warnings: Vec<String>,
    pub(crate) problem_count: usize,
}

pub(super) fn build(
    mut scan: ExtensionScan,
    disabled: &[String],
    user_settings: &BTreeMap<String, toml::Value>,
) -> ExtensionContributions {
    scan.apply_disabled(disabled);
    let mut commands = Vec::new();
    let mut services = Vec::new();
    let mut agents = Vec::new();
    let mut sidebar_tabs = Vec::new();
    let mut active_ids = HashSet::new();
    let mut installed_ids = HashSet::new();
    let mut runtime = Vec::new();
    let mut problem_count = 0;
    let mut warnings = scan.root_errors;
    for extension in scan.extensions {
        if let Some(id) = extension.info.id.clone() {
            installed_ids.insert(id);
        }
        if extension.info.status == ExtensionStatus::Loaded {
            let id = extension.info.id.clone().unwrap_or_default();
            // Settings reach a process the same way its identity does: as environment. They are
            // resolved here because this is the first point holding both the manifest's declaration
            // and the user's overrides, and they are injected before the fingerprint is taken, so
            // changing one rotates the generation and restarts the services that read it.
            let merged = settings::merge(
                &extension.settings,
                &id,
                user_settings.get(&id),
                &mut warnings,
            );
            let value = settings::env_value(&merged);
            let mut extension_commands = extension.commands;
            for command in &mut extension_commands {
                command.env.push((SETTINGS_ENV.to_string(), value.clone()));
            }
            let mut extension_services = extension.services;
            for service in &mut extension_services {
                service.env.insert(SETTINGS_ENV.to_string(), value.clone());
            }
            let mut extension_tabs = extension.sidebar_tabs;
            for tab in &mut extension_tabs {
                if let SidebarTab::Launcher { env, .. } | SidebarTab::Command { env, .. } = tab {
                    env.push((SETTINGS_ENV.to_string(), value.clone()));
                }
            }
            if extension.info.id.is_some() {
                runtime.push((
                    id.clone(),
                    fingerprint(
                        extension.info.api.unwrap_or(EXTENSION_API_VERSION),
                        extension.info.path.clone(),
                        &extension_commands,
                        &extension_services,
                    ),
                ));
                active_ids.insert(id);
            }
            commands.extend(extension_commands);
            services.extend(extension_services);
            agents.extend(extension.agents);
            sidebar_tabs.extend(extension_tabs);
        } else if extension.info.status != ExtensionStatus::Disabled {
            problem_count += 1;
            warnings.extend(
                extension
                    .info
                    .errors
                    .iter()
                    .map(|error| format!("extension `{}`: {error}", extension.info.display_name())),
            );
        }
    }
    // A settings table naming nothing installed is a typo or a leftover from an extension that was
    // removed. Being disabled is not enough to earn this warning: the extension is still there, and
    // the settings are waiting for it to come back.
    for id in user_settings.keys() {
        if !installed_ids.contains(id) {
            warnings.push(format!(
                "`[extensions.{id}]` configures no installed extension; ignored"
            ));
        }
    }
    ExtensionContributions {
        commands,
        services,
        agents,
        sidebar_tabs,
        active_ids,
        installed_ids,
        runtime: fingerprints_by_id(runtime),
        warnings,
        problem_count,
    }
}
