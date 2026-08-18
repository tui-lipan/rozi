//! This build speaks exactly one wire version, so any peer advertising a range that does not
//! contain it must fail negotiation rather than half-attach.
//!
//! The scripted server is pinned one above the ceiling. Below the floor is unavailable now that
//! the floor is 1, and above tests the same property: ranges that do not overlap are rejected.
//! Pinning relative to [`PROTOCOL_VERSION`] also keeps this honest across future bumps.

use std::io::pipe;
use std::sync::mpsc;
use std::thread;

use rozi::platform::ipc::{IpcConnection, PipedConnection};
use rozi::session::client::SessionClient;
use rozi::session::protocol::{
    self, ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
    negotiate_protocol,
};

const PINNED_SERVER_PROTOCOL: u32 = PROTOCOL_VERSION + 1;

const _: () = assert!(
    MIN_SUPPORTED_PROTOCOL == PROTOCOL_VERSION,
    "this build intentionally speaks one version"
);
const _: () = assert!(
    PINNED_SERVER_PROTOCOL > PROTOCOL_VERSION,
    "the pinned server must sit outside this build's range"
);

#[test]
fn a_server_outside_this_builds_range_cannot_negotiate() {
    assert!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            PINNED_SERVER_PROTOCOL,
            PINNED_SERVER_PROTOCOL,
        )
        .is_err()
    );

    let session = "skew-test".to_string();
    let (server_reader, client_writer) = pipe().expect("client->server pipe");
    let (client_reader, server_writer) = pipe().expect("server->client pipe");

    let server = thread::spawn(move || {
        let mut conn = PipedConnection::from_reader_writer(server_writer, server_reader);
        let attach: ClientMessage = protocol::read_frame(&mut conn).expect("read client attach");
        let (client_max, client_min) = match attach {
            ClientMessage::Attach {
                protocol_version,
                min_protocol_version,
                ..
            } => (protocol_version, min_protocol_version),
            other => panic!("expected Attach, got {other:?}"),
        };
        assert_eq!(client_max, PROTOCOL_VERSION);
        assert_eq!(client_min, MIN_SUPPORTED_PROTOCOL);

        let effective = negotiate_protocol(
            client_max,
            client_min,
            PINNED_SERVER_PROTOCOL,
            PINNED_SERVER_PROTOCOL,
        );
        assert!(effective.is_err(), "ranges must not overlap");

        protocol::write_frame(
            &mut conn,
            &ServerMessage::Error {
                code: "protocol".into(),
                message: "incompatible protocol".into(),
            },
        )
        .expect("send Error");
    });

    let client_conn = IpcConnection::from_piped(PipedConnection::from_reader_writer(
        client_writer,
        client_reader,
    ));
    let (tx, _rx) = mpsc::channel();
    let attach = SessionClient::from_stream_attached(client_conn, session, tx, false);
    assert!(
        attach.is_err(),
        "attach must fail against a peer outside the range"
    );

    server.join().expect("server thread");
}
