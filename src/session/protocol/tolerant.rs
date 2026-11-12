//! Tolerant decoding for closed vocabularies that cannot grow an `Unknown` variant.
//!
//! Most vocabularies a peer receives end in a `#[serde(other)] Unknown` variant. Two kinds cannot:
//! a type that is also in the control API schema, where a new value would change that public
//! contract, and a type the server computes itself and matches on in many places, where an
//! `Unknown` it never produces would only add dead arms. The session wire fields that carry one of
//! these decode through [`deserialize`] instead: a value this build does not know becomes the
//! type's [`UnknownValue::unknown`] rather than failing the whole message, which would drop the
//! connection.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// What a value this build does not know decodes as. Chosen per type to be neutral: it must not
/// claim more than the peer said.
pub trait UnknownValue: Sized {
    /// `raw` is the unknown value when it was a string.
    fn unknown(raw: Option<&str>) -> Self;
}

/// `#[serde(deserialize_with = "tolerant::deserialize")]`: decode `T`, or fall back to
/// [`UnknownValue::unknown`] for a value this build does not know.
pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + UnknownValue,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(&value).unwrap_or_else(|_| T::unknown(value.as_str())))
}

impl UnknownValue for super::AgentState {
    /// The rule a custom status already follows: a state Rozi has no word for is a run still
    /// going, never a stop it cannot vouch for.
    fn unknown(raw: Option<&str>) -> Self {
        Self::from_status(raw.unwrap_or_default())
    }
}

impl UnknownValue for super::DetectedAgentState {
    /// As for [`super::AgentState`]: an agent on screen in a state Rozi cannot name is still at it.
    fn unknown(_: Option<&str>) -> Self {
        Self::Working
    }
}

impl UnknownValue for super::AgentAuthority {
    /// The weakest claim: what the screen shows.
    fn unknown(_: Option<&str>) -> Self {
        Self::Detected
    }
}

impl UnknownValue for super::PublisherKind {
    /// The kind that claims nothing: an activity is shown, but never counted as an agent.
    fn unknown(_: Option<&str>) -> Self {
        Self::Activity
    }
}

impl UnknownValue for crate::control::AgentResponseSource {
    /// The source that claims least: text read off a terminal.
    fn unknown(_: Option<&str>) -> Self {
        Self::Terminal
    }
}
