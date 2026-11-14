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

struct DeliverySlot {
    request: String,
    tracker: AgentTurnTracker,
    delivered: Instant,
    /// Whether the agent has been seen leaving its stopped state since delivery.
    departed: bool,
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

    fn has_work(&self) -> bool {
        !self.slots.is_empty()
            || !self.awaits.is_empty()
            || self.requests.iter().any(|stored| {
                stored.info.state == AgentRequestState::Queued
                    || (stored.later && stored.info.state == AgentRequestState::Active)
            })
    }

    /// Where a request asked by `from` sits in the lineage: under the request `from` is answering,
    /// or else the newest one it promised to answer later.
    fn lineage(&self, from: Option<&protocol::AgentRef>) -> Option<(String, String, usize)> {
        let from = from?;
        let parent = match self.answering(from) {
            Some(id) => self.get(id)?,
            None => self.requests.iter().rev().find(|stored| {
                stored.later
                    && stored.info.state == AgentRequestState::Active
                    && stored.info.to == *from
            })?,
        };
        Some((
            parent.info.id.clone(),
            parent.info.root.clone(),
            parent.depth + 1,
        ))
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
        let lineage = self.agent_requests.lineage(from.as_ref());
        if let Err(response) = self.agent_requests.admit(&to, lineage.as_ref()) {
            return response;
        }
        let id = new_request_id();
        let now = crate::runtime_metrics::unix_time_millis();
        let (parent, root, depth) = match lineage {
            Some((parent, root, depth)) => (Some(parent), root, depth),
            None => (None, id.clone(), 1),
        };
        let info = AgentRequestInfo {
            id: id.clone(),
            root,
            parent,
            from_native_session: from
                .as_ref()
                .and_then(|from| self.response_native_session(from)),
            from,
            to_native_session: self.response_native_session(&to),
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

    /// Keep request `id` open for its agent to reply after the turn it was delivered in: that
    /// turn's end no longer answers it from the terminal, and the agent can take the next request.
    /// The requests attached clients are shown, for a client that has just attached.
    pub(super) fn agent_requests_message(&self) -> protocol::ServerMessage {
        protocol::ServerMessage::AgentRequestsChanged {
            requests: self.agent_requests.sent.clone(),
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
        self.broadcast_control(&protocol::ServerMessage::AgentRequestsChanged { requests });
    }

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
        if self.agent_requests.answering(&to) == Some(id) {
            self.agent_requests.slots.remove(&to);
        }
        self.agent_requests.revision += 1;
        let stored = self
            .agent_requests
            .requests
            .iter_mut()
            .find(|stored| stored.info.id == id)
            .expect("looked up above");
        stored.later = true;
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
            let reason = match self.request_agent(&to) {
                Err(reason) => Some(reason),
                Ok(_)
                    if recorded_session.is_some()
                        && self.response_native_session(&to) != recorded_session =>
                {
                    Some("the agent moved to another conversation".to_string())
                }
                Ok(_) => None,
            };
            if let Some(reason) = reason {
                let info = self.agent_requests.info_mut(&id).expect("listed above");
                settle(info, AgentRequestState::Failed, Some(reason));
            }
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
        let from_screen = request
            .response
            .as_ref()
            .is_some_and(|existing| existing.content.fidelity == AgentResponseFidelity::Rendered);
        if let Some(existing) = request.response.as_ref().filter(|_| !from_screen) {
            let repeat = existing.content.source == AgentResponseSource::AgentCli
                && existing.content.text == text;
            return Some(if repeat {
                ControlResponse::ok(request.clone())
            } else {
                conflict(format!("request `{id}` is already answered"))
            });
        }
        // A turn can end before the agent replies, such as while a command it started in the
        // background still runs. Its reply then replaces what was read from its terminal.
        if request.state.is_resolved() && !from_screen {
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
                "request `{inbound}` is yours to answer and `{}` waits on it; answer it with `rozi agents reply {inbound} --stdin` instead of waiting",
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
        self.agent_requests.observe(target, stopped);
        let undetected_turn = self.agent_requests.undetected_turn;
        let slot = self
            .agent_requests
            .slots
            .get_mut(target)
            .expect("listed slot");
        slot.tracker.adopt_native_session(native_session);
        slot.departed |= !stopped;
        let ended = slot.tracker.turn_ended(stopped, Some(runtime.state))
            || (stopped && !slot.departed && slot.delivered.elapsed() >= undetected_turn);
        if ended {
            self.answer_request_from_screen(target);
        }
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
        let Some(stopped_since) = self.agent_requests.observe(&to, runtime.state.is_stopped())
        else {
            return Readiness::Wait(format!("the agent is {}", runtime.state.as_str()));
        };
        if let Some(reason) = self.delivery_blocker(&to) {
            return Readiness::Wait(reason);
        }
        if !self
            .agent_requests
            .quiet_since(to.pane.pane_id, stopped_since)
        {
            return Readiness::Wait("waiting for the agent's pane to settle".into());
        }
        if let Err(response) = self.refuse_unannounced_dialog(to.pane.pane_id) {
            return Readiness::Wait(error_text(response));
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
        if let Err(response) = self.validate_agent_input_reference(to) {
            return Some(error_text(response));
        }
        if self.input_locked {
            return Some("the session's input is locked".into());
        }
        if self.has_conflicting_prompt(to, true) {
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
            },
        );
        if let Err(response) = self.session_write_input(to.pane.pane_id, bytes) {
            self.agent_requests.slots.remove(&to);
            return Err(error_text(response));
        }
        let info = self.agent_requests.info_mut(id).expect("queued request");
        settle(info, AgentRequestState::Active, None);
        Ok(())
    }

    fn request_frame(&self, request: &AgentRequestInfo) -> String {
        let asker = request
            .from
            .as_ref()
            .and_then(|from| self.current_agent_runtime(from).ok())
            .map(|runtime| {
                format!(
                    " from {} in pane {}",
                    runtime.label, runtime.reference.pane.pane_id
                )
            })
            .unwrap_or_default();
        format!(
            "Request {id}{asker}:\n\n{prompt}\n\nAnswer with `rozi agents reply {id} --stdin`. To answer after this turn ends, such as when a background command finishes, run `rozi agents reply {id} --later` first.",
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
