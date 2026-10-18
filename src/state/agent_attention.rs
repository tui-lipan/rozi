//! Which finished agent runs this client has looked at.
//!
//! The session server counts every run an agent finishes ([`AgentRecord::completions`]); what this
//! client has *seen* of those is the client's own business, and two clients watching one session
//! disagree about it, correctly. This is the one record of it for every agent this client knows
//! about, however it knows: a live pane in the session on screen, a pane in a background
//! attachment, or a summary from a host monitor. All of them name an agent by its exact
//! [`AgentRef`], so looking at an agent anywhere acknowledges it everywhere.
//!
//! An agent whose latest finished run has not been seen is "done, not yet looked at": the filled
//! dot in the sidebar, the finished pane border, and the workspace marker. Generic activities
//! (`rozi status`, activity rows) have no agent identity and keep their own per-pane flags.

use std::collections::HashMap;

use crate::session::protocol::{AgentRecord, AgentRef, AgentRuntime, AgentState, AgentSummary};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentAttention {
    /// The completion count each agent had when it was last looked at.
    seen: HashMap<AgentRef, u64>,
}

/// How a newly observed agent's history is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstSighting {
    /// Part of the first picture of a source - attaching to a session, a monitor's first snapshot of
    /// a host. Whatever finished before this client was watching is not news.
    Baseline,
    /// Appeared while this client was already watching the source, so every run it has finished
    /// happened since.
    New,
}

impl AgentAttention {
    /// Record an agent's completion count the first time it is seen. An agent already known keeps
    /// what has been seen of it, whatever the source reporting it now.
    pub fn observe(&mut self, reference: &AgentRef, completions: u64, sighting: FirstSighting) {
        self.seen
            .entry(reference.clone())
            .or_insert(match sighting {
                FirstSighting::Baseline => completions,
                FirstSighting::New => 0,
            });
    }

    pub fn observe_records(&mut self, records: &[AgentRecord], sighting: FirstSighting) {
        for record in records {
            self.observe(&record.reference, record.completions, sighting);
        }
    }

    pub fn observe_summaries(&mut self, summaries: &[AgentSummary], sighting: FirstSighting) {
        for summary in summaries {
            self.observe(&summary.reference, summary.completions, sighting);
        }
    }

    /// Whether this agent finished a run that has not been looked at. Only a quiet agent can be:
    /// one that is working or waiting again shows that instead, and its next finish is news anew.
    pub fn finished_unseen(
        &self,
        reference: &AgentRef,
        completions: u64,
        state: AgentState,
    ) -> bool {
        state.is_quiescent()
            && self
                .seen
                .get(reference)
                .is_some_and(|seen| completions > *seen)
    }

    pub fn runtime_finished_unseen(&self, runtime: &AgentRuntime) -> bool {
        self.finished_unseen(&runtime.reference, runtime.completions, runtime.state)
    }

    pub fn summary_finished_unseen(&self, summary: &AgentSummary) -> bool {
        self.finished_unseen(&summary.reference, summary.completions, summary.state)
    }

    /// The user looked at this agent: every run it has finished so far is seen. Returns whether
    /// that changed anything.
    pub fn acknowledge(&mut self, reference: &AgentRef, completions: u64) -> bool {
        let seen = self.seen.entry(reference.clone()).or_insert(completions);
        let changed = *seen < completions;
        *seen = (*seen).max(completions);
        changed
    }

    /// Forget agents no source reports any more. A reference names one incarnation, which never
    /// comes back once gone, so nothing worth keeping is lost.
    pub fn retain(&mut self, mut live: impl FnMut(&AgentRef) -> bool) {
        self.seen.retain(|reference, _| live(reference));
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.seen.len()
    }
}

impl crate::state::State {
    /// Forget attention for agents nothing reports any more: no live pane in any attachment, and no
    /// monitor snapshot.
    pub(crate) fn prune_agent_attention(&mut self) {
        let mut live: std::collections::HashSet<AgentRef> = std::collections::HashSet::new();
        let attachments = std::iter::once(self.current()).chain(self.background.values());
        for attachment in attachments {
            for pane in attachment
                .workspaces
                .iter()
                .flat_map(|workspace| &workspace.panes)
            {
                live.extend(pane.agents.iter().map(|record| record.reference.clone()));
            }
        }
        for pane in self.popup.iter().chain(&self.scratch.panes) {
            live.extend(pane.agents.iter().map(|record| record.reference.clone()));
        }
        let summaries = self
            .local_agent_snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.agents)
            .chain(self.remote.agents.values().flatten());
        live.extend(summaries.map(|summary| summary.reference.clone()));
        self.agent_attention
            .retain(|reference| live.contains(reference));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(incarnation: u64) -> AgentRef {
        AgentRef {
            incarnation,
            ..AgentRef::for_test(3, 1, None)
        }
    }

    /// Connecting to a host that finished six jobs an hour ago is not six fresh finishes; a run
    /// that ends while this client is watching is one.
    #[test]
    fn a_baseline_hides_old_finishes_and_a_new_finish_shows() {
        let mut attention = AgentAttention::default();
        attention.observe(&reference(1), 6, FirstSighting::Baseline);
        assert!(!attention.finished_unseen(&reference(1), 6, AgentState::Idle));
        assert!(attention.finished_unseen(&reference(1), 7, AgentState::Idle));
        assert!(
            !attention.finished_unseen(&reference(1), 7, AgentState::Working),
            "an agent working again shows that, not its last finish"
        );
        assert!(attention.acknowledge(&reference(1), 7));
        assert!(!attention.finished_unseen(&reference(1), 7, AgentState::Done));
        assert!(!attention.acknowledge(&reference(1), 7));
    }

    /// An agent that appears after a source was first seen started while this client watched, so
    /// a run it has already finished by the time it is first reported is still news.
    #[test]
    fn an_agent_that_appears_later_keeps_its_finishes() {
        let mut attention = AgentAttention::default();
        attention.observe(&reference(2), 1, FirstSighting::New);
        assert!(attention.finished_unseen(&reference(2), 1, AgentState::Done));
        // A second source reporting the same agent does not reset what has been seen.
        attention.observe(&reference(2), 1, FirstSighting::Baseline);
        assert!(attention.finished_unseen(&reference(2), 1, AgentState::Done));
        attention.retain(|candidate| *candidate != reference(2));
        assert_eq!(attention.tracked(), 0);
    }

    /// One agent, two sources: its live pane and a monitor's summary of the same session. Looking
    /// at the pane acknowledges the finish both of them report, and pruning keeps an agent while
    /// any source still reports it.
    #[test]
    fn looking_at_a_pane_acknowledges_the_agent_everywhere() {
        let mut state = crate::state::State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        );
        let pane_id = state.current().workspaces[0].panes[0].id;
        let reference = AgentRef::for_test(pane_id, 1, None);
        let summary = AgentSummary {
            session: "dev".into(),
            reference: reference.clone(),
            agent: "claude".into(),
            label: "Claude Code".into(),
            state: AgentState::Idle,
            reason: None,
            task: None,
            completions: 1,
            changed_at: 0,
        };
        {
            let pane = &mut state.current_mut().workspaces[0].panes[0];
            pane.terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("claude", "Claude Code").into(),
                state: crate::session::protocol::DetectedAgentState::Idle,
            });
            pane.agents = vec![AgentRecord {
                reference: reference.clone(),
                completions: 1,
            }];
        }
        state
            .agent_attention
            .observe(&reference, 0, FirstSighting::Baseline);
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            sessions: Vec::new(),
            agents: vec![summary.clone()],
            projects: Vec::new(),
        });
        assert!(state.agent_attention.summary_finished_unseen(&summary));

        state.window_focused = true;
        state.current_mut().focused_pane = Some(pane_id);
        assert!(crate::ops::focus::acknowledge_pane_if_attended(
            &mut state, pane_id
        ));
        assert!(!state.agent_attention.summary_finished_unseen(&summary));

        state.prune_agent_attention();
        assert_eq!(state.agent_attention.tracked(), 1);
        state.current_mut().workspaces[0].panes[0].agents.clear();
        state.local_agent_snapshot = None;
        state.prune_agent_attention();
        assert_eq!(state.agent_attention.tracked(), 0);
    }
}
