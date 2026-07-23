//! A real mixed-version client/server pair over the typed protocol.
//!
//! `negotiate_protocol` is unit-tested in isolation, but nothing exercised a live pair whose builds
//! disagree on the wire version. This drives a real [`SessionClient`] (which always advertises this
//! build's `PROTOCOL_VERSION`) against a scripted server pinned to protocol 12, and asserts two
//! things the additive-changes policy depends on: the connection negotiates 12, and the v13-only
//! file-tree messages are never put on the wire. If a future change lets a v13 client leak a v13
//! message to a v12 peer, this fails.

use std::io::pipe;
use std::sync::mpsc;
use std::thread;

use hyprmux::platform::ipc::{IpcConnection, PipedConnection};
use hyprmux::session::client::SessionClient;
use hyprmux::session::protocol::{
    self, ClientInfo, ClientMessage, FILE_TREE_PROTOCOL, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION,
    ServerMessage, negotiate_protocol,
};

/// The protocol version the scripted server pretends to be — one below this build's file-tree floor.
const PINNED_SERVER_PROTOCOL: u32 = 12;

// The scenario is only meaningful while these hold; assert them at compile time so a future
// version bump that invalidates the setup fails to build rather than passing vacuously.
const _: () = assert!(
    PROTOCOL_VERSION >= FILE_TREE_PROTOCOL,
    "the client build must speak the file-tree protocol"
);
const _: () = assert!(
    PINNED_SERVER_PROTOCOL < FILE_TREE_PROTOCOL,
    "the pinned server must be below the file-tree floor"
);
const _: () = assert!(
    PINNED_SERVER_PROTOCOL >= MIN_SUPPORTED_PROTOCOL,
    "the pinned server must still be within this build's supported range"
);

#[test]
fn v13_client_against_a_v12_server_negotiates_12_and_sends_no_file_tree_messages() {
    let session = "skew-test".to_string();

    // Two pipes make one duplex: client -> server and server -> client.
    let (server_reader, client_writer) = pipe().expect("client->server pipe");
    let (client_reader, server_writer) = pipe().expect("server->client pipe");

    let server_session = session.clone();
    let server = thread::spawn(move || {
        let mut conn = PipedConnection::from_reader_writer(server_writer, server_reader);

        // The client opens with an Attach advertising its full supported range.
        let attach: ClientMessage = protocol::read_frame(&mut conn).expect("read client attach");
        let (client_max, client_min) = match attach {
            ClientMessage::Attach {
                protocol_version,
                min_protocol_version,
                ..
            } => (protocol_version, min_protocol_version),
            other => panic!("expected Attach, got {other:?}"),
        };
        assert_eq!(
            client_max, PROTOCOL_VERSION,
            "the real client must advertise this build's max version"
        );

        // Negotiate as a protocol-12 server and confirm the meeting point is 12.
        let effective = negotiate_protocol(
            client_max,
            client_min,
            PINNED_SERVER_PROTOCOL,
            PINNED_SERVER_PROTOCOL,
        )
        .expect("ranges overlap at 12");
        assert_eq!(effective, PINNED_SERVER_PROTOCOL);

        protocol::write_frame(
            &mut conn,
            &ServerMessage::Attached {
                protocol_version: PINNED_SERVER_PROTOCOL,
                effective_protocol: effective,
                session: server_session,
                client_id: 1,
                panes: Vec::new(),
                layout_rev: 0,
                layout: None,
                controller: Some(1),
                clients: vec![ClientInfo {
                    id: 1,
                    label: "client".into(),
                    read_only: false,
                    requesting_control: false,
                }],
                input_locked: false,
                created_from_profile: None,
            },
        )
        .expect("send Attached");

        // The very next control frame the client emits must be the always-sent RequestControl, not
        // a suppressed file-tree query. If the gate leaked, ListDirectory/ListChanges would arrive
        // here first and this assertion would catch it.
        let next: ClientMessage = protocol::read_frame(&mut conn).expect("read next client frame");
        assert!(
            matches!(next, ClientMessage::RequestControl),
            "expected RequestControl as the first post-attach frame, got {next:?} — a v13-only \
             message leaked to a v12 server"
        );
    });

    let client_conn = IpcConnection::from_piped(PipedConnection::from_reader_writer(
        client_writer,
        client_reader,
    ));
    let (tx, _rx) = mpsc::channel();
    let (client, _attached) =
        SessionClient::from_stream_attached(client_conn, session, tx, false).expect("attach");

    assert_eq!(
        client.effective_protocol(),
        PINNED_SERVER_PROTOCOL,
        "the client must record the negotiated version, not its own max"
    );
    assert!(
        !client.supports_file_tree(),
        "a v12 negotiation must disable the file-tree feature client-side"
    );

    // These two must be swallowed (no wire frame); the RequestControl after them is what the server
    // asserts it actually receives first.
    client.list_directory("/tmp".into(), false);
    client.list_changes("/tmp".into());
    client.request_control();

    server.join().expect("server thread");
    drop(client);
}
