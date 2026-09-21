use super::*;

impl SessionServer {
    /// What this session's agents are doing, for a client that will never attach.
    ///
    /// This is the host monitor's whole view of a session, so it is deliberately the *semantic*
    /// layer and nothing more: who the agent is, what state it is in, and when that state last
    /// changed. Screen contents, scrollback, cwd, layout, and the pane runtime at large stay
    /// behind a real attachment. A monitor that grew those fields would be a second session
    /// protocol wearing a smaller name.
    ///
    /// One entry per *agent*, not per pane: a pane publishing several rows contributes one summary
    /// each, named the way the Agents tab names them, because an alert has to say which of a
    /// program's runs wants attention. Panes are visited in id order, and a pane's own rows keep
    /// their publish order, so a client comparing consecutive snapshots sees a state change rather
    /// than a reshuffle.
    pub(super) fn agent_summaries(&self) -> Vec<protocol::AgentSummary> {
        let mut summaries = Vec::new();
        for (id, pane) in &self.panes {
            // An exited pane is not an agent that went quiet — it is a pane that is gone, and
            // leaving it here would park its last state beside the session forever.
            if pane.exited.is_some() {
                continue;
            }
            let pane_ref = protocol::PaneRef {
                session_instance: self.instance_id.clone(),
                pane_id: *id,
                generation: pane.generation,
            };
            let runtimes =
                protocol::effective_agent_runtimes(&pane.runtime, &pane.agent.references(pane_ref));
            let multiple = runtimes.len() > 1;
            for (index, runtime) in runtimes.into_iter().enumerate() {
                let label = if multiple {
                    format!("{} #{}", runtime.label, index + 1)
                } else {
                    runtime.label
                };
                summaries.push(protocol::AgentSummary {
                    session: self.session_name.clone(),
                    pane: *id,
                    generation: pane.generation,
                    row: runtime.reference.slot,
                    agent: runtime.identity.id,
                    label,
                    state: runtime.state.as_str().into(),
                    changed_at: pane.agent.summary_changed_at,
                });
            }
        }
        // `panes` is a hash map, so without this the wire order would vary between polls and a
        // client diffing two snapshots would see churn that never happened. Stable, so a pane's
        // rows keep the order their publisher sent them in.
        summaries.sort_by_key(|summary| summary.pane);
        summaries
    }
}
