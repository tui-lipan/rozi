use std::collections::{BTreeMap, HashSet};

use crate::config::{NamedCommand, ServiceConfig};

use super::{
    EXTENSION_API_VERSION, ExtensionRuntimeFingerprint, ExtensionScan, ExtensionStatus,
    fingerprint, fingerprints_by_id,
};

pub(super) type ExtensionContributions = (
    Vec<NamedCommand>,
    Vec<ServiceConfig>,
    HashSet<String>,
    BTreeMap<String, ExtensionRuntimeFingerprint>,
    Vec<String>,
);

pub(super) fn build(mut scan: ExtensionScan, disabled: &[String]) -> ExtensionContributions {
    scan.apply_disabled(disabled);
    let mut commands = Vec::new();
    let mut services = Vec::new();
    let mut active_ids = HashSet::new();
    let mut runtime = Vec::new();
    let mut warnings = scan.root_errors;
    for extension in scan.extensions {
        if extension.info.status == ExtensionStatus::Loaded {
            if let Some(id) = extension.info.id.clone() {
                runtime.push((
                    id.clone(),
                    fingerprint(
                        extension.info.api.unwrap_or(EXTENSION_API_VERSION),
                        extension.info.path.clone(),
                        &extension.commands,
                        &extension.services,
                    ),
                ));
                active_ids.insert(id);
            }
            commands.extend(extension.commands);
            services.extend(extension.services);
        } else if extension.info.status != ExtensionStatus::Disabled {
            warnings.extend(
                extension
                    .info
                    .errors
                    .iter()
                    .map(|error| format!("extension `{}`: {error}", extension.info.display_name())),
            );
        }
    }
    (
        commands,
        services,
        active_ids,
        fingerprints_by_id(runtime),
        warnings,
    )
}
