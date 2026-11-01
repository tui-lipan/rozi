//! `agent-state-changed` and `foreground-changed` for the panes this client owns: its scratchpad and
//! popup, which the session server records no events for.

use crate::events::{Event, EventKind};
use crate::session::protocol::{AgentRuntime, EventFields, PaneForeground, agent_state_changes};
use crate::state::{Pane, State};

/// What a pane looked like before a runtime update, for the events that compare against it.
pub(crate) struct RuntimeBaseline {
    /// `None` when nobody listens for `agent-state-changed`.
    agents: Option<Vec<AgentRuntime>>,
    /// `None` when nobody listens for `foreground-changed`.
    foreground: Option<PaneForeground>,
}

fn foreground_of(pane: &Pane) -> PaneForeground {
    let terminal = &pane.terminal;
    PaneForeground::running(
        terminal.command_phase,
        terminal.foreground_program.as_deref(),
        terminal.foreground_shell,
        terminal.foreground_pid,
    )
}

impl RuntimeBaseline {
    pub(crate) fn of(state: &State, pane: &Pane) -> Self {
        Self {
            agents: crate::events::wanted(state, EventKind::AgentStateChanged)
                .then(|| pane.agent_runtimes()),
            foreground: crate::events::wanted(state, EventKind::ForegroundChanged)
                .then(|| foreground_of(pane)),
        }
    }

    /// Emit an event for each agent whose state changed and for a changed foreground program.
    /// `local` says which namespace `pane` is in: a shared pane's events come from the server, and
    /// a scratch pane can share its id with a session pane, of which only the one focused counts.
    pub(crate) fn emit_changes(self, state: &State, pane: &Pane, local: bool) {
        if !local {
            return;
        }
        let focused = (state.focused_pane_target() == Some((pane.id, local))).to_string();
        let emit = |kind: EventKind, mut fields: EventFields| {
            fields.push(("focused", focused.clone()));
            crate::events::emit(state, Event::new(kind, fields));
        };
        if let Some(before) = self.agents {
            for fields in agent_state_changes(pane.id, &before, &pane.agent_runtimes()) {
                emit(EventKind::AgentStateChanged, fields);
            }
        }
        if let Some(before) = self.foreground {
            let after = foreground_of(pane);
            if after != before {
                emit(
                    EventKind::ForegroundChanged,
                    after.changed_from(pane.id, &before),
                );
            }
        }
    }
}
