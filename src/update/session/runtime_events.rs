//! `agent-state-changed` and `foreground-changed`: what a pane's runtime update changed about its
//! agents and the program it runs, told to subscribers and hooks.
//!
//! Both are read off what the rest of rozi answers from - an agent's state as `rozi agents list`
//! reports it, and the running program as profile capture and resurrection read it - so an event
//! never disagrees with what a subscriber reads back when it asks.

use crate::events::{Event, EventKind};
use crate::session::protocol::{AgentRuntime, AgentState};
use crate::state::{Pane, PaneId, State};

/// What a pane looked like before a runtime update, for the events that compare against it.
pub(crate) struct RuntimeBaseline {
    /// `None` when nobody listens for `agent-state-changed`.
    agents: Option<Vec<AgentRuntime>>,
    /// `None` when nobody listens for `foreground-changed`.
    foreground: Option<Foreground>,
}

/// The program a pane runs in its foreground, other than its shell, and its process group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Foreground {
    program: Option<String>,
    pid: Option<u32>,
}

impl Foreground {
    fn of(pane: &Pane) -> Self {
        let terminal = &pane.terminal;
        let program = crate::pane::launch::running_program(
            terminal.command_phase,
            terminal.foreground_program.as_deref(),
            terminal.foreground_shell,
        );
        Self {
            program: program.map(str::to_string),
            // The group a shell leads is the shell sitting at its prompt.
            pid: program.and(terminal.foreground_pid),
        }
    }
}

impl RuntimeBaseline {
    pub(crate) fn of(state: &State, pane: &Pane) -> Self {
        Self {
            agents: crate::events::wanted(state, EventKind::AgentStateChanged)
                .then(|| pane.agent_runtimes()),
            foreground: crate::events::wanted(state, EventKind::ForegroundChanged)
                .then(|| Foreground::of(pane)),
        }
    }

    /// Emit an event for each agent whose state changed and for a changed foreground program.
    /// `local` says which namespace `pane` is in: a scratch pane can share its id with a session
    /// pane, and only the one actually focused is.
    pub(crate) fn emit_changes(self, state: &State, pane: &Pane, local: bool) {
        if !local {
            return;
        }
        let focused = (state.focused_pane_target() == Some((pane.id, local))).to_string();
        if let Some(before) = self.agents {
            for event in agent_events(pane.id, &before, &pane.agent_runtimes(), &focused) {
                crate::events::emit(state, event);
            }
        }
        if let Some(before) = self.foreground {
            let after = Foreground::of(pane);
            if after != before {
                crate::events::emit(state, foreground_event(pane.id, &before, &after, focused));
            }
        }
    }
}

/// One event per agent whose state differs between `before` and `after`. An agent is matched by
/// its reference, which names one incarnation of an agent across all the runs it finishes. A
/// replacement - the agent relaunched in the same pane - is a new incarnation: the old one goes
/// (`state` empty) and the new one appears (`previous_state` empty).
fn agent_events(
    pane: PaneId,
    before: &[AgentRuntime],
    after: &[AgentRuntime],
    focused: &str,
) -> Vec<Event> {
    let earlier = |runtime: &AgentRuntime| {
        before
            .iter()
            .find(|candidate| candidate.reference == runtime.reference)
    };
    let changed = after.iter().filter_map(|runtime| {
        let previous = earlier(runtime).map(|earlier| earlier.state);
        (previous != Some(runtime.state))
            .then(|| agent_event(pane, runtime, Some(runtime.state), previous, focused))
    });
    let gone = before
        .iter()
        .filter(|runtime| {
            !after
                .iter()
                .any(|candidate| candidate.reference == runtime.reference)
        })
        .map(|runtime| agent_event(pane, runtime, None, Some(runtime.state), focused));
    changed.chain(gone).collect()
}

fn agent_event(
    pane: PaneId,
    runtime: &AgentRuntime,
    state: Option<AgentState>,
    previous: Option<AgentState>,
    focused: &str,
) -> Event {
    let name = |state: Option<AgentState>| state.map(AgentState::as_str).unwrap_or_default();
    Event::new(
        EventKind::AgentStateChanged,
        vec![
            ("pane", pane.to_string()),
            ("agent", runtime.identity.id.clone()),
            ("label", runtime.label.clone()),
            (
                "ref",
                serde_json::to_string(&runtime.reference).expect("agent references serialize"),
            ),
            ("state", name(state).to_string()),
            ("previous_state", name(previous).to_string()),
            (
                "reason",
                state.and(runtime.reason.clone()).unwrap_or_default(),
            ),
            ("focused", focused.to_string()),
        ],
    )
}

fn foreground_event(
    pane: PaneId,
    before: &Foreground,
    after: &Foreground,
    focused: String,
) -> Event {
    Event::new(
        EventKind::ForegroundChanged,
        vec![
            ("pane", pane.to_string()),
            ("program", after.program.clone().unwrap_or_default()),
            (
                "previous_program",
                before.program.clone().unwrap_or_default(),
            ),
            (
                "pid",
                after.pid.map(|pid| pid.to_string()).unwrap_or_default(),
            ),
            ("focused", focused),
        ],
    )
}
