//! Session-owned asynchronous requests from one agent to another.
//!
//! An `ask` is stored and typed into its target once the target is ready to take a prompt; the
//! answer comes from an explicit `agents reply`, from the target's integration, or from its screen
//! when the turn ends. Like responses, requests are ephemeral: never in snapshots, runtime
//! broadcasts, or the session event stream.
use super::headless::ReplyTo;
use super::responses::MAX_RESPONSE_BYTES;
use super::waits::{AgentTurnTracker, WaitKey};
use super::*;
use crate::control::{
    AgentRequestDirection, AgentRequestInfo, AgentRequestList, AgentRequestState,
    AgentRequestSummary, AgentResponse, AgentResponseFidelity, AgentResponsePayload,
    AgentResponseSource, AgentTarget, ControlErrorCode, ControlResponse,
};

const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_PER_ROOT: usize = 64;
const MAX_QUEUED_PER_TARGET: usize = 16;
const MAX_RESOLVED_BYTES: usize = 16 * MAX_RESPONSE_BYTES;
/// Answered requests whose delivery slots prevent ordinary retention eviction.
const MAX_PROTECTED_BYTES: usize = 16 * MAX_RESPONSE_BYTES;
/// With `MAX_LIVE`, keeps the list of every request within one session frame.
const MAX_RESOLVED: usize = 1024;
/// Unresolved requests and answered requests whose delivery turn has not ended.
const MAX_LIVE: usize = 1024;
/// How long a target must have been stopped, with nobody typing into its pane, before a request is
/// typed into it.
const QUIET_WINDOW: Duration = Duration::from_secs(2);
/// How long after delivery a stopped agent that was never seen leaving its stopped state counts
/// as having answered anyway, so a turn too short to observe cannot hold its slot forever.
const UNDETECTED_TURN: Duration = Duration::from_secs(60);
/// How long after delivery an agent that has neither started a turn nor shown the request is
/// checked for having taken it at all. A program still starting, or busy redrawing, can drop
/// what is typed into it without a trace.
const DELIVERY_CHECK: Duration = Duration::from_secs(10);
/// How many times a request is typed into an agent that shows no trace of it.
const MAX_DELIVERY_ATTEMPTS: u8 = 2;
/// The trailing lines of a pane, visible screen and scrollback, searched for a delivered request.
const RECENT_LINES: usize = 500;
/// Why a request fails when its agent never took it.
const NOT_TAKEN: &str =
    "the agent did not take the request: it never appeared on the agent's screen";

pub(super) struct AgentRequestStore {
    /// Oldest first.
    requests: Vec<StoredRequest>,
    /// The request each agent is answering, until its turn ends.
    slots: HashMap<protocol::AgentRef, DeliverySlot>,
    awaits: HashMap<WaitKey, PendingAwait>,
    /// When input was last written to a pane, by a client or a headless command, kept for one
    /// quiet window.
    pane_input: HashMap<PaneId, Instant>,
    /// Since when each target has been seen stopped.
    stopped_since: HashMap<protocol::AgentRef, Instant>,
    pub(super) quiet: Duration,
    undetected_turn: Duration,
    delivery_check: Duration,
    /// Bumped on every change to `requests`, so attached clients are told only when it moves.
    revision: u64,
    /// The revision attached clients were last sent, and what they were sent.
    sent_revision: u64,
    sent: Vec<AgentRequestSummary>,
}

impl Default for AgentRequestStore {
    fn default() -> Self {
        Self {
            requests: Vec::new(),
            slots: HashMap::new(),
            awaits: HashMap::new(),
            pane_input: HashMap::new(),
            stopped_since: HashMap::new(),
            quiet: QUIET_WINDOW,
            undetected_turn: UNDETECTED_TURN,
            delivery_check: DELIVERY_CHECK,
            revision: 0,
            sent_revision: 0,
            sent: Vec::new(),
        }
    }
}

struct StoredRequest {
    info: AgentRequestInfo,
    /// 1 for a request asked outside any other.
    depth: usize,
    /// Its agent will reply after the turn it was delivered in, so that turn's end does not
    /// answer it from the terminal.
    later: bool,
}

/// One side of a conversation: an agent, in the native conversation it was in. A `/clear` or
/// a resumed session keeps the agent but not what it knew, so it starts a new conversation.
#[derive(Clone)]
struct Party<'a> {
    agent: protocol::AgentInstance,
    native_session: Option<&'a str>,
}

impl<'a> Party<'a> {
    fn new(agent: &protocol::AgentRef, native_session: Option<&'a str>) -> Self {
        Self {
            agent: agent.instance(),
            native_session,
        }
    }

    /// Whether this side, as a request recorded it, is `now`. An agent often reports its native
    /// conversation only after its first turn, so one unknown then is not a different one.
    fn is(&self, now: &Party<'_>) -> bool {
        self.agent == now.agent
            && (self.native_session.is_none() || self.native_session == now.native_session)
    }
}

/// How an asker that is not an agent of this session names itself: one line, at most
/// [`MAX_ASKER_LABEL_CHARS`] characters. It comes from outside the session, so it is never trusted
/// to name anything; it is only shown.
fn outside_asker_label(label: Option<String>) -> Option<String> {
    // Only letters, digits, and plain punctuation are kept: anything else, such as a line break,
    // an escape, a bidirectional control, or an invisible tag character, could hide or reorder
    // text, or smuggle instructions into the agent's prompt. A quote cannot close the quotes the
    // request's header puts around the label.
    let label: String = label?
        .chars()
        .map(|c| match c {
            '"' => '\'',
            c if shown_in_asker_label(c) => c,
            _ => ' ',
        })
        .collect();
    let label: String = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_ASKER_LABEL_CHARS)
        .collect();
    (!label.is_empty()).then_some(label)
}

const MAX_ASKER_LABEL_CHARS: usize = 80;

fn shown_in_asker_label(c: char) -> bool {
    c.is_alphanumeric() || " -_.,:;()[]/@#+'&".contains(c)
}

struct DeliverySlot {
    request: String,
    tracker: AgentTurnTracker,
    /// When the request was last typed.
    delivered: Instant,
    /// Whether the agent has been seen leaving its stopped state since delivery.
    departed: bool,
    /// A blocker held the turn open. Wait for work to resume or for a fresh quiet window.
    waiting_after_screen_block: bool,
    /// Whether the request has been seen on the agent's screen, so it is known to have arrived.
    shown: bool,
    /// How many times the request has been typed.
    attempts: u8,
    /// The pane's recent text just before the request was last typed, to tell a delivery that
    /// left no trace at all from a screen that moved on to something else.
    before: u64,
}

/// What became of a delivered request its agent has neither started nor answered.
enum Delivery {
    /// It is on the agent's screen.
    Shown,
    /// Nothing on the pane changed: what was typed was dropped.
    Lost,
    /// The pane changed, but never showed it.
    Missing,
}

struct PendingAwait {
    id: String,
    caller: Option<protocol::AgentRef>,
    deadline: Option<Instant>,
    reply: ReplyTo,
}

/// Whether a queued request can be typed into its target now.
enum Readiness {
    Ready(protocol::AgentState),
    Wait(String),
    Fail(String),
}

/// How a delivered request's turn ended.
enum TurnOutcome {
    Failed(String),
    Answered(AgentResponse),
    /// Ended with nothing to answer with.
    Unanswered,
}

impl AgentRequestStore {
    /// Move every request and slot the handoff's old reference holds to its row's, under the
    /// row's id for the conversation. A request that did not yet know it is pinned to the row's,
    /// so a later conversation in that row cannot answer it.
    pub(super) fn handoff(&mut self, handoff: &super::responses::AgentHandoff) {
        let (old, new) = (&handoff.old, &handoff.new);
        self.revision += 1;
        for stored in &mut self.requests {
            if stored.info.to == *old {
                stored.info.to = new.clone();
                handoff.rename_session(&mut stored.info.to_native_session);
            }
            if stored.info.from.as_ref() == Some(old) {
                stored.info.from = Some(new.clone());
                handoff.rename_session(&mut stored.info.from_native_session);
            }
        }
        for pending in self.awaits.values_mut() {
            if pending.caller.as_ref() == Some(old) {
                pending.caller = Some(new.clone());
            }
        }
        if let Some(mut slot) = self.slots.remove(old) {
            slot.tracker.handoff(handoff);
            self.slots.insert(new.clone(), slot);
        }
        if let Some(since) = self.stopped_since.remove(old) {
            self.stopped_since.insert(new.clone(), since);
        }
    }

    pub(super) fn note_pane_input(&mut self, pane: PaneId) {
        self.pane_input.insert(pane, Instant::now());
    }

    /// The request `agent` is answering, if any.
    pub(super) fn answering(&self, agent: &protocol::AgentRef) -> Option<&str> {
        self.slots.get(agent).map(|slot| slot.request.as_str())
    }

    pub(super) fn remove_awaits(&mut self, client: ClientId) {
        self.awaits.retain(|&(holder, _), _| holder != client);
    }

    pub(super) fn holds_await(&self, client: ClientId) -> bool {
        self.awaits.keys().any(|&(holder, _)| holder == client)
    }

    fn get(&self, id: &str) -> Option<&StoredRequest> {
        self.requests.iter().find(|stored| stored.info.id == id)
    }

    fn info_mut(&mut self, id: &str) -> Option<&mut AgentRequestInfo> {
        self.revision += 1;
        self.requests
            .iter_mut()
            .find(|stored| stored.info.id == id)
            .map(|stored| &mut stored.info)
    }

    fn lookup(&self, id: &str) -> std::result::Result<&AgentRequestInfo, ControlResponse> {
        self.get(id).map(|stored| &stored.info).ok_or_else(|| {
            ControlResponse::error_with(
                ControlErrorCode::StaleReference,
                format!("no request `{id}` in this session"),
            )
        })
    }

    /// The conversation a new request from `from` to `to` joins, or `None` to start one. A
    /// request asked on behalf of another stays in that one's conversation; one that `follows`
    /// an earlier request to the same agent joins its conversation; otherwise an agent asking
    /// joins the latest conversation between the same two agents, either way round, as long as
    /// both are still in the native conversations it was held in. Callers that are not agents
    /// all look alike, so they never join one without `follows`.
    fn conversation(
        &self,
        from: Option<Party<'_>>,
        to: Party<'_>,
        parent: Option<&str>,
        follows: Option<&str>,
    ) -> std::result::Result<Option<String>, ControlResponse> {
        let of = |info: &AgentRequestInfo| info.conversation().to_string();
        if let Some(parent) = parent {
            return Ok(self.get(parent).map(|stored| of(&stored.info)));
        }
        if let Some(id) = follows {
            let earlier = self.lookup(id)?;
            if earlier.to.instance() != to.agent {
                return Err(invalid(format!(
                    "request `{id}` asked another agent; follow a request to this one"
                )));
            }
            // Only the asker continues its own conversation.
            if earlier.from.as_ref().map(protocol::AgentRef::instance)
                != from.as_ref().map(|from| from.agent.clone())
            {
                return Err(invalid(format!(
                    "request `{id}` was asked by someone else; follow one of your own"
                )));
            }
            // Nor can it continue once either side has moved on to another native conversation:
            // the agent no longer knows what was said. Where each side last held the conversation
            // decides, not the request named, which may predate the agent naming its own.
            let conversation = earlier.conversation();
            let still_held = |now: &Party<'_>| {
                self.last_held(conversation, &now.agent)
                    .is_none_or(|held| held.is(now))
            };
            let still_held = still_held(&to) && from.as_ref().is_none_or(still_held);
            if !still_held {
                return Err(invalid(format!(
                    "the conversation of request `{id}` ended when its agent moved to another one; ask without --follows"
                )));
            }
            return Ok(Some(of(earlier)));
        }
        let Some(from) = from else {
            return Ok(None);
        };
        // Only the latest exchange between these two agents decides: an older one, however it
        // matches, was left behind when they moved to the conversation the latest one was in.
        let latest = self
            .requests
            .iter()
            .rev()
            .map(|stored| &stored.info)
            .find_map(|info| {
                // A request asked on behalf of another belongs to that one's conversation, so
                // it says nothing about what these two agents were discussing.
                let (Some(asker), None) = (info.from.as_ref(), info.parent.as_ref()) else {
                    return None;
                };
                let asker = Party::new(asker, info.from_native_session.as_deref());
                let asked = Party::new(&info.to, info.to_native_session.as_deref());
                if asker.agent == from.agent && asked.agent == to.agent {
                    Some((info, asker.is(&from) && asked.is(&to)))
                } else if asker.agent == to.agent && asked.agent == from.agent {
                    Some((info, asker.is(&to) && asked.is(&from)))
                } else {
                    None
                }
            });
        Ok(latest
            .filter(|(_, still_held)| *still_held)
            .map(|(info, _)| of(info)))
    }

    /// How `agent` was recorded in the latest request of `conversation` it took part in, on
    /// either side, including one asked on another's behalf: it held the conversation there too.
    fn last_held(&self, conversation: &str, agent: &protocol::AgentInstance) -> Option<Party<'_>> {
        self.requests
            .iter()
            .rev()
            .map(|stored| &stored.info)
            .filter(|info| info.conversation() == conversation)
            .find_map(|info| {
                if info.to.instance() == *agent {
                    Some(Party::new(&info.to, info.to_native_session.as_deref()))
                } else {
                    info.from
                        .as_ref()
                        .filter(|from| from.instance() == *agent)
                        .map(|from| Party::new(from, info.from_native_session.as_deref()))
                }
            })
    }

    fn has_work(&self) -> bool {
        !self.slots.is_empty()
            || !self.awaits.is_empty()
            || self.requests.iter().any(|stored| {
                stored.info.state == AgentRequestState::Queued
                    || (stored.later && stored.info.state == AgentRequestState::Active)
            })
    }

    /// Where a request asked by `from` sits in the lineage: under `parent` when it names one,
    /// else under the one request `from` owes, whether it is answering it now or promised to
    /// answer it later. Several owed requests make the parent ambiguous, so the ask must name it.
    fn lineage(
        &self,
        from: Option<&protocol::AgentRef>,
        parent: Option<&str>,
    ) -> std::result::Result<Option<(String, String, usize)>, ControlResponse> {
        let Some(from) = from else {
            return match parent {
                Some(_) => Err(invalid("only an agent can ask on behalf of a request")),
                None => Ok(None),
            };
        };
        let owed: Vec<&StoredRequest> = self
            .requests
            .iter()
            .filter(|stored| {
                self.answering(from) == Some(stored.info.id.as_str())
                    || (stored.later
                        && stored.info.state == AgentRequestState::Active
                        && stored.info.to == *from)
            })
            .collect();
        let parent = match (parent, owed.as_slice()) {
            (Some(id), _) => *owed
                .iter()
                .find(|stored| stored.info.id == id)
                .ok_or_else(|| {
                    not_permitted(format!(
                        "request `{id}` is not one this agent is answering or promised to answer"
                    ))
                })?,
            (None, []) => return Ok(None),
            (None, [only]) => only,
            (None, several) => {
                let ids = several
                    .iter()
                    .map(|stored| format!("`{}`", stored.info.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid(format!(
                    "this agent owes requests {ids}; pass --for <ID> to say which one this ask is for"
                )));
            }
        };
        Ok(Some((
            parent.info.id.clone(),
            parent.info.root.clone(),
            parent.depth + 1,
        )))
    }

    fn admit(
        &self,
        to: &protocol::AgentRef,
        lineage: Option<&(String, String, usize)>,
    ) -> std::result::Result<(), ControlResponse> {
        let refuse = |message: String| {
            Err(ControlResponse::error_with(
                ControlErrorCode::Conflict,
                message,
            ))
        };
        let live = self
            .requests
            .iter()
            .filter(|stored| {
                !stored.info.state.is_resolved()
                    || self.answering(&stored.info.to) == Some(stored.info.id.as_str())
            })
            .count();
        if live >= MAX_LIVE {
            return refuse(format!("{live} requests are still live in this session"));
        }
        let queued = self
            .requests
            .iter()
            .filter(|stored| {
                stored.info.state == AgentRequestState::Queued && stored.info.to == *to
            })
            .count();
        if queued >= MAX_QUEUED_PER_TARGET {
            return refuse(format!(
                "{queued} requests are already queued for this agent"
            ));
        }
        let Some((_, root, depth)) = lineage else {
            return Ok(());
        };
        if *depth > MAX_DEPTH {
            return refuse(format!(
                "request chain `{root}` is already {MAX_DEPTH} requests deep"
            ));
        }
        let in_root = self
            .requests
            .iter()
            .filter(|stored| stored.info.root == *root)
            .count();
        if in_root >= MAX_PER_ROOT {
            return refuse(format!(
                "request chain `{root}` already has {MAX_PER_ROOT} requests"
            ));
        }
        Ok(())
    }

    /// Refuse an answer that would exceed the storage protected by unfinished delivery turns.
    fn admit_reply(
        &self,
        request: &AgentRequestInfo,
        text: &str,
    ) -> std::result::Result<(), ControlResponse> {
        if self.answering(&request.to) != Some(request.id.as_str()) {
            return Ok(());
        }
        let protected_bytes: usize = self
            .requests
            .iter()
            .filter(|stored| {
                stored.info.state.is_resolved()
                    && self.answering(&stored.info.to) == Some(stored.info.id.as_str())
            })
            .map(|stored| request_bytes(&stored.info))
            .sum();
        if protected_bytes + request_bytes(request) + text.len() > MAX_PROTECTED_BYTES {
            return Err(conflict(
                "reply would exceed the session's 16 MiB budget for unfinished delivery turns",
            ));
        }
        Ok(())
    }

    /// Whether `request` is `ancestor` or descends from it.
    fn descends_from(&self, request: &AgentRequestInfo, ancestor: &str) -> bool {
        let mut current = Some(request);
        for _ in 0..=MAX_DEPTH {
            let Some(info) = current else {
                return false;
            };
            if info.id == ancestor {
                return true;
            }
            current = info
                .parent
                .as_deref()
                .and_then(|parent| self.get(parent))
                .map(|stored| &stored.info);
        }
        false
    }

    /// A request `caller` has yet to answer, queued for it or already typed into it, that
    /// `awaited` cannot resolve without.
    fn await_cycle(&self, caller: Option<&protocol::AgentRef>, awaited: &str) -> Option<&str> {
        let caller = caller?;
        self.requests
            .iter()
            .map(|stored| &stored.info)
            .find(|info| {
                !info.state.is_resolved() && info.to == *caller && self.descends_from(info, awaited)
            })
            .map(|info| info.id.as_str())
    }

    /// Note whether `agent` is stopped now, and answer since when it has been.
    fn observe(&mut self, agent: &protocol::AgentRef, stopped: bool) -> Option<Instant> {
        if !stopped {
            self.stopped_since.remove(agent);
            return None;
        }
        Some(
            *self
                .stopped_since
                .entry(agent.clone())
                .or_insert_with(Instant::now),
        )
    }

    fn quiet_since(&self, pane: PaneId, stopped_since: Instant) -> bool {
        stopped_since.elapsed() >= self.quiet
            && self
                .pane_input
                .get(&pane)
                .is_none_or(|input| input.elapsed() >= self.quiet)
    }

    /// Forget what no request needs any more, and drop the oldest resolved requests over the cap.
    fn prune(&mut self) {
        let quiet = self.quiet;
        self.pane_input.retain(|_, input| input.elapsed() < quiet);
        let targets: HashSet<protocol::AgentRef> = self
            .requests
            .iter()
            .filter(|stored| stored.info.state == AgentRequestState::Queued)
            .map(|stored| stored.info.to.clone())
            .chain(self.slots.keys().cloned())
            .collect();
        self.stopped_since
            .retain(|agent, _| targets.contains(agent));
        let resolved = || {
            self.requests
                .iter()
                .filter(|stored| stored.info.state.is_resolved())
        };
        let mut resolved_count = resolved().count();
        let mut resolved_bytes: usize = resolved().map(|stored| request_bytes(&stored.info)).sum();
        while resolved_bytes > MAX_RESOLVED_BYTES || resolved_count > MAX_RESOLVED {
            let Some(index) = self.requests.iter().position(|stored| {
                stored.info.state.is_resolved()
                    && self.answering(&stored.info.to) != Some(&stored.info.id)
            }) else {
                break;
            };
            resolved_bytes -= request_bytes(&self.requests.remove(index).info);
            self.revision += 1;
            resolved_count -= 1;
        }
    }
}

/// How many settled requests attached clients are shown besides every live one.
const SHOWN_SETTLED: usize = 32;

impl AgentRequestStore {
    /// What attached clients are shown: every live request and the most recently settled ones,
    /// oldest first, without prompts or answers.
    fn summaries(&self) -> Vec<AgentRequestSummary> {
        let mut settled: Vec<&AgentRequestInfo> = self
            .requests
            .iter()
            .map(|stored| &stored.info)
            .filter(|info| info.state.is_resolved())
            .collect();
        settled.sort_by_key(|info| std::cmp::Reverse(info.updated_at_unix_ms));
        settled.truncate(SHOWN_SETTLED);
        self.requests
            .iter()
            .map(|stored| &stored.info)
            .filter(|info| {
                !info.state.is_resolved() || settled.iter().any(|shown| shown.id == info.id)
            })
            .map(AgentRequestSummary::from)
            .collect()
    }
}

fn request_bytes(info: &AgentRequestInfo) -> usize {
    info.prompt.len()
        + info
            .response
            .as_ref()
            .map_or(0, |response| response.content.text.len())
        + 512
}

fn new_request_id() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("rq_{hex}")
}

fn error_text(response: ControlResponse) -> String {
    response.error.unwrap_or_default()
}

fn not_permitted(message: impl Into<String>) -> ControlResponse {
    ControlResponse::error_with(ControlErrorCode::NotPermitted, message)
}

fn conflict(message: impl Into<String>) -> ControlResponse {
    ControlResponse::error_with(ControlErrorCode::Conflict, message)
}

fn invalid(message: impl Into<String>) -> ControlResponse {
    ControlResponse::error_with(ControlErrorCode::InvalidArgument, message)
}

fn settle(info: &mut AgentRequestInfo, state: AgentRequestState, reason: Option<String>) {
    info.state = state;
    info.reason = reason;
    info.updated_at_unix_ms = crate::runtime_metrics::unix_time_millis();
}

impl SessionServer {
    pub(super) fn ask_agent(
        &mut self,
        from: Option<protocol::AgentRef>,
        target: AgentTarget,
        prompt: String,
        parent: Option<&str>,
        follows: Option<&str>,
        asker_label: Option<String>,
    ) -> ControlResponse {
        if prompt.trim().is_empty() {
            return invalid("a request needs prompt text");
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            return invalid(format!(
                "a request prompt is limited to {} KiB",
                MAX_PROMPT_BYTES / 1024
            ));
        }
        let to = match self.resolve_agent_wait_target(target) {
            Ok(to) => to,
            Err(response) => return response,
        };
        if from.as_ref() == Some(&to) {
            return invalid("an agent cannot ask itself");
        }
        let lineage = match self.agent_requests.lineage(from.as_ref(), parent) {
            Ok(lineage) => lineage,
            Err(response) => return response,
        };
        if lineage.is_some() && follows.is_some() {
            return invalid(
                "this ask belongs to the request you are answering, so it cannot follow another",
            );
        }
        if let Err(response) = self.agent_requests.admit(&to, lineage.as_ref()) {
            return response;
        }
        let from_native_session = from
            .as_ref()
            .and_then(|from| self.response_native_session(from));
        let to_native_session = self.response_native_session(&to);
        let conversation = match self.agent_requests.conversation(
            from.as_ref()
                .map(|from| Party::new(from, from_native_session.as_deref())),
            Party::new(&to, to_native_session.as_deref()),
            lineage.as_ref().map(|(parent, _, _)| parent.as_str()),
            follows,
        ) {
            Ok(conversation) => conversation,
            Err(response) => return response,
        };
        let id = new_request_id();
        let now = crate::runtime_metrics::unix_time_millis();
        let (parent, root, depth) = match lineage {
            Some((parent, root, depth)) => (Some(parent), root, depth),
            None => (None, id.clone(), 1),
        };
        let label = |agent: &protocol::AgentRef| {
            self.current_agent_runtime(agent)
                .ok()
                .map(|runtime| runtime.label)
        };
        let from_label = match &from {
            Some(from) => label(from),
            None => outside_asker_label(asker_label),
        };
        let to_label = label(&to);
        let info = AgentRequestInfo {
            conversation: Some(conversation.unwrap_or_else(|| id.clone())),
            id: id.clone(),
            root,
            parent,
            from_label,
            to_label,
            from_native_session,
            from,
            to_native_session,
            to,
            prompt,
            state: AgentRequestState::Queued,
            reason: Some("waiting for the agent to be ready".into()),
            response: None,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        self.agent_requests.revision += 1;
        self.agent_requests.requests.push(StoredRequest {
            info,
            depth,
            later: false,
        });
        self.pump_agent_requests();
        self.agent_request(&id)
    }

    pub(super) fn reply_to_request(
        &mut self,
        caller: Option<protocol::AgentRef>,
        id: &str,
        text: String,
    ) -> ControlResponse {
        let request = match self.agent_requests.lookup(id) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if let Some(response) = self.settled_reply(caller.as_ref(), request, &text) {
            return response;
        }
        let payload = AgentResponsePayload {
            text,
            turn_id: None,
            source: AgentResponseSource::AgentCli,
            fidelity: AgentResponseFidelity::Semantic,
            complete: true,
            truncated: false,
        };
        if payload.text.is_empty() {
            return invalid("a reply needs text");
        }
        if let Err((_, message)) = super::responses::validate_payload(&payload) {
            return invalid(message);
        }
        if let Err(response) = self.agent_requests.admit_reply(request, &payload.text) {
            return response;
        }
        // Kept on the request alone: `agent-cli` must not reach a peer through `agent-response`,
        // which a client without the request capability may send.
        let response = AgentResponse {
            response_id: self.agent_responses.next_id(),
            content: payload,
        };
        let info = self.agent_requests.info_mut(id).expect("looked up above");
        info.response = Some(response);
        settle(info, AgentRequestState::Answered, None);
        let answered = info.clone();
        self.pump_agent_requests();
        ControlResponse::ok(answered)
    }

    /// The requests attached clients are shown, for a client that has just attached.
    pub(super) fn agent_requests_message(&self) -> protocol::ServerMessage {
        protocol::ServerMessage::AgentRequestsChanged {
            requests: self.agent_requests.sent.clone(),
            server_now_unix_ms: crate::runtime_metrics::unix_time_millis(),
        }
    }

    /// Tell attached clients about requests that changed since they were last told.
    pub(super) fn broadcast_agent_request_changes(&mut self) {
        let store = &mut self.agent_requests;
        if store.revision == store.sent_revision {
            return;
        }
        store.sent_revision = store.revision;
        let requests = store.summaries();
        if requests == store.sent {
            return;
        }
        store.sent = requests.clone();
        self.broadcast_control(&protocol::ServerMessage::AgentRequestsChanged {
            requests,
            server_now_unix_ms: crate::runtime_metrics::unix_time_millis(),
        });
    }

    /// Keep request `id` open for its agent to reply after the turn it was delivered in: that
    /// turn's end no longer answers it from the terminal, and the agent can take the next request.
    pub(super) fn reply_later(
        &mut self,
        caller: Option<protocol::AgentRef>,
        id: &str,
    ) -> ControlResponse {
        let request = match self.agent_requests.lookup(id) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if let Some(refusal) = self.settled_reply(caller.as_ref(), request, "") {
            return refusal;
        }
        if request.state != AgentRequestState::Active {
            return conflict(format!(
                "request `{id}` is {}; only a delivered request can be answered later",
                state_name(request.state)
            ));
        }
        let to = request.to.clone();
        // The conversation the turn bound itself to, so the reply comes from the same one.
        let mut turn_session = None;
        if self.agent_requests.answering(&to) == Some(id) {
            let slot = self
                .agent_requests
                .slots
                .remove(&to)
                .expect("answering slot");
            if let Some(pane) = self.panes.get_mut(&to.pane.pane_id) {
                pane.agent.end_request_run(&to);
            }
            turn_session = slot
                .tracker
                .fence()
                .and_then(|fence| fence.native_session.clone());
        }
        self.agent_requests.revision += 1;
        let stored = self
            .agent_requests
            .requests
            .iter_mut()
            .find(|stored| stored.info.id == id)
            .expect("looked up above");
        stored.later = true;
        if stored.info.to_native_session.is_none() {
            stored.info.to_native_session = turn_session;
        }
        settle(
            &mut stored.info,
            AgentRequestState::Active,
            Some("the agent will reply later".into()),
        );
        let deferred = stored.info.clone();
        self.pump_agent_requests();
        ControlResponse::ok(deferred)
    }

    /// Fail a request its agent promised to answer later once that agent can no longer reply.
    fn check_deferred_requests(&mut self) {
        let deferred: Vec<(String, protocol::AgentRef, Option<String>)> = self
            .agent_requests
            .requests
            .iter()
            .filter(|stored| stored.later && stored.info.state == AgentRequestState::Active)
            .map(|stored| {
                (
                    stored.info.id.clone(),
                    stored.info.to.clone(),
                    stored.info.to_native_session.clone(),
                )
            })
            .collect();
        for (id, to, recorded_session) in deferred {
            let native_session = self.response_native_session(&to);
            let agent = self.request_agent(&to);
            let info = self.agent_requests.info_mut(&id).expect("listed above");
            let reason = match agent {
                Err(reason) => reason,
                // Bound to the first conversation the agent reports, as a turn's fence is.
                Ok(_) if recorded_session.is_none() => {
                    info.to_native_session = native_session;
                    continue;
                }
                Ok(_) if native_session != recorded_session => {
                    "the agent moved to another conversation".to_string()
                }
                Ok(_) => continue,
            };
            settle(info, AgentRequestState::Failed, Some(reason));
        }
    }

    /// The answer to a reply that stores nothing: a refusal, or an identical repeat.
    fn settled_reply(
        &self,
        caller: Option<&protocol::AgentRef>,
        request: &AgentRequestInfo,
        text: &str,
    ) -> Option<ControlResponse> {
        let id = &request.id;
        if caller != Some(&request.to) {
            return Some(not_permitted(format!(
                "only the agent request `{id}` was sent to can reply to it"
            )));
        }
        if request.to_native_session.is_some()
            && self.response_native_session(&request.to) != request.to_native_session
        {
            return Some(not_permitted(format!(
                "the agent has moved to another conversation since request `{id}` was sent"
            )));
        }
        if let Some(existing) = &request.response {
            let repeat = existing.content.source == AgentResponseSource::AgentCli
                && existing.content.text == text;
            return Some(if repeat {
                ControlResponse::ok(request.clone())
            } else {
                conflict(format!("request `{id}` is already answered"))
            });
        }
        if request.state.is_resolved() {
            return Some(conflict(format!(
                "request `{id}` is {}",
                state_name(request.state)
            )));
        }
        None
    }

    pub(super) fn agent_request(&self, id: &str) -> ControlResponse {
        match self.agent_requests.lookup(id) {
            Ok(request) => ControlResponse::ok(request.clone()),
            Err(response) => response,
        }
    }

    pub(super) fn agent_request_list(
        &self,
        caller: Option<protocol::AgentRef>,
        direction: Option<AgentRequestDirection>,
    ) -> ControlResponse {
        let filter = match (direction, caller) {
            (None, _) => None,
            (Some(direction), Some(caller)) => Some((direction, caller)),
            (Some(_), None) => {
                return invalid("--incoming and --outgoing need to be run by an agent");
            }
        };
        let requests = self
            .agent_requests
            .requests
            .iter()
            .map(|stored| &stored.info)
            .filter(|info| match &filter {
                None => true,
                Some((AgentRequestDirection::Incoming, caller)) => info.to == *caller,
                Some((AgentRequestDirection::Outgoing, caller)) => {
                    info.from.as_ref() == Some(caller)
                }
            })
            .map(Into::into)
            .collect();
        ControlResponse::ok(AgentRequestList(requests))
    }

    pub(super) fn cancel_request(
        &mut self,
        caller: Option<protocol::AgentRef>,
        id: &str,
    ) -> ControlResponse {
        let request = match self.agent_requests.lookup(id) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if request.from != caller {
            return not_permitted(format!("only whoever asked request `{id}` can cancel it"));
        }
        match request.state {
            AgentRequestState::Cancelled => return ControlResponse::ok(request.clone()),
            AgentRequestState::Answered | AgentRequestState::Failed => {
                return conflict(format!(
                    "request `{id}` is already {}",
                    state_name(request.state)
                ));
            }
            AgentRequestState::Queued | AgentRequestState::Active => {}
        }
        let info = self.agent_requests.info_mut(id).expect("looked up above");
        settle(
            info,
            AgentRequestState::Cancelled,
            Some("cancelled by the requester".into()),
        );
        let cancelled = info.clone();
        self.pump_agent_requests();
        ControlResponse::ok(cancelled)
    }

    /// Answer at once for a resolved request, or hold `reply` until it resolves.
    pub(super) fn register_request_await(
        &mut self,
        client_id: ClientId,
        caller: Option<protocol::AgentRef>,
        id: &str,
        timeout_ms: Option<u64>,
        reply: ReplyTo,
    ) -> Option<ControlResponse> {
        let pending = PendingAwait {
            id: id.to_owned(),
            caller,
            deadline: timeout_ms
                .map(Duration::from_millis)
                .and_then(|duration| Instant::now().checked_add(duration)),
            reply,
        };
        if let Some(response) = self.await_answer(&pending) {
            return Some(response);
        }
        self.agent_requests
            .awaits
            .insert((client_id, pending.reply.hold_id()), pending);
        None
    }

    /// The answer an await gets now, or `None` while it keeps waiting.
    fn await_answer(&self, pending: &PendingAwait) -> Option<ControlResponse> {
        let request = match self.agent_requests.lookup(&pending.id) {
            Ok(request) => request,
            Err(response) => return Some(response),
        };
        if request.state.is_resolved() {
            return Some(ControlResponse::ok(request.clone()));
        }
        if let Some(inbound) = self
            .agent_requests
            .await_cycle(pending.caller.as_ref(), &pending.id)
        {
            return Some(conflict(format!(
                "request `{inbound}` is yours to answer and `{}` waits on it; answer it with `rozi agents reply {inbound} --format text --stdin` instead of waiting",
                pending.id
            )));
        }
        pending
            .deadline
            .filter(|deadline| Instant::now() >= *deadline)
            .map(|_| {
                ControlResponse::error_with(
                    ControlErrorCode::Timeout,
                    format!(
                        "timed out waiting for request `{}`; it is {}",
                        pending.id,
                        state_name(request.state)
                    ),
                )
            })
    }

    /// Advance every request: settle the turns of delivered ones, deliver what is ready, and
    /// answer the awaits that can be answered.
    pub(super) fn pump_agent_requests(&mut self) {
        if self.agent_requests.has_work() {
            let targets: Vec<_> = self.agent_requests.slots.keys().cloned().collect();
            for target in targets {
                self.settle_request_turn(&target);
            }
            self.check_deferred_requests();
            self.deliver_queued_requests();
            self.answer_request_awaits();
        }
        self.agent_requests.prune();
    }

    fn settle_request_turn(&mut self, target: &protocol::AgentRef) {
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        // The history read settles it when it finishes.
        if self.reading_request_history(&slot.request) {
            return;
        }
        let runtime = match self.request_agent(target) {
            Ok(runtime) => runtime,
            Err(reason) => return self.finish_request_turn(target, TurnOutcome::Failed(reason)),
        };
        let native_session = self.response_native_session(target);
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        if slot.tracker.native_session_changed(&native_session) {
            return self.finish_request_turn(
                target,
                TurnOutcome::Failed("the agent moved to another conversation".into()),
            );
        }
        if let Some(response) = slot.tracker.fresh_response(&self.agent_responses, target) {
            let response = response.clone();
            return self.finish_request_turn(target, TurnOutcome::Answered(response));
        }
        let stopped = runtime.state.is_stopped();
        if !self.request_turn_ready(target, runtime.state) {
            return;
        }
        let undetected_turn = self.agent_requests.undetected_turn;
        let slot = self
            .agent_requests
            .slots
            .get_mut(target)
            .expect("listed slot");
        slot.tracker.adopt_native_session(native_session);
        slot.departed |= !stopped;
        let elapsed = slot.delivered.elapsed();
        let turn_ended = slot.tracker.turn_ended(stopped, Some(runtime.state));
        let unconfirmed = stopped && !slot.departed && !slot.shown && !turn_ended;
        let undetected = stopped && !slot.departed && elapsed >= undetected_turn;
        let id = slot.request.clone();
        // Neither a turn, a reply, nor the screen has shown that the agent took the request.
        if unconfirmed
            && (undetected || elapsed >= self.agent_requests.delivery_check)
            && self.awaiting_delivery(&id)
        {
            match self.delivery(target) {
                Delivery::Shown => {
                    self.agent_requests
                        .slots
                        .get_mut(target)
                        .expect("listed slot")
                        .shown = true;
                }
                // Until the turn is given up on, a dropped request is typed again.
                Delivery::Lost if !undetected && self.retype_request(target) => return,
                Delivery::Lost | Delivery::Missing => {
                    return self.finish_request_turn(target, TurnOutcome::Failed(NOT_TAKEN.into()));
                }
            }
        }
        if turn_ended || undetected {
            self.answer_request_from_screen(target);
        }
    }

    /// A screen can clear before the agent reports work again. Wait out that gap before
    /// treating the old stopped report as a turn ending or a delivery timing out.
    fn request_turn_ready(
        &mut self,
        target: &protocol::AgentRef,
        state: protocol::AgentState,
    ) -> bool {
        let was_blocked = self.agent_requests.slots[target].waiting_after_screen_block;
        let working = state == protocol::AgentState::Working;
        let screen_blocked = (!working || was_blocked)
            && self.validate_agent_input_reference(target).is_ok()
            && self
                .refuse_blocked_agent_screen(target.pane.pane_id)
                .is_err();
        let stopped_since = self
            .agent_requests
            .observe(target, state.is_stopped() && !screen_blocked);
        let waiting_after_screen_block = screen_blocked
            || (was_blocked
                && !working
                && stopped_since.is_none_or(|since| {
                    !self.agent_requests.quiet_since(target.pane.pane_id, since)
                }));
        self.agent_requests
            .slots
            .get_mut(target)
            .expect("listed slot")
            .waiting_after_screen_block = waiting_after_screen_block;
        !waiting_after_screen_block
    }

    /// Whether request `id` still needs proof its agent took it: it is unanswered, and its agent
    /// has not promised a later reply, which it could only do having read it.
    fn awaiting_delivery(&self, id: &str) -> bool {
        self.agent_requests
            .get(id)
            .is_some_and(|stored| stored.info.state == AgentRequestState::Active && !stored.later)
    }

    /// Whether `target`'s pane shows the request it was given, and if not, whether anything on
    /// it changed since the request was typed.
    fn delivery(&self, target: &protocol::AgentRef) -> Delivery {
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        let Some(text) = self.recent_pane_text(target.pane.pane_id) else {
            return Delivery::Missing;
        };
        if shows_request(&text, &slot.request) {
            Delivery::Shown
        } else if fingerprint(&text) == slot.before {
            Delivery::Lost
        } else {
            Delivery::Missing
        }
    }

    /// Type `target`'s request again after the first typing left no trace, when nothing stops a
    /// delivery now. False when it cannot be retyped, having been typed as often as it may be.
    fn retype_request(&mut self, target: &protocol::AgentRef) -> bool {
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        if slot.attempts >= MAX_DELIVERY_ATTEMPTS {
            return false;
        }
        let id = slot.request.clone();
        // The same guards as the first typing: the pane must have been quiet for the whole quiet
        // window, so a draft someone just started is never sent with the request.
        let quiet = self
            .agent_requests
            .stopped_since
            .get(target)
            .is_some_and(|since| self.agent_requests.quiet_since(target.pane.pane_id, *since));
        let blocked = !quiet
            || self.typing_blocker(target).is_some()
            || self
                .refuse_blocked_agent_screen(target.pane.pane_id)
                .is_err();
        if blocked {
            // Typed nowhere yet; the next pump looks again, until the turn is given up on.
            return true;
        }
        let request = &self.agent_requests.get(&id).expect("slot request").info;
        let frame = self.request_frame(request);
        let before = self
            .recent_pane_text(target.pane.pane_id)
            .map(|text| fingerprint(&text));
        let typed = self
            .session_prompt_bytes(target.pane.pane_id, &frame)
            .and_then(|bytes| self.session_write_input(target.pane.pane_id, bytes));
        if typed.is_err() {
            return false;
        }
        let slot = self
            .agent_requests
            .slots
            .get_mut(target)
            .expect("listed slot");
        slot.attempts += 1;
        slot.delivered = Instant::now();
        slot.before = before.unwrap_or_default();
        true
    }

    /// The trailing [`RECENT_LINES`] of pane `pane`, scrollback included.
    fn recent_pane_text(&self, pane: PaneId) -> Option<String> {
        let screen = self.panes.get(&pane)?.screen();
        let total = screen.total_text_lines();
        Some(screen.export_text(total.saturating_sub(RECENT_LINES), total))
    }

    /// Answer `target`'s request from what its turn left on the terminal. The slot stays occupied
    /// until any history read this starts has restored the viewport.
    fn answer_request_from_screen(&mut self, target: &protocol::AgentRef) {
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        let id = slot.request.clone();
        // A request its agent already replied to needs nothing from the screen.
        let Some(fence) = slot
            .tracker
            .fence()
            .cloned()
            .filter(|_| self.request_active(&id))
        else {
            return self.finish_request_turn(target, TurnOutcome::Unanswered);
        };
        let wait = super::agent_history::HistoryWait {
            consumer: super::agent_history::HistoryConsumer::Request {
                id,
                reference: target.clone(),
            },
            deadline: None,
            fence,
        };
        self.answer_from_screen(wait, Some(target.clone()));
    }

    /// Settle request `id` with what was read from `target`'s terminal, and free the slot.
    pub(super) fn finish_request_history(
        &mut self,
        target: &protocol::AgentRef,
        id: &str,
        error: Option<ControlResponse>,
        response: Option<AgentResponse>,
    ) {
        if self.agent_requests.answering(target) != Some(id) {
            return;
        }
        let outcome = match (error, response) {
            (Some(error), _) => TurnOutcome::Failed(error_text(error)),
            (None, Some(response)) => TurnOutcome::Answered(response),
            (None, None) => TurnOutcome::Unanswered,
        };
        self.finish_request_turn(target, outcome);
    }

    pub(super) fn request_active(&self, id: &str) -> bool {
        self.agent_requests
            .get(id)
            .is_some_and(|stored| stored.info.state == AgentRequestState::Active)
    }

    /// Free `target`'s slot, resolving its request if nothing has resolved it yet.
    fn finish_request_turn(&mut self, target: &protocol::AgentRef, outcome: TurnOutcome) {
        let slot = self
            .agent_requests
            .slots
            .remove(target)
            .expect("listed slot");
        if let Some(pane) = self.panes.get_mut(&target.pane.pane_id) {
            pane.agent.end_request_run(target);
        }
        if !self.request_active(&slot.request) {
            return;
        }
        let (state, reason, response) = match outcome {
            TurnOutcome::Failed(reason) => (AgentRequestState::Failed, Some(reason), None),
            TurnOutcome::Answered(response) => (AgentRequestState::Answered, None, Some(response)),
            TurnOutcome::Unanswered => (
                AgentRequestState::Failed,
                Some("the turn ended with nothing on screen".into()),
                None,
            ),
        };
        let info = self
            .agent_requests
            .info_mut(&slot.request)
            .expect("checked above");
        info.response = response;
        settle(info, state, reason);
    }

    fn deliver_queued_requests(&mut self) {
        let queued: Vec<String> = self
            .agent_requests
            .requests
            .iter()
            .filter(|stored| stored.info.state == AgentRequestState::Queued)
            .map(|stored| stored.info.id.clone())
            .collect();
        for id in queued {
            if let Some(reason) = self.requester_gone(&id) {
                let info = self.agent_requests.info_mut(&id).expect("queued above");
                settle(info, AgentRequestState::Cancelled, Some(reason));
                continue;
            }
            let outcome = match self.request_readiness(&id) {
                Readiness::Ready(state) => match self.deliver_request(&id, state) {
                    Ok(()) => continue,
                    Err(reason) => Readiness::Wait(reason),
                },
                other => other,
            };
            let info = self.agent_requests.info_mut(&id).expect("queued above");
            match outcome {
                Readiness::Fail(reason) => settle(info, AgentRequestState::Failed, Some(reason)),
                Readiness::Wait(reason) if info.reason.as_deref() != Some(reason.as_str()) => {
                    settle(info, AgentRequestState::Queued, Some(reason));
                }
                Readiness::Wait(_) | Readiness::Ready(_) => {}
            }
        }
    }

    /// Why a queued request's asking agent can no longer receive its answer. A conversation not
    /// known when it asked is the one it is first seen in.
    fn requester_gone(&mut self, id: &str) -> Option<String> {
        let info = &self.agent_requests.get(id)?.info;
        let from = info.from.as_ref()?;
        if let Err(reason) = self.request_agent(from) {
            return Some(format!("the asking agent is gone: {reason}"));
        }
        let native_session = self.response_native_session(from);
        if info.from_native_session.is_none() {
            self.agent_requests.info_mut(id)?.from_native_session = native_session;
            return None;
        }
        (native_session != info.from_native_session)
            .then(|| "the asking agent moved to another conversation".into())
    }

    /// The current runtime of `agent`, or why it is gone.
    fn request_agent(
        &self,
        agent: &protocol::AgentRef,
    ) -> std::result::Result<protocol::AgentRuntime, String> {
        if self
            .panes
            .get(&agent.pane.pane_id)
            .is_some_and(|pane| pane.exited.is_some())
        {
            return Err("the agent's pane has exited".into());
        }
        self.current_agent_runtime(agent).map_err(error_text)
    }

    fn request_readiness(&mut self, id: &str) -> Readiness {
        let request = &self.agent_requests.get(id).expect("queued request").info;
        let to = request.to.clone();
        let recorded_session = request.to_native_session.clone();
        let runtime = match self.request_agent(&to) {
            Ok(runtime) => runtime,
            Err(reason) => return Readiness::Fail(reason),
        };
        let native_session = self.response_native_session(&to);
        if recorded_session.is_some() && native_session != recorded_session {
            return Readiness::Fail("the agent moved to another conversation".into());
        }
        if !runtime.state.is_stopped() {
            self.agent_requests.observe(&to, false);
            return Readiness::Wait(format!("the agent is {}", runtime.state.as_str()));
        }
        if let Some(reason) = self.delivery_blocker(&to) {
            return Readiness::Wait(reason);
        }
        if let Err(response) = self.refuse_blocked_agent_screen(to.pane.pane_id) {
            self.agent_requests.observe(&to, false);
            return Readiness::Wait(error_text(response));
        }
        let stopped_since = self
            .agent_requests
            .observe(&to, true)
            .expect("stopped agent has a timestamp");
        if !self
            .agent_requests
            .quiet_since(to.pane.pane_id, stopped_since)
        {
            return Readiness::Wait("waiting for the agent's pane to settle".into());
        }
        if recorded_session.is_none() {
            let info = self.agent_requests.info_mut(id).expect("queued request");
            info.to_native_session = native_session;
        }
        Readiness::Ready(runtime.state)
    }

    /// What keeps a stopped agent from taking a request now, other than time.
    fn delivery_blocker(&self, to: &protocol::AgentRef) -> Option<String> {
        if let Some(busy) = self.agent_requests.answering(to) {
            return Some(format!("the agent is answering request `{busy}`"));
        }
        self.typing_blocker(to)
    }

    /// What makes typing into `to`'s pane unsafe now, whether a request is first typed or typed
    /// again: anything but the request it may already be answering.
    fn typing_blocker(&self, to: &protocol::AgentRef) -> Option<String> {
        if let Err(response) = self.validate_agent_input_reference(to) {
            return Some(error_text(response));
        }
        if self.input_locked {
            return Some("the session's input is locked".into());
        }
        if self.has_prompt_in_progress(to, true) {
            return Some("the agent has a prompt or history read in progress".into());
        }
        None
    }

    /// Type request `id` into its target, armed to recognize the end of the turn it starts.
    fn deliver_request(
        &mut self,
        id: &str,
        state: protocol::AgentState,
    ) -> std::result::Result<(), String> {
        let request = &self.agent_requests.get(id).expect("queued request").info;
        let to = request.to.clone();
        let frame = self.request_frame(request);
        let bytes = self
            .session_prompt_bytes(to.pane.pane_id, &frame)
            .map_err(error_text)?;
        let fence = self
            .agent_responses
            .fence(self.response_native_session(&to));
        let before = self
            .recent_pane_text(to.pane.pane_id)
            .map(|text| fingerprint(&text))
            .unwrap_or_default();
        // The delivery may end without the agent ever leaving its stopped state, so the next
        // request waits out a fresh quiet window instead of inheriting the one before it.
        self.agent_requests.stopped_since.remove(&to);
        self.agent_requests.slots.insert(
            to.clone(),
            DeliverySlot {
                request: id.to_owned(),
                tracker: AgentTurnTracker::new(true, Some(state), false, Some(fence)),
                delivered: Instant::now(),
                departed: false,
                waiting_after_screen_block: false,
                shown: false,
                attempts: 1,
                before,
            },
        );
        if let Err(response) = self.session_write_input(to.pane.pane_id, bytes) {
            self.agent_requests.slots.remove(&to);
            return Err(error_text(response));
        }
        if let Some(pane) = self.panes.get_mut(&to.pane.pane_id) {
            pane.agent.begin_request_run(&to);
        }
        let info = self.agent_requests.info_mut(id).expect("queued request");
        settle(info, AgentRequestState::Active, None);
        Ok(())
    }

    fn request_frame(&self, request: &AgentRequestInfo) -> String {
        let asker = match &request.from {
            Some(from) => self
                .current_agent_runtime(from)
                .ok()
                .map(|runtime| {
                    format!(
                        " from {} in pane {}",
                        runtime.label, runtime.reference.pane.pane_id
                    )
                })
                .unwrap_or_default(),
            None => request
                .from_label
                .as_ref()
                .map(|label| format!(" from \"{label}\", outside this session"))
                .unwrap_or_default(),
        };
        // The exchange before this one in its conversation; requests are stored as asked.
        let requests = &self.agent_requests.requests;
        let asked_before = requests
            .iter()
            .position(|stored| stored.info.id == request.id)
            .unwrap_or(requests.len());
        let follows = requests[..asked_before]
            .iter()
            .map(|stored| &stored.info)
            .filter(|_| request.parent.is_none())
            .rfind(|earlier| {
                earlier.parent.is_none() && earlier.conversation() == request.conversation()
            })
            .map(|earlier| format!(", following {}", earlier.id))
            .unwrap_or_default();
        format!(
            "Request {id}{asker}{follows}:\n\n{prompt}\n\nAnswer with `rozi agents reply {id} --format text --stdin`. To answer after this turn ends, such as when a background command finishes, run `rozi agents reply {id} --format text --later` first.",
            id = request.id,
            prompt = request.prompt,
        )
    }

    fn answer_request_awaits(&mut self) {
        let answered: Vec<(WaitKey, ControlResponse)> = self
            .agent_requests
            .awaits
            .iter()
            .filter_map(|(key, pending)| Some((*key, self.await_answer(pending)?)))
            .collect();
        for (key, response) in answered {
            if let Some(pending) = self.agent_requests.awaits.remove(&key) {
                self.answer_held(key.0, &pending.reply, response);
            }
        }
    }
}

/// Whether `text` shows request `id`: its id, which the typed request opens with, even where the
/// terminal wrapped it across rows.
fn shows_request(text: &str, id: &str) -> bool {
    text.contains(id) || text.replace('\n', "").contains(id)
}

fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn state_name(state: AgentRequestState) -> &'static str {
    match state {
        AgentRequestState::Queued => "queued",
        AgentRequestState::Active => "active",
        AgentRequestState::Answered => "answered",
        AgentRequestState::Failed => "failed",
        AgentRequestState::Cancelled => "cancelled",
    }
}

#[cfg(all(test, unix))]
mod tests;
