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
