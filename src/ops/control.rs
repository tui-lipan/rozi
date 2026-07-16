use serde::Serialize;
use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::actions::execute_action;
use crate::control::{ControlCommand, ControlEnvelope, ControlResponse};
use crate::input::Action;
use crate::ops::focus::{
    focus_pane, move_focused_to_workspace, request_current_pane_focus, request_pane_focus,
    switch_workspace,
};
use crate::pane_lifecycle::{find_pane_mut, spawn_interactive_pane};
use crate::state::{PaneId, PaneIdentity, WORKSPACE_COUNT};

#[derive(Serialize)]
struct PaneInfo {
    id: PaneId,
    title: String,
    workspace: usize,
    command: Option<String>,
    cwd: Option<String>,
    status: String,
    reported_status: Option<String>,
    status_reason: Option<String>,
}

#[derive(Serialize)]
struct NewPaneAccepted {
    id: PaneId,
    accepted: bool,
    pty_ready: bool,
}

#[derive(Serialize)]
struct PaneCapture {
    id: PaneId,
    text: String,
}

pub(crate) fn handle_control_request(
    ctx: &mut Context<HyprmuxApp>,
    envelope: ControlEnvelope,
) -> Update {
    let response = match envelope.request.command {
        ControlCommand::ListPanes => list_panes(ctx),
        ControlCommand::Focus { target } => focus_target(ctx, target),
        ControlCommand::SendText { target, text } => {
            send_text(ctx, target.or(envelope.request.source_pane), text)
        }
        ControlCommand::NewPane {
            command,
            cwd,
            title,
            keep_open,
        } => {
            return new_pane(
                ctx,
                envelope.request.source_pane,
                command,
                cwd,
                title,
                keep_open,
                envelope.reply,
            );
        }
        ControlCommand::RunAction { action } => {
            return run_action(ctx, &action, envelope.reply);
        }
        ControlCommand::CapturePane { target } => {
            capture_pane(ctx, target.or(envelope.request.source_pane))
        }
        ControlCommand::SwitchWorkspace { index } => switch_workspace_command(ctx, index),
        ControlCommand::MoveToWorkspace { index } => move_to_workspace_command(ctx, index),
        ControlCommand::Popup {
            command,
            cwd,
            width,
            height,
            title,
        } => match crate::popup::open(ctx, command, cwd, width, height, title) {
            Ok(update) => {
                let _ = envelope.reply.send(ControlResponse::empty());
                return update;
            }
            Err(error) => ControlResponse::error(error),
        },
        ControlCommand::Subscribe { .. } => {
            ControlResponse::error("subscribe is handled by the control listener")
        }
        ControlCommand::PaneLogging { target, enabled } => {
            let id = target
                .or(envelope.request.source_pane)
                .or(ctx.state.focused_pane);
            match id.and_then(|id| crate::pane_lifecycle::find_pane(&ctx.state, id)) {
                Some(pane) => {
                    if let Some(client) = &ctx.state.session_client {
                        client.set_pane_logging(
                            pane.id,
                            pane.pty_generation,
                            enabled.unwrap_or(!pane.logging),
                        );
                    }
                    ControlResponse::empty()
                }
                None => ControlResponse::error("pane not found"),
            }
        }
        ControlCommand::SetStatus {
            target,
            status,
            reason,
        } => set_status(
            ctx,
            target
                .or(envelope.request.source_pane)
                .or(ctx.state.focused_pane),
            status,
            reason,
        ),
    };
    let _ = envelope.reply.send(response);
    Update::full()
}

fn list_panes(ctx: &Context<HyprmuxApp>) -> ControlResponse {
    let mut panes = Vec::new();
    for (workspace_index, workspace) in ctx.state.workspaces.iter().enumerate() {
        for pane in workspace.panes.iter().filter(|pane| !pane.closing) {
            panes.push(PaneInfo {
                id: pane.id,
                title: pane.display_title(None),
                workspace: workspace_index + 1,
                command: pane.identity.command.clone(),
                cwd: pane.live_cwd().or_else(|| pane.identity.cwd.clone()),
                status: pane.terminal.status_text(),
                reported_status: pane
                    .terminal
                    .reported_status
                    .as_ref()
                    .map(|status| status.value.clone()),
                status_reason: pane
                    .terminal
                    .reported_status
                    .as_ref()
                    .and_then(|status| status.reason.clone()),
            });
        }
    }
    if let Some(pane) = ctx.state.scratch.as_ref().filter(|pane| !pane.closing) {
        panes.push(PaneInfo {
            id: pane.id,
            title: pane.display_title(None),
            workspace: 0,
            command: pane.identity.command.clone(),
            cwd: pane.live_cwd().or_else(|| pane.identity.cwd.clone()),
            status: pane.terminal.status_text(),
            reported_status: pane
                .terminal
                .reported_status
                .as_ref()
                .map(|status| status.value.clone()),
            status_reason: pane
                .terminal
                .reported_status
                .as_ref()
                .and_then(|status| status.reason.clone()),
        });
    }
    ControlResponse::ok(panes)
}

fn set_status(
    ctx: &mut Context<HyprmuxApp>,
    target: Option<PaneId>,
    status: Option<String>,
    reason: Option<String>,
) -> ControlResponse {
    let Some(id) = target else {
        return ControlResponse::error("no target pane and no focused pane");
    };
    let Some(pane) = crate::pane_lifecycle::find_pane(&ctx.state, id).filter(|pane| !pane.closing)
    else {
        return ControlResponse::error(format!("pane {id} not found"));
    };
    let generation = pane.pty_generation;
    if !ctx.state.session_attached {
        return ControlResponse::error(format!("pane {id} session is not attached"));
    }
    if ctx
        .state
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        return ControlResponse::error("attached read-only");
    }
    let Some(client) = ctx.state.session_client.clone() else {
        return ControlResponse::error(format!("pane {id} session is not connected"));
    };
    client.set_pane_status(id, generation, status, reason);
    ControlResponse::empty()
}

fn focus_target(ctx: &mut Context<HyprmuxApp>, target: PaneId) -> ControlResponse {
    let Some((workspace_index, _)) = ctx
        .state
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.panes.iter().any(|p| p.id == target && !p.closing))
    else {
        return ControlResponse::error(format!("pane {target} not found"));
    };
    ctx.state.active_workspace = workspace_index;
    focus_pane(&mut ctx.state, target);
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.activity.has_unseen_output = false;
    }
    request_pane_focus(ctx, target);
    ControlResponse::empty()
}

fn send_text(
    ctx: &mut Context<HyprmuxApp>,
    target: Option<PaneId>,
    text: String,
) -> ControlResponse {
    if let Some(reason) = ctx.state.pane_input_block_reason() {
        return ControlResponse::error(reason);
    }
    let id = target.or(ctx.state.focused_pane);
    let Some(id) = id else {
        return ControlResponse::error("no target pane and no focused pane");
    };
    let client = ctx.state.session_client.clone();
    let Some(pane) = find_pane_mut(&mut ctx.state, id).filter(|pane| !pane.closing) else {
        return ControlResponse::error(format!("pane {id} not found"));
    };
    if !pane.terminal.accepts_input() {
        return ControlResponse::error(format!("pane {id} PTY is not ready"));
    }
    let Some(client) = client else {
        return ControlResponse::error(format!("pane {id} session is not connected"));
    };
    client.send_input(id, pane.pty_generation, text.into_bytes());
    ControlResponse::empty()
}

/// Run any keybindable action by its stable id, the same names used in `[keys]` config and the
/// command palette (see `Action::id`/`Action::from_id`).
fn run_action(
    ctx: &mut Context<HyprmuxApp>,
    action_id: &str,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let Some(action) = Action::from_id(action_id) else {
        let _ = reply.send(ControlResponse::error(format!(
            "unknown action `{action_id}`"
        )));
        return Update::full();
    };
    if crate::actions::is_layout_mutating(&ctx.state, action) && !ctx.state.is_controller() {
        let _ = reply.send(ControlResponse::error("not controller"));
        return Update::full();
    }
    let update = execute_action(ctx, action);
    let _ = reply.send(ControlResponse::empty());
    update
}

fn capture_pane(ctx: &mut Context<HyprmuxApp>, target: Option<PaneId>) -> ControlResponse {
    let Some(id) = target.or(ctx.state.focused_pane) else {
        return ControlResponse::error("no target pane and no focused pane");
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return ControlResponse::error(format!("pane {id} not found"));
    };
    ControlResponse::ok(PaneCapture {
        id,
        text: pane.terminal.capture_text(),
    })
}

fn switch_workspace_command(ctx: &mut Context<HyprmuxApp>, index: usize) -> ControlResponse {
    let Some(response) = validate_workspace_index(index) else {
        switch_workspace(&mut ctx.state, index - 1);
        request_current_pane_focus(ctx);
        return ControlResponse::empty();
    };
    response
}

fn move_to_workspace_command(ctx: &mut Context<HyprmuxApp>, index: usize) -> ControlResponse {
    if !ctx.state.is_controller() {
        return ControlResponse::error("not controller");
    }
    let Some(response) = validate_workspace_index(index) else {
        move_focused_to_workspace(&mut ctx.state, index - 1);
        request_current_pane_focus(ctx);
        return ControlResponse::empty();
    };
    response
}

/// `Some(error response)` when `index` (1-based) is out of the `1..=WORKSPACE_COUNT` range,
/// `None` when it is valid.
fn validate_workspace_index(index: usize) -> Option<ControlResponse> {
    if index == 0 || index > WORKSPACE_COUNT {
        Some(ControlResponse::error(format!(
            "workspace index must be between 1 and {WORKSPACE_COUNT}"
        )))
    } else {
        None
    }
}

fn new_pane(
    ctx: &mut Context<HyprmuxApp>,
    source: Option<PaneId>,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    if !ctx.state.is_controller() {
        let _ = reply.send(ControlResponse::error("not controller"));
        return Update::full();
    }
    let source_workspace = match workspace_for_source(&ctx.state, source) {
        Ok(index) => index,
        Err(message) => {
            let _ = reply.send(ControlResponse::error(message));
            return Update::full();
        }
    };
    let mut identity = PaneIdentity {
        command,
        cwd,
        keep_open,
        ..PaneIdentity::default()
    };
    if let Some(title) = title {
        identity.set_custom_title(title);
    }
    let (id, update) = spawn_interactive_pane(ctx, source_workspace, source, identity);
    let _ = reply.send(ControlResponse::ok(NewPaneAccepted {
        id,
        accepted: true,
        pty_ready: false,
    }));
    update
}

fn workspace_for_source(
    state: &crate::state::State,
    source: Option<PaneId>,
) -> std::result::Result<usize, String> {
    match source {
        Some(id) => state
            .workspaces
            .iter()
            .position(|ws| ws.panes.iter().any(|p| p.id == id && !p.closing))
            .ok_or_else(|| format!("source pane {id} not found")),
        None => Ok(state.active_workspace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlCommand, ControlEnvelope, ControlRequest};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::{Pane, State};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    #[test]
    fn workspace_for_source_errors_on_invalid_explicit_source() {
        let state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        assert_eq!(
            workspace_for_source(&state, Some(999)),
            Err("source pane 999 not found".to_string())
        );
    }

    #[test]
    fn workspace_for_source_falls_back_only_without_source() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.active_workspace = 2;
        state.workspaces[1].panes.push(Pane::new(7, 100, rect()));
        assert_eq!(workspace_for_source(&state, None), Ok(2));
        assert_eq!(workspace_for_source(&state, Some(7)), Ok(1));
    }

    #[test]
    fn validate_workspace_index_rejects_out_of_range() {
        assert!(validate_workspace_index(0).is_some());
        assert!(validate_workspace_index(1).is_none());
        assert!(validate_workspace_index(crate::state::WORKSPACE_COUNT).is_none());
        assert!(validate_workspace_index(crate::state::WORKSPACE_COUNT + 1).is_some());
    }

    #[test]
    fn writable_follower_can_queue_status_and_read_only_client_cannot() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let (client, outbound) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    state.workspaces[0].panes[0].pty_generation = 9;
                    let mut shared = crate::state::SharedSessionState::new(1);
                    shared.controller = Some(2);
                    state.shared = Some(shared);
                }
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SetStatus {
                                target: Some(1),
                                status: Some("blocked".into()),
                                reason: Some("waiting".into()),
                            },
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch writable status request");
                assert!(response.recv().unwrap().ok);
                assert!(outbound.try_iter().any(|message| matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::SetPaneStatus {
                        pane_id: 1,
                        generation: 9,
                        status: Some(status),
                        reason: Some(reason),
                    }) if status == "blocked" && reason == "waiting"
                )));

                backend.state_mut().shared.as_mut().unwrap().read_only = true;
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SetStatus {
                                target: Some(1),
                                status: None,
                                reason: None,
                            },
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch read-only status request");
                let response = response.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("attached read-only"));
                assert!(outbound.try_recv().is_err());
            })
            .expect("spawn control status test thread")
            .join()
            .expect("control status test thread completes");
    }

    #[test]
    fn list_panes_keeps_terminal_status_and_adds_reported_status_fields() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                backend.state_mut().workspaces[0].panes[0]
                    .terminal
                    .reported_status = Some(crate::session::protocol::PaneStatus {
                    value: "working".into(),
                    reason: Some("building".into()),
                    set_at: 1,
                });
                backend.state_mut().workspaces[0].panes[0].terminal.status =
                    ManagedTerminalStatus::Ready;
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::ListPanes,
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch list panes");
                let data = response.recv().unwrap().data.unwrap();
                assert!(data[0]["status"].is_string());
                assert_ne!(data[0]["status"], "working");
                assert_eq!(data[0]["reported_status"], "working");
                assert_eq!(data[0]["status_reason"], "building");
            })
            .expect("spawn list panes test thread")
            .join()
            .expect("list panes test thread completes");
    }
}
