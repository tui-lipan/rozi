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
    /// Which agent this is: its pane, that pane's spawn generation, the published row for a pane
    /// running several agents, and the incarnation of the occupant. Everything identifying is
    /// here, so a respawned pane, a relaunched agent, or a row that left and came back each read as
    /// a different agent rather than as the old one changing state.
    pub reference: super::AgentRef,
    /// Agent id: the detected or reported definition (`claude`, `codex`), or for an agent publisher
    /// in a pane with neither, the row's own id.
    pub agent: String,
    /// The agent's display name (`Claude Code`). Several agents of one pane share it; [`Self::task`]
    /// is what tells them apart.
    pub label: String,
    pub state: super::AgentState,
    /// Why the agent is in its state, as it said (`Permission required`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the agent works on, when it named its work. See [`super::AgentRuntime::task`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Runs this agent has finished. See [`super::AgentRecord::completions`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub completions: u64,
    /// Unix milliseconds when this pane's semantic agent state last changed.
    pub changed_at: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl AgentSummary {
    pub fn pane(&self) -> crate::state::PaneId {
        self.reference.pane.pane_id
    }

    pub fn row(&self) -> Option<&super::RowKey> {
        self.reference.slot.as_ref()
    }

    /// Whether two summaries describe the same agent, so a client can tell a state *change* from a
    /// different agent appearing in the same pane. Everything identifying is compared and nothing
    /// that varies with state is, which is what makes this usable as a diff key across polls.
    pub fn same_agent(&self, other: &Self) -> bool {
        self.session == other.session
            && self.reference == other.reference
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
        let summary = |incarnation, state| AgentSummary {
            session: "dev".into(),
            reference: super::super::AgentRef {
                pane: super::super::PaneRef {
                    session_instance: super::super::SessionInstanceId::for_test("dev"),
                    pane_id: 1,
                    generation: 1,
                },
                slot: None,
                incarnation,
            },
            agent: "pi".into(),
            label: "Pi".into(),
            state,
            reason: None,
            task: None,
            completions: 0,
            changed_at: 0,
        };
        use super::super::AgentState::{Idle, Working};
        assert!(summary(1, Working).same_agent(&summary(1, Idle)));
        assert!(!summary(1, Working).same_agent(&summary(2, Idle)));
    }
}
