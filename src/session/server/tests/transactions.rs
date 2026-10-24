//! Exercise the protocol boundary and real PTY spawn, rather than mocking transaction steps.
use super::*;

fn spawn(server: &SessionServer, id: PaneId) -> ClientMessage {
    let mut message = colliding_spawn(id, false, 1, "cat");
    if let ClientMessage::SpawnPane { placement, .. } = &mut message {
        let mut panes: Vec<_> = server
            .panes
            .iter()
            .filter(|(_, pane)| pane.exited.is_none())
            .map(|(id, pane)| (*id, pane.generation))
            .collect();
        panes.push((id, 1));
        *placement = Some(protocol::SpawnPlacementCommit {
            base_rev: server.layout_rev,
            layout: shared_layout_for_panes(&panes),
        });
    }
    message
}

fn accepted(messages: &[(Target, ServerMessage)]) -> bool {
    messages
        .iter()
        .any(|(_, message)| matches!(message, ServerMessage::SpawnResult { ok: true, .. }))
}

fn assert_placed(server: &SessionServer) {
    let layout = server
        .layout
        .as_ref()
        .expect("successful spawn publishes its document");
    layout.validate().unwrap();
    assert!(server.validate_shared_layout_against_panes(layout));
    for id in server.panes.keys() {
        assert_eq!(
            layout
                .workspaces
                .iter()
                .flat_map(|ws| &ws.panes)
                .filter(|pane| pane.pane_id == *id)
                .count(),
            1
        );
    }
}

#[test]
fn atomic_spawn_validates_authority_revision_and_document_before_starting_a_pty() {
    let mut server = SessionServer::new_named("atomic");
    let (controller, _a) = attach_client(&mut server);
    let (follower, _b) = attach_client(&mut server);
    let original = spawn(&server, 1);
    assert!(!accepted(
        &server.handle_message(follower, original.clone())
    ));
    server.client_mut(controller).unwrap().read_only = true;
    assert!(!accepted(
        &server.handle_message(controller, original.clone())
    ));
    server.client_mut(controller).unwrap().read_only = false;

    let mut missing = original.clone();
    if let ClientMessage::SpawnPane { placement, .. } = &mut missing {
        *placement = None;
    }
    assert!(!accepted(&server.handle_message(controller, missing)));
    let mut stale = original.clone();
    if let ClientMessage::SpawnPane {
        placement: Some(placement),
        ..
    } = &mut stale
    {
        placement.base_rev = 1;
    }
    assert!(!accepted(&server.handle_message(controller, stale)));
    let mut invalid = original.clone();
    if let ClientMessage::SpawnPane {
        placement: Some(placement),
        ..
    } = &mut invalid
    {
        placement.layout.workspaces[0].panes.clear();
    }
    assert!(!accepted(&server.handle_message(controller, invalid)));
    let mut duplicate = original.clone();
    if let ClientMessage::SpawnPane {
        placement: Some(placement),
        ..
    } = &mut duplicate
    {
        let pane = placement.layout.workspaces[0].panes[0].clone();
        placement.layout.workspaces[0].panes.push(pane);
    }
    assert!(!accepted(&server.handle_message(controller, duplicate)));
    assert!(server.panes.is_empty());
    assert!(server.layout.is_none());
    assert_eq!(server.layout_rev, 0);
    assert_eq!(server.next_generation, 1);

    server.remove_client(controller);
    assert!(!accepted(&server.handle_message(controller, original)));
    assert!(
        server.panes.is_empty(),
        "disconnect before request cannot start a process"
    );
}

#[test]
fn failed_spawn_does_not_publish_a_revision() {
    let mut server = SessionServer::new_named("atomic");
    let (controller, _a) = attach_client(&mut server);
    let mut message = spawn(&server, 1);
    if let ClientMessage::SpawnPane { launch, .. } = &mut message {
        *launch = Some(crate::pane::launch::PaneLaunch::Direct {
            argv: vec!["/rozi-test-no-such-program/never-exists".into()],
        });
    }
    assert!(!accepted(&server.handle_message(controller, message)));
    assert!(server.panes.is_empty());
    assert!(server.layout.is_none());
    assert_eq!(server.layout_rev, 0);
}

#[test]
fn first_and_later_spawns_publish_one_revision_and_survive_immediate_disconnect() {
    let mut server = SessionServer::new_named("atomic");
    let (controller, _a) = attach_client(&mut server);
    let (follower, _b) = attach_client(&mut server);
    for id in [1, 2] {
        let message = spawn(&server, id);
        let replies = server.handle_message(controller, message);
        assert!(accepted(&replies));
        assert_eq!(server.layout_rev, u64::from(id));
        let commits: Vec<_> = replies
            .iter()
            .filter(|(_, message)| matches!(message, ServerMessage::LayoutCommitted { .. }))
            .collect();
        assert_eq!(commits.len(), 1);
        let metadata = replies
            .iter()
            .position(|(_, m)| matches!(m, ServerMessage::PaneMetadata { .. }))
            .unwrap();
        let commit = replies
            .iter()
            .position(|(_, m)| matches!(m, ServerMessage::LayoutCommitted { .. }))
            .unwrap();
        assert!(
            metadata < commit,
            "followers learn capacity before creating their parser"
        );
        assert_placed(&server);
    }
    let committed = server.layout.clone();
    server.remove_client(controller);
    assert_eq!(server.controller, Some(follower));
    assert_eq!(server.layout, committed);
    assert_placed(&server);
    server.remove_client(follower);
    assert_placed(&server);
}

#[test]
fn server_reload_changes_only_future_pane_capacities_and_snapshot_preserves_them() {
    let _config = crate::test_support::lock_config_file();
    let mut server = SessionServer::new_named("atomic");
    server.settings.scrollback = 7;
    let (controller, _a) = attach_client(&mut server);
    let first = spawn(&server, 1);
    assert!(accepted(&server.handle_message(controller, first)));
    // The server reload path reads its own config, never a capacity provided by a client.
    let config = crate::config::config_path();
    let previous = std::fs::read(&config).ok();
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&config, "scrollback = 19\n").unwrap();
    server.handle_message(controller, ClientMessage::ReloadSpawnPolicy);
    let second = spawn(&server, 2);
    assert!(accepted(&server.handle_message(controller, second)));
    assert_eq!(server.panes[&1].scrollback, 7);
    assert_eq!(server.panes[&2].scrollback, 19);
    let capacities: std::collections::HashMap<_, _> = server
        .pane_meta()
        .iter()
        .map(|meta| (meta.pane_id, meta.scrollback))
        .collect();
    assert_eq!(capacities[&1], 7);
    assert_eq!(capacities[&2], 19);
    let root = std::env::temp_dir().join(format!(
        "rozi-atomic-history-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    server.settings.resurrect = true;
    server.settings.snapshot_dir = Some(root.clone());
    server.snapshot_before_shutdown().unwrap();
    let mut restored = SessionServer::new_named_with_settings(
        "atomic",
        ServerSettings {
            scrollback: 99,
            resurrect: true,
            snapshot_dir: Some(root.clone()),
            ..Default::default()
        },
    );
    assert_eq!(restored.restore().unwrap(), 2);
    assert_eq!(restored.panes[&1].scrollback, 7);
    assert_eq!(restored.panes[&2].scrollback, 19);
    assert_placed(&restored);
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
    match previous {
        Some(bytes) => std::fs::write(config, bytes).unwrap(),
        None => {
            let _ = std::fs::remove_file(config);
        }
    }
}

fn request(command: crate::control::ControlCommand) -> crate::control::ControlRequest {
    crate::control::ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    }
}

/// Drive both public control dispatchers through a sequence of edits from the same document.
#[test]
fn ui_and_headless_control_commit_identical_documents_for_shared_edits() {
    std::thread::Builder::new().stack_size(16 * 1024 * 1024).spawn(|| {
        use crate::control::{ControlCommand as C, ControlEnvelope, ControlLayoutKind as K};
        use crate::session::client::SessionClient;
        use tui_lipan::TestBackend;
        let mut ui = TestBackend::new(crate::AppRoot::default());
        let viewport = Rect { x: 0, y: 0, w: 100, h: 30 };
        ui.set_viewport(viewport);
        for ws in &mut ui.state_mut().current_mut().workspaces { ws.panes.clear(); ws.tile_tree = None; }
        for id in [1, 2, 3] {
            let mut pane = crate::state::Pane::new(id, 7, FloatRect::default());
            pane.pty_generation = 1;
            ui.state_mut().current_mut().workspaces[0].panes.push(pane);
        }
        let bounds = ui.state().canvas_bounds_from_terminal_viewport(viewport);
        let canvas = (bounds.w.max(1.0) as u16, bounds.h.max(1.0) as u16);
        ui.state_mut().current_mut().workspaces[0].split_ratios.clear();
        let layout = crate::layout::shared::shared_layout_from_state(ui.state(), canvas);
        let mut server = SessionServer::new_named("parity");
        server.layout = Some(layout.clone());
        for id in [1, 2, 3] { server.panes.insert(id, test_pane(1)); }
        let (client, _outbound) = SessionClient::test_channel();
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.last_committed_layout = Some(layout);
        shared.canonical_canvas = Some(canvas);
        ui.state_mut().current_mut().shared = Some(shared);
        ui.state_mut().current_mut().session_client = Some(client);
        ui.state_mut().current_mut().session_attached = true;

        let pane_edit = |floating, fullscreen, split_ratio, width_ratio| C::PaneSet {
            target: 2, floating, fullscreen, rect: None, rect_fraction: None,
            split_ratio, width_ratio, if_revision: None,
        };
        let commands = [
            C::LayoutSet { workspace: 1, layout: Some(K::Master), master_ratio: None, if_revision: None },
            C::LayoutSet { workspace: 1, layout: None, master_ratio: Some(f64::from(crate::state::DEFAULT_RATIO)), if_revision: None },
            C::LayoutSet { workspace: 1, layout: None, master_ratio: Some(f64::from(crate::state::DEFAULT_RATIO)), if_revision: None },
            C::LayoutSet { workspace: 1, layout: Some(K::Master), master_ratio: Some(0.65), if_revision: None },
            C::LayoutSet { workspace: 1, layout: Some(K::Scrollable), master_ratio: None, if_revision: None },
            pane_edit(None, None, None, Some(0.7)),
            C::LayoutSet { workspace: 1, layout: Some(K::Dwindle), master_ratio: None, if_revision: None },
            pane_edit(None, None, Some(0.6), None),
            pane_edit(Some(true), Some(true), None, None),
            C::PaneSet { target: 2, floating: None, fullscreen: None, rect: None,
                rect_fraction: Some(crate::control::FractionRect { x: -0.1, y: 0.2, width: 0.45, height: 0.6 }),
                split_ratio: None, width_ratio: None, if_revision: None },
            pane_edit(Some(false), Some(false), None, None),
            C::PaneSwap { target: 1, with: 2, if_revision: None },
            C::PaneMove { target: 2, workspace: 3, if_revision: None },
            C::PaneClose { target: 2, if_revision: None },
        ];
        for command in commands {
            let (reply, response) = std::sync::mpsc::channel();
            ui.dispatch(crate::Msg::ControlRequest(ControlEnvelope { request: request(command.clone()), reply, worker: None })).unwrap();
            let response = response.recv().unwrap();
            assert!(response.ok, "UI {command:?}: {response:?}");
            let messages = server.handle_session_control(99, "parity".into(), PROTOCOL_VERSION,
                protocol::MIN_SUPPORTED_PROTOCOL, None, request(command.clone()));
            assert!(messages.iter().any(|(_, message)| matches!(message, ServerMessage::SessionControlResult { response, .. } if response.ok)), "headless {command:?}: {messages:?}");
            let headless = messages.iter().find_map(|(_, message)| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response), _ => None
            }).unwrap();
            assert_eq!(response.data.as_ref().and_then(|data| data.get("changed")), headless.data.as_ref().and_then(|data| data.get("changed")), "{command:?}");
            assert_eq!(ui.state().current().shared.as_ref().unwrap().assumed_rev, server.layout_rev, "{command:?}");
            let actual = crate::layout::shared::shared_layout_from_state(ui.state(), canvas);
            assert_eq!(Some(&actual), server.layout.as_ref(), "{command:?}");
        }
    }).unwrap().join().unwrap();
}

#[test]
fn atomic_spawn_and_ordinary_commit_can_retain_a_known_exited_pane() {
    let mut server = SessionServer::new_named("retained");
    let (controller, _stream) = attach_client(&mut server);
    let mut exited = test_pane(1);
    exited.exited = Some(7);
    server.panes.insert(1, exited);
    server.layout = Some(shared_layout_for_panes(&[(1, 1)]));
    server.layout_rev = 1;
    let mut message = spawn(&server, 2);
    if let ClientMessage::SpawnPane {
        placement: Some(placement),
        ..
    } = &mut message
    {
        placement.layout = shared_layout_for_panes(&[(1, 1), (2, 1)]);
    }
    assert!(accepted(&server.handle_message(controller, message)));
    assert_eq!(server.layout_rev, 2);
    assert_placed(&server);
    let layout = server.layout.clone().unwrap();
    assert!(
        server
            .handle_commit_layout(controller, 2, layout, false)
            .iter()
            .any(|(_, message)| matches!(message, ServerMessage::LayoutCommitted { rev: 3, .. }))
    );
    assert!(
        !server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(1, 9), (2, 1)]))
    );
}

struct StartupSpawns {
    ui: tui_lipan::TestBackend<crate::AppRoot>,
    outbound: std::sync::mpsc::Receiver<crate::session::client::ClientOutbound>,
    server: SessionServer,
    controller: ClientId,
    _stream: UnixStream,
    attempts: Vec<(PaneId, u64)>,
    results: Vec<(PaneId, bool)>,
}

impl StartupSpawns {
    fn new(fail_middle: bool) -> Self {
        use crate::session::client::SessionClient;
        let mut server = SessionServer::new_named("startup");
        let (controller, stream) = attach_client(&mut server);
        let mut ui = tui_lipan::TestBackend::new(crate::AppRoot::default());
        ui.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        ui.state_mut().config.animations.enabled = false;
        for ws in &mut ui.state_mut().current_mut().workspaces {
            ws.panes.clear();
            ws.tile_tree = None;
        }
        for id in [1, 2, 3] {
            let mut pane = crate::state::Pane::new(id, 100, FloatRect::default());
            if fail_middle && id == 2 {
                pane.identity.launch = Some(crate::pane::launch::PaneLaunch::Direct {
                    argv: vec!["/rozi-test-no-such-program/never-exists".into()],
                });
            }
            ui.state_mut().current_mut().workspaces[0].panes.push(pane);
        }
        // Profile panes can already have deferred requests when the first attachment arrives.
        for id in [1, 2, 3] {
            let identity = ui.state().current().workspaces[0]
                .panes
                .iter()
                .find(|pane| pane.id == id)
                .unwrap()
                .identity
                .clone();
            crate::pane::lifecycle::request_pane_spawn(
                ui.state_mut(),
                crate::pane::lifecycle::PaneSpawnRequest {
                    pane_id: id,
                    local: false,
                    generation: 0,
                    identity,
                    cols: 80,
                    rows: 24,
                    env: Vec::new(),
                    palette: TerminalColorPalette::default(),
                },
            );
        }
        let (client, outbound) = SessionClient::test_channel();
        ui.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: 0,
                request: crate::state::AttachRequest::new(
                    "startup",
                    crate::state::AttachMode::Join,
                ),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        ui.dispatch(crate::Msg::SessionAttached {
            epoch: 0,
            session_instance: server.instance_id.clone(),
            session: "startup".into(),
            client_id: controller,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(controller),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: true,
            read_only: false,
            origin: Default::default(),
        })
        .unwrap();
        Self {
            ui,
            outbound,
            server,
            controller,
            _stream: stream,
            attempts: Vec::new(),
            results: Vec::new(),
        }
    }

    fn step(&mut self) -> bool {
        let outbound: Vec<_> = self.outbound.try_iter().collect();
        if outbound.is_empty() {
            return false;
        }
        for frame in outbound {
            let crate::session::client::ClientOutbound::Control(message) = frame else {
                continue;
            };
            if let ClientMessage::SpawnPane {
                pane_id,
                placement: Some(placement),
                ..
            } = &message
            {
                self.attempts.push((*pane_id, placement.base_rev));
            }
            for (_, message) in self.server.handle_message(self.controller, message) {
                if let ServerMessage::SpawnResult { pane_id, ok, .. } = &message {
                    self.results.push((*pane_id, *ok));
                }
                let message =
                    crate::session::bootstrap::server_message_to_msg(0, Frame::Control(message));
                self.ui.dispatch(message).unwrap();
            }
        }
        true
    }

    fn finish(&mut self) {
        for _ in 0..30 {
            if !self.step() {
                break;
            }
        }
        assert_eq!(self.results, [(1, true), (2, false), (3, true)]);
        assert_eq!(self.server.panes.len(), 2);
        assert_placed(&self.server);
        assert!(self.ui.state().current().pending_spawns.is_empty());
        assert!(
            self.ui
                .state()
                .current()
                .shared
                .as_ref()
                .unwrap()
                .spawn_in_flight
                .is_none()
        );
    }
}

fn startup_test(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn queued_startup_spawns_continue_after_the_middle_pty_fails() {
    startup_test(|| {
        let mut test = StartupSpawns::new(true);
        test.finish();
        assert_eq!(test.attempts, [(1, 0), (2, 1), (3, 1)]);
        assert_eq!(test.server.layout_rev, 2);
    });
}

#[test]
fn queued_startup_spawns_rebase_after_a_stale_middle_transaction() {
    startup_test(|| {
        let mut test = StartupSpawns::new(false);
        test.step();
        test.server.layout_rev = 4;
        test.finish();
        assert_eq!(test.attempts, [(1, 0), (2, 1), (3, 4)]);
    });
}

#[test]
fn queued_startup_spawns_wait_for_control_after_a_rejected_transaction() {
    startup_test(|| {
        let mut test = StartupSpawns::new(false);
        test.step();
        test.server.controller = None;
        test.ui
            .dispatch(crate::Msg::SessionControllerChanged {
                epoch: 0,
                controller: None,
                reason: protocol::ControllerChangeReason::Released,
            })
            .unwrap();
        test.step();
        assert_eq!(test.attempts, [(1, 0), (2, 1)]);
        test.server.controller = Some(test.controller);
        test.ui
            .dispatch(crate::Msg::SessionControllerChanged {
                epoch: 0,
                controller: Some(test.controller),
                reason: protocol::ControllerChangeReason::Granted,
            })
            .unwrap();
        test.finish();
        assert_eq!(test.attempts, [(1, 0), (2, 1), (3, 1)]);
    });
}

#[test]
fn queued_startup_spawns_do_not_inherit_a_rejected_read_only_base() {
    startup_test(|| {
        let mut test = StartupSpawns::new(false);
        test.step();
        test.server.client_mut(test.controller).unwrap().read_only = true;
        test.step();
        test.server.client_mut(test.controller).unwrap().read_only = false;
        test.finish();
        assert_eq!(test.attempts, [(1, 0), (2, 1), (3, 1)]);
    });
}

#[test]
fn attaching_to_a_layout_with_a_retained_exit_preserves_its_status() {
    startup_test(|| {
        let mut test = StartupSpawns::new(true);
        test.finish();
        let pane = test.server.panes.get_mut(&1).unwrap();
        pane.exited = Some(7);
        let mut ui = tui_lipan::TestBackend::new(crate::AppRoot::default());
        let (client, _outbound) = crate::session::client::SessionClient::test_channel();
        ui.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: 0,
                request: crate::state::AttachRequest::new(
                    "startup",
                    crate::state::AttachMode::Join,
                ),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        ui.dispatch(crate::Msg::SessionAttached {
            epoch: 0,
            session_instance: test.server.instance_id.clone(),
            session: "startup".into(),
            client_id: 99,
            panes: test.server.pane_meta(),
            layout_rev: test.server.layout_rev,
            layout: test.server.layout.clone(),
            controller: Some(test.controller),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: true,
            read_only: false,
            origin: Default::default(),
        })
        .unwrap();
        let pane = crate::pane::lifecycle::find_pane(ui.state(), 1).unwrap();
        assert_eq!(pane.terminal.status, ManagedTerminalStatus::Exited(7));
        assert!(!pane.closing);
        assert_eq!(
            pane.terminal.scrollback_capacity(),
            test.server.panes[&1].scrollback
        );
    });
}
