//! Typed settings: what a setting's value may be, how it is presented, and how a value read from
//! TOML is checked against it.
//!
//! One model serves every setting a picker can edit, whoever declares it. A built-in sidebar tab
//! declares its settings in Rust; an extension declares its own in its manifest's `[settings]`.
//! Both are checked, displayed, and written back through this module, so a setting behaves the
//! same in `config.toml`, in the tab settings picker, and in `rozi extensions check`.

use serde::Serialize;

/// One setting's value.
///
/// Deliberately narrower than TOML: strings, integers, booleans, and string lists cover what a
/// setting needs to say, and every one of them compares exactly. Floats are rejected rather than
/// supported so a setting can never be a value two runs disagree about, and nested tables are
/// rejected so a set of values stays a flat map an author can read out of one environment variable.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    String(String),
    List(Vec<String>),
}

impl SettingValue {
    /// What this value is, for a type-mismatch message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::String(_) => "string",
            Self::List(_) => "string list",
        }
    }

    pub fn parse(value: &toml::Value) -> Option<Self> {
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

    /// The value as a TOML expression, safe to write after `key = `.
    pub fn to_toml(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::String(value) => toml::Value::String(value.clone()).to_string(),
            Self::List(values) => {
                let values = values
                    .iter()
                    .map(|value| toml::Value::String(value.clone()).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

/// What every carriable value is, for a message about one that is not.
pub const SETTING_VALUE_KINDS: &str = "a string, integer, boolean, or list of strings";

/// One value a choice setting may take, and the words a picker shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingChoice {
    pub value: String,
    pub label: String,
}

impl SettingChoice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// How a setting is edited. The kind always agrees with the type of the setting's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingKind {
    /// A boolean, shown as `Enabled` or `Disabled`.
    Toggle,
    /// A whole number, optionally bounded. A value outside the bounds is clamped.
    Integer { min: Option<i64>, max: Option<i64> },
    /// Free text.
    Text,
    /// One string out of a fixed set.
    Choice(Vec<SettingChoice>),
    /// Any subset of a fixed set of strings, stored as a list. A picker toggles one member at a
    /// time.
    Flags(Vec<SettingChoice>),
    /// A list of strings. Carried and displayed, but edited only in `config.toml`.
    List,
}

/// A checked value, and whether it had to be moved into range to get there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    Value(SettingValue),
    Clamped { value: SettingValue, requested: i64 },
}

/// Why a value cannot stand for a setting. The caller names the setting and says what happens
/// instead; this only says what was wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// Not a value a setting can hold at all, such as a float or a table.
    Unsupported,
    /// A carriable value of the wrong type.
    WrongKind { found: &'static str },
    /// A string outside a choice setting's set.
    NotAChoice { found: String },
}

/// A setting: its key, how a person reads it, how it is edited, and its value when nobody set one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: SettingKind,
    pub default: SettingValue,
}

impl SettingSpec {
    /// A setting known only by its default, as an extension declares one with a bare value. The
    /// kind follows the value's type and the label is the key.
    pub fn plain(key: impl Into<String>, default: SettingValue) -> Self {
        let key = key.into();
        let kind = match default {
            SettingValue::Bool(_) => SettingKind::Toggle,
            SettingValue::Integer(_) => SettingKind::Integer {
                min: None,
                max: None,
            },
            SettingValue::String(_) => SettingKind::Text,
            SettingValue::List(_) => SettingKind::List,
        };
        Self {
            label: key.clone(),
            key,
            description: None,
            kind,
            default,
        }
    }

    pub fn toggle(key: &str, label: &str, default: bool) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            description: None,
            kind: SettingKind::Toggle,
            default: SettingValue::Bool(default),
        }
    }

    pub fn integer(key: &str, label: &str, default: i64, min: i64, max: i64) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            description: None,
            kind: SettingKind::Integer {
                min: Some(min),
                max: Some(max),
            },
            default: SettingValue::Integer(default),
        }
    }

    pub fn choice(key: &str, label: &str, default: &str, choices: &[(&str, &str)]) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            description: None,
            kind: SettingKind::Choice(
                choices
                    .iter()
                    .map(|(value, label)| SettingChoice::new(*value, *label))
                    .collect(),
            ),
            default: SettingValue::String(default.to_string()),
        }
    }

    /// A subset of `choices`, none by default.
    pub fn flags(key: &str, label: &str, choices: &[(&str, &str)]) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            description: None,
            kind: SettingKind::Flags(
                choices
                    .iter()
                    .map(|(value, label)| SettingChoice::new(*value, *label))
                    .collect(),
            ),
            default: SettingValue::List(Vec::new()),
        }
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// Whether a picker can edit this setting. Lists are edited in `config.toml`.
    pub fn editable(&self) -> bool {
        !matches!(self.kind, SettingKind::List)
    }

    /// Check a value read from TOML against this setting.
    pub fn check(&self, raw: &toml::Value) -> Result<Checked, Rejection> {
        let value = SettingValue::parse(raw).ok_or(Rejection::Unsupported)?;
        self.accept(value)
    }

    /// Check the value a user wrote at `path` in `config.toml`, the one way every settings table
    /// is read: a clamped integer is kept with a warning, and a rejected value is reported and
    /// dropped so the setting keeps its default.
    pub fn check_config_value(
        &self,
        raw: &toml::Value,
        path: &str,
        warnings: &mut Vec<String>,
    ) -> Option<SettingValue> {
        match self.check(raw) {
            Ok(Checked::Value(value)) => Some(value),
            Ok(Checked::Clamped { value, requested }) => {
                warnings.push(format!(
                    "`{path}` {requested} is out of range; clamped to {}",
                    self.display(&value)
                ));
                Some(value)
            }
            Err(rejection) => {
                warnings.push(format!(
                    "`{path}` {}; ignored",
                    self.describe_rejection(&rejection)
                ));
                None
            }
        }
    }

    /// Check a value against this setting: its type must match the default's, a choice must be
    /// one of the set, and an integer is clamped into its bounds.
    pub fn accept(&self, value: SettingValue) -> Result<Checked, Rejection> {
        if value.kind() != self.default.kind() {
            return Err(Rejection::WrongKind {
                found: value.kind(),
            });
        }
        match (&self.kind, value) {
            (SettingKind::Choice(choices), SettingValue::String(found)) => {
                if choices.iter().any(|choice| choice.value == found) {
                    Ok(Checked::Value(SettingValue::String(found)))
                } else {
                    Err(Rejection::NotAChoice { found })
                }
            }
            (SettingKind::Flags(choices), SettingValue::List(found)) => {
                match found
                    .iter()
                    .find(|item| !choices.iter().any(|choice| &choice.value == *item))
                {
                    Some(unknown) => Err(Rejection::NotAChoice {
                        found: unknown.clone(),
                    }),
                    None => Ok(Checked::Value(SettingValue::List(
                        choices
                            .iter()
                            .filter(|choice| found.contains(&choice.value))
                            .map(|choice| choice.value.clone())
                            .collect(),
                    ))),
                }
            }
            (SettingKind::Integer { min, max }, SettingValue::Integer(requested)) => {
                let clamped = requested
                    .max(min.unwrap_or(i64::MIN))
                    .min(max.unwrap_or(i64::MAX));
                if clamped == requested {
                    Ok(Checked::Value(SettingValue::Integer(requested)))
                } else {
                    Ok(Checked::Clamped {
                        value: SettingValue::Integer(clamped),
                        requested,
                    })
                }
            }
            (_, value) => Ok(Checked::Value(value)),
        }
    }

    /// A warning fragment for a rejected value, phrased to follow the setting's name.
    pub fn describe_rejection(&self, rejection: &Rejection) -> String {
        match rejection {
            Rejection::Unsupported => format!("must be {SETTING_VALUE_KINDS}"),
            Rejection::WrongKind { found } => {
                format!("is {found}, but `{}` is {}", self.key, self.default.kind())
            }
            Rejection::NotAChoice { found } => {
                format!(
                    "`{found}` is unknown (expected {})",
                    self.expected_choices()
                )
            }
        }
    }

    /// The choices, quoted and joined, as a message lists them: `` `a`, `b`, or `c` ``.
    pub fn expected_choices(&self) -> String {
        let (SettingKind::Choice(choices) | SettingKind::Flags(choices)) = &self.kind else {
            return String::new();
        };
        let quoted: Vec<_> = choices
            .iter()
            .map(|choice| format!("`{}`", choice.value))
            .collect();
        match quoted.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [first, second] => format!("{first} or {second}"),
            [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
        }
    }

    /// How a picker shows a value of this setting.
    pub fn display(&self, value: &SettingValue) -> String {
        match (&self.kind, value) {
            (_, SettingValue::Bool(true)) => "Enabled".to_string(),
            (_, SettingValue::Bool(false)) => "Disabled".to_string(),
            (SettingKind::Choice(choices), SettingValue::String(value)) => choices
                .iter()
                .find(|choice| &choice.value == value)
                .map_or_else(|| value.clone(), |choice| choice.label.clone()),
            (_, SettingValue::Integer(value)) => value.to_string(),
            (_, SettingValue::String(value)) => value.clone(),
            (SettingKind::Flags(choices), SettingValue::List(values)) => {
                let labels: Vec<_> = choices
                    .iter()
                    .filter(|choice| values.contains(&choice.value))
                    .map(|choice| choice.label.as_str())
                    .collect();
                if labels.is_empty() {
                    "None".to_string()
                } else {
                    labels.join(" + ")
                }
            }
            (_, SettingValue::List(values)) => values.join(", "),
        }
    }

    /// The bounds an integer setting accepts, as a prompt states them.
    pub fn range_hint(&self) -> Option<String> {
        match self.kind {
            SettingKind::Integer {
                min: Some(min),
                max: Some(max),
            } => Some(format!("{min} to {max}")),
            SettingKind::Integer {
                min: Some(min),
                max: None,
            } => Some(format!("{min} or more")),
            SettingKind::Integer {
                min: None,
                max: Some(max),
            } => Some(format!("{max} or less")),
            _ => None,
        }
    }
}

/// Where a group of settings is written in `config.toml`, and so whose they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsOwner {
    /// A built-in sidebar tab's own settings, in `[sidebar.settings.<tab>]`.
    SidebarTab(String),
    /// An extension's settings, in `[extensions.<id>]`. An extension's sidebar tab offers a subset
    /// of them; changing one there changes it for every process of that extension.
    Extension(String),
    /// The `[worktrees]` table, which the CLI and the removal picker read as well as the
    /// Worktrees tab.
    Worktrees,
}

impl SettingsOwner {
    /// The table path the settings live under.
    pub fn table_path(&self) -> Vec<String> {
        match self {
            Self::SidebarTab(tab) => vec!["sidebar".into(), "settings".into(), tab.clone()],
            Self::Extension(id) => vec!["extensions".into(), id.clone()],
            Self::Worktrees => vec!["worktrees".into()],
        }
    }
}

/// A set of settings a picker edits together, at their effective values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsGroup {
    pub owner: SettingsOwner,
    /// In the order a picker lists them.
    pub specs: Vec<SettingSpec>,
    /// The effective value of every setting in `specs`.
    pub values: std::collections::BTreeMap<String, SettingValue>,
    /// Keys the user's `config.toml` sets, which resetting removes.
    pub overridden: std::collections::BTreeSet<String>,
}

impl SettingsGroup {
    pub fn spec(&self, key: &str) -> Option<&SettingSpec> {
        self.specs.iter().find(|spec| spec.key == key)
    }

    pub fn value(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_keep_members_in_the_order_of_their_set_and_refuse_strangers() {
        let spec = SettingSpec::flags("branches", "Branches", &[("a", "A"), ("b", "B")]);
        let list =
            |items: &[&str]| SettingValue::List(items.iter().map(|i| i.to_string()).collect());
        assert_eq!(
            spec.accept(list(&["b", "a"])),
            Ok(Checked::Value(list(&["a", "b"])))
        );
        assert_eq!(
            spec.accept(list(&["a", "c"])),
            Err(Rejection::NotAChoice { found: "c".into() })
        );
        assert_eq!(spec.display(&list(&[])), "None");
        assert_eq!(spec.display(&list(&["b"])), "B");
        assert_eq!(spec.display(&list(&["a", "b"])), "A + B");
    }

    #[test]
    fn values_round_trip_through_toml() {
        for value in [
            SettingValue::Bool(true),
            SettingValue::Integer(-3),
            SettingValue::String("quote\"slash\\newline\n".to_string()),
            SettingValue::List(vec!["a".to_string(), "b\"c".to_string()]),
        ] {
            let parsed: toml::Table = toml::from_str(&format!("key = {}", value.to_toml()))
                .expect("written value is valid TOML");
            assert_eq!(SettingValue::parse(&parsed["key"]), Some(value));
        }
    }

    #[test]
    fn a_value_is_checked_against_the_kind_choices_and_bounds() {
        let choice = SettingSpec::choice("mode", "Mode", "a", &[("a", "A"), ("b", "B")]);
        assert_eq!(
            choice.accept(SettingValue::String("b".into())),
            Ok(Checked::Value(SettingValue::String("b".into())))
        );
        assert_eq!(
            choice.accept(SettingValue::String("c".into())),
            Err(Rejection::NotAChoice { found: "c".into() })
        );
        assert_eq!(
            choice.accept(SettingValue::Bool(true)),
            Err(Rejection::WrongKind { found: "boolean" })
        );
        assert_eq!(choice.expected_choices(), "`a` or `b`");

        let number = SettingSpec::integer("rows", "Rows", 5, 1, 10);
        assert_eq!(
            number.accept(SettingValue::Integer(40)),
            Ok(Checked::Clamped {
                value: SettingValue::Integer(10),
                requested: 40
            })
        );
        assert_eq!(
            number.check(&toml::Value::Float(1.5)),
            Err(Rejection::Unsupported)
        );
    }

    #[test]
    fn display_uses_status_words_and_choice_labels() {
        let toggle = SettingSpec::toggle("on", "On", true);
        assert_eq!(toggle.display(&SettingValue::Bool(true)), "Enabled");
        assert_eq!(toggle.display(&SettingValue::Bool(false)), "Disabled");
        let choice = SettingSpec::choice("mode", "Mode", "a", &[("a", "Alpha")]);
        assert_eq!(choice.display(&SettingValue::String("a".into())), "Alpha");
    }
}
