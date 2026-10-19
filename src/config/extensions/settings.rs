use std::collections::{BTreeMap, BTreeSet};

use crate::config::{
    SETTING_VALUE_KINDS, SettingChoice, SettingKind, SettingSpec, SettingValue, SettingsGroup,
    SettingsOwner,
};

/// An extension's settings at their merged values: the manifest's defaults with the user's
/// overrides applied.
pub type ExtensionSettings = BTreeMap<String, SettingValue>;

/// What an extension's manifest declares about each of its settings.
pub type ExtensionSettingSpecs = BTreeMap<String, SettingSpec>;

/// Keys a setting's table form accepts.
const TABLE_KEYS: &[&str] = &["default", "label", "description", "choices", "min", "max"];

/// Read a manifest's `[settings]` table into declared settings. A value Rozi cannot carry, or a
/// table form that contradicts itself, is an error, so an extension never loads with a setting it
/// believes in and Rozi silently dropped.
///
/// A setting is either its default (`rows = 50`) or a table that also says how a picker shows it
/// (`rows = { default = 50, label = "Rows", min = 1, max = 500 }`).
pub(super) fn declared(
    raw: BTreeMap<String, toml::Value>,
    errors: &mut Vec<String>,
) -> ExtensionSettingSpecs {
    let mut declared = ExtensionSettingSpecs::new();
    for (key, value) in raw {
        if !super::super::commands::valid_command_segment(&key) {
            errors.push(format!("setting `{key}` must match [a-z0-9_-]+"));
            continue;
        }
        let spec = match &value {
            toml::Value::Table(table) => declared_table(&key, table, errors),
            value => match SettingValue::parse(value) {
                Some(default) => Some(SettingSpec::plain(&key, default)),
                None => {
                    errors.push(format!(
                        "setting `{key}` must be {SETTING_VALUE_KINDS}, or a table with `default`"
                    ));
                    None
                }
            },
        };
        if let Some(spec) = spec {
            declared.insert(key, spec);
        }
    }
    declared
}

fn declared_table(key: &str, table: &toml::Table, errors: &mut Vec<String>) -> Option<SettingSpec> {
    let before = errors.len();
    for field in table.keys() {
        if !TABLE_KEYS.contains(&field.as_str()) {
            errors.push(format!("setting `{key}` has unknown field `{field}`"));
        }
    }
    let Some(default) = table.get("default").map(SettingValue::parse) else {
        errors.push(format!("setting `{key}` needs a `default`"));
        return None;
    };
    let Some(default) = default else {
        errors.push(format!(
            "setting `{key}` default must be {SETTING_VALUE_KINDS}"
        ));
        return None;
    };
    let text = |field: &str, errors: &mut Vec<String>| match table.get(field) {
        None => None,
        Some(toml::Value::String(value)) if !value.trim().is_empty() => {
            Some(value.trim().to_string())
        }
        Some(_) => {
            errors.push(format!("setting `{key}` {field} must be a nonempty string"));
            None
        }
    };
    let label = text("label", errors);
    let description = text("description", errors);
    let mut spec = SettingSpec::plain(key, default);
    if let Some(label) = label {
        spec.label = label;
    }
    spec.description = description;

    if let Some(raw) = table.get("choices") {
        match (&spec.default, choices(raw)) {
            (SettingValue::String(default), Some(choices)) if !choices.is_empty() => {
                let mut seen = BTreeSet::new();
                if choices.iter().any(|choice| !seen.insert(choice.value.clone())) {
                    errors.push(format!("setting `{key}` lists a choice twice"));
                }
                if !choices.iter().any(|choice| &choice.value == default) {
                    errors.push(format!(
                        "setting `{key}` default `{default}` is not one of its choices"
                    ));
                }
                spec.kind = SettingKind::Choice(choices);
            }
            (SettingValue::String(_), _) => errors.push(format!(
                "setting `{key}` choices must be a nonempty list of strings or `{{ value, label }}` tables"
            )),
            _ => errors.push(format!(
                "setting `{key}` has choices, which only a string setting can"
            )),
        }
    }

    let bound = |field: &str, errors: &mut Vec<String>| match table.get(field) {
        None => None,
        Some(toml::Value::Integer(value)) => Some(*value),
        Some(_) => {
            errors.push(format!("setting `{key}` {field} must be an integer"));
            None
        }
    };
    let (min, max) = (bound("min", errors), bound("max", errors));
    if min.is_some() || max.is_some() {
        match spec.default {
            SettingValue::Integer(default) => {
                if let (Some(min), Some(max)) = (min, max)
                    && min > max
                {
                    errors.push(format!("setting `{key}` min is above its max"));
                }
                if min.is_some_and(|min| default < min) || max.is_some_and(|max| default > max) {
                    errors.push(format!("setting `{key}` default is outside min and max"));
                }
                spec.kind = SettingKind::Integer { min, max };
            }
            _ => errors.push(format!(
                "setting `{key}` has min or max, which only an integer setting can"
            )),
        }
    }
    (errors.len() == before).then_some(spec)
}

/// A `choices` list: each entry a bare value, labelled by itself, or a `{ value, label }` table.
fn choices(raw: &toml::Value) -> Option<Vec<SettingChoice>> {
    raw.as_array()?
        .iter()
        .map(|choice| match choice {
            toml::Value::String(value) if !value.is_empty() => {
                Some(SettingChoice::new(value.clone(), value.clone()))
            }
            toml::Value::Table(table) => {
                if table.keys().any(|key| key != "value" && key != "label") {
                    return None;
                }
                let value = table.get("value")?.as_str().filter(|v| !v.is_empty())?;
                let label = match table.get("label") {
                    None => value,
                    Some(label) => label.as_str().map(str::trim).filter(|l| !l.is_empty())?,
                };
                Some(SettingChoice::new(value, label))
            }
            _ => None,
        })
        .collect()
}

/// The declared defaults, before any user override.
pub(crate) fn defaults(specs: &ExtensionSettingSpecs) -> ExtensionSettings {
    specs
        .iter()
        .map(|(key, spec)| (key.clone(), spec.default.clone()))
        .collect()
}

/// Overlay the user's `[extensions.<id>]` table onto an extension's declared defaults.
///
/// The declaration is the contract: an undeclared key, a key of the wrong type, and a choice
/// outside the declared set are all the user's mistakes, reported and skipped, and the extension
/// still runs on its own defaults. An integer outside its declared bounds is clamped. That keeps a
/// stale line in `config.toml` from breaking an extension after an update removes a setting.
pub(crate) fn merge(
    declared: &ExtensionSettingSpecs,
    id: &str,
    user: Option<&toml::Value>,
    warnings: &mut Vec<String>,
) -> ExtensionSettings {
    let mut merged = defaults(declared);
    let Some(user) = user else {
        return merged;
    };
    let Some(table) = user.as_table() else {
        warnings.push(format!("`[extensions.{id}]` must be a table; ignored"));
        return merged;
    };
    for (key, value) in table {
        let Some(spec) = declared.get(key.as_str()) else {
            warnings.push(format!(
                "`{id}` has no setting `{key}`; ignored (run `rozi extensions check` to see the ones it declares)"
            ));
            continue;
        };
        match spec.check(value) {
            Ok(crate::config::Checked::Value(value)) => {
                merged.insert(key.clone(), value);
            }
            Ok(crate::config::Checked::Clamped { value, requested }) => {
                warnings.push(format!(
                    "`extensions.{id}.{key}` {requested} is out of range; clamped to {}",
                    spec.display(&value)
                ));
                merged.insert(key.clone(), value);
            }
            Err(rejection) => warnings.push(format!(
                "`extensions.{id}.{key}` {}; ignored",
                spec.describe_rejection(&rejection)
            )),
        }
    }
    merged
}

/// The settings group an extension's sidebar tab offers: the declared settings it names, at their
/// merged values, owned by the extension so a change is written to `[extensions.<id>]`.
pub(crate) fn tab_group(
    declared: &ExtensionSettingSpecs,
    keys: &[String],
    id: &str,
    merged: &ExtensionSettings,
    user: Option<&toml::Value>,
) -> SettingsGroup {
    let user = user.and_then(toml::Value::as_table);
    let specs: Vec<SettingSpec> = keys
        .iter()
        .filter_map(|key| declared.get(key).cloned())
        .collect();
    SettingsGroup {
        owner: SettingsOwner::Extension(id.to_string()),
        values: specs
            .iter()
            .map(|spec| {
                let value = merged
                    .get(&spec.key)
                    .cloned()
                    .unwrap_or_else(|| spec.default.clone());
                (spec.key.clone(), value)
            })
            .collect(),
        overridden: specs
            .iter()
            .filter(|spec| user.is_some_and(|user| user.contains_key(&spec.key)))
            .map(|spec| spec.key.clone())
            .collect(),
        specs,
    }
}

/// The settings as one compact JSON object, the form every extension process reads them in.
///
/// One variable rather than one per setting: a name like `ROZI_EXT_PATH` would collide with the
/// user's own environment, and an author parsing JSON gets types back rather than strings to
/// re-interpret.
pub(super) fn env_value(settings: &ExtensionSettings) -> String {
    serde_json::to_string(settings).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Value {
        toml::from_str(text).expect("settings table parses")
    }

    fn declare(text: &str) -> (ExtensionSettingSpecs, Vec<String>) {
        let mut errors = Vec::new();
        let built = declared(
            table(text)
                .as_table()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
            &mut errors,
        );
        (built, errors)
    }

    #[test]
    fn declared_settings_accept_carriable_values_and_reject_the_rest() {
        let (built, errors) = declare(
            r#"
            runner = "auto"
            rows = 50
            notify = true
            ignore = ["target", "node_modules"]
            "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(built.len(), 4);
        assert_eq!(
            built["runner"].default,
            SettingValue::String("auto".to_string())
        );
        assert_eq!(built["runner"].label, "runner");

        let (built, errors) = declare("ratio = 0.5\nnested = { a = 1 }\nmixed = [1, \"two\"]\n");
        assert!(built.is_empty());
        assert_eq!(errors.len(), 4, "{errors:?}");
    }

    #[test]
    fn the_table_form_declares_how_a_picker_shows_a_setting() {
        let (built, errors) = declare(
            r#"
            group = { default = "status", label = "Group by", description = "How rows nest", choices = ["status", { value = "repo", label = "Repository" }] }
            rows = { default = 50, label = "Rows", min = 1, max = 500 }
            done = { default = false, label = "Show finished" }
            "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(built["group"].label, "Group by");
        assert_eq!(built["group"].description.as_deref(), Some("How rows nest"));
        assert_eq!(
            built["group"].kind,
            SettingKind::Choice(vec![
                SettingChoice::new("status", "status"),
                SettingChoice::new("repo", "Repository"),
            ])
        );
        assert_eq!(
            built["rows"].kind,
            SettingKind::Integer {
                min: Some(1),
                max: Some(500)
            }
        );
        assert_eq!(built["done"].kind, SettingKind::Toggle);
    }

    #[test]
    fn a_contradictory_table_form_invalidates_the_setting() {
        let (built, errors) = declare(
            r#"
            a = { label = "No default" }
            b = { default = "x", choices = ["y"] }
            c = { default = 5, min = 10 }
            d = { default = true, choices = ["x"] }
            e = { default = 1, colour = "red" }
            f = { default = "x", min = 1 }
            g = { default = 1, min = 5, max = 2 }
            "#,
        );
        assert!(built.is_empty(), "{built:?}");
        assert_eq!(errors.len(), 8, "{errors:?}");
    }

    #[test]
    fn user_values_override_by_key_and_are_checked_against_the_declaration() {
        let (built, _) = declare(
            "runner = \"auto\"\nrows = { default = 50, min = 1, max = 100 }\nmode = { default = \"a\", choices = [\"a\", \"b\"] }\n",
        );
        let mut warnings = Vec::new();
        let merged = merge(
            &built,
            "tasks",
            Some(&table(
                "runner = \"just\"\nrows = 400\nmode = \"c\"\ntypo = 1\n",
            )),
            &mut warnings,
        );
        assert_eq!(merged["runner"], SettingValue::String("just".to_string()));
        assert_eq!(merged["rows"], SettingValue::Integer(100));
        assert_eq!(merged["mode"], SettingValue::String("a".to_string()));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().any(|warning| warning.contains("clamped")));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("`c` is unknown"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("no setting"))
        );

        let mut warnings = Vec::new();
        merge(
            &built,
            "tasks",
            Some(&table("rows = \"lots\"\n")),
            &mut warnings,
        );
        assert!(warnings[0].contains("is string"), "{warnings:?}");
    }

    #[test]
    fn a_tab_group_carries_the_named_settings_and_what_the_user_set() {
        let (built, _) = declare("a = true\nb = 3\nc = \"x\"\n");
        let user = table("b = 4\n");
        let mut warnings = Vec::new();
        let merged = merge(&built, "tasks", Some(&user), &mut warnings);
        let group = tab_group(
            &built,
            &["b".to_string(), "a".to_string()],
            "tasks",
            &merged,
            Some(&user),
        );
        assert_eq!(group.owner, SettingsOwner::Extension("tasks".into()));
        let keys: Vec<_> = group.specs.iter().map(|spec| spec.key.as_str()).collect();
        assert_eq!(keys, ["b", "a"], "the tab's order, not the declaration's");
        assert_eq!(group.values["b"], SettingValue::Integer(4));
        assert!(group.overridden.contains("b"));
        assert!(!group.overridden.contains("a"));
    }

    #[test]
    fn the_env_value_is_compact_json_with_real_types() {
        let (built, _) =
            declare("runner = \"just\"\nrows = 50\nnotify = true\nignore = [\"target\"]\n");
        assert_eq!(
            env_value(&defaults(&built)),
            r#"{"ignore":["target"],"notify":true,"rows":50,"runner":"just"}"#
        );
    }
}
