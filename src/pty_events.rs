use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane::PaneEventOutcome;
use crate::pane_lifecycle::{begin_close_pane, find_pane_mut};
use crate::state::PaneId;

pub(crate) fn info_toast(message: impl Into<String>) -> Toast {
    Toast::new(message.into()).duration(3.0)
}

pub(crate) fn error_toast(title: impl Into<String>, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .title(Some(title.into()))
        .duration(6.0)
        .border(true)
}

pub(crate) fn forward_key_to_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    key: KeyEvent,
) -> Update {
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Update::none();
    };

    match pane.terminal.send_key(key) {
        Ok(result) => {
            if result.repaint {
                Update::full()
            } else {
                Update::none()
            }
        }
        Err(message) => {
            let toast_message = message.clone();
            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
            ctx.toast()
                .push(error_toast(format!("Pane {id}"), toast_message));
            Update::full()
        }
    }
}

pub(crate) fn handle_pty_event(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    event: TerminalPtyEvent,
) -> Update {
    let pty_error = match &event {
        TerminalPtyEvent::Error(message) => Some(message.to_string()),
        _ => None,
    };
    let (outcome, was_closing, status_text) = {
        let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
            return Update::none();
        };
        let outcome = pane.terminal.handle_pty_event(event);
        (outcome, pane.closing, pane.terminal.status_text())
    };
    match outcome {
        PaneEventOutcome::Repaint => Update::full(),
        PaneEventOutcome::StatusChanged => {
            if let Some(message) =
                pty_error.or_else(|| status_text.strip_prefix("error: ").map(str::to_string))
            {
                ctx.toast().push(error_toast(format!("Pane {id}"), message));
            }
            Update::full()
        }
        PaneEventOutcome::Exited(code) => {
            if crate::scratchpad::is_scratch(id) {
                // The scratch shell exited; drop it so the next toggle re-spawns a fresh one.
                return crate::scratchpad::handle_scratch_exit(ctx);
            }
            if was_closing {
                return Update::full();
            }
            ctx.toast()
                .push(info_toast(format!("Pane {id} exited with code {code}")));
            begin_close_pane(ctx, id, ctx.state.config.animations)
        }
    }
}

pub(crate) fn handle_pane_input(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    if matches!(input.kind, TerminalInputKind::Key) {
        // Key input is routed through Msg::PaneKey so prefix and held-modifier
        // bindings can intercept before bytes reach the PTY. Keeping on_input
        // installed still enables bracketed paste and focus reports.
        return Update::none();
    }

    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && let Err(message) = pane.terminal.send_bytes(&input.bytes)
    {
        let toast_message = message.clone();
        pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
        ctx.toast()
            .push(error_toast(format!("Pane {id}"), toast_message));
        return Update::full();
    }
    Update::none()
}

pub(crate) fn handle_pane_mouse(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    bytes: Vec<u8>,
) -> Update {
    let mut error = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && let Err(message) = pane.terminal.send_bytes(&bytes)
    {
        error = Some(message.clone());
        pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
    }
    if let Some(message) = error {
        ctx.toast().push(error_toast(format!("Pane {id}"), message));
        Update::full()
    } else {
        Update::none()
    }
}

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        match pane.terminal.resize(cols, rows) {
            Ok(true) => Update::full(),
            Ok(false) => Update::none(),
            Err(message) => {
                let toast_message = message.clone();
                pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
                ctx.toast()
                    .push(error_toast(format!("Pane {id}"), toast_message));
                Update::full()
            }
        }
    } else {
        Update::none()
    }
}

pub(crate) fn handle_pane_scroll(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    offset: usize,
) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && pane.terminal.set_scrollback(offset)
    {
        return Update::full();
    }
    Update::none()
}

pub(crate) fn handle_pty_ready(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    pty: TerminalPty,
) -> Update {
    let mut error = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Err(message) = pane.terminal.set_pty(pty) {
            error = Some(message.clone());
            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
        }
    }
    if let Some(message) = error {
        ctx.toast().push(error_toast(format!("Pane {id}"), message));
    }
    Update::full()
}
