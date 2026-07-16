use std::collections::HashMap;
use std::str::FromStr;

use tui_lipan::prelude::KeyBinding;

use super::file::{KeyBindingSpec, UserCommandTableSpec};
use super::schema::{InputConfig, UserCommand, UserCommandAction, WmModifier, scheme_shortcuts};

pub(super) fn apply_input_config(
    input: &mut InputConfig,
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
    warnings: &mut Vec<String>,
) {
    if let Some(modifier_shortcuts) = modifier_shortcuts {
        input.modifier_shortcuts = modifier_shortcuts;
    }
    if let Some(modifier) = modifier {
        match parse_modifier(&modifier) {
            Some(parsed) => input.modifier = parsed,
            None => warnings.push(format!(
                "Unknown modifier `{modifier}`; expected `alt` or `super`"
            )),
        }
    }
    if let Some(prefix) = prefix {
        match KeyBinding::from_str(&prefix) {
            Ok(parsed) if parsed.step_count() == 1 => input.prefix = parsed,
            Ok(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; prefix must be a single key"
            )),
            Err(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; try e.g. `ctrl-a`"
            )),
        }
    }
}

/// True when a `[keys]` candidate is a bare key step: a single chord step carrying at most
/// `shift`. Such bindings are expanded through the configured input scheme.
fn is_bare_key_step(candidate: &str) -> bool {
    let mut steps = candidate.split_whitespace();
    let (Some(step), None) = (steps.next(), steps.next()) else {
        return false;
    };
    const MODIFIERS: &[&str] = &[
        "ctrl", "control", "alt", "option", "super", "cmd", "command", "meta", "win", "windows",
    ];
    !step
        .split(['-', '+'])
        .any(|token| MODIFIERS.contains(&token.to_ascii_lowercase().as_str()))
}

/// Build `[keys]` overrides. Strings/lists replace an action's defaults, `{ add = ... }` extends
/// its generated defaults, and `{ run = ... }` / `{ send = ... }` defines a user command.
pub(super) fn build_key_overrides(
    keys: HashMap<String, KeyBindingSpec>,
    input: &InputConfig,
    user_commands: &mut Vec<UserCommand>,
    warnings: &mut Vec<String>,
) -> HashMap<String, Vec<KeyBinding>> {
    let mut overrides = HashMap::new();
    for (key, spec) in keys {
        if let KeyBindingSpec::UserCommand(table) = spec {
            bind_user_command(user_commands, &key, table, warnings);
            continue;
        }

        let Some(default_bindings) = crate::commands::default_shortcuts_for_action(input, &key)
        else {
            warnings.push(format!("Unknown key action `{key}`; skipped"));
            continue;
        };

        let (bindings, additive) = match spec {
            KeyBindingSpec::One(value) => (vec![value], false),
            KeyBindingSpec::Many(values) => (values, false),
            KeyBindingSpec::Add(table) => (table.add.into_vec(), true),
            KeyBindingSpec::UserCommand(_) => unreachable!(),
        };

        let mut parsed_bindings = if additive {
            default_bindings
        } else {
            Vec::new()
        };
        let mut candidate_count = 0;
        let initial_binding_count = parsed_bindings.len();
        for binding in bindings {
            for candidate in binding
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                candidate_count += 1;
                let scheme_key = if let Some(key_step) = candidate.strip_prefix("scheme:") {
                    match KeyBinding::from_str(key_step) {
                        Ok(binding) if binding.step_count() == 1 => Some(key_step),
                        _ => {
                            warnings.push(format!(
                                "Could not parse scheme binding `{candidate}` for `{key}`; expected one key step"
                            ));
                            continue;
                        }
                    }
                } else {
                    is_bare_key_step(candidate).then_some(candidate)
                };
                if let Some(key_step) = scheme_key {
                    let expanded = scheme_shortcuts(input, key_step);
                    if expanded.is_empty() {
                        warnings.push(format!(
                            "Could not parse binding `{candidate}` for `{key}`; skipped"
                        ));
                    } else {
                        for binding in expanded {
                            if !parsed_bindings.contains(&binding) {
                                parsed_bindings.push(binding);
                            }
                        }
                    }
                    continue;
                }
                match KeyBinding::from_str(candidate) {
                    Ok(parsed) => {
                        if !parsed_bindings.contains(&parsed) {
                            parsed_bindings.push(parsed);
                        }
                    }
                    Err(_) => warnings.push(format!(
                        "Could not parse binding `{candidate}` for `{key}`; skipped"
                    )),
                }
            }
        }
        if candidate_count > 0 && parsed_bindings.len() == initial_binding_count && !additive {
            warnings.push(format!(
                "No valid bindings for `{key}`; keeping its default shortcuts"
            ));
            continue;
        }
        overrides.insert(key, parsed_bindings);
    }
    overrides
}

pub(super) fn bind_user_command(
    user_commands: &mut Vec<UserCommand>,
    key: &str,
    table: UserCommandTableSpec,
    warnings: &mut Vec<String>,
) {
    let Some(action) = parse_user_command_action(table, &format!("User command `{key}`"), warnings)
    else {
        return;
    };
    let Ok(binding) = KeyBinding::from_str(key) else {
        warnings.push(format!(
            "Could not parse binding `{key}` for a user command; skipped"
        ));
        return;
    };
    user_commands.push(UserCommand { action, binding });
}

pub(super) fn parse_user_command_action(
    table: UserCommandTableSpec,
    context: &str,
    warnings: &mut Vec<String>,
) -> Option<UserCommandAction> {
    // Preserve `send` byte-for-byte because trailing whitespace often submits the command.
    let run = table.run.map(|value| value.trim().to_string());
    let send = table.send.filter(|value| !value.is_empty());
    let popup = table
        .popup
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let choices = usize::from(run.as_ref().is_some_and(|v| !v.is_empty()))
        + usize::from(send.is_some())
        + usize::from(popup.is_some());
    let action = match choices {
        1 if run.as_ref().is_some_and(|v| !v.is_empty()) => UserCommandAction::Run(run.unwrap()),
        1 if send.is_some() => UserCommandAction::Send(send.unwrap()),
        1 => UserCommandAction::Popup(popup.unwrap()),
        0 => {
            warnings.push(format!(
                "{context} needs a `run`, `send`, or `popup` value; skipped"
            ));
            return None;
        }
        _ => {
            warnings.push(format!(
                "{context} has conflicting `run`, `send`, or `popup` values; skipped"
            ));
            return None;
        }
    };
    Some(action)
}

fn parse_modifier(value: &str) -> Option<WmModifier> {
    match value.trim().to_ascii_lowercase().as_str() {
        "alt" | "mod" => Some(WmModifier::Alt),
        "super" | "meta" | "logo" | "win" | "windows" => Some(WmModifier::Super),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct KeysOnly {
        keys: HashMap<String, KeyBindingSpec>,
    }

    fn keys(text: &str) -> HashMap<String, KeyBindingSpec> {
        toml::from_str::<KeysOnly>(text)
            .expect("config parses")
            .keys
    }

    fn overrides(
        text: &str,
        input: &InputConfig,
    ) -> (HashMap<String, Vec<KeyBinding>>, Vec<String>) {
        let mut warnings = Vec::new();
        let result = build_key_overrides(keys(text), input, &mut Vec::new(), &mut warnings);
        (result, warnings)
    }

    #[test]
    fn key_overrides_parse_native_bindings_and_warn_on_unknown_action() {
        let (overrides, warnings) = overrides(
            r#"[keys]
            spawn = ["alt-enter"]
            close = "ctrl-b q"
            notanaction = "x""#,
            &InputConfig::default(),
        );
        assert_eq!(
            overrides["spawn"],
            vec![KeyBinding::from_str("alt-enter").unwrap()]
        );
        assert_eq!(
            overrides["close"],
            vec![KeyBinding::from_str("ctrl-b q").unwrap()]
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("notanaction"));
    }

    #[test]
    fn user_command_tables_register_run_and_send_actions() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys("[keys]\n\"ctrl-a g\" = { run = \"lazygit\" }\nalt-g = { send = \"echo hi\\n\" }"),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().any(|command| {
            command.action == UserCommandAction::Run("lazygit".into())
                && command.binding == KeyBinding::from_str("ctrl-a g").unwrap()
        }));
        assert!(commands.iter().any(|command| {
            command.action == UserCommandAction::Send("echo hi\n".into())
                && command.binding == KeyBinding::from_str("alt-g").unwrap()
        }));
    }

    #[test]
    fn invalid_user_command_tables_warn_and_are_skipped() {
        for table in ["{}", "{ run = \"lazygit\", send = \"hi\" }"] {
            let mut commands = Vec::new();
            let mut warnings = Vec::new();
            build_key_overrides(
                keys(&format!("[keys]\n\"ctrl-a g\" = {table}")),
                &InputConfig::default(),
                &mut commands,
                &mut warnings,
            );
            assert!(commands.is_empty());
            assert_eq!(warnings.len(), 1, "{warnings:?}");
        }
    }

    #[test]
    fn bare_key_override_expands_through_the_input_scheme() {
        let (overrides, warnings) = overrides(
            "[keys]\ncopy-mode = \"b\"\nclose = \"shift-w\"",
            &InputConfig::default(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides["copy-mode"],
            ["ctrl-a b", "alt-b"].map(|v| KeyBinding::from_str(v).unwrap())
        );
        assert_eq!(
            overrides["close"],
            ["ctrl-a shift-w", "alt-shift-w"].map(|v| KeyBinding::from_str(v).unwrap())
        );
    }

    #[test]
    fn bare_key_override_follows_custom_scheme_and_mirror_toggle() {
        let mut input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };
        let text = "[keys]\ncopy-mode = \"b\"";
        let (result, _) = overrides(text, &input);
        assert_eq!(
            result["copy-mode"],
            ["ctrl-b b", "super-b"].map(|v| KeyBinding::from_str(v).unwrap())
        );
        input.modifier_shortcuts = false;
        let (overrides, warnings) = overrides(text, &input);
        assert_eq!(
            overrides["copy-mode"],
            vec![KeyBinding::from_str("ctrl-b b").unwrap()]
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn bare_and_literal_bindings_mix_in_one_override() {
        let (result, warnings) = overrides(
            "[keys]\nspawn = [\"n\", \"super-enter\"]\nfocus-left = \"ctrl-h\"",
            &InputConfig::default(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            result["spawn"],
            ["ctrl-a n", "alt-n", "super-enter"].map(|v| KeyBinding::from_str(v).unwrap())
        );
        assert_eq!(
            result["focus-left"],
            vec![KeyBinding::from_str("ctrl-h").unwrap()]
        );
    }

    #[test]
    fn additive_override_keeps_defaults_and_deduplicates() {
        let input = InputConfig::default();
        let defaults = crate::commands::default_shortcuts_for_action(&input, "spawn").unwrap();
        let (overrides, warnings) = overrides(
            "[keys]\nspawn = { add = [\"n\", \"super-enter\", \"enter\"] }",
            &input,
        );
        let bindings = &overrides["spawn"];
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(bindings.starts_with(&defaults));
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| **binding == KeyBinding::from_str("ctrl-a enter").unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn scheme_marker_expands_modified_keys_and_rejects_chords() {
        let input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };
        let (result, warnings) = overrides(
            "[keys]\ncopy-mode = \"scheme:ctrl-t\"\nspawn = { add = [\"scheme:ctrl-n\"] }",
            &input,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            result["copy-mode"],
            ["ctrl-b ctrl-t", "super-ctrl-t"].map(|v| KeyBinding::from_str(v).unwrap())
        );

        let (result, warnings) = overrides(
            "[keys]\ncopy-mode = \"scheme:ctrl-t x\"",
            &InputConfig::default(),
        );
        assert!(!result.contains_key("copy-mode"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("one key step"))
        );
    }

    #[test]
    fn empty_specs_unbind_but_malformed_specs_keep_defaults() {
        let (result, warnings) = overrides(
            "[keys]\nscratchpad = []\nspawn = \"\"",
            &InputConfig::default(),
        );
        assert_eq!(result["scratchpad"], Vec::<KeyBinding>::new());
        assert_eq!(result["spawn"], Vec::<KeyBinding>::new());
        assert!(warnings.is_empty(), "{warnings:?}");

        let (overrides, warnings) = overrides(
            "[keys]\nscratchpad = \"not-a-real-key\"",
            &InputConfig::default(),
        );
        assert!(!overrides.contains_key("scratchpad"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn actions_without_defaults_are_bindable() {
        let (overrides, warnings) = overrides(
            "[keys]\ntoggle-pane-synchronization = \"ctrl-a y\"\nrename-session = \"alt-y\"",
            &InputConfig::default(),
        );
        assert_eq!(
            overrides["toggle-pane-synchronization"],
            vec![KeyBinding::from_str("ctrl-a y").unwrap()]
        );
        assert_eq!(
            overrides["rename-session"],
            vec![KeyBinding::from_str("alt-y").unwrap()]
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
