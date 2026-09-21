use serde::Serialize;

use super::*;
use crate::control::{AgentTarget, AgentWaitCondition, ControlErrorCode, ControlResponse};

pub(super) struct PendingAgentWait {
    reference: protocol::AgentRef,
    until: AgentWaitCondition,
    deadline: Option<Instant>,
    capabilities: protocol::Capabilities,
    effective_protocol: u32,
}

#[derive(Serialize)]
struct AgentWaitResult {
    condition: AgentWaitCondition,
    agent: Option<protocol::AgentRuntime>,
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
        capabilities: protocol::Capabilities,
        effective_protocol: u32,
    ) -> Option<ControlResponse> {
        let reference = match self.resolve_agent_wait_target(target) {
            Ok(reference) => reference,
            Err(response) => return Some(response),
        };
        match self.evaluate_agent_wait(&reference, until) {
            WaitEvaluation::Ready(agent) => Some(ControlResponse::ok(AgentWaitResult {
                condition: until,
                agent,
            })),
            WaitEvaluation::Error(code, message) => {
                Some(ControlResponse::error_with(code, message))
            }
            WaitEvaluation::Pending => {
                self.agent_waits.insert(
                    client_id,
                    PendingAgentWait {
                        reference,
                        until,
                        deadline: timeout_ms
                            .map(Duration::from_millis)
                            .and_then(|duration| Instant::now().checked_add(duration)),
                        capabilities,
                        effective_protocol,
                    },
                );
                None
            }
        }
    }

    pub(super) fn resolve_agent_waits(&mut self) {
        let mut completed = Vec::new();
        for (&client_id, wait) in &self.agent_waits {
            let evaluation = self.evaluate_agent_wait(&wait.reference, wait.until);
            if !matches!(evaluation, WaitEvaluation::Pending) {
                completed.push((client_id, evaluation));
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
            .map(|(&client_id, wait)| {
                (
                    client_id,
                    WaitEvaluation::Error(
                        ControlErrorCode::Timeout,
                        format!(
                            "timed out waiting for agent {} in pane {} to become {}",
                            wait.reference.incarnation,
                            wait.reference.pane.pane_id,
                            wait_condition_name(wait.until)
                        ),
                    ),
                )
            })
            .collect();
        self.finish_agent_waits(completed);
    }

    fn finish_agent_waits(&mut self, completed: Vec<(ClientId, WaitEvaluation)>) {
        for (client_id, evaluation) in completed {
            let Some(wait) = self.agent_waits.remove(&client_id) else {
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
            self.enqueue(
                client_id,
                Target::Sender,
                ServerMessage::SessionControlResult {
                    capabilities: Some(wait.capabilities),
                    effective_protocol: wait.effective_protocol,
                    response,
                },
            );
            self.set_close_after_flush(client_id);
        }
    }

    pub(super) fn remove_agent_wait(&mut self, client_id: ClientId) {
        self.agent_waits.remove(&client_id);
    }

    fn resolve_agent_wait_target(
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
                let runtimes = self.agent_runtimes_for(pane_id, pane);
                match runtimes.as_slice() {
                    [runtime] => Ok(runtime.reference.clone()),
                    [] => Err(ControlResponse::error_with(
                        ControlErrorCode::AgentGone,
                        format!("pane {pane_id} has no agent"),
                    )),
                    _ => Err(ControlResponse::error_with(
                        ControlErrorCode::TargetRequired,
                        format!(
                            "pane {pane_id} has multiple agent activities; use an exact agent reference"
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

    fn agent_runtimes_for(
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
                protocol::Capabilities::default(),
                PROTOCOL_VERSION,
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
    fn gone_is_a_success_condition() {
        let server = server_with_agent(protocol::DetectedAgentState::Working);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        let mut server = server;
        server.panes.remove(&3);

        assert!(matches!(
            server.evaluate_agent_wait(&reference, AgentWaitCondition::Gone),
            WaitEvaluation::Ready(None)
        ));
    }
}
