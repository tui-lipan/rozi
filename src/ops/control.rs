use serde::Serialize;
use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::actions::execute_action;
use crate::control::{
    CaptureScrollback, CaptureScrollbackNamed, ControlCommand, ControlEnvelope, ControlResponse,
};
use crate::input::Action;
use crate::ops::focus::{
    focus_pane_anywhere, move_focused_to_workspace, request_current_pane_focus, switch_workspace,
};
use crate::pane_lifecycle::{find_pane_mut, spawn_interactive_pane_with_focus};
use crate::pty_events::terminal_key_event_bytes;
use crate::send_keys::{SendKeysItem, parse_send_keys_arg};
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
pub(crate) struct NewPaneAccepted {
    pub id: PaneId,
    pub accepted: bool,
    pub pty_ready: bool,
}

#[derive(Serialize)]
struct PaneCapture {
    id: PaneId,
    text: String,
}

pub(crate) fn handle_control_request(
    ctx: &mut Context<AppRoot>,
    envelope: ControlEnvelope,
) -> Update {
    let response = match envelope.request.command {
        ControlCommand::ListPanes => list_panes(ctx),
        ControlCommand::Metrics => {
            let response = runtime_metrics(ctx);
            let _ = envelope.reply.send(response);
            return Update::none();
        }
        ControlCommand::Focus { target } => focus_target(ctx, target),
        ControlCommand::SendText { target, text } => {
            send_text(ctx, target.or(envelope.request.source_pane), text)
        }
        ControlCommand::SendKeys {
            target,
            keys,
            literal,
        } => send_keys(ctx, target.or(envelope.request.source_pane), keys, literal),
        ControlCommand::NewPane {
            command,
            cwd,
            title,
            keep_open,
            focus,
        } => {
            return new_pane(
                ctx,
                envelope.request.source_pane,
                command,
                cwd,
                title,
                keep_open,
                focus,
                envelope.reply,
            );
        }
        ControlCommand::RunAction { action } => {
            return run_action(ctx, &action, envelope.reply);
        }
        ControlCommand::CapturePane { target, scrollback } => {
            capture_pane(ctx, target.or(envelope.request.source_pane), scrollback)
        }
        ControlCommand::Notify {
            message,
            title,
            level,
        } => notify_command(ctx, message, title, level),
        ControlCommand::SwitchWorkspace { index } => switch_workspace_command(ctx, index),
        ControlCommand::MoveToWorkspace { index } => move_to_workspace_command(ctx, index),
        ControlCommand::Popup {
            command,
            cwd,
            width,
            height,
            title,
            keep_open,
        } => {
            let keep_open = keep_open.unwrap_or(true);
            if let Some(update) = crate::ops::session::ensure_session_for_pty(
                ctx,
                crate::state::PendingSessionAction::Popup {
                    command: command.clone(),
                    cwd: cwd.clone(),
                    width,
                    height,
                    title: title.clone(),
                    keep_open,
                },
            ) {
                ctx.state.pending_control_reply = Some(envelope.reply);
                return update;
            }
            match crate::popup::open(
                ctx,
                command,
                cwd,
                width,
                height,
                title,
                keep_open,
                Vec::new(),
            ) {
                Ok(update) => {
                    let _ = envelope.reply.send(ControlResponse::empty());
                    return update;
                }
                Err(error) => ControlResponse::error(error),
            }
        }
        ControlCommand::Subscribe { .. } => {
            ControlResponse::error("subscribe is handled by the control listener")
        }
        ControlCommand::Publish => {
            ControlResponse::error("publish is handled by the control listener")
        }
        ControlCommand::Pick { .. } => {
            ControlResponse::error("pick is handled by the control listener")
        }
        ControlCommand::PaneLogging { target, enabled } => {
            let id = target
                .or(envelope.request.source_pane)
                .or(ctx.state.focused_pane());
            match id.and_then(|id| crate::pane_lifecycle::find_pane(&ctx.state, id)) {
                Some(pane) => {
                    if let Some(client) = &ctx.state.current().session_client {
                        client.set_pane_logging(
                            pane.id,
                            pane.pty_generation,
                            crate::pane_lifecycle::pane_is_local(&ctx.state, pane.id),
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
                .or(ctx.state.focused_pane()),
            status,
            reason,
        ),
    };
    let _ = envelope.reply.send(response);
    Update::full()
}

fn runtime_metrics(ctx: &Context<AppRoot>) -> ControlResponse {
    if let Some(client) = &ctx.state.current().session_client {
        // Refresh asynchronously for the next sample; this response always uses the cache.
        client.request_runtime_metrics();
    }
    ControlResponse::ok(crate::runtime_metrics::RuntimeMetrics::capture(
        ctx.state.current(),
    ))
}

fn list_panes(ctx: &Context<AppRoot>) -> ControlResponse {
    let mut panes = Vec::new();
    for (workspace_index, workspace) in ctx.state.current().workspaces.iter().enumerate() {
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
    for pane in ctx.state.scratch.panes.iter().filter(|pane| !pane.closing) {
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
    ctx: &mut Context<AppRoot>,
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
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    if !ctx.state.current().session_attached {
        return ControlResponse::error(format!("pane {id} session is not attached"));
    }
    if ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        return ControlResponse::error("attached read-only");
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return ControlResponse::error(format!("pane {id} session is not connected"));
    };
    client.set_pane_status(id, generation, local, status, reason);
    ControlResponse::empty()
}

fn focus_target(ctx: &mut Context<AppRoot>, target: PaneId) -> ControlResponse {
    if crate::scratchpad::contains(&ctx.state, target) {
        if !ctx.state.scratch_visible {
            return ControlResponse::error("scratchpad is hidden");
        }
        crate::ops::focus::focus_pane(&mut ctx.state, target);
        crate::ops::focus::request_pane_focus(ctx, target);
        return ControlResponse::empty();
    }
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
    if !focus_pane_anywhere(ctx, target) {
        return ControlResponse::error(format!("pane {target} not found"));
    }
    ControlResponse::empty()
}

/// A resolved `send-text` / `send-keys` destination.
struct InputTarget {
    id: PaneId,
    generation: u64,
    local: bool,
    modes: TerminalKeyModes,
    /// The PTY accepts input now. When false the spawn is still in flight and bytes are queued as
    /// type-ahead instead.
    starting: bool,
    client: Option<crate::session::client::SessionClient>,
}

/// Resolve and validate the pane a control input request targets.
///
/// A pane whose PTY is still starting is a valid target: its bytes are queued rather than rejected,
/// which is what a person typing into a freshly split pane already gets. A pane that has exited or
/// failed is not — those keep failing loudly, because nothing will ever read the input.
fn control_input_target(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
) -> std::result::Result<InputTarget, ControlResponse> {
    if let Some(reason) = ctx.state.pane_input_block_reason() {
        return Err(ControlResponse::error(reason));
    }
    let Some(id) = target.or(ctx.state.focused_pane()) else {
        return Err(ControlResponse::error("no target pane and no focused pane"));
    };
    let client = ctx.state.current().session_client.clone();
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    let Some(pane) = find_pane_mut(&mut ctx.state, id).filter(|pane| !pane.closing) else {
        return Err(ControlResponse::error(format!("pane {id} not found")));
    };
    let ready = pane.terminal.accepts_input();
    if !ready && !pane.terminal.is_running() {
        return Err(ControlResponse::error(format!(
            "pane {id} PTY is not running"
        )));
    }
    if ready && client.is_none() {
        return Err(ControlResponse::error(format!(
            "pane {id} session is not connected"
        )));
    }
    Ok(InputTarget {
        id,
        generation: pane.pty_generation,
        local,
        modes: pane.terminal.snapshot().key_modes,
        starting: !ready,
        client,
    })
}

/// Write control input to the PTY, or append it to the pane's type-ahead queue while the spawn is
/// still in flight (see [`crate::state::State::pending_control_input`]).
fn deliver_control_input(ctx: &mut Context<AppRoot>, target: &InputTarget, bytes: Vec<u8>) {
    if target.starting {
        ctx.state
            .pending_control_input
            .entry((target.local, target.id, target.generation))
            .or_default()
            .extend(bytes);
        return;
    }
    if let Some(client) = target.client.as_ref() {
        client.send_input(target.id, target.generation, target.local, bytes);
    }
}

fn send_text(ctx: &mut Context<AppRoot>, target: Option<PaneId>, text: String) -> ControlResponse {
    let target = match control_input_target(ctx, target) {
        Ok(target) => target,
        Err(response) => return response,
    };
    deliver_control_input(ctx, &target, text.into_bytes());
    ControlResponse::empty()
}

fn send_keys(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    keys: Vec<String>,
    literal: bool,
) -> ControlResponse {
    let target = match control_input_target(ctx, target) {
        Ok(target) => target,
        Err(response) => return response,
    };

    // Encode every argument before writing any so invalid input never reaches the PTY, and so a
    // queued batch is all-or-nothing rather than half-written when a later key is unrepresentable.
    let mut bytes = Vec::new();
    for key in &keys {
        let item = match parse_send_keys_arg(key, literal) {
            Ok(item) => item,
            Err(message) => return ControlResponse::error(message),
        };
        match item {
            SendKeysItem::Text(text) => bytes.extend(text.into_bytes()),
            SendKeysItem::Key(event) => {
                let Some(encoded) = terminal_key_event_bytes(event, target.modes) else {
                    return ControlResponse::error(
                        "key is not representable for session forwarding yet",
                    );
                };
                bytes.extend(encoded);
            }
        }
    }

    deliver_control_input(ctx, &target, bytes);
    ControlResponse::empty()
}

/// Run any keybindable action by its stable id, the same names used in `[keys]` config and the
/// command palette (see `Action::id`/`Action::from_id`).
fn run_action(
    ctx: &mut Context<AppRoot>,
    action_id: &str,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let Some(action) = Action::from_id(action_id) else {
        let _ = reply.send(ControlResponse::error(format!(
            "unknown action `{action_id}`"
        )));
        return Update::full();
    };
    if crate::actions::is_layout_mutating(&ctx.state, action)
        && !ctx.state.scratch_visible
        && !ctx.state.is_controller()
    {
        let _ = reply.send(ControlResponse::error("not controller"));
        return Update::full();
    }
    if crate::actions::is_blocked_by_scratchpad(&ctx.state, action) {
        let _ = reply.send(ControlResponse::error("scratchpad is open"));
        return Update::none();
    }
    // Leaving is interactive: it can raise the prompt that asks whether to keep a temporary
    // session, and there is nobody on this socket to answer it. A scripted exit takes the
    // preserving path instead, so automation can never be what closes a session.
    if matches!(action, Action::Quit | Action::Detach) {
        let update = crate::ops::exit::leave_client_unattended(ctx);
        let _ = reply.send(ControlResponse::empty());
        return update;
    }
    let update = execute_action(ctx, action);
    let _ = reply.send(ControlResponse::empty());
    update
}

fn capture_pane(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    scrollback: Option<CaptureScrollback>,
) -> ControlResponse {
    let Some(id) = target.or(ctx.state.focused_pane()) else {
        return ControlResponse::error("no target pane and no focused pane");
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return ControlResponse::error(format!("pane {id} not found"));
    };
    let text = match scrollback {
        None => pane.terminal.capture_text(),
        Some(CaptureScrollback::Lines(n)) => pane.terminal.capture_scrollback_text(Some(n)),
        Some(CaptureScrollback::Named(CaptureScrollbackNamed::Full)) => {
            pane.terminal.capture_scrollback_text(None)
        }
        Some(CaptureScrollback::Named(CaptureScrollbackNamed::LastOutput)) => {
            match pane.terminal.capture_last_command_output() {
                Some(text) => text,
                None => {
                    return ControlResponse::error(
                        "no last command output (shell integration marks missing)",
                    );
                }
            }
        }
    };
    ControlResponse::ok(PaneCapture { id, text })
}

/// Raise a toast on behalf of a script.
///
/// Empty messages are rejected rather than shown: a blank toast is a bug in the caller, and it
/// would still occupy the slot a real message needs.
fn notify_command(
    ctx: &mut Context<AppRoot>,
    message: String,
    title: Option<String>,
    level: crate::control::NotifyLevel,
) -> ControlResponse {
    let message = message.trim().to_string();
    if message.is_empty() {
        return ControlResponse::error("notify requires a message");
    }
    match level {
        // `title` is meaningful only here: a titled toast is drawn in the error style, so an
        // `info` carrying one would read as a failure. Info stays the single-line form.
        crate::control::NotifyLevel::Error => {
            crate::pty_events::notify_error(
                ctx,
                title.unwrap_or_else(|| "Notice".to_string()),
                message,
            );
        }
        crate::control::NotifyLevel::Info => {
            crate::pty_events::notify_info(ctx, message);
        }
    }
    ControlResponse::empty()
}

fn switch_workspace_command(ctx: &mut Context<AppRoot>, index: usize) -> ControlResponse {
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
    let Some(response) = validate_workspace_index(index) else {
        switch_workspace(&mut ctx.state, index - 1);
        request_current_pane_focus(ctx);
        return ControlResponse::empty();
    };
    response
}

fn move_to_workspace_command(ctx: &mut Context<AppRoot>, index: usize) -> ControlResponse {
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
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

#[allow(clippy::too_many_arguments)]
fn new_pane(
    ctx: &mut Context<AppRoot>,
    source: Option<PaneId>,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let scratch_source = source.is_some_and(|id| crate::scratchpad::contains(&ctx.state, id));
    if ctx.state.scratch_visible && source.is_some() && !scratch_source {
        let _ = reply.send(ControlResponse::error(
            "source pane is hidden behind scratchpad",
        ));
        return Update::full();
    }
    if !scratch_source && !ctx.state.is_controller() {
        let _ = reply.send(ControlResponse::error("not controller"));
        return Update::full();
    }
    if let Some(update) = crate::ops::session::ensure_session_for_pty(
        ctx,
        crate::state::PendingSessionAction::NewPane {
            source,
            command: command.clone(),
            cwd: cwd.clone(),
            title: title.clone(),
            keep_open,
            focus,
        },
    ) {
        ctx.state.pending_control_reply = Some(reply);
        return update;
    }
    if scratch_source || (ctx.state.scratch_visible && source.is_none()) {
        let mut identity = PaneIdentity {
            command,
            cwd,
            keep_open,
            ..PaneIdentity::default()
        };
        if let Some(title) = title {
            identity.set_custom_title(title);
        }
        let previous = source.or(ctx.state.scratch.focused_pane);
        let (id, update) = crate::pane_lifecycle::spawn_pane_in_scratch(ctx, previous, identity);
        if !focus && let Some(previous) = previous {
            crate::ops::focus::focus_pane(&mut ctx.state, previous);
        }
        hold_spawn_reply(ctx, id, reply);
        return update;
    }
    let source_workspace = match workspace_for_source(&ctx.state, source) {
        Ok(index) => index,
        Err(message) => {
            let _ = reply.send(ControlResponse::error(message));
            return Update::full();
        }
    };
    let (id, update) = spawn_new_pane(
        ctx,
        source_workspace,
        source,
        command,
        cwd,
        title,
        keep_open,
        focus,
    );
    hold_spawn_reply(ctx, id, reply);
    update
}

/// How long a held `new-pane` reply waits for the PTY before answering `pty_ready:false` anyway.
/// The control connection gives up at 10s (see [`crate::control::handle_connection`]), so this has
/// to leave room for the reply to travel back; a remote spawn over a slow SSH link is the case that
/// actually uses the budget.
const SPAWN_READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Park a `new-pane` reply until the pane's PTY reports ready, so the answer describes readiness
/// rather than acceptance. Falls back to answering immediately when there is no command link to arm
/// the deadline with — without one nothing would ever release the reply.
pub(crate) fn hold_spawn_reply(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) {
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    let generation = crate::pane_lifecycle::find_pane_in_namespace(&ctx.state, id, local)
        .map(|pane| pane.pty_generation);
    let (Some(generation), Some(link)) = (generation, ctx.state.command_link.clone()) else {
        let _ = reply.send(ControlResponse::ok(NewPaneAccepted {
            id,
            accepted: true,
            pty_ready: false,
        }));
        return;
    };
    let epoch = ctx.state.runtime_epoch;
    ctx.state
        .pending_spawn_replies
        .insert((epoch, local, id, generation), reply);
    link.send_after(
        SPAWN_READY_DEADLINE,
        crate::Msg::SpawnReplyDeadline {
            epoch,
            pane_id: id,
            local,
            generation,
        },
    );
}

/// Answer a held `new-pane` reply. `ready` is the pane's real PTY state; `error` replaces the
/// success payload when the spawn failed outright.
pub(crate) fn resolve_spawn_reply(
    state: &mut crate::state::State,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    ready: bool,
    error: Option<&str>,
) {
    let Some(reply) = state
        .pending_spawn_replies
        .remove(&(epoch, local, pane_id, generation))
    else {
        return;
    };
    let _ = reply.send(match error {
        Some(message) => ControlResponse::error(message),
        None => ControlResponse::ok(NewPaneAccepted {
            id: pane_id,
            accepted: true,
            pty_ready: ready,
        }),
    });
}

/// Spawn a pane once a session client is available (shared by the live control path and the
/// deferred launcher replay).
pub(crate) fn new_pane_after_session(
    ctx: &mut Context<AppRoot>,
    source: Option<PaneId>,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
) -> (PaneId, Update) {
    if !ctx.state.is_controller() {
        return (0, Update::full());
    }
    let Ok(source_workspace) = workspace_for_source(&ctx.state, source) else {
        return (0, Update::full());
    };
    spawn_new_pane(
        ctx,
        source_workspace,
        source,
        command,
        cwd,
        title,
        keep_open,
        focus,
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_new_pane(
    ctx: &mut Context<AppRoot>,
    source_workspace: usize,
    source: Option<PaneId>,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
) -> (PaneId, Update) {
    let mut identity = PaneIdentity {
        command,
        cwd,
        keep_open,
        ..PaneIdentity::default()
    };
    if let Some(title) = title {
        identity.set_custom_title(title);
    }
    spawn_interactive_pane_with_focus(ctx, source_workspace, source, identity, Some(focus))
}

fn workspace_for_source(
    state: &crate::state::State,
    source: Option<PaneId>,
) -> std::result::Result<usize, String> {
    match source {
        Some(id) => state
            .current()
            .workspaces
            .iter()
            .position(|ws| ws.panes.iter().any(|p| p.id == id && !p.closing))
            .ok_or_else(|| format!("source pane {id} not found")),
        None => Ok(state.current().active_workspace),
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
        let state = State::new(crate::config::Config::default(), Theme::default());
        assert_eq!(
            workspace_for_source(&state, Some(999)),
            Err("source pane 999 not found".to_string())
        );
    }

    #[test]
    fn workspace_for_source_falls_back_only_without_source() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().active_workspace = 2;
        state.current_mut().workspaces[1]
            .panes
            .push(Pane::new(7, 100, rect()));
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
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (client, outbound) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    state.current_mut().workspaces[0].panes[0].pty_generation = 9;
                    let mut shared = crate::state::SharedSessionState::new(1);
                    shared.controller = Some(2);
                    state.current_mut().shared = Some(shared);
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
                local: false,
                            generation: 9,
                            status: Some(status),
                            reason: Some(reason),
                        }) if status == "blocked" && reason == "waiting"
                    )));

                backend
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_mut()
                    .unwrap()
                    .read_only = true;
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

    /// A backend whose mount has delivered the command link, which `hold_spawn_reply` needs to arm
    /// its deadline; without one it answers immediately and the held-reply behavior never runs.
    fn settled_backend() -> TestBackend<crate::AppRoot> {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the mount never delivered the command link"
            );
            backend.pump().expect("settle the mount");
            std::thread::yield_now();
        }
        backend
    }

    fn attach_test_session(
        backend: &mut TestBackend<crate::AppRoot>,
    ) -> mpsc::Receiver<ClientOutbound> {
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        outbound
    }

    fn new_pane_request(
        focus: bool,
    ) -> (
        ControlEnvelope,
        mpsc::Receiver<crate::control::ControlResponse>,
    ) {
        let (reply, response) = mpsc::channel();
        (
            ControlEnvelope {
                request: ControlRequest {
                    command: ControlCommand::NewPane {
                        command: None,
                        cwd: None,
                        title: None,
                        keep_open: false,
                        focus,
                    },
                    source_pane: None,
                },
                reply,
            },
            response,
        )
    }

    #[test]
    fn new_pane_leaves_focus_put_and_answers_only_once_the_pty_is_ready() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                let focused_before = backend.state().current().focused_pane;

                let (envelope, response) = new_pane_request(false);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane");

                // Acceptance alone must not answer: the caller is told about readiness, not about
                // the request having been received.
                assert!(
                    response.try_recv().is_err(),
                    "reply was sent before the PTY reported ready"
                );
                assert_eq!(
                    backend.state().current().focused_pane,
                    focused_before,
                    "an automation spawn moved focus"
                );

                let spawned = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .map(|pane| (pane.id, pane.pty_generation))
                    .max_by_key(|(id, _)| *id)
                    .expect("the pane was created");
                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: spawned.0,
                        local: false,
                        generation: spawned.1,
                        pid: Some(4242),
                        ok: true,
                        error: None,
                    })
                    .expect("dispatch spawn result");

                let response = response.try_recv().expect("reply released by spawn result");
                assert!(response.ok);
                let data = response.data.unwrap();
                assert_eq!(data["id"], spawned.0);
                assert_eq!(data["pty_ready"], true);
                assert_eq!(
                    backend.state().current().focused_pane,
                    focused_before,
                    "focus moved when the spawn completed"
                );
            })
            .expect("spawn new-pane readiness test thread")
            .join()
            .expect("new-pane readiness test thread completes");
    }

    #[test]
    fn new_pane_with_focus_moves_focus_to_the_new_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                let focused_before = backend.state().current().focused_pane;

                let (envelope, _response) = new_pane_request(true);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane --focus");

                let focused_after = backend.state().current().focused_pane;
                assert_ne!(focused_after, focused_before);
                assert!(focused_after.is_some());
            })
            .expect("spawn new-pane focus test thread")
            .join()
            .expect("new-pane focus test thread completes");
    }

    /// A blank toast would occupy the slot a real message needs, so it is refused rather than
    /// drawn empty.
    #[test]
    fn notify_refuses_an_empty_message() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                for blank in ["", "   "] {
                    let (tx, rx) = std::sync::mpsc::channel();
                    backend
                        .dispatch(crate::Msg::ControlRequest(
                            crate::control::ControlEnvelope {
                                request: crate::control::ControlRequest {
                                    command: crate::control::ControlCommand::Notify {
                                        message: blank.to_string(),
                                        title: None,
                                        level: crate::control::NotifyLevel::Info,
                                    },
                                    source_pane: None,
                                },
                                reply: tx,
                            },
                        ))
                        .expect("dispatch notify");
                    let response = rx.try_recv().expect("answered");
                    assert!(!response.ok, "an empty message was accepted");
                    assert_eq!(response.error.as_deref(), Some("notify requires a message"));
                }

                let (tx, rx) = std::sync::mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(
                        crate::control::ControlEnvelope {
                            request: crate::control::ControlRequest {
                                command: crate::control::ControlCommand::Notify {
                                    message: "deploy finished".into(),
                                    title: None,
                                    level: crate::control::NotifyLevel::Info,
                                },
                                source_pane: None,
                            },
                            reply: tx,
                        },
                    ))
                    .expect("dispatch notify");
                assert!(rx.try_recv().expect("answered").ok);
            })
            .expect("spawn notify test thread")
            .join()
            .expect("notify test thread completes");
    }

    #[test]
    fn a_failed_spawn_answers_the_held_new_pane_reply_with_an_error() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);

                let (envelope, response) = new_pane_request(false);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane");
                let spawned = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .map(|pane| (pane.id, pane.pty_generation))
                    .max_by_key(|(id, _)| *id)
                    .expect("the pane was created");
                let epoch = backend.state().runtime_epoch;

                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: spawned.0,
                        local: false,
                        generation: spawned.1,
                        pid: None,
                        ok: false,
                        error: Some("no such file".into()),
                    })
                    .expect("dispatch failed spawn result");

                let response = response.try_recv().expect("reply released by spawn result");
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("no such file"));
            })
            .expect("spawn failed-spawn test thread")
            .join()
            .expect("failed-spawn test thread completes");
    }

    #[test]
    fn input_for_a_starting_pane_is_queued_and_flushed_once_the_pty_is_ready() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                let outbound = attach_test_session(&mut backend);
                {
                    let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                    pane.pty_generation = 4;
                    pane.terminal.status = ManagedTerminalStatus::Starting;
                }

                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SendText {
                                target: Some(1),
                                text: "cargo test\n".into(),
                            },
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch send-text at a starting pane");
                assert!(response.recv().unwrap().ok);
                assert!(
                    outbound.try_recv().is_err(),
                    "input reached the PTY before it was ready"
                );

                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: 1,
                        local: false,
                        generation: 4,
                        pid: Some(11),
                        ok: true,
                        error: None,
                    })
                    .expect("dispatch spawn result");

                assert!(outbound.try_iter().any(|message| matches!(
                    message,
                    ClientOutbound::PaneInput {
                        pane_id: 1,
                        generation: 4,
                        ref bytes,
                        ..
                    } if bytes == b"cargo test\n"
                )));
            })
            .expect("spawn queued-input test thread")
            .join()
            .expect("queued-input test thread completes");
    }

    #[test]
    fn input_for_a_pane_that_is_not_running_still_fails() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .status = ManagedTerminalStatus::Exited(0);

                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SendText {
                                target: Some(1),
                                text: "hi".into(),
                            },
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch send-text at an exited pane");
                let response = response.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("pane 1 PTY is not running"));
            })
            .expect("spawn exited-pane input test thread")
            .join()
            .expect("exited-pane input test thread completes");
    }

    #[test]
    fn list_panes_keeps_terminal_status_and_adds_reported_status_fields() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .reported_status = Some(crate::session::protocol::PaneStatus {
                    value: "working".into(),
                    reason: Some("building".into()),
                    set_at: 1,
                });
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .status = ManagedTerminalStatus::Ready;
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

    #[test]
    fn metrics_control_is_render_neutral_and_returns_cached_shape_without_waiting() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (reply, response) = mpsc::channel();
                let level = backend
                    .update_level(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::Metrics,
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("update metrics");
                assert_eq!(level, tui_lipan::UpdateLevel::None);
                let response = response
                    .recv_timeout(std::time::Duration::from_millis(100))
                    .expect("metrics response is immediate");
                let data = response.data.expect("metrics data");
                assert!(response.ok);
                assert!(data["sampled_at_unix_ms"].is_number());
                assert!(data["client_inbound"].is_null());
                assert!(data["client_outbound"].is_null());
                assert!(data["piped_remote"].is_null());
                assert_eq!(
                    data["orphan_output"]["capacity_bytes"],
                    crate::state::ORPHAN_OUTPUT_GLOBAL_CAP as u64
                );
                assert_eq!(
                    data["orphan_output"]["capacity_keys"],
                    crate::state::ORPHAN_OUTPUT_KEY_CAP as u64
                );
                assert!(data["server"].is_null());
            })
            .expect("spawn metrics control test")
            .join()
            .expect("metrics control test completes");
    }
}
