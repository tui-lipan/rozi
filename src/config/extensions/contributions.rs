use std::collections::{BTreeMap, HashSet};

use crate::config::{NamedCommand, NavigationTargetContribution, ServiceConfig, SidebarTab};

use super::{
    EXTENSION_API_VERSION, ExtensionRuntimeFingerprint, ExtensionScan, ExtensionStatus,
    SETTINGS_ENV, SuggestedKeybindingContribution, fingerprint, fingerprints_by_id, settings,
};

/// Everything a scan of the extension directory contributes to a loaded [`crate::config::Config`].
#[derive(Debug, Default)]
pub(crate) struct ExtensionContributions {
    pub(crate) commands: Vec<NamedCommand>,
    pub(crate) services: Vec<ServiceConfig>,
    pub(crate) agents: Vec<crate::agent_detection::AgentDefinition>,
    pub(crate) sidebar_tabs: Vec<SidebarTab>,
    pub(crate) sidebar_locations: BTreeMap<String, crate::config::SidebarTabLocation>,
    pub(crate) sidebar_presets: Vec<crate::config::SidebarLayoutPreset>,
    pub(crate) navigation_targets: Vec<NavigationTargetContribution>,
    pub(crate) suggested_keybindings: Vec<SuggestedKeybindingContribution>,
    pub(crate) active_ids: HashSet<String>,
    /// Every extension present on disk, whatever its status. A sidebar placement naming a tab from
    /// one of these is kept rather than pruned: the extension is here, it just is not contributing
    /// right now, and the user's arrangement should survive disabling or a broken update.
    pub(crate) installed_ids: HashSet<String>,
    pub(crate) runtime: BTreeMap<String, ExtensionRuntimeFingerprint>,
    /// Placed contributions per loaded extension that has any.
    pub(crate) placements: BTreeMap<String, super::placement::SharedPlacements>,
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
    let mut sidebar_locations = BTreeMap::new();
    let mut sidebar_presets = Vec::new();
    let mut navigation_targets = Vec::new();
    let mut suggested_keybindings = Vec::new();
    let mut active_ids = HashSet::new();
    let mut installed_ids = HashSet::new();
    let mut runtime = Vec::new();
    let mut placements = BTreeMap::new();
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
            // A placed process gets its settings from the same merge; the placement keeps its own
            // copy of the environment because it is started away from `extension_services`.
            let extension_placements = extension.placements.map(|mut placed| {
                for service in &mut placed.services {
                    service.env.insert(SETTINGS_ENV.to_string(), value.clone());
                }
                std::sync::Arc::new(placed)
            });
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
                        extension_placements.clone(),
                    ),
                ));
                if let Some(placed) = extension_placements {
                    placements.insert(id.clone(), placed);
                }
                active_ids.insert(id);
            }
            commands.extend(extension_commands);
            services.extend(extension_services);
            agents.extend(extension.agents);
            sidebar_tabs.extend(extension_tabs);
            sidebar_locations.extend(extension.sidebar_locations);
            sidebar_presets.extend(extension.sidebar_presets);
            navigation_targets.extend(extension.navigation_targets);
            suggested_keybindings.extend(extension.suggested_keybindings);
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
        sidebar_locations,
        sidebar_presets,
        navigation_targets,
        suggested_keybindings,
        active_ids,
        installed_ids,
        runtime: fingerprints_by_id(runtime),
        placements,
        warnings,
        problem_count,
    }
}
