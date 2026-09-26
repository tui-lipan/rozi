//! Control commands a UI cannot serve itself, forwarded to the session server that owns them.
//!
//! A recording lives in the session server, next to the pane's PTY, so `record-*` sent to a UI's
//! control socket has nothing to act on here. Rather than refuse and send the caller off to find
//! `--session <NAME>`, the UI passes the request down the attachment it is about as an
//! [`crate::session::protocol::ClientMessage::AttachedControl`] and relays the server's answer.
//! The server keeps the decision: what it accepts on this path is its allowlist
//! ([`crate::session::server::attached_control_refusal`]), not this module's.

use std::sync::mpsc::Sender;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::{ControlCommand, ControlErrorCode, ControlRequest, ControlResponse};
use crate::state::{AttachedReply, PendingAttachedControl, State};

/// Forward a `record-*` request to the session it is about.
///
/// A request from a shared pane goes to that pane's own session, found by the
/// [`crate::session::protocol::SESSION_INSTANCE_ENV`] it carries, whether this UI shows that
/// session or holds it in the background. A request from outside rozi goes to the session on
/// screen. A pane that cannot say which session it runs in is refused rather than guessed at.
///
/// `record-start` is pinned to the explicit target, the calling pane, or the focused pane of
/// that session. `record-stop` and `record-mark` without an id or target use the calling pane.
/// The server never reads `source_pane`, so a resolved pane id travels as `target`.
pub(crate) fn forward_recording(
    ctx: &mut Context<AppRoot>,
    mut request: ControlRequest,
    reply: Sender<ControlResponse>,
) -> Update {
    let routed = if let Some(provenance) = &request.extension {
        Err(ControlResponse::error(format!(
            "a session server cannot check whether extension `{}` is still active, so it does not record on an extension's behalf",
            provenance.id
        )))
    } else {
        route(&ctx.state, &request)
            .and_then(|epoch| pin_recording_target(&ctx.state, epoch, &mut request).map(|()| epoch))
    };
    match routed {
        Ok(epoch) => {
            request.source_pane = None;
            request.source_session = None;
            send_to_epoch(ctx, epoch, request, AttachedReply::Control(reply));
        }
        Err(response) => {
            let _ = reply.send(response);
        }
    }
    Update::none()
}

/// The epoch of the attachment `request` is about: the calling pane's session, current or in the
/// background, or the one on screen for a caller outside every session.
fn route(state: &State, request: &ControlRequest) -> std::result::Result<u64, ControlResponse> {
    match (&request.source_session, request.source_pane) {
        (Some(instance), _) => std::iter::once((state.runtime_epoch, state.current()))
            .chain(
                state
                    .background
                    .iter()
                    .map(|(&epoch, attachment)| (epoch, attachment)),
            )
            .find(|(_, attachment)| attachment.session_instance.as_ref() == Some(instance))
            .map(|(epoch, _)| epoch)
            .ok_or_else(|| {
                ControlResponse::error_with(
                    ControlErrorCode::SessionNotAttached,
                    "the calling pane's session is not attached to this rozi; name it with --session <NAME>",
                )
            }),
        // A bare id names a pane in *some* namespace. Which one is exactly what is missing, and
        // the session on screen is only a guess: the caller may be a scratch or popup pane, or a
        // pane of a session this UI has since switched away from.
        (None, Some(id)) => Err(ControlResponse::error_with(
            ControlErrorCode::Unsupported,
            format!(
                "pane {id} does not say which session it runs in (a scratch or popup pane runs outside every session); name the session with --session <NAME>"
            ),
        )),
        (None, None) => Ok(state.runtime_epoch),
    }
}

fn pin_recording_target(
    state: &State,
    epoch: u64,
    request: &mut ControlRequest,
) -> std::result::Result<(), ControlResponse> {
    let source = request.source_pane;
    match &mut request.command {
        ControlCommand::RecordStart { target, follow, .. } => {
            if *follow {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::InvalidArgument,
                    "foreground recording needs --session <NAME>; a UI cannot hold its caller open until the recording ends",
                ));
            }
            let Some(attachment) = state.attachment_for_epoch(epoch) else {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::SessionNotAttached,
                    "this rozi is not attached to a session",
                ));
            };
            let id = match target.or(source) {
                Some(id) => id,
                // A caller outside every session uses the pane focused in this attachment.
                None if state.scratch_visible => {
                    return Err(ControlResponse::error_with(
                        ControlErrorCode::Unsupported,
                        "the focused pane is a scratch pane, which runs outside the session and cannot be recorded",
                    ));
                }
                None => attachment.focused_pane.ok_or_else(|| {
                    ControlResponse::error_with(
                        ControlErrorCode::TargetRequired,
                        "no target pane and no focused pane",
                    )
                })?,
            };
            // A session pane can share its number with a scratch or popup pane.
            let pane = attachment
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.panes.iter())
                .find(|pane| pane.id == id);
            if pane.is_none_or(|pane| pane.closing) {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::PaneNotFound,
                    format!("pane {id} not found"),
                ));
            }
            *target = Some(id);
        }
        ControlCommand::RecordStop { id, target }
        | ControlCommand::RecordMark { id, target, .. } => {
            if id.is_none() && target.is_none() {
                *target = source;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Send a UI action down the current attachment, holding its reply for on-screen feedback.
pub(crate) fn send(ctx: &mut Context<AppRoot>, request: ControlRequest, reply: AttachedReply) {
    let epoch = ctx.state.runtime_epoch;
    send_to_epoch(ctx, epoch, request, reply);
}

/// Send a request down attachment `epoch`, holding its reply until the server answers.
fn send_to_epoch(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request: ControlRequest,
    reply: AttachedReply,
) {
    let state = &mut ctx.state;
    forget_orphans(state);
    let Some(client) = state
        .attachment_for_epoch(epoch)
        .and_then(|attachment| attachment.session_client.clone())
    else {
        answer(
            ctx,
            epoch,
            reply,
            ControlResponse::error_with(
                ControlErrorCode::SessionNotAttached,
                "this rozi is not attached to a session",
            ),
        );
        return;
    };
    let request_id = state.next_attached_control_request_id;
    state.next_attached_control_request_id = request_id.wrapping_add(1).max(1);
    state
        .pending_attached_controls
        .insert(request_id, PendingAttachedControl { epoch, reply });
    client.attached_control(request_id, request);
}

/// Relay a session server's answer to the request it belongs to.
pub(crate) fn result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    response: ControlResponse,
) -> Update {
    let state = &mut ctx.state;
    if state
        .pending_attached_controls
        .get(&request_id)
        .is_some_and(|pending| pending.epoch == epoch)
        && let Some(pending) = state.pending_attached_controls.remove(&request_id)
    {
        return answer(ctx, epoch, pending.reply, response);
    }
    Update::none()
}

fn answer(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    reply: AttachedReply,
    response: ControlResponse,
) -> Update {
    match reply {
        AttachedReply::Control(reply) => {
            let _ = reply.send(response);
            Update::none()
        }
        AttachedReply::Action(action) => {
            crate::ops::recording::answered(ctx, epoch, action, response)
        }
    }
}

/// Answer every request still waiting on attachment `epoch`, which will now never reply.
///
/// This UI's own recording commands go unanswered: the attachment ending says more on screen than
/// a toast per command lost with it.
pub(crate) fn fail_epoch(state: &mut State, epoch: u64, message: &str) {
    let before = state.pending_recording_toggles.len();
    state
        .pending_recording_toggles
        .retain(|(pending_epoch, _), _| *pending_epoch != epoch);
    if state.pending_recording_toggles.len() != before {
        state.commands_dirty = true;
    }
    let ended = state
        .pending_attached_controls
        .iter()
        .filter_map(|(&id, pending)| (pending.epoch == epoch).then_some(id))
        .collect::<Vec<_>>();
    for id in ended {
        if let Some(PendingAttachedControl {
            reply: AttachedReply::Control(reply),
            ..
        }) = state.pending_attached_controls.remove(&id)
        {
            let _ = reply.send(ControlResponse::error_with(
                ControlErrorCode::SessionNotConnected,
                message,
            ));
        }
    }
}

/// Answer requests whose attachment is gone without a disconnect to say so, such as one replaced
/// by a session switch.
fn forget_orphans(state: &mut State) {
    let gone = state
        .pending_attached_controls
        .values()
        .map(|pending| pending.epoch)
        .filter(|&epoch| state.attachment_for_epoch(epoch).is_none())
        .collect::<Vec<_>>();
    for epoch in gone {
        fail_epoch(
            state,
            epoch,
            "the session attachment ended before answering",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tui_lipan::TestBackend;

    use crate::control::{ControlEnvelope, ControlRequest};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientMessage, SessionInstanceId};

    use super::*;

    fn start(follow: bool) -> ControlCommand {
        ControlCommand::RecordStart {
            target: None,
            output: Some("/tmp/pane.rozirec".into()),
            max_fps: None,
            duration_ms: None,
            max_bytes: None,
            force: false,
            follow,
        }
    }

    fn ask(
        backend: &mut TestBackend<AppRoot>,
        command: ControlCommand,
    ) -> mpsc::Receiver<ControlResponse> {
        ask_from(backend, command, None, None)
    }

    /// Ask as a pane would: `ROZI_PANE` and `ROZI_SESSION_INSTANCE`.
    fn ask_from(
        backend: &mut TestBackend<AppRoot>,
        command: ControlCommand,
        source_pane: Option<crate::state::PaneId>,
        source_session: Option<&str>,
    ) -> mpsc::Receiver<ControlResponse> {
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane,
                    source_session: source_session.map(SessionInstanceId::for_test),
                    extension: None,
                },
                reply,
            }))
            .expect("dispatch a record request");
        response
    }

    fn attach(attachment: &mut crate::state::Attachment, client: SessionClient, instance: &str) {
        attachment.session_attached = true;
        attachment.session_client = Some(client);
        attachment.session_instance = Some(SessionInstanceId::for_test(instance));
    }

    fn code(response: &mpsc::Receiver<ControlResponse>) -> Option<ControlErrorCode> {
        response.try_recv().expect("answered at once").code
    }

    fn forwarded(outbound: &mpsc::Receiver<ClientOutbound>) -> Vec<(u64, ControlRequest)> {
        outbound
            .try_iter()
            .filter_map(|message| match message {
                ClientOutbound::Control(ClientMessage::AttachedControl {
                    request_id,
                    request,
                }) => Some((request_id, request)),
                _ => None,
            })
            .collect()
    }

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn a_record_request_goes_to_the_session_pinned_to_a_pane_and_its_answer_comes_back() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, outbound) = SessionClient::test_channel();
            let (epoch, focused) = {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client);
                (state.runtime_epoch, state.focused_pane().unwrap())
            };

            let response = ask(&mut backend, start(false));
            let [(request_id, request)] = forwarded(&outbound).try_into().unwrap();
            let ControlCommand::RecordStart { target, .. } = request.command else {
                panic!("expected the start to be forwarded");
            };
            assert_eq!(target, Some(focused), "the server is told which pane");
            assert!(response.try_recv().is_err(), "held for the server's answer");

            let answer = |epoch, request_id| crate::Msg::SessionAttachedControlResult {
                epoch,
                request_id,
                response: ControlResponse::empty(),
            };
            backend.dispatch(answer(epoch + 1, request_id)).unwrap();
            assert!(
                response.try_recv().is_err(),
                "another attachment's answer is not this one"
            );
            backend.dispatch(answer(epoch, request_id)).unwrap();
            assert!(response.try_recv().unwrap().ok);

            let stop = ask(
                &mut backend,
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                },
            );
            assert_eq!(forwarded(&outbound).len(), 1);
            backend
                .dispatch(crate::Msg::SessionDisconnected {
                    epoch,
                    name: String::new(),
                })
                .unwrap();
            assert_eq!(
                stop.try_recv().unwrap().code,
                Some(ControlErrorCode::SessionNotConnected),
                "a dropped session answers what it left pending"
            );
            assert!(backend.state().pending_attached_controls.is_empty());
        });
    }

    #[test]
    fn a_stop_or_a_mark_run_inside_a_pane_names_that_pane() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, outbound) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                attach(state.current_mut(), client, "a");
            }
            let from_pane = |command| {
                crate::Msg::ControlRequest(ControlEnvelope {
                    request: ControlRequest {
                        command,
                        source_pane: Some(7),
                        source_session: Some(SessionInstanceId::for_test("a")),
                        extension: None,
                    },
                    reply: mpsc::channel().0,
                })
            };
            let commands = [
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                },
                ControlCommand::RecordMark {
                    label: "x".into(),
                    id: None,
                    target: None,
                },
                ControlCommand::RecordStop {
                    id: Some(3),
                    target: None,
                },
            ];
            for command in commands {
                backend.dispatch(from_pane(command)).unwrap();
            }
            let targets = forwarded(&outbound)
                .into_iter()
                .map(|(_, request)| match request.command {
                    ControlCommand::RecordStop { id, target }
                    | ControlCommand::RecordMark { id, target, .. } => (id, target),
                    other => panic!("unexpected {other:?}"),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                targets,
                [(None, Some(7)), (None, Some(7)), (Some(3), None)],
                "a named recording is not narrowed to the caller's pane"
            );
        });
    }

    #[test]
    fn a_record_request_the_session_could_not_take_never_leaves_the_ui() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let refused = ask(&mut backend, ControlCommand::RecordList);
            assert_eq!(
                refused.try_recv().unwrap().code,
                Some(ControlErrorCode::SessionNotAttached)
            );

            let (client, outbound) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client);
            }
            let foreground = ask(&mut backend, start(true));
            assert_eq!(
                foreground.try_recv().unwrap().code,
                Some(ControlErrorCode::InvalidArgument)
            );
            let (reply, missing) = mpsc::channel();
            backend
                .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                    request: ControlRequest {
                        command: ControlCommand::RecordStart {
                            target: Some(999),
                            output: Some("/tmp/pane.rozirec".into()),
                            max_fps: None,
                            duration_ms: None,
                            max_bytes: None,
                            force: false,
                            follow: false,
                        },
                        source_pane: None,
                        source_session: None,
                        extension: None,
                    },
                    reply,
                }))
                .unwrap();
            assert_eq!(
                missing.try_recv().unwrap().code,
                Some(ControlErrorCode::PaneNotFound)
            );
            assert!(forwarded(&outbound).is_empty());
        });
    }

    /// Session A's pane 1 keeps running after the UI switches to session B, which has a pane 1 of
    /// its own. What A's pane asks for is A's business, not whatever is on screen.
    #[test]
    fn a_pane_in_a_background_session_is_answered_by_its_own_session() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client_a, outbound_a) = SessionClient::test_channel();
            let (client_b, outbound_b) = SessionClient::test_channel();
            let (epoch_a, pane) = {
                let state = backend.state_mut();
                let epoch_a = state.runtime_epoch;
                let pane = state.focused_pane().unwrap();
                attach(state.current_mut(), client_a, "a");
                let mut b = crate::state::fresh_default_attachment(&state.config);
                attach(&mut b, client_b, "b");
                state.park_current(epoch_a, b);
                state.runtime_epoch = state.mint_attachment_id();
                assert_eq!(state.focused_pane(), Some(pane), "B has a pane {pane} too");
                (epoch_a, pane)
            };

            let response = ask_from(&mut backend, start(false), Some(pane), Some("a"));
            assert!(forwarded(&outbound_b).is_empty(), "B is only on screen");
            let [(request_id, request)] = forwarded(&outbound_a).try_into().unwrap();
            assert!(
                matches!(request.command, ControlCommand::RecordStart { target: Some(id), .. } if id == pane)
            );
            assert_eq!(request.source_session, None, "the server never reads it");
            backend
                .dispatch(crate::Msg::SessionAttachedControlResult {
                    epoch: epoch_a,
                    request_id,
                    response: ControlResponse::empty(),
                })
                .unwrap();
            assert!(response.try_recv().unwrap().ok);

            let _stop = ask_from(
                &mut backend,
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                },
                Some(pane),
                Some("a"),
            );
            assert_eq!(
                forwarded(&outbound_a).len(),
                1,
                "stop follows the caller too"
            );
            assert!(forwarded(&outbound_b).is_empty());

            let elsewhere = ask_from(
                &mut backend,
                ControlCommand::RecordList,
                Some(pane),
                Some("c"),
            );
            assert_eq!(code(&elsewhere), Some(ControlErrorCode::SessionNotAttached));
            let unnamed = ask_from(&mut backend, ControlCommand::RecordList, Some(pane), None);
            assert_eq!(
                code(&unnamed),
                Some(ControlErrorCode::Unsupported),
                "a bare pane id is not guessed onto the session on screen"
            );
            assert!(forwarded(&outbound_a).is_empty() && forwarded(&outbound_b).is_empty());

            let outside = ask(&mut backend, ControlCommand::RecordList);
            assert!(outside.try_recv().is_err(), "held for B's answer");
            assert_eq!(
                forwarded(&outbound_b).len(),
                1,
                "no pane asking means the screen"
            );
        });
    }

    #[test]
    fn a_scratch_pane_with_the_same_number_does_not_hide_the_session_pane() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, outbound) = SessionClient::test_channel();
            let shared = {
                let state = backend.state_mut();
                attach(state.current_mut(), client, "a");
                let shared = state.focused_pane().unwrap();
                state.scratch.panes.push(crate::state::Pane::new(
                    shared,
                    100,
                    FloatRect::default(),
                ));
                shared
            };

            let (reply, _response) = mpsc::channel();
            backend
                .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                    request: ControlRequest {
                        command: ControlCommand::RecordStart {
                            target: Some(shared),
                            output: Some("/tmp/pane.rozirec".into()),
                            max_fps: None,
                            duration_ms: None,
                            max_bytes: None,
                            force: false,
                            follow: false,
                        },
                        source_pane: None,
                        source_session: None,
                        extension: None,
                    },
                    reply,
                }))
                .unwrap();
            let [(_, request)] = forwarded(&outbound).try_into().unwrap();
            assert!(
                matches!(request.command, ControlCommand::RecordStart { target: Some(id), .. } if id == shared)
            );

            {
                let state = backend.state_mut();
                state.scratch_visible = true;
                state.scratch.focused_pane = Some(shared);
            }
            let focused_scratch = ask(&mut backend, start(false));
            assert_eq!(
                code(&focused_scratch),
                Some(ControlErrorCode::Unsupported),
                "focus on the scratch pane is not focus on session pane {shared}"
            );
            assert!(forwarded(&outbound).is_empty());
        });
    }
}
