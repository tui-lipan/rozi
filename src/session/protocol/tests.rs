use super::*;
use crate::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use crate::runtime_metrics::ServerRuntimeMetrics;
use tui_lipan::Color;

#[test]
fn detected_blocked_elevates_over_quiescent_reports_but_not_working() {
    let detected = DetectedAgent {
        agent: AgentIdentity::new("opencode", "OpenCode").into(),
        state: DetectedAgentState::Blocked,
    };
    for value in [pane_status::IDLE, pane_status::DONE] {
        let reported = PaneStatus {
            value: value.into(),
            reason: None,
            set_at: 0,
        };
        assert_eq!(
            effective_agent_status(Some(&reported), Some(&detected)),
            Some(pane_status::BLOCKED)
        );
    }
    let working = PaneStatus {
        value: pane_status::WORKING.into(),
        reason: None,
        set_at: 0,
    };
    assert_eq!(
        effective_agent_status(Some(&working), Some(&detected)),
        Some(pane_status::WORKING)
    );
    assert_eq!(
        effective_agent_status(Some(&working), None),
        Some(pane_status::WORKING)
    );
}

#[test]
fn golden_layout_commit_json_shape() {
    let layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 120,
        canvas_rows: 40,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: Some("dev".to_string()),
            synchronized: true,
            layout: SharedLayoutKind::Master,
            start_axis: SharedSplitAxis::Vertical,
            split_ratios: vec![0.4],
            tree: Some(SharedTree::Split {
                axis: SharedSplitAxis::Vertical,
                ratio: 0.375,
                first: Box::new(SharedTree::Leaf { pane: 2 }),
                second: Box::new(SharedTree::Leaf { pane: 9 }),
            }),
            panes: vec![SharedPane {
                pane_id: 2,
                generation: 7,
                title: Some("editor".to_string()),
                profile_name: None,
                cwd: Some("/repo".to_string()),
                launch: Some(crate::pane::launch::PaneLaunch::shell("nvim")),
                replay: false,
                keep_open: false,
                floating: false,
                pinned: false,
                fullscreen: false,
                rect: None,
                scrollable_width: crate::layout::model::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    };

    assert_eq!(
        serde_json::to_value(ServerMessage::LayoutCommitted {
            rev: 4,
            author: 3,
            layout,
            live: false,
        })
        .unwrap(),
        serde_json::json!({
            "type": "layout-committed",
            "rev": 4,
            "author": 3,
            "layout": {
                "version": 4,
                "pane_geometry": {
                    "show_titles": true,
                    "titlebar": "bar",
                    "border_mode": "separate",
                    "keep_special_borders": true,
                    "padding": [0, 0, 0, 0],
                    "top_gap": 1
                },
                "canvas_cols": 120,
                "canvas_rows": 40,
                "workspaces": [{
                    "index": 0,
                    "name": "dev",
                    "synchronized": true,
                    "layout": "master",
                    "start_axis": "vertical",
                    "split_ratios": [0.4000000059604645],
                    "tree": {
                        "kind": "split",
                        "axis": "vertical",
                        "ratio": 0.375,
                        "first": {"kind": "leaf", "pane": 2},
                        "second": {"kind": "leaf", "pane": 9}
                    },
                    "panes": [{
                        "pane_id": 2,
                        "generation": 7,
                        "title": "editor",
                        "profile_name": null,
                        "cwd": "/repo",
                        "launch": {"kind": "shell", "command": "nvim"},
                        "replay": false,
                        "keep_open": false,
                        "floating": false,
                        "pinned": false,
                        "fullscreen": false,
                        "rect": null,
                        "scrollable_width": 0.44999998807907104
                    }]
                }]
            }
        })
    );
}

fn assert_frame_round_trip<T>(message: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let mut buf = Vec::new();
    write_frame(&mut buf, message).unwrap();
    let decoded: T = read_frame(&mut &buf[..]).unwrap();
    assert_eq!(&decoded, message);
}

#[test]
fn framed_messages_round_trip() {
    let rect = crate::layout::shared::FracRect {
        x: 0.125,
        y: 0.25,
        w: 0.5,
        h: 0.375,
    };
    let clients = [
        ClientMessage::DragUpdate { pane_id: 3, rect },
        ClientMessage::DragEnd,
        ClientMessage::ListDirectory {
            path: "/srv/project".into(),
            show_hidden: true,
        },
        ClientMessage::Worktree {
            request_id: 47,
            request: WorktreeRequest::Remove {
                cwd: "C:\\code\\repo".into(),
                path: "C:\\code\\repo-worktrees\\feature".into(),
                force: false,
                unlock_stale: true,
                delete_branch: crate::git::worktrees::BranchDeletion {
                    local: true,
                    remote: true,
                },
            },
        },
        ClientMessage::ReportPaneRows {
            pane_id: 3,
            local: false,
            generation: 2,
            publisher: "opencode".into(),
            kind: PublisherKind::Agent,
            rows: vec![
                PublishedRow {
                    publisher: "sessions".into(),
                    kind: PublisherKind::Agent,
                    live_session: None,
                    reported: false,
                    id: "ses_abc".into(),
                    title: "audit the widget layer".into(),
                    status: "working".into(),
                    reason: None,
                    active: true,
                    work_started_at: Some(120),
                    cwd: None,
                    project: None,
                    native_session: None,
                },
                PublishedRow {
                    publisher: "sessions".into(),
                    kind: PublisherKind::Agent,
                    live_session: None,
                    reported: false,
                    id: "ses_def".into(),
                    title: "fix the flaky test".into(),
                    status: "blocked".into(),
                    reason: Some("permission required".into()),
                    active: false,
                    work_started_at: None,
                    cwd: None,
                    project: None,
                    native_session: None,
                },
            ],
        },
        ClientMessage::RequestRuntimeMetrics,
    ];
    for message in clients {
        assert_frame_round_trip(&message);
    }

    let servers = [
        ServerMessage::DragChanged {
            author: 7,
            drag: Some(crate::layout::shared::RemoteDrag { pane_id: 3, rect }),
        },
        ServerMessage::DragChanged {
            author: 7,
            drag: None,
        },
        ServerMessage::DirectoryListing {
            path: "/srv/project".into(),
            entries: vec![
                WireDirEntry {
                    name: "src".into(),
                    is_dir: true,
                    is_symlink: false,
                    symlink_target: None,
                    ignored: false,
                    git_staged: None,
                    git_unstaged: Some(WireChangeState::Modified),
                },
                // A link rides its target along: the client cannot read it from the other host.
                WireDirEntry {
                    name: "guide-link.md".into(),
                    is_dir: false,
                    is_symlink: true,
                    symlink_target: Some("AGENTS.md".into()),
                    ignored: false,
                    git_staged: None,
                    git_unstaged: None,
                },
            ],
            error: None,
        },
        ServerMessage::ChangeListing {
            root: "/srv/project".into(),
            changes: vec![WireChange {
                path: "src/lib.rs".into(),
                state: WireChangeState::Modified,
                staged: false,
            }],
            error: None,
        },
        ServerMessage::WorktreeResult {
            request_id: 47,
            result: WorktreeResult::Removed {
                path: "C:\\code\\repo-worktrees\\feature".into(),
                branches: vec![crate::git::worktrees::BranchOutcome::Kept {
                    branch: "feature".into(),
                    copy: crate::git::worktrees::BranchCopy::Remote {
                        remote: Some("origin".into()),
                    },
                    reason: "not merged".into(),
                }],
            },
        },
        ServerMessage::RuntimeMetrics {
            metrics: Box::new(ServerRuntimeMetrics {
                sampled_at_unix_ms: 42,
                ..ServerRuntimeMetrics::default()
            }),
        },
    ];
    for message in servers {
        assert_frame_round_trip(&message);
    }
}

#[test]
fn pane_meta_from_older_peer_defaults_original_user() {
    let mut value = serde_json::to_value(PaneMeta {
        scrollback: 100,
        pane_id: 1,
        generation: 2,
        agents: Vec::new(),
        cols: 80,
        rows: 24,
        pid: Some(42),
        title: Some("user@host:~".to_string()),
        original_user: Some("user".to_string()),
        exited: None,
        logging: false,
        runtime: PaneRuntimeState::default(),
    })
    .unwrap();
    value.as_object_mut().unwrap().remove("original_user");

    let decoded: PaneMeta = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.original_user, None);
}
#[test]
fn peers_negotiate_any_generation_inside_both_windows() {
    const {
        assert!(MIN_SUPPORTED_PROTOCOL <= PROTOCOL_VERSION);
        // Generation 52 is the first with the send gate and tolerant decoding. A peer before it
        // closes the connection on a message it does not know, so it must stay outside the window.
        assert!(MIN_SUPPORTED_PROTOCOL >= 52);
    }
    assert_eq!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
        )
        .expect("same-generation peers negotiate"),
        PROTOCOL_VERSION
    );
    // A newer peer whose window still reaches back to this generation talks this generation.
    assert_eq!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            PROTOCOL_VERSION + 1,
            PROTOCOL_VERSION,
        )
        .expect("a newer peer with this generation in its window negotiates down"),
        PROTOCOL_VERSION
    );
    assert_eq!(
        negotiate_protocol(
            PROTOCOL_VERSION + 1,
            PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
        )
        .expect("a newer client talks to an older in-window server"),
        PROTOCOL_VERSION
    );
    // Out of either window: refused at the handshake, never shimmed.
    assert!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            MIN_SUPPORTED_PROTOCOL - 1,
            MIN_SUPPORTED_PROTOCOL - 1,
        )
        .is_err(),
        "a generation older than the window is rejected"
    );
    // A pre-negotiation peer sends no minimum at all; it is still just an old generation.
    assert!(
        negotiate_protocol(
            MIN_SUPPORTED_PROTOCOL - 1,
            0,
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL
        )
        .is_err(),
        "a pre-negotiation client is rejected"
    );
    assert!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            PROTOCOL_VERSION + 1,
            PROTOCOL_VERSION + 1,
        )
        .is_err(),
        "a newer-only peer is rejected"
    );
    assert!(
        negotiate_protocol(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1, 1, 1).is_err(),
        "an older-only peer is rejected"
    );
}

#[test]
fn pane_status_message_round_trips() {
    let msg = ClientMessage::SetPaneStatus {
        pane_id: 7,
        local: false,
        generation: 9,
        status: Some("blocked".into()),
        reason: Some("needs approval".into()),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        serde_json::json!({
            "type": "set-pane-status",
            "pane_id": 7,
            "local": false,
            "generation": 9,
            "status": "blocked",
            "reason": "needs approval"
        })
    );
}
/// The overwhelming majority of panes publish nothing, and must not pay for the field.
#[test]
fn a_pane_without_rows_does_not_serialize_the_key() {
    let json = serde_json::to_string(&PaneRuntimeState::default()).unwrap();
    assert!(!json.contains("rows"), "{json}");
}

#[test]
fn row_aggregation_is_by_severity_not_recency() {
    let row = |status: &str| PublishedRow {
        publisher: "sessions".into(),
        kind: crate::session::protocol::PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: status.into(),
        title: status.into(),
        status: status.into(),
        reason: None,
        active: false,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: None,
    };
    assert_eq!(aggregate_row_state(&[]), None);
    assert_eq!(
        aggregate_row_state(&[row("idle"), row("done")]),
        Some(DetectedAgentState::Idle)
    );
    assert_eq!(
        aggregate_row_state(&[row("idle"), row("working")]),
        Some(DetectedAgentState::Working),
        "one running session keeps the pane working"
    );
    assert_eq!(
        aggregate_row_state(&[row("working"), row("blocked")]),
        Some(DetectedAgentState::Blocked),
        "a prompt outranks work happening beside it"
    );
    assert_eq!(
        aggregate_row_state(&[row("idle"), row("compacting")]),
        Some(DetectedAgentState::Working),
        "a custom status is an active run, matching status_is_quiescent"
    );
}

#[test]
fn pane_runtime_status_is_optional_for_serde_compatibility() {
    let old_shape = serde_json::json!({
        "cwd": null,
        "cwd_host": null,
        "cwd_source": "unknown",
        "command_phase": {"phase": "unknown"},
        "foreground_program": null,
        "last_exit_status": null,
        "sequence": 4
    });
    let decoded: PaneRuntimeState = serde_json::from_value(old_shape).unwrap();
    assert_eq!(decoded.display_path, None);
    assert!(decoded.foreground_programs.is_empty());
    assert_eq!(decoded.status, None);
    assert_eq!(decoded.detected_agent, None);
    assert_eq!(decoded.work_started_at, None);

    let state = PaneRuntimeState {
        status: Some(PaneStatus {
            value: "working".into(),
            reason: None,
            set_at: 123,
        }),
        foreground_programs: vec!["npm".into(), "nvim".into()].into_boxed_slice(),
        work_started_at: Some(120),
        sequence: 5,
        ..PaneRuntimeState::default()
    };
    assert_eq!(
        serde_json::from_value::<PaneRuntimeState>(serde_json::to_value(&state).unwrap()).unwrap(),
        state
    );
}

#[test]
fn oversized_frame_is_rejected_before_allocation() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(9_u32).to_be_bytes());
    buf.extend_from_slice(b"{}");
    let err = read_frame_with_limit::<_, ClientMessage>(&mut &buf[..], 8).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn oversized_write_frame_is_rejected() {
    let msg = ClientMessage::SpawnPane {
        placement: None,
        pane_id: 1,
        local: false,
        generation: 1,
        launch: Some(crate::pane::launch::PaneLaunch::shell(
            "x".repeat(MAX_FRAME_SIZE),
        )),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: None,
        palette: WirePalette {
            foreground: None,
            background: None,
            ansi: [Color::Black; 16],
        },
        cell_width: 0,
        cell_height: 0,
        shell: vec!["/bin/sh".to_string()],
        command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
    };
    let err = write_frame(&mut Vec::new(), &msg).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn frame_decoder_preserves_partial_bytes_until_complete() {
    let msg = ClientMessage::Detach;
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &msg).unwrap();
    let split = 6.min(encoded.len() - 1);
    let mut decoder = FrameDecoder::default();
    assert!(matches!(
        decoder.read_from_status(&mut &encoded[..split]).unwrap(),
        FrameReadStatus::Read(_)
    ));
    assert!(decoder.next_frame::<ClientMessage>().unwrap().is_none());
    assert!(matches!(
        decoder.read_from_status(&mut &encoded[split..]).unwrap(),
        FrameReadStatus::Read(_)
    ));
    assert_eq!(
        decoder.next_frame::<ClientMessage>().unwrap(),
        Some(Frame::Control(msg))
    );
}

#[test]
fn golden_client_attach_json_shape() {
    let value = serde_json::to_value(ClientMessage::Attach {
        capabilities: None,
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "alice".into(),
        read_only: true,
        shares_filesystem: true,
        expected_server_nonce: None,
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "type":"attach",
            "session":"dev",
            "protocol_version":PROTOCOL_VERSION,
            "min_protocol_version":MIN_SUPPORTED_PROTOCOL,
            "label":"alice",
            "read_only":true,
            "shares_filesystem":true
        })
    );
    assert_eq!(
        serde_json::to_value(ClientMessage::SetSessionOrigin {
            origin: crate::session::origin::SessionOrigin {
                profile: Some("work".into()),
                ..Default::default()
            }
        })
        .unwrap(),
        serde_json::json!({"type":"set-session-origin","origin":{"profile":"work"}})
    );
}

#[test]
fn golden_query_json_shape() {
    let value = serde_json::to_value(ClientMessage::Query {
        capabilities: None,
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "type":"query",
            "session":"dev",
            "protocol_version":PROTOCOL_VERSION,
            "min_protocol_version":MIN_SUPPORTED_PROTOCOL
        })
    );
}

#[test]
fn golden_request_control_and_pong_json_shape() {
    assert_eq!(
        serde_json::to_value(ClientMessage::RequestControl).unwrap(),
        serde_json::json!({"type":"request-control"})
    );
    assert_eq!(
        serde_json::to_value(ClientMessage::Pong { seq: 5 }).unwrap(),
        serde_json::json!({"type":"pong","seq":5})
    );
}

#[test]
fn client_messages_round_trip() {
    let messages = [
        ClientMessage::Attach {
            capabilities: None,
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            label: "alice".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
        },
        ClientMessage::SetSessionOrigin {
            origin: crate::session::origin::SessionOrigin {
                profile: Some("work".into()),
                ..Default::default()
            },
        },
        ClientMessage::GrantControl { to: 7 },
        ClientMessage::DeclineControl { to: 7 },
        ClientMessage::EvictClient { target: 7 },
        ClientMessage::RequestControl,
        ClientMessage::SetControlTakeover { allowed: true },
        ClientMessage::SetInputLock { locked: true },
    ];
    for message in messages {
        assert_frame_round_trip(&message);
    }
}

#[test]
fn golden_controller_changed_and_clients_changed_json_shape() {
    assert_eq!(
        serde_json::to_value(ServerMessage::ControllerChanged {
            controller: Some(3),
            reason: ControllerChangeReason::Granted,
        })
        .unwrap(),
        serde_json::json!({"type":"controller-changed","controller":3,"reason":"granted"})
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::ClientsChanged {
            clients: vec![ClientInfo {
                id: 1,
                label: "alice".into(),
                read_only: false,
                requesting_control: true,
                parked: false,
            }],
            input_locked: true,
            allow_takeover: false,
        })
        .unwrap(),
        serde_json::json!({"type":"clients-changed","clients":[{"id":1,"label":"alice","read_only":false,"requesting_control":true,"parked":false}],"input_locked":true,"allow_takeover":false})
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Ping { seq: 9 }).unwrap(),
        serde_json::json!({"type":"ping","seq":9})
    );
}

#[test]
fn golden_session_info_json_shape() {
    assert_eq!(
        serde_json::to_value(ServerMessage::SessionInfo {
            projects: Vec::new(),
            agents: Vec::new(),
            capabilities: None,
            session: "dev".into(),
            panes: 2,
            clients: 1,
            has_layout: true,
            effective_protocol: PROTOCOL_VERSION,
            origin: crate::session::origin::SessionOrigin {
                profile: Some("work".into()),
                ..Default::default()
            },
            instance: Some(super::SessionInstanceId::for_test("abc123")),
            rozi_version: None,
        })
        .unwrap(),
        serde_json::json!({
            "type":"session-info",
            "session":"dev",
            "panes":2,
            "clients":1,
            "has_layout":true,
            "effective_protocol":PROTOCOL_VERSION,
            "origin":{"profile":"work"},
            "instance":"abc123"
        })
    );
}

#[test]
fn binary_pane_frame_has_golden_shape() {
    let mut buf = Vec::new();
    write_pane_output_frame(&mut buf, 7, 9, false, OutputMetadata::default(), b"abc").unwrap();

    let mut expected = Vec::new();
    expected.extend_from_slice(&66_u32.to_be_bytes());
    expected.push(FRAME_KIND_PANE_OUTPUT);
    expected.extend_from_slice(&7_u32.to_be_bytes());
    expected.extend_from_slice(&9_u64.to_be_bytes());
    expected.push(0);
    expected.extend_from_slice(&[0; 49]);
    expected.extend_from_slice(b"abc");

    assert_eq!(buf, expected);

    let mut local = Vec::new();
    write_pane_input_frame(&mut local, 7, 9, true, b"abc").unwrap();
    let mut expected_local = Vec::new();
    expected_local.extend_from_slice(&17_u32.to_be_bytes());
    expected_local.push(FRAME_KIND_PANE_INPUT);
    expected_local.extend_from_slice(&7_u32.to_be_bytes());
    expected_local.extend_from_slice(&9_u64.to_be_bytes());
    expected_local.push(1);
    expected_local.extend_from_slice(b"abc");
    assert_eq!(local, expected_local);
}

#[test]
fn frame_decoder_decodes_interleaved_control_and_binary_frames() {
    let attach = ClientMessage::Attach {
        capabilities: None,
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "alice".into(),
        read_only: false,
        shares_filesystem: true,
        expected_server_nonce: None,
    };
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &attach).unwrap();
    write_pane_input_frame(&mut encoded, 7, 9, false, b"abc").unwrap();

    let mut decoder = FrameDecoder::default();
    assert!(matches!(
        decoder.read_from_status(&mut &encoded[..]).unwrap(),
        FrameReadStatus::Read(_)
    ));
    assert_eq!(
        decoder.next_frame::<ClientMessage>().unwrap(),
        Some(Frame::Control(attach))
    );
    assert_eq!(
        decoder.next_frame::<ClientMessage>().unwrap(),
        Some(Frame::PaneBytes {
            latency_sensitive: true,
            pane_id: 7,
            local: false,
            generation: 9,
            bytes: b"abc".to_vec(),
        })
    );
    assert_eq!(decoder.next_frame::<ClientMessage>().unwrap(), None);
}

/// A frame of `kind` exactly as a newer peer would put it on the wire.
fn raw_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
    frame.push(kind);
    frame.extend_from_slice(payload);
    frame
}

/// A kind this build has never heard of, with and without the ignorable bit.
const FUTURE_KIND: u8 = 0x3f;

#[test]
fn an_unknown_ignorable_frame_is_skipped_by_the_incremental_decoder() {
    let mut encoded = raw_frame(FUTURE_KIND | 0x80, b"future payload");
    write_frame(&mut encoded, &ClientMessage::Detach).unwrap();
    encoded.extend(raw_frame(FUTURE_KIND | 0x80, b""));

    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut &encoded[..]).unwrap();
    assert_eq!(
        decoder.next_frame::<ClientMessage>().unwrap(),
        Some(Frame::Control(ClientMessage::Detach))
    );
    assert_eq!(decoder.next_frame::<ClientMessage>().unwrap(), None);
}

#[test]
fn an_unknown_critical_frame_stays_fatal() {
    let mut encoded = raw_frame(FUTURE_KIND, b"future payload");
    write_frame(&mut encoded, &ClientMessage::Detach).unwrap();

    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut &encoded[..]).unwrap();
    let error = decoder.next_frame::<ClientMessage>().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("unknown frame kind 63"),
        "{error}"
    );

    let error = read_frame::<_, ClientMessage>(&mut &encoded[..]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn the_blocking_reader_skips_ignorable_frames_before_the_control_frame() {
    let mut encoded = raw_frame(FUTURE_KIND | 0x80, b"future payload");
    write_frame(&mut encoded, &ClientMessage::Detach).unwrap();
    assert_eq!(
        read_frame::<_, ClientMessage>(&mut &encoded[..]).unwrap(),
        ClientMessage::Detach
    );
}

#[test]
fn golden_client_spawn_json_shape() {
    let palette = WirePalette {
        foreground: Some(Color::White),
        background: Some(Color::Black),
        ansi: [Color::Black; 16],
    };
    let value = serde_json::to_value(ClientMessage::SpawnPane {
        placement: None,
        pane_id: 7,
        local: false,
        generation: 9,
        launch: Some(crate::pane::launch::PaneLaunch::Direct {
            argv: vec!["printf".into(), "space $literal".into()],
        }),
        cwd: Some("/repo".into()),
        cols: 80,
        rows: 24,
        keep_open: true,
        env: vec![("A".into(), "B".into())],
        title: Some("shell".into()),
        palette,
        shell: vec!["/bin/zsh".to_string()],
        command_shell: vec!["/bin/sh".to_string(), "-c".to_string()],
        cell_width: 9,
        cell_height: 18,
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({"type":"spawn-pane","placement":null,"pane_id":7,"local":false,"generation":9,"launch":{"kind":"direct","argv":["printf","space $literal"]},"cwd":"/repo","cols":80,"rows":24,"keep_open":true,"env":[["A","B"]],"title":"shell","palette":serde_json::to_value(palette).unwrap(),"shell":["/bin/zsh"],"command_shell":["/bin/sh","-c"],"cell_width":9,"cell_height":18})
    );
}

#[test]
fn golden_server_messages_json_shape() {
    assert_eq!(
        serde_json::to_value(ServerMessage::Resized {
            pane_id: 1,
            local: false,
            generation: 2,
            cols: 80,
            rows: 24,
            cell_width: 14,
            cell_height: 32,
        })
        .unwrap(),
        serde_json::json!({"type":"resized","pane_id":1,"local":false,"generation":2,"cols":80,"rows":24,"cell_width":14,"cell_height":32})
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Error {
            code: "bad".into(),
            message: "no".into()
        })
        .unwrap(),
        serde_json::json!({"type":"error","code":"bad","message":"no"})
    );
}

#[test]
fn negotiate_protocol_table() {
    // (client_max, client_min, server_max, server_min, expected Ok(effective) or Err(older))
    type Case = (u32, u32, u32, u32, std::result::Result<u32, ProtocolSide>);
    let cases: &[Case] = &[
        (12, 12, 12, 12, Ok(12)),
        (13, 12, 12, 12, Ok(12)),
        (12, 12, 13, 12, Ok(12)),
        (14, 12, 13, 12, Ok(13)),
        (11, 11, 12, 12, Err(ProtocolSide::Client)),
        (13, 13, 12, 12, Err(ProtocolSide::Server)),
        (12, 0, 12, 12, Ok(12)), // missing min => exactly max
        (11, 0, 12, 12, Err(ProtocolSide::Client)),
    ];
    for &(client_max, client_min, server_max, server_min, expected) in cases {
        let result = negotiate_protocol(client_max, client_min, server_max, server_min);
        match expected {
            Ok(effective) => {
                assert_eq!(
                    result,
                    Ok(effective),
                    "client {client_min}-{client_max} vs server {server_min}-{server_max}"
                );
            }
            Err(older) => {
                let err = result.expect_err("expected mismatch");
                assert_eq!(err.older_side, older);
                let message = err.message();
                assert!(message.contains("incompatible"), "{message}");
                assert!(
                    message.contains("client") && message.contains("server"),
                    "mismatch must name both sides: {message}"
                );
                assert!(
                    message.contains(match older {
                        ProtocolSide::Client => "client is older",
                        ProtocolSide::Server => "server is older",
                    }),
                    "{message}"
                );
            }
        }
    }
}

#[test]
fn attach_without_min_protocol_deserializes_as_legacy_exact() {
    let value = serde_json::json!({
        "type": "attach",
        "session": "dev",
        "protocol_version": 12,
        "label": "alice",
        "read_only": false
    });
    let decoded: ClientMessage = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded,
        ClientMessage::Attach {
            capabilities: None,
            session: "dev".into(),
            protocol_version: 12,
            min_protocol_version: 0,
            label: "alice".into(),
            read_only: false,
            // A client that does not say cannot be assumed to be looking at this machine's files.
            shares_filesystem: false,
            expected_server_nonce: None,
        }
    );
}

#[test]
fn attached_without_effective_protocol_deserializes_as_zero() {
    let value = serde_json::json!({
        "type": "attached",
        "session_instance": "server-a",
        "protocol_version": 12,
        "session": "dev",
        "client_id": 1,
        "panes": [],
        "layout_rev": 0,
        "layout": null,
        "controller": null,
        "clients": [],
        "origin": {},
        "input_locked": false
    });
    let decoded: ServerMessage = serde_json::from_value(value).unwrap();
    let ServerMessage::Attached {
        effective_protocol, ..
    } = decoded
    else {
        panic!("expected Attached");
    };
    assert_eq!(effective_protocol, 0);
}

#[test]
fn session_control_messages_round_trip_with_the_documented_wire_shape() {
    let request = crate::control::ControlRequest {
        command: crate::control::ControlCommand::CapturePane {
            target: Some(3),
            scrollback: Some(crate::control::CaptureScrollback::Named(
                crate::control::CaptureScrollbackNamed::Full,
            )),
            render: crate::control::CaptureRender::Text,
            scale: None,
            wait: None,
            image_pixels: false,
        },
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    };
    let msg = ClientMessage::SessionControl {
        capabilities: None,
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        request,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        serde_json::json!({
            "type": "session-control",
            "session": "dev",
            "protocol_version": PROTOCOL_VERSION,
            "min_protocol_version": MIN_SUPPORTED_PROTOCOL,
            // The request is the UI endpoint's own document, flattened exactly as that endpoint
            // receives it, so `docs/control-protocol.md` describes one request vocabulary rather
            // than two spellings of it.
            "request": {
                "cmd": "capture-pane",
                "target": 3,
                "scrollback": "full",
                "render": "text",
                "source_pane": null
            }
        })
    );

    let reply = ServerMessage::SessionControlResult {
        capabilities: Some(Capabilities::current()),
        effective_protocol: PROTOCOL_VERSION,
        response: crate::control::ControlResponse::ok(serde_json::json!({"id": 3})),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &reply).unwrap();
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
        reply
    );
    let encoded = serde_json::to_value(&reply).unwrap();
    assert_eq!(encoded["type"], "session-control-result");
    assert_eq!(
        encoded["response"],
        serde_json::json!({"ok": true, "data": {"id": 3}})
    );
}

#[test]
fn attached_control_messages_round_trip_with_the_documented_wire_shape() {
    let msg = ClientMessage::AttachedControl {
        request_id: 12,
        request: crate::control::ControlRequest {
            command: crate::control::ControlCommand::RecordList,
            source_pane: None,
            source_session: None,
            extension: None,
            caller_context: None,
            credential: None,
        },
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        serde_json::json!({
            "type": "attached-control",
            "request_id": 12,
            "request": {"cmd": "record-list", "source_pane": null}
        })
    );

    let reply = ServerMessage::AttachedControlResult {
        request_id: 12,
        response: crate::control::ControlResponse::ok(serde_json::json!([])),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &reply).unwrap();
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
        reply
    );
    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        serde_json::json!({
            "type": "attached-control-result",
            "request_id": 12,
            "response": {"ok": true, "data": []}
        })
    );
}

/// A refusal is the whole answer, so it has to survive the trip back with its `ok: false` and its
/// sentence intact rather than collapsing into an absent field.
#[test]
fn a_refused_control_response_round_trips_through_the_session_wire() {
    let reply = ServerMessage::SessionControlResult {
        capabilities: None,
        effective_protocol: PROTOCOL_VERSION,
        response: crate::control::ControlResponse::error("pane 9 not found"),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &reply).unwrap();
    let ServerMessage::SessionControlResult { response, .. } =
        read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap()
    else {
        panic!("expected a control result");
    };
    assert!(!response.ok);
    assert_eq!(response.error.as_deref(), Some("pane 9 not found"));
    assert!(response.data.is_none());
}

#[test]
fn semantic_output_and_presentation_frames_round_trip_at_every_wire_split() {
    let metadata = OutputMetadata {
        sequence: u64::MAX - 1,
        coalesce_epoch: 17,
        class: OutputClass::IndependentGraphics {
            image: 7,
            incarnation: 19,
            revision: 23,
        },
    };
    let acks = vec![PresentationAck {
        local: false,
        pane_id: 7,
        generation: 11,
        sequence: 29,
    }];
    let interest = vec![PanePresentationInterest {
        local: false,
        pane_id: 7,
        generation: 11,
        visible: true,
        focused: true,
        frames_per_second: 240,
    }];
    let mut wire = Vec::new();
    write_pane_output_frame(&mut wire, 7, 11, false, metadata, b"original bytes").unwrap();
    write_presentation_acks(&mut wire, &acks).unwrap();
    write_presentation_interest(&mut wire, &interest).unwrap();
    for split in 0..=wire.len() {
        let mut decoder = FrameDecoder::default();
        decoder.read_from_status(&mut &wire[..split]).unwrap();
        let mut frames = Vec::new();
        while let Some(frame) = decoder.next_frame::<serde_json::Value>().unwrap() {
            frames.push(frame);
        }
        decoder.read_from_status(&mut &wire[split..]).unwrap();
        while let Some(frame) = decoder.next_frame::<serde_json::Value>().unwrap() {
            frames.push(frame);
        }
        assert_eq!(
            frames,
            vec![
                Frame::PaneOutput {
                    media: None,
                    pane_id: 7,
                    local: false,
                    generation: 11,
                    metadata,
                    bytes: b"original bytes".to_vec(),
                },
                Frame::PresentationAcks(acks.clone()),
                Frame::PresentationInterest(interest.clone())
            ],
            "split {split}"
        );
    }
}

#[test]
fn invalid_graphics_classification_and_interest_flags_are_rejected() {
    let mut wire = Vec::new();
    write_pane_output_frame(&mut wire, 7, 11, false, OutputMetadata::default(), b"data").unwrap();
    wire[4 + 1 + 13 + 16] = 3;
    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut wire.as_slice()).unwrap();
    assert!(decoder.next_frame::<serde_json::Value>().is_err());
    let mut wire = Vec::new();
    write_presentation_interest(
        &mut wire,
        &[PanePresentationInterest {
            local: false,
            pane_id: 7,
            generation: 11,
            visible: true,
            focused: false,
            frames_per_second: 60,
        }],
    )
    .unwrap();
    wire[4 + 1 + 12] = 8;
    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut wire.as_slice()).unwrap();
    assert!(decoder.next_frame::<serde_json::Value>().is_err());
}

#[test]
fn continuous_input_priority_round_trips_without_changing_bytes() {
    let mut bytes = Vec::new();
    write_pane_input_with_priority(&mut bytes, 7, 11, true, false, b"pointer bytes").unwrap();
    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut bytes.as_slice()).unwrap();
    assert_eq!(
        decoder.next_frame::<ClientMessage>().unwrap(),
        Some(Frame::PaneBytes {
            pane_id: 7,
            generation: 11,
            local: true,
            latency_sensitive: false,
            bytes: b"pointer bytes".to_vec(),
        })
    );
}

#[test]
fn attachment_steps_and_acknowledgements_round_trip_through_framing() {
    for operation in [
        AttachmentOperation::Begin {
            pane_id: 7,
            generation: 9,
            local: true,
            name: "photo.png".into(),
            size: 4,
        },
        AttachmentOperation::Chunk {
            data: "AAECAw==".into(),
        },
        AttachmentOperation::Finish,
        AttachmentOperation::Cancel,
    ] {
        let message = ClientMessage::Attachment {
            transfer_id: 42,
            operation,
        };
        let mut bytes = vec![];
        write_frame(&mut bytes, &message).unwrap();
        let decoded: ClientMessage = read_frame(&mut std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoded, message);
    }
    let ack = ServerMessage::AttachmentAck {
        transfer_id: 42,
        error: Some("read-only client".into()),
    };
    let mut bytes = vec![];
    write_frame(&mut bytes, &ack).unwrap();
    let decoded: ServerMessage = read_frame(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(decoded, ack);
}

#[test]
fn immutable_media_and_acquisition_acks_round_trip() {
    let media = OutputMedia {
        acquisition_sequence: 42,
        files: vec![ImmutableFileBinding {
            command_range: 1..8,
            path: std::path::PathBuf::from("immutable-frame"),
            byte_len: 4,
            format: 32,
        }],
        acquired: Vec::new(),
    };
    let mut wire = Vec::new();
    write_pane_output_media_frame(
        &mut wire,
        7,
        2,
        false,
        OutputMetadata::default(),
        Some(&media),
        b"123456789",
    )
    .unwrap();
    let acks = vec![AcquisitionAck {
        local: false,
        pane_id: 7,
        generation: 2,
        sequence: 42,
    }];
    write_acquisition_acks(&mut wire, &acks).unwrap();
    let mut decoder = FrameDecoder::default();
    decoder.read_from_status(&mut &wire[..]).unwrap();
    assert_eq!(
        decoder.next_frame::<serde_json::Value>().unwrap(),
        Some(Frame::PaneOutput {
            pane_id: 7,
            local: false,
            generation: 2,
            metadata: OutputMetadata::default(),
            media: Some(media),
            bytes: b"123456789".to_vec(),
        })
    );
    assert_eq!(
        decoder.next_frame::<serde_json::Value>().unwrap(),
        Some(Frame::AcquisitionAcks(acks))
    );
}

#[test]
fn a_message_type_from_a_newer_peer_decodes_as_unknown_instead_of_failing() {
    let request: ClientMessage =
        serde_json::from_str(r#"{"type":"teleport-pane","pane_id":3,"to":"mars"}"#).unwrap();
    assert_eq!(request, ClientMessage::Unknown);
    let event: ServerMessage =
        serde_json::from_str(r#"{"type":"pane-teleported","pane_id":3}"#).unwrap();
    assert_eq!(event, ServerMessage::Unknown);

    // Through real framing too: the frame decodes and the next one still arrives.
    let mut buf = Vec::new();
    write_frame(
        &mut buf,
        &serde_json::json!({"type": "pane-teleported", "pane_id": 3}),
    )
    .unwrap();
    write_frame(&mut buf, &ServerMessage::Ping { seq: 4 }).unwrap();
    let mut reader = &buf[..];
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut reader).unwrap(),
        ServerMessage::Unknown
    );
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut reader).unwrap(),
        ServerMessage::Ping { seq: 4 }
    );
}

#[test]
fn generation_baseline_messages_need_no_capability() {
    assert_eq!(ClientMessage::Detach.required_capability(), None);
    assert_eq!(
        ClientMessage::ShutdownForRestart.required_capability(),
        None
    );
    assert_eq!(ServerMessage::Ping { seq: 1 }.required_capability(), None);
    assert_eq!(ServerMessage::ControlDeclined.required_capability(), None);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "never sent")]
fn asking_what_unknown_needs_is_a_bug() {
    let _ = ServerMessage::Unknown.required_capability();
}

/// Re-encode `value` with the JSON at `field` replaced by a value from a newer peer, and decode it.
fn decode_with<T>(value: &T, field: &str, newer: serde_json::Value) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut json = serde_json::to_value(value).unwrap();
    json[field] = newer;
    serde_json::from_value(json).expect("a newer value still decodes")
}

/// A value a newer peer added to a closed vocabulary decodes instead of failing the whole message,
/// which would drop the connection.
#[test]
fn a_vocabulary_value_from_a_newer_peer_decodes_as_unknown() {
    let newer = || serde_json::json!("teleporting");
    assert_eq!(
        serde_json::from_value::<WireChangeState>(newer()).unwrap(),
        WireChangeState::Unknown
    );
    assert_eq!(
        serde_json::from_value::<ControllerChangeReason>(newer()).unwrap(),
        ControllerChangeReason::Unknown
    );
    assert_eq!(
        serde_json::from_value::<PaneCwdSource>(newer()).unwrap(),
        PaneCwdSource::Unknown
    );
    assert_eq!(
        serde_json::from_value::<EventKind>(newer()).unwrap(),
        EventKind::Unknown
    );
    assert_eq!(
        serde_json::from_str::<PaneCommandPhase>(r#"{"phase":"suspended","since":3}"#).unwrap(),
        PaneCommandPhase::Unknown
    );
    assert_eq!(
        serde_json::from_str::<WorktreeRequest>(r#"{"kind":"rebase","cwd":"/src"}"#).unwrap(),
        WorktreeRequest::Unknown
    );
    assert_eq!(
        serde_json::from_str::<WorktreeResult>(r#"{"kind":"rebased","path":"/src"}"#).unwrap(),
        WorktreeResult::Unknown
    );
    assert_eq!(
        serde_json::from_str::<AttachmentOperation>(r#"{"step":"resume","offset":9}"#).unwrap(),
        AttachmentOperation::Unknown
    );

    // Inside a whole message, which is what actually crosses the wire.
    let changed = ServerMessage::ControllerChanged {
        controller: Some(2),
        reason: ControllerChangeReason::Granted,
    };
    assert_eq!(
        decode_with(&changed, "reason", newer()),
        ServerMessage::ControllerChanged {
            controller: Some(2),
            reason: ControllerChangeReason::Unknown,
        }
    );
}

/// Vocabularies that are also part of the public control API, or that the server only ever
/// produces itself, stay closed; the session wire fields carrying them decode a newer value as
/// the type's neutral reading instead.
#[test]
fn a_closed_vocabulary_on_the_wire_reads_a_newer_value_neutrally() {
    let identity = AgentIdentity::new("claude", "Claude Code");
    let runtime = AgentRuntime {
        reference: AgentRef::for_test(1, 0, None),
        identity: identity.clone(),
        label: "Claude Code".into(),
        state: AgentState::Idle,
        reason: None,
        task: None,
        completions: 0,
        source: AgentAuthority::Reported,
    };
    let newer = decode_with(&runtime, "state", serde_json::json!("compacting"));
    // A state Rozi has no word for is a run still going, as a custom status is.
    assert_eq!(newer.state, AgentState::Working);
    let newer = decode_with(&runtime, "source", serde_json::json!("inferred"));
    assert_eq!(newer.source, AgentAuthority::Detected);
    // A value it knows still decodes as itself.
    assert_eq!(
        decode_with(&runtime, "state", serde_json::json!("blocked")).state,
        AgentState::Blocked
    );

    let detected = DetectedAgent {
        agent: std::sync::Arc::new(identity),
        state: DetectedAgentState::Idle,
    };
    assert_eq!(
        decode_with(&detected, "state", serde_json::json!("compacting")).state,
        DetectedAgentState::Working
    );

    let row: PublishedRow = serde_json::from_value(serde_json::json!({
        "id": "build",
        "kind": "daemon",
        "title": "Build",
        "status": "working",
    }))
    .unwrap();
    assert_eq!(row.kind, PublisherKind::Activity);
}

/// The values a peer can receive but never sends stay out of everything this build offers or
/// lists, so it cannot pick one by accident.
#[test]
fn unknown_vocabulary_values_are_never_offered() {
    assert!(!EventKind::ALL.contains(&EventKind::Unknown));
    assert_eq!(EventKind::parse("unknown"), None);
}
