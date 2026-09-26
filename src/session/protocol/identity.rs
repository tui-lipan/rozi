use serde::{Deserialize, Serialize};

use crate::state::PaneId;

/// Opaque identity of one running session-server process.
///
/// Names can be reused and pane generations restart after a server dies. This non-persistent token
/// fences references from one server instance away from every later server with the same name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SessionInstanceId(String);

impl SessionInstanceId {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
        let mut id = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(id, "{byte:02x}").expect("writing to a string cannot fail");
        }
        Self(id)
    }

    /// The instance a pane's [`SESSION_INSTANCE_ENV`] names. Empty means the pane is not a shared
    /// pane of any session: a popup, or a pane of the scratch runtime.
    pub fn from_env_value(value: &str) -> Option<Self> {
        (!value.is_empty()).then(|| Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// Names the session server a shared pane runs in, beside `ROZI_PANE`.
///
/// A bare pane id is only meaningful inside one session's shared namespace, and a UI holds several
/// sessions at once once it parks attachments in the background. The server sets this itself on
/// every spawn, so it is never stale across a rename and never matches a later server that reused
/// the name. A pane outside the shared namespace gets it empty, which also hides a value the
/// server process inherited from wherever it was started.
pub const SESSION_INSTANCE_ENV: &str = "ROZI_SESSION_INSTANCE";

/// Exact identity of one PTY incarnation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct PaneRef {
    pub session_instance: SessionInstanceId,
    pub pane_id: PaneId,
    pub generation: u64,
}

/// Exact identity of one semantic agent or published activity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentRef {
    pub pane: PaneRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    pub incarnation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_session_instances_are_opaque_and_unique() {
        let first = SessionInstanceId::generate();
        let second = SessionInstanceId::generate();
        assert_ne!(first, second);
        assert_eq!(first.0.len(), 32);
        assert!(first.0.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn agent_reference_round_trips_as_structured_json() {
        let reference = AgentRef {
            pane: PaneRef {
                session_instance: SessionInstanceId::for_test("server-a"),
                pane_id: 3,
                generation: 7,
            },
            slot: Some("review".into()),
            incarnation: 11,
        };
        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(serde_json::from_str::<AgentRef>(&json).unwrap(), reference);
    }
}
