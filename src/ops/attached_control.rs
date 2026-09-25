//! Control commands a UI cannot serve itself, forwarded to the session server that owns them.
//!
//! A recording lives in the session server, next to the pane's PTY, so `record-*` sent to a UI's
//! control socket has nothing to act on here. Rather than refuse and send the caller off to find
//! `--session <NAME>`, the UI passes the request down its own attachment as an
//! [`crate::session::protocol::ClientMessage::AttachedControl`] and relays the server's answer.
//! The server keeps the decision: what it accepts on this path is its allowlist
//! ([`crate::session::server::attached_control_refusal`]), not this module's.

use std::sync::mpsc::Sender;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::{ControlCommand, ControlErrorCode, ControlRequest, ControlResponse};
use crate::state::{PendingAttachedControl, State};

/// Forward a `record-*` request to the session this UI is looking at.
///
/// `record-start` is pinned to one pane before it leaves: the explicit target, else the pane the
/// caller runs in, else the focused one. The server never reads `source_pane` (it cannot tell which
/// session a bare id came from), so the resolved id always travels as `target`.
pub(crate) fn forward_recording(
    ctx: &mut Context<AppRoot>,
    mut request: ControlRequest,
    reply: Sender<ControlResponse>,
) -> Update {
    let refusal = if let Some(provenance) = &request.extension {
        Some(ControlResponse::error(format!(
            "a session server cannot check whether extension `{}` is still active, so it does not record on an extension's behalf",
            provenance.id
        )))
    } else {
        pin_recording_target(&ctx.state, &mut request).err()
    };
    if let Some(response) = refusal {
        let _ = reply.send(response);
        return Update::none();
    }
    request.source_pane = None;
    send(&mut ctx.state, request, reply);
    Update::none()
}

fn pin_recording_target(
    state: &State,
    request: &mut ControlRequest,
) -> std::result::Result<(), ControlResponse> {
    let source = request.source_pane;
    let ControlCommand::RecordStart { target, follow, .. } = &mut request.command else {
        return Ok(());
    };
    if *follow {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            "foreground recording needs --session <NAME>; a UI cannot hold its caller open until the recording ends",
        ));
    }
    let Some(id) = target.or(source).or(state.focused_pane()) else {
        return Err(ControlResponse::error_with(
            ControlErrorCode::TargetRequired,
            "no target pane and no focused pane",
        ));
    };
    if crate::pane::lifecycle::pane_is_local(state, id) {
        return Err(ControlResponse::error_with(
            ControlErrorCode::Unsupported,
            format!(
                "pane {id} is a scratch or popup pane, which runs outside the session and cannot be recorded"
            ),
        ));
    }
    if crate::pane::lifecycle::find_pane(state, id).is_none_or(|pane| pane.closing) {
        return Err(ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {id} not found"),
        ));
    }
    *target = Some(id);
    Ok(())
}

/// Send `request` down the current attachment, holding `reply` until the server answers.
fn send(state: &mut State, request: ControlRequest, reply: Sender<ControlResponse>) {
    forget_orphans(state);
    let Some(client) = state.current().session_client.clone() else {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::SessionNotAttached,
            "this rozi is not attached to a session",
        ));
        return;
    };
    let request_id = state.next_attached_control_request_id;
    state.next_attached_control_request_id = request_id.wrapping_add(1).max(1);
    state.pending_attached_controls.insert(
        request_id,
        PendingAttachedControl {
            epoch: state.runtime_epoch,
            reply,
        },
    );
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
        let _ = pending.reply.send(response);
    }
    Update::none()
}

/// Answer every request still waiting on attachment `epoch`, which will now never reply.
pub(crate) fn fail_epoch(state: &mut State, epoch: u64, message: &str) {
    let ended = state
        .pending_attached_controls
        .iter()
        .filter_map(|(&id, pending)| (pending.epoch == epoch).then_some(id))
        .collect::<Vec<_>>();
    for id in ended {
        if let Some(pending) = state.pending_attached_controls.remove(&id) {
            let _ = pending.reply.send(ControlResponse::error_with(
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
    use crate::session::protocol::ClientMessage;

    use super::*;

    fn start(follow: bool) -> ControlCommand {
        ControlCommand::RecordStart {
            target: None,
            output: "/tmp/pane.rozirec".into(),
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
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane: None,
                    extension: None,
                },
                reply,
            }))
            .expect("dispatch a record request");
        response
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

            let stop = ask(&mut backend, ControlCommand::RecordStop { id: None });
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
                            output: "/tmp/pane.rozirec".into(),
                            max_fps: None,
                            duration_ms: None,
                            max_bytes: None,
                            force: false,
                            follow: false,
                        },
                        source_pane: None,
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
}
