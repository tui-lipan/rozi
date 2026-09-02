use std::collections::{HashMap, HashSet};

use tui_lipan::prelude::KeyBinding;

use crate::config::Config;

use super::{
    ExtensionSuggestedKeybindingDiagnostic, ExtensionSuggestedKeybindingStatus,
    SuggestedKeybindingContribution,
};

pub(crate) struct SuggestedKeybindingResolution {
    pub(crate) active: HashMap<String, Vec<KeyBinding>>,
    pub(crate) diagnostics: Vec<ExtensionSuggestedKeybindingDiagnostic>,
}

pub(crate) fn resolve(
    config: &Config,
    mut suggestions: Vec<SuggestedKeybindingContribution>,
    explicitly_configured_actions: &HashSet<String>,
    warnings: &mut Vec<String>,
) -> SuggestedKeybindingResolution {
    suggestions.sort_by(|left, right| suggestion_key(left).cmp(&suggestion_key(right)));

    let higher_priority = higher_priority_claims(config);
    let core_bound_actions = core_bound_actions(config);
    let mut diagnostics = suggestions
        .iter()
        .map(declared_diagnostic)
        .collect::<Vec<_>>();
    let pending = classify_by_precedence(
        &suggestions,
        &higher_priority,
        &core_bound_actions,
        explicitly_configured_actions,
        &mut diagnostics,
    );
    resolve_extension_conflicts(&suggestions, &pending, &mut diagnostics);
    let active = collect_active(&suggestions, &diagnostics, warnings);

    SuggestedKeybindingResolution {
        active,
        diagnostics,
    }
}

fn classify_by_precedence(
    suggestions: &[SuggestedKeybindingContribution],
    higher_priority: &[(String, String)],
    core_bound_actions: &HashSet<String>,
    explicitly_configured_actions: &HashSet<String>,
    diagnostics: &mut [ExtensionSuggestedKeybindingDiagnostic],
) -> Vec<usize> {
    let mut pending = Vec::new();
    for (index, suggestion) in suggestions.iter().enumerate() {
        match higher_priority_blocker(
            suggestion,
            higher_priority,
            core_bound_actions,
            explicitly_configured_actions,
        ) {
            Some((status, detail)) => {
                diagnostics[index].status = status;
                diagnostics[index].detail = Some(detail);
            }
            None => pending.push(index),
        }
    }
    pending
}

fn higher_priority_blocker(
    suggestion: &SuggestedKeybindingContribution,
    higher_priority: &[(String, String)],
    core_bound_actions: &HashSet<String>,
    explicitly_configured_actions: &HashSet<String>,
) -> Option<(ExtensionSuggestedKeybindingStatus, String)> {
    if explicitly_configured_actions.contains(&suggestion.action) {
        return Some((
            ExtensionSuggestedKeybindingStatus::Suppressed,
            format!("user configured action `{}`", suggestion.action),
        ));
    }
    if core_bound_actions.contains(&suggestion.action) {
        return Some((
            ExtensionSuggestedKeybindingStatus::Suppressed,
            format!("core provides bindings for `{}`", suggestion.action),
        ));
    }
    let canonical = suggestion.binding.canonical_lowercase();
    let (_, owner) = higher_priority
        .iter()
        .find(|(claimed, _)| bindings_conflict(&canonical, claimed))?;
    Some((
        ExtensionSuggestedKeybindingStatus::Conflict,
        format!("already bound to {owner}"),
    ))
}

fn resolve_extension_conflicts(
    suggestions: &[SuggestedKeybindingContribution],
    pending: &[usize],
    diagnostics: &mut [ExtensionSuggestedKeybindingDiagnostic],
) {
    for &index in pending {
        let suggestion = &suggestions[index];
        let peers = conflicting_peer_labels(suggestions, pending, suggestion);
        if !peers.is_empty() {
            diagnostics[index].status = ExtensionSuggestedKeybindingStatus::Conflict;
            diagnostics[index].detail = Some(format!("also suggested as {}", peers.join(", ")));
        } else {
            diagnostics[index].status = ExtensionSuggestedKeybindingStatus::Active;
        }
    }
}

fn conflicting_peer_labels(
    suggestions: &[SuggestedKeybindingContribution],
    pending: &[usize],
    suggestion: &SuggestedKeybindingContribution,
) -> Vec<String> {
    let canonical = suggestion.binding.canonical_lowercase();
    let mut peers = pending
        .iter()
        .copied()
        .filter(|other_index| {
            let other = &suggestions[*other_index];
            other.action != suggestion.action
                && bindings_conflict(&canonical, &other.binding.canonical_lowercase())
        })
        .map(|other_index| {
            let other = &suggestions[other_index];
            format!(
                "`{}` for `{}` from `{}`",
                other.key, other.action, other.extension_id
            )
        })
        .collect::<Vec<_>>();
    peers.sort();
    peers.dedup();
    peers
}

fn collect_active(
    suggestions: &[SuggestedKeybindingContribution],
    diagnostics: &[ExtensionSuggestedKeybindingDiagnostic],
    warnings: &mut Vec<String>,
) -> HashMap<String, Vec<KeyBinding>> {
    let mut active: HashMap<String, Vec<KeyBinding>> = HashMap::new();
    for (suggestion, diagnostic) in suggestions.iter().zip(diagnostics) {
        if diagnostic.status != ExtensionSuggestedKeybindingStatus::Active {
            push_conflict_warning(suggestion, diagnostic, warnings);
            continue;
        }
        let bindings = active.entry(suggestion.action.clone()).or_default();
        if !bindings.contains(&suggestion.binding) {
            bindings.push(suggestion.binding.clone());
        }
    }
    active
}

fn push_conflict_warning(
    suggestion: &SuggestedKeybindingContribution,
    diagnostic: &ExtensionSuggestedKeybindingDiagnostic,
    warnings: &mut Vec<String>,
) {
    if diagnostic.status != ExtensionSuggestedKeybindingStatus::Conflict {
        return;
    }
    warnings.push(format!(
        "Extension `{}` suggests `{}` for `{}`, but it conflicts: {}",
        suggestion.extension_id,
        suggestion.key,
        suggestion.action,
        diagnostic.detail.as_deref().unwrap_or("key unavailable")
    ));
}

fn suggestion_key(suggestion: &SuggestedKeybindingContribution) -> (String, &str, &str) {
    (
        suggestion.binding.canonical_lowercase(),
        suggestion.action.as_str(),
        suggestion.extension_id.as_str(),
    )
}

fn declared_diagnostic(
    suggestion: &SuggestedKeybindingContribution,
) -> ExtensionSuggestedKeybindingDiagnostic {
    ExtensionSuggestedKeybindingDiagnostic {
        extension_id: suggestion.extension_id.clone(),
        action: suggestion.action.clone(),
        key: suggestion.key.clone(),
        status: ExtensionSuggestedKeybindingStatus::Declared,
        detail: None,
    }
}

fn higher_priority_claims(config: &Config) -> Vec<(String, String)> {
    let mut claimed = crate::commands::core_default_shortcuts(&config.input)
        .into_iter()
        .filter(|(id, _)| !config.key_overrides.contains_key(id))
        .map(|(id, binding)| (binding.canonical_lowercase(), format!("core action `{id}`")))
        .collect::<Vec<_>>();
    for (id, bindings) in &config.key_overrides {
        for binding in bindings {
            claimed.push((
                binding.canonical_lowercase(),
                format!("user binding for `{id}`"),
            ));
        }
    }
    for command in &config.user_commands {
        for binding in &command.bindings {
            claimed.push((
                binding.canonical_lowercase(),
                "a user `[keys]` command".to_string(),
            ));
        }
    }
    for (id, bindings) in &config.extension_key_defaults {
        for binding in bindings {
            claimed.push((
                binding.canonical_lowercase(),
                format!("extension command `{id}`"),
            ));
        }
    }
    claimed
}

fn core_bound_actions(config: &Config) -> HashSet<String> {
    crate::commands::extension_bindable_action_ids()
        .iter()
        .filter(|id| !config.key_overrides.contains_key(**id))
        .filter(|id| {
            crate::commands::default_shortcuts_for_action(&config.input, id)
                .is_some_and(|bindings| !bindings.is_empty())
        })
        .map(|id| (*id).to_string())
        .collect()
}

fn bindings_conflict(left: &str, right: &str) -> bool {
    left == right
        || left.starts_with(&format!("{right} "))
        || right.starts_with(&format!("{left} "))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn suggestion(extension_id: &str, action: &str, key: &str) -> SuggestedKeybindingContribution {
        SuggestedKeybindingContribution {
            extension_id: extension_id.to_string(),
            action: action.to_string(),
            key: key.to_string(),
            binding: KeyBinding::from_str(key).unwrap(),
        }
    }

    #[test]
    fn identical_suggestions_deduplicate_but_keep_provenance() {
        let resolved = resolve(
            &Config::default(),
            vec![
                suggestion("vim-one", "smart-focus-left", "ctrl-h"),
                suggestion("vim-two", "smart-focus-left", "ctrl-h"),
            ],
            &HashSet::new(),
            &mut Vec::new(),
        );
        assert_eq!(resolved.active["smart-focus-left"].len(), 1);
        assert!(
            resolved
                .diagnostics
                .iter()
                .all(|item| item.status == ExtensionSuggestedKeybindingStatus::Active)
        );
    }

    #[test]
    fn extension_conflicts_are_symmetric_and_discovery_order_independent() {
        let suggestions = vec![
            suggestion("one", "smart-focus-left", "ctrl-h"),
            suggestion("two", "smart-focus-right", "ctrl-h"),
        ];
        for suggestions in [suggestions.clone(), suggestions.into_iter().rev().collect()] {
            let resolved = resolve(
                &Config::default(),
                suggestions,
                &HashSet::new(),
                &mut Vec::new(),
            );
            assert!(resolved.active.is_empty());
            assert!(
                resolved
                    .diagnostics
                    .iter()
                    .all(|item| item.status == ExtensionSuggestedKeybindingStatus::Conflict)
            );
        }
    }

    #[test]
    fn different_free_keys_for_one_action_are_all_granted() {
        let resolved = resolve(
            &Config::default(),
            vec![
                suggestion("one", "smart-focus-left", "ctrl-h"),
                suggestion("two", "smart-focus-left", "ctrl-shift-h"),
            ],
            &HashSet::new(),
            &mut Vec::new(),
        );
        assert_eq!(resolved.active["smart-focus-left"].len(), 2);
        assert!(
            resolved
                .diagnostics
                .iter()
                .all(|item| item.status == ExtensionSuggestedKeybindingStatus::Active)
        );
    }

    #[test]
    fn explicit_action_configuration_suppresses_only_that_action() {
        let resolved = resolve(
            &Config::default(),
            vec![
                suggestion("vim", "smart-focus-left", "ctrl-h"),
                suggestion("vim", "smart-focus-right", "ctrl-l"),
            ],
            &HashSet::from(["smart-focus-left".to_string()]),
            &mut Vec::new(),
        );
        assert!(!resolved.active.contains_key("smart-focus-left"));
        assert_eq!(resolved.active["smart-focus-right"].len(), 1);
        assert_eq!(
            resolved.diagnostics[0].status,
            ExtensionSuggestedKeybindingStatus::Suppressed
        );
    }

    #[test]
    fn occupied_keys_conflict_without_disabling_other_suggestions() {
        let mut config = Config::default();
        config.key_overrides.insert(
            "spawn".to_string(),
            vec![KeyBinding::from_str("ctrl-h").unwrap()],
        );
        let resolved = resolve(
            &config,
            vec![
                suggestion("vim", "smart-focus-left", "ctrl-h"),
                suggestion("vim", "smart-focus-down", "ctrl-j"),
            ],
            &HashSet::new(),
            &mut Vec::new(),
        );
        assert!(!resolved.active.contains_key("smart-focus-left"));
        assert_eq!(resolved.active["smart-focus-down"].len(), 1);
        assert_eq!(
            resolved.diagnostics[0].status,
            ExtensionSuggestedKeybindingStatus::Conflict
        );
    }

    #[test]
    fn core_direct_defaults_outrank_extension_suggestions() {
        let resolved = resolve(
            &Config::default(),
            vec![suggestion("vim", "smart-focus-left", "ctrl-v")],
            &HashSet::new(),
            &mut Vec::new(),
        );
        assert!(resolved.active.is_empty());
        assert_eq!(
            resolved.diagnostics[0].status,
            ExtensionSuggestedKeybindingStatus::Conflict
        );
        assert!(
            resolved.diagnostics[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("core action `paste`"))
        );
    }
}
