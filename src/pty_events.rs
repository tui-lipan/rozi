use std::sync::Arc;
use std::time::Instant;

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane::PaneEventOutcome;
use crate::pane_lifecycle::{begin_close_pane, find_pane_mut};
use crate::state::PaneId;

pub(crate) fn info_toast(message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(3.0)
        .padding((0, 1, 0, 0))
}

pub(crate) fn error_toast(title: impl Into<String>, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .title(Some(title.into()))
        .duration(6.0)
        .border(true)
        .padding((0, 1, 0, 0))
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

    let client = ctx.state.session_client.clone();
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Update::none();
    };

    if pane.terminal.is_server_backed() {
        if client.is_none() {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            ctx.toast()
                .push(error_toast(format!("Pane {id}"), "session disconnected"));
            return Update::full();
        }
        if let Some(client) = client {
            if let Err(message) = send_key_to_session_client(&client, id, pane.pty_generation, key)
            {
                ctx.toast().push(error_toast(format!("Pane {id}"), message));
                return Update::full();
            }
            return Update::none();
        }
    }

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
    let client = ctx.state.session_client.clone();
    for id in targets {
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        if pane.terminal.is_server_backed() {
            if client.is_none() {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                errors.push((*id, "session disconnected".to_string()));
            } else if let Some(client) = client.clone() {
                if let Err(message) =
                    send_key_to_session_client(&client, *id, pane.pty_generation, key)
                {
                    errors.push((*id, message));
                }
            } else {
                errors.push((*id, "session disconnected".to_string()));
            }
            continue;
        }
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
    let (outcome, was_closing, status_text): (PaneEventOutcome, bool, String) = {
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
            if let Some(message) = status_text.strip_prefix("error: ").map(str::to_string) {
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

    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.terminal.is_server_backed() {
            if let Some(client) = client {
                client.send_input(id, pane.pty_generation, input.bytes.to_vec());
            } else {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                ctx.toast()
                    .push(error_toast(format!("Pane {id}"), "session disconnected"));
                return Update::full();
            }
            return Update::none();
        }
        if let Err(message) = pane.terminal.send_bytes(&input.bytes) {
            let toast_message = message.clone();
            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
            ctx.toast()
                .push(error_toast(format!("Pane {id}"), toast_message));
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn handle_pane_mouse(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    bytes: Vec<u8>,
) -> Update {
    let mut error = None;
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.terminal.is_server_backed() {
            if let Some(client) = client {
                client.send_input(id, pane.pty_generation, bytes);
            } else {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                ctx.toast()
                    .push(error_toast(format!("Pane {id}"), "session disconnected"));
                return Update::full();
            }
            return Update::none();
        }
        if let Err(message) = pane.terminal.send_bytes(&bytes) {
            error = Some(message.clone());
            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
        }
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
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.terminal.is_server_backed()
            && let Some(client) = client
        {
            client.resize(id, pane.pty_generation, cols.max(1), rows.max(1));
        }
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
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.terminal.is_server_backed()
            && let Some(client) = client
        {
            client.scroll(id, pane.pty_generation, offset);
            return Update::none();
        }
        if pane.terminal.set_scrollback(offset) {
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn terminal_key_event_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    key_event_to_bytes(key)
}

pub(crate) fn send_key_to_session_client(
    client: &crate::session::client::SessionClient,
    pane_id: PaneId,
    generation: u64,
    key: KeyEvent,
) -> std::result::Result<(), String> {
    let bytes = terminal_key_event_bytes(key)
        .ok_or_else(|| "key is not representable for session forwarding yet".to_string())?;
    client.send_input(pane_id, generation, bytes);
    Ok(())
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
        pane.terminal.bind_session(id, generation);
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

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    #[test]
    fn terminal_key_encoding_matches_local_terminal_encoder_representatives() {
        let cases = [
            (key(KeyCode::Char('x'), KeyMods::NONE), b"x".to_vec()),
            (key(KeyCode::Char('c'), KeyMods::CTRL), vec![3]),
            (key(KeyCode::Char('x'), KeyMods::ALT), b"\x1bx".to_vec()),
            (key(KeyCode::Enter, KeyMods::NONE), b"\r".to_vec()),
            (key(KeyCode::BackTab, KeyMods::NONE), b"\x1b[Z".to_vec()),
            (key(KeyCode::Delete, KeyMods::NONE), b"\x1b[3~".to_vec()),
            (key(KeyCode::Home, KeyMods::NONE), b"\x1b[H".to_vec()),
            (key(KeyCode::End, KeyMods::NONE), b"\x1b[F".to_vec()),
            (key(KeyCode::PageUp, KeyMods::NONE), b"\x1b[5~".to_vec()),
            (key(KeyCode::F(12), KeyMods::NONE), b"\x1b[24~".to_vec()),
        ];

        for (key, expected) in cases {
            assert_eq!(terminal_key_event_bytes(key), Some(expected));
        }
    }

    #[test]
    fn server_key_forwarding_enqueues_session_input_bytes() {
        let (client, rx) = crate::session::client::SessionClient::test_channel();

        send_key_to_session_client(&client, 7, 9, key(KeyCode::F(5), KeyMods::ALT))
            .expect("modified navigation key forwards");
        send_key_to_session_client(&client, 7, 9, key(KeyCode::Char('c'), KeyMods::CTRL))
            .expect("control key forwards");

        assert_eq!(
            rx.recv().expect("first message"),
            crate::session::protocol::ClientMessage::Input {
                pane_id: 7,
                generation: 9,
                bytes: b"\x1b\x1b[15~".to_vec(),
            }
        );
        assert_eq!(
            rx.recv().expect("second message"),
            crate::session::protocol::ClientMessage::Input {
                pane_id: 7,
                generation: 9,
                bytes: vec![3],
            }
        );
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
