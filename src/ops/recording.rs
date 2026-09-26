//! The Start/Stop pane recording and Mark pane recording commands.
//!
//! A recording lives in the session server, so these send `record-*` down this UI's own attachment
//! ([`crate::ops::attached_control`]) and show the server's answer as a toast. They name no file:
//! the server picks the name and the directory, from its own `[recording]` settings, on its own
//! host.

use std::time::Duration;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::{
    ControlCommand, ControlRequest, ControlResponse, RecordingInfo, RecordingStopList,
};
use crate::ops::focus::{request_current_pane_focus, request_recording_mark_focus};
use crate::pane::lifecycle::{find_pane, pane_is_local};
use crate::pane::pty_events::{notify_error, notify_info};
use crate::state::{AttachedReply, Mode, PaneId, RecordingAction, RecordingMarkPrompt, State};

/// How long a mark's blink holds the dot highlighted.
const MARK_BLINK: Duration = Duration::from_millis(300);

/// The pane the recording commands act on: the focused one, when it runs in the attached session
/// and this client may change it. A scratch or popup pane runs outside the session, and a read-only
/// client may watch a recording but not start or stop one.
pub(crate) fn recording_command_target(state: &State) -> Option<PaneId> {
    let attachment = state.current();
    attachment.session_client.as_ref()?;
    if attachment
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        return None;
    }
    let pane = state.focused_pane()?;
    (!pane_is_local(state, pane)).then_some(pane)
}

/// Whether the recording commands' pane is being recorded.
pub(crate) fn target_is_recording(state: &State) -> bool {
    recording_command_target(state)
        .and_then(|pane| find_pane(state, pane))
        .is_some_and(|pane| pane.terminal.recording)
}

pub(crate) fn target_toggle_pending(state: &State) -> bool {
    recording_command_target(state).is_some_and(|pane| {
        state
            .pending_recording_toggles
            .contains_key(&(state.runtime_epoch, pane))
    })
}

/// Release a toggle only after the server's recording flag reaches the requested state.
pub(crate) fn runtime_recording_changed(
    state: &mut State,
    epoch: u64,
    pane: PaneId,
    recording: bool,
) {
    let key = (epoch, pane);
    if state.pending_recording_toggles.get(&key) == Some(&recording) {
        state.pending_recording_toggles.remove(&key);
        state.commands_dirty = true;
    }
}

pub(crate) fn toggle_pane_recording(ctx: &mut Context<AppRoot>) -> Update {
    let Some(pane) = recording_command_target(&ctx.state) else {
        return Update::none();
    };
    if target_toggle_pending(&ctx.state) {
        return Update::none();
    }
    let (command, action) = if target_is_recording(&ctx.state) {
        (
            ControlCommand::RecordStop {
                id: None,
                target: Some(pane),
            },
            RecordingAction::Stop(pane),
        )
    } else {
        (
            ControlCommand::RecordStart {
                target: Some(pane),
                output: None,
                max_fps: None,
                duration_ms: None,
                max_bytes: None,
                force: false,
                follow: false,
            },
            RecordingAction::Start(pane),
        )
    };
    ctx.state.pending_recording_toggles.insert(
        (ctx.state.runtime_epoch, pane),
        matches!(action, RecordingAction::Start(_)),
    );
    ctx.state.commands_dirty = true;
    send(ctx, command, action);
    Update::none()
}

pub(crate) fn open_mark_prompt(ctx: &mut Context<AppRoot>) -> Update {
    if !target_is_recording(&ctx.state) {
        return Update::none();
    }
    let Some(target) = recording_command_target(&ctx.state) else {
        return Update::none();
    };
    ctx.state.recording_mark = Some(RecordingMarkPrompt {
        target,
        input: TextInput::new(""),
    });
    ctx.state.show_palette = false;
    ctx.state.keybindings = None;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_recording_mark_focus(ctx);
    Update::full()
}

pub(crate) fn mark_prompt_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(prompt) = ctx.state.recording_mark.as_mut() {
        event.apply_to(&mut prompt.input);
    }
    request_recording_mark_focus(ctx);
    Update::full()
}

pub(crate) fn close_mark_prompt(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.recording_mark = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

/// Send the prompt's label. An empty one keeps the prompt open: a mark needs a label.
pub(crate) fn submit_mark_prompt(ctx: &mut Context<AppRoot>) -> Update {
    let Some(prompt) = ctx.state.recording_mark.as_ref() else {
        return Update::none();
    };
    let label = prompt.input.text().trim().to_string();
    if label.is_empty() {
        return Update::none();
    }
    let target = prompt.target;
    let update = close_mark_prompt(ctx);
    send(
        ctx,
        ControlCommand::RecordMark {
            label,
            id: None,
            target: Some(target),
        },
        RecordingAction::Mark(target),
    );
    update
}

fn send(ctx: &mut Context<AppRoot>, command: ControlCommand, action: RecordingAction) {
    crate::ops::attached_control::send(
        ctx,
        ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            extension: None,
        },
        AttachedReply::Action(action),
    );
}

/// Show the session server's answer to one of these commands, sent on attachment `epoch`.
pub(crate) fn answered(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    action: RecordingAction,
    response: ControlResponse,
) -> Update {
    if !response.ok {
        if let RecordingAction::Start(pane) | RecordingAction::Stop(pane) = action {
            let expected = matches!(action, RecordingAction::Start(_));
            let key = (epoch, pane);
            if ctx.state.pending_recording_toggles.get(&key) == Some(&expected) {
                ctx.state.pending_recording_toggles.remove(&key);
                ctx.state.commands_dirty = true;
            }
        }
        let title = match action {
            RecordingAction::Start(_) => "Recording did not start",
            RecordingAction::Stop(_) => "Recording did not stop",
            RecordingAction::Mark(_) => "Mark failed",
        };
        notify_error(ctx, title, response.error.unwrap_or_default());
        return Update::full();
    }
    // A path on another host is shown as that host spells it; `~` would name this user's home.
    let host = ctx
        .state
        .attachment_for_epoch(epoch)
        .and_then(|attachment| attachment.remote_host.clone());
    let shown = |path: &str| match host {
        Some(_) => path.to_string(),
        None => crate::platform::paths::compress_home(path),
    };
    let on_host = host
        .as_deref()
        .map(|host| format!(" on {host}"))
        .unwrap_or_default();
    match action {
        RecordingAction::Start(_) => {
            let path = response
                .data
                .and_then(|data| serde_json::from_value::<RecordingInfo>(data).ok())
                .map(|info| shown(&info.path))
                .unwrap_or_default();
            notify_info(ctx, format!("Recording started{on_host}\n{path}"));
        }
        RecordingAction::Stop(_) => {
            let paths = response
                .data
                .and_then(|data| serde_json::from_value::<RecordingStopList>(data).ok())
                .map(|list| {
                    list.stopped
                        .iter()
                        .map(|stopped| shown(&stopped.path))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            notify_info(ctx, format!("Recording stopped{on_host}\n{paths}"));
        }
        RecordingAction::Mark(pane) => {
            notify_info(ctx, "Recording marked");
            if let Some(end) = blink(ctx, pane) {
                return Update::with_command(end);
            }
        }
    }
    Update::full()
}

/// Highlight `pane`'s recording dot for a moment. The blink is chrome feedback, so it follows the
/// same switches as the focus chrome fades; with them off the mark lands without it.
fn blink(ctx: &mut Context<AppRoot>, pane: PaneId) -> Option<Command> {
    if !crate::layout::anim::screenshot_flash_enabled(ctx.state.config.animations) {
        return None;
    }
    let blink = &mut ctx.state.recording_mark_blink;
    blink.revision = blink.revision.wrapping_add(1);
    blink.pane = Some(pane);
    let revision = blink.revision;
    Some(Command::spawn(move |link: CommandLink<crate::Msg>| {
        link.send_after(MARK_BLINK, crate::Msg::RecordingMarkBlinkEnded { revision });
    }))
}

pub(crate) fn blink_ended(ctx: &mut Context<AppRoot>, revision: u64) -> Update {
    let blink = &mut ctx.state.recording_mark_blink;
    if blink.revision != revision || blink.pane.is_none() {
        return Update::none();
    }
    blink.pane = None;
    Update::full()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tui_lipan::TestBackend;

    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::session::protocol::PaneRuntimeState;

    use super::*;

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    /// A UI attached to a session on `host`, and what it sends that session.
    fn attached(host: Option<&str>) -> (TestBackend<AppRoot>, mpsc::Receiver<ClientOutbound>) {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        state.current_mut().remote_host = host.map(str::to_string);
        (backend, outbound)
    }

    fn sent(outbound: &mpsc::Receiver<ClientOutbound>) -> Vec<(u64, ControlCommand)> {
        outbound
            .try_iter()
            .filter_map(|message| match message {
                ClientOutbound::Control(ClientMessage::AttachedControl {
                    request_id,
                    request,
                }) => Some((request_id, request.command)),
                _ => None,
            })
            .collect()
    }

    fn answer(backend: &mut TestBackend<AppRoot>, request_id: u64, data: impl serde::Serialize) {
        let epoch = backend.state().runtime_epoch;
        backend
            .dispatch(Msg::SessionAttachedControlResult {
                epoch,
                request_id,
                response: ControlResponse::ok(data),
            })
            .unwrap();
    }

    fn toasts(backend: &TestBackend<AppRoot>) -> Vec<String> {
        backend
            .state()
            .replaceable_toasts
            .values()
            .map(|tracked| tracked.content().replace(['\u{0}', '\n'], " "))
            .collect()
    }

    fn set_recording(backend: &mut TestBackend<AppRoot>, recording: bool) -> PaneId {
        let state = backend.state_mut();
        let focused = state.focused_pane().unwrap();
        crate::pane::lifecycle::find_pane_mut(state, focused)
            .unwrap()
            .terminal
            .recording = recording;
        focused
    }

    fn runtime_recording(backend: &mut TestBackend<AppRoot>, recording: bool) {
        let state = backend.state();
        let pane_id = state.focused_pane().unwrap();
        let pane = find_pane(state, pane_id).unwrap();
        let generation = pane.pty_generation;
        let sequence = pane.terminal.runtime_sequence + 1;
        let epoch = state.runtime_epoch;
        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id,
                local: false,
                generation,
                agent_refs: Vec::new(),
                state: PaneRuntimeState {
                    sequence,
                    recording,
                    ..PaneRuntimeState::default()
                },
            })
            .unwrap();
    }

    #[test]
    fn start_and_stop_name_no_file_and_say_which_host_the_file_is_on() {
        on_large_stack(|| {
            let (mut backend, outbound) = attached(Some("devbox"));
            let focused = set_recording(&mut backend, false);

            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(request_id, command)] = sent(&outbound).try_into().unwrap();
            let ControlCommand::RecordStart { target, output, .. } = command else {
                panic!("expected a start, got {command:?}");
            };
            assert_eq!((target, output), (Some(focused), None));
            let path = "/home/dev/.local/state/rozi/recordings/work-pane-1.rozirec";
            answer(
                &mut backend,
                request_id,
                RecordingInfo {
                    id: 1,
                    session: "work".into(),
                    pane: focused,
                    path: path.into(),
                    started_at_unix_ms: 0,
                    elapsed_ms: 0,
                    max_fps: 30,
                    duration_ms: 1,
                    max_bytes: 1,
                    follow: false,
                    totals: Default::default(),
                },
            );
            let expected = format!("Recording started on devbox {path}");
            assert!(
                toasts(&backend).contains(&expected),
                "{:?}",
                toasts(&backend)
            );

            runtime_recording(&mut backend, true);
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(request_id, command)] = sent(&outbound).try_into().unwrap();
            assert_eq!(
                command,
                ControlCommand::RecordStop {
                    id: None,
                    target: Some(focused)
                }
            );
            answer(
                &mut backend,
                request_id,
                RecordingStopList {
                    stopped: vec![crate::control::RecordingStopped {
                        id: 1,
                        pane: focused,
                        path: path.into(),
                        reason: crate::recording::EndReason::Stopped,
                        elapsed_ms: 5,
                        error: None,
                        totals: Default::default(),
                    }],
                },
            );
            let expected = format!("Recording stopped on devbox {path}");
            assert!(
                toasts(&backend).contains(&expected),
                "{:?}",
                toasts(&backend)
            );
        });
    }

    #[test]
    fn toggle_waits_for_runtime_state_before_sending_another_request() {
        on_large_stack(|| {
            let (mut backend, outbound) = attached(Some("devbox"));
            let pane = backend.state().focused_pane().unwrap();
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(start_id, command)] = sent(&outbound).try_into().unwrap();
            assert!(matches!(command, ControlCommand::RecordStart { .. }));
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            assert!(sent(&outbound).is_empty());

            answer(
                &mut backend,
                start_id,
                RecordingInfo {
                    id: 1,
                    session: "work".into(),
                    pane,
                    path: "/tmp/a.rozirec".into(),
                    started_at_unix_ms: 0,
                    elapsed_ms: 0,
                    max_fps: 30,
                    duration_ms: 1,
                    max_bytes: 1,
                    follow: false,
                    totals: Default::default(),
                },
            );
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            assert!(
                sent(&outbound).is_empty(),
                "the reply precedes runtime state"
            );

            runtime_recording(&mut backend, true);
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(stop_id, command)] = sent(&outbound).try_into().unwrap();
            let ControlCommand::RecordStop { target, .. } = command else {
                panic!("expected one stop request");
            };
            assert_eq!(target, Some(pane));
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            assert!(sent(&outbound).is_empty());
            answer(&mut backend, stop_id, RecordingStopList { stopped: vec![] });
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            assert!(
                sent(&outbound).is_empty(),
                "stop stays pending until runtime state"
            );

            runtime_recording(&mut backend, false);
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(_, command)] = sent(&outbound).try_into().unwrap();
            let ControlCommand::RecordStart { target, .. } = command else {
                panic!("expected a new start after the stop completed");
            };
            assert_eq!(target, Some(pane));
        });
    }

    #[test]
    fn a_refused_start_says_why() {
        on_large_stack(|| {
            let (mut backend, outbound) = attached(None);
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            let [(request_id, _)] = sent(&outbound).try_into().unwrap();
            let epoch = backend.state().runtime_epoch;
            backend
                .dispatch(Msg::SessionAttachedControlResult {
                    epoch,
                    request_id,
                    response: ControlResponse::error("cannot create /nope: denied"),
                })
                .unwrap();
            assert!(
                toasts(&backend)
                    .contains(&"Recording did not start cannot create /nope: denied".to_string()),
                "{:?}",
                toasts(&backend)
            );
            backend
                .dispatch(Msg::RunAction(Action::TogglePaneRecording))
                .unwrap();
            assert!(matches!(
                sent(&outbound).as_slice(),
                [(_, ControlCommand::RecordStart { .. })]
            ));
        });
    }

    #[test]
    fn the_mark_prompt_sends_its_label_to_the_pane_and_blinks_the_dot() {
        on_large_stack(|| {
            let (mut backend, outbound) = attached(None);
            let focused = set_recording(&mut backend, true);
            {
                let animations = &mut backend.state_mut().config.animations;
                animations.enabled = true;
                animations.focus_chrome = true;
            }

            backend
                .dispatch(Msg::RunAction(Action::MarkPaneRecording))
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .recording_mark
                    .as_ref()
                    .map(|prompt| prompt.target),
                Some(focused)
            );
            backend.dispatch(Msg::SubmitRecordingMark).unwrap();
            assert!(sent(&outbound).is_empty(), "a mark needs a label");
            assert!(backend.state().recording_mark.is_some());

            backend.state_mut().recording_mark.as_mut().unwrap().input =
                TextInput::new("  tests pass ");
            backend.dispatch(Msg::SubmitRecordingMark).unwrap();
            assert!(backend.state().recording_mark.is_none());
            let [(request_id, command)] = sent(&outbound).try_into().unwrap();
            assert_eq!(
                command,
                ControlCommand::RecordMark {
                    label: "tests pass".into(),
                    id: None,
                    target: Some(focused)
                }
            );

            answer(&mut backend, request_id, serde_json::json!({ "ids": [1] }));
            assert!(toasts(&backend).contains(&"Recording marked".to_string()));
            let blink = &backend.state().recording_mark_blink;
            assert_eq!(blink.pane, Some(focused));
            let revision = blink.revision;
            backend
                .dispatch(Msg::RecordingMarkBlinkEnded {
                    revision: revision.wrapping_sub(1),
                })
                .unwrap();
            assert_eq!(backend.state().recording_mark_blink.pane, Some(focused));
            backend
                .dispatch(Msg::RecordingMarkBlinkEnded { revision })
                .unwrap();
            assert_eq!(backend.state().recording_mark_blink.pane, None);

            backend.state_mut().config.animations.enabled = false;
            backend
                .dispatch(Msg::RunAction(Action::MarkPaneRecording))
                .unwrap();
            backend.state_mut().recording_mark.as_mut().unwrap().input = TextInput::new("again");
            backend.dispatch(Msg::SubmitRecordingMark).unwrap();
            let [(request_id, _)] = sent(&outbound).try_into().unwrap();
            answer(&mut backend, request_id, serde_json::json!({ "ids": [1] }));
            assert_eq!(
                backend.state().recording_mark_blink.pane,
                None,
                "motion off lands a mark without the blink"
            );
        });
    }
}
