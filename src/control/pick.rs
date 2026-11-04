//! The `pick` request vocabulary: the rows, tabs and actions a caller declares for a picker.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PickRow {
    #[serde(default)]
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub disabled: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub priority: Option<i32>,
}

/// One tab a `pick` caller declares up front. Rows arrive per tab afterwards.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickTab {
    /// Names the tab in row snapshots, and rides back as `tab` on selections, actions, and tab
    /// switches. Row ids only need to be unique within their tab.
    pub id: String,
    /// Strip text. Defaults to `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One extra key the caller offers alongside select and cancel.
///
/// Actions are what turn a picker from a menu into a working surface: delete the branch under the
/// cursor and push a fresh list, or open a prompt and create one. They are declared up front so the
/// footer can advertise them the way every built-in picker advertises its own chords.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickAction {
    /// Returned to the caller as `action`.
    pub id: String,
    /// Chord that fires it, in the same spelling `[keys]` uses (`ctrl-n`).
    pub key: String,
    /// Footer text, e.g. `new branch`.
    pub label: String,
    /// When set, the key opens a text prompt and the entered text rides back as `input`.
    /// A string is the title. An object may also set placeholder, a seed value, and masking.
    /// Cancelling the prompt returns to the picker without reporting anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PickPromptSpec>,
    /// Whether firing it ends the picker. Default `false`: the caller usually wants to push an
    /// updated row set and keep going.
    #[serde(default)]
    pub close: bool,
    /// Require a second press to fire, the way the session and profile pickers arm a kill or a
    /// delete. The armed row is struck through in the error colour with an `again to <label>` cue,
    /// and moving the highlight disarms it.
    #[serde(default)]
    pub confirm: bool,
}

/// How a pick action describes the stacked text prompt.
///
/// The string form is the title. The object form is additive: every existing `"prompt":"Title"`
/// caller stays valid.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum PickPromptSpec {
    Title(String),
    Fields(PickPromptFields),
}

/// Object form of [`PickPromptSpec`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PickPromptFields {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Seed the prompt with the picker's filter text, the way the session picker's `new` starts
    /// from what was typed. `value` still applies while the filter is empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_query: bool,
    /// Hide typed characters on screen. The submitted `input` is still plaintext on the stream.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub masked: bool,
}

impl PickPromptSpec {
    pub fn title(&self) -> &str {
        match self {
            Self::Title(title) => title,
            Self::Fields(fields) => &fields.title,
        }
    }

    pub fn placeholder(&self) -> &str {
        match self {
            Self::Title(_) => "",
            Self::Fields(fields) => fields.placeholder.as_deref().unwrap_or(""),
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Self::Title(_) => "",
            Self::Fields(fields) => fields.value.as_deref().unwrap_or(""),
        }
    }

    pub fn from_query(&self) -> bool {
        match self {
            Self::Title(_) => false,
            Self::Fields(fields) => fields.from_query,
        }
    }

    pub fn masked(&self) -> bool {
        match self {
            Self::Title(_) => false,
            Self::Fields(fields) => fields.masked,
        }
    }
}
