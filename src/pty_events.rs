use std::sync::Arc;
use std::time::Instant;

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
    let targets = synchronized_key_targets(&ctx.state, id);
    if targets.len() > 1 {
        return forward_key_to_targets(ctx, &targets, key);
    }

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

fn forward_key_to_targets(
    ctx: &mut Context<HyprmuxApp>,
    targets: &[PaneId],
    key: KeyEvent,
) -> Update {
    let mut repaint = false;
    let mut errors = Vec::new();
    for id in targets {
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        match pane.terminal.send_key(key) {
            Ok(result) => repaint |= result.repaint,
            Err(message) => {
                pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message.clone()));
                errors.push((*id, message));
            }
        }
    }
    for (id, message) in errors {
        ctx.toast().push(error_toast(format!("Pane {id}"), message));
    }
    if repaint {
        Update::full()
    } else {
        Update::none()
    }
}

pub(crate) fn synchronized_key_targets(state: &crate::state::State, source: PaneId) -> Vec<PaneId> {
    let workspace = &state.workspaces[state.active_workspace];
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

pub(crate) fn handle_pty_event(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    generation: u64,
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
        if pane.pty_generation != generation {
            return Update::none();
        }
        let outcome = pane.terminal.handle_pty_event(event);
        (outcome, pane.closing, pane.terminal.status_text())
    };
    match outcome {
        PaneEventOutcome::Repaint => {
            let focused = ctx.state.focused_pane;
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                pane.activity.last_activity = Some(Instant::now());
                if focused != Some(id) {
                    pane.activity.has_unseen_output = true;
                }
            }
            Update::full()
        }
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
            maybe_notify_pane_exit(&ctx.state.config, id, code);
            ctx.toast()
                .push(info_toast(format!("Pane {id} exited with code {code}")));
            begin_close_pane(ctx, id, ctx.state.config.animations)
        }
    }
}

fn maybe_notify_pane_exit(config: &crate::config::HyprmuxConfig, id: PaneId, code: i32) {
    if !config.notifications.enabled || !config.notifications.pane_exit {
        return;
    }
    std::thread::spawn(move || {
        let _ = std::process::Command::new("notify-send")
            .arg("hyprmux")
            .arg(format!("Pane {id} exited with code {code}"))
            .status();
    });
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
    generation: u64,
    pty: TerminalPty,
) -> Update {
    let mut error = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
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
    fn synchronized_targets_default_to_source_only() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes.push(Pane::new(2, 100, rect()));

        assert_eq!(synchronized_key_targets(&state, 1), vec![1]);
    }

    #[test]
    fn synchronized_targets_exclude_floating_closing_and_scratch() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].synchronized = true;
        state.workspaces[0].panes.push(Pane::new(2, 100, rect()));
        let mut floating = Pane::new(3, 100, rect());
        floating.floating = true;
        state.workspaces[0].panes.push(floating);
        let mut closing = Pane::new(4, 100, rect());
        closing.closing = true;
        state.workspaces[0].panes.push(closing);
        state.scratch = Some(Pane::new(crate::state::SCRATCH_PANE_ID, 100, rect()));

        assert_eq!(synchronized_key_targets(&state, 1), vec![1, 2]);
        assert_eq!(synchronized_key_targets(&state, 3), vec![3]);
    }
}
