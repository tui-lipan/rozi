use super::*;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;

use crate::shared_layout::SHARED_LAYOUT_VERSION;

/// Minimal placeholder palette for spawn requests in tests.
fn test_palette() -> WirePalette {
    WirePalette {
        foreground: None,
        background: None,
        ansi: [Color::Black; 16],
    }
}

/// A 20x5 pane with no PTY behind it, for tests that only exercise server-side pane bookkeeping.
fn test_pane(generation: u64) -> ServerPane {
    ServerPane {
        generation,
        title: None,
        cwd: None,
        command: None,
        keep_open: false,
        command_completed: false,
        cell: tui_lipan::TerminalCellSize::default(),
        shell: Vec::new(),
        env: Vec::new(),
        palette: test_palette(),
        pty: None,
        terminal: TerminalScreen::new(5, 20, 100),
        content_generation: 0,
        cols: 20,
        rows: 5,
        exited: None,
        log: None,
        runtime: protocol::PaneRuntimeState::default(),
        agent: AgentScratch::default(),
        last_git_read: None,
        initial_cursor_report_primed: false,
    }
}

/// Placeholder resolved interactive shell for spawn requests in tests.
fn test_shell() -> Vec<String> {
    vec!["/bin/sh".to_string()]
}

/// Placeholder resolved command-runner shell for spawn requests in tests.
fn test_command_shell() -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string()]
}

fn status_test_pane(generation: u64, exited: Option<i32>) -> ServerPane {
    ServerPane {
        exited,
        ..test_pane(generation)
    }
}

/// A server with takeover turned off, so a control request has to be granted or declined rather
/// than transferring the lease outright. Takeover is on by default (see
/// [`crate::config::SessionConfig::allow_takeover`]), so the cooperative path only exists in
/// a session that has opted into it — and a test exercising that path has to say so.
fn cooperative_server(session_name: &str) -> SessionServer {
    SessionServer::new_named_with_settings(
        session_name,
        ServerSettings {
            allow_takeover: false,
            ..ServerSettings::default()
        },
    )
}

/// Register a client backed by a socketpair and return its id plus the client-side stream.
fn add_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    server_stream.set_nonblocking(true).unwrap();
    let id = server.next_client_id;
    server.next_client_id += 1;
    server
        .clients
        .push(ClientConn::new(id, IpcConnection::from_unix(server_stream)));
    (id, client_stream)
}

/// Register and attach a client, returning its id and client-side stream.
fn attach_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
    let (id, stream) = add_client(server);
    let responses = server.handle_message(
        id,
        ClientMessage::Attach {
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: format!("client-{id}"),
            read_only: false,
        },
    );
    assert!(
        responses
            .iter()
            .any(|(_, msg)| matches!(msg, ServerMessage::Attached { .. }))
    );
    (id, stream)
}

fn attach_read_only_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
    let (id, stream) = add_client(server);
    server.handle_message(
        id,
        ClientMessage::Attach {
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: format!("viewer-{id}"),
            read_only: true,
        },
    );
    (id, stream)
}

#[test]
fn session_socket_path_rejects_invalid_names() {
    assert!(session_socket_path("dev/../../x").is_err());
}

#[test]
fn slow_client_is_disconnected_at_exact_backlog_boundary() {
    let mut server = SessionServer::new_named("dev");
    server.max_backlog = 32;
    let (id, _stream) = attach_client(&mut server);
    let client = server.client_mut(id).unwrap();
    client.outbox.clear();
    client.outbox_bytes = 0;

    server.push_to_attached(Arc::from(vec![0; 32]));
    assert!(server.client_attached(id));
    assert_eq!(server.client_mut(id).unwrap().outbox_bytes, 32);
    server.push_to_attached(Arc::from(vec![1]));
    assert!(!server.client_attached(id));
}

#[test]
fn two_client_broadcast_shares_one_encoded_allocation() {
    let mut server = SessionServer::new_named("dev");
    let (first, _first_stream) = attach_client(&mut server);
    let (second, _second_stream) = attach_client(&mut server);
    for client in &mut server.clients {
        client.outbox.clear();
        client.outbox_bytes = 0;
    }

    server.broadcast_control(&ServerMessage::Ping { seq: 9 });

    let first_frame = server
        .client_mut(first)
        .unwrap()
        .outbox
        .front()
        .unwrap()
        .clone();
    let second_frame = server
        .client_mut(second)
        .unwrap()
        .outbox
        .front()
        .unwrap()
        .clone();
    assert!(Arc::ptr_eq(&first_frame, &second_frame));
}

#[test]
fn aggregate_outbox_high_water_survives_flush_and_disconnect() {
    let mut server = SessionServer::new_named("dev");
    let (first, mut first_stream) = attach_client(&mut server);
    let (second, mut second_stream) = attach_client(&mut server);

    server.push_to_attached(Arc::from(vec![7; 32]));
    let queued = server.runtime_metrics().client_outboxes;
    assert_eq!(queued.bytes.current_bytes, 64);
    assert_eq!(queued.bytes.high_water_bytes, 64);
    assert_eq!(queued.clients, 2);

    server.flush_clients();
    let mut bytes = [0; 32];
    std::io::Read::read_exact(&mut first_stream, &mut bytes).unwrap();
    std::io::Read::read_exact(&mut second_stream, &mut bytes).unwrap();
    let flushed = server.runtime_metrics().client_outboxes;
    assert_eq!(flushed.bytes.current_bytes, 0);
    assert_eq!(flushed.bytes.high_water_bytes, 64);

    server.remove_client(first);
    server.remove_client(second);
    let disconnected = server.runtime_metrics().client_outboxes;
    assert_eq!(disconnected.bytes.current_bytes, 0);
    assert!(disconnected.bytes.high_water_bytes >= 64);
    assert_eq!(disconnected.clients, 0);
}

#[test]
fn runtime_metrics_request_serves_protocol_19_peers() {
    let mut server = SessionServer::new_named("dev");
    let (current, _stream) = attach_client(&mut server);
    assert!(matches!(
        server
            .handle_message(current, ClientMessage::RequestRuntimeMetrics)
            .as_slice(),
        [(Target::Client(id), ServerMessage::RuntimeMetrics { .. })] if *id == current
    ));

    let (legacy, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        legacy,
        ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: 18,
            min_protocol_version: 12,
            label: "legacy".into(),
            read_only: true,
        },
    );
    assert!(
        matches!(
            responses.as_slice(),
            [(_, ServerMessage::Error { code, .. })] if code == "protocol-mismatch"
        ),
        "older peers are rejected"
    );
}

#[test]
fn pty_ingress_coalesces_only_adjacent_output_for_the_same_pane() {
    let queue = ByteQueue::new(64);
    let first = ServerEvent::Pty(None, 1, 2, TerminalPtyEvent::Output(Arc::from(&b"abc"[..])));
    queue
        .try_push_with(first, 3, ServerEvent::coalesce_output)
        .unwrap();
    let second = ServerEvent::Pty(None, 1, 2, TerminalPtyEvent::Output(Arc::from(&b"def"[..])));
    queue
        .try_push_with(second, 3, ServerEvent::coalesce_output)
        .unwrap();
    queue
        .try_push(ServerEvent::Pty(None, 1, 2, TerminalPtyEvent::Exited(0)), 0)
        .unwrap();

    let ServerEvent::Pty(None, _, _, TerminalPtyEvent::Output(bytes)) = queue.try_pop().unwrap()
    else {
        panic!("expected output")
    };
    assert_eq!(&*bytes, b"abcdef");
    assert!(matches!(
        queue.try_pop(),
        Some(ServerEvent::Pty(None, 1, 2, TerminalPtyEvent::Exited(0)))
    ));
}

#[test]
fn browse_events_account_retained_bytes_in_the_server_queue() {
    let event = ServerEvent::DirectoryListing {
        client_id: 1,
        path: "/tmp/project".to_string(),
        show_hidden: true,
        entries: vec![protocol::WireDirEntry {
            name: "visible.txt".to_string(),
            is_dir: false,
            is_symlink: false,
            ignored: false,
            git_staged: None,
            git_unstaged: Some(protocol::WireChangeState::Modified),
        }],
        error: None,
    };
    let bytes = event.payload_bytes();
    assert!(bytes > 0, "browse events must retain accounted memory");

    let queue = ByteQueue::new(MAX_PTY_INGRESS_BYTES);
    queue.try_push(event, bytes).unwrap();
    assert_eq!(queue.stats().bytes, bytes);

    let change = ServerEvent::ChangeListing {
        client_id: 1,
        root: "/tmp/project".to_string(),
        changes: vec![protocol::WireChange {
            path: "visible.txt".to_string(),
            state: protocol::WireChangeState::Modified,
            staged: false,
        }],
        error: None,
    };
    let change_bytes = change.payload_bytes();
    queue.try_push(change, change_bytes).unwrap();
    assert_eq!(queue.stats().bytes, bytes + change_bytes);
}

#[test]
fn oversized_browse_events_become_bounded_error_results() {
    let entries = vec![
        protocol::WireDirEntry {
            name: "x".repeat(600),
            is_dir: false,
            is_symlink: false,
            ignored: false,
            git_staged: None,
            git_unstaged: None,
        };
        10_000
    ];
    let event = ServerEvent::DirectoryListing {
        client_id: 1,
        path: "/tmp/project".to_string(),
        show_hidden: false,
        entries,
        error: None,
    };
    assert!(event.payload_bytes() > MAX_PTY_INGRESS_BYTES);

    let queue = ByteQueue::new(MAX_PTY_INGRESS_BYTES);
    push_browse_event(&queue, event);
    let Some(ServerEvent::DirectoryListing { entries, error, .. }) = queue.try_pop() else {
        panic!("expected bounded browse error event");
    };
    assert!(entries.is_empty());
    assert!(error.is_some());
}

#[test]
fn browse_admission_queue_is_strictly_bounded() {
    let (jobs, _queue) = std::sync::mpsc::sync_channel(BROWSE_QUEUE_CAPACITY);
    let worker = BrowseWorker {
        jobs: Some(jobs),
        handle: None,
    };
    for index in 0..BROWSE_QUEUE_CAPACITY {
        worker
            .try_submit(BrowseRequest::Changes {
                client_id: 1,
                root: format!("/tmp/{index}"),
            })
            .unwrap();
    }
    assert!(matches!(
        worker.try_submit(BrowseRequest::Changes {
            client_id: 1,
            root: "/tmp/overflow".to_string(),
        }),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
}

#[test]
fn browse_burst_waits_in_pending_queue_and_eventually_completes() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let count = BROWSE_QUEUE_CAPACITY + 8;

    for index in 0..count {
        assert!(
            server
                .handle_message(
                    client_id,
                    ClientMessage::ListDirectory {
                        path: format!("/definitely/not/here/rozi-{index}"),
                        show_hidden: false,
                    },
                )
                .is_empty()
        );
    }
    assert_eq!(server.browse_in_flight.len(), count);

    for _ in 0..count {
        let event = wait_for_server_events(&server, 1).remove(0);
        assert!(server.handle_event(event).is_none());
    }
    assert!(server.browse_in_flight.is_empty());
}

#[test]
fn browse_pending_map_rejects_only_after_total_bound() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let (second_client, _second_stream) = attach_client(&mut server);

    for (client, offset) in [
        (client_id, 0),
        (second_client, MAX_BROWSE_PENDING_PER_CLIENT),
    ] {
        for index in 0..MAX_BROWSE_PENDING_PER_CLIENT {
            assert!(
                server
                    .handle_message(
                        client,
                        ClientMessage::ListDirectory {
                            path: format!("/definitely/not/here/rozi-bound-{}", offset + index),
                            show_hidden: false,
                        },
                    )
                    .is_empty()
            );
        }
    }
    assert_eq!(server.browse_in_flight.len(), MAX_BROWSE_PENDING);

    let response = server.handle_message(
        client_id,
        ClientMessage::ListDirectory {
            path: "/definitely/not/here/rozi-over-bound".to_string(),
            show_hidden: false,
        },
    );
    assert!(matches!(
        response.as_slice(),
        [(Target::Client(id), ServerMessage::DirectoryListing { error: Some(error), .. })]
            if *id == client_id && error.contains("too many")
    ));
    assert_eq!(server.browse_in_flight.len(), MAX_BROWSE_PENDING);
}

fn wait_for_server_events(server: &SessionServer, count: usize) -> Vec<ServerEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::with_capacity(count);
    while events.len() < count && Instant::now() < deadline {
        if let Some(event) = server.events.try_pop() {
            events.push(event);
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(events.len(), count, "timed out waiting for server events");
    events
}

fn decode_outbox_controls(client: &ClientConn) -> Vec<ServerMessage> {
    client
        .outbox
        .iter()
        .filter_map(|bytes| {
            let mut decoder = protocol::FrameDecoder::default();
            decoder.read_from_status(&mut &bytes[..]).ok()?;
            match decoder.next_frame::<ServerMessage>().ok()? {
                Some(Frame::Control(message)) => Some(message),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn filesystem_request_returns_without_a_synchronous_response() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let path = "/definitely/not/here/rozi".to_string();
    let started = Instant::now();

    let responses = server.handle_message(
        client_id,
        ClientMessage::ListDirectory {
            path: path.clone(),
            show_hidden: true,
        },
    );

    assert!(
        responses.is_empty(),
        "filesystem work must not reply inline"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "request handling unexpectedly waited on filesystem work"
    );
    assert!(
        server
            .browse_in_flight
            .contains_key(&BrowseRequestKey::Directory { client_id, path })
    );

    let event = wait_for_server_events(&server, 1).remove(0);
    assert!(matches!(event, ServerEvent::DirectoryListing { .. }));
    server.handle_event(event);
}

#[test]
fn oversized_browse_path_is_rejected_without_pending_state() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let root = "x".repeat(MAX_BROWSE_PATH_BYTES + 1);

    let responses = server.handle_message(client_id, ClientMessage::ListChanges { root });
    assert!(matches!(
        responses.as_slice(),
        [(Target::Client(id), ServerMessage::Error { code, .. })]
            if *id == client_id && code == "browse-path-too-long"
    ));
    assert!(server.browse_in_flight.is_empty());
}

#[test]
fn filesystem_results_route_through_handle_event_to_only_the_requester() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (requester, _requester_stream) = attach_client(&mut server);
    let (read_only, _read_only_stream) = attach_read_only_client(&mut server);
    let root = "/definitely/not/here/rozi".to_string();

    assert!(
        server
            .handle_message(
                requester,
                ClientMessage::ListDirectory {
                    path: root.clone(),
                    show_hidden: true,
                },
            )
            .is_empty()
    );
    assert!(
        server
            .handle_message(read_only, ClientMessage::ListChanges { root: root.clone() },)
            .is_empty()
    );

    for event in wait_for_server_events(&server, 2) {
        assert!(server.handle_event(event).is_none());
    }

    let requester_messages = decode_outbox_controls(server.client_mut(requester).unwrap());
    assert!(requester_messages.iter().any(|message| matches!(
        message,
        ServerMessage::DirectoryListing { path: replied, .. } if replied == &root
    )));
    assert!(
        decode_outbox_controls(server.client_mut(read_only).unwrap())
            .iter()
            .any(|message| matches!(
                message,
                ServerMessage::ChangeListing { root: replied, .. } if replied == &root
            ))
    );
    assert!(decode_outbox_controls(server.client_mut(controller).unwrap()).is_empty());
    assert!(server.browse_in_flight.is_empty());
}

#[test]
fn duplicate_filesystem_requests_share_one_in_flight_job() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let path = "/definitely/not/here/rozi".to_string();
    let directory_request = |show_hidden| ClientMessage::ListDirectory {
        path: path.clone(),
        show_hidden,
    };
    let changes_request = || ClientMessage::ListChanges { root: path.clone() };

    assert!(
        server
            .handle_message(client_id, directory_request(true))
            .is_empty()
    );
    assert!(
        server
            .handle_message(client_id, directory_request(false))
            .is_empty()
    );
    assert!(
        server
            .handle_message(client_id, changes_request())
            .is_empty()
    );
    assert!(
        server
            .handle_message(client_id, changes_request())
            .is_empty()
    );
    assert_eq!(server.browse_in_flight.len(), 2);
    let outbox_before = server.client_mut(client_id).unwrap().outbox.len();
    assert!(matches!(
        server
            .browse_in_flight
            .get(&BrowseRequestKey::Directory {
                client_id,
                path: path.clone(),
            })
            .and_then(|state| state.rerun.as_ref()),
        Some(BrowseRequest::Directory {
            show_hidden: false,
            ..
        })
    ));

    let events = wait_for_server_events(&server, 2);
    assert!(server.events.try_pop().is_none());
    for event in events {
        server.handle_event(event);
    }
    assert_eq!(
        server.client_mut(client_id).unwrap().outbox.len(),
        outbox_before,
        "superseded results stay server-side"
    );
    let reruns = wait_for_server_events(&server, 2);
    assert!(server.events.try_pop().is_none());
    for event in reruns {
        server.handle_event(event);
    }
    assert!(server.client_mut(client_id).unwrap().outbox.len() > outbox_before);
    assert!(server.browse_in_flight.is_empty());
}

#[test]
fn filesystem_completion_after_disconnect_is_harmless() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let root = "/definitely/not/here/rozi".to_string();

    assert!(
        server
            .handle_message(client_id, ClientMessage::ListChanges { root })
            .is_empty()
    );
    server.remove_client(client_id);

    let event = wait_for_server_events(&server, 1).remove(0);
    assert!(server.handle_event(event).is_none());
    assert!(server.clients.is_empty());
    assert!(server.browse_in_flight.is_empty());
}

#[test]
fn disconnect_drops_pending_browse_requests_but_not_submitted_jobs() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let pending_key = BrowseRequestKey::Directory {
        client_id,
        path: "/tmp/pending".to_string(),
    };
    let submitted_key = BrowseRequestKey::Changes {
        client_id,
        root: "/tmp/submitted".to_string(),
    };
    server.browse_in_flight.insert(
        pending_key.clone(),
        BrowseState {
            request: BrowseRequest::Directory {
                client_id,
                path: "/tmp/pending".to_string(),
                show_hidden: false,
            },
            rerun: None,
            submitted: false,
        },
    );
    server.browse_in_flight.insert(
        submitted_key.clone(),
        BrowseState {
            request: BrowseRequest::Changes {
                client_id,
                root: "/tmp/submitted".to_string(),
            },
            rerun: Some(BrowseRequest::Changes {
                client_id,
                root: "/tmp/latest".to_string(),
            }),
            submitted: true,
        },
    );

    server.remove_client(client_id);

    assert!(!server.browse_in_flight.contains_key(&pending_key));
    assert!(
        server
            .browse_in_flight
            .get(&submitted_key)
            .is_some_and(|state| state.rerun.is_none())
    );
}

#[test]
fn cursor_position_report_detection_is_strict() {
    assert!(super::panes::is_cursor_position_report(b"\x1b[1;1R"));
    assert!(super::panes::is_cursor_position_report(b"\x1b[24;120R"));
    assert!(!super::panes::is_cursor_position_report(b"\x1b[6n"));
    assert!(!super::panes::is_cursor_position_report(b"\x1b[;R"));
    assert!(!super::panes::is_cursor_position_report(b"1;1R"));
}

#[test]
fn rename_in_place_updates_session_name() {
    let mut server = SessionServer::new_named("eph-123");
    let response = server.rename_session("renametest-unlikely-xyz".into());
    assert!(
        matches!(response, ServerMessage::Renamed { session } if session == "renametest-unlikely-xyz")
    );
    assert_eq!(server.session_name, "renametest-unlikely-xyz");
}

#[test]
fn rename_rejects_reserved_ephemeral_prefix() {
    let mut server = SessionServer::new_named("eph-123");
    let response = server.rename_session("eph-999".into());
    assert!(matches!(response, ServerMessage::Error { code, .. } if code == "invalid-name"));
    assert_eq!(server.session_name, "eph-123");
}

#[test]
fn attach_reports_protocol_mismatch() {
    let mut server = SessionServer::new_named("dev");
    let (id, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        id,
        ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION + 1,
            min_protocol_version: PROTOCOL_VERSION + 1,
            label: "client".into(),
            read_only: false,
        },
    );
    assert!(
        matches!(responses.as_slice(), [(_, ServerMessage::Error { code, .. })] if code == "protocol-mismatch")
    );
}

#[test]
fn first_attacher_is_granted_control() {
    let mut server = SessionServer::new_named("dev");
    let (id, _stream) = attach_client(&mut server);
    assert_eq!(server.controller, Some(id));
}

#[test]
fn controller_kill_reclaims_server_pane_state() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    server.panes.insert(7, status_test_pane(3, Some(0)));

    server.handle_message(
        client,
        ClientMessage::Kill {
            pane_id: 7,
            local: false,
            generation: 3,
        },
    );

    assert!(!server.panes.contains_key(&7));
    assert!(server.snapshot_dirty());
}

/// The scenario the parked flag exists for: one client keeps `dev` open in the background while it
/// works elsewhere. The next client to attach must get control of `dev` outright, not join as a
/// follower of a connection nobody is looking at.
#[test]
fn a_parked_client_releases_control_so_the_next_attacher_leads() {
    let mut server = SessionServer::new_named("dev");
    let (first, _first_stream) = attach_client(&mut server);
    assert_eq!(server.controller, Some(first));

    server.handle_message(first, ClientMessage::SetParked { parked: true });
    assert_eq!(server.controller, None, "parking gives up the lease");

    let (second, _second_stream) = attach_client(&mut server);
    assert_eq!(server.controller, Some(second));
    assert!(
        server
            .client_roster()
            .iter()
            .any(|client| client.id == first && client.parked),
        "the roster must say which clients are only parked"
    );
}

/// Unparking is how a client comes back to a session it left in the background. It reclaims the
/// lease when nobody took it, and never steals it from a client that did.
#[test]
fn unparking_reclaims_a_free_lease_but_never_steals_one() {
    let mut server = SessionServer::new_named("dev");
    let (first, _first_stream) = attach_client(&mut server);
    server.handle_message(first, ClientMessage::SetParked { parked: true });
    server.handle_message(first, ClientMessage::SetParked { parked: false });
    assert_eq!(server.controller, Some(first), "nobody else held the lease");

    let (second, _second_stream) = attach_client(&mut server);
    server.handle_message(second, ClientMessage::SetParked { parked: true });
    server.handle_message(second, ClientMessage::SetParked { parked: false });
    assert_eq!(
        server.controller,
        Some(first),
        "returning from the background must not steal an occupied session"
    );
}

/// A leaving controller hands the lease to a client that is actually using the session; a parked
/// one would take control of a view nobody is watching, locking out the client in front of the user.
#[test]
fn promotion_skips_parked_clients() {
    let mut server = SessionServer::new_named("dev");
    let (first, _first_stream) = attach_client(&mut server);
    let (second, _second_stream) = attach_client(&mut server);
    let (third, _third_stream) = attach_client(&mut server);
    server.handle_message(second, ClientMessage::SetParked { parked: true });

    server.remove_client(first);

    assert_eq!(server.controller, Some(third));
}

#[test]
fn profile_origin_is_recorded_only_for_an_empty_session_and_never_overwritten() {
    let mut server = SessionServer::new_named("dev");
    let (first, _stream) = attach_client(&mut server);
    server.handle_message(
        first,
        ClientMessage::SetSessionOrigin {
            profile: "too-early".into(),
        },
    );
    assert_eq!(server.created_from_profile, None);
    server.handle_message(
        first,
        ClientMessage::SpawnPane {
            pane_id: 1,
            local: false,
            generation: 1,
            command: None,
            cwd: None,
            cols: 20,
            rows: 5,
            keep_open: false,
            env: Vec::new(),
            title: None,
            palette: test_palette(),
            shell: test_shell(),
            command_shell: test_command_shell(),
            cell_width: 0,
            cell_height: 0,
        },
    );
    server.handle_message(
        first,
        ClientMessage::SetSessionOrigin {
            profile: "work".into(),
        },
    );
    assert_eq!(server.created_from_profile.as_deref(), Some("work"));
    assert!(server.snapshot_dirty());

    let (second, _stream) = attach_client(&mut server);
    server.handle_message(
        second,
        ClientMessage::SetSessionOrigin {
            profile: "other".into(),
        },
    );
    assert_eq!(server.created_from_profile.as_deref(), Some("work"));

    let query = server.handle_query("dev".into(), PROTOCOL_VERSION, PROTOCOL_VERSION);
    assert!(matches!(
        query.as_slice(),
        [(
            Target::Sender,
            ServerMessage::SessionInfo {
                created_from_profile: Some(profile),
                ..
            }
        )] if profile == "work"
    ));
}

#[test]
fn profile_origin_claim_is_ignored_without_seeded_panes() {
    let mut server = SessionServer::new_named("dev");
    server.layout = Some(SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    });
    let (client, _stream) = attach_client(&mut server);
    server.handle_message(
        client,
        ClientMessage::SetSessionOrigin {
            profile: "too-late".into(),
        },
    );
    assert_eq!(server.created_from_profile, None);
}

#[test]
fn second_attacher_is_a_follower() {
    let mut server = SessionServer::new_named("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    assert_eq!(server.controller, Some(first));
    assert_ne!(server.controller, Some(second));
    assert_eq!(server.attached_count(), 2);
}

#[test]
fn server_stalls_do_not_consume_client_heartbeat_deadlines() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let before = Instant::now() - Duration::from_secs(10);
    server.client_mut(client_id).unwrap().last_pong = before;
    let stall = Duration::from_secs(6);

    server.credit_server_stall(stall);

    let after = server.client_mut(client_id).unwrap().last_pong;
    assert!(after.duration_since(before) >= stall);
    assert!(after <= Instant::now());
}

#[test]
fn query_registers_nothing_and_seeds_nothing() {
    let mut server = SessionServer::new_named("dev");
    let (id, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        id,
        ClientMessage::Query {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
        },
    );
    assert!(matches!(
        responses.as_slice(),
        [(Target::Sender, ServerMessage::SessionInfo { .. })]
    ));
    assert_eq!(server.attached_count(), 0);
    assert!(server.client_mut(id).unwrap().outbox.is_empty());
}

#[test]
fn non_controller_commit_is_ignored() {
    let mut server = SessionServer::new_named("dev");
    let (_controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        follower,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
        },
    );
    assert!(responses.is_empty());
    assert_eq!(server.layout_rev, 0);
}

#[test]
fn controller_commit_increments_rev_and_broadcasts_author() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
        },
    );
    assert_eq!(server.layout_rev, 1);
    let [(Target::Broadcast, ServerMessage::LayoutCommitted { rev, author, .. })] =
        responses.as_slice()
    else {
        panic!("expected broadcast commit, got {responses:?}");
    };
    assert_eq!(*rev, 1);
    assert_eq!(*author, controller);
}

#[test]
fn stale_base_rev_is_rejected_with_authoritative_layout() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: layout.clone(),
        },
    );
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
        },
    );
    let [
        (
            Target::Sender,
            ServerMessage::LayoutRejected {
                current_rev,
                layout,
            },
        ),
    ] = responses.as_slice()
    else {
        panic!("expected rejection, got {responses:?}");
    };
    assert_eq!(*current_rev, 1);
    assert!(layout.is_some());
}

#[test]
fn malformed_layout_commit_is_rejected() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let invalid_layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 0, // invalid 0 canvas
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: invalid_layout,
        },
    );
    let [
        (
            Target::Sender,
            ServerMessage::LayoutRejected {
                current_rev,
                layout,
            },
        ),
    ] = responses.as_slice()
    else {
        panic!("expected rejection for malformed layout, got {responses:?}");
    };
    assert_eq!(*current_rev, 0);
    assert!(layout.is_none());
}

#[test]
fn request_control_flags_requester_and_notifies_controller_without_stealing() {
    let mut server = cooperative_server("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    let responses = server.handle_message(second, ClientMessage::RequestControl);
    // A present controller is never stolen from; control stays put.
    assert_eq!(server.controller, Some(first));
    // The requester is flagged in the broadcast roster...
    assert!(responses.iter().any(|(target, message)| matches!(
        (target, message),
        (Target::Broadcast, ServerMessage::ClientsChanged { clients, .. })
            if clients.iter().any(|c| c.id == second && c.requesting_control)
    )));
    // ...and only the controller is notified.
    assert!(responses.iter().any(|(target, message)| matches!(
        (target, message),
        (Target::Client(id), ServerMessage::ControlRequested { from })
            if *id == first && *from == second
    )));
}

/// The controller can remove another client: it is told why, and its connection is closed once that
/// reached the wire. Everyone else — and the session — stays put.
#[test]
fn controller_evicts_another_client_with_a_reason_then_closes_it() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let (other, _s2) = attach_client(&mut server);

    let responses = server.handle_message(controller, ClientMessage::EvictClient { target: other });

    let [(Target::Client(id), ServerMessage::Error { code, message })] = responses.as_slice()
    else {
        panic!("expected one evict error, got {responses:?}");
    };
    assert_eq!(*id, other);
    assert_eq!(code, protocol::EVICTED_ERROR_CODE);
    assert!(message.contains(&format!("#{controller}")), "{message}");
    assert!(
        server
            .clients
            .iter()
            .find(|client| client.id == other)
            .is_some_and(|client| client.close_after_flush)
    );
    assert_eq!(server.controller, Some(controller));
    assert!(!server.shutdown);
}

/// Evicting is a controller power, and never a way to remove yourself.
#[test]
fn eviction_is_refused_from_a_follower_a_read_only_controller_or_at_oneself() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);

    assert!(
        server
            .handle_message(follower, ClientMessage::EvictClient { target: controller })
            .is_empty()
    );
    assert!(
        server
            .handle_message(
                controller,
                ClientMessage::EvictClient { target: controller }
            )
            .is_empty()
    );
    assert!(
        !server.clients.iter().any(|client| client.close_after_flush),
        "a refused eviction must not close anyone"
    );

    // A read-only client holding the lease only because nobody else does cannot use it to evict.
    let mut viewer_server = SessionServer::new_named("dev");
    let (viewer, _s3) = attach_read_only_client(&mut viewer_server);
    let (writer, _s4) = attach_client(&mut viewer_server);
    viewer_server.controller = Some(viewer);
    assert!(
        viewer_server
            .handle_message(viewer, ClientMessage::EvictClient { target: writer })
            .is_empty()
    );
}

#[test]
fn enabled_takeover_transfers_control_on_request() {
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            allow_takeover: true,
            ..ServerSettings::default()
        },
    );
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);

    let responses = server.handle_message(second, ClientMessage::RequestControl);

    assert_eq!(server.controller, Some(second));
    assert!(responses.iter().any(|(_, message)| matches!(
        message,
        ServerMessage::ControllerChanged {
            controller: Some(id),
            reason: ControllerChangeReason::Granted,
        } if *id == second
    )));
    assert!(!responses.iter().any(|(target, message)| matches!(
        (target, message),
        (Target::Client(id), ServerMessage::ControlRequested { .. }) if *id == first
    )));
}

#[test]
fn takeover_still_rejects_read_only_and_parked_clients() {
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            allow_takeover: true,
            ..ServerSettings::default()
        },
    );
    let (controller, _s1) = attach_client(&mut server);
    let (viewer, _s2) = attach_read_only_client(&mut server);
    let (parked, _s3) = attach_client(&mut server);
    server.handle_message(parked, ClientMessage::SetParked { parked: true });

    assert!(
        server
            .handle_message(viewer, ClientMessage::RequestControl)
            .is_empty()
    );
    assert!(
        server
            .handle_message(parked, ClientMessage::RequestControl)
            .is_empty()
    );
    assert_eq!(server.controller, Some(controller));
}

#[test]
fn only_controller_can_toggle_takeover() {
    let mut server = cooperative_server("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);

    assert!(
        server
            .handle_message(second, ClientMessage::SetControlTakeover { allowed: true },)
            .is_empty()
    );
    assert!(!server.allow_takeover);

    let responses =
        server.handle_message(first, ClientMessage::SetControlTakeover { allowed: true });
    assert!(server.allow_takeover);
    assert!(responses.iter().any(|(_, message)| matches!(
        message,
        ServerMessage::ClientsChanged {
            allow_takeover: true,
            ..
        }
    )));
}

#[test]
fn repeated_requests_are_debounced_to_one_controller_notification() {
    let mut server = SessionServer::new_named("dev");
    let (_first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    server.handle_message(second, ClientMessage::RequestControl);
    // Already flagged and inside the notify cooldown: no roster churn, no repeat toast.
    let responses = server.handle_message(second, ClientMessage::RequestControl);
    assert!(responses.is_empty(), "got {responses:?}");
}

#[test]
fn granting_a_requested_control_clears_the_flag() {
    let mut server = cooperative_server("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    server.handle_message(second, ClientMessage::RequestControl);
    let responses = server.handle_message(first, ClientMessage::GrantControl { to: second });
    assert_eq!(server.controller, Some(second));
    assert!(responses.iter().any(|(_, message)| matches!(
        message,
        ServerMessage::ClientsChanged { clients, .. }
            if clients.iter().all(|c| !c.requesting_control)
    )));
}

#[test]
fn declining_control_clears_flag_and_notifies_requester() {
    let mut server = cooperative_server("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    server.handle_message(second, ClientMessage::RequestControl);
    let responses = server.handle_message(first, ClientMessage::DeclineControl { to: second });
    // Control unchanged; the requester is un-flagged and told.
    assert_eq!(server.controller, Some(first));
    assert!(responses.iter().any(|(target, message)| matches!(
        (target, message),
        (Target::Client(id), ServerMessage::ControlDeclined) if *id == second
    )));
    assert!(responses.iter().any(|(_, message)| matches!(
        message,
        ServerMessage::ClientsChanged { clients, .. }
            if clients.iter().all(|c| !c.requesting_control)
    )));
    // A non-controller cannot decline.
    server.handle_message(second, ClientMessage::RequestControl);
    assert!(
        server
            .handle_message(second, ClientMessage::DeclineControl { to: second })
            .is_empty()
    );
}

#[test]
fn removing_controller_promotes_oldest_survivor() {
    let mut server = SessionServer::new_named("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    let (third, _s3) = attach_client(&mut server);
    assert_eq!(server.controller, Some(first));
    server.remove_client(first);
    assert_eq!(server.controller, Some(second));
    let _ = third;
}

#[test]
fn spawn_from_follower_is_rejected() {
    let mut server = SessionServer::new_named("dev");
    let (_controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    let responses = server.handle_message(
        follower,
        ClientMessage::SpawnPane {
            pane_id: 1,
            local: false,
            generation: 1,
            command: None,
            cwd: None,
            cols: 20,
            rows: 5,
            keep_open: false,
            env: Vec::new(),
            title: None,
            palette: test_palette(),
            shell: test_shell(),
            command_shell: test_command_shell(),
            cell_width: 0,
            cell_height: 0,
        },
    );
    assert!(matches!(
        responses.as_slice(),
        [(Target::Sender, ServerMessage::SpawnResult { ok: false, error: Some(error), .. })]
            if error == "not controller"
    ));
    assert!(server.panes.is_empty());
}

fn local_spawn(pane_id: PaneId, generation: u64) -> ClientMessage {
    ClientMessage::SpawnPane {
        pane_id,
        local: true,
        generation,
        command: Some("true".to_string()),
        cwd: None,
        cols: 20,
        rows: 5,
        keep_open: false,
        env: Vec::new(),
        title: None,
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell_width: 0,
        cell_height: 0,
    }
}

#[test]
fn client_local_panes_are_owner_scoped_and_survive_control_transfer() {
    let mut server = SessionServer::new_named("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);

    for owner in [first, second] {
        let response = server.handle_message(owner, local_spawn(7, owner));
        assert!(matches!(
            response.as_slice(),
            [(Target::Sender, ServerMessage::SpawnResult { ok: true, .. })]
        ));
    }
    assert!(server.panes.is_empty());
    assert!(server.local_panes.contains_key(&(first, 7)));
    assert!(server.local_panes.contains_key(&(second, 7)));

    server.handle_message(first, ClientMessage::GrantControl { to: second });
    assert_eq!(server.controller, Some(second));
    server.handle_message(
        first,
        ClientMessage::Resize {
            pane_id: 7,
            local: true,
            generation: first,
            cols: 33,
            rows: 11,
            cell_width: 0,
            cell_height: 0,
        },
    );
    assert_eq!(server.local_panes[&(first, 7)].cols, 33);
    assert_eq!(server.local_panes[&(second, 7)].cols, 20);
}

#[test]
fn client_local_panes_are_not_seeded_and_disconnect_kills_the_ownership_entry() {
    let mut server = SessionServer::new_named("dev");
    let (owner, _s1) = attach_client(&mut server);
    let (other, _s2) = attach_client(&mut server);
    server.handle_message(owner, local_spawn(7, 1));
    assert!(
        server.origin_seed_client.is_none(),
        "a local spawn must not seed shared-session origin"
    );

    server.enqueue_attach_seeds(other);
    assert!(
        server.panes.is_empty(),
        "local pane never joins attach state"
    );
    server.remove_client(owner);
    assert!(
        !server
            .local_panes
            .keys()
            .any(|(client, _)| *client == owner)
    );
}

fn colliding_spawn(pane_id: PaneId, local: bool, generation: u64, command: &str) -> ClientMessage {
    ClientMessage::SpawnPane {
        pane_id,
        local,
        generation,
        command: Some(command.to_string()),
        cwd: None,
        cols: 40,
        rows: 10,
        keep_open: false,
        env: Vec::new(),
        title: None,
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell_width: 0,
        cell_height: 0,
    }
}

fn drain_pty_events(server: &mut SessionServer) {
    while let Some(event) = server.events.try_pop() {
        if let Some(outbound) = server.handle_event(event) {
            server.broadcast_outbound(&outbound);
        }
    }
}

fn wait_until(server: &mut SessionServer, predicate: impl Fn(&mut SessionServer) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        drain_pty_events(server);
        if predicate(server) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for colliding-pane condition");
}

fn pane_text(server: &mut SessionServer, owner: Option<ClientId>, pane_id: PaneId) -> String {
    server
        .pane_mut(owner, pane_id)
        .map(|pane| pane.screen_without_change().snapshot().to_string())
        .unwrap_or_default()
}

fn decode_outbox_pane_bytes(client: &ClientConn) -> Vec<(PaneId, bool, u64, Vec<u8>)> {
    client
        .outbox
        .iter()
        .filter_map(|bytes| {
            let mut decoder = crate::session::protocol::FrameDecoder::default();
            decoder.read_from_status(&mut &bytes[..]).ok()?;
            match decoder.next_frame::<ServerMessage>().ok()? {
                Some(Frame::PaneBytes {
                    pane_id,
                    local,
                    generation,
                    bytes,
                }) => Some((pane_id, local, generation, bytes)),
                _ => None,
            }
        })
        .collect()
}

fn resize_message(
    pane_id: PaneId,
    local: bool,
    generation: u64,
    cols: u16,
    rows: u16,
) -> ClientMessage {
    ClientMessage::Resize {
        pane_id,
        local,
        generation,
        cols,
        rows,
        cell_width: 0,
        cell_height: 0,
    }
}

/// A shared pane and an owner-local pane share numeric id 7. Wire `local` must select exactly one.
#[test]
fn colliding_local_and_shared_pane_ids_are_addressed_independently() {
    let mut server = SessionServer::new_named("dev");
    let (owner, _s1) = attach_client(&mut server);
    let (other, _s2) = attach_client(&mut server);
    const PANE: PaneId = 7;
    const GENERATION: u64 = 1;

    server
        .local_panes
        .insert((owner, PANE), test_pane(GENERATION));
    server.panes.insert(PANE, test_pane(GENERATION));

    server.handle_message(owner, resize_message(PANE, false, GENERATION, 41, 12));
    server.handle_message(owner, resize_message(PANE, true, GENERATION, 33, 9));
    assert_eq!(server.panes[&PANE].cols, 41);
    assert_eq!(server.panes[&PANE].rows, 12);
    assert_eq!(server.local_panes[&(owner, PANE)].cols, 33);
    assert_eq!(server.local_panes[&(owner, PANE)].rows, 9);

    server.handle_message(owner, ClientMessage::GrantControl { to: other });
    assert_eq!(server.controller, Some(other));
    server.handle_message(owner, resize_message(PANE, false, GENERATION, 50, 15));
    server.handle_message(owner, resize_message(PANE, true, GENERATION, 28, 8));
    assert_eq!(
        server.panes[&PANE].cols, 41,
        "a follower must not resize the shared pane"
    );
    assert_eq!(
        server.local_panes[&(owner, PANE)].cols,
        28,
        "the owner still resizes its local pane after losing control"
    );

    for client in &mut server.clients {
        client.outbox.clear();
        client.outbox_bytes = 0;
    }
    assert!(
        server
            .handle_event(ServerEvent::Pty(
                None,
                PANE,
                GENERATION,
                TerminalPtyEvent::Output(b"shared-out".to_vec().into()),
            ))
            .is_none()
    );
    assert!(
        server
            .handle_event(ServerEvent::Pty(
                Some(owner),
                PANE,
                GENERATION,
                TerminalPtyEvent::Output(b"local-out".to_vec().into()),
            ))
            .is_none()
    );

    let owner_frames = decode_outbox_pane_bytes(server.client_mut(owner).unwrap());
    let other_frames = decode_outbox_pane_bytes(server.client_mut(other).unwrap());
    assert!(owner_frames.iter().any(|(id, local, generation, bytes)| {
        *id == PANE && !*local && *generation == GENERATION && bytes == b"shared-out"
    }));
    assert!(owner_frames.iter().any(|(id, local, generation, bytes)| {
        *id == PANE && *local && *generation == GENERATION && bytes == b"local-out"
    }));
    assert!(other_frames.iter().any(|(id, local, generation, bytes)| {
        *id == PANE && !*local && *generation == GENERATION && bytes == b"shared-out"
    }));
    assert!(
        other_frames
            .iter()
            .all(|(id, local, _, bytes)| !(*id == PANE && *local && bytes == b"local-out")),
        "local output must not reach a non-owner"
    );
    assert!(pane_text(&mut server, None, PANE).contains("shared-out"));
    assert!(pane_text(&mut server, Some(owner), PANE).contains("local-out"));
    assert!(!pane_text(&mut server, None, PANE).contains("local-out"));
    assert!(!pane_text(&mut server, Some(owner), PANE).contains("shared-out"));

    server.handle_message(
        other,
        ClientMessage::Kill {
            pane_id: PANE,
            local: true,
            generation: GENERATION,
        },
    );
    assert!(
        server.local_panes.contains_key(&(owner, PANE)),
        "another client cannot kill this owner's local pane"
    );
    server.handle_message(
        owner,
        ClientMessage::Kill {
            pane_id: PANE,
            local: false,
            generation: GENERATION,
        },
    );
    assert!(
        server.panes.contains_key(&PANE),
        "a follower cannot kill the shared pane"
    );
    server.handle_message(
        owner,
        ClientMessage::Kill {
            pane_id: PANE,
            local: true,
            generation: GENERATION,
        },
    );
    assert!(!server.local_panes.contains_key(&(owner, PANE)));
    assert!(server.panes.contains_key(&PANE));

    server
        .local_panes
        .insert((owner, PANE), test_pane(GENERATION));
    server.remove_client(owner);
    assert!(!server.local_panes.contains_key(&(owner, PANE)));
    assert!(
        server.panes.contains_key(&PANE),
        "owner disconnect must not remove the shared pane with the same id"
    );
}

#[test]
fn colliding_local_and_shared_pane_input_is_namespaced() {
    let mut server = SessionServer::new_named("dev");
    let (owner, _s1) = attach_client(&mut server);
    const PANE: PaneId = 7;
    const GENERATION: u64 = 1;

    for local in [true, false] {
        let spawned = server.handle_message(
            owner,
            colliding_spawn(PANE, local, GENERATION, "printf 'READY\\n'; cat"),
        );
        assert!(
            matches!(
                spawned.as_slice(),
                [(Target::Sender, ServerMessage::SpawnResult { ok: true, .. })]
            ),
            "spawn local={local} failed: {spawned:?}"
        );
        assert!(
            server
                .pane(local.then_some(owner), PANE)
                .is_some_and(|pane| pane.pty.is_some()),
            "spawn local={local} did not retain a PTY"
        );
    }
    assert_eq!(server.origin_seed_client, Some(owner));

    wait_until(&mut server, |server| {
        pane_text(server, None, PANE).contains("READY")
            && pane_text(server, Some(owner), PANE).contains("READY")
    });

    server.process_client_frame(
        owner,
        Frame::PaneBytes {
            pane_id: PANE,
            local: false,
            generation: GENERATION,
            bytes: b"FROM-SHARED\n".to_vec(),
        },
    );
    server.process_client_frame(
        owner,
        Frame::PaneBytes {
            pane_id: PANE,
            local: true,
            generation: GENERATION,
            bytes: b"FROM-LOCAL\n".to_vec(),
        },
    );
    wait_until(&mut server, |server| {
        pane_text(server, None, PANE).contains("FROM-SHARED")
            && pane_text(server, Some(owner), PANE).contains("FROM-LOCAL")
    });
    let shared_text = pane_text(&mut server, None, PANE);
    let local_text = pane_text(&mut server, Some(owner), PANE);
    assert!(
        !shared_text.contains("FROM-LOCAL"),
        "shared pane received local input; screen was:\n{shared_text}"
    );
    assert!(
        !local_text.contains("FROM-SHARED"),
        "local pane received shared input; screen was:\n{local_text}"
    );
}

#[test]
fn read_only_and_locked_follower_input_is_denied() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    let (viewer, _s3) = attach_read_only_client(&mut server);
    assert!(server.client_may_input(controller));
    assert!(server.client_may_input(follower));
    assert!(!server.client_may_input(viewer));

    server.handle_message(controller, ClientMessage::SetInputLock { locked: true });
    assert!(server.client_may_input(controller));
    assert!(!server.client_may_input(follower));
}

#[test]
fn writable_follower_sets_sanitized_status_and_broadcasts() {
    let mut server = SessionServer::new_named("dev");
    let (_controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    server.panes.insert(1, status_test_pane(2, None));
    let raw_status = format!("\u{1b}[31m{}\u{1b}[0m\r\n", "é".repeat(70));

    let responses = server.handle_message(
        follower,
        ClientMessage::SetPaneStatus {
            pane_id: 1,
            local: false,
            generation: 2,
            status: Some(raw_status),
            reason: Some("\u{1b}]0;hidden\u{7} needs\u{0} approval\n".into()),
        },
    );

    let [(Target::Broadcast, ServerMessage::PaneRuntimeChanged { state, .. })] =
        responses.as_slice()
    else {
        panic!("expected status broadcast, got {responses:?}");
    };
    let status = state.status.as_ref().unwrap();
    assert_eq!(status.value, "é".repeat(protocol::PANE_STATUS_MAX_LEN));
    assert_eq!(status.reason.as_deref(), Some("needs approval"));
    assert!(status.set_at > 0);
    assert_eq!(state.sequence, 1);
    assert_eq!(server.panes[&1].runtime, *state);
}

#[test]
fn pane_status_rejects_invalid_clients_and_panes() {
    let mut server = SessionServer::new_named("dev");
    let (writable, _s1) = attach_client(&mut server);
    let (viewer, _s2) = attach_read_only_client(&mut server);
    let (unattached, _s3) = add_client(&mut server);
    server.panes.insert(1, status_test_pane(2, None));
    server.panes.insert(2, status_test_pane(4, Some(0)));

    let request = |pane_id, generation| ClientMessage::SetPaneStatus {
        pane_id,
        local: false,
        generation,
        status: Some("working".into()),
        reason: None,
    };
    for (client, message, expected) in [
        (viewer, request(1, 2), "read-only"),
        (unattached, request(1, 2), "attach-required"),
        (writable, request(9, 2), "pane-not-found"),
        (writable, request(1, 3), "stale-generation"),
        (writable, request(2, 4), "pane-exited"),
    ] {
        let responses = server.handle_message(client, message);
        assert!(matches!(
            responses.as_slice(),
            [(Target::Sender, ServerMessage::Error { code, .. })] if code == expected
        ));
    }
}

#[test]
fn pane_status_no_op_reason_change_and_clear_have_single_sequences() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    server.panes.insert(1, status_test_pane(2, None));
    let set = |reason: Option<&str>| ClientMessage::SetPaneStatus {
        pane_id: 1,
        local: false,
        generation: 2,
        status: Some("blocked".into()),
        reason: reason.map(str::to_string),
    };

    assert_eq!(server.handle_message(client, set(Some("one"))).len(), 1);
    let first = server.panes[&1].runtime.clone();
    assert!(server.handle_message(client, set(Some("one"))).is_empty());
    assert_eq!(server.panes[&1].runtime, first);

    assert_eq!(server.handle_message(client, set(Some("two"))).len(), 1);
    assert_eq!(server.panes[&1].runtime.sequence, first.sequence + 1);
    assert_eq!(
        server.panes[&1]
            .runtime
            .status
            .as_ref()
            .and_then(|status| status.reason.as_deref()),
        Some("two")
    );

    let responses = server.handle_message(
        client,
        ClientMessage::SetPaneStatus {
            pane_id: 1,
            local: false,
            generation: 2,
            status: None,
            reason: Some("discarded".into()),
        },
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(server.panes[&1].runtime.status, None);
    assert_eq!(server.panes[&1].runtime.sequence, first.sequence + 2);
    assert!(
        server
            .handle_message(
                client,
                ClientMessage::SetPaneStatus {
                    pane_id: 1,
                    local: false,
                    generation: 2,
                    status: Some("\n\u{1b}[31m".into()),
                    reason: Some("ignored".into()),
                },
            )
            .is_empty()
    );
}

#[test]
fn pane_status_run_start_survives_block_and_reattach() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    server.panes.insert(1, status_test_pane(2, None));

    let set = |status: &str| ClientMessage::SetPaneStatus {
        pane_id: 1,
        local: false,
        generation: 2,
        status: Some(status.into()),
        reason: None,
    };

    server.handle_message(client, set("working"));
    let started = server.panes[&1]
        .runtime
        .work_started_at
        .expect("working starts a run");
    server.handle_message(client, set("blocked"));
    assert_eq!(server.panes[&1].runtime.work_started_at, Some(started));
    server.handle_message(client, set("working"));
    assert_eq!(server.panes[&1].runtime.work_started_at, Some(started));

    assert_eq!(server.pane_meta()[0].runtime.work_started_at, Some(started));
}

#[test]
fn grant_control_validates_sender_and_target() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    let (viewer, _s3) = attach_read_only_client(&mut server);

    assert!(
        server
            .handle_message(follower, ClientMessage::GrantControl { to: controller })
            .is_empty()
    );
    assert_eq!(server.controller, Some(controller));
    assert!(
        server
            .handle_message(controller, ClientMessage::GrantControl { to: viewer })
            .is_empty()
    );
    let responses = server.handle_message(controller, ClientMessage::GrantControl { to: follower });
    assert_eq!(server.controller, Some(follower));
    assert!(responses.iter().any(|(target, message)| matches!(
        (target, message),
        (
            Target::Broadcast,
            ServerMessage::ControllerChanged {
                reason: ControllerChangeReason::Granted,
                ..
            }
        )
    )));
}

#[test]
fn shutdown_accepts_writable_followers_and_rejects_read_only_or_unattached_clients() {
    let mut server = SessionServer::new_named("dev");
    let (_controller, _s1) = attach_client(&mut server);
    let (follower, _s2) = attach_client(&mut server);
    let (_viewer, _s3) = attach_read_only_client(&mut server);

    server.input_locked = true;
    server.handle_message(follower, ClientMessage::Shutdown);
    assert!(server.shutdown);

    let mut read_only_server = SessionServer::new_named("dev");
    let (viewer, _viewer_stream) = attach_read_only_client(&mut read_only_server);
    read_only_server.handle_message(viewer, ClientMessage::Shutdown);
    assert!(!read_only_server.shutdown);

    let mut unattached_server = SessionServer::new_named("dev");
    let (unattached, _unattached_stream) = add_client(&mut unattached_server);
    unattached_server.handle_message(unattached, ClientMessage::Shutdown);
    assert!(!unattached_server.shutdown);
}

#[test]
fn local_named_shutdown_waits_for_the_real_server_to_retire() {
    // A pid and a counter, not 19 digits of nanoseconds: this name goes into a Unix socket path,
    // and macOS caps those at 104 bytes against Linux's 108. The long form fit on one platform and
    // not the other.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let name = format!(
        "sd-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (listener, endpoint) = bind_session_socket(&name).expect("bind session endpoint");
    let thread_endpoint = endpoint.clone();
    let thread_name = name.clone();
    let server_thread = std::thread::spawn(move || {
        let mut server =
            SessionServer::new_named_with_settings(thread_name, ServerSettings::default());
        let result = server.run_listener(listener);
        // `run_named_session_mode` owns this same post-loop retirement in production. Keep the
        // test's direct server thread faithful to that lifecycle so the helper can observe it.
        thread_endpoint.remove_stale();
        result
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !endpoint.is_live() {
        assert!(
            Instant::now() < deadline,
            "server endpoint never became live"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    shutdown_named_session(&name).expect("graceful named shutdown");
    assert!(
        !endpoint.is_live(),
        "shutdown must wait for endpoint retirement"
    );
    server_thread
        .join()
        .expect("server thread joined")
        .expect("server stopped cleanly");
}

#[test]
fn clients_changed_contains_roster_and_lock_state() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let (viewer, _s2) = attach_read_only_client(&mut server);
    server.input_locked = true;
    let ServerMessage::ClientsChanged {
        clients,
        input_locked,
        ..
    } = server.clients_changed()
    else {
        panic!("expected clients changed");
    };
    assert!(input_locked);
    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].id, controller);
    assert_eq!(clients[1].id, viewer);
    assert!(clients[1].read_only);
}

#[test]
fn resize_updates_screen_and_broadcasts_ack() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    server.panes.insert(
        1,
        ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            command: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );

    let responses = server.handle_message(
        controller,
        ClientMessage::Resize {
            pane_id: 1,
            local: false,
            generation: 2,
            cols: 80,
            rows: 24,
            cell_width: 0,
            cell_height: 0,
        },
    );

    assert!(matches!(
        responses.as_slice(),
        [(
            Target::Broadcast,
            ServerMessage::Resized {
                pane_id: 1,
                local: false,
                generation: 2,
                cols: 80,
                rows: 24,
            }
        )]
    ));
    let pane = server.panes.get_mut(&1).unwrap();
    assert_eq!((pane.cols, pane.rows), (80, 24));
    assert_eq!(
        pane.screen_without_change()
            .render_snapshot()
            .text
            .lines()
            .count(),
        24
    );
}

#[test]
fn resize_adopts_the_controllers_cell_size() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    server.panes.insert(1, status_test_pane(2, None));

    server.handle_message(
        controller,
        ClientMessage::Resize {
            pane_id: 1,
            local: false,
            generation: 2,
            cols: 80,
            rows: 24,
            cell_width: 9,
            cell_height: 18,
        },
    );
    let pane = server.panes.get(&1).unwrap();
    // The PTY reports this to the child in pixels, and the client that rendered the resize sizes
    // images against the same cell - a mismatch is images overlapping the text below them.
    assert_eq!(pane.cell, tui_lipan::TerminalCellSize::new(9, 18));
    assert_eq!(
        pane.screen().cell_size(),
        tui_lipan::TerminalCellSize::new(9, 18)
    );

    // A pre-17 client reports no cell size; the last known one stands rather than collapsing.
    server.handle_message(
        controller,
        ClientMessage::Resize {
            pane_id: 1,
            local: false,
            generation: 2,
            cols: 100,
            rows: 30,
            cell_width: 0,
            cell_height: 0,
        },
    );
    let pane = server.panes.get(&1).unwrap();
    assert_eq!(pane.cell, tui_lipan::TerminalCellSize::new(9, 18));
}

#[test]
fn duplicate_spawn_is_rejected() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(
        1,
        ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            command: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );
    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 3,
        command: None,
        cwd: None,
        title: None,
        cols: 20,
        rows: 5,
        keep_open: false,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });
    assert!(matches!(
        result,
        ServerMessage::SpawnResult { ok: false, .. }
    ));
}

#[test]
fn exited_pane_can_be_respawned() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(
        1,
        ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            command: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            cols: 20,
            rows: 5,
            exited: Some(0),
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 3,
        command: Some("true".into()),
        cwd: None,
        title: None,
        cols: 20,
        rows: 5,
        keep_open: false,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });

    assert!(matches!(
        result,
        ServerMessage::SpawnResult {
            pane_id: 1,
            generation: 3,
            ok: true,
            ..
        }
    ));
    assert_eq!(server.panes.get(&1).unwrap().generation, 3);
}

#[test]
fn attach_reports_layout_and_panes() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = ServerPane {
        generation: 8,
        title: Some("editor".into()),
        cwd: Some("/repo".into()),
        command: None,
        keep_open: false,
        command_completed: false,
        cell: tui_lipan::TerminalCellSize::default(),
        shell: Vec::new(),
        env: Vec::new(),
        palette: test_palette(),
        pty: None,
        terminal: TerminalScreen::new(5, 20, 100),
        content_generation: 0,
        cols: 20,
        rows: 5,
        exited: None,
        log: None,
        runtime: protocol::PaneRuntimeState::default(),
        agent: AgentScratch::default(),
        last_git_read: None,
        initial_cursor_report_primed: false,
    };
    pane.screen_mut().process_bytes(b"ready");
    server.panes.insert(4, pane);
    server.layout = Some(SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 20,
        canvas_rows: 5,
        workspaces: Vec::new(),
    });
    server.layout_rev = 7;

    let (id, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        id,
        ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "client".into(),
            read_only: false,
        },
    );
    let Some((
        _,
        ServerMessage::Attached {
            session,
            panes,
            layout_rev,
            layout,
            controller,
            ..
        },
    )) = responses.first()
    else {
        panic!("unexpected responses: {responses:?}");
    };
    assert_eq!(session, "dev");
    assert_eq!(*layout_rev, 7);
    assert!(layout.is_some());
    assert_eq!(*controller, Some(id));
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pane_id, 4);
}

#[test]
fn pane_logging_writes_raw_bytes_under_a_header_and_is_reported_on_attach() {
    let root = std::env::temp_dir().join(format!("rozi-log-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            log_dir: Some(root.clone()),
            resurrect: false,
            ..ServerSettings::default()
        },
    );
    server.panes.insert(1, test_pane(2));

    let changed = server.set_pane_logging(None, 1, 2, true);
    let path = match changed {
        ServerMessage::PaneLoggingChanged {
            enabled: true,
            path: Some(path),
            ..
        } => PathBuf::from(path),
        other => panic!("unexpected response: {other:?}"),
    };
    assert!(server.pane_meta()[0].logging);
    server.handle_event(ServerEvent::Pty(
        None,
        1,
        2,
        TerminalPtyEvent::Output(b"raw\x1b[31m\n".to_vec().into()),
    ));
    let logged = fs::read(&path).unwrap();
    let header_end = logged.windows(5).position(|w| w == b"===\r\n").unwrap() + 5;
    let (header, body) = logged.split_at(header_end);
    assert!(String::from_utf8_lossy(header).contains("session dev · pane 1-2 · 20x5"));
    // Everything after the header is the pane's own bytes, escapes intact.
    assert_eq!(body, b"raw\x1b[31m\n");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.set_pane_logging(None, 1, 2, false);
    server.handle_event(ServerEvent::Pty(
        None,
        1,
        2,
        TerminalPtyEvent::Output(b"later".to_vec().into()),
    ));
    assert_eq!(&fs::read(&path).unwrap()[header_end..], b"raw\x1b[31m\n");
    let _ = fs::remove_dir_all(root);
}

/// rozi injects its own OSC 133 `rozi_exe=` marker through shell integration; a user's log is
/// their program's output, not rozi's protocol. The standard bare marker is what other
/// terminals' integrations emit, so it stays.
#[test]
fn pane_logging_strips_rozi_own_shell_integration_marker() {
    let root = std::env::temp_dir().join(format!("rozi-log-marker-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            log_dir: Some(root.clone()),
            resurrect: false,
            ..ServerSettings::default()
        },
    );
    server.panes.insert(1, test_pane(2));

    let path = match server.set_pane_logging(None, 1, 2, true) {
        ServerMessage::PaneLoggingChanged {
            path: Some(path), ..
        } => PathBuf::from(path),
        other => panic!("unexpected response: {other:?}"),
    };
    server.handle_event(ServerEvent::Pty(
        None,
        1,
        2,
        TerminalPtyEvent::Output(
            b"\x1b]133;A\x1b\\\x1b]133;C;rozi_exe=eza\x1b\\out"
                .to_vec()
                .into(),
        ),
    ));
    let logged = fs::read(&path).unwrap();
    let body = &logged[logged.windows(5).position(|w| w == b"===\r\n").unwrap() + 5..];
    assert_eq!(body, b"\x1b]133;A\x1b\\\x1b]133;C\x1b\\out");
    let _ = fs::remove_dir_all(root);
}

/// An `eph-*` session is disposable, so the logs it wrote are too. A named session's are not.
#[test]
fn ephemeral_session_logs_are_discarded_at_shutdown_and_named_ones_are_kept() {
    for (session, survives) in [("eph-4242", false), ("dev", true)] {
        let root =
            std::env::temp_dir().join(format!("rozi-log-reap-{}-{session}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            session,
            ServerSettings {
                log_dir: Some(root.clone()),
                resurrect: false,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(1, test_pane(2));
        server.set_pane_logging(None, 1, 2, true);
        let dir = root.join(session);
        assert!(dir.is_dir(), "{session}: log directory should exist");

        server.discard_ephemeral_logs();
        assert_eq!(dir.is_dir(), survives, "{session}: unexpected retention");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn semantic_runtime_change_is_queued_after_its_raw_output() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    server.panes.insert(
        1,
        ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            command: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );

    let bytes = b"\x1b]7;file://localhost/repo\x1b\\".to_vec();
    assert!(
        server
            .handle_event(ServerEvent::Pty(
                None,
                1,
                2,
                TerminalPtyEvent::Output(bytes.clone().into()),
            ))
            .is_none()
    );

    let client = server
        .clients
        .iter()
        .find(|item| item.id == client)
        .unwrap();
    assert_eq!(client.outbox.len(), 2);
    assert_eq!(client.outbox[0][4], 2, "raw pane frame must be first");
    assert_eq!(client.outbox[1][4], 1, "runtime control frame must follow");
    assert_eq!(server.panes[&1].runtime.cwd.as_deref(), Some("/repo"));
}

#[test]
fn snapshot_round_trip_skips_exited_panes_and_refreshes_generations() {
    let root = std::env::temp_dir().join(format!("rozi-resurrect-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let settings = ServerSettings {
        resurrect: true,
        snapshot_dir: Some(root.clone()),
        snapshot_interval: Duration::ZERO,
        ..ServerSettings::default()
    };
    let mut server = SessionServer::new_named_with_settings("dev", settings.clone());
    server.created_from_profile = Some("work".into());
    for (id, exited) in [(1, None), (2, Some(0)), (crate::state::POPUP_PANE_ID, None)] {
        let mut screen = TerminalScreen::new(5, 20, 100);
        screen.process_bytes(format!("marker-{id}").as_bytes());
        server.panes.insert(
            id,
            ServerPane {
                generation: id as u64,
                title: Some(format!("pane-{id}")),
                cwd: None,
                command: Some("true".into()),
                keep_open: false,
                command_completed: false,
                cell: tui_lipan::TerminalCellSize::default(),
                shell: Vec::new(),
                env: Vec::new(),
                palette: test_palette(),
                pty: None,
                terminal: screen,
                content_generation: 0,
                cols: 20,
                rows: 5,
                exited,
                log: None,
                runtime: protocol::PaneRuntimeState::default(),
                agent: AgentScratch::default(),
                last_git_read: None,
                initial_cursor_report_primed: false,
            },
        );
    }
    server.layout = Some(SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 20,
        canvas_rows: 5,
        workspaces: Vec::new(),
    });
    server.mark_dirty();
    server.maybe_snapshot().unwrap();
    server.wait_for_snapshots().unwrap();
    // Snapshot again with pane 1 untouched, so the restore below reads a *reused* replay file
    // rather than a freshly exported one. Reuse is the path that could silently drop scrollback,
    // so the round trip has to cover it, not just the first write.
    assert_eq!(
        server.persisted_replays.get(&1),
        Some(&server.panes[&1].content_generation),
        "the second snapshot must be eligible to reuse pane 1, or this round trip is vacuous"
    );
    server.mark_dirty();
    server.maybe_snapshot().unwrap();
    server.wait_for_snapshots().unwrap();
    assert!(root.join("dev/meta.json").is_file());
    assert!(root.join("dev/panes/1.replay").is_file());
    assert!(!root.join("dev/panes/2.replay").exists());
    assert!(
        !root
            .join(format!("dev/panes/{}.replay", crate::state::POPUP_PANE_ID))
            .exists()
    );

    let mut restored = SessionServer::new_named_with_settings("dev", settings);
    assert_eq!(restored.restore().unwrap(), 1);
    assert!(restored.panes.contains_key(&1));
    assert!(!restored.panes.contains_key(&2));
    assert!(!restored.panes.contains_key(&crate::state::POPUP_PANE_ID));
    assert!(
        restored
            .panes
            .get_mut(&1)
            .unwrap()
            .screen_without_change()
            .render_snapshot()
            .text
            .contains("marker-1")
    );
    assert_eq!(restored.layout_rev, 1);
    assert_eq!(restored.created_from_profile.as_deref(), Some("work"));
    restored.delete_snapshot().unwrap();
    assert!(!root.join("dev").exists());
    let _ = fs::remove_dir_all(root);
}

/// A `keep_open` pane whose command finishes must not die: the server replaces the dead PTY with the
/// interactive shell in place, and everything the command printed stays on screen above it.
///
/// The status and the surviving scrollback are both load-bearing: they are what the server-driven
/// replacement buys over appending `; exec <shell>` to the command line (cross-platform plan
/// Phase 4).
#[test]
fn keep_open_replaces_the_pty_after_the_command_exits_preserving_status_and_scrollback() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        command: Some("printf 'hello from the command\\n'; exit 3".to_string()),
        cwd: None,
        title: None,
        cols: 40,
        rows: 10,
        keep_open: true,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });
    assert!(matches!(
        result,
        ServerMessage::SpawnResult { ok: true, .. }
    ));

    // Drain PTY events until the command has exited and the replacement shell is running.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_exit_broadcast = false;
    while Instant::now() < deadline {
        while let Some(event) = server.events.try_pop() {
            if let Some(outbound) = server.handle_event(event) {
                if matches!(
                &outbound,
                ServerOutbound::Control(message) if matches!(**message, ServerMessage::Exited { .. })
                ) {
                    saw_exit_broadcast = true;
                }
                server.broadcast_outbound(&outbound);
            }
        }
        // Wait for the whole outcome the assertions below read, not just the lifecycle flag.
        // `command_completed` flips on the exit event, while the command's own output is separate
        // PTY traffic that can still be in flight behind it - so breaking here on the flag alone
        // asserted against a screen the output had not reached yet.
        let pane = server.panes.get_mut(&1).expect("pane still exists");
        if pane.command_completed
            && pane
                .screen_without_change()
                .snapshot()
                .contains("hello from the command")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let pane = server.panes.get(&1).expect("pane still exists");
    assert!(
        pane.command_completed,
        "the keep-open replacement never ran"
    );
    // The pane is alive on a fresh PTY - it did not exit with the command.
    assert!(pane.pty.is_some(), "replacement shell is not running");
    assert_eq!(
        pane.exited, None,
        "a keep-open pane must not report an exit"
    );
    assert!(
        !saw_exit_broadcast,
        "clients must not be told the pane exited; it did not"
    );

    // Scrollback continuity: the same TerminalScreen was kept, so the command's output is still
    // there - and the status it exited with was reported into it rather than swallowed.
    let pane = server.panes.get_mut(&1).expect("pane still exists");
    let text = pane.screen_without_change().snapshot();
    assert!(
        text.contains("hello from the command"),
        "the command's own output was lost across the replacement; screen was:\n{text}"
    );
    assert!(
        text.contains("command exited with status 3"),
        "the command's exit status was not reported; screen was:\n{text}"
    );
}

#[test]
fn keep_open_recovers_terminal_modes_before_starting_the_shell() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        command: Some(
            "printf 'primary marker\\n\\033[?1049h\\033[?1003h\\033[?1006h\\033[?1004h\\033[?2004h\\033[?1h\\033=stale app'; exit 3"
                .to_string(),
        ),
        cwd: None,
        title: None,
        cols: 40,
        rows: 10,
        keep_open: true,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });
    assert!(matches!(
        result,
        ServerMessage::SpawnResult { ok: true, .. }
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(event) = server.events.try_pop() {
            if let Some(outbound) = server.handle_event(event) {
                server.broadcast_outbound(&outbound);
            }
        }
        if server
            .panes
            .get(&1)
            .is_some_and(|pane| pane.command_completed)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let pane = server.panes.get_mut(&1).expect("pane still exists");
    assert!(
        pane.command_completed,
        "the keep-open replacement never ran"
    );
    let snapshot = pane.screen_without_change().render_snapshot();
    assert!(
        snapshot.text.contains("primary marker"),
        "primary screen was not restored:\n{}",
        snapshot.text
    );
    assert!(
        snapshot.text.contains("command exited with status 3"),
        "replacement banner was not drawn on the primary screen:\n{}",
        snapshot.text
    );
    assert!(
        !snapshot.text.contains("stale app"),
        "alternate-screen content remained visible:\n{}",
        snapshot.text
    );
    assert_eq!(snapshot.mouse_mode, tui_lipan::MouseModeState::default());
    assert!(!snapshot.key_modes.app_cursor);
    assert!(!snapshot.key_modes.bracketed_paste);
}

#[test]
fn keep_open_popup_retains_output_without_starting_a_shell() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);
    let pane_id = crate::state::POPUP_PANE_ID;

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id,
        generation: 1,
        command: Some("printf 'popup result\\n'; exit 3".to_string()),
        cwd: None,
        title: None,
        cols: 40,
        rows: 10,
        keep_open: true,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });
    assert!(matches!(
        result,
        ServerMessage::SpawnResult { ok: true, .. }
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_exit = false;
    while Instant::now() < deadline {
        while let Some(event) = server.events.try_pop() {
            if let Some(outbound) = server.handle_event(event) {
                saw_exit |= matches!(
                &outbound,
                ServerOutbound::Control(message) if matches!(**message, ServerMessage::Exited { .. })
                );
                server.broadcast_outbound(&outbound);
            }
        }
        // Wait for the whole outcome the assertions below read, not just the exit status. The
        // command's output and its exit arrive as separate PTY events, so breaking on the status
        // alone can leave "popup result" still queued - which is exactly how this failed on a
        // loaded machine.
        if let Some(pane) = server.panes.get_mut(&pane_id)
            && pane.exited == Some(3)
            && pane
                .screen_without_change()
                .snapshot()
                .contains("popup result")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let pane = server.panes.get_mut(&pane_id).expect("popup still exists");
    assert_eq!(pane.exited, Some(3));
    assert!(
        pane.pty.is_none(),
        "a completed popup must remain read-only"
    );
    assert!(saw_exit, "the client must be told the popup completed");
    let text = pane.screen_without_change().snapshot();
    assert!(text.contains("popup result"));
    assert!(text.contains("[exit 3]  Enter/Esc/Space: close"));
}
