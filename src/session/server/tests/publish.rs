//! `publish` streams the session server holds for panes no UI serves.

use super::*;
use crate::control::{ControlCommand, ControlRequest};
use crate::session::protocol::{
    Capabilities, ClientMessage, Frame, PROTOCOL_VERSION, PublishedRow, PublisherKind, RowKey,
    ServerMessage,
};

fn row(id: &str) -> PublishedRow {
    PublishedRow {
        publisher: String::new(),
        kind: PublisherKind::Activity,
        live_session: None,
        reported: false,
        id: id.into(),
        title: format!("task {id}"),
        status: "working".into(),
        reason: None,
        active: false,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: None,
    }
}

/// Open a `publish` stream for `pane` on a fresh headless connection, as `rozi publish` does from
/// inside the pane, and return the connection.
fn open_publisher(server: &mut SessionServer, pane: PaneId, name: &str) -> ClientId {
    let (id, stream) = add_client(server);
    std::mem::forget(stream);
    server.process_client_frame(
        id,
        Frame::Control(ClientMessage::SessionControl {
            capabilities: Some(Capabilities::current()),
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            request: ControlRequest {
                command: ControlCommand::Publish {
                    kind: PublisherKind::Activity,
                    name: Some(name.into()),
                },
                source_pane: Some(pane),
                source_session: Some(server.instance_id().clone()),
                extension: None,
                caller_context: None,
                credential: None,
            },
        }),
    );
    let replies = decode_outbox_controls(server.client(id).unwrap());
    assert!(
        matches!(
            replies.as_slice(),
            [ServerMessage::SessionControlResult { response, .. }] if response.ok
        ),
        "{replies:?}"
    );
    clear_outboxes(server, &[id]);
    id
}

fn publish(server: &mut SessionServer, connection: ClientId, rows: &[&str]) {
    server.process_client_frame(
        connection,
        Frame::Control(ClientMessage::PublishRows {
            responses: Vec::new(),
            rows: rows.iter().map(|id| row(id)).collect(),
        }),
    );
}

fn shown(server: &SessionServer, pane: PaneId) -> Vec<(String, String)> {
    server.panes[&pane]
        .runtime
        .rows
        .iter()
        .map(|row| (row.publisher.clone(), row.id.clone()))
        .collect()
}

fn activations(server: &SessionServer, connection: ClientId) -> Vec<String> {
    decode_outbox_controls(server.client(connection).unwrap())
        .into_iter()
        .filter_map(|message| match message {
            ServerMessage::PublishActivation { row } => Some(row),
            _ => None,
        })
        .collect()
}

/// A restarted publisher opens a new connection and takes over the same rows; the connection it
/// replaced can neither change nor withdraw them, even when it closes afterwards.
#[test]
fn a_new_stream_takes_over_a_publisher_and_the_old_one_cannot_withdraw_it() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(3, test_pane(4));
    let first = open_publisher(&mut server, 3, "jobs");
    publish(&mut server, first, &["a"]);
    assert_eq!(shown(&server, 3), [("0/publish:jobs".into(), "a".into())]);

    let second = open_publisher(&mut server, 3, "jobs");
    assert!(server.client(first).unwrap().close_after_flush);
    publish(&mut server, first, &["stale"]);
    publish(&mut server, second, &["b"]);
    server.remove_client(first);
    assert_eq!(shown(&server, 3), [("0/publish:jobs".into(), "b".into())]);

    // The stream that holds the publisher withdraws its rows when it goes.
    server.remove_client(second);
    assert!(shown(&server, 3).is_empty());
}

/// A stream speaks for the pane generation it opened in, never for the pane's next one.
#[test]
fn a_stream_cannot_publish_into_a_later_generation_of_its_pane() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(3, test_pane(4));
    let publisher = open_publisher(&mut server, 3, "jobs");
    server.panes.insert(3, test_pane(5));
    publish(&mut server, publisher, &["a"]);
    assert!(shown(&server, 3).is_empty());
    assert!(server.client(publisher).unwrap().close_after_flush);
    assert!(matches!(
        decode_outbox_controls(server.client(publisher).unwrap()).as_slice(),
        [ServerMessage::Error { code, .. }] if code == "stale-generation"
    ));
}

/// Only a row that is in the pane now reaches its publisher, and only from a client that may type.
#[test]
fn activation_reaches_the_publisher_only_for_a_shown_row_from_a_client_that_may_type() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(3, test_pane(4));
    let publisher = open_publisher(&mut server, 3, "jobs");
    publish(&mut server, publisher, &["a"]);
    let (controller, _c) = attach_client(&mut server);
    let (follower, _f) = attach_client(&mut server);
    server.controller = Some(controller);
    clear_outboxes(&mut server, &[publisher]);
    let activate = |server: &mut SessionServer, client, generation, id: &str| {
        server.process_client_frame(
            client,
            Frame::Control(ClientMessage::ActivatePublishedRow {
                pane_id: 3,
                generation,
                row: RowKey::new("0/publish:jobs", id),
            }),
        );
    };

    activate(&mut server, follower, 4, "a");
    assert_eq!(activations(&server, publisher), ["a"]);

    // A row the publisher never listed, or the right row in another generation, goes nowhere.
    clear_outboxes(&mut server, &[publisher]);
    activate(&mut server, follower, 4, "invented");
    activate(&mut server, follower, 5, "a");
    assert!(activations(&server, publisher).is_empty());

    // Under input lock only the controller makes a program in the pane act.
    server.input_locked = true;
    activate(&mut server, follower, 4, "a");
    assert!(activations(&server, publisher).is_empty());
    activate(&mut server, controller, 4, "a");
    assert_eq!(activations(&server, publisher), ["a"]);
}
