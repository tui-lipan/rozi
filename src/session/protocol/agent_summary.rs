//! What a host monitor may know about an agent in a session nothing is attached to.
//!
//! The boundary this type draws is the whole point of the monitor layer:
//!
//! ```text
//! host monitor        "what exists here, and does any of it want me?"
//! session attachment  "I am participating in this session."
//! ```
//!
//! So this carries semantics only — who, what state, when it changed. Screen contents, scrollback,
//! cwd, project, layout, and process arguments belong to an attachment, and adding one of them
//! here would turn a supervisor into a second session protocol.
use serde::{Deserialize, Serialize};

/// One agent's state, as reported by the session server that owns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    pub session: String,
    pub pane: crate::state::PaneId,
    /// The pane's spawn generation. Part of the agent's identity, so a respawned pane reads as a
    /// new agent rather than as the old one having changed state.
    pub generation: u64,
    /// Which published row this is, for a pane running several agents. `None` for the one-row-per-
    /// pane case every agent that publishes nothing takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<super::RowKey>,
    /// Agent id: the detected or reported definition (`claude`, `codex`), or for an agent publisher
    /// in a pane with neither, the row's own id.
    pub agent: String,
    /// Display name, already disambiguated per row where a pane publishes several.
    pub label: String,
    /// The effective agent status: `working`, `blocked`, `done`, `idle`, or a publisher's own word.
    pub state: String,
    /// Unix milliseconds when this pane's semantic agent state last changed.
    pub changed_at: u64,
    /// Which launch of the pane's agent this is; see
    /// [`super::PaneRuntimeState::agent_launch`]. Identifying, like [`Self::generation`]: a
    /// restarted agent process is a new agent, and its loading screen is not the old run finishing.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub launch: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl AgentSummary {
    /// Whether two summaries describe the same agent, so a client can tell a state *change* from a
    /// different agent appearing in the same pane. Everything identifying is compared and nothing
    /// that varies with state is, which is what makes this usable as a diff key across polls.
    pub fn same_agent(&self, other: &Self) -> bool {
        self.session == other.session
            && self.pane == other.pane
            && self.generation == other.generation
            && self.launch == other.launch
            && self.row == other.row
            && self.agent == other.agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host monitor matches each agent to its previous summary before reading an edge. A
    /// restarted process matched to its predecessor read `working` -> `idle` as a finished run and
    /// notified for it; matched to nothing, its first summary is a baseline.
    #[test]
    fn a_relaunched_agent_is_a_different_agent() {
        let summary = |launch, state: &str| AgentSummary {
            session: "dev".into(),
            pane: 1,
            generation: 1,
            row: None,
            agent: "pi".into(),
            label: "Pi".into(),
            state: state.into(),
            changed_at: 0,
            launch,
        };
        assert!(summary(1, "working").same_agent(&summary(1, "idle")));
        assert!(!summary(1, "working").same_agent(&summary(2, "idle")));
    }
}
