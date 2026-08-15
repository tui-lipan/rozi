use std::collections::HashMap;
use std::str::FromStr;

use tui_lipan::prelude::KeyBinding;

use super::file::{KeyBindingSpec, UserCommandTableSpec};
use super::schema::{
    InputConfig, UserCommand, UserCommandAction, WhichKey, WmModifier, scheme_shortcuts,
};

pub(super) fn apply_input_config(
    input: &mut InputConfig,
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
    which_key: Option<String>,
    warnings: &mut Vec<String>,
) {
    if let Some(modifier_shortcuts) = modifier_shortcuts {
        input.modifier_shortcuts = modifier_shortcuts;
    }
    if let Some(which_key) = which_key {
        match WhichKey::parse(&which_key) {
            Some(parsed) => input.which_key = parsed,
            None => warnings.push(format!(
                "Unknown which_key `{which_key}`; expected `off`, `instant`, `short`, or `long`"
            )),
        }
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
            bind_user_command(user_commands, &key, table, input, warnings);
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
    input: &InputConfig,
    warnings: &mut Vec<String>,
) {
    let label = table
        .label
        .as_ref()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty());
    let Some(action) = parse_user_command_action(table, &format!("User command `{key}`"), warnings)
    else {
        return;
    };
    // A user command earns the same treatment as a rebound built-in: a bare key step expands
    // through the input scheme, so `i = { run = … }` answers to both the prefix chord and the
    // held modifier. Binding it literally made every user command prefix-only, which contradicts
    // the rule that those two spellings are one keymap.
    let (bindings, hint) = if is_bare_key_step(key) {
        let hint = KeyBinding::from_str(key)
            .map(|binding| crate::keys_display::format_binding(&binding))
            .unwrap_or_else(|_| key.to_string());
        (scheme_shortcuts(input, key), hint)
    } else {
        match KeyBinding::from_str(key) {
            Ok(binding) => {
                let hint = crate::keys_display::format_binding(&binding);
                (vec![binding], hint)
            }
            Err(_) => (Vec::new(), String::new()),
        }
    };
    if bindings.is_empty() {
        warnings.push(format!(
            "Could not parse binding `{key}` for a user command; skipped"
        ));
        return;
    }
    user_commands.push(UserCommand {
        action,
        bindings,
        hint,
        label,
    });
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
    let exec = table
        .exec
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let choices = usize::from(run.as_ref().is_some_and(|v| !v.is_empty()))
        + usize::from(send.is_some())
        + usize::from(popup.is_some())
        + usize::from(exec.is_some());
    // A command pane holds after its command exits unless the config says otherwise; see
    // `UserCommandAction`.
    let keep_open = table.keep_open.unwrap_or(true);
    let action = match choices {
        1 if run.as_ref().is_some_and(|v| !v.is_empty()) => UserCommandAction::Run {
            command: run.unwrap(),
            keep_open,
        },
        1 if send.is_some() => UserCommandAction::Send(send.unwrap()),
        1 if exec.is_some() => UserCommandAction::Exec {
            command: exec.unwrap(),
        },
        1 => UserCommandAction::Popup {
            command: popup.unwrap(),
            keep_open,
        },
        0 => {
            warnings.push(format!(
                "{context} needs a `run`, `send`, `popup`, or `exec` value; skipped"
            ));
            return None;
        }
        _ => {
            warnings.push(format!(
                "{context} has conflicting `run`, `send`, `popup`, or `exec` values; skipped"
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
            command.action == UserCommandAction::run("lazygit")
                && command.bindings == vec![KeyBinding::from_str("ctrl-a g").unwrap()]
        }));
        assert!(commands.iter().any(|command| {
            command.action == UserCommandAction::Send("echo hi\n".into())
                && command.bindings == vec![KeyBinding::from_str("alt-g").unwrap()]
        }));
    }

    /// A bare key step means the same thing for a user command as for a rebound built-in: one
    /// entry, both spellings. Binding it literally used to make every user command prefix-only,
    /// so `alt-i` did nothing and the palette advertised `ctrl+a i` where a built-in shows `i`.
    #[test]
    fn a_bare_user_command_key_binds_both_the_prefix_and_the_modifier() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys("[keys]\ni = { run = \"lazygit\", label = \"Git UI\" }"),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        let command = commands.first().expect("one user command");
        assert!(
            command
                .bindings
                .contains(&KeyBinding::from_str("ctrl-a i").unwrap()),
            "prefix chord missing: {:?}",
            command.bindings
        );
        assert!(
            command
                .bindings
                .contains(&KeyBinding::from_str("alt-i").unwrap()),
            "held-modifier chord missing: {:?}",
            command.bindings
        );
        // The palette shows the bare key, the way a built-in's does.
        assert_eq!(command.hint, "i");
        assert_eq!(command.label(), "Git UI");
    }

    /// `exec` is the fourth verb: no pane, no popup, nothing held open. `keep_open` is accepted
    /// but meaningless there, so it must not silently turn into a pane.
    #[test]
    fn exec_commands_run_without_a_pane() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys("[keys]\nu = { exec = \"rozi run-action toggle-float\", label = \"Float\" }"),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            commands.first().map(|command| command.action.clone()),
            Some(UserCommandAction::Exec {
                command: "rozi run-action toggle-float".to_string(),
            })
        );
    }

    /// Exactly one verb, still. `exec` joins the existing conflict check rather than sitting
    /// outside it, or `{ run = …, exec = … }` would silently pick one.
    #[test]
    fn exec_conflicts_with_the_other_verbs() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys("[keys]\nu = { exec = \"date\", run = \"date\" }"),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(commands.is_empty(), "conflicting table was still bound");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("conflicting")),
            "{warnings:?}"
        );
    }

    /// An explicitly-spelled chord still binds only itself - that is how someone pins a command to
    /// the prefix alone, and it keeps its own spelling as the hint.
    #[test]
    fn an_explicit_user_command_chord_binds_only_itself() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys("[keys]\n\"ctrl-a i\" = { run = \"lazygit\" }"),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        let command = commands.first().expect("one user command");
        assert_eq!(
            command.bindings,
            vec![KeyBinding::from_str("ctrl-a i").unwrap()]
        );
        assert!(
            !command
                .bindings
                .contains(&KeyBinding::from_str("alt-i").unwrap())
        );
        assert_eq!(command.label(), "Run: lazygit");
    }

    /// A `run`/`popup` command pane preserves output after its command exits by default, so a build
    /// that fails in milliseconds remains readable. `send` carries no pane of its own and so has
    /// nothing to hold.
    #[test]
    fn run_and_popup_commands_hold_the_pane_open_unless_opted_out() {
        let mut commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            keys(concat!(
                "[keys]\n",
                "\"ctrl-a b\" = { run = \"cargo build\" }\n",
                "\"ctrl-a l\" = { run = \"lazygit\", keep_open = false }\n",
                "\"ctrl-a d\" = { popup = \"date\" }\n",
                "\"ctrl-a f\" = { popup = \"fzf\", keep_open = false }\n",
            )),
            &InputConfig::default(),
            &mut commands,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        let action = |trigger: &str| {
            let binding = KeyBinding::from_str(trigger).unwrap();
            commands
                .iter()
                .find(|command| command.bindings.contains(&binding))
                .map(|command| command.action.clone())
                .unwrap_or_else(|| panic!("{trigger} is bound"))
        };
        assert_eq!(
            action("ctrl-a b"),
            UserCommandAction::Run {
                command: "cargo build".into(),
                keep_open: true
            }
        );
        assert_eq!(
            action("ctrl-a l"),
            UserCommandAction::Run {
                command: "lazygit".into(),
                keep_open: false
            }
        );
        assert_eq!(
            action("ctrl-a d"),
            UserCommandAction::Popup {
                command: "date".into(),
                keep_open: true
            }
        );
        assert_eq!(
            action("ctrl-a f"),
            UserCommandAction::Popup {
                command: "fzf".into(),
                keep_open: false
            }
        );
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
            ..InputConfig::default()
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
            ..InputConfig::default()
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
