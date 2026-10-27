use super::panes::ImageMedia;
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;

use crate::layout::shared::SHARED_LAYOUT_VERSION;

/// Minimal placeholder palette for spawn requests in tests.
fn test_palette() -> WirePalette {
    WirePalette {
        foreground: None,
        background: None,
        ansi: [Color::Black; 16],
    }
}

/// A 20x5 pane with no PTY behind it, for tests that only exercise server-side pane bookkeeping.
pub(super) fn test_pane(generation: u64) -> ServerPane {
    ServerPane {
        scrollback: 100,
        generation,
        title: None,
        cwd: None,
        launch: None,
        agent_resume: None,
        keep_open: false,
        command_completed: false,
        cell: tui_lipan::TerminalCellSize::default(),
        shell: Vec::new(),
        env: Vec::new(),
        palette: test_palette(),
        pty: None,
        pty_resize_pending: false,
        terminal: TerminalScreen::new(5, 20, 100),
        content_generation: 0,
        output_sequence: 0,
        output_coalesce_epoch: 0,
        output_times: VecDeque::new(),
        graphics_pacer: Default::default(),
        output_seen: false,
        cols: 20,
        rows: 5,
        exited: None,
        log: None,
        runtime: protocol::PaneRuntimeState::default(),
        program_status_sync: Default::default(),
        agent: AgentScratch::default(),
        program_on_path: None,
        borrowed_program_name: None,
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

#[test]
fn attached_idle_wait_backs_off_but_stays_bounded() {
    let mut wait = ServerIdleWait::default();

    assert_eq!(wait.next_timeout(true, false), Duration::from_millis(1));
    for _ in 0..2 {
        wait.next_timeout(true, false);
    }
    assert_eq!(wait.next_timeout(true, false), Duration::from_millis(2));
    for _ in 0..4 {
        wait.next_timeout(true, false);
    }
    assert_eq!(wait.next_timeout(true, false), Duration::from_millis(4));
    for _ in 0..4 {
        wait.next_timeout(true, false);
    }
    assert_eq!(
        wait.next_timeout(true, false),
        SERVER_ATTACHED_IDLE_WAIT_MAX
    );
    for _ in 0..100 {
        assert!(wait.next_timeout(true, false) <= SERVER_ATTACHED_IDLE_WAIT_MAX);
    }
}

#[test]
fn server_activity_resets_attached_idle_wait_immediately() {
    let mut wait = ServerIdleWait::default();
    for _ in 0..32 {
        wait.next_timeout(true, false);
    }
    assert_eq!(wait.next_timeout(true, true), SERVER_ACTIVE_WAIT);
    assert_eq!(wait.next_timeout(true, false), SERVER_ACTIVE_WAIT);
}

#[test]
fn server_without_clients_keeps_the_existing_wait_and_resets_backoff() {
    let mut wait = ServerIdleWait::default();
    for _ in 0..32 {
        wait.next_timeout(true, false);
    }
    assert_eq!(wait.next_timeout(false, false), SERVER_UNATTACHED_WAIT);
    assert_eq!(wait.next_timeout(true, false), SERVER_ACTIVE_WAIT);
}

/// Register a client backed by a socketpair and return its id plus the client-side stream.
pub(super) fn add_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
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
pub(super) fn attach_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
    let (id, stream) = add_client(server);
    let responses = server.handle_message(
        id,
        ClientMessage::Attach {
            capabilities: None,
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: format!("client-{id}"),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
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
            capabilities: None,
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: format!("viewer-{id}"),
            read_only: true,
            shares_filesystem: true,
            expected_server_nonce: None,
        },
    );
    (id, stream)
}

#[derive(Debug)]
enum DecodedOutboxFrame {
    Control(Box<ServerMessage>),
    Pane {
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
}

fn decode_outbox_frames(client: &ClientConn) -> Vec<DecodedOutboxFrame> {
    client
        .outbox
        .iter()
        .filter_map(|outbox| {
            let mut decoder = protocol::FrameDecoder::default();
            let mut input = &outbox.bytes[..];
            while !input.is_empty() {
                decoder.read_from_status(&mut input).ok()?;
            }
            match decoder.next_frame::<ServerMessage>().ok()? {
                Some(Frame::Control(message)) => {
                    Some(DecodedOutboxFrame::Control(Box::new(message)))
                }
                Some(Frame::PaneOutput {
                    pane_id,
                    generation,
                    bytes,
                    ..
                }) => Some(DecodedOutboxFrame::Pane {
                    pane_id,
                    generation,
                    bytes,
                }),
                _ => None,
            }
        })
        .collect()
}

fn install_manual_replay(
    server: &mut SessionServer,
    client_id: ClientId,
    key: PaneSeedKey,
    bytes: Vec<u8>,
) {
    let remaining = bytes.len() as u64;
    let mut seed = AttachSeedState::new(vec![key]);
    seed.pending.clear();
    seed.manifest.insert(key, PaneSeedState::Replaying);
    seed.exported.insert(key);
    seed.replay = Some(PaneSeedReplay {
        key,
        reset_frame: None,
        spool: PaneReplayStorage::Memory(Cursor::new(bytes)),
        remaining,
    });
    server.client_mut(client_id).unwrap().seed = Some(seed);
}

#[test]
fn session_socket_path_rejects_invalid_names() {
    assert!(session_socket_path("dev/../../x").is_err());
}

/// A pane hands a frame over by naming a file only when every client can open it. One client
/// watching from another machine is enough to change that for all of them, since the panes are
/// shared and a path from this machine would resolve against that one's files: the server then
/// reads the file itself and forwards the pixels inline.
#[test]
fn a_client_that_cannot_reach_the_filesystem_switches_panes_to_inline_relay() {
    let mut server = SessionServer::new_named("dev");
    let (_local, _stream) = attach_client(&mut server);
    let shared = ImageMedia {
        policy: GraphicsMediaPolicy::default(),
        relay: tui_lipan::GraphicsRelayPolicy::NamedAsFiles,
    };
    assert_eq!(server.image_media(), shared);

    let (remote, _remote_stream) = add_client(&mut server);
    server.handle_message(
        remote,
        ClientMessage::Attach {
            capabilities: None,
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "over-ssh".into(),
            read_only: false,
            shares_filesystem: false,
            expected_server_nonce: None,
        },
    );
    assert_eq!(
        server.image_media(),
        ImageMedia {
            policy: GraphicsMediaPolicy::default(),
            relay: tui_lipan::GraphicsRelayPolicy::All,
        }
    );

    server.remove_client(remote);
    assert_eq!(server.image_media(), shared);
}

/// Inline relay sends a replaced image as edits only while every attached client can apply them.
/// One older client, which would keep showing the previous frame, sends everyone whole frames.
#[test]
fn inline_relay_sends_edits_only_while_every_client_applies_them() {
    let mut server = SessionServer::new_named("dev");
    let attach = |server: &mut SessionServer, capabilities| {
        let (client, stream) = add_client(server);
        server.handle_message(
            client,
            ClientMessage::Attach {
                capabilities,
                session: server.session_name.clone(),
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: PROTOCOL_VERSION,
                label: "over-ssh".into(),
                read_only: false,
                shares_filesystem: false,
                expected_server_nonce: None,
            },
        );
        (client, stream)
    };
    let relay = |server: &SessionServer| server.image_media().relay;
    let (_current, _current_stream) = attach(&mut server, Some(protocol::Capabilities::current()));
    assert_eq!(relay(&server), tui_lipan::GraphicsRelayPolicy::AllWithEdits);

    let (older, _older_stream) = attach(&mut server, None);
    assert_eq!(relay(&server), tui_lipan::GraphicsRelayPolicy::All);

    server.remove_client(older);
    assert_eq!(relay(&server), tui_lipan::GraphicsRelayPolicy::AllWithEdits);
}

/// Attach a client that is looking at this session from another machine.
fn attach_remote_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
    let (remote, stream) = add_client(server);
    server.handle_message(
        remote,
        ClientMessage::Attach {
            capabilities: None,
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "over-ssh".into(),
            read_only: false,
            shares_filesystem: false,
            expected_server_nonce: None,
        },
    );
    (remote, stream)
}

/// The server owns consuming transports as local and remote attachments change.
#[test]
fn consuming_temp_files_are_accepted_with_and_without_remote_clients() {
    use base64::Engine as _;

    let mut server = SessionServer::new_named("graphics");
    server.panes.insert(7, test_pane(2));
    let (_local, _local_stream) = attach_client(&mut server);
    let (remote, _remote_stream) = attach_remote_client(&mut server);

    for remote_attached in [true, false] {
        if !remote_attached {
            server.remove_client(remote);
        }
        let source = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-rozi-refused-")
            .tempfile()
            .unwrap();
        std::fs::write(source.path(), [255, 0, 0]).unwrap();
        let named = base64::engine::general_purpose::STANDARD
            .encode(source.path().as_os_str().as_encoded_bytes());
        let frame = format!("\x1b_Ga=T,f=24,s=1,v=1,t=t,i=7,q=2;{named}\x1b\\");
        let screen = server.panes.get_mut(&7).unwrap().screen_mut();
        screen.process_bytes(frame.as_bytes());
        assert!(
            !source.path().exists(),
            "the server must consume temporary files, remote attached: {remote_attached}"
        );
        assert!(
            !screen.capture_frame().images.is_empty(),
            "temporary-file frames must be retained, remote attached: {remote_attached}"
        );
    }
}

/// The pane output the server has queued for a client, in order.
fn sent_pane_output(server: &mut SessionServer, client: ClientId) -> Vec<u8> {
    sent_pane_output_matching(server, client, |_| true)
}

fn sent_pane_output_matching(
    server: &mut SessionServer,
    client: ClientId,
    include: impl Fn(PaneId) -> bool,
) -> Vec<u8> {
    let client = server.client_mut(client).unwrap();
    let mut forwarded = Vec::new();
    while let Some(queued) = client.outbox.pop_front() {
        let mut decoder = protocol::FrameDecoder::default();
        decoder.read_from_status(&mut &queued.bytes[..]).unwrap();
        while let Ok(Some(frame)) = decoder.next_frame::<ServerMessage>() {
            if let Frame::PaneOutput { pane_id, bytes, .. } = frame
                && include(pane_id)
            {
                forwarded.extend_from_slice(&bytes);
            }
        }
    }
    forwarded
}

/// A child that chose `t=f` while only local clients were attached keeps drawing after a client
/// arrives over SSH: the server reads the file and every client receives the pixels inline. Once
/// that client leaves, the name is forwarded again.
#[test]
fn a_file_frame_reaches_a_remote_client_as_inline_pixels() {
    use base64::Engine as _;

    let mut server = SessionServer::new_named("graphics");
    let mut pane = test_pane(2);
    pane.screen_without_change()
        .set_output_transaction_recording(true);
    server.panes.insert(7, pane);
    let (local, _local_stream) = attach_client(&mut server);
    let (remote, _remote_stream) = attach_remote_client(&mut server);
    server.client_mut(local).unwrap().outbox.clear();
    server.client_mut(remote).unwrap().outbox.clear();

    let pixels: Vec<u8> = (0u8..12).collect();
    let path = std::env::temp_dir().join(format!(
        "tty-graphics-protocol-rozi-relay-{}",
        std::process::id()
    ));
    std::fs::write(&path, &pixels).unwrap();
    let named =
        base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
    let frame = format!("\x1b_Ga=T,f=24,s=2,v=2,t=f,i=7,p=9,C=1,q=2;{named}\x1b\\");
    let send = |server: &mut SessionServer| {
        server
            .panes
            .get_mut(&7)
            .unwrap()
            .screen_mut()
            .process_bytes(frame.as_bytes());
        server.send_outbound(
            None,
            &ServerOutbound::PaneOutput {
                media: Vec::new(),
                metadata: protocol::OutputMetadata::default(),
                pane_id: 7,
                local: false,
                generation: 2,
                bytes: frame.as_bytes().to_vec(),
            },
        );
    };

    send(&mut server);
    let forwarded = sent_pane_output(&mut server, remote);
    let text = String::from_utf8_lossy(&forwarded);
    assert!(
        !text.contains("t=f"),
        "a path reached the remote client: {text}"
    );
    let mut receiver = TerminalScreen::new(5, 20, 100);
    receiver.set_image_media_policy(GraphicsMediaPolicy::NONE);
    receiver.process_bytes(&forwarded);
    let capture = receiver.capture_frame();
    let image = capture
        .images
        .first()
        .expect("the remote client received no image");
    // The capture lays the 2x2 picture on its cell's canvas; compare its two rows there.
    let stride = image.width as usize * 4;
    let rgba = |rgb: &[u8]| -> Vec<u8> {
        rgb.chunks(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect()
    };
    assert_eq!(
        (&image.rgba[..8], &image.rgba[stride..stride + 8]),
        (&rgba(&pixels[..6])[..], &rgba(&pixels[6..])[..]),
        "the remote client must receive the file's pixels"
    );
    assert_eq!(sent_pane_output(&mut server, local), forwarded);

    server.remove_client(remote);
    send(&mut server);
    assert_eq!(sent_pane_output(&mut server, local), frame.as_bytes());
    let _ = std::fs::remove_file(path);
}

#[test]
fn slow_client_is_disconnected_at_exact_backlog_boundary() {
    let mut server = SessionServer::new_named("dev");
    server.max_backlog = 32;
    let (id, _stream) = attach_client(&mut server);
    let client = server.client_mut(id).unwrap();
    client.outbox.clear();
    client.outbox_bytes = 0;

    server.push_to_attached(Arc::new(EncodedOutput {
        bytes: vec![0; 32],
        ..Default::default()
    }));
    assert!(server.client_attached(id));
    assert_eq!(server.client_mut(id).unwrap().outbox_bytes, 32);
    server.push_to_attached(Arc::new(EncodedOutput {
        bytes: vec![1],
        ..Default::default()
    }));
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
        .bytes
        .clone();
    let second_frame = server
        .client_mut(second)
        .unwrap()
        .outbox
        .front()
        .unwrap()
        .bytes
        .clone();
    assert!(Arc::ptr_eq(&first_frame, &second_frame));
}

/// A connection that answers one message and hangs up is not an occupant, and the metric that
/// counts occupants must not count it. Otherwise a detached session asked for its own metrics
/// reports a client - the request asking the question.
#[test]
fn outbox_metrics_count_attached_clients_rather_than_open_sockets() {
    let mut server = SessionServer::new_named("dev");
    let (_attached, _stream) = attach_client(&mut server);
    assert_eq!(server.runtime_metrics().client_outboxes.clients, 1);

    // A second socket that has not attached: a discovery probe, or a headless control request
    // between `accept` and `close_after_flush`.
    let (probe, _probe_stream) = add_client(&mut server);
    assert!(
        !server.client_attached(probe),
        "the probe must not be attached for this test to mean anything"
    );
    assert_eq!(
        server.runtime_metrics().client_outboxes.clients,
        1,
        "an unattached socket is a connection, not a client"
    );
}

#[test]
fn aggregate_outbox_high_water_survives_flush_and_disconnect() {
    let mut server = SessionServer::new_named("dev");
    let (first, mut first_stream) = attach_client(&mut server);
    let (second, mut second_stream) = attach_client(&mut server);

    server.push_to_attached(Arc::new(EncodedOutput {
        bytes: vec![7; 32],
        ..Default::default()
    }));
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
fn pending_pane_output_is_represented_only_by_its_future_snapshot() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(7, test_pane(3));
    let (client_id, _stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(client_id);

    server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_mut()
        .process_bytes(b"before snapshot");
    server.broadcast_outbound(&ServerOutbound::PaneOutput {
        media: Vec::new(),
        metadata: protocol::OutputMetadata::default(),
        pane_id: 7,
        local: false,
        generation: 3,
        bytes: b"before snapshot".to_vec(),
    });

    let client = server.client_mut(client_id).unwrap();
    assert!(
        client.outbox.is_empty(),
        "pre-snapshot output is suppressed"
    );
    assert_eq!(client.seed.as_ref().unwrap().catch_up_bytes, 0);

    assert!(server.pump_attach_seeds());
    let replay: Vec<u8> = decode_outbox_frames(server.client_mut(client_id).unwrap())
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Pane {
                pane_id: 7,
                generation: 3,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect();
    let mut restored = TerminalScreen::new(5, 20, 100);
    restored.process_bytes(&replay);
    assert!(restored.snapshot().to_string().contains("before snapshot"));
}

#[test]
fn attach_replay_restores_retained_kitty_images() {
    let mut pane = test_pane(3);
    pane.screen_mut()
        .process_bytes(b"\x1b[2;4H\x1b_Ga=T,f=24,s=1,v=1,t=d,i=7,c=2,r=2,C=1,z=-9,q=2;gICA\x1b\\");
    assert_eq!(pane.screen_mut().render_snapshot().images.len(), 1);

    let mut server = SessionServer::new_named("dev");
    server.panes.insert(7, pane);
    let (client_id, _stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(client_id);
    for _ in 0..4 {
        server.pump_attach_seeds();
    }

    let replay: Vec<u8> = decode_outbox_frames(server.client_mut(client_id).unwrap())
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Pane {
                pane_id: 7,
                generation: 3,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect();
    let mut restored = TerminalScreen::new(5, 20, 100);
    restored.process_bytes(&replay);
    let snapshot = restored.render_snapshot();
    assert_eq!(snapshot.images.len(), 1);
    assert_eq!(snapshot.images[0].image_id, 7);
    assert_eq!((snapshot.images[0].row, snapshot.images[0].col), (1, 3));
    assert_eq!((snapshot.images[0].rows, snapshot.images[0].cols), (2, 2));
    assert_eq!(snapshot.images[0].z, -9);
}

#[test]
fn output_after_snapshot_waits_behind_the_complete_replay() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let key = PaneSeedKey {
        pane_id: 9,
        generation: 4,
    };
    let snapshot = vec![b'x'; SEED_PUMP_BYTES_PER_TICK + 32 * 1024];
    install_manual_replay(&mut server, client_id, key, snapshot.clone());

    assert!(server.pump_attach_seeds());
    server.broadcast_outbound(&ServerOutbound::PaneOutput {
        media: Vec::new(),
        metadata: protocol::OutputMetadata::default(),
        pane_id: key.pane_id,
        local: false,
        generation: key.generation,
        bytes: b"after".to_vec(),
    });
    assert_eq!(
        server
            .client_mut(client_id)
            .unwrap()
            .seed
            .as_ref()
            .unwrap()
            .catch_up_bytes,
        protocol::PANE_OUTPUT_FRAME_OVERHEAD + b"after".len()
    );
    for _ in 0..4 {
        server.pump_attach_seeds();
    }

    let delivered: Vec<u8> = decode_outbox_frames(server.client_mut(client_id).unwrap())
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Pane {
                pane_id: 9,
                generation: 4,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(&delivered[..snapshot.len()], snapshot);
    assert_eq!(&delivered[snapshot.len()..], b"after");
}

#[test]
fn attach_replay_obeys_per_tick_work_and_resident_window_bounds() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let key = PaneSeedKey {
        pane_id: 1,
        generation: 1,
    };
    let replay_len = SEED_SEND_WINDOW * 2;
    install_manual_replay(&mut server, client_id, key, vec![b'r'; replay_len]);

    server.pump_attach_seeds();
    let first = server.client_mut(client_id).unwrap();
    assert!(first.seed_queued_bytes <= SEED_PUMP_BYTES_PER_TICK);
    assert!(
        first
            .seed
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .remaining
            < replay_len as u64
    );

    for _ in 0..16 {
        server.pump_attach_seeds();
    }
    let stalled = server.client_mut(client_id).unwrap();
    assert!(stalled.seed_queued_bytes <= SEED_SEND_WINDOW);
    assert!(
        stalled.seed.is_some(),
        "window pressure pauses, not rejects"
    );
    assert!(
        stalled
            .seed
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .remaining
            > 0
    );
}

#[test]
fn large_pane_replay_spills_to_file_and_round_trips_exactly() {
    let mut pane = test_pane(1);
    pane.cols = 120;
    pane.rows = 32;
    pane.terminal = TerminalScreen::new(pane.rows, pane.cols, 5_000);
    let mut input = Vec::new();
    for _ in 0..3_000 {
        input.extend(std::iter::repeat_n(b'x', usize::from(pane.cols) - 1));
        input.extend_from_slice(b"\r\n");
    }
    pane.screen_mut().process_bytes(&input);
    let expected = pane.screen_without_change().export_replay_bytes();
    assert!(expected.len() > SEED_MEMORY_SPOOL_LIMIT);

    let mut server = SessionServer::new_named("dev");
    server.panes.insert(1, pane);
    let (client_id, _stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(client_id);
    let key = PaneSeedKey {
        pane_id: 1,
        generation: 1,
    };
    server.begin_pane_replay(client_id, key);
    assert!(matches!(
        &server
            .client_mut(client_id)
            .unwrap()
            .seed
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .spool,
        PaneReplayStorage::File(_)
    ));

    for _ in 0..4 {
        server.pump_attach_seeds();
    }
    let delivered: Vec<u8> = decode_outbox_frames(server.client_mut(client_id).unwrap())
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Pane {
                pane_id: 1,
                generation: 1,
                bytes,
            } => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(delivered, expected);
}

#[test]
fn small_pane_replay_stays_in_memory() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(1, test_pane(1));
    let (client_id, _stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(client_id);
    server.begin_pane_replay(
        client_id,
        PaneSeedKey {
            pane_id: 1,
            generation: 1,
        },
    );

    assert!(matches!(
        &server
            .client_mut(client_id)
            .unwrap()
            .seed
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .spool,
        PaneReplayStorage::Memory(_)
    ));
}

#[test]
fn concurrent_attachers_take_turns_under_the_global_work_budget() {
    let mut server = SessionServer::new_named("dev");
    let (first, _first_stream) = attach_client(&mut server);
    let (second, _second_stream) = attach_client(&mut server);
    let replay = vec![b'r'; SEED_PUMP_BYTES_PER_TICK * 2];
    install_manual_replay(
        &mut server,
        first,
        PaneSeedKey {
            pane_id: 1,
            generation: 1,
        },
        replay.clone(),
    );
    install_manual_replay(
        &mut server,
        second,
        PaneSeedKey {
            pane_id: 2,
            generation: 1,
        },
        replay,
    );

    server.pump_attach_seeds();
    assert!(server.client_mut(first).unwrap().seed_queued_bytes > 0);
    assert_eq!(server.client_mut(second).unwrap().seed_queued_bytes, 0);

    server.pump_attach_seeds();
    assert!(server.client_mut(second).unwrap().seed_queued_bytes > 0);
}

#[test]
fn pending_resize_deltas_and_snapshot_geometry_precede_replay() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(5, test_pane(2));
    let (controller, _controller_stream) = attach_client(&mut server);
    let (seeding, _seeding_stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(seeding);

    for (cols, rows) in [(37, 11), (41, 12)] {
        let responses = server.handle_message(controller, resize_message(5, false, 2, cols, rows));
        for (target, message) in responses {
            server.enqueue(controller, target, message);
        }
    }
    server.pump_attach_seeds();

    let frames = decode_outbox_frames(server.client_mut(seeding).unwrap());
    let replay_index = frames
        .iter()
        .position(|frame| matches!(frame, DecodedOutboxFrame::Pane { pane_id: 5, .. }))
        .expect("pane replay");
    let resizes: Vec<_> = frames
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| {
            let DecodedOutboxFrame::Control(message) = frame else {
                return None;
            };
            let ServerMessage::Resized {
                pane_id: 5,
                generation: 2,
                cols,
                rows,
                ..
            } = message.as_ref()
            else {
                return None;
            };
            Some((index, *cols, *rows))
        })
        .collect();
    assert_eq!(
        resizes
            .iter()
            .map(|(_, cols, rows)| (*cols, *rows))
            .collect::<Vec<_>>(),
        [(37, 11), (41, 12)],
        "the two live resizes"
    );
    assert!(resizes.iter().all(|(index, _, _)| *index < replay_index));
    let reset_index = frames
        .iter()
        .position(|frame| {
            matches!(
                frame,
                DecodedOutboxFrame::Control(message)
                    if matches!(message.as_ref(), ServerMessage::PaneReset {
                        pane_id: 5,
                        generation: 2,
                        cols: 41,
                        rows: 12,
                        ..
                    })
            )
        })
        .expect("reset at snapshot-point geometry");
    assert!(resizes.iter().all(|(index, _, _)| *index < reset_index));
    assert!(reset_index < replay_index);
}

#[test]
fn resize_during_replay_waits_behind_the_last_snapshot_chunk() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    let key = PaneSeedKey {
        pane_id: 6,
        generation: 9,
    };
    install_manual_replay(
        &mut server,
        client_id,
        key,
        vec![b'r'; SEED_PUMP_BYTES_PER_TICK + 1024],
    );

    server.pump_attach_seeds();
    server.broadcast_control(&ServerMessage::Resized {
        pane_id: key.pane_id,
        local: false,
        generation: key.generation,
        cols: 55,
        rows: 14,
        cell_width: 0,
        cell_height: 0,
    });
    for _ in 0..4 {
        server.pump_attach_seeds();
    }

    let frames = decode_outbox_frames(server.client_mut(client_id).unwrap());
    let last_replay = frames
        .iter()
        .rposition(|frame| matches!(frame, DecodedOutboxFrame::Pane { pane_id: 6, .. }))
        .expect("pane replay");
    let resize = frames
        .iter()
        .position(|frame| {
            matches!(
                frame,
                DecodedOutboxFrame::Control(message)
                    if matches!(message.as_ref(), ServerMessage::Resized {
                        pane_id: 6,
                        generation: 9,
                        cols: 55,
                        rows: 14,
                        ..
                    })
            )
        })
        .expect("resize catch-up");
    assert!(resize > last_replay);
}

#[test]
fn pane_closed_while_pending_is_not_exported_and_its_delta_survives() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(2, test_pane(8));
    let (client_id, _stream) = attach_client(&mut server);
    server.enqueue_attach_seeds(client_id);

    server.panes.get_mut(&2).unwrap().exited = Some(17);
    server.broadcast_control(&ServerMessage::Exited {
        pane_id: 2,
        local: false,
        generation: 8,
        code: 17,
    });
    server.pump_attach_seeds();

    let frames = decode_outbox_frames(server.client_mut(client_id).unwrap());
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, DecodedOutboxFrame::Pane { pane_id: 2, .. }))
    );
    assert!(frames.iter().any(|frame| {
        matches!(
            frame,
            DecodedOutboxFrame::Control(message)
                if matches!(message.as_ref(), ServerMessage::Exited {
                    pane_id: 2,
                    generation: 8,
                    code: 17,
                    ..
                })
        )
    }));
}

#[test]
fn catch_up_overflow_sheds_the_flooding_pane_and_requeues_its_replay() {
    let mut server = SessionServer::new_named("dev");
    server.max_backlog = 20 * 1024 * 1024;
    let (live_client, _live_stream) = attach_client(&mut server);
    let (client_id, _stream) = attach_client(&mut server);
    let key = PaneSeedKey {
        pane_id: 3,
        generation: 5,
    };
    install_manual_replay(&mut server, client_id, key, vec![b's'; 64]);
    let chunk = vec![b'o'; 5 * 1024 * 1024];

    for _ in 0..2 {
        server.broadcast_outbound(&ServerOutbound::PaneOutput {
            media: Vec::new(),
            metadata: protocol::OutputMetadata::default(),
            pane_id: key.pane_id,
            local: false,
            generation: key.generation,
            bytes: chunk.clone(),
        });
    }

    assert!(server.client_attached(client_id));
    assert!(server.client_attached(live_client));
    let client = server.client_mut(client_id).unwrap();
    let seed = client.seed.as_ref().unwrap();
    assert!(seed.replay.is_none(), "the interrupted replay is abandoned");
    assert_eq!(seed.manifest.get(&key), Some(&PaneSeedState::Pending));
    assert_eq!(seed.pending.iter().copied().collect::<Vec<_>>(), [key]);
    assert_eq!(seed.catch_up_bytes, 0);
    assert!(client.shed_bytes > 2 * chunk.len() as u64);
    assert_eq!(server.runtime_metrics().attach_seed.disconnected, 0);
    assert_eq!(server.runtime_metrics().client_resync.requeued_panes, 1);
}

#[test]
fn catch_up_overflow_with_no_pane_output_to_shed_disconnects_with_a_reason() {
    let mut server = SessionServer::new_named("dev");
    server.max_backlog = 20 * 1024 * 1024;
    let (live_client, _live_stream) = attach_client(&mut server);
    let (client_id, _stream) = attach_client(&mut server);
    install_manual_replay(
        &mut server,
        client_id,
        PaneSeedKey {
            pane_id: 3,
            generation: 5,
        },
        vec![b's'],
    );

    for _ in 0..2 {
        server.push_to_attached(Arc::new(EncodedOutput {
            bytes: vec![b'c'; 5 * 1024 * 1024],
            ..Default::default()
        }));
    }

    assert!(!server.client_attached(client_id));
    assert!(server.client_attached(live_client));
    let metrics = server.runtime_metrics().attach_seed;
    assert_eq!(metrics.disconnected, 1);
    assert_eq!(
        metrics.last_disconnect_reason.as_deref(),
        Some("attach-catch-up-overflow")
    );
}

fn broadcast_pane_output(
    server: &mut SessionServer,
    pane_id: PaneId,
    generation: u64,
    bytes: &[u8],
) {
    server.broadcast_outbound(&ServerOutbound::PaneOutput {
        media: Vec::new(),
        metadata: protocol::OutputMetadata::default(),
        pane_id,
        local: false,
        generation,
        bytes: bytes.to_vec(),
    });
}

fn outbox_pane_bytes(client: &ClientConn, pane: PaneId) -> Vec<u8> {
    decode_outbox_frames(client)
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Pane { pane_id, bytes, .. } if pane_id == pane => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn a_client_behind_on_one_pane_sheds_it_and_is_resynced_from_the_server_screen() {
    let mut server = SessionServer::new_named("dev");
    server.max_backlog = 16 * 1024;
    server.panes.insert(7, test_pane(3));
    server.panes.insert(8, test_pane(1));
    let (client_id, _stream) = attach_client(&mut server);
    let client = server.client_mut(client_id).unwrap();
    client.outbox.clear();
    client.outbox_bytes = 0;

    broadcast_pane_output(&mut server, 8, 1, b"quiet");
    let flood = vec![b'f'; 4 * 1024];
    for _ in 0..8 {
        broadcast_pane_output(&mut server, 7, 3, &flood);
    }
    server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_mut()
        .process_bytes(b"latest");

    assert!(server.client_attached(client_id));
    let client = server.client_mut(client_id).unwrap();
    let key = PaneSeedKey {
        pane_id: 7,
        generation: 3,
    };
    assert_eq!(
        client.resync_pending.iter().copied().collect::<Vec<_>>(),
        [key]
    );
    assert_eq!(
        outbox_pane_bytes(client, 8),
        b"quiet",
        "the quiet pane keeps its output"
    );
    let queued_flood = outbox_pane_bytes(client, 7).len();
    assert!(queued_flood < 8 * flood.len());

    broadcast_pane_output(&mut server, 7, 3, b"dropped");
    let client = server.client_mut(client_id).unwrap();
    assert_eq!(
        outbox_pane_bytes(client, 7).len(),
        queued_flood,
        "shed output stays dropped"
    );

    // The client drains what it already has, which lets the resync start.
    server.max_backlog = SEED_SEND_WINDOW * 2;
    let client = server.client_mut(client_id).unwrap();
    client.outbox.clear();
    client.outbox_bytes = 0;
    for _ in 0..4 {
        server.pump_attach_seeds();
    }

    let client = server.client_mut(client_id).unwrap();
    assert!(client.resync_pending.is_empty());
    let frames = decode_outbox_frames(client);
    let reset = frames
        .iter()
        .position(|frame| {
            matches!(
                frame,
                DecodedOutboxFrame::Control(message)
                    if matches!(message.as_ref(), ServerMessage::PaneReset {
                        pane_id: 7,
                        generation: 3,
                        ..
                    })
            )
        })
        .expect("resync resets the pane before replaying it");
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, DecodedOutboxFrame::Pane { pane_id: 8, .. })),
        "only the shed pane is replayed"
    );
    assert!(
        frames[..reset]
            .iter()
            .all(|frame| !matches!(frame, DecodedOutboxFrame::Pane { pane_id: 7, .. }))
    );
    let mut restored = TerminalScreen::new(5, 20, 100);
    restored.process_bytes(&outbox_pane_bytes(client, 7));
    assert!(restored.snapshot().to_string().contains("latest"));

    let metrics = server.runtime_metrics().client_resync;
    assert_eq!(
        (metrics.started, metrics.exports, metrics.requeued_panes),
        (1, 1, 0)
    );
    assert!(metrics.shed_bytes > 0);
}

#[test]
fn resync_waits_for_the_outbox_to_drain() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(7, test_pane(3));
    let (client_id, _stream) = attach_client(&mut server);
    let client = server.client_mut(client_id).unwrap();
    client.resync_pending.insert(PaneSeedKey {
        pane_id: 7,
        generation: 3,
    });
    client.outbox_bytes = RESYNC_START_BACKLOG + 1;

    server.pump_attach_seeds();
    assert!(server.client_mut(client_id).unwrap().seed.is_none());

    server.client_mut(client_id).unwrap().outbox_bytes = 0;
    server.pump_attach_seeds();
    let seed = server.client_mut(client_id).unwrap().seed.as_ref().unwrap();
    assert_eq!(seed.kind, SeedKind::Resync);
}

#[test]
fn a_pane_spawned_behind_a_replay_barrier_is_never_shed() {
    let mut server = SessionServer::new_named("dev");
    let (client_id, _stream) = attach_client(&mut server);
    install_manual_replay(
        &mut server,
        client_id,
        PaneSeedKey {
            pane_id: 3,
            generation: 5,
        },
        vec![b's'],
    );

    for _ in 0..2 {
        broadcast_pane_output(&mut server, 11, 1, &vec![b'n'; 5 * 1024 * 1024]);
    }

    // The client learns about pane 11 from control frames still behind the barrier, so a replay of
    // it would arrive first. With nothing it may shed, the client is disconnected instead.
    assert!(!server.client_attached(client_id));
}

#[test]
fn a_resyncing_client_is_still_pinged_past_the_barrier() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(7, test_pane(3));
    let (client_id, _stream) = attach_client(&mut server);
    let key = PaneSeedKey {
        pane_id: 7,
        generation: 3,
    };
    let client = server.client_mut(client_id).unwrap();
    client.outbox.clear();
    client.outbox_bytes = 0;
    client.seed = Some(AttachSeedState::resync(&[key], vec![key]));
    client.last_ping = Instant::now() - HEARTBEAT_INTERVAL;

    server.heartbeat();

    let client = server.client_mut(client_id).unwrap();
    assert!(
        decode_outbox_frames(client).iter().any(|frame| matches!(
            frame,
            DecodedOutboxFrame::Control(message) if matches!(message.as_ref(), ServerMessage::Ping { .. })
        ))
    );
    assert!(client.seed.as_ref().unwrap().catch_up.is_empty());
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
            capabilities: None,
            session: "dev".into(),
            // Entirely older than this build, whatever the current version is.
            protocol_version: PROTOCOL_VERSION - 1,
            min_protocol_version: 1,
            label: "legacy".into(),
            read_only: true,
            shares_filesystem: true,
            expected_server_nonce: None,
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

/// Protocol range answers "can we talk at all"; capabilities answer "which optional work may this
/// connection ask for". They are separate questions, and the second must degrade one feature at a
/// time rather than turning a perfectly usable peer away.
#[test]
fn attach_negotiates_capabilities_and_serves_only_the_agreed_ones() {
    let mut server = SessionServer::new_named("dev");
    let attach = |capabilities| ClientMessage::Attach {
        capabilities,
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_VERSION,
        label: "test".into(),
        read_only: false,
        shares_filesystem: true,
        expected_server_nonce: None,
    };
    let agreed = |responses: &[(Target, ServerMessage)]| match responses.first() {
        Some((_, ServerMessage::Attached { capabilities, .. })) => capabilities.clone(),
        other => panic!("expected an Attached reply, got {other:?}"),
    };

    // A peer that says nothing is a pre-capability build. Protocol 5 already carried runtime
    // metrics, so a missing field has to keep meaning "metrics, and nothing added since".
    let (legacy, _stream) = add_client(&mut server);
    let responses = server.handle_message(legacy, attach(None));
    assert_eq!(agreed(&responses), Some(protocol::Capabilities::legacy()));
    assert!(matches!(
        server
            .handle_message(legacy, ClientMessage::RequestRuntimeMetrics)
            .as_slice(),
        [(Target::Client(id), ServerMessage::RuntimeMetrics { .. })] if *id == legacy
    ));

    // An explicit empty set is a client declining the optional work, which the server must then
    // not do on its behalf — including the metrics a silent peer would have got.
    let (quiet, _stream) = add_client(&mut server);
    let responses = server.handle_message(quiet, attach(Some(protocol::Capabilities::default())));
    assert_eq!(agreed(&responses), Some(protocol::Capabilities::default()));
    assert!(
        server
            .handle_message(quiet, ClientMessage::RequestRuntimeMetrics)
            .is_empty()
    );

    // A name this build has never heard of is ignored rather than refused: that is the whole point
    // of negotiating an open set instead of pinning both ends to one version.
    let (future, _stream) = add_client(&mut server);
    let offered: protocol::Capabilities =
        serde_json::from_str(r#"["runtime-metrics","teleportation"]"#).expect("capability set");
    let responses = server.handle_message(future, attach(Some(offered)));
    assert_eq!(agreed(&responses), Some(protocol::Capabilities::legacy()));
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
            symlink_target: None,
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
            symlink_target: None,
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

/// The catalog a `[[agents]]` block resolves to, as the controller's config reload would build it.
fn catalog_from(toml_text: &str) -> Arc<crate::agent_detection::AgentCatalog> {
    let mut warnings = Vec::new();
    let specs: Vec<crate::agent_detection::AgentSpec> = toml::from_str::<toml::Table>(toml_text)
        .expect("agents parse")
        .remove("agents")
        .expect("agents array")
        .try_into()
        .expect("specs parse");
    let definitions = crate::agent_detection::build_definitions(
        specs,
        crate::agent_detection::AgentOrigin::Config,
        &[],
        &mut warnings,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    Arc::new(crate::agent_detection::AgentCatalog::with_definitions(
        definitions,
    ))
}

/// A pane whose foreground program is declared by `ROZI_AGENT`, showing `screen`.
fn agent_hinted_pane(generation: u64, hint: &str, screen: &str) -> ServerPane {
    let mut pane = test_pane(generation);
    pane.env.push(("ROZI_AGENT".to_string(), hint.to_string()));
    pane.screen_mut().process_bytes(screen.as_bytes());
    pane
}

fn detected_agents(client: &ClientConn) -> Vec<Option<protocol::DetectedAgent>> {
    decode_outbox_controls(client)
        .into_iter()
        .filter_map(|message| match message {
            ServerMessage::PaneRuntimeChanged { state, .. } => Some(state.detected_agent),
            _ => None,
        })
        .collect()
}

fn clear_outboxes(server: &mut SessionServer, ids: &[ClientId]) {
    for id in ids {
        let client = server.client_mut(*id).unwrap();
        client.outbox.clear();
        client.outbox_bytes = 0;
    }
}

/// Detection is server-owned, so a definition change reaches every attached client as one
/// broadcast answer rather than each client reading its own config. Two clients are attached here
/// precisely so "they converge" is an assertion rather than an assumption.
#[test]
fn reloading_agent_definitions_re_detects_and_every_client_converges() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    assert_eq!(server.controller, Some(controller));
    server
        .panes
        .insert(1, agent_hinted_pane(1, "mca", "  thinking…"));

    // Nothing ships a definition for `mca`, so the pane is not an agent yet.
    let mut scan = crate::platform::process::LazyProcessScan::default();
    server.sync_pane_runtime_inner(None, 1, 1, true, &mut scan);
    assert_eq!(server.panes[&1].runtime.detected_agent, None);
    clear_outboxes(&mut server, &[controller, follower]);

    server.apply_agent_definitions(catalog_from(
        r#"
        [[agents]]
        id = "mycoolagent"
        label = "My Cool Agent"
        match = { names = ["mca"] }

        [[agents.states]]
        state = "working"
        scope = "footer"
        screen = { any_of = ["thinking…"] }
        "#,
    ));

    let expected = protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("mycoolagent", "My Cool Agent").into(),
        state: protocol::DetectedAgentState::Working,
    };
    assert_eq!(
        server.panes[&1].runtime.detected_agent.as_ref(),
        Some(&expected),
        "the server re-detected the pane against the new definitions"
    );
    assert_eq!(
        detected_agents(server.client_mut(controller).unwrap()),
        vec![Some(expected.clone())]
    );
    assert_eq!(
        detected_agents(server.client_mut(follower).unwrap()),
        vec![Some(expected.clone())],
        "the follower is told the same thing, not left to read its own config"
    );

    // Re-applying the same catalog is the ordinary reload - a theme edit, a keybinding - and must
    // not churn a broadcast or restart the pane's run clock.
    clear_outboxes(&mut server, &[controller, follower]);
    let sequence = server.panes[&1].runtime.sequence;
    server.apply_agent_definitions(catalog_from(
        r#"
        [[agents]]
        id = "mycoolagent"
        label = "My Cool Agent"
        match = { names = ["mca"] }

        [[agents.states]]
        state = "working"
        scope = "footer"
        screen = { any_of = ["thinking…"] }
        "#,
    ));
    assert_eq!(server.panes[&1].runtime.sequence, sequence);
    assert!(detected_agents(server.client_mut(controller).unwrap()).is_empty());
    assert!(detected_agents(server.client_mut(follower).unwrap()).is_empty());

    // Renaming the agent re-detects again, and both clients move together.
    clear_outboxes(&mut server, &[controller, follower]);
    server.apply_agent_definitions(catalog_from(
        r#"
        [[agents]]
        id = "mycoolagent"
        label = "Renamed Agent"
        match = { names = ["mca"] }

        [[agents.states]]
        state = "working"
        scope = "footer"
        screen = { any_of = ["thinking…"] }
        "#,
    ));
    let renamed = detected_agents(server.client_mut(controller).unwrap());
    assert_eq!(
        renamed,
        detected_agents(server.client_mut(follower).unwrap())
    );
    assert_eq!(
        renamed
            .first()
            .and_then(|agent| agent.as_ref())
            .map(|agent| agent.agent.label.as_str()),
        Some("Renamed Agent")
    );

    // Dropping the definition drops the pane's agent, and that clearing is broadcast too rather
    // than leaving clients showing an identity the server no longer holds.
    clear_outboxes(&mut server, &[controller, follower]);
    server.apply_agent_definitions(crate::agent_detection::AgentCatalog::shared_builtin());
    assert_eq!(server.panes[&1].runtime.detected_agent, None);
    assert_eq!(
        detected_agents(server.client_mut(controller).unwrap()),
        vec![None]
    );
    assert_eq!(
        detected_agents(server.client_mut(follower).unwrap()),
        vec![None]
    );
}

/// Only the controller may change what the session detects with. A follower reloading its own
/// config must not redefine every other client's view of every pane.
#[test]
fn only_the_controller_can_reload_agent_definitions() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let (viewer, _viewer_stream) = attach_read_only_client(&mut server);
    assert_eq!(server.controller, Some(controller));

    let declared = catalog_from(
        r#"
        [[agents]]
        id = "mycoolagent"
        match = { names = ["mca"] }
        "#,
    );
    server.apply_agent_definitions(Arc::clone(&declared));

    // A follower's reload is dropped. The session keeps the controller's definitions; the server's
    // own config (which declares no agents) does not get to replace them.
    for id in [follower, viewer] {
        let responses = server.handle_message(id, ClientMessage::ReloadAgents);
        assert!(responses.is_empty(), "reload is silent, not an error reply");
        assert!(
            Arc::ptr_eq(&server.settings.agents, &declared),
            "client {id} is not the controller and must not swap the catalog"
        );
    }

    // The controller's reload is honored: it re-reads config, which declares no agents, so the
    // session falls back to the built-in catalog.
    server.handle_message(controller, ClientMessage::ReloadAgents);
    assert!(
        !Arc::ptr_eq(&server.settings.agents, &declared),
        "the controller's reload replaced the catalog"
    );
    assert!(
        server.settings.agents.by_name("mca").is_none(),
        "the removed definition is gone"
    );
    assert!(
        server.settings.agents.by_name("claude").is_some(),
        "the built-ins are back"
    );
}

pub(super) fn decode_outbox_controls(client: &ClientConn) -> Vec<ServerMessage> {
    client
        .outbox
        .iter()
        .filter_map(|frame| {
            let mut decoder = protocol::FrameDecoder::default();
            let mut input = &frame.bytes[..];
            while !input.is_empty() {
                decoder.read_from_status(&mut input).ok()?;
            }
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
fn resurrected_fullscreen_state_becomes_shell_safe_scrollback() {
    let mut source = TerminalScreen::new(6, 30, 100);
    source.process_bytes(b"shell before app\r\n");
    source.process_bytes(
        b"\x1b[?1049h\x1b[H\x1b[1;38;2;12;34;56;48;5;22mdead app\x1b[0m\r\nwaiting for input\x1b[3;9H\
          \x1b[?25l\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[?1h",
    );
    let replay = source.export_replay_bytes();

    let mut restored = TerminalScreen::new(6, 30, 100);
    super::panes::recover_restored_screen(&mut restored, &replay);
    let snapshot = restored.render_snapshot();

    assert!(
        snapshot.text.contains("dead app"),
        "dead app output should remain readable:\n{}",
        snapshot.text
    );
    assert_eq!(snapshot.cursor_row, 5);
    assert_eq!(snapshot.cursor_col, 0);
    assert!(snapshot.cursor_visible);
    assert_eq!(snapshot.mouse_mode, tui_lipan::MouseModeState::default());
    assert_eq!(snapshot.key_modes, tui_lipan::TerminalKeyModes::default());

    let frame = restored.capture_frame();
    let app_cell = frame
        .cells
        .iter()
        .find(|cell| cell.symbol == "d")
        .expect("restored app text");
    assert_eq!(app_cell.fg, tui_lipan::Color::Rgb(12, 34, 56));
    assert_eq!(app_cell.bg, tui_lipan::Color::Indexed(22));
    assert!(app_cell.modifiers.bold);

    restored.process_bytes(b"$ ");
    let prompt = restored.render_snapshot();
    assert_eq!(prompt.cursor_row, 5);
    assert_eq!(prompt.cursor_col, 2);
    assert!(prompt.text.contains("dead app"));
    let frame = restored.capture_frame();
    let prompt_cell = &frame.row(5)[0];
    assert_eq!(prompt_cell.fg, tui_lipan::Color::Reset);
    assert_eq!(prompt_cell.bg, tui_lipan::Color::Reset);
    assert!(!prompt_cell.modifiers.bold);

    // Recovery already left the alternate screen. A redundant reset must not make the retained
    // transcript disappear.
    restored.process_bytes(b"\x1b[?1049l");
    assert!(restored.render_snapshot().text.contains("dead app"));
}

#[test]
fn resurrected_alternate_screen_keeps_style_only_changes() {
    let mut source = TerminalScreen::new(4, 12, 100);
    source.process_bytes(b"same text");
    source.process_bytes(b"\x1b[?1049h\x1b[H\x1b[1;31msame text\x1b[0m");
    let replay = source.export_replay_bytes();

    let mut restored = TerminalScreen::new(4, 12, 100);
    super::panes::recover_restored_screen(&mut restored, &replay);
    let frame = restored.capture_frame();
    let retained = frame
        .cells
        .iter()
        .rfind(|cell| cell.symbol == "s")
        .expect("retained alternate screen text");
    assert_eq!(retained.fg, tui_lipan::Color::Red);
    assert!(retained.modifiers.bold);
}

#[test]
fn resurrected_scrollback_keeps_styles_after_new_output() {
    for alternate in [false, true] {
        let mut source = TerminalScreen::new(4, 12, 100);
        if alternate {
            source.process_bytes(b"\x1b[?1049h");
        }
        // Include a wide glyph and a row that fills the terminal width.
        source.process_bytes(b"\x1b[3;32m123456789012\x1b[0m\r\n");
        source.process_bytes("\x1b[38;5;200m界 colored\x1b[0m".as_bytes());
        let replay = source.export_replay_bytes();
        let mut restored = TerminalScreen::new(4, 12, 100);
        super::panes::recover_restored_screen(&mut restored, &replay);
        restored.process_bytes(b"new\r\nnew\r\nnew\r\nnew\r\n");

        let mut found_number = false;
        let mut found_wide = false;
        for offset in 0..=restored.total_scrollback_rows() {
            restored.set_scrollback(offset);
            for cell in &restored.capture_frame().cells {
                if cell.symbol == "1" {
                    assert_eq!(cell.fg, tui_lipan::Color::Green);
                    assert!(cell.modifiers.italic);
                    found_number = true;
                }
                if cell.symbol == "界" {
                    assert_eq!(cell.fg, tui_lipan::Color::Indexed(200));
                    found_wide = true;
                }
            }
        }
        assert!(found_number && found_wide, "alternate screen: {alternate}");
    }
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
            capabilities: None,
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION + 1,
            min_protocol_version: PROTOCOL_VERSION + 1,
            label: "client".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
        },
    );
    assert!(
        matches!(responses.as_slice(), [(_, ServerMessage::Error { code, .. })] if code == "protocol-mismatch")
    );
}

#[test]
fn attach_requires_the_just_started_servers_nonce() {
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            startup_nonce: Some("proxy-a".to_string()),
            ..ServerSettings::default()
        },
    );
    let (raced_client, _stream) = add_client(&mut server);
    let raced = server.handle_message(
        raced_client,
        ClientMessage::Attach {
            capabilities: None,
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "client".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: Some("proxy-b".to_string()),
        },
    );
    assert!(matches!(
        raced.as_slice(),
        [(_, ServerMessage::Error { code, .. })] if code == "server-identity-mismatch"
    ));
    assert!(!server.client_attached(raced_client));

    let (creator, _stream) = add_client(&mut server);
    let matching = server.handle_message(
        creator,
        ClientMessage::Attach {
            capabilities: None,
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "client".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: Some("proxy-a".to_string()),
        },
    );
    assert!(
        matching
            .iter()
            .any(|(_, message)| matches!(message, ServerMessage::Attached { .. }))
    );
    assert!(server.client_attached(creator));
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
            origin: SessionOrigin {
                profile: Some("too-early".into()),
                ..Default::default()
            },
        },
    );
    assert_eq!(server.origin.profile, None);
    server.handle_message(
        first,
        ClientMessage::SpawnPane {
            placement: Some(protocol::SpawnPlacementCommit {
                base_rev: 0,
                layout: shared_layout_for_panes(&[(1, 1)]),
            }),
            pane_id: 1,
            local: false,
            generation: 1,
            launch: None,
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
            origin: SessionOrigin {
                profile: Some("work".into()),
                ..Default::default()
            },
        },
    );
    assert_eq!(server.origin.profile.as_deref(), Some("work"));
    assert!(server.snapshot_dirty());

    let (second, _stream) = attach_client(&mut server);
    server.handle_message(
        second,
        ClientMessage::SetSessionOrigin {
            origin: SessionOrigin {
                profile: Some("other".into()),
                ..Default::default()
            },
        },
    );
    assert_eq!(server.origin.profile.as_deref(), Some("work"));

    let query = server.handle_query("dev".into(), PROTOCOL_VERSION, PROTOCOL_VERSION, None);
    assert!(matches!(
        query.as_slice(),
        [(
            Target::Sender,
            ServerMessage::SessionInfo {
                origin: SessionOrigin { profile: Some(profile), .. },
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
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    });
    let (client, _stream) = attach_client(&mut server);
    server.handle_message(
        client,
        ClientMessage::SetSessionOrigin {
            origin: SessionOrigin {
                profile: Some("too-late".into()),
                ..Default::default()
            },
        },
    );
    assert_eq!(server.origin.profile, None);
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

/// A query is the host monitor's only window into a session it never attaches to, and agent
/// summaries are the payload it exists for. They are capability-gated rather than unconditional:
/// building them walks every pane, and the picker probes that share this message want none of it.
#[test]
fn query_carries_agent_summaries_only_for_a_client_that_asked_for_them() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = test_pane(2);
    pane.runtime.status = Some(protocol::PaneStatus {
        value: "blocked".into(),
        reason: Some("waiting".into()),
        set_at: 0,
    });
    // A bare status is an activity, not an agent: no summary for it.
    pane.agent.sync_references(&pane.runtime);
    server.panes.insert(1, pane);
    assert!(server.agent_summaries().is_empty());
    let pane = server.panes.get_mut(&1).unwrap();
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
        state: protocol::DetectedAgentState::Idle,
    });
    pane.agent.sync_references(&pane.runtime);

    let summaries = |responses: &[(Target, ServerMessage)]| match responses {
        [(Target::Sender, ServerMessage::SessionInfo { agents, .. })] => agents.clone(),
        other => panic!("expected one SessionInfo, got {other:?}"),
    };

    let asked = server.handle_query(
        "dev".into(),
        PROTOCOL_VERSION,
        PROTOCOL_VERSION,
        Some(protocol::Capabilities::current()),
    );
    let agents = summaries(&asked);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].session, "dev");
    assert_eq!(agents[0].pane(), 1);
    assert_eq!(agents[0].state, protocol::AgentState::Blocked);

    // An explicit empty set is a client asking for no optional work, and a missing field is a
    // pre-capability peer that cannot deserialize the summaries anyway. Neither gets them.
    for capabilities in [Some(protocol::Capabilities::default()), None] {
        let responses = server.handle_query(
            "dev".into(),
            PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            capabilities,
        );
        assert!(summaries(&responses).is_empty());
    }
}

/// A pane running several agents contributes one summary each, named the way the Agents tab names
/// them. Collapsing them to one pane-shaped row would leave an alert unable to say *which* of a
/// program's runs is waiting, which is the only thing the alert is for.
#[test]
fn a_pane_publishing_rows_contributes_one_named_summary_per_row() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = test_pane(2);
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("codex", "Codex").into(),
        state: protocol::DetectedAgentState::Working,
    });
    pane.runtime.rows = vec![
        protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: "left".into(),
            title: "running tests".into(),
            status: "working".into(),
            reason: None,
            active: true,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        },
        protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: "right".into(),
            title: "waiting on approval".into(),
            status: "blocked".into(),
            reason: None,
            active: false,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        },
    ];
    pane.agent.sync_references(&pane.runtime);
    server.panes.insert(1, pane);

    let summaries = server.agent_summaries();
    assert_eq!(summaries.len(), 2);
    // Publish order, not row-id order.
    assert_eq!(summaries[0].row().map(|row| row.id.as_str()), Some("left"));
    assert_eq!(summaries[0].state, protocol::AgentState::Working);
    assert_eq!(summaries[1].row().map(|row| row.id.as_str()), Some("right"));
    assert_eq!(summaries[1].state, protocol::AgentState::Blocked);
    // Identity and task are different fields: the label stays the program's name, and each row's
    // title is the task that tells the two apart.
    assert!(summaries.iter().all(|summary| summary.label == "Codex"));
    assert_eq!(summaries[0].task.as_deref(), Some("running tests"));
    assert_eq!(summaries[1].task.as_deref(), Some("waiting on approval"));
    assert!(!summaries[0].same_agent(&summaries[1]));
}

/// An exited pane is not an agent that went quiet — it is a pane that is gone. Leaving it in the
/// summary would park a `done` beside a session on a machine nobody is watching, permanently.
#[test]
fn agent_summaries_skip_exited_panes() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = status_test_pane(2, Some(0));
    pane.runtime.status = Some(protocol::PaneStatus {
        value: "working".into(),
        reason: None,
        set_at: 0,
    });
    server.panes.insert(1, pane);

    assert!(server.agent_summaries().is_empty());
}

/// A session reports the checkouts its panes work in, with a published row that works in a
/// worktree of its own listed there rather than under its pane — and only to a client that asked.
#[test]
fn query_carries_project_summaries_by_checkout() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = test_pane(2);
    pane.runtime.project_root = Some("/x/rozi".into());
    pane.runtime.git_branch = Some("master".into());
    pane.runtime.rows = vec![protocol::PublishedRow {
        publisher: "sessions".into(),
        kind: crate::session::protocol::PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: "fix".into(),
        title: "fix".into(),
        status: "working".into(),
        reason: None,
        active: true,
        work_started_at: None,
        cwd: Some("/x/rozi/.worktrees/fix".into()),
        project: Some(protocol::RowProject {
            root: "/x/rozi/.worktrees/fix".into(),
            repository: Some("/x/rozi".into()),
            branch: Some("fix".into()),
        }),
        native_session: None,
    }];
    server.panes.insert(1, pane);
    let mut shell = test_pane(2);
    shell.runtime.project_root = Some("/x/rozi".into());
    shell.runtime.git_branch = Some("master".into());
    server.panes.insert(2, shell);
    // A pane outside any repository is no project's.
    server.panes.insert(3, test_pane(2));

    let projects = server.project_summaries();
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].root, "/x/rozi");
    assert_eq!(projects[0].panes, vec![1, 2]);
    assert!(projects[0].rows.is_empty());
    assert_eq!(projects[1].root, "/x/rozi/.worktrees/fix");
    assert_eq!(projects[1].repository.as_deref(), Some("/x/rozi"));
    assert_eq!(projects[1].rows.len(), 1);
    assert_eq!(projects[1].rows[0].pane, 1);

    let mut reported = |capabilities| match server
        .handle_query(
            "dev".into(),
            PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            capabilities,
        )
        .as_slice()
    {
        [(Target::Sender, ServerMessage::SessionInfo { projects, .. })] => projects.clone(),
        other => panic!("expected one SessionInfo, got {other:?}"),
    };
    assert_eq!(reported(Some(protocol::Capabilities::current())).len(), 2);
    let agents_only = protocol::Capabilities(
        [protocol::AGENT_SUMMARIES.to_string()]
            .into_iter()
            .collect(),
    );
    assert!(reported(Some(agents_only)).is_empty());
    assert!(reported(None).is_empty());
}

#[test]
fn query_registers_nothing_and_seeds_nothing() {
    let mut server = SessionServer::new_named("dev");
    let (id, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        id,
        ClientMessage::Query {
            capabilities: None,
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
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        follower,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
            live: false,
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
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
            live: false,
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

/// The server keeps no gesture state, but it must hand the controller's `live` mark on to every
/// follower: it decides whether they ease toward the revision or draw it as it lands.
#[test]
fn controller_commit_relays_live_mark() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    for (base_rev, live) in [(0, true), (1, false)] {
        let layout = SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            pane_geometry: Default::default(),
            canvas_cols: 80 + base_rev as u16,
            canvas_rows: 24,
            workspaces: Vec::new(),
        };
        let responses = server.handle_message(
            controller,
            ClientMessage::CommitLayout {
                base_rev,
                layout,
                live,
            },
        );
        let [(Target::Broadcast, ServerMessage::LayoutCommitted { live: relayed, .. })] =
            responses.as_slice()
        else {
            panic!("expected broadcast commit, got {responses:?}");
        };
        assert_eq!(*relayed, live);
    }
}

#[test]
fn stale_base_rev_is_rejected_with_authoritative_layout() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: layout.clone(),
            live: false,
        },
    );
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout,
            live: false,
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
        pane_geometry: Default::default(),
        canvas_cols: 0, // invalid 0 canvas
        canvas_rows: 24,
        workspaces: Vec::new(),
    };
    let responses = server.handle_message(
        controller,
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: invalid_layout,
            live: false,
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

fn shared_layout_for_panes(panes: &[(PaneId, u64)]) -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: vec![crate::layout::shared::SharedWorkspace {
            index: 0,
            name: None,
            synchronized: false,
            layout: crate::layout::shared::SharedLayoutKind::Dwindle,
            start_axis: crate::layout::shared::SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: None,
            panes: panes
                .iter()
                .map(|(pane_id, generation)| crate::layout::shared::SharedPane {
                    pane_id: *pane_id,
                    generation: *generation,
                    title: None,
                    profile_name: None,
                    cwd: None,
                    launch: None,
                    replay: false,
                    keep_open: false,
                    floating: false,
                    pinned: false,
                    fullscreen: false,
                    rect: None,
                    scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                })
                .collect(),
        }],
    }
}

#[test]
fn layout_commit_requires_the_exact_live_pane_set_and_generations() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(1, test_pane(7));
    assert!(server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(1, 7)])));
    assert!(!server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(1, 8)])));
    assert!(!server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(2, 7)])));
    assert!(!server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[])));
    assert!(!server.validate_shared_layout_against_panes(&SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 80,
        canvas_rows: 24,
        workspaces: Vec::new(),
    }));

    server.panes.get_mut(&1).unwrap().exited = Some(127);
    assert!(server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(1, 7)])));
    assert!(!server.validate_shared_layout_against_panes(&shared_layout_for_panes(&[(1, 8)])));
}

#[test]
fn request_control_flags_requester_and_notifies_controller_without_stealing() {
    let mut server = cooperative_server("dev");
    let (first, _s1) = attach_client(&mut server);
    let (second, _s2) = attach_client(&mut server);
    let responses = server.handle_message(second, ClientMessage::RequestControl);
    // A present controller is never stolen from; control stays put.
    assert_eq!(server.controller, Some(first));
    // The requester is flagged in the broadcast roster…
    assert!(responses.iter().any(|(target, message)| matches!(
        (target, message),
        (Target::Broadcast, ServerMessage::ClientsChanged { clients, .. })
            if clients.iter().any(|c| c.id == second && c.requesting_control)
    )));
    // …and only the controller is notified.
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
            placement: None,
            pane_id: 1,
            local: false,
            generation: 1,
            launch: None,
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
        [(Target::Sender, ServerMessage::SpawnResult { ok: false, error: Some(error), .. }), (_, ServerMessage::LayoutRejected { .. })]
            if error == "not controller or read-only"
    ));
    assert!(server.panes.is_empty());
}

fn local_spawn(pane_id: PaneId, generation: u64) -> ClientMessage {
    ClientMessage::SpawnPane {
        placement: None,
        pane_id,
        local: true,
        generation,
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
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
            [
                (Target::Sender, ServerMessage::SpawnResult { ok: true, .. }),
                ..
            ]
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
        placement: (!local).then(|| protocol::SpawnPlacementCommit {
            base_rev: 0,
            layout: shared_layout_for_panes(&[(pane_id, generation)]),
        }),
        pane_id,
        local,
        generation,
        launch: Some(crate::pane::launch::PaneLaunch::shell(command)),
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

/// A pane's `ROZI_PANE` means nothing without the session it counts in, and only the server can say
/// which one that is. A local pane shares numbers with the shared panes, so it names none.
#[test]
fn a_shared_pane_is_told_its_session_instance_and_a_local_pane_is_told_none() {
    let mut server = SessionServer::new_named("dev");
    let (owner, _stream) = attach_client(&mut server);
    let forged = || {
        vec![(
            protocol::SESSION_INSTANCE_ENV.to_string(),
            "not-this-server".to_string(),
        )]
    };
    for local in [false, true] {
        let mut message = colliding_spawn(1, local, 1, "true");
        if let ClientMessage::SpawnPane { env, .. } = &mut message {
            *env = forged();
        }
        server.handle_message(owner, message);
    }
    let instance = |env: &[(String, String)]| {
        env.iter()
            .filter(|(key, _)| key == protocol::SESSION_INSTANCE_ENV)
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        instance(&server.panes[&1].env),
        [server.instance_id().as_str().to_string()]
    );
    assert_eq!(
        instance(&server.local_panes[&(owner, 1)].env),
        [String::new()]
    );
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
        .filter_map(|frame| {
            let mut decoder = crate::session::protocol::FrameDecoder::default();
            let mut input = &frame.bytes[..];
            while !input.is_empty() {
                decoder.read_from_status(&mut input).ok()?;
            }
            match decoder.next_frame::<ServerMessage>().ok()? {
                Some(Frame::PaneOutput {
                    pane_id,
                    local,
                    generation,
                    bytes,
                    ..
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
                [
                    (Target::Sender, ServerMessage::SpawnResult { ok: true, .. }),
                    ..
                ]
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
            latency_sensitive: true,
            pane_id: PANE,
            local: false,
            generation: GENERATION,
            bytes: b"FROM-SHARED\n".to_vec(),
        },
    );
    server.process_client_frame(
        owner,
        Frame::PaneBytes {
            latency_sensitive: true,
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

/// A UI waiting on a send's answer takes its baseline at `InputMarked`. The pane's answer to the
/// input must therefore reach the client after the mark, however soon the program replies: the
/// server writes the input and queues the mark in one step, before it drains the PTY again.
#[test]
fn marked_input_is_answered_ahead_of_the_output_it_causes() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    const PANE: PaneId = 7;
    const GENERATION: u64 = 1;
    let spawned = server.handle_message(
        client,
        colliding_spawn(PANE, false, GENERATION, "printf 'READY\\n'; cat"),
    );
    assert!(
        matches!(
            spawned.as_slice(),
            [
                (Target::Sender, ServerMessage::SpawnResult { ok: true, .. }),
                ..
            ]
        ),
        "spawn failed: {spawned:?}"
    );
    wait_until(&mut server, |server| {
        pane_text(server, None, PANE).contains("READY")
    });
    clear_outboxes(&mut server, &[client]);

    server.process_client_frame(
        client,
        Frame::Control(ClientMessage::MarkedInput {
            pane_id: PANE,
            local: false,
            generation: GENERATION,
            bytes: b"PING\n".to_vec(),
            token: 9,
        }),
    );
    wait_until(&mut server, |server| {
        pane_text(server, None, PANE).contains("PING")
    });

    let frames = decode_outbox_frames(server.client_mut(client).unwrap());
    let mark = frames
        .iter()
        .position(|frame| {
            matches!(frame, DecodedOutboxFrame::Control(message)
                if matches!(**message, ServerMessage::InputMarked { token: 9 }))
        })
        .expect("the input is marked");
    let output = |frames: &[DecodedOutboxFrame]| -> Vec<u8> {
        frames
            .iter()
            .filter_map(|frame| match frame {
                DecodedOutboxFrame::Pane { bytes, .. } => Some(bytes.clone()),
                DecodedOutboxFrame::Control(_) => None,
            })
            .flatten()
            .collect()
    };
    let before = String::from_utf8_lossy(&output(&frames[..mark])).into_owned();
    let after = String::from_utf8_lossy(&output(&frames[mark..])).into_owned();
    assert!(
        !before.contains("PING"),
        "answer ahead of the mark: {before:?}"
    );
    assert!(
        after.contains("PING"),
        "answer missing after the mark: {after:?}"
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
        // Production gives the endpoint to `SessionServer`, which retires it synchronously before
        // `run_listener` returns.
        server.endpoint = Some(thread_endpoint);
        server.run_listener(listener)
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

/// Killing the session you are attached to drops the client on the line after the request is sent,
/// which shuts the socket down and stops the writer thread. The request must still reach the
/// server: a lost one leaves a named server running, and the killed session walks back into the
/// picker on the next discovery sweep.
#[test]
fn an_attached_kill_reaches_the_server_before_the_client_is_torn_down() {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let name = format!(
        "kl-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (listener, endpoint) = bind_session_socket(&name).expect("bind session endpoint");
    let thread_endpoint = endpoint.clone();
    let thread_name = name.clone();
    let server_thread = std::thread::spawn(move || {
        let mut server =
            SessionServer::new_named_with_settings(thread_name, ServerSettings::default());
        server.endpoint = Some(thread_endpoint);
        server.run_listener(listener)
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    while !endpoint.is_live() {
        assert!(
            Instant::now() < deadline,
            "server endpoint never became live"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let (inbound, _inbound_rx) = std::sync::mpsc::channel();
    let (client, _attached) = crate::session::client::SessionClient::connect_attached(
        &endpoint,
        name.clone(),
        inbound,
        false,
    )
    .expect("attach to the session server");
    let shutdown_started = Instant::now();
    client.shutdown();
    let shutdown_elapsed = shutdown_started.elapsed();
    let queued_after_shutdown = client.runtime_stats().outbound.queued_items;
    // Exactly what `kill_current_session` does next: the attachment is replaced, so the last handle
    // goes away immediately.
    drop(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    while endpoint.is_live() {
        assert!(
            Instant::now() < deadline,
            "kill was dropped on the floor: the server outlived it (shutdown waited {shutdown_elapsed:?}, {queued_after_shutdown} outbound frames queued)"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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
fn unchanged_resize_preserves_server_content_before_and_after_output() {
    for local in [false, true] {
        for output_seen in [false, true] {
            let mut server = SessionServer::new_named("dev");
            let (controller, _stream) = attach_client(&mut server);
            let mut pane = test_pane(2);
            pane.output_seen = output_seen;
            if output_seen {
                pane.terminal.process_bytes(b"kept");
            }
            let before = pane.terminal.render_snapshot();
            let content_generation = pane.content_generation;
            if local {
                server.local_panes.insert((controller, 1), pane);
            } else {
                server.panes.insert(1, pane);
            }
            let responses = server.handle_message(
                controller,
                ClientMessage::Resize {
                    pane_id: 1,
                    local,
                    generation: 2,
                    cols: 20,
                    rows: 5,
                    cell_width: 0,
                    cell_height: 0,
                },
            );
            assert!(
                responses.is_empty(),
                "unchanged geometry must not broadcast a resize"
            );
            let pane = if local {
                server.local_panes.get_mut(&(controller, 1)).unwrap()
            } else {
                server.panes.get_mut(&1).unwrap()
            };
            assert_eq!(pane.content_generation, content_generation);
            let after = pane.screen_without_change().render_snapshot();
            assert!(Arc::ptr_eq(&before.color_lines, &after.color_lines));
            assert_eq!(after.text, before.text);
        }
    }
}

#[test]
fn failed_pty_resize_retries_without_changing_server_content_or_broadcasting() {
    for local in [false, true] {
        for output_seen in [false, true] {
            for pixel_only in [false, true] {
                let mut server = SessionServer::new_named("dev");
                let (controller, _stream) = attach_client(&mut server);
                let mut pane = test_pane(2);
                pane.output_seen = output_seen;
                if output_seen {
                    pane.terminal.process_bytes(b"kept");
                }
                if local {
                    server.local_panes.insert((controller, 1), pane);
                } else {
                    server.panes.insert(1, pane);
                }
                let (cols, rows) = if pixel_only { (20, 5) } else { (30, 8) };
                let cell = tui_lipan::TerminalCellSize::new(9, 18);
                let request = |legacy| super::connection::PaneResizeRequest {
                    pane_id: 1,
                    local,
                    generation: 2,
                    cols,
                    rows,
                    cell_width: if legacy { 0 } else { 9 },
                    cell_height: if legacy { 0 } else { 18 },
                };
                let mut attempts = 0;
                let responses = server.handle_resize_with(controller, request(false), |pane| {
                    attempts += 1;
                    assert_eq!((pane.cols, pane.rows, pane.cell), (cols, rows, cell));
                    Err(io::Error::other("transient resize failure"))
                });
                assert_eq!(
                    responses.len(),
                    1,
                    "the initial change must be acknowledged"
                );
                assert!(
                    matches!(&responses[0].1, ServerMessage::Resized { cols: c, rows: r, .. } if (*c, *r) == (cols, rows))
                );
                let owner = local.then_some(controller);
                let pane = server.pane_mut(owner, 1).unwrap();
                assert!(pane.pty_resize_pending);
                let before = pane.screen_without_change().render_snapshot();
                let content_generation = pane.content_generation;

                // A failed retry stays pending; a successful retry clears it. A legacy request
                // must retry the canonical pixel size rather than resetting it to zero.
                for (legacy, fail) in [(false, true), (true, false)] {
                    let responses =
                        server.handle_resize_with(controller, request(legacy), |pane| {
                            attempts += 1;
                            assert_eq!((pane.cols, pane.rows, pane.cell), (cols, rows, cell));
                            if fail {
                                Err(io::Error::other("still unavailable"))
                            } else {
                                Ok(())
                            }
                        });
                    assert!(responses.is_empty(), "a PTY-only retry must not broadcast");
                    let pane = server.pane_mut(owner, 1).unwrap();
                    assert_eq!(pane.pty_resize_pending, fail);
                    assert_eq!(pane.content_generation, content_generation);
                    let after = pane.screen_without_change().render_snapshot();
                    assert!(Arc::ptr_eq(&before.color_lines, &after.color_lines));
                    assert_eq!(before.text, after.text);
                }
                assert_eq!(attempts, 3);
                assert!(
                    server
                        .handle_resize_with(controller, request(false), |_| {
                            panic!("a successful PTY resize must restore the duplicate fast path")
                        })
                        .is_empty()
                );
            }
        }
    }
}

#[test]
fn pending_pty_resize_obeys_authorization_generation_and_latest_geometry() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _stream) = attach_client(&mut server);
    let (viewer, _viewer_stream) = attach_read_only_client(&mut server);
    let mut pane = test_pane(2);
    pane.pty_resize_pending = true;
    server.panes.insert(1, pane);
    let request = |generation, cols| super::connection::PaneResizeRequest {
        pane_id: 1,
        local: false,
        generation,
        cols,
        rows: 5,
        cell_width: 0,
        cell_height: 0,
    };
    for (client, generation) in [(viewer, 2), (controller, 1)] {
        assert!(
            server
                .handle_resize_with(client, request(generation, 20), |_| {
                    panic!("unauthorized or stale requests must not retry the PTY")
                })
                .is_empty()
        );
        assert!(server.panes[&1].pty_resize_pending);
    }
    let responses = server.handle_resize_with(controller, request(2, 40), |pane| {
        assert_eq!((pane.cols, pane.rows), (40, 5));
        Ok(())
    });
    assert_eq!(responses.len(), 1);
    assert!(!server.panes[&1].pty_resize_pending);
    assert!(
        server
            .handle_resize_with(controller, request(2, 40), |_| {
                panic!("the new geometry was already applied successfully")
            })
            .is_empty()
    );
}

#[test]
fn pixel_only_resize_updates_cell_size_without_reflowing_the_grid() {
    for output_seen in [false, true] {
        let mut server = SessionServer::new_named("dev");
        let (controller, _stream) = attach_client(&mut server);
        let mut pane = test_pane(2);
        pane.output_seen = output_seen;
        let content_generation = pane.content_generation;
        server.panes.insert(1, pane);
        let resize = |width, height| ClientMessage::Resize {
            pane_id: 1,
            local: false,
            generation: 2,
            cols: 20,
            rows: 5,
            cell_width: width,
            cell_height: height,
        };
        assert!(!server.handle_message(controller, resize(9, 18)).is_empty());
        let pane = &server.panes[&1];
        assert_eq!(pane.cell, tui_lipan::TerminalCellSize::new(9, 18));
        assert_eq!(pane.screen().cell_size(), pane.cell);
        assert_eq!(pane.content_generation, content_generation + 1);
        assert!(server.handle_message(controller, resize(0, 0)).is_empty());
        assert!(server.handle_message(controller, resize(9, 18)).is_empty());
        assert_eq!(server.panes[&1].content_generation, content_generation + 1);
    }
}

#[test]
fn resize_updates_screen_and_broadcasts_ack() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _s1) = attach_client(&mut server);
    server.panes.insert(
        1,
        ServerPane {
            scrollback: 100,
            generation: 2,
            title: None,
            cwd: None,
            launch: None,
            agent_resume: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            pty_resize_pending: false,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            output_sequence: 0,
            output_coalesce_epoch: 0,
            output_times: VecDeque::new(),
            graphics_pacer: Default::default(),
            output_seen: false,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            program_status_sync: Default::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            borrowed_program_name: None,
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
                ..
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

    let responses = server.handle_message(
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
    // Followers lay the pane's images out against the same cells, whatever their own size.
    assert!(matches!(
        responses.as_slice(),
        [(
            Target::Broadcast,
            ServerMessage::Resized {
                cell_width: 9,
                cell_height: 18,
                ..
            }
        )]
    ));
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
fn resize_after_output_reflows_without_replacing_server_content() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _stream) = attach_client(&mut server);
    server.panes.insert(1, test_pane(2));
    server.handle_event(ServerEvent::Pty(
        None,
        1,
        2,
        TerminalPtyEvent::Output(b"content-before-resize".to_vec().into()),
    ));
    assert!(server.panes[&1].output_seen);

    server.handle_message(
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

    let pane = server.panes.get_mut(&1).unwrap();
    assert!(
        pane.screen_without_change()
            .render_snapshot()
            .text
            .contains("content-before-resize")
    );
}

#[test]
fn duplicate_spawn_is_rejected() {
    let mut server = SessionServer::new_named("dev");
    server.panes.insert(
        1,
        ServerPane {
            scrollback: 100,
            generation: 2,
            title: None,
            cwd: None,
            launch: None,
            agent_resume: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            pty_resize_pending: false,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            output_sequence: 0,
            output_coalesce_epoch: 0,
            output_times: VecDeque::new(),
            graphics_pacer: Default::default(),
            output_seen: false,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            program_status_sync: Default::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            borrowed_program_name: None,
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );
    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 3,
        launch: None,
        agent_resume: None,
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
            scrollback: 100,
            generation: 2,
            title: None,
            cwd: None,
            launch: None,
            agent_resume: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            pty_resize_pending: false,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            output_sequence: 0,
            output_coalesce_epoch: 0,
            output_times: VecDeque::new(),
            graphics_pacer: Default::default(),
            output_seen: false,
            cols: 20,
            rows: 5,
            exited: Some(0),
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            program_status_sync: Default::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            borrowed_program_name: None,
            last_git_read: None,
            initial_cursor_report_primed: false,
        },
    );

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 3,
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
        agent_resume: None,
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
fn attach_streams_full_osc_stores_under_frame_and_backlog_limits() {
    use base64::Engine as _;
    const PANES: u32 = 16;
    let mut server = SessionServer::new_named("osc-capacity");
    let mut aggregate_bytes = 0;
    for pane_id in 1..=PANES {
        let mut pane = test_pane(1);
        pane.screen_mut().set_program_status_enabled(true);
        // The last pane also exercises JSON expansion beyond the per-tick byte budget.
        let text = if pane_id == PANES { "\"" } else { "m" };
        let msg = base64::engine::general_purpose::STANDARD.encode(text.repeat(2048));
        let title = base64::engine::general_purpose::STANDARD.encode(text.repeat(192));
        for record in 0..256 {
            let id = format!(
                "{record:032}/{}/{}/{}",
                "b".repeat(32),
                "c".repeat(32),
                "d".repeat(29)
            );
            assert_eq!(id.len(), 128);
            let wire = format!(
                "\x1b]7501;state=working:id={id}:app={}:title={title}:msg={msg}\x07",
                "a".repeat(32)
            );
            assert!(wire.len() <= 4096);
            pane.screen_mut().process_bytes(wire.as_bytes());
        }
        let snapshot = pane.screen().program_status_snapshot();
        assert_eq!(snapshot.records.len(), 256);
        aggregate_bytes += serde_json::to_vec(&snapshot).unwrap().len();
        server.panes.insert(pane_id, pane);
    }
    assert!(aggregate_bytes > protocol::MAX_FRAME_SIZE);
    let (client, mut stream) = add_client(&mut server);
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    server.process_client_frame(
        client,
        Frame::Control(ClientMessage::Attach {
            capabilities: None,
            session: "osc-capacity".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "fresh client".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
        }),
    );
    let controls = decode_outbox_controls(server.client_mut(client).unwrap());
    assert!(
        matches!(&controls[0], ServerMessage::Attached { panes, .. } if panes.len() == PANES as usize)
    );
    assert!(
        !controls
            .iter()
            .any(|message| matches!(message, ServerMessage::ProgramStatusChanged { .. }))
    );
    // A stalled client must pause the stream instead of filling an aggregate control outbox.
    for _ in 0..32 {
        server.pump_attach_seeds();
    }
    assert!(server.client_mut(client).unwrap().outbox_bytes <= SEED_SEND_WINDOW);
    assert!(
        !server
            .client_mut(client)
            .unwrap()
            .seed
            .as_ref()
            .unwrap()
            .pending_program_status
            .is_empty()
    );
    let reader = std::thread::spawn(move || {
        let mut received = std::collections::BTreeSet::new();
        while received.len() < PANES as usize {
            let message: ServerMessage = protocol::read_frame(&mut stream).unwrap();
            if let ServerMessage::ProgramStatusChanged {
                pane_id,
                local: false,
                generation: 1,
                initial: true,
                snapshot,
            } = message
            {
                assert_eq!(snapshot.records.len(), 256);
                assert_eq!(snapshot.revision, 256);
                assert!(snapshot.records.iter().all(|record| record.id.len() == 128
                    && record.msg.as_ref().unwrap().len() == 2048
                    && record.title.as_ref().unwrap().len() == 192));
                assert!(received.insert(pane_id));
            }
        }
        // Stay connected until the server finishes terminal replay too.
        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut stream, &mut rest).ok();
        received
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        server.pump_attach_seeds();
        server.flush_clients();
        let connection = server
            .client_mut(client)
            .expect("attach must remain connected");
        assert!(connection.outbox_bytes <= SEED_SEND_WINDOW);
        if connection.seed.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "attach failed to drain"
        );
        std::thread::yield_now();
    }
    server.remove_client(client);
    assert_eq!(reader.join().unwrap().len(), PANES as usize);
}

#[test]
fn attach_reports_layout_and_panes() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = ServerPane {
        scrollback: 100,
        generation: 8,
        title: Some("editor".into()),
        cwd: Some("/repo".into()),
        launch: None,
        agent_resume: None,
        keep_open: false,
        command_completed: false,
        cell: tui_lipan::TerminalCellSize::default(),
        shell: Vec::new(),
        env: Vec::new(),
        palette: test_palette(),
        pty: None,
        pty_resize_pending: false,
        terminal: TerminalScreen::new(5, 20, 100),
        content_generation: 0,
        output_sequence: 0,
        output_coalesce_epoch: 0,
        output_times: VecDeque::new(),
        graphics_pacer: Default::default(),
        output_seen: false,
        cols: 20,
        rows: 5,
        exited: None,
        log: None,
        runtime: protocol::PaneRuntimeState::default(),
        program_status_sync: Default::default(),
        agent: AgentScratch::default(),
        program_on_path: None,
        borrowed_program_name: None,
        last_git_read: None,
        initial_cursor_report_primed: false,
    };
    pane.screen_mut().process_bytes(b"ready");
    server.panes.insert(4, pane);
    server.layout = Some(SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 20,
        canvas_rows: 5,
        workspaces: Vec::new(),
    });
    server.layout_rev = 7;

    let (id, _stream) = add_client(&mut server);
    let responses = server.handle_message(
        id,
        ClientMessage::Attach {
            capabilities: None,
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "client".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
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
            scrollback: 100,
            generation: 2,
            title: None,
            cwd: None,
            launch: None,
            agent_resume: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: test_palette(),
            pty: None,
            pty_resize_pending: false,
            terminal: TerminalScreen::new(5, 20, 100),
            content_generation: 0,
            output_sequence: 0,
            output_coalesce_epoch: 0,
            output_times: VecDeque::new(),
            graphics_pacer: Default::default(),
            output_seen: false,
            cols: 20,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            program_status_sync: Default::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            borrowed_program_name: None,
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
    assert_eq!(client.outbox[0].bytes[4], 2, "raw pane frame must be first");
    assert_eq!(
        client.outbox[1].bytes[4], 1,
        "runtime control frame must follow"
    );
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
    server.origin.profile = Some("work".into());
    server.origin.worktree = Some(crate::session::origin::WorktreeOrigin {
        path: "C:\\code\\feature".into(),
    });
    for (id, exited) in [(1, None), (2, Some(0)), (crate::state::POPUP_PANE_ID, None)] {
        let mut screen = TerminalScreen::new(5, 20, 100);
        screen.process_bytes(format!("marker-{id}").as_bytes());
        server.panes.insert(
            id,
            ServerPane {
                scrollback: 100,
                generation: id as u64,
                title: Some(format!("pane-{id}")),
                cwd: None,
                launch: Some(if id == 1 {
                    crate::pane::launch::PaneLaunch::Direct {
                        argv: vec!["printf".into(), "literal argument".into()],
                    }
                } else {
                    crate::pane::launch::PaneLaunch::shell("true")
                }),
                agent_resume: None,
                keep_open: false,
                command_completed: false,
                cell: tui_lipan::TerminalCellSize::default(),
                shell: Vec::new(),
                env: Vec::new(),
                palette: test_palette(),
                pty: None,
                pty_resize_pending: false,
                terminal: screen,
                content_generation: 0,
                output_sequence: 0,
                output_coalesce_epoch: 0,
                output_times: VecDeque::new(),
                graphics_pacer: Default::default(),
                output_seen: true,
                cols: 20,
                rows: 5,
                exited,
                log: None,
                runtime: protocol::PaneRuntimeState::default(),
                program_status_sync: Default::default(),
                agent: AgentScratch::default(),
                program_on_path: None,
                borrowed_program_name: None,
                last_git_read: None,
                initial_cursor_report_primed: false,
            },
        );
    }
    server.layout = Some(SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
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
    assert_eq!(
        restored.panes[&1].launch,
        Some(crate::pane::launch::PaneLaunch::Direct {
            argv: vec!["printf".into(), "literal argument".into()]
        })
    );
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
    // A restored pane learns where it runs exactly as a freshly split one does; agent hooks inside
    // it give up without `ROZI` and `ROZI_PANE`.
    let env = &restored.panes[&1].env;
    assert!(
        env.contains(&("ROZI".to_string(), "1".to_string())),
        "{env:?}"
    );
    assert!(
        env.contains(&("ROZI_PANE".to_string(), "1".to_string())),
        "{env:?}"
    );
    assert!(
        env.contains(&(
            protocol::SESSION_INSTANCE_ENV.to_string(),
            restored.instance_id().as_str().to_string()
        )),
        "{env:?}"
    );
    assert_eq!(restored.layout_rev, 1);
    assert_eq!(restored.origin.profile.as_deref(), Some("work"));
    assert_eq!(
        restored
            .origin
            .worktree
            .as_ref()
            .map(|tree| tree.path.as_str()),
        Some("C:\\code\\feature")
    );
    restored.delete_snapshot().unwrap();
    assert!(!root.join("dev").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn structured_argv_reaches_the_child_without_shell_interpretation() {
    let mut server = SessionServer::new_named("dev");
    let literal = "space; $HOME $(printf unsafe) 'quoted'";
    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        launch: Some(crate::pane::launch::PaneLaunch::Direct {
            argv: vec!["printf".into(), "%s".into(), literal.into()],
        }),
        agent_resume: None,
        cwd: None,
        title: None,
        cols: 80,
        rows: 10,
        keep_open: false,
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
            let _ = server.handle_event(event);
        }
        if server
            .panes
            .get(&1)
            .is_some_and(|pane| pane.exited.is_some())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let text = server
        .panes
        .get_mut(&1)
        .expect("direct pane retained")
        .screen_without_change()
        .snapshot();
    assert!(
        text.contains(literal),
        "argv was altered before exec: {text:?}"
    );
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
        launch: Some(crate::pane::launch::PaneLaunch::shell(
            "printf 'hello from the command\\n'; exit 3",
        )),
        agent_resume: None,
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

/// The other way a resume fails, and the more likely one: the snapshot is older than the machine,
/// and the command its agent definition names is not there any more. Nothing can be spawned at
/// all, so there is no exit status to report and no process to hold the pane open - and the pane
/// must still come back as a usable terminal rather than as an exited husk.
#[test]
fn a_resume_command_that_cannot_start_still_leaves_a_working_shell() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        // The pane was created to run the agent. Falling back to *this* would open a fresh
        // conversation, which is the one thing a failed resume must not do.
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
        agent_resume: Some(AgentResumeLaunch {
            label: "Claude Code".to_string(),
            launch: crate::pane::launch::PaneLaunch::direct(vec![
                "/nonexistent/rozi-test/claude".to_string(),
                "--resume".to_string(),
                "opaque-session-123".to_string(),
            ])
            .expect("resume argv"),
            attach: None,
        }),
        cwd: None,
        title: None,
        cols: 80,
        rows: 10,
        keep_open: false,
        env: Vec::new(),
        palette: test_palette(),
        shell: test_shell(),
        command_shell: test_command_shell(),
        cell: None,
    });

    assert!(
        matches!(result, ServerMessage::SpawnResult { ok: true, .. }),
        "the pane exists: the shell started even though the resume command did not"
    );
    let pane = server.panes.get_mut(&1).expect("pane retained");
    assert!(pane.pty.is_some(), "no shell was left to use");
    assert_eq!(pane.exited, None, "the pane must not come back exited");
    assert!(
        pane.agent_resume.is_none(),
        "nothing was resumed, so nothing is waiting to report a resume failure later"
    );
    assert_eq!(
        pane.launch, None,
        "the pane is a shell now, and its next snapshot must not carry a launch intent that \
         would open a fresh conversation on the restore after this one"
    );
    let text = pane.screen_without_change().snapshot();
    assert!(
        text.contains("Claude Code resume failed"),
        "the failure was not named; screen was:\n{text}"
    );
}

/// A native agent resume that fails must not look like a restore that quietly did nothing: the
/// pane stays, the agent's own output stays above it, the failure is named, and the shell
/// underneath is usable. The pane's own `keep_open` is false here - the hold comes from the resume.
#[test]
fn a_failed_agent_resume_names_the_failure_and_leaves_a_usable_shell() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        // The pane was created to run the agent, so its launch intent is the thing that must not
        // outlive a failed resume into the next snapshot.
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
        agent_resume: Some(AgentResumeLaunch {
            label: "Claude Code".to_string(),
            launch: crate::pane::launch::PaneLaunch::direct(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf 'no conversation with that id\\n' >&2; exit 1".to_string(),
            ])
            .expect("resume argv"),
            attach: None,
        }),
        cwd: None,
        title: None,
        // Wide enough that the notice is not wrapped away from the phrase asserted below.
        cols: 80,
        rows: 10,
        keep_open: false,
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
        let pane = server.panes.get_mut(&1).expect("pane still exists");
        if pane.command_completed
            && pane
                .screen_without_change()
                .snapshot()
                .contains("resume failed")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let pane = server.panes.get_mut(&1).expect("pane still exists");
    assert_eq!(pane.exited, None, "the pane must survive a failed resume");
    assert!(pane.pty.is_some(), "no shell was left to use");
    assert!(
        pane.agent_resume.is_none(),
        "the resume attempt is over; the pane is an ordinary shell now"
    );
    let text = pane.screen_without_change().snapshot();
    assert!(
        text.contains("no conversation with that id"),
        "the agent's own error was lost; screen was:\n{text}"
    );
    assert!(
        text.contains("Claude Code resume failed · exited 1"),
        "the failure was not named; screen was:\n{text}"
    );
    assert_eq!(
        pane.launch, None,
        "a resume that started and then failed leaves the same shell one that never started \
         does, so its launch intent must not survive into the next snapshot either"
    );
}

/// A resume the agent refuses straight away, because the conversation still runs elsewhere, is
/// followed by the agent's own attach command in the same pane. Only an attach that fails too is
/// reported as a failed resume.
#[test]
fn a_refused_agent_resume_falls_back_to_attaching() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);
    let sh = |script: &str| vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
        agent_resume: Some(AgentResumeLaunch {
            label: "Claude Code".to_string(),
            launch: crate::pane::launch::PaneLaunch::direct(sh(
                "printf 'running in the background\\n' >&2; exit 1",
            ))
            .expect("resume argv"),
            attach: Some(sh("printf 'attached to it\\n'; exit 1")),
        }),
        cwd: None,
        title: None,
        cols: 80,
        rows: 10,
        keep_open: false,
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
        let pane = server.panes.get_mut(&1).expect("pane still exists");
        if pane.command_completed
            && pane
                .screen_without_change()
                .snapshot()
                .contains("resume failed")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let pane = server.panes.get_mut(&1).expect("pane still exists");
    assert_eq!(pane.exited, None, "the pane must survive");
    let text = pane.screen_without_change().snapshot();
    for expected in [
        "running in the background",
        "Claude Code is still running · attaching",
        "attached to it",
        "Claude Code resume failed · exited 1",
    ] {
        assert!(
            text.contains(expected),
            "missing {expected:?}; screen was:\n{text}"
        );
    }
    assert_eq!(pane.launch, None, "a failed attach leaves a plain shell");
}

/// Runs a pane whose resume is refused straight away and whose attach is `attach`, until the pane
/// has settled into the shell that follows, and returns it.
fn pane_after_refused_resume(attach: Vec<String>) -> SessionServer {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);
    let sh = |script: &str| vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        // The pane was created to run the agent: restoring this would open a fresh conversation.
        launch: Some(crate::pane::launch::PaneLaunch::shell("true")),
        agent_resume: Some(AgentResumeLaunch {
            label: "Claude Code".to_string(),
            launch: crate::pane::launch::PaneLaunch::direct(sh("exit 1")).expect("resume argv"),
            attach: Some(attach),
        }),
        cwd: None,
        title: None,
        cols: 80,
        rows: 10,
        keep_open: false,
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
    server
}

/// An attach ends normally when the user detaches from a conversation that carries on elsewhere.
/// The shell that follows must not keep the launch that would start a fresh one.
#[test]
fn an_attach_that_exits_cleanly_does_not_leave_a_launch_behind() {
    let server = pane_after_refused_resume(vec!["/bin/sh".into(), "-c".into(), "exit 0".into()]);

    let pane = server.panes.get(&1).expect("pane still exists");
    assert!(
        pane.command_completed,
        "the attach never gave way to a shell"
    );
    assert_eq!(pane.launch, None);
    assert!(pane.agent_resume.is_none());
}

/// An attach command that cannot be spawned falls back to the shell the same way.
#[test]
fn an_attach_that_cannot_start_does_not_leave_a_launch_behind() {
    let mut server = pane_after_refused_resume(vec!["/nonexistent/rozi-test/claude".into()]);

    let pane = server.panes.get_mut(&1).expect("pane still exists");
    assert!(
        pane.command_completed,
        "no shell followed the failed attach"
    );
    assert_eq!(pane.launch, None);
    let text = pane.screen_without_change().snapshot();
    assert!(
        text.contains("Claude Code attach failed"),
        "the failed attach was not named; screen was:\n{text}"
    );
}

/// A command that merely exits keeps its launch intent: a pane created to run something restores
/// by running it again, which is the ordinary keep-open contract. Only a *failed conversation
/// resume* is special, and conflating the two would stop every keep-open pane coming back.
#[test]
fn an_ordinary_keep_open_command_keeps_its_launch_intent() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let launch = crate::pane::launch::PaneLaunch::shell("exit 3");
    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        launch: Some(launch.clone()),
        agent_resume: None,
        cwd: None,
        title: None,
        cols: 80,
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

    let pane = server.panes.get(&1).expect("pane still exists");
    assert!(pane.command_completed, "the keep-open swap never ran");
    assert_eq!(pane.launch, Some(launch));
}

#[test]
fn keep_open_recovers_terminal_modes_before_starting_the_shell() {
    let mut server = SessionServer::new_named("dev");
    let (_client, _stream) = attach_client(&mut server);

    let result = server.spawn_pane(SpawnRequest {
        owner: None,
        pane_id: 1,
        generation: 1,
        launch: Some(crate::pane::launch::PaneLaunch::shell(
            "printf 'primary marker\\n\\033[?1049h\\033[?1003h\\033[?1006h\\033[?1004h\\033[?2004h\\033[?1h\\033=stale app'; exit 3",
        )),
        agent_resume: None,
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
    'wait: while Instant::now() < deadline {
        while let Some(event) = server.events.try_pop() {
            if let Some(outbound) = server.handle_event(event) {
                server.broadcast_outbound(&outbound);
            }
            // The exit event writes SHELL_MODE_RECOVERY and only then marks the replacement
            // complete. Stop at that boundary, before already-queued startup output from the live
            // replacement shell can legitimately re-enable one of the recovered modes.
            if server
                .panes
                .get(&1)
                .is_some_and(|pane| pane.command_completed)
            {
                break 'wait;
            }
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
        launch: Some(crate::pane::launch::PaneLaunch::shell(
            "printf 'popup result\\n'; exit 3",
        )),
        agent_resume: None,
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

/// A drag is relayed, not recorded. The server keeps no copy of it, so the only thing it has to
/// get right is who may speak: the controller, and nobody else.
#[test]
fn only_the_controller_may_publish_a_drag() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    assert_eq!(server.controller, Some(controller));

    let rect = crate::layout::shared::FracRect {
        x: 0.25,
        y: 0.1,
        w: 0.5,
        h: 0.4,
    };
    let responses =
        server.handle_message(controller, ClientMessage::DragUpdate { pane_id: 7, rect });
    assert_eq!(
        responses
            .iter()
            .filter_map(|(target, message)| match (target, message) {
                (Target::Broadcast, ServerMessage::DragChanged { author, drag }) =>
                    Some((*author, *drag)),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(
            controller,
            Some(crate::state::RemoteDrag { pane_id: 7, rect })
        )],
        "the controller's lift is broadcast verbatim"
    );

    assert!(
        server
            .handle_message(follower, ClientMessage::DragUpdate { pane_id: 7, rect })
            .is_empty(),
        "a follower cannot lift a pane on everyone else's screen"
    );
    assert!(
        server
            .handle_message(follower, ClientMessage::DragEnd)
            .is_empty(),
        "nor end a gesture it never owned"
    );
}

/// A read-only client holding the lease is still read-only: its drag messages are as inert as its
/// layout commits.
#[test]
fn a_read_only_client_cannot_publish_a_drag() {
    let mut server = SessionServer::new_named("dev");
    let (_controller, _stream) = attach_client(&mut server);
    let (viewer, _viewer_stream) = attach_read_only_client(&mut server);
    server.controller = Some(viewer);

    assert!(
        server
            .handle_message(
                viewer,
                ClientMessage::DragUpdate {
                    pane_id: 1,
                    rect: crate::layout::shared::FracRect {
                        x: 0.0,
                        y: 0.0,
                        w: 0.5,
                        h: 0.5,
                    },
                },
            )
            .is_empty()
    );
}

/// Ending a drag broadcasts the clearing message to everyone, its author included - one relay
/// path, with the author recognising its own echo client-side.
#[test]
fn ending_a_drag_broadcasts_a_cleared_drag() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _stream) = attach_client(&mut server);

    let responses = server.handle_message(controller, ClientMessage::DragEnd);
    assert!(matches!(
        responses.as_slice(),
        [(
            Target::Broadcast,
            ServerMessage::DragChanged {
                author,
                drag: None
            }
        )] if *author == controller
    ));
}

#[test]
fn sleep_policy_aggregates_semantic_states_across_shared_and_local_panes() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = test_pane(2);
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("codex", "Codex").into(),
        state: protocol::DetectedAgentState::Working,
    });
    pane.runtime.rows = ["done", "working"]
        .into_iter()
        .enumerate()
        .map(|(index, status)| protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: index.to_string(),
            title: status.into(),
            status: status.into(),
            reason: None,
            active: index == 0,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        })
        .collect();
    pane.agent.sync_references(&pane.runtime);
    server.panes.insert(1, pane);
    // The active row is Done, but another subagent is Working.
    assert!(server.any_agent_working());
    for status in ["blocked", "idle", "done"] {
        let pane = server.panes.get_mut(&1).unwrap();
        pane.runtime.rows[1].status = status.into();
        assert!(
            !server.any_agent_working(),
            "{status} overrides detected Working"
        );
    }
    let mut local = test_pane(3);
    local.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("claude", "Claude").into(),
        state: protocol::DetectedAgentState::Working,
    });
    local.agent.sync_references(&local.runtime);
    server.local_panes.insert((42, 1), local);
    assert!(server.any_agent_working());
    server.local_panes.get_mut(&(42, 1)).unwrap().exited = Some(0);
    assert!(
        !server.any_agent_working(),
        "exited panes cannot retain a working lock"
    );
    server.panes.get_mut(&1).unwrap().runtime.rows[1].status = "working".into();
    assert!(server.any_agent_working());
    server.panes.clear();
    assert!(
        !server.any_agent_working(),
        "pane disappearance clears aggregate work"
    );
}

#[test]
fn sleep_policy_reload_uses_host_config_and_requires_writable_controller() {
    let _config = crate::test_support::lock_config_file();
    crate::config::persist_session_flag("keep_awake_while_agents_work", false).unwrap();
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let (viewer, _viewer_stream) = attach_read_only_client(&mut server);
    server.settings.keep_awake_while_agents_work = true;
    for id in [follower, viewer] {
        assert!(
            server
                .handle_message(id, ClientMessage::ReloadSleepPolicy)
                .is_empty()
        );
        assert!(server.settings.keep_awake_while_agents_work);
    }
    server
        .clients
        .iter_mut()
        .find(|client| client.id == controller)
        .unwrap()
        .read_only = true;
    server.handle_message(controller, ClientMessage::ReloadSleepPolicy);
    assert!(server.settings.keep_awake_while_agents_work);
    server
        .clients
        .iter_mut()
        .find(|client| client.id == controller)
        .unwrap()
        .read_only = false;
    server.handle_message(controller, ClientMessage::ReloadSleepPolicy);
    assert!(!server.settings.keep_awake_while_agents_work);
    crate::config::persist_session_flag("keep_awake_while_agents_work", true).unwrap();
    server.handle_message(controller, ClientMessage::ReloadSleepPolicy);
    assert!(server.settings.keep_awake_while_agents_work);
    // A malformed host config must preserve the last accepted policy.
    std::fs::write(crate::config::config_path(), "[session").unwrap();
    server.handle_message(controller, ClientMessage::ReloadSleepPolicy);
    assert!(server.settings.keep_awake_while_agents_work);
    std::fs::write(crate::config::config_path(), "").unwrap();
}

#[test]
fn sleep_policy_uses_integration_authority_including_for_published_rows() {
    let mut server = SessionServer::new_named("dev");
    let mut pane = test_pane(2);
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("claude", "Claude").into(),
        state: protocol::DetectedAgentState::Working,
    });
    pane.runtime.integration = Some(Box::new(protocol::AgentIntegrationReport {
        integration: "hook".into(),
        identity: protocol::AgentIdentity::new("claude", "Claude"),
        reference: protocol::AgentRef {
            pane: protocol::PaneRef {
                session_instance: server.instance_id.clone(),
                pane_id: 1,
                generation: 2,
            },
            slot: None,
            incarnation: 1,
        },
        state: protocol::AgentState::Blocked,
        reason: None,
        native_session: Some("conversation".into()),
        seq: 1,
        reported_at_unix_ms: 0,
    }));
    server.panes.insert(1, pane);
    assert!(
        !server.any_agent_working(),
        "integration overrides detected Working"
    );
    server
        .panes
        .get_mut(&1)
        .unwrap()
        .runtime
        .integration
        .as_mut()
        .unwrap()
        .state = protocol::AgentState::Working;
    assert!(server.any_agent_working());
    let pane = server.panes.get_mut(&1).unwrap();
    pane.runtime.integration.as_mut().unwrap().state = protocol::AgentState::Blocked;
    pane.runtime.rows = vec![protocol::PublishedRow {
        publisher: "sessions".into(),
        kind: crate::session::protocol::PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: "row".into(),
        title: "task".into(),
        status: "working".into(),
        reason: None,
        active: true,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("conversation".into()),
    }];
    pane.agent.sync_references(&pane.runtime);
    assert!(
        !server.any_agent_working(),
        "matching hook overrides published Working"
    );
    let pane = server.panes.get_mut(&1).unwrap();
    pane.runtime.rows[0].native_session = Some("other-conversation".into());
    assert!(
        server.any_agent_working(),
        "a different subagent remains Working"
    );
}

/// A pane running Claude whose client lists `origin` on screen and `fork`'s conversation beside it.
fn pane_with_session_rows(server: &mut SessionServer, id: PaneId) {
    let mut pane = test_pane(1);
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
        state: protocol::DetectedAgentState::Idle,
    });
    pane.runtime.rows = ["origin", "background"]
        .into_iter()
        .map(|id| protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            id: id.into(),
            title: id.into(),
            status: "working".into(),
            reason: None,
            active: id == "origin",
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: Some(id.into()),
            live_session: None,
            reported: false,
        })
        .collect();
    pane.agent.sync_references(&pane.runtime);
    server.panes.insert(id, pane);
}

fn agent_report(
    server: &mut SessionServer,
    token: &str,
    state: Option<protocol::AgentState>,
    native: &str,
    seq: u64,
) -> crate::control::ControlResponse {
    let command = match state {
        Some(state) => crate::control::ControlCommand::AgentReport {
            target: Some(7),
            agent: "claude".into(),
            integration: token.into(),
            state,
            reason: None,
            native_session: Some(native.into()),
            seq,
        },
        None => crate::control::ControlCommand::AgentRelease {
            target: Some(7),
            integration: token.into(),
            seq,
        },
    };
    let messages = server.handle_session_control(
        99,
        server.session_name.clone(),
        PROTOCOL_VERSION,
        protocol::MIN_SUPPORTED_PROTOCOL,
        None,
        crate::control::ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            extension: None,
            caller_context: None,
            credential: None,
        },
    );
    match messages.into_iter().next() {
        Some((Target::Sender, ServerMessage::SessionControlResult { response, .. })) => response,
        other => panic!("expected a control result, got {other:?}"),
    }
}

#[test]
fn a_released_row_reporter_can_never_report_again_or_claim_the_pane() {
    let mut server = SessionServer::new_named("dev");
    pane_with_session_rows(&mut server, 7);
    let working = Some(protocol::AgentState::Working);

    assert!(agent_report(&mut server, "client", working, "origin", 1).ok);
    assert!(agent_report(&mut server, "fork-hooks", working, "background", 1).ok);
    assert!(agent_report(&mut server, "fork-hooks", None, "background", 2).ok);
    assert!(agent_report(&mut server, "client", None, "origin", 2).ok);
    assert!(server.panes[&7].runtime.integration.is_none());

    // A delayed hook from the released background process, with a newer sequence.
    let late = agent_report(&mut server, "fork-hooks", working, "background", 5);
    assert_eq!(late.code, Some(crate::control::ControlErrorCode::Conflict));
    let late = agent_report(&mut server, "fork-hooks", working, "origin", 6);
    assert_eq!(
        late.code,
        Some(crate::control::ControlErrorCode::Conflict),
        "nor may it take the pane the owner gave up"
    );
    assert!(server.panes[&7].runtime.integration.is_none());
}

#[test]
fn a_row_reporter_whose_conversation_is_withdrawn_is_retired() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    pane_with_session_rows(&mut server, 7);
    let working = Some(protocol::AgentState::Working);
    assert!(agent_report(&mut server, "client", working, "origin", 1).ok);
    assert!(agent_report(&mut server, "fork-hooks", working, "background", 1).ok);

    // The rows are this client's: its stream now publishes only the origin row.
    for row in &mut server.panes.get_mut(&7).unwrap().runtime.rows {
        row.publisher = protocol::qualified_publisher(client, "sessions");
    }
    let only_origin = vec![server.panes[&7].runtime.rows[0].clone()];
    server
        .report_pane_rows(
            client,
            7,
            1,
            false,
            super::runtime::PanePublisher {
                id: "sessions".into(),
                kind: protocol::PublisherKind::Agent,
            },
            only_origin,
        )
        .expect("rows publish");
    assert!(server.panes[&7].agent.row_reports.is_empty());

    assert!(agent_report(&mut server, "client", None, "origin", 2).ok);
    let late = agent_report(&mut server, "fork-hooks", working, "origin", 2);
    assert_eq!(late.code, Some(crate::control::ControlErrorCode::Conflict));
    assert!(server.panes[&7].runtime.integration.is_none());
}

fn session_row(
    publisher: String,
    kind: protocol::PublisherKind,
    id: &str,
    native: &str,
) -> protocol::PublishedRow {
    protocol::PublishedRow {
        publisher,
        kind,
        id: id.into(),
        title: id.into(),
        status: "idle".into(),
        reason: None,
        active: false,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some(native.into()),
        live_session: None,
        reported: false,
    }
}

/// A window that detaches takes its rows with it, and a row reporter whose conversation no row
/// names any more goes with them. One another window still lists keeps reporting into that row.
#[test]
fn a_row_reporter_whose_window_detaches_is_retired_unless_another_lists_it() {
    let mut server = SessionServer::new_named("dev");
    let (leaving, _leaving_stream) = attach_client(&mut server);
    let (staying, _staying_stream) = attach_client(&mut server);
    pane_with_session_rows(&mut server, 7);
    let agent = protocol::PublisherKind::Agent;
    let pane = server.panes.get_mut(&7).unwrap();
    for row in &mut pane.runtime.rows {
        row.publisher = protocol::qualified_publisher(leaving, "sessions");
    }
    pane.runtime.rows.push(session_row(
        protocol::qualified_publisher(leaving, "sessions"),
        agent,
        "side",
        "side",
    ));
    pane.runtime.rows.push(session_row(
        protocol::qualified_publisher(staying, "sessions"),
        agent,
        "side",
        "side",
    ));
    pane.agent.sync_references(&pane.runtime);
    let working = Some(protocol::AgentState::Working);
    assert!(agent_report(&mut server, "client", working, "origin", 1).ok);
    assert!(agent_report(&mut server, "fork-hooks", working, "background", 1).ok);
    assert!(agent_report(&mut server, "side-hooks", working, "side", 1).ok);

    server.withdraw_client_rows(leaving);
    let reports = &server.panes[&7].agent.row_reports;
    assert!(!reports.contains_key("fork-hooks"));
    assert!(reports.contains_key("side-hooks"));

    let late = agent_report(&mut server, "fork-hooks", working, "background", 2);
    assert_eq!(late.code, Some(crate::control::ControlErrorCode::Conflict));
    // A row naming the conversation again does not revive the retired token's state.
    let pane = server.panes.get_mut(&7).unwrap();
    pane.runtime.rows.push(session_row(
        protocol::qualified_publisher(staying, "sessions"),
        agent,
        "background",
        "background",
    ));
    super::runtime::fold_row_reports(&mut pane.runtime.rows, &pane.agent.row_reports);
    let row = |server: &SessionServer, id: &str| {
        server.panes[&7]
            .runtime
            .rows
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .unwrap()
    };
    assert!(!row(&server, "background").reported);

    let blocked = Some(protocol::AgentState::Blocked);
    assert!(agent_report(&mut server, "side-hooks", blocked, "side", 2).ok);
    let side = row(&server, "side");
    assert_eq!(
        side.publisher,
        protocol::qualified_publisher(staying, "sessions")
    );
    assert!(side.reported);
    assert_eq!(side.status, protocol::AgentState::Blocked.as_str());
}

/// An activity reports its own state: a hook naming the session an activity row carries neither
/// claims a row report nor rewrites the row.
#[test]
fn a_hook_report_never_reaches_an_activity_row() {
    let mut server = SessionServer::new_named("dev");
    let (client, _stream) = attach_client(&mut server);
    pane_with_session_rows(&mut server, 7);
    let working = Some(protocol::AgentState::Working);
    assert!(agent_report(&mut server, "client", working, "origin", 1).ok);

    // Whatever a publisher sends, an activity row keeps no conversation.
    let publisher = super::runtime::PanePublisher {
        id: "watch".into(),
        kind: protocol::PublisherKind::Activity,
    };
    let sent = session_row(String::new(), publisher.kind, "watch", "side");
    server
        .report_pane_rows(client, 7, 1, false, publisher, vec![sent])
        .expect("rows publish");
    let activity = |server: &SessionServer| {
        server.panes[&7]
            .runtime
            .rows
            .iter()
            .find(|row| !row.is_agent())
            .cloned()
            .unwrap()
    };
    assert_eq!(activity(&server).native_session, None);

    // Even one that slipped a session through is out of a hook's reach.
    let pane = server.panes.get_mut(&7).unwrap();
    for row in &mut pane.runtime.rows {
        if !row.is_agent() {
            row.native_session = Some("side".into());
        }
    }
    let before = activity(&server);
    let report = agent_report(&mut server, "side-hooks", working, "side", 1);
    assert!(!report.ok);
    assert!(server.panes[&7].agent.row_reports.is_empty());
    assert_eq!(activity(&server), before);
}

#[test]
fn a_forwarded_prompt_types_with_its_clients_authority_under_an_input_lock() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let (viewer, _viewer_stream) = attach_read_only_client(&mut server);
    server.controller = Some(controller);
    server.input_locked = true;
    pane_with_session_rows(&mut server, 7);

    let caller_context = protocol::PaneRef {
        session_instance: server.instance_id.clone(),
        pane_id: 7,
        generation: server.panes[&7].generation,
    };
    let mut prompt = |client| {
        let messages = server.handle_attached_control(
            client,
            1,
            crate::control::ControlRequest {
                command: crate::control::ControlCommand::AgentPrompt {
                    target: crate::control::AgentTarget::Pane(7),
                    prompt: "go".into(),
                    wait: None,
                    timeout_ms: None,
                    capture: None,
                    allow_working: true,
                },
                source_pane: None,
                source_session: None,
                extension: None,
                caller_context: Some(Box::new(caller_context.clone())),
                credential: None,
            },
        );
        match messages.into_iter().next() {
            Some((Target::Sender, ServerMessage::AttachedControlResult { response, .. })) => {
                response.code
            }
            other => panic!("expected an attached result, got {other:?}"),
        }
    };

    // The test pane has no process, so the controller gets as far as the write.
    assert_eq!(
        prompt(controller),
        Some(crate::control::ControlErrorCode::PaneNotRunning)
    );
    assert_eq!(
        prompt(follower),
        Some(crate::control::ControlErrorCode::InputLocked)
    );
    assert_eq!(
        prompt(viewer),
        Some(crate::control::ControlErrorCode::ReadOnly)
    );
}

#[test]
fn program_status_is_authoritative_without_clients_and_coalesces_progress() {
    let mut server = SessionServer::new_named("osc-test");
    let mut pane = test_pane(1);
    pane.screen_mut().set_program_status_enabled(true);
    server.panes.insert(1, pane);
    let output = |text: &str| {
        ServerEvent::Pty(
            None,
            1,
            1,
            TerminalPtyEvent::Output(text.as_bytes().to_vec().into()),
        )
    };
    server.handle_event(output("\x1b]7501;state=working:app=codex:progress=0\x07"));
    assert!(server.clients.is_empty());
    let initial = server.panes[&1].screen().program_status_snapshot().clone();
    assert_eq!(initial.records.len(), 1);
    assert!(server.panes[&1].runtime.detected_agent.is_none());
    let signature = protocol::agent_signature(&server.panes[&1].runtime);
    server
        .panes
        .get_mut(&1)
        .unwrap()
        .program_status_sync
        .last_sent = Some(std::time::Instant::now() + std::time::Duration::from_secs(3600));
    for i in 1..=100 {
        server.handle_event(output(&format!(
            "\x1b]7501;state=working:app=codex:progress={i}\x07"
        )));
    }
    assert_eq!(server.panes[&1].program_status_sync.sent, initial);
    assert!(server.panes[&1].program_status_sync.pending);
    assert_eq!(
        server.panes[&1].screen().program_status_snapshot().records[0].progress,
        Some(100)
    );
    server.sync_program_status(None, 1, 1, true);
    let latest = server.panes[&1].program_status_sync.sent.clone();
    assert_eq!(latest.records[0].progress, Some(100));
    server.handle_event(output("\x1b]7501;state=working:app=codex:progress=100\x07"));
    assert_eq!(server.panes[&1].program_status_sync.sent, latest);
    assert!(!server.panes[&1].program_status_sync.pending);
    server.handle_event(output("\x1b]7501;state=error:app=codex\x07"));
    assert_eq!(
        server.panes[&1].program_status_sync.sent.records[0].state,
        tui_lipan::ProgramStatusState::Error
    );
    assert_eq!(
        protocol::agent_signature(&server.panes[&1].runtime),
        signature
    );
    server.handle_event(ServerEvent::Pty(None, 1, 1, TerminalPtyEvent::Exited(1)));
    assert_eq!(
        server.panes[&1].screen().program_status_snapshot().records[0].state,
        tui_lipan::ProgramStatusState::Error
    );
    // A fresh pane generation starts with a new, empty authoritative terminal.
    server.panes.insert(1, test_pane(2));
    assert!(
        server.panes[&1]
            .screen()
            .program_status_snapshot()
            .records
            .is_empty()
    );
}

#[test]
fn an_error_stops_the_server_run_clock() {
    let mut server = SessionServer::new_named("activity-error");
    let (client, _stream) = attach_client(&mut server);
    server.panes.insert(1, status_test_pane(1, None));
    let set = |status: &str| ClientMessage::SetPaneStatus {
        pane_id: 1,
        local: false,
        generation: 1,
        status: Some(status.into()),
        reason: None,
    };
    server.handle_message(client, set("custom"));
    assert!(server.panes[&1].runtime.work_started_at.is_some());
    server.handle_message(client, set("error"));
    assert!(server.panes[&1].runtime.work_started_at.is_none());
}

#[test]
fn program_status_snapshot_follows_the_layout_that_introduces_a_pane() {
    let mut server = SessionServer::new_named("osc-layout");
    let (controller, _stream) = attach_client(&mut server);
    let mut pane = test_pane(1);
    pane.screen_mut().set_program_status_enabled(true);
    pane.screen_mut()
        .process_bytes(b"\x1b]7501;state=working:progress=42\x07");
    server.panes.insert(1, pane);
    let layout = shared_layout_for_panes(&[(1, 1)]);
    let responses = server.handle_commit_layout(controller, 0, layout.clone(), false);
    assert!(matches!(
        responses[0],
        (Target::Broadcast, ServerMessage::LayoutCommitted { .. })
    ));
    let (
        Target::Broadcast,
        ServerMessage::ProgramStatusChanged {
            pane_id: 1,
            generation: 1,
            snapshot,
            ..
        },
    ) = &responses[1]
    else {
        panic!("missing post-layout snapshot: {responses:?}");
    };
    assert_eq!(snapshot.records[0].progress, Some(42));
    assert!(
        server
            .program_status_for_new_layout_panes(&layout)
            .is_empty()
    );
}

fn independent_output(sequence: u64, epoch: u64, image: u32) -> protocol::OutputMetadata {
    protocol::OutputMetadata {
        sequence,
        coalesce_epoch: epoch,
        class: protocol::OutputClass::IndependentGraphics {
            image,
            incarnation: 1,
            revision: sequence,
        },
    }
}

#[test]
fn client_outboxes_coalesce_independent_frames_and_preserve_ordered_barriers() {
    let mut server = SessionServer::new_named("graphics");
    let (id, _stream) = attach_client(&mut server);
    for sequence in 1..=100 {
        server.broadcast_outbound(&ServerOutbound::PaneOutput {
            media: Vec::new(),
            pane_id: 7,
            local: false,
            generation: 2,
            metadata: independent_output(sequence, 0, 7),
            bytes: sequence.to_be_bytes().to_vec(),
        });
    }
    let client = server.client_mut(id).unwrap();
    assert_eq!(client.outbox.len(), 1);
    assert_eq!(client.coalesced_frames, 99);
    assert_eq!(decode_outbox_frames(client).len(), 1);
    client.front_offset = 1;
    client.outbox_bytes -= 1;
    server.broadcast_outbound(&ServerOutbound::PaneOutput {
        media: Vec::new(),
        pane_id: 7,
        local: false,
        generation: 2,
        metadata: independent_output(101, 0, 7),
        bytes: b"pinned frame continuation".to_vec(),
    });
    assert_eq!(server.client_mut(id).unwrap().outbox.len(), 2);
    server.broadcast_outbound(&ServerOutbound::PaneOutput {
        media: Vec::new(),
        pane_id: 7,
        local: false,
        generation: 2,
        metadata: protocol::OutputMetadata {
            sequence: 102,
            coalesce_epoch: 1,
            class: protocol::OutputClass::Ordered,
        },
        bytes: b"text barrier".to_vec(),
    });
    for sequence in 103..=104 {
        server.broadcast_outbound(&ServerOutbound::PaneOutput {
            media: Vec::new(),
            pane_id: 7,
            local: false,
            generation: 2,
            metadata: independent_output(sequence, 1, 7),
            bytes: sequence.to_be_bytes().to_vec(),
        });
    }
    assert_eq!(server.client_mut(id).unwrap().outbox.len(), 4);
}

#[test]
fn semantic_packetization_waits_for_complete_graphics_commands() {
    let mut server = SessionServer::new_named("graphics");
    let mut pane = test_pane(2);
    pane.screen_without_change()
        .process_bytes(b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,q=2,C=1;YWJj\x1b\\");
    pane.screen_without_change()
        .set_output_transaction_recording(true);
    server.panes.insert(7, pane);
    let (id, _stream) = attach_client(&mut server);
    let frame = b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,C=1,q=2;ZGVm\x1b\\";
    for bytes in [&frame[..10], &frame[10..]] {
        server
            .panes
            .get_mut(&7)
            .unwrap()
            .screen_mut()
            .process_bytes(bytes);
        server.send_outbound(
            None,
            &ServerOutbound::PaneOutput {
                media: Vec::new(),
                metadata: protocol::OutputMetadata::default(),
                pane_id: 7,
                local: false,
                generation: 2,
                bytes: bytes.to_vec(),
            },
        );
        if bytes.len() == 10 {
            assert!(server.client_mut(id).unwrap().outbox.is_empty());
        }
    }
    let client = server.client_mut(id).unwrap();
    assert_eq!(client.outbox.len(), 1);
    let bytes = client.outbox.front().unwrap().bytes.clone();
    let mut decoder = protocol::FrameDecoder::default();
    decoder.read_from_status(&mut &bytes[..]).unwrap();
    let Some(Frame::PaneOutput {
        metadata, bytes, ..
    }) = decoder.next_frame::<ServerMessage>().unwrap()
    else {
        panic!("output frame")
    };
    assert_eq!(bytes, frame);
    assert!(matches!(
        metadata.class,
        protocol::OutputClass::IndependentGraphics { image: 7, .. }
    ));
    assert_eq!(metadata.sequence, 1);
}

/// `InputMarked` is a send's wait baseline: an independent graphics frame produced after the mark
/// must not replace one queued ahead of it, or the change it carries would land before the mark.
#[test]
fn independent_graphics_after_an_input_mark_never_replace_frames_ahead_of_it() {
    let mut server = SessionServer::new_named("graphics");
    let mut pane = test_pane(2);
    pane.screen_without_change()
        .process_bytes(b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,q=2,C=1;YWJj\x1b\\");
    pane.screen_without_change()
        .set_output_transaction_recording(true);
    server.panes.insert(7, pane);
    let (id, _stream) = attach_client(&mut server);
    let send_frame = |server: &mut SessionServer, payload: &[u8]| {
        let mut bytes = b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,C=1,q=2;".to_vec();
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(b"\x1b\\");
        server
            .panes
            .get_mut(&7)
            .unwrap()
            .screen_mut()
            .process_bytes(&bytes);
        server.send_outbound(
            None,
            &ServerOutbound::PaneOutput {
                media: Vec::new(),
                metadata: protocol::OutputMetadata::default(),
                pane_id: 7,
                local: false,
                generation: 2,
                bytes: bytes.clone(),
            },
        );
        bytes
    };

    let before = send_frame(&mut server, b"QUFB");
    server.process_client_frame(
        id,
        Frame::Control(ClientMessage::MarkedInput {
            pane_id: 7,
            local: false,
            generation: 2,
            bytes: b"x".to_vec(),
            token: 9,
        }),
    );
    send_frame(&mut server, b"QkJC");
    let after = send_frame(&mut server, b"Q0ND");

    let frames = decode_outbox_frames(server.client_mut(id).unwrap());
    assert_eq!(frames.len(), 3, "frames: {frames:?}");
    assert!(matches!(&frames[0], DecodedOutboxFrame::Pane { bytes, .. } if *bytes == before));
    assert!(matches!(&frames[1], DecodedOutboxFrame::Control(message)
        if matches!(**message, ServerMessage::InputMarked { token: 9 })));
    // Frames behind the mark still replace one another.
    assert!(matches!(&frames[2], DecodedOutboxFrame::Pane { bytes, .. } if *bytes == after));
}

#[test]
fn semantic_staging_coalesces_independent_transactions_before_client_outboxes() {
    let mut server = SessionServer::new_named("graphics");
    let mut pane = test_pane(2);
    pane.screen_without_change()
        .process_bytes(b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,q=2,C=1;YWJj\x1b\\");
    pane.screen_without_change()
        .set_output_transaction_recording(true);
    server.panes.insert(7, pane);
    let (id, _stream) = attach_client(&mut server);
    let frame = b"\x1b_Ga=T,i=7,p=9,f=24,s=1,v=1,C=1,q=2;ZGVm\x1b\\";
    let bytes = frame.repeat(100);
    server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_mut()
        .process_bytes(&bytes);
    server.send_outbound(
        None,
        &ServerOutbound::PaneOutput {
            media: Vec::new(),
            metadata: protocol::OutputMetadata::default(),
            pane_id: 7,
            local: false,
            generation: 2,
            bytes,
        },
    );
    assert_eq!(server.client_mut(id).unwrap().outbox.len(), 1);
    assert_eq!(
        server.client_mut(id).unwrap().coalesced_frames,
        0,
        "supersession happened before fanout"
    );
    let metrics = server.runtime_metrics().graphics;
    assert_eq!(metrics.replaceable_transactions, 100);
    assert_eq!(metrics.staged_coalesced_frames, 99);
    assert_eq!(metrics.staged_coalesced_bytes, frame.len() as u64 * 99);
}

#[test]
fn resize_flushes_incomplete_output_and_creates_a_coalescing_barrier() {
    let mut server = SessionServer::new_named("graphics");
    let mut pane = test_pane(2);
    pane.screen_without_change()
        .set_output_transaction_recording(true);
    pane.output_seen = true;
    server.panes.insert(7, pane);
    let (id, _stream) = attach_client(&mut server);
    server.controller = Some(id);
    let prefix = b"\x1b_Ga=t,i=7,f=24,s=1,v=1,q=2;";
    server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_mut()
        .process_bytes(prefix);
    server.handle_resize_with(
        id,
        super::connection::PaneResizeRequest {
            pane_id: 7,
            local: false,
            generation: 2,
            cols: 81,
            rows: 25,
            cell_width: 10,
            cell_height: 20,
        },
        |_| Ok(()),
    );
    assert!(server.panes.get(&7).unwrap().output_coalesce_epoch > 0);
    let frames = decode_outbox_frames(server.client_mut(id).unwrap());
    assert!(
        frames.iter().any(
            |frame| matches!(frame, DecodedOutboxFrame::Pane { bytes, .. } if bytes == prefix)
        )
    );
}

#[test]
fn focused_controller_paces_independently_of_a_slow_read_only_follower() {
    let mut server = SessionServer::new_named("graphics-priority");
    server.panes.insert(7, test_pane(11));
    let (fast, _fast_stream) = attach_client(&mut server);
    let (slow, _slow_stream) = attach_read_only_client(&mut server);
    for (client, fps, sequence) in [(fast, 240, 100), (slow, 30, 1)] {
        server.process_client_frame(
            client,
            Frame::PresentationInterest(vec![protocol::PanePresentationInterest {
                pane_id: 7,
                generation: 11,
                local: false,
                visible: true,
                focused: true,
                frames_per_second: fps,
            }]),
        );
        server
            .client_mut(client)
            .unwrap()
            .presented_sequences
            .insert(
                PresentationKey {
                    pane_id: 7,
                    generation: 11,
                    local: false,
                },
                sequence,
            );
    }
    let presenter = server.graphics_presenter(7, 11, None).unwrap();
    assert_eq!(
        (
            presenter.client,
            presenter.frames_per_second,
            presenter.presented
        ),
        (fast, 240, 100)
    );
    server.client_mut(fast).unwrap().parked = true;
    assert_eq!(server.graphics_presenter(7, 11, None).unwrap().client, slow);
    server
        .client_mut(slow)
        .unwrap()
        .presentation_interest
        .clear();
    assert_eq!(server.graphics_presenter(7, 11, None), None);
}

fn report_visible(server: &mut SessionServer, client: ClientId, generation: u64, fps: u16) {
    server.process_client_frame(
        client,
        Frame::PresentationInterest(vec![protocol::PanePresentationInterest {
            pane_id: 7,
            generation,
            local: false,
            visible: true,
            focused: true,
            frames_per_second: fps,
        }]),
    );
}

#[test]
fn a_stalled_leader_yields_pacing_to_a_responsive_follower() {
    let mut server = SessionServer::new_named("graphics-stalled-leader");
    let mut pane = test_pane(11);
    pane.output_sequence = 10;
    let long_ago = Instant::now() - Duration::from_secs(1);
    pane.output_times = (6..=10).map(|sequence| (sequence, long_ago)).collect();
    server.panes.insert(7, pane);
    let (leader, _leader_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_read_only_client(&mut server);
    report_visible(&mut server, leader, 11, 240);
    report_visible(&mut server, follower, 11, 60);
    let key = PresentationKey {
        pane_id: 7,
        generation: 11,
        local: false,
    };
    // The leader stopped acknowledging at 5; the follower has presented everything.
    for (client, sequence) in [(leader, 5), (follower, 10)] {
        let client = server.client_mut(client).unwrap();
        client.presented_sequences.insert(key, sequence);
        client.presented_at.insert(key, long_ago);
    }
    assert_eq!(
        server.graphics_presenter(7, 11, None).unwrap().client,
        follower
    );

    // With no responsive alternative, the leader keeps the lead rather than none leading.
    server
        .client_mut(follower)
        .unwrap()
        .presentation_interest
        .clear();
    assert_eq!(
        server.graphics_presenter(7, 11, None).unwrap().client,
        leader
    );
}

#[test]
fn presentation_state_forgets_replaced_pane_generations() {
    let mut server = SessionServer::new_named("graphics-pruning");
    let mut pane = test_pane(11);
    pane.output_sequence = 3;
    server.panes.insert(7, pane);
    let (client, _stream) = attach_client(&mut server);
    report_visible(&mut server, client, 11, 60);
    server.process_client_frame(
        client,
        Frame::PresentationAcks(vec![protocol::PresentationAck {
            pane_id: 7,
            generation: 11,
            local: false,
            sequence: 3,
        }]),
    );
    assert_eq!(
        server.client_mut(client).unwrap().presented_sequences.len(),
        1
    );

    server.panes.get_mut(&7).unwrap().generation = 12;
    report_visible(&mut server, client, 12, 60);
    let state = server.client_mut(client).unwrap();
    let generations = |keys: Vec<&PresentationKey>| {
        keys.into_iter()
            .map(|key| key.generation)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        generations(state.presentation_interest.keys().collect()),
        [12]
    );
    assert!(state.presented_sequences.is_empty());
    assert!(state.presented_at.is_empty());
}

#[test]
fn local_presentation_interest_and_acknowledgements_are_owner_scoped() {
    let mut server = SessionServer::new_named("graphics-local");
    let (first, _first_stream) = attach_client(&mut server);
    let (second, _second_stream) = attach_client(&mut server);
    for owner in [first, second] {
        let mut pane = test_pane(11);
        pane.output_sequence = 10;
        server.local_panes.insert((owner, 7), pane);
        server.process_client_frame(
            owner,
            Frame::PresentationInterest(vec![protocol::PanePresentationInterest {
                pane_id: 7,
                generation: 11,
                local: true,
                visible: true,
                focused: true,
                frames_per_second: 120,
            }]),
        );
    }
    server.process_client_frame(
        first,
        Frame::PresentationAcks(vec![protocol::PresentationAck {
            pane_id: 7,
            generation: 11,
            local: true,
            sequence: 10,
        }]),
    );
    assert_eq!(
        server
            .graphics_presenter(7, 11, Some(first))
            .unwrap()
            .presented,
        10
    );
    assert_eq!(
        server
            .graphics_presenter(7, 11, Some(second))
            .unwrap()
            .presented,
        0
    );
    assert_eq!(server.graphics_presenter(7, 12, Some(first)), None);
}

/// What a pane's child prints for `TERM_PROGRAM` and `TERM_PROGRAM_VERSION`, spawned through the
/// server's own PTY configuration with `env` applied afterwards, as a pane's environment is.
fn child_terminal_program(env: &[(&str, &str)]) -> String {
    let launch = crate::pane::launch::PaneLaunch::Direct {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            r#"printf '[%s|%s]' "$TERM_PROGRAM" "$TERM_PROGRAM_VERSION""#.to_string(),
        ],
    };
    let mut config = pty_config(Some(&launch), &[], &[]);
    for (key, value) in env {
        config = config.env(*key, *value);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let _pty = TerminalPty::spawn(config, move |event| {
        let _ = sender.send(event);
    })
    .expect("spawn pane child");
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(event) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        match event {
            TerminalPtyEvent::Output(bytes) => output.extend_from_slice(&bytes),
            TerminalPtyEvent::Exited(_) | TerminalPtyEvent::Error(_) => break,
        }
    }
    let output = String::from_utf8_lossy(&output).into_owned();
    let start = output
        .find('[')
        .expect("child printed its terminal program");
    let end = output[start..].find(']').expect("complete line") + start;
    output[start + 1..end].to_string()
}

#[test]
fn the_server_names_itself_as_every_panes_terminal() {
    assert_eq!(
        child_terminal_program(&[]),
        format!("rozi|{}", env!("CARGO_PKG_VERSION"))
    );
    // A pane's own environment comes after the server's, so an explicit value still wins.
    assert_eq!(
        child_terminal_program(&[("TERM_PROGRAM", "custom"), ("TERM_PROGRAM_VERSION", "9")]),
        "custom|9"
    );
}

mod transactions;

fn media_test_server() -> SessionServer {
    let mut server = SessionServer::new_named("graphics-owned");
    let mut pane = test_pane(2);
    pane.screen_mut().set_output_transaction_recording(true);
    // As in production, see `new_terminal_screen`.
    pane.screen_mut().set_image_file_storage_enabled(true);
    server.panes.insert(7, pane);
    server
}

fn named_media_command(medium: char, name: &[u8], offset: usize, size: usize) -> Vec<u8> {
    use base64::Engine as _;
    let name = base64::engine::general_purpose::STANDARD.encode(name);
    format!("\x1b_Ga=T,t={medium},f=32,s=1,v=1,i=7,p=1,C=1,q=2,O={offset},S={size};{name}\x1b\\")
        .into_bytes()
}

fn queue_media_bytes(server: &SessionServer, pane: PaneId, bytes: Vec<u8>) {
    let len = bytes.len();
    server
        .events
        .try_push(
            ServerEvent::Pty(None, pane, 2, TerminalPtyEvent::Output(bytes.into())),
            len,
        )
        .unwrap_or_else(|_| panic!("queue media input"));
}

fn finish_media_events(server: &mut SessionServer) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        server.drain_pty_events();
        server.pump_media_output();
        if server.events.stats().len == 0 && !server.media.pending() {
            return;
        }
        assert!(Instant::now() < deadline, "media worker failed to complete");
        std::thread::yield_now();
    }
}

fn assert_inline_pixel(bytes: &[u8], expected: &[u8]) {
    assert!(!bytes.windows(3).any(|key| key == b"t=t" || key == b"t=s"));
    let mut receiver = TerminalScreen::new(5, 20, 100);
    receiver.set_image_media_policy(GraphicsMediaPolicy::NONE);
    receiver.process_bytes(bytes);
    let capture = receiver.capture_frame();
    assert_eq!(
        &capture.images.first().expect("retained image").rgba[..4],
        expected
    );
}

/// Local clients are sent the server's immutable snapshot by name: one `t=f` reference that every
/// client can read, which is neither the consumed source nor a consuming reference.
fn assert_snapshot_pixel(bytes: &[u8], source: &std::path::Path, expected: &[u8]) {
    use base64::Engine as _;
    assert!(!bytes.windows(3).any(|key| key == b"t=t" || key == b"t=s"));
    assert!(bytes.windows(3).any(|key| key == b"t=f"));
    let source =
        base64::engine::general_purpose::STANDARD.encode(source.as_os_str().as_encoded_bytes());
    assert!(
        !bytes
            .windows(source.len())
            .any(|name| name == source.as_bytes())
    );
    for _ in 0..2 {
        let mut receiver = TerminalScreen::new(5, 20, 100);
        receiver.set_image_media_policy(GraphicsMediaPolicy::SHARED);
        receiver.process_bytes(bytes);
        let capture = receiver.capture_frame();
        assert_eq!(
            &capture.images.first().expect("retained image").rgba[..4],
            expected
        );
    }
}

#[test]
fn the_server_consumes_a_temp_file_once_for_two_local_clients_and_late_attach() {
    let mut server = media_test_server();
    let (first, _first_stream) = attach_client(&mut server);
    let (second, _second_stream) = attach_client(&mut server);
    server.client_mut(first).unwrap().outbox.clear();
    server.client_mut(second).unwrap().outbox.clear();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tty-graphics-protocol-once");
    fs::write(&path, [1, 2, 3, 4, 255, 0, 0, 255, 5]).unwrap();
    queue_media_bytes(
        &server,
        7,
        named_media_command('t', path.as_os_str().as_encoded_bytes(), 4, 4),
    );
    finish_media_events(&mut server);
    assert!(!path.exists());
    // A later unlink or a downstream consuming reference would destroy this replacement.
    fs::write(&path, [0, 255, 0, 255]).unwrap();
    let bytes = sent_pane_output(&mut server, first);
    assert_eq!(sent_pane_output(&mut server, second), bytes);
    assert_snapshot_pixel(&bytes, &path, &[255, 0, 0, 255]);
    let replay = server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_without_change()
        .export_replay_bytes();
    assert_inline_pixel(&replay, &[255, 0, 0, 255]);
    let (late, _late_stream) = attach_remote_client(&mut server);
    server.enqueue_attach_seeds(late);
    while server.pump_attach_seeds() {}
    assert_inline_pixel(&sent_pane_output(&mut server, late), &[255, 0, 0, 255]);
    assert!(
        path.exists(),
        "clients/replay must not delete a replacement at the old name"
    );
}

#[test]
fn shared_memory_is_unlinked_once_and_retained_for_local_and_remote_readers() {
    use crate::platform::media_fixture::SharedMemoryFixture;
    let mut server = media_test_server();
    let (local, _local_stream) = attach_client(&mut server);
    let (remote, _remote_stream) = attach_remote_client(&mut server);
    server.client_mut(local).unwrap().outbox.clear();
    server.client_mut(remote).unwrap().outbox.clear();
    let source = SharedMemoryFixture::new(&[1, 2, 3, 4, 255, 0, 0, 255, 5]).unwrap();
    queue_media_bytes(&server, 7, named_media_command('s', source.name(), 4, 4));
    finish_media_events(&mut server);
    assert!(!source.exists());
    source.recreate(&[0, 255, 0, 255]).unwrap();
    let bytes = sent_pane_output(&mut server, local);
    assert_eq!(sent_pane_output(&mut server, remote), bytes);
    assert_inline_pixel(&bytes, &[255, 0, 0, 255]);
    let replay = server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_without_change()
        .export_replay_bytes();
    assert_inline_pixel(&replay, &[255, 0, 0, 255]);
    assert!(
        source.exists(),
        "no second unlink may target the old source name"
    );
}

#[test]
fn a_remote_attaching_while_a_file_is_prepared_gets_the_acquired_pixels() {
    let mut server = media_test_server();
    let (local, _local_stream) = attach_client(&mut server);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("frame");
    fs::write(&path, [255, 0, 0, 255]).unwrap();
    let frame = named_media_command('f', path.as_os_str().as_encoded_bytes(), 0, 4);
    queue_media_bytes(&server, 7, frame);
    assert!(
        server.next_ready_event().is_none(),
        "named input waits for its worker"
    );
    let (remote, _remote_stream) = attach_remote_client(&mut server);
    server.client_mut(local).unwrap().outbox.clear();
    server.client_mut(remote).unwrap().outbox.clear();
    finish_media_events(&mut server);
    let bytes = sent_pane_output(&mut server, local);
    assert!(!bytes.windows(3).any(|key| key == b"t=f"));
    assert_eq!(sent_pane_output(&mut server, remote), bytes);
    assert_inline_pixel(&bytes, &[255, 0, 0, 255]);
}

#[test]
fn failed_consuming_sources_emit_no_reference_and_leave_other_panes_live() {
    let mut server = media_test_server();
    server.panes.insert(8, test_pane(2));
    let (local, _local_stream) = attach_client(&mut server);
    server.client_mut(local).unwrap().outbox.clear();
    let root = tempfile::tempdir().unwrap();
    for exists in [true, false] {
        let path = root.path().join("tty-graphics-protocol-empty");
        if exists {
            fs::write(&path, []).unwrap();
        }
        queue_media_bytes(
            &server,
            7,
            named_media_command('t', path.as_os_str().as_encoded_bytes(), 0, 0),
        );
        queue_media_bytes(&server, 8, b"other pane continues".to_vec());
        let ready = server
            .next_ready_event()
            .expect("unrelated text must be ready");
        assert!(matches!(ready, ServerEvent::Pty(None, 8, 2, _)));
        assert!(
            server.events.stats().bytes > 0,
            "pending input remains charged"
        );
        server.handle_event(ready);
        finish_media_events(&mut server);
        assert!(!path.exists());
        assert!(
            !server
                .panes
                .get_mut(&7)
                .unwrap()
                .screen_without_change()
                .has_images()
        );
        let bytes = sent_pane_output(&mut server, local);
        assert!(!bytes.windows(3).any(|key| key == b"t=t"));
        assert!(String::from_utf8_lossy(&bytes).contains("other pane continues"));
    }
}

#[test]
fn deferred_image_and_its_input_tail_commit_before_a_resize_barrier() {
    let mut server = media_test_server();
    let (local, _local_stream) = attach_client(&mut server);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tty-graphics-protocol-resize");
    fs::write(&path, [255, 0, 0, 255]).unwrap();
    let mut frame = named_media_command('t', path.as_os_str().as_encoded_bytes(), 0, 4);
    frame.extend_from_slice(b"before-resize");
    queue_media_bytes(&server, 7, frame);
    assert!(server.next_ready_event().is_none());
    server.process_client_frame(local, Frame::Control(resize_message(7, false, 2, 40, 10)));
    assert_eq!(server.panes[&7].cols, 20, "resize must not overtake media");
    queue_media_bytes(&server, 7, b"after-resize".to_vec());
    finish_media_events(&mut server);
    assert_eq!(server.panes[&7].cols, 40);
    let screen = server.panes.get_mut(&7).unwrap().screen_without_change();
    assert!(screen.has_images());
    let text = screen.export_text(0, screen.total_text_lines());
    assert!(text.contains("before-resize"));
    assert!(text.contains("after-resize"));
}

#[test]
fn a_remote_detaching_during_preparation_restores_immutable_snapshot_references() {
    for medium in ['f', 't'] {
        let mut server = media_test_server();
        let (local, _local_stream) = attach_client(&mut server);
        let (remote, _remote_stream) = attach_remote_client(&mut server);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tty-graphics-protocol-detach");
        fs::write(&path, [255, 0, 0, 255]).unwrap();
        let frame = named_media_command(medium, path.as_os_str().as_encoded_bytes(), 0, 4);
        server.client_mut(local).unwrap().outbox.clear();
        queue_media_bytes(&server, 7, frame.clone());
        assert!(server.next_ready_event().is_none());
        server.remove_client(remote);
        finish_media_events(&mut server);
        let bytes = sent_pane_output(&mut server, local);
        assert_eq!(path.exists(), medium == 'f');
        assert_snapshot_pixel(&bytes, &path, &[255, 0, 0, 255]);
    }
}

#[test]
fn large_relay_emission_allows_text_between_its_chunks_and_keeps_the_next_source_unclaimed() {
    let mut server = media_test_server();
    // Relay inline, as for a remote reader or a frame no snapshot can hold.
    server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_mut()
        .set_image_file_storage_enabled(false);
    server.panes.insert(8, test_pane(2));
    let (local, _local_stream) = attach_client(&mut server);
    server.client_mut(local).unwrap().outbox.clear();
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("tty-graphics-protocol-large");
    let second = root.path().join("tty-graphics-protocol-next");
    let mut pixels = vec![0; 1024 * 1024];
    let mut state = 0x12345678_u32;
    for pixel in pixels.chunks_exact_mut(4) {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        pixel.copy_from_slice(&state.to_le_bytes());
    }
    fs::write(&first, pixels).unwrap();
    fs::write(&second, [0, 255, 0, 255]).unwrap();
    let frame = named_media_command('t', first.as_os_str().as_encoded_bytes(), 0, 0);
    let frame = String::from_utf8(frame)
        .unwrap()
        .replace("s=1,v=1", "s=512,v=512");
    queue_media_bytes(&server, 7, frame.into_bytes());
    queue_media_bytes(
        &server,
        7,
        named_media_command('t', second.as_os_str().as_encoded_bytes(), 0, 4),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !server
        .panes
        .get_mut(&7)
        .unwrap()
        .screen_without_change()
        .has_images()
    {
        server.drain_pty_events();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!first.exists());
    assert!(second.exists());
    assert!(server.pump_media_output());
    let first_chunk = sent_pane_output(&mut server, local);
    assert!(first_chunk.len() <= 256 * 1024);
    let mut receiver = TerminalScreen::new(5, 20, 100);
    receiver.process_bytes(&first_chunk);
    assert!(
        !receiver.has_images(),
        "the large image has not completed yet"
    );
    let (late, _late_stream) = attach_remote_client(&mut server);
    server.enqueue_attach_seeds(late);
    while server.pump_attach_seeds() {}
    assert_eq!(
        server
            .client_mut(late)
            .unwrap()
            .seed
            .as_ref()
            .unwrap()
            .manifest
            .get(&PaneSeedKey {
                pane_id: 7,
                generation: 2
            }),
        Some(&PaneSeedState::Pending),
        "a baseline must not start between image continuation chunks"
    );
    queue_media_bytes(&server, 8, b"__text_between_chunks__".to_vec());
    let text = server
        .next_ready_event()
        .expect("unrelated text can bypass pending image output");
    assert!(matches!(text, ServerEvent::Pty(None, 8, 2, _)));
    server.handle_event(text);
    assert!(
        String::from_utf8_lossy(&sent_pane_output(&mut server, local))
            .contains("__text_between_chunks__")
    );
    assert!(
        second.exists(),
        "a second expanded image must not accumulate while emitting the first"
    );
    finish_media_events(&mut server);
    assert!(!second.exists());
    while server.pump_attach_seeds() {}
    assert_inline_pixel(
        &sent_pane_output_matching(&mut server, late, |pane| pane == 7),
        &[0, 255, 0, 255],
    );
}

fn start_barrier_test_media(server: &mut SessionServer) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tty-graphics-protocol-barrier");
    fs::write(&path, [255, 0, 0, 255]).unwrap();
    queue_media_bytes(
        server,
        7,
        named_media_command('t', path.as_os_str().as_encoded_bytes(), 0, 4),
    );
    assert!(server.next_ready_event().is_none());
    assert!(server.media.pending());
    root
}

#[test]
fn media_fence_keeps_layout_drag_end_and_detach_in_client_order() {
    let mut server = media_test_server();
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let _source = start_barrier_test_media(&mut server);
    for message in [
        resize_message(7, false, 2, 40, 10),
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: shared_layout_for_panes(&[(7, 2)]),
            live: false,
        },
        ClientMessage::DragEnd,
        ClientMessage::Detach,
    ] {
        server.process_client_frame(controller, Frame::Control(message));
    }
    assert!(
        server.client_attached(controller),
        "Detach overtook the fence"
    );
    assert_eq!(server.layout_rev, 0);
    assert!(
        !decode_outbox_controls(server.client_mut(follower).unwrap())
            .iter()
            .any(|message| matches!(message, ServerMessage::DragChanged { .. }))
    );
    finish_media_events(&mut server);
    assert_eq!(server.panes[&7].cols, 40);
    assert_eq!(server.layout_rev, 1, "the final commit must survive Detach");
    assert!(!server.client_attached(controller));
    let messages = decode_outbox_controls(server.client_mut(follower).unwrap());
    let commit = messages
        .iter()
        .position(|message| matches!(message, ServerMessage::LayoutCommitted { .. }))
        .unwrap();
    let drag_end = messages
        .iter()
        .position(|message| matches!(message, ServerMessage::DragChanged { drag: None, .. }))
        .unwrap();
    assert!(commit < drag_end, "layout must precede DragEnd");
}

#[test]
fn media_fence_drains_final_controls_even_when_the_connection_has_closed() {
    let mut server = media_test_server();
    let (controller, mut stream) = attach_client(&mut server);
    let _source = start_barrier_test_media(&mut server);
    for message in [
        resize_message(7, false, 2, 40, 10),
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: shared_layout_for_panes(&[(7, 2)]),
            live: false,
        },
    ] {
        protocol::write_frame(&mut stream, &message).unwrap();
    }
    server.enqueue(
        controller,
        Target::Sender,
        ServerMessage::LayoutRejected {
            current_rev: 0,
            layout: None,
        },
    );
    drop(stream);
    server.pump_clients();
    server.flush_clients();
    assert!(server.client_attached(controller));
    assert!(server.client_mut(controller).unwrap().transport_closed);
    finish_media_events(&mut server);
    assert_eq!(server.layout_rev, 1);
    assert!(!server.client_attached(controller));
}

#[test]
fn media_fence_does_not_hold_heartbeat_pongs() {
    let mut server = media_test_server();
    server.settings.heartbeat_timeout = Duration::from_millis(50);
    let (controller, _stream) = attach_client(&mut server);
    let _source = start_barrier_test_media(&mut server);
    server.process_client_frame(
        controller,
        Frame::Control(resize_message(7, false, 2, 40, 10)),
    );
    assert!(server.media.client_fenced(controller));
    std::thread::sleep(Duration::from_millis(60));
    server.process_client_frame(controller, Frame::Control(ClientMessage::Pong { seq: 1 }));
    server.heartbeat();
    assert!(server.client_attached(controller));
    assert!(server.media.client_fenced(controller));
    finish_media_events(&mut server);
    assert_eq!(server.panes[&7].cols, 40);
}

#[test]
fn heartbeat_keeps_a_closed_client_until_its_media_fence_drains() {
    let mut server = media_test_server();
    server.settings.heartbeat_timeout = Duration::from_millis(1);
    let (controller, mut stream) = attach_client(&mut server);
    let _source = start_barrier_test_media(&mut server);
    for message in [
        resize_message(7, false, 2, 40, 10),
        ClientMessage::CommitLayout {
            base_rev: 0,
            layout: shared_layout_for_panes(&[(7, 2)]),
            live: false,
        },
    ] {
        protocol::write_frame(&mut stream, &message).unwrap();
    }
    drop(stream);
    server.pump_clients();
    assert!(server.client_mut(controller).unwrap().transport_closed);
    std::thread::sleep(Duration::from_millis(5));
    server.heartbeat();
    assert!(server.client_attached(controller));
    finish_media_events(&mut server);
    assert_eq!(server.layout_rev, 1);
    assert!(!server.client_attached(controller));
}

#[test]
fn media_in_one_pane_does_not_fence_other_panes_or_layout_metadata() {
    let mut server = media_test_server();
    server.panes.insert(8, test_pane(2));
    let (controller, _stream) = attach_client(&mut server);
    let _source = start_barrier_test_media(&mut server);
    server.process_client_frame(
        controller,
        Frame::Control(resize_message(8, false, 2, 40, 10)),
    );
    let mut palette = test_palette();
    palette.background = Some(Color::Red);
    server.process_client_frame(
        controller,
        Frame::Control(ClientMessage::SetPalette {
            pane_id: 8,
            local: false,
            generation: 2,
            palette,
        }),
    );
    assert_eq!(
        server
            .panes
            .get_mut(&8)
            .unwrap()
            .screen_without_change()
            .palette()
            .background,
        Some(Color::Red)
    );
    palette.background = Some(Color::Blue);
    server.process_client_frame(
        controller,
        Frame::Control(ClientMessage::ConfigurePane {
            pane_id: 8,
            local: false,
            generation: 2,
            palette: Some(palette),
            title: Some("other pane".into()),
            cwd: None,
        }),
    );
    assert_eq!(server.panes[&8].cols, 40);
    assert_eq!(server.panes[&8].title.as_deref(), Some("other pane"));
    assert_eq!(
        server
            .panes
            .get_mut(&8)
            .unwrap()
            .screen_without_change()
            .palette()
            .background,
        Some(Color::Blue)
    );
    server.process_client_frame(
        controller,
        Frame::Control(ClientMessage::CommitLayout {
            base_rev: 0,
            layout: shared_layout_for_panes(&[(7, 2), (8, 2)]),
            live: false,
        }),
    );
    assert_eq!(server.layout_rev, 1, "metadata does not mutate a screen");
    assert!(server.media.pending());
    assert!(!server.media.client_fenced(controller));
    finish_media_events(&mut server);
}

#[test]
fn media_fences_are_per_client_and_match_the_screen_owner_and_generation() {
    let mut server = media_test_server();
    let (controller, _controller_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    server.local_panes.insert((follower, 7), test_pane(2));
    let _source = start_barrier_test_media(&mut server);
    // A stale resize does not depend on the current screen.
    server.process_client_frame(
        controller,
        Frame::Control(resize_message(7, false, 3, 60, 10)),
    );
    assert!(!server.media.client_fenced(controller));
    server.process_client_frame(
        controller,
        Frame::Control(resize_message(7, false, 2, 40, 10)),
    );
    assert!(server.media.client_fenced(controller));
    server.process_client_frame(follower, Frame::Control(resize_message(7, true, 2, 50, 10)));
    assert_eq!(server.local_panes[&(follower, 7)].cols, 50);
    assert_eq!(server.panes[&7].cols, 20);
    // An event following the blocked barrier may still update another screen.
    server.panes.insert(8, test_pane(2));
    queue_media_bytes(&server, 8, b"independent output".to_vec());
    let event = server
        .next_ready_event()
        .expect("other pane can bypass the fence");
    assert!(matches!(event, ServerEvent::Pty(None, 8, 2, _)));
    server.handle_event(event);
    finish_media_events(&mut server);
    assert_eq!(server.panes[&7].cols, 40);
    assert!(!server.media.client_fenced(controller));
}
