//! Immutable provenance of the seed that populated a session.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOrigin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeOrigin>,
}

impl SessionOrigin {
    pub fn is_empty(&self) -> bool {
        self.profile.is_none() && self.worktree.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeOrigin {
    /// Absolute checkout path on the session server's host; remote clients keep it opaque.
    pub path: String,
}
