//! Fence caller-aware UI mutations to the verified session and pane, before any side effect.
use crate::control::{
    AgentTarget, ControlCommand, ControlErrorCode, ControlRequest, ControlResponse,
};
use crate::state::State;

pub(super) fn validate(state: &State, request: &ControlRequest) -> Result<(), ControlResponse> {
    let Some(caller) = &request.caller_context else {
        return Ok(());
    };
    if request.source_pane != Some(caller.pane_id)
        || request.source_session.as_ref() != Some(&caller.session_instance)
    {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            "caller context must match the source pane and session",
        ));
    }
    let attachment = state.current();
    if !attachment.session_attached
        || attachment.session_instance.as_ref() != Some(&caller.session_instance)
    {
        return Err(ControlResponse::error_with(
            ControlErrorCode::OutOfScope,
            "the caller's UI no longer displays its verified session",
        ));
    }
    let pane = crate::pane::lifecycle::find_pane_in_namespace(state, caller.pane_id, false);
    if !pane.is_some_and(|pane| pane.pty_generation == caller.generation && !pane.closing) {
        return Err(ControlResponse::error_with(
            ControlErrorCode::StaleReference,
            "the verified caller pane was replaced or removed",
        ));
    }
    validate_targets(state, &request.command, caller)
}

fn validate_targets(
    state: &State,
    command: &ControlCommand,
    caller: &crate::session::protocol::PaneRef,
) -> Result<(), ControlResponse> {
    let targets = match command {
        ControlCommand::NewPane { .. } => vec![caller.pane_id],
        ControlCommand::PaneSet { target, .. }
        | ControlCommand::PaneMove { target, .. }
        | ControlCommand::PaneClose { target, .. } => vec![*target],
        ControlCommand::PaneSwap { target, with, .. } => vec![*target, *with],
        ControlCommand::SendText { target, .. } | ControlCommand::SendKeys { target, .. } => {
            vec![target.unwrap_or(caller.pane_id)]
        }
        ControlCommand::AgentPrompt {
            target: AgentTarget::Ref(reference),
            ..
        } => {
            if reference.pane.session_instance != caller.session_instance {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::StaleReference,
                    "agent reference belongs to another caller session",
                ));
            }
            return Ok(());
        }
        // Prompts are forwarded to the server's shared namespace, without a client-local lookup.
        _ => return Ok(()),
    };
    if targets
        .into_iter()
        .any(|id| crate::pane::lifecycle::pane_is_local(state, id))
    {
        return Err(ControlResponse::error_with(
            ControlErrorCode::OutOfScope,
            "a client-local pane shadows the target in the verified session",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppRoot;
    use crate::control::ControlEnvelope;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientMessage, PaneRef, SessionInstanceId};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn on_stack(test: impl FnOnce() + Send + 'static) {
        crate::test_support::on_large_stack(test);
    }

    fn attached() -> (
        TestBackend<AppRoot>,
        mpsc::Receiver<ClientOutbound>,
        PaneRef,
    ) {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline);
            backend.pump().unwrap();
            std::thread::yield_now();
        }
        let (client, outbound) = SessionClient::test_channel();
        let attachment = backend.state_mut().current_mut();
        attachment.session_attached = true;
        attachment.session_client = Some(client);
        attachment.session_instance = Some(SessionInstanceId::for_test("caller"));
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.input_locked = true;
        attachment.shared = Some(shared);
        let pane = &mut attachment.workspaces[0].panes[0];
        pane.terminal.process_server_output(b"");
        let caller = PaneRef {
            session_instance: SessionInstanceId::for_test("caller"),
            pane_id: pane.id,
            generation: pane.pty_generation,
        };
        (backend, outbound, caller)
    }

    fn ask(
        backend: &mut TestBackend<AppRoot>,
        caller: &PaneRef,
        command: ControlCommand,
    ) -> mpsc::Receiver<ControlResponse> {
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane: Some(caller.pane_id),
                    source_session: Some(caller.session_instance.clone()),
                    caller_context: Some(Box::new(caller.clone())),
                    extension: None,
                    credential: None,
                },
                reply,
                worker: None,
            }))
            .unwrap();
        response
    }

    #[test]
    fn caller_ui_uses_its_controller_to_split() {
        on_stack(|| {
            let (mut backend, outbound, caller) = attached();
            let response = ask(
                &mut backend,
                &caller,
                ControlCommand::NewPane {
                    command: None,
                    argv: Some(vec!["cargo".into(), "test".into()]),
                    local_cwd: None,
                    cwd: None,
                    title: None,
                    keep_open: false,
                    focus: false,
                    workspace: None,
                    size: None,
                },
            );
            assert!(
                response.try_recv().is_err(),
                "the reply waits for the PTY, rather than refusing controller authority"
            );
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::SpawnPane { .. })
            )));
        });
    }

    #[test]
    fn caller_ui_preserves_controller_input_and_follower_read_only_rules() {
        on_stack(|| {
            for (controller, read_only, allowed) in [
                (true, false, true),
                (false, false, false),
                (true, true, false),
            ] {
                let (mut backend, outbound, caller) = attached();
                let shared = backend.state_mut().current_mut().shared.as_mut().unwrap();
                shared.controller = Some(if controller { 1 } else { 2 });
                shared.read_only = read_only;
                let response = ask(
                    &mut backend,
                    &caller,
                    ControlCommand::SendText {
                        target: Some(caller.pane_id),
                        text: "hello".into(),
                        wait: None,
                        capture: None,
                        scale: None,
                    },
                )
                .try_recv()
                .unwrap();
                assert_eq!(response.ok, allowed, "{response:?}");
                assert_eq!(
                    outbound
                        .try_iter()
                        .any(|message| matches!(message, ClientOutbound::PaneInput { .. })),
                    allowed
                );
            }
        });
    }

    #[test]
    fn a_session_switch_or_replaced_caller_refuses_input_before_forwarding() {
        on_stack(|| {
            for replaced in [false, true] {
                let (mut backend, outbound, caller) = attached();
                if replaced {
                    backend.state_mut().current_mut().workspaces[0].panes[0].pty_generation += 1;
                } else {
                    backend.state_mut().current_mut().session_instance =
                        Some(SessionInstanceId::for_test("elsewhere"));
                }
                let response = ask(
                    &mut backend,
                    &caller,
                    ControlCommand::SendText {
                        target: Some(caller.pane_id),
                        text: "must not arrive".into(),
                        wait: None,
                        capture: None,
                        scale: None,
                    },
                )
                .try_recv()
                .unwrap();
                assert_eq!(
                    response.code,
                    Some(if replaced {
                        ControlErrorCode::StaleReference
                    } else {
                        ControlErrorCode::OutOfScope
                    })
                );
                assert!(
                    !outbound
                        .try_iter()
                        .any(|message| matches!(message, ClientOutbound::PaneInput { .. }))
                );
            }
        });
    }

    #[test]
    fn a_prompt_keeps_the_caller_fence_on_its_attached_connection() {
        on_stack(|| {
            let (mut backend, outbound, caller) = attached();
            let _response = ask(
                &mut backend,
                &caller,
                ControlCommand::AgentPrompt {
                    target: AgentTarget::Pane(caller.pane_id),
                    prompt: "hello".into(),
                    wait: None,
                    capture: None,
                    timeout_ms: None,
                    allow_working: false,
                },
            );
            assert!(outbound.try_iter().any(|message| matches!(message,
                ClientOutbound::Control(ClientMessage::AttachedControl { request, .. }) if request.caller_context.as_deref() == Some(&caller))));
        });
    }
}
