use super::headless::ReplyTo;
use super::*;
use crate::control::{
    AgentTarget, AgentWaitCondition, AgentWaitResult, ControlErrorCode, ControlResponse,
};

/// Which held wait an answer belongs to: the client it goes to, and the attached request it
/// answers. A headless connection holds at most one wait, so its request id is always 0; an
/// attached UI can forward several at once on one connection.
pub(super) type WaitKey = (ClientId, u64);

pub(super) struct PendingAgentWait {
    subject: WaitSubject,
    until: AgentWaitCondition,
    deadline: Option<Instant>,
    /// Present on a wait registered after a prompt: the wait answers only for that prompt's turn.
    turn: Option<AgentTurnTracker>,
    reply: ReplyTo,
    capture: Option<crate::control::CaptureRender>,
}

/// Whether the turn a prompt started has reached an outcome, for one exact agent.
///
/// The consumer decides which states count as an outcome and reports each observation. An outcome
/// already showing when the prompt was delivered belongs to the previous turn, so it counts only
/// once the agent has left it.
pub(super) struct AgentTurnTracker {
    initial_state: Option<protocol::AgentState>,
    armed: bool,
    state_change_ends_turn: bool,
    fence: Option<super::responses::AgentResponseFence>,
}

impl AgentTurnTracker {
    /// Start tracking at delivery. `reached` and `state` are the consumer's outcome and the agent's
    /// state at that moment. With `state_change_ends_turn`, moving from that state straight to
    /// another outcome ends the turn; otherwise the agent must first leave the outcome entirely.
    /// `fence` marks the responses published before the prompt.
    pub(super) fn new(
        reached: bool,
        state: Option<protocol::AgentState>,
        state_change_ends_turn: bool,
        fence: Option<super::responses::AgentResponseFence>,
    ) -> Self {
        Self {
            initial_state: state.filter(|_| reached),
            armed: !reached,
            state_change_ends_turn,
            fence,
        }
    }

    /// Feed one observation of the agent, and learn whether it ends the prompted turn.
    pub(super) fn turn_ended(
        &mut self,
        reached: bool,
        state: Option<protocol::AgentState>,
    ) -> bool {
        if self.armed {
            return reached;
        }
        if !reached {
            self.armed = true;
            return false;
        }
        // A turn can go directly from idle to an approval or error. It need not leave the
        // entire outcome predicate, which would require observing working first.
        self.state_change_ends_turn && state.is_some_and(|state| Some(state) != self.initial_state)
    }

    pub(super) fn fence(&self) -> Option<&super::responses::AgentResponseFence> {
        self.fence.as_ref()
    }

    pub(super) fn into_fence(self) -> Option<super::responses::AgentResponseFence> {
        self.fence
    }

    /// Bind an unbound fence to the native session the agent reports, once it reports one.
    pub(super) fn adopt_native_session(&mut self, native_session: Option<String>) {
        if let Some(fence) = self.fence.as_mut()
            && fence.native_session.is_none()
        {
            fence.native_session = native_session;
        }
    }

    /// Whether the agent has moved to a native session other than the one the fence is bound to.
    pub(super) fn native_session_changed(&self, native_session: &Option<String>) -> bool {
        self.fence.as_ref().is_some_and(|fence| {
            fence.native_session.is_some() && *native_session != fence.native_session
        })
    }

    /// The first response `reference` published after the fence, if any.
    pub(super) fn fresh_response<'a>(
        &self,
        responses: &'a super::responses::AgentResponseStore,
        reference: &protocol::AgentRef,
    ) -> Option<&'a crate::control::AgentResponse> {
        responses.after(reference, self.fence.as_ref()?)
    }
}

/// What a wait is watching.
#[derive(Clone)]
enum WaitSubject {
    /// One exact agent incarnation.
    Agent(protocol::AgentRef),
    /// A running pane whose agent has not been detected yet, as straight after `split`. The wait
    /// adopts the first agent that appears there; the pane exiting or being replaced ends it.
    Pane { pane_id: PaneId, generation: u64 },
}

impl WaitSubject {
    fn agent_reference(&self) -> Option<&protocol::AgentRef> {
        match self {
            Self::Agent(reference) => Some(reference),
            Self::Pane { .. } => None,
        }
    }
}

enum WaitEvaluation {
    Pending,
    Ready(Option<Box<protocol::AgentRuntime>>),
    Error(ControlErrorCode, String),
}

impl SessionServer {
    pub(super) fn handoff_agent_waits(
        &mut self,
        old: &protocol::AgentRef,
        new: &protocol::AgentRef,
    ) {
        for wait in self.agent_waits.values_mut() {
            if matches!(&wait.subject, WaitSubject::Agent(reference) if reference == old) {
                wait.subject = WaitSubject::Agent(new.clone());
            }
        }
    }

    /// Whether another prompt is in flight for `reference`: a history read on its pane, a prompt
    /// wait, or, for a `result` prompt, a request it is answering.
    pub(super) fn has_conflicting_prompt(
        &self,
        reference: &protocol::AgentRef,
        result: bool,
    ) -> bool {
        self.agent_history.contains_key(&reference.pane.pane_id)
            || (result && self.agent_requests.answering(reference).is_some())
            || self.agent_waits.values().any(|wait| {
                wait.turn
                    .as_ref()
                    .is_some_and(|turn| result || turn.fence().is_some())
                    && matches!(&wait.subject, WaitSubject::Agent(current) if current == reference)
            })
    }

    pub(super) fn register_agent_wait(
        &mut self,
        client_id: ClientId,
        target: AgentTarget,
        until: AgentWaitCondition,
        timeout_ms: Option<u64>,
        reply: ReplyTo,
    ) -> Option<ControlResponse> {
        if until == AgentWaitCondition::Turn {
            return Some(ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "turn is a post-prompt condition; use agent-prompt with wait turn",
            ));
        }
        let subject = match target {
            AgentTarget::Ref(reference) if until == AgentWaitCondition::Gone => {
                if reference.pane.session_instance != self.instance_id {
                    return Some(ControlResponse::error_with(
                        ControlErrorCode::StaleReference,
                        "agent reference belongs to a different session server instance",
                    ));
                }
                WaitSubject::Agent(reference)
            }
            AgentTarget::Pane(pane_id) if self.pane_awaits_agent(pane_id) => {
                if until == AgentWaitCondition::Gone {
                    return Some(ControlResponse::ok(AgentWaitResult {
                        response: None,
                        reference: None,
                        condition: until,
                        result: None,
                        agent: None,
                    }));
                }
                WaitSubject::Pane {
                    pane_id,
                    generation: self.panes[&pane_id].generation,
                }
            }
            target => match self.resolve_agent_wait_target(target) {
                Ok(reference) => WaitSubject::Agent(reference),
                Err(response) => return Some(response),
            },
        };
        let (subject, evaluation) = self.evaluate_wait_subject(subject, until);
        match evaluation {
            WaitEvaluation::Ready(agent) => Some(ControlResponse::ok(AgentWaitResult {
                response: None,
                reference: match subject {
                    WaitSubject::Agent(reference) => Some(reference),
                    WaitSubject::Pane { .. } => None,
                },
                condition: until,
                result: None,
                agent: agent.map(|agent| *agent),
            })),
            WaitEvaluation::Error(code, message) => {
                Some(ControlResponse::error_with(code, message))
            }
            WaitEvaluation::Pending => {
                self.agent_waits.insert(
                    (client_id, reply.hold_id()),
                    PendingAgentWait {
                        subject,
                        until,
                        deadline: timeout_ms
                            .map(Duration::from_millis)
                            .and_then(|duration| Instant::now().checked_add(duration)),
                        capture: None,
                        turn: None,
                        reply,
                    },
                );
                None
            }
        }
    }

    /// Whether `pane_id` is a running pane with no agent detected in it yet.
    fn pane_awaits_agent(&self, pane_id: PaneId) -> bool {
        self.panes.get(&pane_id).is_some_and(|pane| {
            pane.exited.is_none() && self.agent_runtimes_for(pane_id, pane).is_empty()
        })
    }

    /// Evaluate a wait, first adopting the agent a [`WaitSubject::Pane`] wait has been waiting for
    /// once one appears. Returns the subject to keep watching.
    fn evaluate_wait_subject(
        &self,
        subject: WaitSubject,
        until: AgentWaitCondition,
    ) -> (WaitSubject, WaitEvaluation) {
        let reference = match subject {
            WaitSubject::Agent(reference) => reference,
            WaitSubject::Pane {
                pane_id,
                generation,
            } => {
                let Some(pane) = self
                    .panes
                    .get(&pane_id)
                    .filter(|pane| pane.generation == generation && pane.exited.is_none())
                else {
                    return (
                        subject,
                        gone_evaluation(
                            until,
                            ControlErrorCode::AgentGone,
                            &format!("pane {pane_id} exited before an agent appeared in it"),
                        ),
                    );
                };
                match protocol::pane_agent(
                    self.agent_runtimes_for(pane_id, pane),
                    &pane.runtime.rows,
                ) {
                    protocol::PaneAgent::Missing => return (subject, WaitEvaluation::Pending),
                    protocol::PaneAgent::One(runtime) => runtime.reference,
                    protocol::PaneAgent::NoneOnScreen => {
                        return (
                            subject,
                            WaitEvaluation::Error(
                                ControlErrorCode::TargetRequired,
                                protocol::none_on_screen_message(pane_id),
                            ),
                        );
                    }
                }
            }
        };
        let evaluation = self.evaluate_agent_wait(&reference, until);
        (WaitSubject::Agent(reference), evaluation)
    }

    pub(super) fn resolve_agent_waits(&mut self) {
        let mut completed = Vec::new();
        let keys = self.agent_waits.keys().copied().collect::<Vec<_>>();
        for key in keys {
            let Some(wait) = self.agent_waits.get(&key) else {
                continue;
            };
            let (subject, mut evaluation) =
                self.evaluate_wait_subject(wait.subject.clone(), wait.until);
            let native_session = subject
                .agent_reference()
                .and_then(|reference| self.response_native_session(reference));
            let fresh_response = self.fresh_prompt_response(wait, &native_session, &mut evaluation);
            let wait = self.agent_waits.get_mut(&key).expect("wait read above");
            wait.subject = subject;
            if let Some(turn) = wait.turn.as_mut() {
                turn.adopt_native_session(native_session);
                if !fresh_response {
                    match &evaluation {
                        WaitEvaluation::Ready(agent) => {
                            if !turn.turn_ended(true, agent.as_ref().map(|agent| agent.state)) {
                                evaluation = WaitEvaluation::Pending;
                            }
                        }
                        WaitEvaluation::Pending => {
                            turn.turn_ended(false, None);
                        }
                        WaitEvaluation::Error(_, _) => {}
                    }
                }
            }
            if !matches!(evaluation, WaitEvaluation::Pending) {
                completed.push((key, evaluation));
            }
        }
        self.finish_agent_waits(completed);
        self.pump_agent_requests();
    }

    /// Whether the agent has answered the prompt `wait` follows. `native_session` is the one the
    /// agent reports now.
    fn fresh_prompt_response(
        &self,
        wait: &PendingAgentWait,
        native_session: &Option<String>,
        evaluation: &mut WaitEvaluation,
    ) -> bool {
        if matches!(evaluation, WaitEvaluation::Error(_, _)) {
            return false;
        }
        let (Some(reference), Some(turn)) = (wait.subject.agent_reference(), wait.turn.as_ref())
        else {
            return false;
        };
        if turn.native_session_changed(native_session) {
            *evaluation = WaitEvaluation::Error(
                ControlErrorCode::AgentReplaced,
                "agent changed its native session while waiting for a response".into(),
            );
            return false;
        }
        turn.fresh_response(&self.agent_responses, reference)
            .is_some()
    }

    pub(super) fn expire_agent_waits(&mut self) {
        let now = Instant::now();
        let completed = self
            .agent_waits
            .iter()
            .filter(|(_, wait)| wait.deadline.is_some_and(|deadline| now >= deadline))
            .map(|(&key, wait)| {
                (
                    key,
                    WaitEvaluation::Error(ControlErrorCode::Timeout, self.timeout_message(wait)),
                )
            })
            .collect();
        self.finish_agent_waits(completed);
    }

    /// Why a wait timed out, with the state the agent ended in: a wait for `idle` on an agent that
    /// finished a turn sits in `done` until someone looks, which `quiescent` would have accepted.
    fn timeout_message(&self, wait: &PendingAgentWait) -> String {
        let until = wait_condition_name(wait.until);
        let reference = match &wait.subject {
            WaitSubject::Pane { pane_id, .. } => {
                return format!(
                    "timed out waiting for an agent to appear in pane {pane_id} and become {until}"
                );
            }
            WaitSubject::Agent(reference) => reference,
        };
        let state = self.panes.get(&reference.pane.pane_id).and_then(|pane| {
            self.agent_runtimes_for(reference.pane.pane_id, pane)
                .into_iter()
                .find(|runtime| runtime.reference == *reference)
                .map(|runtime| runtime.state)
        });
        let mut message = format!(
            "timed out waiting for agent {} in pane {} to become {until}",
            reference.incarnation, reference.pane.pane_id
        );
        if let Some(state) = state {
            message.push_str(&format!("; it is {}", state.as_str()));
            if wait.until == AgentWaitCondition::Idle && state == protocol::AgentState::Done {
                message.push_str(" (`quiescent` accepts idle or done)");
            }
        }
        message
    }

    fn finish_agent_waits(&mut self, completed: Vec<(WaitKey, WaitEvaluation)>) {
        for (key, evaluation) in completed {
            let Some(wait) = self.agent_waits.remove(&key) else {
                continue;
            };
            let response = match evaluation {
                WaitEvaluation::Ready(agent) => {
                    let reference = wait.subject.agent_reference().cloned();
                    let result = match self.agent_wait_result(
                        wait.until,
                        agent,
                        wait.capture,
                        wait.turn.as_ref().and_then(AgentTurnTracker::fence),
                        reference,
                    ) {
                        Ok(result) => result,
                        Err(response) => {
                            self.answer_held(key.0, &wait.reply, response);
                            continue;
                        }
                    };
                    if let Some(fence) = wait.turn.and_then(AgentTurnTracker::into_fence) {
                        let subject = super::agent_history::history_subject(&result);
                        let pending = super::agent_history::HistoryWait {
                            consumer: super::agent_history::HistoryConsumer::Reply {
                                key,
                                reply: wait.reply,
                                result: Box::new(result),
                            },
                            deadline: wait.deadline,
                            fence,
                        };
                        self.answer_from_screen(pending, subject);
                        continue;
                    } else {
                        ControlResponse::ok(result)
                    }
                }
                WaitEvaluation::Error(code, message) => ControlResponse::error_with(code, message),
                WaitEvaluation::Pending => continue,
            };
            self.answer_held(key.0, &wait.reply, response);
        }
    }

    fn agent_wait_result(
        &mut self,
        condition: AgentWaitCondition,
        agent: Option<Box<protocol::AgentRuntime>>,
        render: Option<crate::control::CaptureRender>,
        response_fence: Option<&super::responses::AgentResponseFence>,
        reference: Option<protocol::AgentRef>,
    ) -> std::result::Result<AgentWaitResult, ControlResponse> {
        let result = if let (Some(render), Some(agent)) = (render, agent.as_ref()) {
            let id = agent.reference.pane.pane_id;
            let pane = self
                .panes
                .get_mut(&id)
                .expect("wait resolved against this pane");
            let content = crate::pane::capture_screen(
                pane.screen_without_change(),
                None,
                render,
                None,
                false,
            )?;
            Some(crate::control::AgentPromptResult::Terminal {
                capture: crate::control::PaneCapture {
                    id,
                    title: pane.screen().title(),
                    content,
                },
            })
        } else {
            None
        };
        let response = reference.as_ref().and_then(|reference| {
            self.agent_responses
                .after(reference, response_fence?)
                .cloned()
        });
        Ok(AgentWaitResult {
            response,
            reference,
            condition,
            agent: agent.map(|agent| *agent),
            result,
        })
    }

    /// Drop every wait `client_id` holds: its connection is gone.
    pub(super) fn remove_agent_waits(&mut self, client_id: ClientId) {
        self.agent_waits
            .retain(|&(client, _), _| client != client_id);
        self.agent_requests.remove_awaits(client_id);
        self.cancel_client_history(client_id);
    }

    /// Whether `client_id` still holds a wait, which keeps a headless connection open.
    pub(super) fn holds_agent_wait(&self, client_id: ClientId) -> bool {
        self.holds_agent_history(client_id)
            || self.agent_requests.holds_await(client_id)
            || self
                .agent_waits
                .keys()
                .any(|&(client, _)| client == client_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn register_post_prompt_wait(
        &mut self,
        client_id: ClientId,
        reference: protocol::AgentRef,
        until: AgentWaitCondition,
        timeout_ms: Option<u64>,
        capture: Option<crate::control::CaptureRender>,
        result: bool,
        reply: ReplyTo,
    ) -> std::result::Result<(), ControlResponse> {
        let (reached, state) = match self.evaluate_agent_wait(&reference, until) {
            WaitEvaluation::Ready(agent) => (true, agent.map(|agent| agent.state)),
            WaitEvaluation::Pending => (false, None),
            WaitEvaluation::Error(code, message) => {
                return Err(ControlResponse::error_with(code, message));
            }
        };
        let fence = result.then(|| {
            self.agent_responses
                .fence(self.response_native_session(&reference))
        });
        self.agent_waits.insert(
            (client_id, reply.hold_id()),
            PendingAgentWait {
                subject: WaitSubject::Agent(reference.clone()),
                until,
                deadline: timeout_ms
                    .map(Duration::from_millis)
                    .and_then(|duration| Instant::now().checked_add(duration)),
                capture,
                turn: Some(AgentTurnTracker::new(
                    reached,
                    state,
                    until == AgentWaitCondition::Turn,
                    fence,
                )),
                reply,
            },
        );
        Ok(())
    }

    pub(super) fn resolve_agent_wait_target(
        &self,
        target: AgentTarget,
    ) -> std::result::Result<protocol::AgentRef, ControlResponse> {
        match target {
            AgentTarget::Pane(pane_id) => {
                let Some(pane) = self.panes.get(&pane_id) else {
                    return Err(ControlResponse::error_with(
                        ControlErrorCode::PaneNotFound,
                        format!("pane {pane_id} does not exist"),
                    ));
                };
                match protocol::pane_agent(
                    self.agent_runtimes_for(pane_id, pane),
                    &pane.runtime.rows,
                ) {
                    protocol::PaneAgent::One(runtime) => Ok(runtime.reference),
                    protocol::PaneAgent::Missing => Err(ControlResponse::error_with(
                        ControlErrorCode::AgentGone,
                        format!(
                            "pane {pane_id} has no agent; `agents wait` waits for one to appear"
                        ),
                    )),
                    protocol::PaneAgent::NoneOnScreen => Err(ControlResponse::error_with(
                        ControlErrorCode::TargetRequired,
                        protocol::none_on_screen_message(pane_id),
                    )),
                }
            }
            AgentTarget::Ref(reference) => self
                .current_agent_runtime(&reference)
                .map(|runtime| runtime.reference),
        }
    }

    /// The runtime of the exact agent `reference` names, while it is still the current one.
    pub(super) fn current_agent_runtime(
        &self,
        reference: &protocol::AgentRef,
    ) -> std::result::Result<protocol::AgentRuntime, ControlResponse> {
        if reference.pane.session_instance != self.instance_id {
            return Err(ControlResponse::error_with(
                ControlErrorCode::StaleReference,
                "agent reference belongs to a different session server instance",
            ));
        }
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return Err(ControlResponse::error_with(
                ControlErrorCode::StaleReference,
                format!("pane {} no longer exists", reference.pane.pane_id),
            ));
        };
        if pane.generation != reference.pane.generation {
            return Err(ControlResponse::error_with(
                ControlErrorCode::StaleReference,
                format!("pane {} has been replaced", reference.pane.pane_id),
            ));
        }
        self.agent_runtimes_for(reference.pane.pane_id, pane)
            .into_iter()
            .find(|runtime| runtime.reference == *reference)
            .ok_or_else(|| {
                ControlResponse::error_with(
                    ControlErrorCode::AgentReplaced,
                    "agent reference no longer names the current incarnation",
                )
            })
    }

    fn evaluate_agent_wait(
        &self,
        reference: &protocol::AgentRef,
        until: AgentWaitCondition,
    ) -> WaitEvaluation {
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return gone_evaluation(until, ControlErrorCode::AgentGone, "agent pane is gone");
        };
        if pane.exited.is_some() {
            return gone_evaluation(until, ControlErrorCode::AgentGone, "agent pane has exited");
        }
        if pane.generation != reference.pane.generation {
            return gone_evaluation(
                until,
                ControlErrorCode::AgentReplaced,
                "agent pane was replaced",
            );
        }
        let runtimes = self.agent_runtimes_for(reference.pane.pane_id, pane);
        if let Some(runtime) = runtimes
            .iter()
            .find(|runtime| runtime.reference == *reference)
        {
            if condition_matches(until, runtime.state) {
                WaitEvaluation::Ready(Some(Box::new(runtime.clone())))
            } else {
                WaitEvaluation::Pending
            }
        } else {
            let replacement = runtimes
                .iter()
                .any(|runtime| runtime.reference.slot == reference.slot);
            gone_evaluation(
                until,
                if replacement {
                    ControlErrorCode::AgentReplaced
                } else {
                    ControlErrorCode::AgentGone
                },
                if replacement {
                    "agent incarnation was replaced"
                } else {
                    "agent is gone"
                },
            )
        }
    }

    pub(super) fn agent_runtimes_for(
        &self,
        pane_id: PaneId,
        pane: &ServerPane,
    ) -> Vec<protocol::AgentRuntime> {
        let pane_ref = protocol::PaneRef {
            session_instance: self.instance_id.clone(),
            pane_id,
            generation: pane.generation,
        };
        protocol::effective_agent_runtimes(&pane.runtime, &pane.agent.records(pane_ref))
    }
}

fn condition_matches(until: AgentWaitCondition, state: protocol::AgentState) -> bool {
    match until {
        AgentWaitCondition::Working => state == protocol::AgentState::Working,
        AgentWaitCondition::Blocked => state == protocol::AgentState::Blocked,
        AgentWaitCondition::Idle => state == protocol::AgentState::Idle,
        AgentWaitCondition::Done => state == protocol::AgentState::Done,
        AgentWaitCondition::Error => state == protocol::AgentState::Error,
        // Stopped for any reason: a script waiting for the end of a turn must not hang on a
        // turn that failed. The answer carries the state, so it can tell which.
        AgentWaitCondition::Quiescent => state.is_stopped(),
        AgentWaitCondition::Turn => state != protocol::AgentState::Working,
        AgentWaitCondition::Gone => false,
    }
}

fn gone_evaluation(
    until: AgentWaitCondition,
    code: ControlErrorCode,
    message: &str,
) -> WaitEvaluation {
    if until == AgentWaitCondition::Gone {
        WaitEvaluation::Ready(None)
    } else {
        WaitEvaluation::Error(code, message.to_string())
    }
}

fn wait_condition_name(condition: AgentWaitCondition) -> &'static str {
    match condition {
        AgentWaitCondition::Working => "working",
        AgentWaitCondition::Blocked => "blocked",
        AgentWaitCondition::Idle => "idle",
        AgentWaitCondition::Done => "done",
        AgentWaitCondition::Error => "error",
        AgentWaitCondition::Quiescent => "quiescent",
        AgentWaitCondition::Turn => "turn",
        AgentWaitCondition::Gone => "gone",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A script waiting for the end of a turn must not hang on a turn that failed, and can wait
    /// for a failure on its own.
    #[test]
    fn a_failed_agent_has_stopped_and_can_be_waited_for() {
        use protocol::AgentState::{Done, Error, Idle, Working};
        assert!(condition_matches(AgentWaitCondition::Quiescent, Error));
        assert!(condition_matches(AgentWaitCondition::Quiescent, Done));
        assert!(condition_matches(AgentWaitCondition::Quiescent, Idle));
        assert!(!condition_matches(AgentWaitCondition::Quiescent, Working));
        assert!(condition_matches(AgentWaitCondition::Error, Error));
        assert!(!condition_matches(AgentWaitCondition::Done, Error));
    }

    #[test]
    fn turn_includes_intervention_without_changing_quiescent() {
        use protocol::AgentState::*;
        for state in [Blocked, Done, Error, Idle] {
            assert!(condition_matches(AgentWaitCondition::Turn, state));
        }
        assert!(!condition_matches(AgentWaitCondition::Turn, Working));
        assert!(!condition_matches(AgentWaitCondition::Quiescent, Blocked));
    }

    #[test]
    fn turn_accepts_a_direct_outcome_without_observing_working() {
        for state in [
            protocol::DetectedAgentState::Blocked,
            protocol::DetectedAgentState::Idle,
        ] {
            let mut server = server_with_agent(protocol::DetectedAgentState::Idle);
            let reference = server
                .resolve_agent_wait_target(AgentTarget::Pane(3))
                .unwrap();
            server
                .register_post_prompt_wait(
                    1,
                    reference,
                    AgentWaitCondition::Turn,
                    None,
                    None,
                    false,
                    headless(),
                )
                .unwrap();
            server
                .panes
                .get_mut(&3)
                .unwrap()
                .runtime
                .detected_agent
                .as_mut()
                .unwrap()
                .state = state;
            server.resolve_agent_waits();
            assert_eq!(
                server.agent_waits.is_empty(),
                state == protocol::DetectedAgentState::Blocked
            );
        }
    }

    #[test]
    fn prompt_wait_captures_at_the_blocked_transition_for_both_transports() {
        for attached in [false, true] {
            let mut server = server_with_agent(protocol::DetectedAgentState::Idle);
            let (client, mut stream) = super::super::tests::attach_client(&mut server);
            // Set the timeout before a headless reply closes the server end. macOS rejects
            // setting it after that peer has closed, even with a reply still buffered.
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let reference = server
                .resolve_agent_wait_target(AgentTarget::Pane(3))
                .unwrap();
            server
                .register_post_prompt_wait(
                    client,
                    reference.clone(),
                    AgentWaitCondition::Turn,
                    None,
                    Some(crate::control::CaptureRender::Text),
                    false,
                    if attached {
                        ReplyTo::Attached { request_id: 42 }
                    } else {
                        headless()
                    },
                )
                .unwrap();
            server.resolve_agent_waits();
            assert_eq!(
                server.agent_waits.len(),
                1,
                "the initial idle is not this turn's outcome"
            );
            server
                .panes
                .get_mut(&3)
                .unwrap()
                .runtime
                .detected_agent
                .as_mut()
                .unwrap()
                .state = protocol::DetectedAgentState::Working;
            server.resolve_agent_waits();
            let pane = server.panes.get_mut(&3).unwrap();
            pane.screen_mut().process_bytes(b"Approve the plan?");
            pane.runtime.detected_agent.as_mut().unwrap().state =
                protocol::DetectedAgentState::Blocked;
            server.resolve_agent_waits();
            assert!(server.agent_waits.is_empty());
            // Later output cannot change the already queued transaction result.
            server
                .panes
                .get_mut(&3)
                .unwrap()
                .screen_mut()
                .process_bytes(b"later output");
            server.flush_clients();
            let message: ServerMessage = protocol::read_frame(&mut stream).unwrap();
            let response = match message {
                ServerMessage::AttachedControlResult {
                    request_id: 42,
                    response,
                } if attached => response,
                ServerMessage::SessionControlResult { response, .. } if !attached => response,
                other => panic!("unexpected reply: {other:?}"),
            };
            assert!(response.ok, "{response:?}");
            let result: AgentWaitResult = serde_json::from_value(response.data.unwrap()).unwrap();
            let agent = result.agent.unwrap();
            assert_eq!(agent.reference, reference);
            assert_eq!(agent.state, protocol::AgentState::Blocked);
            let Some(crate::control::AgentPromptResult::Terminal { capture }) = result.result
            else {
                panic!("terminal result required")
            };
            let crate::control::CaptureContent::Text { text } = capture.content else {
                panic!("text required")
            };
            assert!(text.contains("Approve the plan?"));
            assert!(!text.contains("later output"));
        }
    }

    #[test]
    fn semantic_response_is_atomic_and_fenced_for_both_transports() {
        use crate::control::{AgentResponseFidelity, AgentResponsePayload, AgentResponseSource};
        for (attached, rows, cols) in [
            (false, 5, 20),
            (true, 5, 20),
            (false, 80, 200),
            (true, 80, 200),
        ] {
            let mut server = server_with_agent(protocol::DetectedAgentState::Idle);
            server
                .panes
                .get_mut(&3)
                .unwrap()
                .screen_mut()
                .resize(rows, cols);
            let generation = server.panes[&3].generation;
            let report = |seq, text: &str| runtime::AgentIntegrationUpdate {
                agent: Some("claude".into()),
                integration: "hooks".into(),
                state: Some(protocol::AgentState::Done),
                reason: None,
                native_session: Some("native".into()),
                unless_quiet: None,
                ended: false,
                seq,
                response: Some(AgentResponsePayload {
                    text: text.into(),
                    turn_id: Some(format!("turn-{seq}")),
                    source: AgentResponseSource::NativeHook,
                    fidelity: AgentResponseFidelity::Semantic,
                    complete: true,
                    truncated: false,
                }),
            };
            server
                .apply_agent_integration(None, 3, generation, report(1, "old"))
                .unwrap();
            let reference = server
                .resolve_agent_wait_target(AgentTarget::Pane(3))
                .unwrap();
            let (client, mut stream) = super::super::tests::attach_client(&mut server);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            server
                .register_post_prompt_wait(
                    client,
                    reference.clone(),
                    AgentWaitCondition::Turn,
                    None,
                    Some(crate::control::CaptureRender::Text),
                    true,
                    if attached {
                        ReplyTo::Attached { request_id: 42 }
                    } else {
                        headless()
                    },
                )
                .unwrap();
            server.resolve_agent_waits();
            assert_eq!(
                server.agent_waits.len(),
                1,
                "old response cannot complete a new wait"
            );
            assert!(
                server
                    .apply_agent_integration(None, 3, generation, report(1, "replay"))
                    .is_err()
            );
            assert_eq!(
                server
                    .agent_responses
                    .latest(&reference, &Some("native".into()))
                    .unwrap()
                    .content
                    .text,
                "old"
            );
            assert_eq!(server.agent_waits.len(), 1);
            let text = "## Findings\n\n| Key | Value |\n|---|---|\n| 🌷 | 日本語 |\n".repeat(2000);
            server
                .apply_agent_integration(None, 3, generation, report(2, &text))
                .unwrap();
            assert!(server.agent_waits.is_empty());
            // A nonblocking flush need not drain a frame this large in one pass, so read it on
            // its own thread while the server keeps flushing, as the real loop does.
            stream.set_nonblocking(false).unwrap();
            let reader = std::thread::spawn(move || {
                protocol::read_frame::<_, ServerMessage>(&mut stream)
                    .map(|message| (message, stream))
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while !reader.is_finished() {
                assert!(Instant::now() < deadline, "response frame never drained");
                server.flush_clients();
                std::thread::sleep(Duration::from_millis(1));
            }
            let (message, mut stream) = reader.join().unwrap().unwrap();
            let response = match message {
                ServerMessage::AttachedControlResult {
                    request_id: 42,
                    response,
                } if attached => response,
                ServerMessage::SessionControlResult { response, .. } if !attached => response,
                other => panic!("unexpected response: {other:?}"),
            };
            let result: AgentWaitResult = serde_json::from_value(response.data.unwrap()).unwrap();
            assert_eq!(result.response.unwrap().content.text, text);
            assert!(
                result.result.is_some(),
                "capture and semantic response coexist"
            );
            assert!(
                server
                    .apply_agent_integration(None, 3, generation, report(2, "replacement"))
                    .is_err()
            );
            let snapshot = serde_json::to_string(&server.panes[&3].runtime).unwrap();
            assert!(
                !snapshot.contains("Findings"),
                "content must not enter runtime metadata"
            );
            assert!(
                server
                    .apply_agent_integration(
                        None,
                        3,
                        generation,
                        report(
                            3,
                            &"x".repeat(super::super::responses::MAX_RESPONSE_BYTES + 1)
                        )
                    )
                    .is_err()
            );
            assert_eq!(
                server.panes[&3].runtime.integration.as_ref().unwrap().seq,
                2
            );
            if attached {
                server
                    .register_post_prompt_wait(
                        client,
                        reference.clone(),
                        AgentWaitCondition::Turn,
                        None,
                        None,
                        true,
                        ReplyTo::Attached { request_id: 43 },
                    )
                    .unwrap();
                let mut switched = report(3, "another conversation");
                switched.native_session = Some("another-native-session".into());
                server
                    .apply_agent_integration(None, 3, generation, switched)
                    .unwrap();
                server.flush_clients();
                let message: ServerMessage = protocol::read_frame(&mut stream).unwrap();
                let ServerMessage::AttachedControlResult {
                    request_id: 43,
                    response,
                } = message
                else {
                    panic!("{message:?}")
                };
                assert_eq!(response.code, Some(ControlErrorCode::AgentReplaced));
            }
        }
    }

    fn server_with_agent(state: protocol::DetectedAgentState) -> SessionServer {
        let mut server = SessionServer::new_named("dev");
        let mut pane = super::super::tests::test_pane(7);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state,
        });
        pane.agent.sync_references(&pane.runtime);
        server.panes.insert(3, pane);
        server
    }

    #[test]
    fn registration_checks_current_state_before_waiting() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Idle);
        let response = server
            .register_agent_wait(
                1,
                AgentTarget::Pane(3),
                AgentWaitCondition::Idle,
                None,
                ReplyTo::Headless {
                    capabilities: protocol::Capabilities::default(),
                    effective_protocol: PROTOCOL_VERSION,
                },
            )
            .expect("idle agent should resolve immediately");

        assert!(response.ok);
        assert!(server.agent_waits.is_empty());
    }

    #[test]
    fn an_exact_wait_rejects_a_replacement_incarnation() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Working);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        let pane = server.panes.get_mut(&3).unwrap();
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("codex", "Codex").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        pane.agent.sync_references(&pane.runtime);

        assert!(matches!(
            server.evaluate_agent_wait(&reference, AgentWaitCondition::Idle),
            WaitEvaluation::Error(ControlErrorCode::AgentReplaced, _)
        ));
    }

    #[test]
    fn an_exact_reference_rejects_a_new_launch_of_the_same_agent() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Working);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        let pane = server.panes.get_mut(&3).unwrap();
        pane.runtime.agent_launch += 1;
        pane.runtime.detected_agent.as_mut().unwrap().state = protocol::DetectedAgentState::Idle;
        pane.agent.sync_references(&pane.runtime);

        assert!(matches!(
            server.evaluate_agent_wait(&reference, AgentWaitCondition::Idle),
            WaitEvaluation::Error(ControlErrorCode::AgentReplaced, _)
        ));
        let error = server
            .resolve_agent_wait_target(AgentTarget::Ref(reference.clone()))
            .unwrap_err();
        assert_eq!(error.code, Some(ControlErrorCode::AgentReplaced));
        let fresh = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        assert_ne!(fresh, reference);
        assert!(matches!(
            server.evaluate_agent_wait(&fresh, AgentWaitCondition::Idle),
            WaitEvaluation::Ready(Some(_))
        ));
    }

    #[test]
    fn registration_accepts_an_exact_reference_that_is_already_gone() {
        let server = server_with_agent(protocol::DetectedAgentState::Working);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        let mut server = server;
        server.panes.remove(&3);

        let response = server
            .register_agent_wait(
                1,
                AgentTarget::Ref(reference),
                AgentWaitCondition::Gone,
                None,
                ReplyTo::Headless {
                    capabilities: protocol::Capabilities::default(),
                    effective_protocol: PROTOCOL_VERSION,
                },
            )
            .expect("already-gone wait should complete");
        assert!(response.ok);
        assert!(server.agent_waits.is_empty());
    }

    fn headless() -> ReplyTo {
        ReplyTo::Headless {
            capabilities: protocol::Capabilities::default(),
            effective_protocol: PROTOCOL_VERSION,
        }
    }

    #[test]
    fn a_pane_wait_adopts_the_agent_that_appears_after_it() {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(3, super::super::tests::test_pane(7));

        let response = server.register_agent_wait(
            1,
            AgentTarget::Pane(3),
            AgentWaitCondition::Idle,
            Some(60_000),
            headless(),
        );
        assert!(response.is_none(), "a pane with no agent yet is waited on");

        let pane = server.panes.get_mut(&3).unwrap();
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Working,
        });
        pane.agent.sync_references(&pane.runtime);
        server.resolve_agent_waits();
        assert!(matches!(
            server.agent_waits[&(1, 0)].subject,
            WaitSubject::Agent(_)
        ));

        let pane = server.panes.get_mut(&3).unwrap();
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        pane.agent.sync_references(&pane.runtime);
        server.resolve_agent_waits();
        assert!(server.agent_waits.is_empty());
    }

    #[test]
    fn a_pane_without_an_agent_is_already_gone() {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(3, super::super::tests::test_pane(7));
        let response = server
            .register_agent_wait(
                1,
                AgentTarget::Pane(3),
                AgentWaitCondition::Gone,
                Some(1_000),
                headless(),
            )
            .expect("nothing to wait for");
        assert!(response.ok);
    }

    #[test]
    fn an_attached_client_holds_one_wait_per_request() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Working);
        for request_id in [1, 2] {
            assert!(
                server
                    .register_agent_wait(
                        1,
                        AgentTarget::Pane(3),
                        AgentWaitCondition::Idle,
                        Some(60_000),
                        ReplyTo::Attached { request_id },
                    )
                    .is_none()
            );
        }
        assert_eq!(server.agent_waits.len(), 2);
        assert!(server.holds_agent_wait(1));

        server.remove_agent_waits(1);
        assert!(server.agent_waits.is_empty());
    }

    #[test]
    fn a_timeout_says_which_state_the_agent_ended_in() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Working);
        server.register_agent_wait(
            1,
            AgentTarget::Pane(3),
            AgentWaitCondition::Idle,
            Some(60_000),
            headless(),
        );
        let message = server.timeout_message(&server.agent_waits[&(1, 0)]);
        assert!(
            message.ends_with("to become idle; it is working"),
            "{message}"
        );
    }

    #[test]
    fn post_prompt_wait_requires_departure_before_the_same_state_completes() {
        let mut server = server_with_agent(protocol::DetectedAgentState::Idle);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        server
            .register_post_prompt_wait(
                1,
                reference,
                AgentWaitCondition::Idle,
                None,
                None,
                false,
                ReplyTo::Headless {
                    capabilities: protocol::Capabilities::default(),
                    effective_protocol: PROTOCOL_VERSION,
                },
            )
            .unwrap();
        assert!(!server.agent_waits[&(1, 0)].turn.as_ref().unwrap().armed);

        server.panes.get_mut(&3).unwrap().runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Working,
        });
        server.resolve_agent_waits();
        assert!(server.agent_waits[&(1, 0)].turn.as_ref().unwrap().armed);

        server.panes.get_mut(&3).unwrap().runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        server.resolve_agent_waits();
        assert!(server.agent_waits.is_empty());
    }
}
