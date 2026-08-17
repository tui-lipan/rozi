use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::find_pane_mut;
use crate::pty_events::notifications::{Notified, input_blocked};
use crate::state::PaneId;

pub(crate) fn forward_key_to_pane(ctx: &mut Context<AppRoot>, id: PaneId, key: KeyEvent) -> Update {
    if let Some(blocked) = input_blocked(ctx) {
        return blocked.notified.update();
    }
    let targets = synchronized_key_targets(&ctx.state, id);
    forward_key_to_targets(ctx, &targets, key)
}

fn forward_key_to_targets(ctx: &mut Context<AppRoot>, targets: &[PaneId], key: KeyEvent) -> Update {
    let mut repaint = false;
    let client = ctx.state.current().session_client.clone();
    ctx.state.current_mut().engaged = true;
    for id in targets {
        let local = crate::pane_lifecycle::pane_is_local(&ctx.state, *id);
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        if let Some(client) = client.clone() {
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
    let client = ctx.state.current().session_client.clone();
    ctx.state.current_mut().engaged = true;
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
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

    let client = ctx.state.current().session_client.clone();
    // Only a paste is the user putting something into this session. The focus notifications that
    // also arrive here are the terminal reporting on itself — counting those would mark a session
    // worked-in for having been looked at, which is the opposite of what engagement means.
    if matches!(input.kind, TerminalInputKind::Paste) {
        ctx.state.current_mut().engaged = true;
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
    // A pane running mouse tracking consumes the event in the framework before this pane's
    // `MouseRegion` runs, so the `on_mouse_down` that normally raises `Msg::FocusPane` never fires
    // for a full-screen TUI. The framework has already moved its *own* focus for clicks, drags and
    // scrolls (but deliberately not for plain motion), so reconciling from it restores
    // click-to-focus without reintroducing hover-to-focus against the user's config.
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

    let client = ctx.state.current().session_client.clone();
    let local = crate::pane_lifecycle::pane_is_local(&ctx.state, id);
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            // Motion may be held here until the child answers the position it already has; a report
            // that is not motion always goes. See `pty_events::pointer_flow`.
            if let Some(bytes) = pane.terminal.pointer_flow.admit(bytes) {
                client.send_input(id, pane.pty_generation, local, bytes);
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    focus_update
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
