use serde::Serialize;
use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::actions::execute_action;
use crate::control::{ControlCommand, ControlEnvelope, ControlResponse};
use crate::focus_ops::{
    focus_pane, move_focused_to_workspace, request_current_pane_focus, request_pane_focus,
    switch_workspace,
};
use crate::input::Action;
use crate::pane_lifecycle::{find_pane_mut, spawn_pane_in_workspace};
use crate::state::{PaneId, PaneIdentity, WORKSPACE_COUNT};

#[derive(Serialize)]
struct PaneInfo {
    id: PaneId,
    title: String,
    workspace: usize,
    command: Option<String>,
    cwd: Option<String>,
    status: String,
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
        });
    }
    ControlResponse::ok(panes)
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
    let workspace_index = match workspace_for_source(&ctx.state, source) {
        Ok(index) => index,
        Err(message) => {
            let _ = reply.send(ControlResponse::error(message));
            return Update::full();
        }
    };
    let previous_focused = source.or(ctx.state.workspaces[workspace_index].focused_pane);
    let mut identity = PaneIdentity {
        command,
        cwd,
        keep_open,
        ..PaneIdentity::default()
    };
    if let Some(title) = title {
        identity.set_custom_title(title);
    }
    let (id, update) = spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity);
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
    use crate::state::{Pane, State};

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
}
