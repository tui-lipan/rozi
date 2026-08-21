use std::time::Duration;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::find_pane_mut;
use crate::pty_events::notifications::{Notified, input_blocked};
use crate::state::PaneId;

pub(crate) fn forward_key_to_pane(ctx: &mut Context<AppRoot>, id: PaneId, key: KeyEvent) -> Update {
    if let Some(blocked) = input_blocked(ctx) {
        return blocked.notified.update();
    }
    // The funnel every keystroke passes through on its way to a PTY, whichever route delivered it -
    // the terminal widget's own handler, app-level key routing, a forwarded prefix. Acknowledging
    // here rather than at one of those routes is what makes "typing answers the mark" hold for all
    // of them. Only the pane the key was aimed at counts; synchronized siblings are echoes.
    crate::ops::focus::acknowledge_pane_input(&mut ctx.state, id);
    let targets = synchronized_key_targets(&ctx.state, id);
    forward_key_to_targets(ctx, &targets, key)
}

fn forward_key_to_targets(ctx: &mut Context<AppRoot>, targets: &[PaneId], key: KeyEvent) -> Update {
    let mut repaint = false;
    for id in targets {
        let local = crate::pane_lifecycle::pane_is_local(&ctx.state, *id);
        let scratch = crate::scratchpad::contains(&ctx.state, *id);
        let client = ctx.state.pty_client_for_pane(*id);
        if !scratch {
            ctx.state.current_mut().engaged = true;
        }
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        if let Some(client) = client {
            if send_key_to_session_client(
                &client,
                *id,
                pane.pty_generation,
                local,
                key,
                pane.terminal.snapshot().key_modes,
            )
            .is_ok()
                && pane.terminal.set_scrollback(0)
            {
                repaint = true;
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            repaint = true;
        }
    }
    if repaint {
        Update::full()
    } else {
        Update::none()
    }
}

/// Send raw bytes (paste payloads, user `Send` commands, control-socket text) to a pane's shell
/// through the session server. Returns an error string when no client is connected.
pub(crate) fn send_pane_bytes(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    bytes: Vec<u8>,
) -> std::result::Result<(), String> {
    if let Some(blocked) = input_blocked(ctx) {
        return Err(blocked.reason);
    }
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    let scratch = crate::scratchpad::contains(&ctx.state, id);
    let client = ctx.state.pty_client_for_pane(id);
    if !scratch {
        ctx.state.current_mut().engaged = true;
    }
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Ok(());
    };
    let Some(client) = client else {
        pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
        return Err("session disconnected".to_string());
    };
    client.send_input(id, pane.pty_generation, local, bytes);
    Ok(())
}

pub(crate) fn synchronized_key_targets(state: &crate::state::State, source: PaneId) -> Vec<PaneId> {
    if crate::scratchpad::contains(state, source) {
        return vec![source];
    }
    let workspace = state.current().active_workspace_ref();
    if !workspace.synchronized {
        return vec![source];
    }
    if !workspace
        .panes
        .iter()
        .any(|pane| pane.id == source && !pane.floating && !pane.closing)
    {
        return vec![source];
    }
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.floating && !pane.closing)
        .map(|pane| pane.id)
        .collect()
}

pub(crate) fn handle_pane_input(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    if matches!(input.kind, TerminalInputKind::Key) {
        // Key input is routed through Msg::PaneKey so prefix and held-modifier
        // bindings can intercept before bytes reach the PTY. Keeping on_input
        // installed still enables bracketed paste and focus reports.
        return Update::none();
    }
    if let Some(blocked) = input_blocked(ctx) {
        return blocked.notified.update();
    }

    let client = ctx.state.pty_client_for_pane(id);
    // Only a paste is the user putting something into this session. The focus notifications that
    // also arrive here are the terminal reporting on itself — counting those would mark a session
    // worked-in for having been looked at, which is the opposite of what engagement means.
    if matches!(input.kind, TerminalInputKind::Paste) {
        if !crate::scratchpad::contains(&ctx.state, id) {
            ctx.state.current_mut().engaged = true;
        }
        // Same distinction, applied to attention: a paste is the user acting in this pane, so it
        // answers an alert the way a keystroke does. The focus notifications that also arrive here
        // are the terminal reporting on itself and answer nothing.
        crate::ops::focus::acknowledge_pane_input(&mut ctx.state, id);
    }
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, local, input.bytes.to_vec());
            if matches!(input.kind, TerminalInputKind::Paste) && pane.terminal.set_scrollback(0) {
                return Update::full();
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn handle_pane_mouse(ctx: &mut Context<AppRoot>, id: PaneId, bytes: Vec<u8>) -> Update {
    // A pane running mouse tracking consumes forwarded events before this pane's `MouseRegion`.
    // The framework bubbles a left press that only focuses the terminal, but other focus-bearing
    // reports (right press, drag, scroll) arrive here after it has moved its own focus. Reconciling
    // from it keeps the two focus models together without turning plain motion into hover-focus
    // against the user's config.
    // The press half of this click was consumed to dismiss hint mode; the release completes the
    // same click and is consumed with it, rather than reaching the child or pulling focus along.
    if std::mem::take(&mut ctx.state.consumed_pointer_click) {
        return Update::none();
    }
    let before = ctx.state.focused_pane();
    crate::key_routing::sync_focus_from_framework(ctx);
    let focus_moved = ctx.state.focused_pane() != before;
    // Forwarded activity also means the pointer is over this pane, so re-apply the hover policy.
    let hover = crate::ops::focus::hover_focus_pane(ctx, id);
    let focus_update = if focus_moved { Update::full() } else { hover };
    if let Some(blocked) = input_blocked(ctx) {
        // Pointer motion arrives continuously; a renewed rejection draws nothing new, so fall back
        // to whatever focus already asked for.
        return match blocked.notified {
            Notified::Pushed => Update::full(),
            Notified::Renewed => focus_update,
        };
    }

    let client = ctx.state.pty_client_for_pane(id);
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    let interval =
        crate::pty_events::pointer_flow::interval_for_frame_rate(ctx.state.config.frame_rate);
    let mut hold = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            // Motion is sampled at the configured frame cadence; state changes always go. See
            // `pty_events::pointer_flow`.
            if let Some(bytes) = pane.terminal.pointer_flow.admit(bytes, interval) {
                client.send_input(id, pane.pty_generation, local, bytes);
            } else {
                hold = pane.terminal.pointer_flow.arm(interval);
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    if let Some(after) = hold {
        arm_pointer_flow(ctx, id, after);
    }
    focus_update
}

/// Ask to be woken when a pane's next pointer-motion interval begins.
pub(crate) fn arm_pointer_flow(ctx: &mut Context<AppRoot>, id: PaneId, after: Duration) {
    if let Some(link) = ctx.state.command_link.clone() {
        link.send_after(after, crate::Msg::PointerFlowTick(id));
    }
}

/// A pane's pointer wakeup fired. Forward its newest held position when the cadence permits.
pub(crate) fn pointer_flow_tick(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    use crate::pty_events::pointer_flow::Paced;

    let client = ctx.state.pty_client_for_pane(id);
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    let interval =
        crate::pty_events::pointer_flow::interval_for_frame_rate(ctx.state.config.frame_rate);
    let mut retry = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        match pane.terminal.pointer_flow.paced(interval) {
            Paced::Send(bytes) => {
                if let Some(client) = client {
                    client.send_input(id, pane.pty_generation, local, bytes);
                }
            }
            Paced::Retry(after) => retry = Some(after),
            Paced::Idle => {}
        }
    }
    if let Some(after) = retry {
        arm_pointer_flow(ctx, id, after);
    }
    // Nothing here changes what is on screen: the child may draw in response to the report.
    Update::none()
}

pub(crate) fn handle_pane_scroll(ctx: &mut Context<AppRoot>, id: PaneId, offset: usize) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && pane.terminal.set_scrollback(offset)
    {
        return Update::full();
    }
    Update::none()
}

pub(crate) fn terminal_key_event_bytes(key: KeyEvent, modes: TerminalKeyModes) -> Option<Vec<u8>> {
    key_event_to_bytes(key, modes)
}

pub(crate) fn send_key_to_session_client(
    client: &crate::session::client::SessionClient,
    pane_id: PaneId,
    generation: u64,
    local: bool,
    key: KeyEvent,
    modes: TerminalKeyModes,
) -> std::result::Result<(), String> {
    let bytes = terminal_key_event_bytes(key, modes)
        .ok_or_else(|| "key is not representable for session forwarding yet".to_string())?;
    client.send_input(pane_id, generation, local, bytes);
    Ok(())
}
