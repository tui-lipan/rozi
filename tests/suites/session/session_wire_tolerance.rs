//! A peer from a newer release sends things this build does not know. Each is handled where it
//! lands - refused, skipped, or never sent - and the connection keeps working, through the real
//! session listener.

use rozi::session::protocol::{
    Capabilities, ClientMessage, Frame, ServerMessage, UNSUPPORTED_MESSAGE_ERROR_CODE,
    write_control_frame,
};
use rozi::session::server::ServerSettings;

use crate::common::{TestConnection, attach_message, read_until, spawn_listener};

/// A frame of `kind` exactly as a newer peer would put it on the wire.
fn raw_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
    frame.push(kind);
    frame.extend_from_slice(payload);
    frame
}

/// A frame kind this build has never heard of.
const FUTURE_KIND: u8 = 0x3f;
/// The bit a sender sets on a frame a receiver may skip.
const IGNORABLE: u8 = 0x80;

fn attach(
    server: &crate::common::ListenerGuard,
    capabilities: Option<Capabilities>,
) -> TestConnection {
    let mut client = TestConnection::connect(server.endpoint());
    let mut message = attach_message(server.session(), "newer-peer");
    if let ClientMessage::Attach {
        capabilities: offered,
        ..
    } = &mut message
    {
        *offered = capabilities;
    }
    client.write_control(&message);
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    client
}

/// The connection still works: a request still gets its reply. `probe` tells the replies apart.
#[track_caller]
fn assert_still_served(client: &mut TestConnection, probe: &str) {
    let path = std::env::temp_dir()
        .join(probe)
        .to_string_lossy()
        .into_owned();
    client.write_control(&ClientMessage::ListDirectory {
        path: path.clone(),
        show_hidden: false,
    });
    read_until(
        client,
        |frame| matches!(frame, Frame::Control(ServerMessage::DirectoryListing { path: listed, .. }) if *listed == path),
    );
}

fn is_unsupported(frame: &Frame<ServerMessage>) -> bool {
    matches!(
        frame,
        Frame::Control(ServerMessage::Error { code, .. }) if code == UNSUPPORTED_MESSAGE_ERROR_CODE
    )
}

#[test]
fn an_unknown_request_is_refused_and_the_next_one_still_works() {
    let server = spawn_listener(ServerSettings::default());
    let mut client = attach(&server, Some(Capabilities::current()));

    let mut frame = Vec::new();
    write_control_frame(
        &mut frame,
        &serde_json::json!({"type": "teleport-pane", "pane_id": 1, "to": "mars"}),
    )
    .unwrap();
    client.write_raw(&frame);
    read_until(&mut client, is_unsupported);

    assert_still_served(&mut client, "after-unknown");
}

#[test]
fn an_ignorable_frame_is_skipped_and_a_critical_one_ends_only_that_connection() {
    let server = spawn_listener(ServerSettings::default());
    let mut client = attach(&server, Some(Capabilities::current()));
    client.write_raw(&raw_frame(FUTURE_KIND | IGNORABLE, b"optional future work"));
    assert_still_served(&mut client, "after-ignorable");

    let mut critical = attach(&server, Some(Capabilities::current()));
    critical.write_raw(&raw_frame(FUTURE_KIND, b"required future work"));
    critical.wait_closed();

    assert_still_served(&mut client, "after-critical");
}

/// The send gate through the real listener: a client that did not negotiate runtime metrics has
/// its request refused in words and never receives a metrics frame; one that did gets them.
#[test]
fn runtime_metrics_reach_only_a_client_that_negotiated_them() {
    let server = spawn_listener(ServerSettings::default());

    let mut older = attach(&server, None);
    older.write_control(&ClientMessage::RequestRuntimeMetrics);
    read_until(&mut older, |frame| {
        assert!(
            !matches!(frame, Frame::Control(ServerMessage::RuntimeMetrics { .. })),
            "a client without the capability was sent runtime metrics"
        );
        is_unsupported(frame)
    });
    assert_still_served(&mut older, "after-refusal");

    let mut current = attach(&server, Some(Capabilities::current()));
    current.write_control(&ClientMessage::RequestRuntimeMetrics);
    read_until(&mut current, |frame| {
        matches!(frame, Frame::Control(ServerMessage::RuntimeMetrics { .. }))
    });
}
