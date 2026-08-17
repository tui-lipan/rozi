mod common;

use rozi::session::protocol::{
    ClientMessage, Frame, MAX_FRAME_SIZE, PROTOCOL_VERSION, ServerMessage,
};
use rozi::session::server::ServerSettings;

use common::{TestConnection, attach_client, read_until, spawn_listener};

#[test]
fn version_mismatch_is_refused_without_stopping_the_listener() {
    let server = spawn_listener(ServerSettings::default());
    let mut client = TestConnection::connect(server.endpoint());
    client.write_control(&ClientMessage::Attach {
        session: server.session().to_string(),
        protocol_version: PROTOCOL_VERSION + 1,
        min_protocol_version: PROTOCOL_VERSION + 1,
        label: "future-client".to_string(),
        read_only: false,
        shares_filesystem: true,
    });
    read_until(&mut client, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::Error { code, .. }) if code == "protocol-mismatch"
        )
    });
    drop(client);

    assert_server_accepts_another_client(&server);
}

#[test]
fn malformed_and_truncated_frames_drop_only_the_bad_clients() {
    let server = spawn_listener(ServerSettings::default());

    let malformed_body = b"\x01{not-json";
    let mut malformed = TestConnection::connect(server.endpoint());
    malformed.write_raw(&(malformed_body.len() as u32).to_be_bytes());
    malformed.write_raw(malformed_body);
    drop(malformed);
    assert_server_accepts_another_client(&server);

    let mut truncated = TestConnection::connect(server.endpoint());
    truncated.write_raw(&32_u32.to_be_bytes());
    truncated.write_raw(b"\x01{");
    drop(truncated);
    assert_server_accepts_another_client(&server);
}

#[test]
fn oversized_frame_is_rejected_without_stopping_the_listener() {
    let server = spawn_listener(ServerSettings::default());
    let mut client = TestConnection::connect(server.endpoint());
    client.write_raw(&((MAX_FRAME_SIZE as u32) + 1).to_be_bytes());
    drop(client);

    assert_server_accepts_another_client(&server);
}

fn assert_server_accepts_another_client(server: &common::ListenerGuard) {
    let (client, attached) = attach_client(server.endpoint(), server.session(), "healthy-client");
    assert!(matches!(attached, ServerMessage::Attached { .. }));
    drop(client);
}
