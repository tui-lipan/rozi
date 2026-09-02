use std::collections::BTreeMap;

use serde::Serialize;

/// One extension setting's value.
///
/// Deliberately narrower than TOML: strings, integers, booleans, and string lists cover what an
/// extension needs to be told, and every one of them compares exactly. Floats are rejected rather
/// than supported so a setting can never be a value two runs disagree about, and nested tables are
/// rejected so the whole set stays a flat map an author can read out of one environment variable.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ExtensionSettingValue {
    Bool(bool),
    Integer(i64),
    String(String),
    List(Vec<String>),
}

impl ExtensionSettingValue {
    /// What this value is, for a type-mismatch message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::String(_) => "string",
            Self::List(_) => "string list",
        }
    }

    fn parse(value: &toml::Value) -> Option<Self> {
        match value {
            toml::Value::Boolean(value) => Some(Self::Bool(*value)),
            toml::Value::Integer(value) => Some(Self::Integer(*value)),
            toml::Value::String(value) => Some(Self::String(value.clone())),
            toml::Value::Array(values) => values
                .iter()
                .map(|value| match value {
                    toml::Value::String(value) => Some(value.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(Self::List),
            _ => None,
        }
    }
}

pub type ExtensionSettings = BTreeMap<String, ExtensionSettingValue>;

const KINDS: &str = "a string, integer, boolean, or list of strings";

/// Read a manifest's `[settings]` table into declared defaults. A value Rozi cannot carry is an
/// error, so an extension never loads with a setting it believes in and Rozi silently dropped.
pub(super) fn declared(
    raw: BTreeMap<String, toml::Value>,
    errors: &mut Vec<String>,
) -> ExtensionSettings {
    let mut declared = ExtensionSettings::new();
    for (key, value) in raw {
        if !super::super::commands::valid_command_segment(&key) {
            errors.push(format!("setting `{key}` must match [a-z0-9_-]+"));
            continue;
        }
        match ExtensionSettingValue::parse(&value) {
            Some(value) => {
                declared.insert(key, value);
            }
            None => errors.push(format!("setting `{key}` must be {KINDS}")),
        }
    }
    declared
}

/// Overlay the user's `[extensions.<id>]` table onto an extension's declared defaults.
///
/// The declaration is the contract: an undeclared key and a key of the wrong type are both the
/// user's mistakes, reported and skipped, and the extension still runs on its own defaults. That
/// keeps a stale line in `config.toml` from breaking an extension after an update removes a
/// setting.
pub(crate) fn merge(
    declared: &ExtensionSettings,
    id: &str,
    user: Option<&toml::Value>,
    warnings: &mut Vec<String>,
) -> ExtensionSettings {
    let mut merged = declared.clone();
    let Some(user) = user else {
        return merged;
    };
    let Some(table) = user.as_table() else {
        warnings.push(format!("`[extensions.{id}]` must be a table; ignored"));
        return merged;
    };
    for (key, value) in table {
        let Some(default) = declared.get(key.as_str()) else {
            warnings.push(format!(
                "`{id}` has no setting `{key}`; ignored (run `rozi extensions check` to see the ones it declares)"
            ));
            continue;
        };
        match ExtensionSettingValue::parse(value) {
            Some(value) if value.kind() == default.kind() => {
                merged.insert(key.clone(), value);
            }
            Some(value) => warnings.push(format!(
                "`extensions.{id}.{key}` is {}, but `{key}` is {}; ignored",
                value.kind(),
                default.kind()
            )),
            None => warnings.push(format!("`extensions.{id}.{key}` must be {KINDS}; ignored")),
        }
    }
    merged
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

    #[test]
    fn declared_settings_accept_carriable_values_and_reject_the_rest() {
        let mut errors = Vec::new();
        let raw = table(
            r#"
            runner = "auto"
            rows = 50
            notify = true
            ignore = ["target", "node_modules"]
            "#,
        );
        let built = declared(
            raw.as_table().unwrap().clone().into_iter().collect(),
            &mut errors,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(built.len(), 4);
        assert_eq!(
            built["runner"],
            ExtensionSettingValue::String("auto".to_string())
        );

        let mut errors = Vec::new();
        let raw = table("ratio = 0.5\nnested = { a = 1 }\nmixed = [1, \"two\"]\n");
        let built = declared(
            raw.as_table().unwrap().clone().into_iter().collect(),
            &mut errors,
        );
        assert!(built.is_empty());
        assert_eq!(errors.len(), 3, "{errors:?}");
    }

    #[test]
    fn user_values_override_by_key_and_are_checked_against_the_declared_type() {
        let mut errors = Vec::new();
        let built = declared(
            table("runner = \"auto\"\nrows = 50\n")
                .as_table()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
            &mut errors,
        );
        let mut warnings = Vec::new();
        let merged = merge(
            &built,
            "tasks",
            Some(&table("runner = \"just\"\nrows = \"lots\"\ntypo = 1\n")),
            &mut warnings,
        );
        assert_eq!(
            merged["runner"],
            ExtensionSettingValue::String("just".to_string())
        );
        // A wrong type and an unknown key both fall back to the declaration rather than failing.
        assert_eq!(merged["rows"], ExtensionSettingValue::Integer(50));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|warning| warning.contains("is string")));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("no setting"))
        );
    }

    #[test]
    fn the_env_value_is_compact_json_with_real_types() {
        let mut errors = Vec::new();
        let built = declared(
            table("runner = \"just\"\nrows = 50\nnotify = true\nignore = [\"target\"]\n")
                .as_table()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
            &mut errors,
        );
        assert_eq!(
            env_value(&built),
            r#"{"ignore":["target"],"notify":true,"rows":50,"runner":"just"}"#
        );
    }
}
