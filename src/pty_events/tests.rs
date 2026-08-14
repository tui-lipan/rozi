use super::*;
use crate::AppRoot;
use crate::state::{Pane, State};
use tui_lipan::prelude::*;

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
        false,
        key(KeyCode::F(5), KeyMods::ALT),
        TerminalKeyModes::default(),
    )
    .expect("modified navigation key forwards");
    send_key_to_session_client(
        &client,
        7,
        9,
        false,
        key(KeyCode::Char('c'), KeyMods::CTRL),
        TerminalKeyModes::default(),
    )
    .expect("control key forwards");

    assert_eq!(
        rx.recv().expect("first message"),
        crate::session::client::ClientOutbound::PaneInput {
            pane_id: 7,
            local: false,
            generation: 9,
            bytes: b"\x1b\x1b[15~".to_vec(),
        }
    );
    assert_eq!(
        rx.recv().expect("second message"),
        crate::session::client::ClientOutbound::PaneInput {
            pane_id: 7,
            local: false,
            generation: 9,
            bytes: vec![3],
        }
    );
}

/// The prefix is an explicit entry into rozi's command state, so an unbound key there resolves
/// to nothing rather than being replayed into the shell. Without this, a mistyped chord types a
/// stray character into whatever is running in the pane.
#[test]
fn an_unbound_key_after_the_prefix_reaches_no_pane() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let app = App::new()
                .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
                .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
                .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly);
            let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
            let (client, rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_client = Some(client);
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
            }
            backend.render();
            backend.focus_next();
            while rx.try_recv().is_ok() {}

            let prefix = key(KeyCode::Char('a'), KeyMods::CTRL);
            backend.send_key(prefix).expect("prefix enters chord");
            // `y` is deliberately unbound as a prefix chord: nothing should reach the pane.
            backend
                .send_key(key(KeyCode::Char('y'), KeyMods::NONE))
                .expect("unbound key resolves");

            let inputs: Vec<_> = rx
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::PaneInput { bytes, .. } => Some(bytes),
                    ClientOutbound::Control(_) => None,
                })
                .collect();
            assert!(
                inputs.is_empty(),
                "the prefix and the unbound key must both stay in rozi: {inputs:?}"
            );

            // The chord is over, so the next key is ordinary input again.
            backend
                .send_key(key(KeyCode::Char('y'), KeyMods::NONE))
                .expect("plain key forwards");
            let inputs: Vec<_> = rx
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::PaneInput { bytes, .. } => Some(bytes),
                    ClientOutbound::Control(_) => None,
                })
                .collect();
            assert_eq!(inputs, vec![b"y".to_vec()]);
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn double_prefix_forwards_one_prefix_key() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let app = App::new()
                .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
                .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
                .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly);
            let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
            let (client, rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_client = Some(client);
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
            }
            backend.render();
            backend.focus_next();
            while rx.try_recv().is_ok() {}

            let prefix = key(KeyCode::Char('a'), KeyMods::CTRL);
            backend.send_key(prefix).expect("first prefix enters chord");
            backend
                .send_key(prefix)
                .expect("second prefix forwards the first");

            let inputs: Vec<_> = rx
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::PaneInput { .. } => Some(message),
                    ClientOutbound::Control(_) => None,
                })
                .collect();
            assert_eq!(
                inputs,
                vec![ClientOutbound::PaneInput {
                    pane_id: 1,
                    local: false,
                    generation: 0,
                    bytes: vec![1],
                }]
            );
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

/// Engagement is what keeps a startup temporary session alive across a switch, so it has to
/// mean the user put something into the session. A focus report is the terminal talking about
/// itself — a session that was merely looked at is still untouched.
#[test]
fn focus_reports_do_not_mark_a_session_as_worked_in() {
    use crate::Msg;
    use crate::session::client::SessionClient;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, _rx) = SessionClient::test_channel();
            backend.state_mut().current_mut().session_client = Some(client);
            backend.state_mut().current_mut().engaged = false;
            backend.render();

            for kind in [TerminalInputKind::FocusIn, TerminalInputKind::FocusOut] {
                backend
                    .dispatch(Msg::PaneInput(
                        1,
                        TerminalInputEvent {
                            kind,
                            key: None,
                            bytes: Vec::new().into(),
                        },
                    ))
                    .expect("dispatch focus report");
                assert!(
                    !backend.state().current().engaged,
                    "{kind:?} must not count as working in the session"
                );
            }

            backend
                .dispatch(Msg::PaneInput(
                    1,
                    TerminalInputEvent {
                        kind: TerminalInputKind::Paste,
                        key: None,
                        bytes: b"work".to_vec().into(),
                    },
                ))
                .expect("dispatch paste");
            assert!(
                backend.state().current().engaged,
                "a paste puts the user's own content into the session"
            );
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn terminal_keyboard_and_paste_input_return_scrolled_pane_to_live_view() {
    use crate::Msg;
    use crate::session::client::SessionClient;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, _rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_client = Some(client);
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.terminal
                    .process_server_output("history\n".repeat(80).as_bytes());
                assert!(pane.terminal.set_scrollback(10));
            }
            backend.render();

            backend
                .dispatch(Msg::PaneKey(1, key(KeyCode::Char('x'), KeyMods::NONE)))
                .expect("dispatch terminal key");
            assert_eq!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .scrollback_offset(),
                0
            );

            assert!(
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .set_scrollback(10)
            );
            backend
                .dispatch(Msg::PaneInput(
                    1,
                    TerminalInputEvent {
                        kind: TerminalInputKind::Paste,
                        key: None,
                        bytes: b"pasted".to_vec().into(),
                    },
                ))
                .expect("dispatch terminal paste");
            assert_eq!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .scrollback_offset(),
                0
            );
        })
        .expect("spawn terminal input test thread")
        .join()
        .expect("terminal input test thread completes");
}

#[test]
fn synchronized_targets_default_to_source_only() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    state.current_mut().workspaces[0]
        .panes
        .push(Pane::new(2, 100, rect()));

    assert_eq!(synchronized_key_targets(&state, 1), vec![1]);
}

#[test]
fn pane_status_notification_policy_is_controller_only_and_configurable() {
    let mut config = crate::config::Config::default();
    config.notifications.enabled = true;

    assert!(should_notify_pane_status(&config, true, false, true, false));
    assert!(!should_notify_pane_status(
        &config, false, false, true, false
    ));
    assert!(!should_notify_pane_status(&config, true, true, true, false));
    assert!(!should_notify_pane_status(
        &config, true, false, false, true
    ));
    config.notifications.pane_done = true;
    assert!(should_notify_pane_status(&config, true, false, false, true));
    config.notifications.enabled = false;
    assert!(!should_notify_pane_status(
        &config, true, false, true, false
    ));
}

#[test]
fn status_notification_treats_a_focused_background_window_as_unattended() {
    let mut config = crate::config::Config::default();
    config.notifications.enabled = true;
    let mut state = State::new(config.clone(), Theme::default());
    let pane_id = state.current().focused_pane.expect("fresh pane focus");
    state.window_focused = false;

    assert!(!state.is_pane_attended(pane_id));
    assert!(should_notify_pane_status(
        &config,
        true,
        state.is_pane_attended(pane_id),
        true,
        false
    ));
}

#[test]
fn pane_exit_notification_splits_clean_and_error_codes() {
    let mut config = crate::config::Config::default();
    config.notifications.enabled = true;
    // Enabling notifications is not on its own a reason to announce a clean exit.
    assert!(!should_notify_pane_exit(&config, 0));
    assert!(should_notify_pane_exit(&config, 1));
    config.notifications.pane_exit = true;
    assert!(should_notify_pane_exit(&config, 0));
    assert!(should_notify_pane_exit(&config, 1));
    config.notifications.pane_exit_error = false;
    assert!(should_notify_pane_exit(&config, 0));
    assert!(!should_notify_pane_exit(&config, 1));
}

#[test]
fn follower_resize_is_suppressed_and_controller_resize_debounces() {
    use crate::AppRoot;
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
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            let (client, follower_rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client);
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(2);
                state.current_mut().shared = Some(shared);
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
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            let (client, controller_rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client);
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(1);
                state.current_mut().shared = Some(shared);
            }
            backend.render();
            // Pretend a flush is already armed. A real one is a 16 ms wall-clock timer, and
            // everything asserted below is state that firing it drains - so left to run, how
            // loaded the machine is decides the outcome. Arming it by hand keeps the flush in
            // this test's hands instead of the clock's.
            backend
                .state_mut()
                .current_mut()
                .shared
                .as_mut()
                .expect("controller has shared state")
                .resize_flush_scheduled = true;
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
            assert_eq!(
                backend
                    .state()
                    .current()
                    .shared
                    .as_ref()
                    .expect("controller has shared state")
                    .pending_resizes
                    .get(&(false, 1)),
                Some(&(50, 20)),
                "both resizes coalesce into the latest pending size"
            );
            backend
                .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                .expect("dispatch flush");
            assert_eq!(
                resizes(&controller_rx),
                vec![(50, 20)],
                "flush sends only the latest size per pane"
            );

            // A live sidebar drag follows the same debounce path as every other geometry
            // change. Preview state must not hold PTY resizes until pointer release.
            backend.state_mut().sidebar.width_preview = Some(40);
            backend
                .dispatch(Msg::PaneResize(1, 60, 22))
                .expect("dispatch preview resize");
            backend
                .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                .expect("dispatch flush during preview");
            assert_eq!(resizes(&controller_rx), vec![(60, 22)]);
            backend.state_mut().sidebar.width_preview = None;

            // A flush with no client must hold the size rather than discard it: nothing
            // re-derives one, so dropping it leaves the PTY wrong until the pane's geometry
            // happens to change again.
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport);
            let (client, reconnect_rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client.clone());
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(1);
                state.current_mut().shared = Some(shared);
            }
            backend.render();
            backend
                .dispatch(Msg::PaneResize(1, 50, 20))
                .expect("dispatch resize");
            // The link drops between the report and the trailing-edge flush it armed.
            backend.state_mut().current_mut().session_client = None;
            backend
                .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                .expect("dispatch flush while disconnected");
            assert_eq!(
                backend
                    .state()
                    .current()
                    .shared
                    .as_ref()
                    .expect("controller has shared state")
                    .pending_resizes
                    .get(&(false, 1)),
                Some(&(50, 20)),
                "a flush with no client keeps the size for the next one"
            );

            // The client arriving is what delivers it.
            backend.state_mut().current_mut().session_client = Some(client);
            backend
                .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                .expect("dispatch flush after reconnect");
            assert_eq!(
                resizes(&reconnect_rx),
                vec![(50, 20)],
                "the held size reaches the PTY once a client is back"
            );
        })
        .expect("spawn resize test thread")
        .join()
        .expect("resize test thread completes");
}

#[test]
fn synchronized_targets_exclude_floating_and_scratch() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    state.current_mut().workspaces[0].synchronized = true;
    state.current_mut().workspaces[0]
        .panes
        .push(Pane::new(2, 100, rect()));
    let mut floating = Pane::new(3, 100, rect());
    floating.floating = true;
    state.current_mut().workspaces[0].panes.push(floating);
    state.current_mut().workspaces[0]
        .panes
        .push(Pane::new(4, 100, rect()));
    state.scratch.panes.push(Pane::new(5, 100, rect()));

    assert_eq!(synchronized_key_targets(&state, 1), vec![1, 2, 4]);
    assert_eq!(synchronized_key_targets(&state, 3), vec![3]);
    assert_eq!(synchronized_key_targets(&state, 5), vec![5]);
}
