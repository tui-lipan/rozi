//! Shared attach preferences through the UI and a real session server.
//!
//! Keep this AppRoot test in its own binary: isolating the UI's user directories changes process
//! path resolution, while the session protocol suite also tests discovery of already-bound servers.

mod common;

use common::{attach_client, read_until, spawn_listener};
use rozi::session::protocol::{Frame, ServerMessage};
use rozi::session::server::ServerSettings;
use std::time::Instant;

fn attached_client_id(attached: &ServerMessage) -> rozi::layout::shared::ClientId {
    let ServerMessage::Attached { client_id, .. } = attached else {
        panic!("expected attached response");
    };
    *client_id
}

/// Drive the automatic attach choice through the UI and a real session server. The client's
/// preference must never substitute for the policy of the server it joins.
fn automatic_take_attach(allow_takeover: bool) {
    rozi::test_support::isolate_user_dirs();
    let server = spawn_listener(ServerSettings {
        allow_takeover,
        ..ServerSettings::default()
    });
    let (mut controller, controller_attached) =
        attach_client(server.endpoint(), server.session(), "controller");
    let controller_id = attached_client_id(&controller_attached);
    let (tx, inbound) = std::sync::mpsc::channel();
    let (client, attached) = rozi::session::client::SessionClient::connect_attached(
        server.endpoint(),
        server.session(),
        tx,
        false,
    )
    .expect("attach writable client");
    let ServerMessage::Attached {
        session_instance,
        session,
        client_id,
        panes,
        layout_rev,
        layout,
        controller: current,
        clients,
        input_locked,
        allow_takeover: server_takeover,
        origin,
        ..
    } = attached
    else {
        panic!("expected attached")
    };
    assert_eq!(current, Some(controller_id));
    let mut backend = tui_lipan::TestBackend::new(rozi::AppRoot::default());
    backend.state_mut().config.session.on_attach = rozi::config::SessionOnAttach::Take;
    // Deliberately disagree with the server: only the server's live policy should matter.
    backend.state_mut().config.session.allow_takeover = !allow_takeover;
    backend.state_mut().current_mut().pending_session_attach =
        Some(rozi::state::PendingSessionAttach {
            epoch: 1,
            request: rozi::state::AttachRequest::new(
                session.clone(),
                rozi::state::AttachMode::Join,
            ),
            client: Some(client),
            left: None,
            parked_epoch: None,
        });
    backend
        .dispatch(rozi::Msg::SessionAttached {
            epoch: 1,
            session_instance,
            session,
            client_id,
            panes,
            layout_rev,
            layout,
            controller: current,
            clients,
            input_locked,
            allow_takeover: server_takeover,
            read_only: false,
            origin,
        })
        .expect("apply attached session and automatic take policy");
    assert!(backend.state().follow_prompt.is_none());
    let deadline = Instant::now() + crate::common::io_timeout();
    loop {
        let frame = inbound
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("automatic control request reaches server");
        match frame {
            Frame::Control(ServerMessage::ControllerChanged { controller, reason }) => {
                backend
                    .dispatch(rozi::Msg::SessionControllerChanged {
                        epoch: 1,
                        controller,
                        reason,
                    })
                    .expect("apply server controller change");
                if controller == Some(client_id) {
                    assert!(
                        allow_takeover,
                        "cooperative server must retain its controller"
                    );
                    assert!(backend.state().is_controller());
                    break;
                }
            }
            Frame::Control(ServerMessage::ClientsChanged {
                clients,
                input_locked,
                allow_takeover,
            }) => {
                let requesting = clients
                    .iter()
                    .any(|c| c.id == client_id && c.requesting_control);
                backend
                    .dispatch(rozi::Msg::SessionClientsChanged {
                        epoch: 1,
                        clients,
                        input_locked,
                        allow_takeover,
                    })
                    .expect("apply server roster");
                if requesting {
                    assert!(!allow_takeover);
                    assert!(!backend.state().is_controller());
                    assert_eq!(
                        backend
                            .state()
                            .current()
                            .shared
                            .as_ref()
                            .unwrap()
                            .controller,
                        Some(controller_id)
                    );
                    read_until(&mut controller, |frame| {
                        matches!(frame,
                            Frame::Control(ServerMessage::ControlRequested { from }) if *from == client_id
                        )
                    });
                    break;
                }
            }
            _ => {}
        }
    }
    drop(backend);
    drop(controller);
}

#[test]
fn automatic_take_attach_obeys_server_takeover_policy() {
    rozi::test_support::on_large_stack(|| {
        automatic_take_attach(true);
        automatic_take_attach(false);
    });
}
