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
            let detected = pane.runtime.detected_agent.as_ref();
            let identity = detected.map(|detected| detected.agent.as_ref());
            // A pane with no detected program but a reported status is an agent that named itself
            // through the status API rather than by being recognized on screen.
            let summary = protocol::AgentSummary {
                session: self.session_name.clone(),
                pane: *id,
                generation: pane.generation,
                row: None,
                agent: identity
                    .map_or("reported", |agent| agent.id.as_str())
                    .into(),
                label: identity
                    .map_or("Agent", |agent| agent.label.as_str())
                    .into(),
                state: String::new(),
                changed_at: pane.agent.summary_changed_at,
            };
            if !pane.runtime.rows.is_empty() {
                for (index, row) in pane.runtime.rows.iter().enumerate() {
                    summaries.push(protocol::AgentSummary {
                        row: Some(row.id.clone()),
                        // The same name the Agents tab gives a published row: the program, plus
                        // its position among that pane's runs. The row's own title is what it is
                        // *doing*, which is activity rather than identity and has no field here.
                        label: format!("{} #{}", summary.label, index.saturating_add(1)),
                        state: row.status.clone(),
                        ..summary.clone()
                    });
                }
            } else if let Some(state) =
                protocol::effective_agent_status(pane.runtime.status.as_ref(), detected)
            {
                summaries.push(protocol::AgentSummary {
                    state: state.into(),
                    ..summary
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
