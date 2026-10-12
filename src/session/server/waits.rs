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
    require_transition: bool,
    armed: bool,
    reply: ReplyTo,
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

enum WaitEvaluation {
    Pending,
    Ready(Option<protocol::AgentRuntime>),
    Error(ControlErrorCode, String),
}

impl SessionServer {
    pub(super) fn register_agent_wait(
        &mut self,
        client_id: ClientId,
        target: AgentTarget,
        until: AgentWaitCondition,
        timeout_ms: Option<u64>,
        reply: ReplyTo,
    ) -> Option<ControlResponse> {
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
                        condition: until,
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
                condition: until,
                agent,
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
                        require_transition: false,
                        armed: true,
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
                                format!(
                                    "pane {pane_id} shows none of the agents it lists; pass one's --ref from `agents list`"
                                ),
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
            let (require_transition, armed) = (wait.require_transition, wait.armed);
            let wait = self.agent_waits.get_mut(&key).expect("wait read above");
            wait.subject = subject;
            if require_transition && !armed {
                match evaluation {
                    WaitEvaluation::Ready(_) => evaluation = WaitEvaluation::Pending,
                    WaitEvaluation::Pending => wait.armed = true,
                    WaitEvaluation::Error(_, _) => {}
                }
            }
            if !matches!(evaluation, WaitEvaluation::Pending) {
                completed.push((key, evaluation));
            }
        }
        self.finish_agent_waits(completed);
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
                WaitEvaluation::Ready(agent) => ControlResponse::ok(AgentWaitResult {
                    condition: wait.until,
                    agent,
                }),
                WaitEvaluation::Error(code, message) => ControlResponse::error_with(code, message),
                WaitEvaluation::Pending => continue,
            };
            self.answer_held(key.0, &wait.reply, response);
        }
    }

    /// Drop every wait `client_id` holds: its connection is gone.
    pub(super) fn remove_agent_waits(&mut self, client_id: ClientId) {
        self.agent_waits
            .retain(|&(client, _), _| client != client_id);
    }

    /// Whether `client_id` still holds a wait, which keeps a headless connection open.
    pub(super) fn holds_agent_wait(&self, client_id: ClientId) -> bool {
        self.agent_waits
            .keys()
            .any(|&(client, _)| client == client_id)
    }

    pub(super) fn register_post_prompt_wait(
        &mut self,
        client_id: ClientId,
        reference: protocol::AgentRef,
        until: AgentWaitCondition,
        timeout_ms: Option<u64>,
        reply: ReplyTo,
    ) -> std::result::Result<(), ControlResponse> {
        let evaluation = self.evaluate_agent_wait(&reference, until);
        let armed = match evaluation {
            WaitEvaluation::Ready(_) => false,
            WaitEvaluation::Pending => true,
            WaitEvaluation::Error(code, message) => {
                return Err(ControlResponse::error_with(code, message));
            }
        };
        self.agent_waits.insert(
            (client_id, reply.hold_id()),
            PendingAgentWait {
                subject: WaitSubject::Agent(reference),
                until,
                deadline: timeout_ms
                    .map(Duration::from_millis)
                    .and_then(|duration| Instant::now().checked_add(duration)),
                require_transition: true,
                armed,
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
                        format!(
                            "pane {pane_id} shows none of the agents it lists; pass one's --ref from `agents list`"
                        ),
                    )),
                }
            }
            AgentTarget::Ref(reference) => {
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
                let current = self
                    .agent_runtimes_for(reference.pane.pane_id, pane)
                    .into_iter()
                    .find(|runtime| runtime.reference == reference);
                current.map(|runtime| runtime.reference).ok_or_else(|| {
                    ControlResponse::error_with(
                        ControlErrorCode::AgentReplaced,
                        "agent reference no longer names the current incarnation",
                    )
                })
            }
        }
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
                WaitEvaluation::Ready(Some(runtime.clone()))
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
        protocol::effective_agent_runtimes(&pane.runtime, &pane.agent.references(pane_ref))
    }
}

fn condition_matches(until: AgentWaitCondition, state: protocol::AgentState) -> bool {
    match until {
        AgentWaitCondition::Working => state == protocol::AgentState::Working,
        AgentWaitCondition::Blocked => state == protocol::AgentState::Blocked,
        AgentWaitCondition::Idle => state == protocol::AgentState::Idle,
        AgentWaitCondition::Done => state == protocol::AgentState::Done,
        AgentWaitCondition::Quiescent => state.is_quiescent(),
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
        AgentWaitCondition::Quiescent => "quiescent",
        AgentWaitCondition::Gone => "gone",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

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
                ReplyTo::Headless {
                    capabilities: protocol::Capabilities::default(),
                    effective_protocol: PROTOCOL_VERSION,
                },
            )
            .unwrap();
        assert!(!server.agent_waits[&(1, 0)].armed);

        server.panes.get_mut(&3).unwrap().runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Working,
        });
        server.resolve_agent_waits();
        assert!(server.agent_waits[&(1, 0)].armed);

        server.panes.get_mut(&3).unwrap().runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        server.resolve_agent_waits();
        assert!(server.agent_waits.is_empty());
    }
}
