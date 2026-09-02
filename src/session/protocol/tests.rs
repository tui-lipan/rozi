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
                fullscreen: false,
                rect: None,
                scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    };

    assert_eq!(
        serde_json::to_value(ServerMessage::LayoutCommitted {
            rev: 4,
            author: 3,
            layout,
        })
        .unwrap(),
        serde_json::json!({
            "type": "layout-committed",
            "rev": 4,
            "author": 3,
            "layout": {
                "version": 3,
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
                        "fullscreen": false,
                        "rect": null,
                        "scrollable_width": 0.44999998807907104
                    }]
                }]
            }
        })
    );
}

#[test]
fn protocol_frame_round_trips() {
    let msg = ClientMessage::Attach {
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "alice".into(),
        read_only: false,
        shares_filesystem: true,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn pane_meta_from_older_peer_defaults_original_user() {
    let mut value = serde_json::to_value(PaneMeta {
        pane_id: 1,
        generation: 2,
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
fn session_origin_shape_round_trips() {
    let msg = ClientMessage::SetSessionOrigin {
        profile: "work".into(),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    let decoded: ClientMessage = read_frame(&mut &buf[..]).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn file_tree_messages_round_trip() {
    let request = ClientMessage::ListDirectory {
        path: "/srv/project".into(),
        show_hidden: true,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &request).unwrap();
    assert_eq!(
        read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(),
        request
    );

    let listing = ServerMessage::DirectoryListing {
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
                name: "CLAUDE.md".into(),
                is_dir: false,
                is_symlink: true,
                symlink_target: Some("AGENTS.md".into()),
                ignored: false,
                git_staged: None,
                git_unstaged: None,
            },
        ],
        error: None,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &listing).unwrap();
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
        listing
    );

    let changes = ServerMessage::ChangeListing {
        root: "/srv/project".into(),
        changes: vec![WireChange {
            path: "src/lib.rs".into(),
            state: WireChangeState::Modified,
            staged: false,
        }],
        error: None,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &changes).unwrap();
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut &buf[..]).unwrap(),
        changes
    );
}

#[test]
fn only_the_exact_version_negotiates() {
    const { assert!(MIN_SUPPORTED_PROTOCOL == PROTOCOL_VERSION) };

    assert_eq!(
        negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
        )
        .expect("same-version peers negotiate"),
        PROTOCOL_VERSION
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

    let request = ClientMessage::RequestRuntimeMetrics;
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &request).unwrap();
    assert_eq!(
        read_frame::<_, ClientMessage>(&mut &bytes[..]).unwrap(),
        request
    );

    let sample = ServerMessage::RuntimeMetrics {
        metrics: ServerRuntimeMetrics {
            sampled_at_unix_ms: 42,
            ..ServerRuntimeMetrics::default()
        },
    };
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &sample).unwrap();
    assert_eq!(
        read_frame::<_, ServerMessage>(&mut &bytes[..]).unwrap(),
        sample
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

#[test]
fn pane_rows_message_round_trips() {
    let msg = ClientMessage::ReportPaneRows {
        pane_id: 3,
        local: false,
        generation: 2,
        rows: vec![
            PublishedRow {
                id: "ses_abc".into(),
                title: "audit the widget layer".into(),
                status: "working".into(),
                reason: None,
                active: true,
                work_started_at: Some(120),
            },
            PublishedRow {
                id: "ses_def".into(),
                title: "fix the flaky test".into(),
                status: "blocked".into(),
                reason: Some("permission required".into()),
                active: false,
                work_started_at: None,
            },
        ],
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(read_frame::<_, ClientMessage>(&mut &buf[..]).unwrap(), msg);
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
        id: status.into(),
        title: status.into(),
        status: status.into(),
        reason: None,
        active: false,
        work_started_at: None,
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
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "alice".into(),
        read_only: true,
        shares_filesystem: true,
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
            profile: "work".into()
        })
        .unwrap(),
        serde_json::json!({"type":"set-session-origin","profile":"work"})
    );
}

#[test]
fn golden_query_json_shape() {
    let value = serde_json::to_value(ClientMessage::Query {
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
fn grant_control_and_input_lock_round_trip() {
    for message in [
        ClientMessage::GrantControl { to: 7 },
        ClientMessage::DeclineControl { to: 7 },
        ClientMessage::EvictClient { target: 7 },
        ClientMessage::RequestControl,
        ClientMessage::SetControlTakeover { allowed: true },
        ClientMessage::SetInputLock { locked: true },
    ] {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &message).unwrap();
        assert_eq!(
            read_frame::<_, ClientMessage>(&mut &bytes[..]).unwrap(),
            message
        );
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
            session: "dev".into(),
            panes: 2,
            clients: 1,
            has_layout: true,
            effective_protocol: PROTOCOL_VERSION,
            created_from_profile: Some("work".into()),
        })
        .unwrap(),
        serde_json::json!({
            "type":"session-info",
            "session":"dev",
            "panes":2,
            "clients":1,
            "has_layout":true,
            "effective_protocol":PROTOCOL_VERSION,
            "created_from_profile":"work"
        })
    );
}

#[test]
fn binary_pane_frame_has_golden_shape() {
    let mut buf = Vec::new();
    write_pane_output_frame(&mut buf, 7, 9, false, b"abc").unwrap();

    let mut expected = Vec::new();
    expected.extend_from_slice(&17_u32.to_be_bytes());
    expected.push(FRAME_KIND_PANE_OUTPUT);
    expected.extend_from_slice(&7_u32.to_be_bytes());
    expected.extend_from_slice(&9_u64.to_be_bytes());
    expected.push(0);
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
        session: "dev".into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: "alice".into(),
        read_only: false,
        shares_filesystem: true,
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
            pane_id: 7,
            local: false,
            generation: 9,
            bytes: b"abc".to_vec(),
        })
    );
    assert_eq!(decoder.next_frame::<ClientMessage>().unwrap(), None);
}

#[test]
fn golden_client_spawn_json_shape() {
    let palette = WirePalette {
        foreground: Some(Color::White),
        background: Some(Color::Black),
        ansi: [Color::Black; 16],
    };
    let value = serde_json::to_value(ClientMessage::SpawnPane {
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
        serde_json::json!({"type":"spawn-pane","pane_id":7,"local":false,"generation":9,"launch":{"kind":"direct","argv":["printf","space $literal"]},"cwd":"/repo","cols":80,"rows":24,"keep_open":true,"env":[["A","B"]],"title":"shell","palette":serde_json::to_value(palette).unwrap(),"shell":["/bin/zsh"],"command_shell":["/bin/sh","-c"],"cell_width":9,"cell_height":18})
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
        })
        .unwrap(),
        serde_json::json!({"type":"resized","pane_id":1,"local":false,"generation":2,"cols":80,"rows":24})
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
            session: "dev".into(),
            protocol_version: 12,
            min_protocol_version: 0,
            label: "alice".into(),
            read_only: false,
            // A client that does not say cannot be assumed to be looking at this machine's files.
            shares_filesystem: false,
        }
    );
}

#[test]
fn attached_without_effective_protocol_deserializes_as_zero() {
    let value = serde_json::json!({
        "type": "attached",
        "protocol_version": 12,
        "session": "dev",
        "client_id": 1,
        "panes": [],
        "layout_rev": 0,
        "layout": null,
        "controller": null,
        "clients": [],
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
