use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::config::{Config, ServiceConfig, UserCommandAction};

pub const GENERATION_ENV: &str = "ROZI_EXTENSION_GENERATION";

/// Provenance carried automatically by the CLI for a process launched by an extension.
///
/// The opaque generation is a fencing token, not authentication: it rejects processes from a
/// retired runtime definition, but another process running as the same user is inside Rozi's trust
/// boundary and may be able to observe it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ExtensionProvenance {
    pub id: String,
    pub generation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExtensionCommandFingerprint {
    pub(crate) id: String,
    pub(crate) action: UserCommandAction,
    pub(crate) env: Vec<(String, String)>,
}

/// Canonical process-facing extension definition.
///
/// Presentation metadata is deliberately absent: labels, title, description, and package version
/// may update without restarting services or fencing otherwise unchanged extension processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExtensionRuntimeFingerprint {
    pub(crate) api: u32,
    pub(crate) directory: String,
    pub(crate) commands: Vec<ExtensionCommandFingerprint>,
    pub(crate) services: Vec<ServiceConfig>,
}

pub(crate) fn reconcile_generations(
    old: Option<&Config>,
    new: &mut Config,
    current: &HashMap<String, String>,
) -> (HashMap<String, String>, HashSet<String>) {
    let mut generations = HashMap::new();
    let mut retired = HashSet::new();

    for (id, fingerprint) in &new.extension_runtime {
        let unchanged = old
            .and_then(|config| config.extension_runtime.get(id))
            .is_some_and(|previous| previous == fingerprint);
        let generation = if unchanged {
            current.get(id).cloned().unwrap_or_else(fresh_token)
        } else {
            if old.is_some_and(|config| config.extension_runtime.contains_key(id)) {
                retired.insert(id.clone());
            }
            fresh_token()
        };
        generations.insert(id.clone(), generation);
    }

    if let Some(old) = old {
        retired.extend(
            old.extension_runtime
                .keys()
                .filter(|id| !new.extension_runtime.contains_key(*id))
                .cloned(),
        );
    }
    retired.extend(
        current
            .keys()
            .filter(|id| !new.extension_runtime.contains_key(*id))
            .cloned(),
    );

    inject_generation_env(new, &generations);
    (generations, retired)
}

pub(crate) fn provenance_from_process() -> Option<ExtensionProvenance> {
    let id = std::env::var("ROZI_EXTENSION").ok()?;
    Some(ExtensionProvenance {
        id,
        generation: std::env::var(GENERATION_ENV).unwrap_or_default(),
    })
}

pub(crate) fn provenance_is_active(
    active: &HashMap<String, String>,
    provenance: &ExtensionProvenance,
) -> bool {
    active
        .get(&provenance.id)
        .is_some_and(|generation| generation == &provenance.generation)
}

fn inject_generation_env(config: &mut Config, generations: &HashMap<String, String>) {
    for command in &mut config.commands {
        let Some(id) = extension_id_from_pairs(&command.env) else {
            continue;
        };
        if let Some(generation) = generations.get(id) {
            command
                .env
                .retain(|(key, _)| key.as_str() != GENERATION_ENV);
            command
                .env
                .push((GENERATION_ENV.to_string(), generation.clone()));
        }
    }
    for service in &mut config.services {
        let Some(id) = service.env.get("ROZI_EXTENSION") else {
            continue;
        };
        if let Some(generation) = generations.get(id) {
            service
                .env
                .insert(GENERATION_ENV.to_string(), generation.clone());
        }
    }
}

fn extension_id_from_pairs(env: &[(String, String)]) -> Option<&str> {
    env.iter()
        .find(|(key, _)| key == "ROZI_EXTENSION")
        .map(|(_, value)| value.as_str())
}

fn fresh_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    token
}

pub(crate) fn fingerprint(
    api: u32,
    directory: String,
    commands: &[crate::config::NamedCommand],
    services: &[ServiceConfig],
) -> ExtensionRuntimeFingerprint {
    ExtensionRuntimeFingerprint {
        api,
        directory,
        commands: commands
            .iter()
            .map(|command| ExtensionCommandFingerprint {
                id: command.id.clone(),
                action: command.action.clone(),
                env: command.env.clone(),
            })
            .collect(),
        services: services.to_vec(),
    }
}

pub(crate) fn fingerprints_by_id(
    entries: impl IntoIterator<Item = (String, ExtensionRuntimeFingerprint)>,
) -> BTreeMap<String, ExtensionRuntimeFingerprint> {
    entries.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(run: &str, label: &str) -> Config {
        let mut config = Config::default();
        let id = "tools".to_string();
        let env = vec![
            ("ROZI_EXTENSION".to_string(), id.clone()),
            (
                "ROZI_EXTENSION_DIR".to_string(),
                "/extensions/tools".to_string(),
            ),
        ];
        let command = crate::config::NamedCommand {
            id: "tools.open".to_string(),
            label: Some(label.to_string()),
            action: UserCommandAction::Exec {
                command: run.to_string(),
            },
            category: "Tools".to_string(),
            env: env.clone(),
        };
        config.commands.push(command.clone());
        config.active_extensions.insert(id.clone());
        config.extension_runtime.insert(
            id,
            fingerprint(1, "/extensions/tools".to_string(), &[command], &[]),
        );
        config
    }

    #[test]
    fn unchanged_runtime_preserves_token_while_runtime_change_rotates_it() {
        let mut first = config("one", "Old label");
        let (tokens, retired) = reconcile_generations(None, &mut first, &HashMap::new());
        assert!(retired.is_empty());
        let first_token = tokens["tools"].clone();

        let mut presentation_only = config("one", "New label");
        let (same, retired) = reconcile_generations(Some(&first), &mut presentation_only, &tokens);
        assert!(retired.is_empty());
        assert_eq!(same["tools"], first_token);

        let mut changed = config("two", "New label");
        let (second, retired) =
            reconcile_generations(Some(&presentation_only), &mut changed, &same);
        assert!(retired.contains("tools"));
        assert_ne!(second["tools"], first_token);
    }

    #[test]
    fn reverting_a_definition_never_reuses_its_original_token() {
        let mut a = config("a", "A");
        let (a_tokens, _) = reconcile_generations(None, &mut a, &HashMap::new());
        let original = a_tokens["tools"].clone();

        let mut b = config("b", "B");
        let (b_tokens, _) = reconcile_generations(Some(&a), &mut b, &a_tokens);
        let mut a_again = config("a", "A");
        let (a_again_tokens, _) = reconcile_generations(Some(&b), &mut a_again, &b_tokens);

        assert_ne!(a_again_tokens["tools"], original);
        assert_ne!(a_again_tokens["tools"], b_tokens["tools"]);
    }

    #[test]
    fn fresh_rozi_runtimes_never_reuse_a_generation_counter() {
        let mut first = config("same", "Same");
        let (first_tokens, _) = reconcile_generations(None, &mut first, &HashMap::new());
        let mut second = config("same", "Same");
        let (second_tokens, _) = reconcile_generations(None, &mut second, &HashMap::new());
        assert_ne!(first_tokens["tools"], second_tokens["tools"]);
    }
}
