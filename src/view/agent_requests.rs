//! How the session's agents are talking to each other, as the views draw it.
//!
//! The session server sends the requests between its agents without their prompts or answers, so
//! everything here is who asks whom and how far it got, never what was said.

use crate::control::{AgentRequestState, AgentRequestSummary};
use crate::pane::id::PaneId;
use crate::session::protocol::{AgentRef, PaneRef};
use crate::state::{Pane, State};
use tui_lipan::TreeBranch;

/// The requests that touch one pane's agents, for the marks in its title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneLinks {
    /// Unanswered requests this pane's agents asked.
    pub(crate) asking: usize,
    /// Unanswered requests this pane's agents were asked.
    pub(crate) answering: usize,
}

impl PaneLinks {
    pub(crate) fn is_empty(self) -> bool {
        self.asking == 0 && self.answering == 0
    }
}

/// Now on the session server's clock, the clock the requests' timestamps are on: its time when it
/// last sent them, advanced by how long ago that was here. `None` before the first list arrives.
pub(crate) fn server_now_ms(state: &State) -> Option<u64> {
    let (server_ms, received) = state.attachment.agent_requests_clock?;
    Some(server_ms.saturating_add(received.elapsed().as_millis() as u64))
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
    }
    links
}

/// Something one agent just sent another: a request typed into its target, or an answer back to
/// its asker. The sender is `None` for a request asked from outside any agent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Handoff<'a> {
    pub(crate) sender: Option<&'a AgentRef>,
    pub(crate) receiver: &'a AgentRef,
}

/// What a change from `old` to `new` hands over: each request just delivered, sent by its asker
/// to its target, and each just answered, sent back by its target to its asker. Failures and
/// cancellations hand nothing over.
pub(crate) fn handoffs<'a>(
    old: &[AgentRequestSummary],
    new: &'a [AgentRequestSummary],
) -> Vec<Handoff<'a>> {
    new.iter()
        .filter_map(|request| {
            let before = old.iter().find(|previous| previous.id == request.id)?.state;
            match (before, request.state) {
                (
                    AgentRequestState::Queued,
                    AgentRequestState::Active | AgentRequestState::Delivered,
                ) => Some(Handoff {
                    sender: request.from.as_ref(),
                    receiver: &request.to,
                }),
                (before, AgentRequestState::Answered) if !before.is_resolved() => Some(Handoff {
                    sender: Some(&request.to),
                    receiver: request.from.as_ref()?,
                }),
                _ => None,
            }
        })
        .collect()
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

/// The PTY generation running in `pane` on screen now.
pub(crate) fn pty_generation(state: &State, pane: PaneId) -> Option<u64> {
    panes(state)
        .find(|candidate| candidate.id == pane)
        .map(|pane| pane.pty_generation)
}

/// What a request calls one side of it: the name the server recorded when it was asked, else the
/// agent's name while that exact agent still runs, with its pane either way; [`OUTSIDE`] for a
/// request asked from outside any agent. Never the name of whatever runs in that pane now: a
/// request from a replaced agent must not read as one from its successor.
pub(crate) fn agent_name(
    state: &State,
    agent: Option<&AgentRef>,
    recorded: Option<&str>,
) -> String {
    let Some(agent) = agent else {
        // An outside asker's own description, marked so it never passes for an agent here.
        return recorded.map_or_else(
            || OUTSIDE.to_string(),
            |label| format!("{label}{OUTSIDE_MARK}"),
        );
    };
    let label = recorded.map(str::to_string).or_else(|| {
        panes(state).find_map(|pane| {
            pane.agent_runtimes()
                .into_iter()
                .find(|runtime| runtime.reference.instance() == agent.instance())
                .map(|runtime| runtime.label)
        })
    });
    match label {
        Some(label) => format!("{label} ({})", agent.pane.pane_id),
        None => format!("Pane {}", agent.pane.pane_id),
    }
}

/// Who asked a request from outside any agent: a person or a script at the `rozi` CLI.
pub(crate) const OUTSIDE: &str = "rozi CLI";

/// After the name an asker outside this session gave itself.
const OUTSIDE_MARK: &str = ", outside";

/// `requests` in list order: each conversation together, the one with the latest exchange first.
/// A conversation reads as one tree: its first exchange on top, then everything after it in the
/// order it was asked, each follow-up with the requests asked while answering it beneath it.
/// Pairs each with where it sits in that tree, for the guides drawn in front of it.
pub(crate) fn threaded(
    requests: &[AgentRequestSummary],
) -> Vec<(&AgentRequestSummary, TreeBranch)> {
    use std::collections::{HashMap, HashSet};

    let ids: HashSet<&str> = requests.iter().map(|request| request.id.as_str()).collect();
    // What each request asked while answering it, and each conversation's exchanges: requests
    // that answer none still listed, in the order they were asked.
    let mut children: HashMap<&str, Vec<&AgentRequestSummary>> = HashMap::new();
    let mut exchanges: HashMap<&str, Vec<&AgentRequestSummary>> = HashMap::new();
    for request in requests {
        match request
            .parent
            .as_deref()
            .filter(|parent| ids.contains(parent))
        {
            Some(parent) => children.entry(parent).or_default().push(request),
            None => exchanges
                .entry(request.conversation())
                .or_default()
                .push(request),
        }
    }
    let mut latest: HashMap<&str, u64> = HashMap::new();
    for request in requests {
        let at = latest.entry(request.conversation()).or_default();
        *at = (*at).max(request.created_at_unix_ms);
    }
    let mut conversations: Vec<Vec<&AgentRequestSummary>> = exchanges.into_values().collect();
    for exchanges in &mut conversations {
        exchanges.sort_by_key(|request| request.created_at_unix_ms);
    }
    // Newest activity first; a tie keeps the order the conversations started in.
    conversations.sort_by(|a, b| {
        let latest =
            |exchanges: &[&AgentRequestSummary]| latest.get(exchanges[0].conversation()).copied();
        latest(b)
            .cmp(&latest(a))
            .then_with(|| a[0].created_at_unix_ms.cmp(&b[0].created_at_unix_ms))
            .then_with(|| a[0].id.cmp(&b[0].id))
    });
    let mut ordered = Vec::with_capacity(requests.len());
    for exchanges in conversations {
        // The first exchange's followers sit beside what it delegated, in the order asked.
        let (first, followers) = exchanges
            .split_first()
            .expect("a conversation has an exchange");
        push_thread(
            first,
            TreeBranch::root(),
            followers,
            &children,
            &mut ordered,
        );
    }
    ordered
}

fn push_thread<'a>(
    request: &'a AgentRequestSummary,
    branch: TreeBranch,
    followers: &[&'a AgentRequestSummary],
    children: &std::collections::HashMap<&str, Vec<&'a AgentRequestSummary>>,
    ordered: &mut Vec<(&'a AgentRequestSummary, TreeBranch)>,
) {
    ordered.push((request, branch.clone()));
    let mut below: Vec<&AgentRequestSummary> = children
        .get(request.id.as_str())
        .map(|asked| asked.to_vec())
        .unwrap_or_default();
    below.extend_from_slice(followers);
    below.sort_by_key(|child| child.created_at_unix_ms);
    let last = below.len().saturating_sub(1);
    for (index, child) in below.into_iter().enumerate() {
        push_thread(child, branch.child(index == last), &[], children, ordered);
    }
}

/// Whether `request` continues a conversation an earlier exchange started, rather than starting
/// it or being asked on behalf of another request.
pub(crate) fn is_follow_up(request: &AgentRequestSummary) -> bool {
    request.parent.is_none() && request.conversation() != request.id
}

/// What an answered request's answer is when the agent did not give it with `agents reply`: the
/// word for its last message or its screen, read when its turn ended.
pub(crate) fn not_a_reply(request: &AgentRequestSummary) -> Option<&'static str> {
    use super::vocabulary::{REQUEST_FROM_TERMINAL, REQUEST_LAST_MESSAGE};
    let source = request.response_source?;
    match source {
        _ if request.state != AgentRequestState::Answered => None,
        crate::control::AgentResponseSource::AgentCli => None,
        source if source.is_screen() => Some(REQUEST_FROM_TERMINAL),
        _ => Some(REQUEST_LAST_MESSAGE),
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

/// The arrows of a conversation, in the order they happened: each request asked, and each
/// settled one answered back. An ask comes before a settle at the same moment.
pub(crate) fn chain_events<'a>(
    requests: &'a [AgentRequestSummary],
    conversation: &str,
) -> Vec<ChainEvent<'a>> {
    let mut events: Vec<(u64, u8, ChainEvent<'a>)> = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.conversation() == conversation)
    {
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
        REQUEST_ACTIVE, REQUEST_ANSWERED, REQUEST_CANCELLED, REQUEST_DELIVERED, REQUEST_FAILED,
        REQUEST_LATER, REQUEST_QUEUED,
    };
    match request.state {
        AgentRequestState::Queued => REQUEST_QUEUED,
        AgentRequestState::Active if request.answering_later() => REQUEST_LATER,
        AgentRequestState::Active => REQUEST_ACTIVE,
        AgentRequestState::Answered => REQUEST_ANSWERED,
        AgentRequestState::Delivered => REQUEST_DELIVERED,
        AgentRequestState::Unknown => "Unknown",
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
            conversation: None,
            response_source: None,
            response_complete: None,
            response_truncated: None,
            from: from.map(|pane| AgentRef::for_test(pane, 1, None)),
            to: AgentRef::for_test(to, 1, None),
            from_label: None,
            to_label: None,
            state,
            reason: None,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            delivered_at_unix_ms: None,
            no_reply: false,
        }
    }

    #[test]
    fn conversations_list_newest_first_with_each_request_before_what_it_asked() {
        let at = |mut request: AgentRequestSummary, created: u64, parent: Option<&str>| {
            request.created_at_unix_ms = created;
            request.updated_at_unix_ms = created + 5;
            request.parent = parent.map(Into::into);
            request.root = parent.map_or(request.id.clone(), |_| "a".into());
            request
        };
        // `f` follows `a` in its conversation, later than everything else, so `a`'s conversation
        // lists first and `f` sits beside what `a` delegated.
        let mut follow_up = at(request("f", None, 1, AgentRequestState::Queued), 25, None);
        follow_up.conversation = Some("a".into());
        let requests = [
            follow_up,
            at(request("a", None, 1, AgentRequestState::Active), 10, None),
            at(
                request("b", Some(1), 2, AgentRequestState::Answered),
                11,
                Some("a"),
            ),
            at(request("z", None, 3, AgentRequestState::Queued), 20, None),
            at(
                request("c", Some(2), 3, AgentRequestState::Active),
                12,
                Some("b"),
            ),
        ];
        let order: Vec<(&str, usize)> = threaded(&requests)
            .into_iter()
            .map(|(request, branch)| (request.id.as_str(), branch.depth()))
            .collect();
        assert_eq!(order, [("a", 0), ("b", 1), ("c", 2), ("f", 1), ("z", 0)]);
        // `b` answers at 16, after `c` was asked at 12: the diagram keeps that order.
        let events: Vec<String> = chain_events(&requests, "a")
            .into_iter()
            .map(|event| match event {
                ChainEvent::Ask(request) => format!("ask {}", request.id),
                ChainEvent::Settle(request) => format!("settle {}", request.id),
            })
            .collect();
        assert_eq!(events, ["ask a", "ask b", "ask c", "settle b", "ask f"]);
    }

    #[test]
    fn a_delivery_goes_asker_to_target_and_an_answer_comes_back() {
        let before = [
            request("a", Some(1), 2, AgentRequestState::Queued),
            request("b", Some(1), 3, AgentRequestState::Active),
            request("c", Some(2), 3, AgentRequestState::Active),
            request("d", None, 3, AgentRequestState::Active),
            request("f", Some(4), 5, AgentRequestState::Queued),
        ];
        let after = [
            request("a", Some(1), 2, AgentRequestState::Active),
            request("b", Some(1), 3, AgentRequestState::Answered),
            request("c", Some(2), 3, AgentRequestState::Failed),
            request("d", None, 3, AgentRequestState::Answered),
            request("e", Some(3), 1, AgentRequestState::Active),
            request("f", Some(4), 5, AgentRequestState::Delivered),
        ];
        let pairs: Vec<(Option<u32>, u32)> = handoffs(&before, &after)
            .into_iter()
            .map(|handoff| {
                (
                    handoff.sender.map(|agent| agent.pane.pane_id),
                    handoff.receiver.pane.pane_id,
                )
            })
            .collect();
        // `a` delivered from 1 to 2; `b` answered by 3 back to 1; `d` answered to the CLI, which
        // has no pane to receive it; `f` is a one-way message, delivered the same way as `a`.
        assert_eq!(pairs, [(Some(1), 2), (Some(3), 1), (Some(4), 5)]);
        assert!(handoffs(&after, &after).is_empty());
    }

    #[test]
    fn ages_run_on_the_servers_clock_not_this_machines() {
        let mut state = State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        );
        assert_eq!(server_now_ms(&state), None);
        // A server whose clock reads 1970 plus five seconds, far behind this machine's.
        state.attachment.agent_requests_clock = Some((5_000, std::time::Instant::now()));
        let now = server_now_ms(&state).unwrap();
        assert!((5_000..6_000).contains(&now), "{now}");
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
            }
        );
        assert_eq!(
            links(3),
            PaneLinks {
                asking: 1,
                answering: 1,
            }
        );
    }
}
