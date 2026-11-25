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

const WAITING_FOR_RESUME: &str = "waiting for the agent to resume its conversation";

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
/// How long the launch that took a request may switch conversations with its published row and
/// its own report naming different ones, before the turn gives up on the request.
const SWITCH_WINDOW: Duration = Duration::from_secs(10);
/// Why a turn's request fails when its agent's row and its own report never agree.
const UNSETTLED_SWITCH: &str =
    "the agent's published row and its own report kept naming different conversations";
/// How many times a request is typed into an agent that shows no trace of it.
const MAX_DELIVERY_ATTEMPTS: u8 = 2;
/// The trailing lines of a pane, visible screen and scrollback, searched for a delivered request.
const RECENT_LINES: usize = 500;
/// Why a request fails when its agent never took it.
const NOT_TAKEN: &str =
    "the agent did not take the request: it never appeared on the agent's screen";
/// How long a stopped agent whose screen keeps changing is waited on before it counts as still.
/// Some agents animate even at rest, and those must not hold their requests forever.
const SCREEN_SETTLE_LIMIT: Duration = Duration::from_secs(10);
/// What an active request says once it has been typed into its agent.
const DELIVERED: &str = "typed into the agent; waiting for its answer";
/// What an active request says once typed into an agent that was busy at the time.
const DELIVERED_MID_TURN: &str =
    "typed into the agent while it was busy; it takes it as a follow-up";
/// How many requests may be typed into an agent with `now` during a run it is answering another.
const MAX_RIDERS: usize = 4;

/// What an ask asks for besides its target and text.
#[derive(Default)]
pub(super) struct AskOptions<'a> {
    /// The request the asking agent asks on behalf of.
    pub parent: Option<&'a str>,
    /// An earlier request whose conversation this one continues.
    pub follows: Option<&'a str>,
    /// How an asker that is not an agent of this session names itself.
    pub asker_label: Option<String>,
    /// Type the request in at once, or refuse it.
    pub now: bool,
    pub no_reply: bool,
}

pub(super) struct AgentRequestStore {
    /// Oldest first.
    requests: Vec<StoredRequest>,
    /// The request each agent is answering, until its turn ends.
    slots: HashMap<protocol::AgentRef, DeliverySlot>,
    awaits: HashMap<WaitKey, PendingAwait>,
    /// When unsubmitted input was last written to a pane (keyboard input, `send-text`, or
    /// `send-keys`), kept for one quiet window so requests do not type over a draft.
    pane_input: HashMap<PaneId, Instant>,
    /// Each watched pane's content generation, and since when it has had it.
    screens: HashMap<PaneId, (u64, Instant)>,
    /// Since when each target has been seen stopped.
    stopped_since: HashMap<protocol::AgentRef, Instant>,
    pub(super) quiet: Duration,
    undetected_turn: Duration,
    delivery_check: Duration,
    switch_window: Duration,
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
            screens: HashMap::new(),
            stopped_since: HashMap::new(),
            quiet: QUIET_WINDOW,
            undetected_turn: UNDETECTED_TURN,
            delivery_check: DELIVERY_CHECK,
            switch_window: SWITCH_WINDOW,
            revision: 0,
            sent_revision: 0,
            sent: Vec::new(),
        }
    }
}

struct StoredRequest {
    info: AgentRequestInfo,
    /// Agent kind that can prove it resumed the recorded native conversation.
    target_agent: String,
    /// The native conversation the request was bound to before a handoff to a reference that
    /// names none stopped it insisting on one. Its agent can still exit and resume it.
    resumable_session: Option<String>,
    /// The pane's agent launch the request was asked of, delivered to, or later resumed in. Only a later
    /// launch can have resumed it in a conversation other than the row it was sent to.
    launch: u64,
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
    /// Requests typed in with `now` while this one was being answered. The agent takes them in
    /// the same run, so they settle with it unless answered first.
    riders: Vec<String>,
    /// Something was typed in while the agent was busy, which it may take as a follow-up turn
    /// straight after this one: the run ends only once the agent has stayed stopped for a quiet
    /// window, and its requests are matched to the integration's answers.
    mid_turn: bool,
    /// What the agent's integration published during the run, oldest first: the last answers
    /// one request each, as many as the run holds.
    answers: Vec<AgentResponse>,
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
    /// Since when the agent's published row and its own report have named different
    /// conversations.
    contradicted_since: Option<Instant>,
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

/// Which native conversation an agent is in now, as a request trusts it.
enum Answering {
    Known(String),
    Unknown,
    /// Its row and its own report name different ones.
    Contradicted,
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
    /// Answered by the run's [`DeliverySlot::answers`].
    Published,
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
                let known = stored.info.to_native_session.clone();
                handoff.rename_session(&mut stored.info.to_native_session);
                if stored.info.to_native_session.is_none() && known.is_some() {
                    stored.resumable_session = known;
                }
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

    /// Record that rozi submitted a complete prompt (text + Enter) to `pane`.
    /// That leaves no unsent draft, so it clears any prior typing mark and resets the pane's
    /// stopped-state quiet timer.
    pub(super) fn note_prompt_submitted(&mut self, pane: PaneId) {
        self.pane_input.remove(&pane);
        self.stopped_since
            .retain(|agent, _| agent.pane.pane_id != pane);
    }

    /// The request `agent` is answering, if any.
    pub(super) fn answering(&self, agent: &protocol::AgentRef) -> Option<&str> {
        self.slots.get(agent).map(|slot| slot.request.as_str())
    }

    /// Whether request `id` was typed into `agent` in the run it is in now.
    fn in_run(&self, agent: &protocol::AgentRef, id: &str) -> bool {
        self.slots
            .get(agent)
            .is_some_and(|slot| slot.request == id || slot.riders.iter().any(|rider| rider == id))
    }

    /// Whether someone typed into `pane` within the quiet window.
    fn someone_typed(&self, pane: PaneId) -> bool {
        self.pane_input
            .get(&pane)
            .is_some_and(|input| input.elapsed() < self.quiet)
    }

    /// Note what `pane` shows now.
    fn watch_screen(&mut self, pane: PaneId, generation: u64) {
        let seen = self
            .screens
            .entry(pane)
            .or_insert((generation, Instant::now()));
        if seen.0 != generation {
            *seen = (generation, Instant::now());
        }
    }

    /// Whether `pane` has shown the same thing for a quiet window, or its agent has been stopped
    /// too long to keep waiting for a screen that will not settle.
    fn screen_still(&self, pane: PaneId, stopped_since: Instant) -> bool {
        self.screens
            .get(&pane)
            .is_some_and(|(_, since)| since.elapsed() >= self.quiet)
            || stopped_since.elapsed() >= SCREEN_SETTLE_LIMIT
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

    /// Note the agent launch request `id` is answered from. Clients are not shown it.
    fn set_launch(&mut self, id: &str, launch: u64) {
        if let Some(stored) = self.requests.iter_mut().find(|stored| stored.info.id == id) {
            stored.launch = launch;
        }
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
                (self.in_run(from, &stored.info.id)
                    && stored.info.state == AgentRequestState::Active)
                    || self.answering(from) == Some(stored.info.id.as_str())
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
                !stored.info.state.is_resolved() || self.in_run(&stored.info.to, &stored.info.id)
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
        if !self.in_run(&request.to, &request.id) {
            return Ok(());
        }
        let protected_bytes: usize = self
            .requests
            .iter()
            .filter(|stored| {
                stored.info.state.is_resolved() && self.in_run(&stored.info.to, &stored.info.id)
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
    fn await_cycle(
        &self,
        caller: Option<&protocol::AgentRef>,
        awaited: &str,
    ) -> Option<&AgentRequestInfo> {
        let caller = caller?;
        self.requests
            .iter()
            .map(|stored| &stored.info)
            .find(|info| {
                !info.no_reply
                    && !info.state.is_resolved()
                    && info.to == *caller
                    && self.descends_from(info, awaited)
            })
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
        let panes: HashSet<PaneId> = targets.iter().map(|agent| agent.pane.pane_id).collect();
        self.screens.retain(|pane, _| panes.contains(pane));
        let resolved = || {
            self.requests
                .iter()
                .filter(|stored| stored.info.state.is_resolved())
        };
        let mut resolved_count = resolved().count();
        let mut resolved_bytes: usize = resolved().map(|stored| request_bytes(&stored.info)).sum();
        while resolved_bytes > MAX_RESOLVED_BYTES || resolved_count > MAX_RESOLVED {
            let Some(index) = self.requests.iter().position(|stored| {
                stored.info.state.is_resolved() && !self.in_run(&stored.info.to, &stored.info.id)
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
        options: AskOptions<'_>,
    ) -> ControlResponse {
        let AskOptions {
            parent,
            follows,
            asker_label,
            now,
            no_reply,
        } = options;
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
        let now_state = if now {
            match self.now_readiness(&to) {
                Ok(state) => Some(state),
                Err(response) => return response,
            }
        } else {
            None
        };
        let from_native_session = from.as_ref().and_then(|from| self.answering_session(from));
        let to_native_session = self.answering_session(&to);
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
        let created = crate::runtime_metrics::unix_time_millis();
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
        let target_agent = match self.current_agent_runtime(&to) {
            Ok(runtime) => runtime,
            Err(response) => return response,
        };
        let to_label = Some(target_agent.label);
        let info = AgentRequestInfo {
            conversation: Some(conversation.unwrap_or_else(|| id.clone())),
            id: id.clone(),
            root,
            parent,
            from_label,
            to_label,
            from_native_session,
            from,
            to_native_session: to_native_session.clone(),
            to: to.clone(),
            prompt,
            no_reply,
            state: AgentRequestState::Queued,
            reason: Some("waiting for the agent to be ready".into()),
            response: None,
            created_at_unix_ms: created,
            updated_at_unix_ms: created,
            delivered_at_unix_ms: None,
        };
        self.agent_requests.revision += 1;
        let launch = self.panes[&to.pane.pane_id].runtime.agent_launch;
        self.agent_requests.requests.push(StoredRequest {
            info,
            target_agent: target_agent.identity.id,
            resumable_session: None,
            launch,
            depth,
            later: false,
        });
        if let Some(state) = now_state {
            if to_native_session.is_none() {
                let native_session = self.answering_session(&to);
                let info = self.agent_requests.info_mut(&id).expect("just asked");
                info.to_native_session = native_session;
            }
            if let Err(reason) = self.deliver_request(&id, state) {
                let info = self.agent_requests.info_mut(&id).expect("just asked");
                settle(info, AgentRequestState::Failed, Some(reason));
            }
        }
        self.pump_agent_requests();
        self.agent_request(&id)
    }

    /// The state of an agent a request may be typed into now, whatever it is doing, or why not.
    /// Only what would make the typing land somewhere else refuses it: a dialog, or someone else's
    /// draft.
    fn now_readiness(
        &mut self,
        to: &protocol::AgentRef,
    ) -> std::result::Result<protocol::AgentState, ControlResponse> {
        let runtime = self
            .request_agent(to)
            .map_err(|reason| ControlResponse::error_with(ControlErrorCode::AgentGone, reason))?;
        let blocked = || {
            ControlResponse::error_with(
                ControlErrorCode::AgentBlocked,
                "the agent shows a dialog; refusing to type a request into it",
            )
        };
        if runtime.state == protocol::AgentState::Blocked {
            return Err(blocked());
        }
        if self.refuse_blocked_agent_screen(to.pane.pane_id).is_err() {
            return Err(blocked());
        }
        // A launching agent's work is its loading, which can drop what is typed into it.
        let loading = runtime.state == protocol::AgentState::Working
            && to.slot.is_none()
            && self
                .panes
                .get(&to.pane.pane_id)
                .is_some_and(|pane| pane.agent.launch.is_some());
        if loading {
            return Err(conflict(
                "the agent is still starting; ask without --now to queue the request",
            ));
        }
        if self.agent_requests.someone_typed(to.pane.pane_id) {
            return Err(conflict(
                "someone is typing into the agent's pane; ask again once they stop, or without --now to queue it",
            ));
        }
        if let Some(reason) = self.typing_blocker(to) {
            return Err(conflict(reason));
        }
        if self
            .agent_requests
            .slots
            .get(to)
            .is_some_and(|slot| slot.riders.len() >= MAX_RIDERS)
        {
            return Err(conflict(format!(
                "{} requests were already typed into the agent while it answers request `{}`; ask without --now to queue it",
                MAX_RIDERS + 1,
                self.agent_requests.answering(to).unwrap_or_default()
            )));
        }
        Ok(runtime.state)
    }

    pub(super) fn reply_to_request(
        &mut self,
        caller: Option<protocol::AgentRef>,
        id: &str,
        text: String,
    ) -> ControlResponse {
        self.refresh_deferred_request(id);
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
    pub(super) fn agent_requests_message(&self, include_messages: bool) -> protocol::ServerMessage {
        protocol::ServerMessage::AgentRequestsChanged {
            requests: self
                .agent_requests
                .sent
                .iter()
                .filter(|request| include_messages || !request.no_reply)
                .cloned()
                .collect(),
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
        store.sent = requests;
        let all = self.agent_requests_message(true);
        if !self
            .agent_requests
            .sent
            .iter()
            .any(|request| request.no_reply)
        {
            self.broadcast_control(&all);
            return;
        }
        let originals = self.agent_requests_message(false);
        let clients: Vec<_> = self
            .clients
            .iter()
            .filter(|client| client.attached)
            .map(|client| {
                (
                    client.id,
                    client
                        .capabilities
                        .supports(protocol::Capability::AgentAskNoReply),
                )
            })
            .collect();
        for (id, supports_messages) in clients {
            let message = if supports_messages { &all } else { &originals };
            self.enqueue(id, Target::Client(id), message.clone());
        }
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
        if let Some(slot) = self.agent_requests.slots.get(&to)
            && (slot.riders.iter().any(|rider| rider == id)
                || (slot.request == id && !slot.riders.is_empty()))
        {
            // The run goes on for what was typed in with it. The request keeps its place, so the
            // answer its turn publishes is not taken for another's; the run's end skips it.
            turn_session = slot
                .tracker
                .fence()
                .and_then(|fence| fence.native_session.clone());
        } else if self.agent_requests.answering(&to) == Some(id) {
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
            Some(crate::control::REPLY_LATER_REASON.into()),
        );
        let deferred = stored.info.clone();
        self.pump_agent_requests();
        ControlResponse::ok(deferred)
    }

    /// The native conversation the agent `reference` names is in now, as far as a request trusts
    /// it: what every reply to a request bound to a conversation must match. None when that is
    /// unknown or contradicted.
    fn answering_session(&self, reference: &protocol::AgentRef) -> Option<String> {
        match self.answering(reference) {
            Answering::Known(session) => Some(session),
            Answering::Unknown | Answering::Contradicted => None,
        }
    }

    /// A row says which conversation it shows, unless the agent's own report on the pane names
    /// another for the row on screen. A row that lingered across its agent's exit or relaunch says
    /// nothing until that report confirms it.
    fn answering(&self, reference: &protocol::AgentRef) -> Answering {
        let current = self.response_native_session(reference);
        let known = |session: Option<String>| session.map_or(Answering::Unknown, Answering::Known);
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return Answering::Unknown;
        };
        let Some(row) = reference
            .slot
            .as_ref()
            .and_then(|slot| pane.runtime.rows.iter().find(|row| row.has_key(slot)))
        else {
            return known(current);
        };
        let agent = pane
            .runtime
            .detected_agent
            .as_ref()
            .map(|detected| detected.agent.id.as_str());
        let claim = pane.runtime.integration.as_deref().filter(|claim| {
            row.active
                && claim.native_session.is_some()
                && Some(claim.identity.id.as_str()) == agent
        });
        if let Some(claim) = claim {
            return match protocol::row_reporting_session(row) {
                // A row that names no conversation cannot be held to one.
                None => Answering::Unknown,
                Some(session) if claim.native_session.as_deref() == Some(session) => known(current),
                Some(_) => Answering::Contradicted,
            };
        }
        if pane
            .agent
            .row_resume
            .is_stale(&row.publisher, &pane.agent.publisher_epochs)
        {
            return Answering::Unknown;
        }
        known(current)
    }

    /// Rebind a pending reply to a verified resume in its original pane.
    fn resumed_request_target(
        &self,
        stored: &StoredRequest,
        native_session: &str,
    ) -> std::result::Result<Option<protocol::AgentRef>, String> {
        let target = &stored.info.to;
        let pane_id = target.pane.pane_id;
        let pane = self
            .panes
            .get(&pane_id)
            .filter(|pane| pane.exited.is_none())
            .ok_or_else(|| "the agent's pane has exited".to_string())?;
        if pane.generation != target.pane.generation {
            return Err(format!("pane {pane_id} has been replaced"));
        }
        if let Some(detected) = &pane.runtime.detected_agent
            && detected.agent.id != stored.target_agent
        {
            return Err("another agent runs in the pane now".into());
        }

        let runtimes = self.agent_runtimes_for(pane_id, pane);
        // A published slot resumes in that slot while its row lasts. Existing row handoffs update
        // the stored reference when the server has evidence that a slot moved. Once the agent
        // has been launched again, the request follows the conversation on screen instead; none
        // on screen, as while the agent lists conversations to resume, is not yet a resume.
        let listed = target.slot.as_ref().and_then(|slot| {
            runtimes
                .iter()
                .find(|runtime| runtime.reference.slot.as_ref() == Some(slot))
                .cloned()
        });
        let relaunched = pane.runtime.agent_launch != stored.launch;
        let candidate = match listed {
            Some(runtime) => Some(runtime),
            None if target.slot.is_none() || relaunched => {
                match protocol::pane_agent(runtimes, &pane.runtime.rows) {
                    protocol::PaneAgent::One(runtime) => Some(*runtime),
                    protocol::PaneAgent::Missing | protocol::PaneAgent::NoneOnScreen => None,
                }
            }
            None => None,
        };
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        // A row outlives the agent that ran it. With no agent detected under it, it stands in for
        // one under its own id, so the pane is still between agents.
        if candidate.identity.id != stored.target_agent {
            return if candidate.reference.slot.is_some() && pane.runtime.detected_agent.is_none() {
                Ok(None)
            } else {
                Err("another agent runs in the pane now".into())
            };
        }
        // The launch that owes the request can show another conversation and come back to it, and
        // its publisher and its own report can disagree while it switches. Only a later launch
        // in another conversation has left it for good.
        match self.answering(&candidate.reference) {
            Answering::Known(current) if current == native_session => Ok(Some(candidate.reference)),
            Answering::Known(_) | Answering::Contradicted if relaunched => {
                Err("the agent moved to another conversation".into())
            }
            Answering::Known(_) | Answering::Contradicted | Answering::Unknown => Ok(None),
        }
    }

    /// Keep delivered requests open for an explicit reply if the pane can still resume them.
    fn suspend_request_slot(&mut self, target: &protocol::AgentRef) -> bool {
        if self
            .panes
            .get(&target.pane.pane_id)
            .is_none_or(|pane| pane.exited.is_some() || pane.generation != target.pane.generation)
        {
            return false;
        }
        let slot = &self.agent_requests.slots[target];
        let ids: Vec<_> = std::iter::once(&slot.request)
            .chain(&slot.riders)
            .filter(|id| self.settles_in_run(id))
            .cloned()
            .collect();
        let Some(session) = slot
            .tracker
            .fence()
            .and_then(|fence| fence.native_session.clone())
            .or_else(|| {
                ids.iter().find_map(|id| {
                    self.agent_requests
                        .get(id)
                        .and_then(|stored| stored.resumable_session.clone())
                })
            })
        else {
            return false;
        };
        if ids.is_empty() {
            return false;
        }
        for id in ids {
            let stored = self
                .agent_requests
                .requests
                .iter_mut()
                .find(|stored| stored.info.id == id)
                .expect("active request");
            stored.later = true;
            stored.info.to_native_session = Some(session.clone());
            settle(
                &mut stored.info,
                AgentRequestState::Active,
                Some(WAITING_FOR_RESUME.into()),
            );
            self.agent_requests.revision += 1;
        }
        self.agent_requests.slots.remove(target);
        if let Some(pane) = self.panes.get_mut(&target.pane.pane_id) {
            pane.agent.end_request_run(target);
        }
        true
    }

    fn check_deferred_requests(&mut self) {
        let deferred: Vec<_> = self
            .agent_requests
            .requests
            .iter()
            .filter(|stored| stored.later && stored.info.state == AgentRequestState::Active)
            .map(|stored| stored.info.id.clone())
            .collect();
        for id in deferred {
            self.refresh_deferred_request(&id);
        }
    }

    fn refresh_deferred_request(&mut self, id: &str) {
        let Some(stored) = self
            .agent_requests
            .get(id)
            .filter(|stored| stored.later && stored.info.state == AgentRequestState::Active)
        else {
            return;
        };
        let info = &stored.info;
        let waiting = info.reason.as_deref() == Some(WAITING_FOR_RESUME);
        let route = self.deferred_reply_route(stored);
        match route {
            Ok(Some((target, native_session))) => {
                let unchanged =
                    info.to == target && info.to_native_session == native_session && !waiting;
                // The launch it is answered from now, even one that resumed it unobserved.
                let launch = self.panes[&target.pane.pane_id].runtime.agent_launch;
                self.agent_requests.set_launch(id, launch);
                if unchanged {
                    return;
                }
                let info = self.agent_requests.info_mut(id).expect("listed above");
                info.to = target;
                info.to_native_session = native_session;
                settle(
                    info,
                    AgentRequestState::Active,
                    Some(crate::control::REPLY_LATER_REASON.into()),
                );
            }
            Ok(None) if !waiting => {
                let info = self.agent_requests.info_mut(id).expect("listed above");
                settle(
                    info,
                    AgentRequestState::Active,
                    Some(WAITING_FOR_RESUME.into()),
                );
            }
            Ok(None) => {}
            Err(reason) => {
                let info = self.agent_requests.info_mut(id).expect("listed above");
                settle(info, AgentRequestState::Failed, Some(reason));
            }
        }
    }

    /// The route for a deferred reply. An open pane can be between agent processes.
    fn deferred_reply_route(
        &self,
        stored: &StoredRequest,
    ) -> std::result::Result<Option<(protocol::AgentRef, Option<String>)>, String> {
        let target = &stored.info.to;
        let agent = self.request_agent(target);
        let current = self.answering_session(target);
        // An agent that exits leaves the conversation it was last bound to, even one the request
        // no longer insisted on: only that one can answer it once the agent resumes.
        let resumable = agent
            .is_err()
            .then_some(stored.resumable_session.as_deref())
            .flatten();
        let Some(expected) = stored.info.to_native_session.as_deref().or(resumable) else {
            return agent.map(|agent| Some((agent.reference, current)));
        };
        if let Ok(agent) = agent
            && current.as_deref() == Some(expected)
        {
            return Ok(Some((agent.reference, current)));
        }
        Ok(self
            .resumed_request_target(stored, expected)?
            .map(|target| (target, Some(expected.to_string()))))
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
            && self.answering_session(&request.to) != request.to_native_session
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
            AgentRequestState::Answered
            | AgentRequestState::Delivered
            | AgentRequestState::Failed
            | AgentRequestState::Unknown => {
                return conflict(format!(
                    "request `{id}` is already {}",
                    state_name(request.state)
                ));
            }
            AgentRequestState::Queued | AgentRequestState::Active => {}
        }
        let reason = if request.delivered_at_unix_ms.is_some() {
            "cancelled by the requester after it was typed into the agent, which may still act on it"
        } else {
            "cancelled by the requester"
        };
        let info = self.agent_requests.info_mut(id).expect("looked up above");
        settle(info, AgentRequestState::Cancelled, Some(reason.into()));
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
            let id = &inbound.id;
            let mut response = conflict(format!(
                "request `{id}` is yours to answer and `{}` waits on it; answer it with `rozi agents reply {id} --format text --stdin` instead of waiting",
                pending.id
            ));
            response.data = Some(serde_json::json!({
                "incoming": {
                    "id": id,
                    "from": inbound.from,
                    "from_label": inbound.from_label,
                    "prompt": inbound.prompt,
                },
            }));
            return Some(response);
        }
        if request.state == AgentRequestState::Queued
            && let Ok(target) = self.request_agent(&request.to)
            && let Some(blocker) = target.startup_blocker()
        {
            return Some(ControlResponse::error_with(
                ControlErrorCode::AgentBlocked,
                format!(
                    "request `{}` is queued: the agent in pane {} is blocked ({}); await the request again once its prompt is answered",
                    pending.id,
                    request.to.pane.pane_id,
                    blocker.reason()
                ),
            ));
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

    /// Send the held Enters that are due, then advance every request: settle the turns of delivered
    /// ones, deliver what is ready, and answer the awaits that can be answered.
    pub(super) fn pump_agent_requests(&mut self) {
        self.send_due_submits();
        if self.agent_requests.has_work() {
            self.watch_request_screens();
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

    /// Note what the pane of every agent with a request to take or answer shows now.
    fn watch_request_screens(&mut self) {
        let panes: HashSet<PaneId> = self
            .agent_requests
            .requests
            .iter()
            .filter(|stored| stored.info.state == AgentRequestState::Queued)
            .map(|stored| stored.info.to.pane.pane_id)
            .chain(
                self.agent_requests
                    .slots
                    .keys()
                    .map(|agent| agent.pane.pane_id),
            )
            .collect();
        for pane in panes {
            if let Some(generation) = self.panes.get(&pane).map(|pane| pane.content_generation) {
                self.agent_requests.watch_screen(pane, generation);
            }
        }
    }

    fn settle_request_turn(&mut self, target: &protocol::AgentRef) {
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        // The history read settles it when it finishes.
        if self.reading_request_history(&slot.request) {
            return;
        }
        let runtime = match self.request_agent(target) {
            Ok(runtime) => runtime,
            Err(reason) => {
                if !self.suspend_request_slot(target) {
                    self.finish_request_turn(target, TurnOutcome::Failed(reason));
                }
                return;
            }
        };
        let answering = self.answering(target);
        let switching =
            matches!(answering, Answering::Contradicted) && !self.relaunched_since_delivery(target);
        let window = self.agent_requests.switch_window;
        let slot = slot_mut(&mut self.agent_requests, target);
        if !switching {
            slot.contradicted_since = None;
        } else if slot
            .contradicted_since
            .get_or_insert_with(Instant::now)
            .elapsed()
            < window
        {
            // The launch that took the request is switching conversations: its row and its own
            // report disagree until both have moved, and the switch may yet come back.
            return;
        } else {
            return self.finish_request_turn(target, TurnOutcome::Failed(UNSETTLED_SWITCH.into()));
        }
        let native_session = match answering {
            Answering::Known(session) => Some(session),
            Answering::Unknown | Answering::Contradicted => None,
        };
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        if slot.tracker.native_session_changed(&native_session) {
            if native_session.is_none() && self.suspend_request_slot(target) {
                return;
            }
            return self.finish_request_turn(
                target,
                TurnOutcome::Failed("the agent moved to another conversation".into()),
            );
        }
        self.collect_run_answers(target);
        let slot = self.agent_requests.slots.get(target).expect("listed slot");
        if !slot.mid_turn
            && let Some(response) = slot.answers.first()
        {
            let response = response.clone();
            return self.finish_request_turn(target, TurnOutcome::Answered(response));
        }
        let stopped = runtime.state.is_stopped();
        if !self.request_turn_ready(target, runtime.state) {
            return;
        }
        let rested = self.agent_rested(target);
        let undetected_turn = self.agent_requests.undetected_turn;
        let slot = slot_mut(&mut self.agent_requests, target);
        slot.tracker.adopt_native_session(native_session);
        slot.departed |= !stopped;
        let elapsed = slot.delivered.elapsed();
        let turn_ended = slot.tracker.turn_ended(stopped, Some(runtime.state));
        if slot.mid_turn {
            // Typed in while the agent was busy, the request may have been taken in that turn or
            // in one straight after it: only an agent that has come to rest has had its say.
            if !rested || !(turn_ended || slot.departed || elapsed >= undetected_turn) {
                return;
            }
            if !slot.answers.is_empty() {
                return self.finish_request_turn(target, TurnOutcome::Published);
            }
            return self.answer_request_from_screen(target);
        }
        let unconfirmed = stopped && !slot.departed && !slot.shown && !turn_ended;
        let undetected = stopped && !slot.departed && elapsed >= undetected_turn;
        let id = slot.request.clone();
        // Neither a turn, a reply, nor the screen has shown that the agent took the request.
        if unconfirmed
            && (undetected || elapsed >= self.agent_requests.delivery_check)
            && self.settles_in_run(&id)
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
        // An agent can report the end of its turn while it is still writing what it answers.
        if (turn_ended || undetected) && rested {
            self.answer_request_from_screen(target);
        }
    }

    /// Whether `target`'s pane has launched its agent again since the request its turn settles was
    /// delivered.
    fn relaunched_since_delivery(&self, target: &protocol::AgentRef) -> bool {
        let slot = &self.agent_requests.slots[target];
        let launch = self
            .agent_requests
            .get(&slot.request)
            .map(|stored| stored.launch);
        self.panes
            .get(&target.pane.pane_id)
            .is_none_or(|pane| Some(pane.runtime.agent_launch) != launch)
    }

    /// Keep what `target`'s integration published since the last look, so a run that takes several
    /// turns keeps each turn's answer even once the response journal has dropped it.
    fn collect_run_answers(&mut self, target: &protocol::AgentRef) {
        let slot = slot_mut(&mut self.agent_requests, target);
        let Some(fence) = slot.tracker.fence() else {
            return;
        };
        while let Some(response) =
            self.agent_responses
                .after_seen(target, fence, slot.answers.last())
        {
            slot.answers.push(response.clone());
        }
        // Each request takes one answer, counted from the last.
        let keep = MAX_RIDERS + 1;
        if slot.answers.len() > keep {
            slot.answers.drain(..slot.answers.len() - keep);
        }
    }

    /// Whether `target` has been stopped for a quiet window with its screen still, so a turn that
    /// seems to have ended has not merely paused or is still being written out.
    fn agent_rested(&self, target: &protocol::AgentRef) -> bool {
        let Some(&since) = self.agent_requests.stopped_since.get(target) else {
            return false;
        };
        let quiet = self.agent_requests.quiet;
        let screen_still = self.agent_requests.screen_still(target.pane.pane_id, since);
        let slot = self.agent_requests.slots.get(target);
        let stopped_long_enough =
            !slot.is_some_and(|slot| slot.mid_turn) || since.elapsed() >= quiet;
        screen_still && stopped_long_enough
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

    /// Whether request `id` is owed by the run it was typed in: it is unanswered, and its agent
    /// has not promised a later reply. Only such a request needs proof its agent took it, since a
    /// promise could only follow reading it, and only it is settled by the run's end.
    fn settles_in_run(&self, id: &str) -> bool {
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
            .session_prompt_input(target.pane.pane_id, &frame)
            .and_then(|input| self.write_prompt(target.pane.pane_id, input, false));
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
        let open = std::iter::once(&slot.request)
            .chain(&slot.riders)
            .any(|id| self.settles_in_run(id));
        let Some(fence) = slot.tracker.fence().cloned().filter(|_| open) else {
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
        let published = matches!(outcome, TurnOutcome::Published);
        let (state, reason, response) = match outcome {
            TurnOutcome::Failed(reason) => (AgentRequestState::Failed, Some(reason), None),
            TurnOutcome::Answered(response) => (AgentRequestState::Answered, None, Some(response)),
            TurnOutcome::Published => (AgentRequestState::Answered, None, None),
            TurnOutcome::Unanswered => (
                AgentRequestState::Failed,
                Some("the turn ended with nothing on screen".into()),
                None,
            ),
        };
        // What was typed in during the run is answered by its end as much as the first request.
        // The integration answers each turn: the agent takes what was typed in order, so the last
        // answer is the last request's, and requests it folded into one turn share that turn's.
        let count = 1 + slot.riders.len();
        for (index, id) in std::iter::once(slot.request).chain(slot.riders).enumerate() {
            if !self.settles_in_run(&id) {
                continue;
            }
            let mut response = if published {
                let answer = (slot.answers.len() + index).saturating_sub(count);
                slot.answers.get(answer).cloned()
            } else {
                response.clone()
            };
            if let Some(response) = response.as_mut() {
                response.response_id = self.agent_responses.next_id();
            }
            let info = self.agent_requests.info_mut(&id).expect("checked above");
            info.response = response;
            settle(info, state, reason.clone());
        }
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
        if info.no_reply {
            return None;
        }
        let from = info.from.as_ref()?;
        if let Err(reason) = self.request_agent(from) {
            return Some(format!("the asking agent is gone: {reason}"));
        }
        let native_session = self.answering_session(from);
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
        self.current_agent_runtime(agent).map_err(|response| {
            let exited = response.code == Some(ControlErrorCode::AgentReplaced)
                && self.panes.get(&agent.pane.pane_id).is_some_and(|pane| {
                    pane.runtime.detected_agent.is_none() && pane.runtime.integration.is_none()
                });
            if exited {
                "the agent exited".into()
            } else {
                error_text(response)
            }
        })
    }

    fn request_readiness(&mut self, id: &str) -> Readiness {
        let request = &self.agent_requests.get(id).expect("queued request").info;
        let to = request.to.clone();
        let recorded_session = request.to_native_session.clone();
        let runtime = match self.request_agent(&to) {
            Ok(runtime) => runtime,
            Err(reason) => return Readiness::Fail(reason),
        };
        let answering = self.answering(&to);
        let launch = self.agent_requests.get(id).map(|stored| stored.launch);
        // The launch it was asked of may be switching back to the conversation it is bound to.
        if matches!(answering, Answering::Contradicted)
            && recorded_session.is_some()
            && Some(self.panes[&to.pane.pane_id].runtime.agent_launch) == launch
        {
            return Readiness::Wait("the agent is switching conversations".into());
        }
        let native_session = match answering {
            Answering::Known(session) => Some(session),
            Answering::Unknown | Answering::Contradicted => None,
        };
        if recorded_session.is_some() && native_session != recorded_session {
            return Readiness::Fail("the agent moved to another conversation".into());
        }
        if let Some(busy) = self.agent_requests.answering(&to) {
            let reason = match self
                .agent_requests
                .get(busy)
                .map(|stored| stored.info.state)
            {
                Some(AgentRequestState::Answered) => {
                    format!("the agent answered request `{busy}` and is finishing its turn")
                }
                Some(AgentRequestState::Active) => {
                    format!("waiting for the agent to finish request `{busy}`")
                }
                _ => format!("waiting for the agent to end its turn on request `{busy}`"),
            };
            if !runtime.state.is_stopped() {
                self.agent_requests.observe(&to, false);
            }
            return Readiness::Wait(reason);
        }
        if !runtime.state.is_stopped() {
            self.agent_requests.observe(&to, false);
            return Readiness::Wait(format!("the agent is {}", runtime.state.as_str()));
        }
        if let Some(reason) = self.typing_blocker(&to) {
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
        let screen_still = self
            .agent_requests
            .screen_still(to.pane.pane_id, stopped_since);
        if !self
            .agent_requests
            .quiet_since(to.pane.pane_id, stopped_since)
        {
            return Readiness::Wait("waiting for the agent's pane to settle".into());
        }
        // An agent can say it stopped while it is still writing out its last answer.
        if !screen_still {
            return Readiness::Wait("the agent's screen is still changing".into());
        }
        if recorded_session.is_none() {
            let info = self.agent_requests.info_mut(id).expect("queued request");
            info.to_native_session = native_session;
        }
        Readiness::Ready(runtime.state)
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

    /// Type request `id` into its target, armed to recognize the end of the turn it starts. An
    /// agent already answering another request takes it in that run; one that is busy takes it
    /// as a follow-up.
    fn deliver_request(
        &mut self,
        id: &str,
        state: protocol::AgentState,
    ) -> std::result::Result<(), String> {
        let request = &self.agent_requests.get(id).expect("queued request").info;
        let to = request.to.clone();
        let no_reply = request.no_reply;
        let frame = self.request_frame(request);
        let input = self
            .session_prompt_input(to.pane.pane_id, &frame)
            .map_err(error_text)?;
        // Only a conversation the request trusts filters the answers: behind a stale or
        // contradicted row, any answer is the new agent's.
        let fence = self.agent_responses.fence(self.answering_session(&to));
        let before = self
            .recent_pane_text(to.pane.pane_id)
            .map(|text| fingerprint(&text))
            .unwrap_or_default();
        let busy = !state.is_stopped();
        let rider = self.agent_requests.slots.contains_key(&to);
        // The delivery may end without the agent ever leaving its stopped state, so the next
        // request waits out a fresh quiet window instead of inheriting the one before it.
        self.agent_requests.stopped_since.remove(&to);
        if !rider {
            self.agent_requests.slots.insert(
                to.clone(),
                DeliverySlot {
                    request: id.to_owned(),
                    riders: Vec::new(),
                    mid_turn: busy,
                    answers: Vec::new(),
                    // A busy agent's turn ends the first time it stops.
                    tracker: AgentTurnTracker::new(!busy, Some(state), false, Some(fence)),
                    delivered: Instant::now(),
                    departed: busy,
                    waiting_after_screen_block: false,
                    shown: no_reply,
                    attempts: 1,
                    contradicted_since: None,
                    before,
                },
            );
        }
        if let Err(response) = self.write_prompt(to.pane.pane_id, input, false) {
            if !rider {
                self.agent_requests.slots.remove(&to);
            }
            return Err(error_text(response));
        }
        if rider {
            let slot = slot_mut(&mut self.agent_requests, &to);
            slot.riders.push(id.to_owned());
            slot.mid_turn = true;
        } else if let Some(pane) = self.panes.get_mut(&to.pane.pane_id) {
            pane.agent.begin_request_run(&to);
        }
        let reason = if busy || rider {
            DELIVERED_MID_TURN
        } else {
            DELIVERED
        };
        let launch = self.panes[&to.pane.pane_id].runtime.agent_launch;
        self.agent_requests.set_launch(id, launch);
        let info = self.agent_requests.info_mut(id).expect("queued request");
        if info.no_reply {
            settle(info, AgentRequestState::Delivered, None);
        } else {
            settle(info, AgentRequestState::Active, Some(reason.into()));
        }
        info.delivered_at_unix_ms = Some(info.updated_at_unix_ms);
        let label = info
            .prompt
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| {
                line.chars()
                    .filter(|ch| !ch.is_control())
                    .take(120)
                    .collect::<String>()
            });
        if let Some(label) = label.filter(|label| !label.is_empty()) {
            let task = protocol::AgentTask {
                label,
                delivered_at_unix_ms: info.updated_at_unix_ms,
            };
            self.record_request_task(&to, task);
        }
        Ok(())
    }

    fn record_request_task(&mut self, target: &protocol::AgentRef, task: protocol::AgentTask) {
        let pane_id = target.pane.pane_id;
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if !pane.agent.record_request_task(target, task) {
            return;
        }
        pane.runtime.sequence = pane.runtime.sequence.wrapping_add(1);
        let generation = pane.generation;
        let state = pane.runtime.clone();
        let message = self.finish_pane_runtime_change(None, pane_id, generation, state, None);
        self.broadcast_control(&message);
    }

    fn request_frame(&self, request: &AgentRequestInfo) -> String {
        let asker = match &request.from {
            Some(from) => {
                // A one-way message stays queued after its sender exits. The label was recorded
                // when it was asked, and the reference still names that pane.
                let (label, pane) = self
                    .current_agent_runtime(from)
                    .ok()
                    .map(|runtime| (runtime.label, runtime.reference.pane.pane_id))
                    .unwrap_or_else(|| {
                        (
                            request.from_label.clone().unwrap_or_default(),
                            from.pane.pane_id,
                        )
                    });
                if label.is_empty() {
                    format!(" from pane {pane}")
                } else {
                    format!(" from {label} in pane {pane}")
                }
            }
            None => request
                .from_label
                .as_ref()
                .map(|label| format!(" from \"{label}\", outside this session"))
                .unwrap_or_default(),
        };
        if request.no_reply {
            return format!("Message{asker} (no reply needed)\n\n{}", request.prompt);
        }
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

fn slot_mut<'a>(
    store: &'a mut AgentRequestStore,
    target: &protocol::AgentRef,
) -> &'a mut DeliverySlot {
    store.slots.get_mut(target).expect("listed slot")
}

fn state_name(state: AgentRequestState) -> &'static str {
    match state {
        AgentRequestState::Queued => "queued",
        AgentRequestState::Active => "active",
        AgentRequestState::Answered => "answered",
        AgentRequestState::Delivered => "delivered",
        AgentRequestState::Unknown => "unknown",
        AgentRequestState::Failed => "failed",
        AgentRequestState::Cancelled => "cancelled",
    }
}

#[cfg(all(test, unix))]
mod tests;
