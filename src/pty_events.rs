use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::PaneId;

pub(crate) fn info_toast(theme: &Theme, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(3.0)
        .frame_style(toast_frame_style(theme.status.info))
        .title_style(toast_text_style(theme).bold())
        .message_style(toast_text_style(theme))
        .copyable(true)
        .copy_affordance(ToastCopyAffordance::None)
        .padding((0, 0, 0, 0))
}

/// Toast for an armed destructive action: error-colored chrome, visible for exactly the confirm
/// window so its dismissal coincides with the pending action expiring.
pub(crate) fn confirm_toast(theme: &Theme, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(crate::exit_ops::CONFIRM_WINDOW_SECS)
        .frame_style(toast_frame_style(theme.status.error))
        .message_style(toast_text_style(theme))
        .padding((0, 0, 0, 0))
}

pub(crate) fn error_toast(
    theme: &Theme,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Toast {
    Toast::new(message.into())
        .title(Some(title.into()))
        .duration(6.0)
        .border(true)
        .frame_style(toast_frame_style(theme.status.error))
        .title_style(toast_text_style(theme).bold())
        .message_style(toast_text_style(theme))
        .copyable(true)
        .copy_affordance(ToastCopyAffordance::None)
        .padding((0, 0, 0, 0))
}

fn toast_frame_style(accent: Color) -> Style {
    Style::new().fg(accent)
}

fn toast_text_style(theme: &Theme) -> Style {
    crate::theme_ops::style_fg(theme.primary).map_or_else(Style::new, |text| Style::new().fg(text))
}

pub(crate) fn forward_key_to_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    key: KeyEvent,
) -> Update {
    let targets = synchronized_key_targets(&ctx.state, id);
    forward_key_to_targets(ctx, &targets, key)
}

fn forward_key_to_targets(
    ctx: &mut Context<HyprmuxApp>,
    targets: &[PaneId],
    key: KeyEvent,
) -> Update {
    let mut repaint = false;
    let client = ctx.state.session_client.clone();
    for id in targets {
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        if let Some(client) = client.clone() {
            let _ = send_key_to_session_client(&client, *id, pane.pty_generation, key);
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
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    bytes: Vec<u8>,
) -> std::result::Result<(), String> {
    let client = ctx.state.session_client.clone();
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Ok(());
    };
    let Some(client) = client else {
        pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
        return Err("session disconnected".to_string());
    };
    client.send_input(id, pane.pty_generation, bytes);
    Ok(())
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

pub(crate) fn maybe_notify_pane_exit(config: &crate::config::HyprmuxConfig, id: PaneId, code: i32) {
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
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, input.bytes.to_vec());
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
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
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, bytes);
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    // The pane rect updates immediately, but the client-side screen only reshapes on the server's
    // ordered `Resized` broadcast, so both parsers reshape at the same byte position.
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.resize(id, pane.pty_generation, cols.max(1), rows.max(1));
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn handle_pane_scroll(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    offset: usize,
) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
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
            crate::session::client::ClientOutbound::PaneInput {
                pane_id: 7,
                generation: 9,
                bytes: b"\x1b\x1b[15~".to_vec(),
            }
        );
        assert_eq!(
            rx.recv().expect("second message"),
            crate::session::client::ClientOutbound::PaneInput {
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
