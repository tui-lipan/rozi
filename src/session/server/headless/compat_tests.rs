use super::*;
use crate::session::headless::SessionControlConnection;
use std::sync::atomic::{AtomicU32, Ordering};

struct LiveServer {
    name: String,
    endpoint: IpcEndpoint,
    worker: Option<std::thread::JoinHandle<io::Result<()>>>,
}

impl LiveServer {
    fn start() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "wc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let (listener, endpoint) = bind_session_socket(&name).unwrap();
        let thread_endpoint = endpoint.clone();
        let thread_name = name.clone();
        let worker = std::thread::spawn(move || {
            crate::test_support::isolate_user_dirs();
            let mut server = SessionServer::new_named_with_settings(
                thread_name,
                ServerSettings {
                    resurrect: false,
                    ..Default::default()
                },
            );
            server.endpoint = Some(thread_endpoint);
            server.run_listener(listener)
        });
        Self {
            name,
            endpoint,
            worker: Some(worker),
        }
    }

    fn connect(&self) -> IpcConnection {
        let stream = self.endpoint.connect().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        shutdown_named_session(&self.name).unwrap();
        self.worker.take().unwrap().join().unwrap().unwrap();
    }
}

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    }
}

fn read_until(
    stream: &mut IpcConnection,
    accept: impl Fn(&ServerMessage) -> bool,
) -> ServerMessage {
    for _ in 0..32 {
        let message = protocol::read_frame::<_, ServerMessage>(stream).unwrap();
        if accept(&message) {
            return message;
        }
    }
    panic!("expected reply did not arrive");
}

#[test]
fn unknown_headless_command_gets_typed_error_and_server_keeps_serving() {
    let server = LiveServer::start();
    let mut stream = server.connect();
    let mut message = serde_json::to_value(ClientMessage::SessionControl {
        session: server.name.clone(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: protocol::MIN_SUPPORTED_PROTOCOL,
        capabilities: Some(Capabilities::current()),
        request: request(ControlCommand::ListPanes),
    })
    .unwrap();
    message["request"]["cmd"] = "future-command".into();
    message["request"]["future-data"] = serde_json::json!({"nested": [1,2]});
    protocol::write_frame(&mut stream, &message).unwrap();
    let ServerMessage::SessionControlResult { response, .. } =
        protocol::read_frame::<_, ServerMessage>(&mut stream).unwrap()
    else {
        panic!("expected typed result")
    };
    assert!(!response.ok);
    assert_eq!(response.code, Some(ControlErrorCode::Unsupported));
    assert_eq!(
        response.error.as_deref(),
        Some(control::unsupported_command_reason(true))
    );
    let response = crate::session::headless::run_session_control(
        &server.name,
        request(ControlCommand::ListPanes),
    )
    .unwrap();
    assert!(response.ok);
}

#[test]
fn unknown_attached_control_keeps_the_same_connection_open() {
    let server = LiveServer::start();
    let mut stream = server.connect();
    protocol::write_frame(
        &mut stream,
        &ClientMessage::Attach {
            session: server.name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: protocol::MIN_SUPPORTED_PROTOCOL,
            capabilities: Some(Capabilities::current()),
            label: "compat-test".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
            handoff_token: None,
        },
    )
    .unwrap();
    read_until(&mut stream, |message| {
        matches!(message, ServerMessage::Attached { .. })
    });
    let mut message = serde_json::to_value(ClientMessage::AttachedControl {
        request_id: 42,
        request: request(ControlCommand::ActionsList),
    })
    .unwrap();
    message["request"]["cmd"] = "future-command".into();
    protocol::write_frame(&mut stream, &message).unwrap();
    let ServerMessage::AttachedControlResult { response, .. } =
        read_until(&mut stream, |message| {
            matches!(
                message,
                ServerMessage::AttachedControlResult { request_id: 42, .. }
            )
        })
    else {
        unreachable!()
    };
    assert_eq!(response.code, Some(ControlErrorCode::Unsupported));
    assert_eq!(
        response.error.as_deref(),
        Some(control::unsupported_command_reason(true))
    );
    protocol::write_frame(
        &mut stream,
        &ClientMessage::AttachedControl {
            request_id: 43,
            request: request(ControlCommand::ActionsList),
        },
    )
    .unwrap();
    let ServerMessage::AttachedControlResult { response, .. } =
        read_until(&mut stream, |message| {
            matches!(
                message,
                ServerMessage::AttachedControlResult { request_id: 43, .. }
            )
        })
    else {
        unreachable!()
    };
    assert!(response.ok);
    drop(stream);
}

#[test]
fn headless_capability_probe_refuses_a_missing_feature_before_sending() {
    let server = LiveServer::start();
    let mut connection =
        SessionControlConnection::with_capabilities(&server.name, Capabilities::default()).unwrap();
    let error = connection
        .require_capability(protocol::Capability::RuntimeMetrics)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("needs this session on rozi ≥ 0.0.44")
    );
    assert!(
        error
            .to_string()
            .contains(concat!("session runs rozi ", env!("CARGO_PKG_VERSION")))
    );
    assert!(!connection.supports(protocol::Capability::RuntimeMetrics));
    assert!(
        connection
            .run(request(ControlCommand::ListPanes))
            .unwrap()
            .ok
    );
    let mut connection = SessionControlConnection::new(&server.name).unwrap();
    connection
        .require_capability(protocol::Capability::RuntimeMetrics)
        .unwrap();
    assert!(connection.supports(protocol::Capability::RuntimeMetrics));
    // The byte-proxy preflight uses the same Query decoder and refusal helper.
    let mut stream = server.connect();
    crate::session::headless::require_session_capability_over(
        &server.name,
        &mut stream,
        protocol::Capability::RuntimeMetrics,
    )
    .unwrap();
}
