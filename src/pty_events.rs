use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{PaneId, ToastChannel};

fn input_blocked(ctx: &mut Context<HyprmuxApp>) -> Option<String> {
    let reason = ctx.state.pane_input_block_reason()?.to_string();
    let notify = ctx
        .state
        .last_blocked_input_toast
        .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(2));
    if notify {
        ctx.state.last_blocked_input_toast = Some(std::time::Instant::now());
        replace_toast(
            ctx,
            ToastChannel::InputState,
            info_toast(&ctx.state.theme, reason.clone()),
        );
    }
    Some(reason)
}

/// Show the newest state for a notification channel without disturbing unrelated toasts.
pub(crate) fn replace_toast(ctx: &mut Context<HyprmuxApp>, channel: ToastChannel, toast: Toast) {
    if let Some(id) = ctx.state.replaceable_toasts.remove(&channel) {
        ctx.toast().dismiss_immediately(id);
    }
    let id = ctx.toast().push(toast);
    ctx.state.replaceable_toasts.insert(channel, id);
}

pub(crate) fn info_toast(theme: &Theme, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(3.0)
        .wrap(true)
        .max_width(Length::Px(64))
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
        .duration(crate::ops::exit::CONFIRM_WINDOW_SECS)
        .wrap(true)
        .max_width(Length::Px(64))
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
        .wrap(true)
        .max_width(Length::Px(64))
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
    crate::ops::theme::style_fg(theme.primary).map_or_else(Style::new, |text| Style::new().fg(text))
}

pub(crate) fn forward_key_to_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    key: KeyEvent,
) -> Update {
    if input_blocked(ctx).is_some() {
        return Update::full();
    }
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
            let _ = send_key_to_session_client(
                &client,
                *id,
                pane.pty_generation,
                key,
                pane.terminal.snapshot.key_modes,
            );
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
    if let Some(reason) = input_blocked(ctx) {
        return Err(reason);
    }
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
    crate::platform::notifications::notify(
        "hyprmux",
        &format!("Pane {id} exited with code {code}"),
    );
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
    if input_blocked(ctx).is_some() {
        return Update::full();
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
    // A pane running mouse tracking swallows pointer motion in the framework before our per-pane
    // hover callback runs, so on-hover focus would otherwise never fire over a full-screen TUI.
    // Forwarded mouse activity means the pointer is over this pane, so re-apply the hover policy.
    let hover = crate::ops::focus::hover_focus_pane(ctx, id);
    if input_blocked(ctx).is_some() {
        return Update::full();
    }

    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, bytes);
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    hover
}

/// Trailing-edge debounce window for controller PTY resizes, coalescing a resize storm (drag,
/// tiling reflow) into one `pty.resize`/SIGWINCH per pane.
const RESIZE_DEBOUNCE_MS: u64 = 16;

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    // Followers never drive PTY size: they letterbox to the controller's canonical canvas and their
    // screens reshape only via the server's broadcast `Resized`. Suppress their local resize here.
    if !ctx.state.is_controller() {
        return Update::none();
    }
    // The pane rect updates immediately, but the client-side screen only reshapes on the server's
    // ordered `Resized` broadcast, so both parsers reshape at the same byte position.
    let client = ctx.state.session_client.clone();
    let generation = match find_pane_mut(&mut ctx.state, id) {
        Some(pane) => {
            if client.is_none() {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                return Update::full();
            }
            pane.pty_generation
        }
        None => return Update::none(),
    };
    // Debounce through the shared bookkeeping when attached: record the latest size and arm a single
    // trailing-edge flush. Without shared state (a brief unattached window), send immediately.
    let epoch = ctx.state.runtime_epoch;
    if let Some(shared) = ctx.state.shared.as_mut() {
        shared
            .pending_resizes
            .insert(id, (cols.max(1), rows.max(1)));
        if shared.resize_flush_scheduled {
            return Update::none();
        }
        shared.resize_flush_scheduled = true;
        return Update::with_command(schedule_pane_resize_flush(epoch));
    }
    if let Some(client) = client {
        client.resize(id, generation, cols.max(1), rows.max(1));
    }
    Update::none()
}

fn schedule_pane_resize_flush(epoch: u64) -> Command {
    Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::sleep(std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS));
        link.send(crate::Msg::FlushPaneResizes { epoch });
    })
}

/// Send the latest debounced size for every pane that still exists (see the controller debounce in
/// [`handle_pane_resize`]). Clears the pending set and re-arms scheduling.
pub(crate) fn flush_pending_resizes(ctx: &mut Context<HyprmuxApp>) {
    let client = ctx.state.session_client.clone();
    let pending: Vec<(PaneId, (u16, u16))> = match ctx.state.shared.as_mut() {
        Some(shared) => {
            shared.resize_flush_scheduled = false;
            shared.pending_resizes.drain().collect()
        }
        None => return,
    };
    let Some(client) = client else {
        return;
    };
    for (id, (cols, rows)) in pending {
        if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
            client.resize(id, pane.pty_generation, cols.max(1), rows.max(1));
        }
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

pub(crate) fn terminal_key_event_bytes(key: KeyEvent, modes: TerminalKeyModes) -> Option<Vec<u8>> {
    key_event_to_bytes(key, modes)
}

pub(crate) fn send_key_to_session_client(
    client: &crate::session::client::SessionClient,
    pane_id: PaneId,
    generation: u64,
    key: KeyEvent,
    modes: TerminalKeyModes,
) -> std::result::Result<(), String> {
    let bytes = terminal_key_event_bytes(key, modes)
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
            // Modified cursor keys must carry the xterm parameter so word-wise motion
            // (Ctrl+Left/Right) and shifted selection reach TUIs instead of a bare arrow.
            (key(KeyCode::Left, KeyMods::CTRL), b"\x1b[1;5D".to_vec()),
            (key(KeyCode::Right, KeyMods::CTRL), b"\x1b[1;5C".to_vec()),
            (key(KeyCode::End, KeyMods::SHIFT), b"\x1b[1;2F".to_vec()),
        ];

        for (key, expected) in cases {
            assert_eq!(
                terminal_key_event_bytes(key, TerminalKeyModes::default()),
                Some(expected)
            );
        }
    }

    #[test]
    fn server_key_forwarding_enqueues_session_input_bytes() {
        let (client, rx) = crate::session::client::SessionClient::test_channel();

        send_key_to_session_client(
            &client,
            7,
            9,
            key(KeyCode::F(5), KeyMods::ALT),
            TerminalKeyModes::default(),
        )
        .expect("modified navigation key forwards");
        send_key_to_session_client(
            &client,
            7,
            9,
            key(KeyCode::Char('c'), KeyMods::CTRL),
            TerminalKeyModes::default(),
        )
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
    fn follower_resize_is_suppressed_and_controller_resize_debounces() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        fn resizes(rx: &std::sync::mpsc::Receiver<ClientOutbound>) -> Vec<(u16, u16)> {
            rx.try_iter()
                .filter_map(|msg| match msg {
                    ClientOutbound::Control(ClientMessage::Resize { cols, rows, .. }) => {
                        Some((cols, rows))
                    }
                    _ => None,
                })
                .collect()
        }

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let viewport = Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                };

                // Follower: a resize forwards nothing (it letterboxes to the canonical size).
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(viewport);
                let (client, follower_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2);
                    state.shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::PaneResize(1, 40, 12))
                    .expect("dispatch follower resize");
                assert!(
                    resizes(&follower_rx).is_empty(),
                    "a follower must not forward pane resizes"
                );

                // Controller: rapid resizes coalesce; the flush sends only the latest size.
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(viewport);
                let (client, controller_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    state.shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::PaneResize(1, 40, 12))
                    .expect("dispatch first resize");
                backend
                    .dispatch(Msg::PaneResize(1, 50, 20))
                    .expect("dispatch second resize");
                assert!(
                    resizes(&controller_rx).is_empty(),
                    "debounced resizes are not sent until the flush"
                );
                backend
                    .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                    .expect("dispatch flush");
                assert_eq!(
                    resizes(&controller_rx),
                    vec![(50, 20)],
                    "flush sends only the latest size per pane"
                );
            })
            .expect("spawn resize test thread")
            .join()
            .expect("resize test thread completes");
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
