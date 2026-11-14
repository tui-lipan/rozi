//! How the session's agents are talking to each other, as the views draw it.
//!
//! The session server sends the requests between its agents without their prompts or answers, so
//! everything here is who asks whom and how far it got, never what was said.

use crate::control::{AgentRequestState, AgentRequestSummary};
use crate::pane::id::PaneId;
use crate::session::protocol::{AgentRef, PaneRef};
use crate::state::{Pane, State};

/// The requests that touch one pane's agents, for the marks in its title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneLinks {
    /// Unanswered requests this pane's agents asked.
    pub(crate) asking: usize,
    /// Unanswered requests this pane's agents were asked.
    pub(crate) answering: usize,
    /// Whether any of them has been typed into its agent, rather than still queued.
    pub(crate) delivered: bool,
}

impl PaneLinks {
    pub(crate) fn is_empty(self) -> bool {
        self.asking == 0 && self.answering == 0
    }
}

/// The session's requests as this client last heard of them.
pub(crate) fn requests(state: &State) -> &[AgentRequestSummary] {
    &state.attachment.agent_requests
}

/// Whether `agent` belongs to `pane`: the pane the server knows it by.
fn in_pane(agent: &AgentRef, pane: &[PaneRef]) -> bool {
    pane.contains(&agent.pane)
}

/// The panes the server knows `pane`'s agents by.
fn server_panes(pane: &Pane) -> Vec<PaneRef> {
    let mut panes: Vec<PaneRef> = pane
        .agent_runtimes()
        .into_iter()
        .map(|runtime| runtime.reference.pane)
        .collect();
    panes.dedup();
    panes
}

pub(crate) fn pane_links(state: &State, pane: &Pane) -> PaneLinks {
    if !requests(state)
        .iter()
        .any(|request| !request.state.is_resolved())
    {
        return PaneLinks::default();
    }
    links(requests(state), &server_panes(pane))
}

/// The unanswered requests among `requests` that the agents in `panes` asked or were asked.
fn links(requests: &[AgentRequestSummary], panes: &[PaneRef]) -> PaneLinks {
    let mut links = PaneLinks::default();
    if panes.is_empty() {
        return links;
    }
    for request in requests
        .iter()
        .filter(|request| !request.state.is_resolved())
    {
        let asked = request
            .from
            .as_ref()
            .is_some_and(|from| in_pane(from, panes));
        let answering = in_pane(&request.to, panes);
        links.asking += usize::from(asked);
        links.answering += usize::from(answering);
        links.delivered |= (asked || answering) && request.state == AgentRequestState::Active;
    }
    links
}

/// Every live pane of the session on screen.
fn panes(state: &State) -> impl Iterator<Item = &Pane> {
    state
        .attachment
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .filter(|pane| pane.id != crate::pane::id::POPUP_PANE_ID && !pane.closing)
}

/// The pane on screen that runs `agent`.
pub(crate) fn pane_of(state: &State, agent: &AgentRef) -> Option<PaneId> {
    panes(state)
        .find(|pane| server_panes(pane).contains(&agent.pane))
        .map(|pane| pane.id)
}

/// What a request calls one side of it: the agent's name and its pane, or [`OUTSIDE`] for a
/// request asked from outside any agent.
pub(crate) fn agent_name(state: &State, agent: Option<&AgentRef>) -> String {
    let Some(agent) = agent else {
        return OUTSIDE.to_string();
    };
    let label = panes(state).find_map(|pane| {
        let runtimes = pane.agent_runtimes();
        runtimes
            .iter()
            .find(|runtime| {
                runtime.reference.pane == agent.pane && runtime.reference.slot == agent.slot
            })
            .or_else(|| {
                runtimes
                    .iter()
                    .find(|runtime| runtime.reference.pane == agent.pane)
            })
            .map(|runtime| runtime.label.clone())
    });
    match label {
        Some(label) => format!("{label} ({})", agent.pane.pane_id),
        None => format!("Pane {}", agent.pane.pane_id),
    }
}

/// Who asked a request from outside any agent: a person or a script at the `rozi` CLI.
pub(crate) const OUTSIDE: &str = "rozi CLI";

/// `requests` in list order: each chain together, newest chain first, and within a chain each
/// request followed by the ones asked while answering it. Pairs each with its depth in the chain.
pub(crate) fn threaded(requests: &[AgentRequestSummary]) -> Vec<(&AgentRequestSummary, usize)> {
    let mut roots: Vec<&AgentRequestSummary> = requests
        .iter()
        .filter(|request| {
            request
                .parent
                .as_ref()
                .is_none_or(|parent| !requests.iter().any(|other| &other.id == parent))
        })
        .collect();
    roots.sort_by_key(|request| std::cmp::Reverse(request.created_at_unix_ms));
    let mut ordered = Vec::new();
    for root in roots {
        push_thread(requests, root, 0, &mut ordered);
    }
    ordered
}

fn push_thread<'a>(
    requests: &'a [AgentRequestSummary],
    request: &'a AgentRequestSummary,
    depth: usize,
    ordered: &mut Vec<(&'a AgentRequestSummary, usize)>,
) {
    ordered.push((request, depth));
    let mut children: Vec<&AgentRequestSummary> = requests
        .iter()
        .filter(|child| child.parent.as_deref() == Some(request.id.as_str()))
        .collect();
    children.sort_by_key(|child| child.created_at_unix_ms);
    for child in children {
        push_thread(requests, child, depth + 1, ordered);
    }
}

/// One arrow in a chain's diagram.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ChainEvent<'a> {
    /// The request was asked.
    Ask(&'a AgentRequestSummary),
    /// The request settled: answered, failed, or cancelled.
    Settle(&'a AgentRequestSummary),
}

/// The arrows of the chain rooted at `root`, in the order they happened: each request asked, and
/// each settled one answered back. An ask comes before a settle at the same moment.
pub(crate) fn chain_events<'a>(
    requests: &'a [AgentRequestSummary],
    root: &str,
) -> Vec<ChainEvent<'a>> {
    let mut events: Vec<(u64, u8, ChainEvent<'a>)> = Vec::new();
    for request in requests.iter().filter(|request| request.root == root) {
        events.push((request.created_at_unix_ms, 0, ChainEvent::Ask(request)));
        if request.state.is_resolved() {
            events.push((request.updated_at_unix_ms, 1, ChainEvent::Settle(request)));
        }
    }
    events.sort_by_key(|(at, order, _)| (*at, *order));
    events.into_iter().map(|(_, _, event)| event).collect()
}

/// How long `ms` milliseconds read in a row: `12s`, `3m`, `2h`.
pub(crate) fn short_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

/// The word for a request's state, as every surface spells it.
pub(crate) fn state_word(request: &AgentRequestSummary) -> &'static str {
    use super::vocabulary::{
        REQUEST_ACTIVE, REQUEST_ANSWERED, REQUEST_CANCELLED, REQUEST_FAILED, REQUEST_LATER,
        REQUEST_QUEUED,
    };
    match request.state {
        AgentRequestState::Queued => REQUEST_QUEUED,
        AgentRequestState::Active if request.reason.is_some() => REQUEST_LATER,
        AgentRequestState::Active => REQUEST_ACTIVE,
        AgentRequestState::Answered => REQUEST_ANSWERED,
        AgentRequestState::Failed => REQUEST_FAILED,
        AgentRequestState::Cancelled => REQUEST_CANCELLED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        id: &str,
        from: Option<u32>,
        to: u32,
        state: AgentRequestState,
    ) -> AgentRequestSummary {
        AgentRequestSummary {
            id: id.into(),
            root: id.into(),
            parent: None,
            from: from.map(|pane| AgentRef::for_test(pane, 1, None)),
            to: AgentRef::for_test(to, 1, None),
            state,
            reason: None,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        }
    }

    #[test]
    fn a_pane_counts_only_unanswered_requests_on_each_side() {
        let requests = [
            request("a", Some(1), 2, AgentRequestState::Active),
            request("b", Some(1), 3, AgentRequestState::Queued),
            request("c", Some(3), 1, AgentRequestState::Queued),
            request("d", Some(1), 2, AgentRequestState::Answered),
        ];
        let links = |pane: u32| links(&requests, &[AgentRef::for_test(pane, 1, None).pane]);
        assert_eq!(
            links(1),
            PaneLinks {
                asking: 2,
                answering: 1,
                delivered: true
            }
        );
        assert_eq!(
            links(3),
            PaneLinks {
                asking: 1,
                answering: 1,
                delivered: false
            }
        );
    }
}
