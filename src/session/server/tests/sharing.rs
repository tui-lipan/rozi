use super::*;
use crate::control::{
    ControlCommand, ControlErrorCode, ControlRequest, ControlResponse, ShareState,
};
use crate::platform::user::{UserAccount, UserId, current_user_id};
use crate::session::protocol::{
    Capabilities, ClientInfo, ClientMessage, EVICTED_ERROR_CODE, Frame, NOT_SHARED_ERROR_CODE,
    PERMISSION_DENIED_ERROR_CODE, PROTOCOL_VERSION, ServerMessage, SessionRole, WorktreeRequest,
    WorktreeResult,
};
use std::io;
use std::os::unix::net::UnixStream;
use tempfile::TempDir;

fn bob_uid() -> UserId {
    UserId::from_raw(current_user_id().as_raw().wrapping_add(1).max(1001))
}

fn charlie_uid() -> UserId {
    UserId::from_raw(current_user_id().as_raw().wrapping_add(2).max(1002))
}

fn test_lookup<'a>(
    users: &'a [(UserId, &'a str)],
) -> impl Fn(&str) -> io::Result<Option<UserAccount>> + 'a {
    move |spec| {
        let parsed = spec.parse::<u32>().ok().map(UserId::from_raw);
        Ok(users
            .iter()
            .find(|(uid, name)| *name == spec || parsed == Some(*uid))
            .map(|(uid, name)| UserAccount {
                id: *uid,
                name: (*name).to_string(),
            }))
    }
}

fn shared_test_server(name: &str) -> (SessionServer, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut server = SessionServer::new_named(name);
    server.shares.set_test_shared_dir(dir.path().to_path_buf());
    (server, dir)
}

fn grant_user(
    server: &mut SessionServer,
    uid: UserId,
    name: &str,
    role: SessionRole,
) -> ControlResponse {
    let users = [(uid, name)];
    let mut broadcasts = Vec::new();
    let response = server.share_grant_with(name, role, test_lookup(&users), &mut broadcasts);
    for (target, msg) in broadcasts {
        server.enqueue(0, target, msg);
    }
    response
}

fn add_guest_client(server: &mut SessionServer, uid: UserId) -> (ClientId, UnixStream) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    server_stream.set_nonblocking(true).unwrap();
    let id = server.add_client_for_peer(IpcConnection::from_unix(server_stream), uid);
    (id, client_stream)
}

fn send_frame(server: &mut SessionServer, client_id: ClientId, message: ClientMessage) {
    server.process_client_frame(client_id, Frame::Control(message));
}

fn attach_guest_msg(session: &str, label: &str, read_only: bool) -> ClientMessage {
    ClientMessage::Attach {
        capabilities: Some(Capabilities::current()),
        session: session.to_string(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_VERSION,
        label: label.to_string(),
        read_only,
        shares_filesystem: true,
        expected_server_nonce: None,
        handoff_token: None,
    }
}

fn control_req(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    }
}

fn outbox_controls(server: &SessionServer, id: ClientId) -> Vec<ServerMessage> {
    decode_outbox_controls(server.client(id).expect("registered client"))
}

fn attach_guest(
    server: &mut SessionServer,
    uid: UserId,
    session_target: &str,
    read_only: bool,
) -> (ClientId, UnixStream) {
    let (id, stream) = add_guest_client(server, uid);
    send_frame(
        server,
        id,
        attach_guest_msg(session_target, "spoofed-label", read_only),
    );
    assert!(matches!(
        attached_reply(server, id),
        ServerMessage::Attached { .. }
    ));
    server.pump_attach_seeds();
    server.flush_session_events();
    (id, stream)
}

#[test]
fn ungranted_peer_is_refused_with_not_shared_and_learns_nothing() {
    let (mut server, _dir) = shared_test_server("dev");
    server.panes.insert(1, test_pane(1));
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);

    // Charlie has no grant and probes Attach, Query, and SessionControl.
    for msg in [
        attach_guest_msg("alice/dev", "charlie", false),
        ClientMessage::Query {
            capabilities: Some(Capabilities::current()),
            session: "alice/dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
        },
        ClientMessage::SessionControl {
            capabilities: Some(Capabilities::current()),
            session: "alice/dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            request: control_req(ControlCommand::ListPanes),
        },
        ClientMessage::Shutdown,
    ] {
        let (charlie, _stream) = add_guest_client(&mut server, charlie_uid());
        send_frame(&mut server, charlie, msg);
        let controls = outbox_controls(&server, charlie);
        assert_eq!(controls.len(), 1, "{controls:?}");
        assert!(matches!(
            &controls[0],
            ServerMessage::Error { code, .. } if code == NOT_SHARED_ERROR_CODE
        ));
        assert!(!server.client_attached(charlie));
        assert!(server.client(charlie).unwrap().close_after_flush);
        assert!(!server.shutdown);
    }
}

#[test]
fn guest_attach_clamps_role_forces_local_identity_and_rejects_path_traversal() {
    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);

    // Path-traversal session strings must be rejected even for a granted guest.
    for bad in ["../dev", "alice/../dev", "a/b/c", "/dev", "alice/other"] {
        let (id, _stream) = add_guest_client(&mut server, bob_uid());
        send_frame(&mut server, id, attach_guest_msg(bad, "bob", false));
        let controls = outbox_controls(&server, id);
        assert!(
            matches!(
                controls.as_slice(),
                [ServerMessage::Error { code, .. }] if code == "session-mismatch"
            ),
            "bad target {bad:?}: {controls:?}"
        );
        assert!(!server.client_attached(id));
    }

    let (bob, _stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    let ServerMessage::Attached {
        session, clients, ..
    } = attached_reply(&server, bob)
    else {
        unreachable!()
    };
    assert_eq!(session, "alice/dev");
    assert_eq!(
        clients,
        vec![ClientInfo {
            id: bob,
            label: "bob".to_string(),
            read_only: false,
            requesting_control: false,
            parked: false,
            role: Some(SessionRole::Editor),
            user: Some("bob".to_string()),
        }]
    );
    let conn = server.client(bob).unwrap();
    assert!(!conn.shares_filesystem);
    assert_eq!(conn.label.as_deref(), Some("bob"));
    assert_eq!(conn.access.role, SessionRole::Editor);
    assert!(conn.access.is_guest());

    // Asking for read_only clamps even a Controller grant down to Viewer.
    grant_user(
        &mut server,
        charlie_uid(),
        "charlie",
        SessionRole::Controller,
    );
    let (charlie, _c_stream) = attach_guest(&mut server, charlie_uid(), "dev", true);
    let charlie_conn = server.client(charlie).unwrap();
    assert_eq!(charlie_conn.access.role, SessionRole::Viewer);
    assert!(server.client_read_only(charlie));
}

#[test]
fn viewer_cannot_type_and_legacy_read_only_clients_still_drop_silently() {
    let (mut server, _dir) = shared_test_server("dev");
    server.panes.insert(1, test_pane(2));
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);

    let (viewer, _v_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    clear_outboxes(&mut server, &[viewer]);

    // Raw pane bytes are ignored for Viewer.
    server.process_client_frame(
        viewer,
        Frame::PaneBytes {
            latency_sensitive: true,
            pane_id: 1,
            local: false,
            generation: 2,
            bytes: b"rm -rf /\n".to_vec(),
        },
    );
    assert!(!server.client_may_input(viewer));

    // Control messages needing Input/Admin get permission-denied when SessionSharing was negotiated.
    send_frame(
        &mut server,
        viewer,
        ClientMessage::SetPaneStatus {
            pane_id: 1,
            local: false,
            generation: 2,
            status: Some("spoof".into()),
            reason: None,
        },
    );
    assert!(matches!(
        outbox_controls(&server, viewer).as_slice(),
        [ServerMessage::Error { code, .. }] if code == PERMISSION_DENIED_ERROR_CODE
    ));
    assert!(server.panes[&1].runtime.status.is_none());

    // A pre-SessionSharing read-only owner client gets silent drops (no permission-denied error).
    let (legacy_ro, _l_stream) = attach_read_only_client(&mut server);
    server.pump_attach_seeds();
    clear_outboxes(&mut server, &[legacy_ro]);
    send_frame(&mut server, legacy_ro, ClientMessage::Shutdown);
    assert!(outbox_controls(&server, legacy_ro).is_empty());
    assert!(!server.shutdown);
}

#[test]
fn editor_can_type_but_cannot_take_lease_and_controller_can() {
    let (mut server, _dir) = shared_test_server("dev");
    server.panes.insert(1, test_pane(2));
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    grant_user(
        &mut server,
        charlie_uid(),
        "charlie",
        SessionRole::Controller,
    );

    // Editor attaches first to an uncontrolled session: must NOT be auto-granted the lease.
    let (editor, _e_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    assert_eq!(server.controller, None);
    assert!(server.client_may_input(editor));

    // Editor can set pane status (Input) through the real gate.
    clear_outboxes(&mut server, &[editor]);
    send_frame(
        &mut server,
        editor,
        ClientMessage::SetPaneStatus {
            pane_id: 1,
            local: false,
            generation: 2,
            status: Some("working".into()),
            reason: None,
        },
    );
    assert_eq!(
        server.panes[&1]
            .runtime
            .status
            .as_ref()
            .map(|s| s.value.as_str()),
        Some("working")
    );

    // Typing does not extend to writing the owner's files: an editor cannot turn on pane logging.
    clear_outboxes(&mut server, &[editor]);
    send_frame(
        &mut server,
        editor,
        ClientMessage::SetPaneLogging {
            pane_id: 1,
            local: false,
            generation: 2,
            enabled: true,
        },
    );
    assert!(matches!(
        outbox_controls(&server, editor).as_slice(),
        [ServerMessage::PaneLoggingChanged {
            enabled: false,
            error: Some(_),
            ..
        }]
    ));

    // Editor requesting the lease is refused with permission-denied.
    clear_outboxes(&mut server, &[editor]);
    send_frame(&mut server, editor, ClientMessage::RequestControl);
    assert_eq!(server.controller, None);
    assert!(matches!(
        outbox_controls(&server, editor).as_slice(),
        [ServerMessage::Error { code, .. }] if code == PERMISSION_DENIED_ERROR_CODE
    ));

    // Controller attaches: auto-granted the lease since session was uncontrolled.
    let (ctrl, _c_stream) = attach_guest(&mut server, charlie_uid(), "alice/dev", false);
    assert_eq!(server.controller, Some(ctrl));

    // Controller cannot grant the lease to an Editor.
    send_frame(
        &mut server,
        ctrl,
        ClientMessage::GrantControl { to: editor },
    );
    assert_eq!(server.controller, Some(ctrl));
}

#[test]
fn guest_cannot_shutdown_rename_browse_files_or_manage_shares_even_as_controller() {
    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Controller);
    let (ctrl, _stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    assert_eq!(server.controller, Some(ctrl));
    clear_outboxes(&mut server, &[ctrl]);

    // Shutdown / ShutdownForRestart / Rename -> permission-denied, no side effects.
    for msg in [
        ClientMessage::Shutdown,
        ClientMessage::ShutdownForRestart,
        ClientMessage::Rename {
            name: "pwned".into(),
        },
    ] {
        clear_outboxes(&mut server, &[ctrl]);
        send_frame(&mut server, ctrl, msg);
        assert!(matches!(
            outbox_controls(&server, ctrl).as_slice(),
            [ServerMessage::Error { code, .. }] if code == PERMISSION_DENIED_ERROR_CODE
        ));
        assert!(!server.shutdown);
        assert_eq!(server.session_name, "dev");
    }

    // HostFiles messages return their typed error replies.
    clear_outboxes(&mut server, &[ctrl]);
    send_frame(
        &mut server,
        ctrl,
        ClientMessage::ListDirectory {
            path: "/".into(),
            show_hidden: false,
        },
    );
    assert!(matches!(
        outbox_controls(&server, ctrl).as_slice(),
        [ServerMessage::DirectoryListing { error: Some(_), .. }]
    ));

    clear_outboxes(&mut server, &[ctrl]);
    send_frame(
        &mut server,
        ctrl,
        ClientMessage::ListChanges { root: "/".into() },
    );
    assert!(matches!(
        outbox_controls(&server, ctrl).as_slice(),
        [ServerMessage::ChangeListing { error: Some(_), .. }]
    ));

    clear_outboxes(&mut server, &[ctrl]);
    send_frame(
        &mut server,
        ctrl,
        ClientMessage::Worktree {
            request_id: 99,
            request: WorktreeRequest::List { cwd: "/".into() },
        },
    );
    assert!(matches!(
        outbox_controls(&server, ctrl).as_slice(),
        [ServerMessage::WorktreeResult {
            request_id: 99,
            result: WorktreeResult::Failed { .. }
        }]
    ));
    assert!(server.browse_in_flight.is_empty());

    // AttachedControl Admin commands are refused with NotPermitted.
    clear_outboxes(&mut server, &[ctrl]);
    send_frame(
        &mut server,
        ctrl,
        ClientMessage::AttachedControl {
            request_id: 7,
            request: control_req(ControlCommand::RecordStart {
                target: None,
                output: None,
                max_fps: None,
                duration_ms: None,
                max_bytes: None,
                force: false,
                follow: false,
            }),
        },
    );
    assert!(matches!(
        outbox_controls(&server, ctrl).as_slice(),
        [ServerMessage::AttachedControlResult {
            request_id: 7,
            response: ControlResponse {
                ok: false,
                code: Some(ControlErrorCode::NotPermitted),
                ..
            }
        }]
    ));

    // Headless SessionControl share management / Admin commands are refused with NotPermitted.
    for cmd in [
        ControlCommand::ShareGrant {
            user: "charlie".into(),
            role: SessionRole::Viewer,
        },
        ControlCommand::ShareRevoke { user: None },
        ControlCommand::ShareList,
        ControlCommand::PaneLogging {
            target: None,
            enabled: Some(true),
        },
    ] {
        let (headless_bob, _h_stream) = add_guest_client(&mut server, bob_uid());
        send_frame(
            &mut server,
            headless_bob,
            ClientMessage::SessionControl {
                capabilities: Some(Capabilities::current()),
                session: "alice/dev".into(),
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: PROTOCOL_VERSION,
                request: control_req(cmd),
            },
        );
        assert!(matches!(
            outbox_controls(&server, headless_bob).as_slice(),
            [ServerMessage::SessionControlResult {
                response: ControlResponse {
                    ok: false,
                    code: Some(ControlErrorCode::NotPermitted),
                    ..
                },
                ..
            }]
        ));
    }
}

#[test]
fn headless_session_control_gates_commands_by_guest_role() {
    let (mut server, _dir) = shared_test_server("dev");
    server.panes.insert(1, test_pane(1));
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);

    let send_headless = |server: &mut SessionServer, cmd: ControlCommand| -> ControlResponse {
        let (id, _stream) = add_guest_client(server, bob_uid());
        send_frame(
            server,
            id,
            ClientMessage::SessionControl {
                capabilities: Some(Capabilities::current()),
                session: "alice/dev".into(),
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: PROTOCOL_VERSION,
                request: control_req(cmd),
            },
        );
        match outbox_controls(server, id).into_iter().next().unwrap() {
            ServerMessage::SessionControlResult { response, .. } => response,
            other => panic!("expected SessionControlResult, got {other:?}"),
        }
    };

    // Viewer can View (ListPanes), cannot Input (Notify).
    assert!(send_headless(&mut server, ControlCommand::ListPanes).ok);
    let denied = send_headless(
        &mut server,
        ControlCommand::Notify {
            message: "hi".into(),
            title: None,
            level: crate::control::NotifyLevel::Info,
        },
    );
    assert_eq!(denied.code, Some(ControlErrorCode::NotPermitted));

    // A viewer may list requests, but a prompt or an agent's answer can exist outside any pane.
    assert!(
        send_headless(
            &mut server,
            ControlCommand::AgentRequests { direction: None }
        )
        .ok
    );
    for command in [
        ControlCommand::AgentRequest { id: "r1".into() },
        ControlCommand::AgentResponse {
            target: crate::control::AgentTarget::Pane(1),
        },
    ] {
        let denied = send_headless(&mut server, command);
        assert_eq!(denied.code, Some(ControlErrorCode::NotPermitted));
    }

    // Upgrade to Editor: Notify succeeds, PaneClose (Layout) is denied.
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    assert!(
        send_headless(
            &mut server,
            ControlCommand::Notify {
                message: "hi".into(),
                title: None,
                level: crate::control::NotifyLevel::Info,
            },
        )
        .ok
    );
    let denied_layout = send_headless(
        &mut server,
        ControlCommand::PaneClose {
            target: 1,
            if_revision: None,
        },
    );
    assert_eq!(denied_layout.code, Some(ControlErrorCode::NotPermitted));
}

#[test]
fn downgrade_moves_lease_and_revoke_disconnects_and_unlinks_endpoint() {
    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Controller);
    grant_user(
        &mut server,
        charlie_uid(),
        "charlie",
        SessionRole::Controller,
    );

    let (bob, _b_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    let (charlie, _c_stream) = attach_guest(&mut server, charlie_uid(), "alice/dev", false);
    assert_eq!(server.controller, Some(bob));

    let shared_path = server.shares.endpoint_path().unwrap().to_path_buf();
    assert!(shared_path.exists());

    // Downgrade Bob from Controller to Editor: lease moves to Charlie immediately.
    clear_outboxes(&mut server, &[bob, charlie]);
    let resp = grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    assert!(resp.ok);
    assert_eq!(server.controller, Some(charlie));
    assert!(outbox_controls(&server, charlie).iter().any(|m| matches!(
        m,
        ServerMessage::ControllerChanged {
            controller: Some(id),
            ..
        } if *id == charlie
    )));

    // Revoke Bob: Bob is sent EVICTED_ERROR_CODE and closed; shared socket remains while Charlie is granted.
    let mut broadcasts = Vec::new();
    let revoked = server.share_revoke_with(Some("bob"), test_lookup(&[]), &mut broadcasts);
    for (target, msg) in broadcasts {
        server.enqueue(0, target, msg);
    }
    assert!(revoked.ok);
    let state: ShareState = serde_json::from_value(revoked.data.unwrap()).unwrap();
    assert_eq!(state.grants.len(), 1);
    assert_eq!(state.grants[0].user, "charlie");
    assert_eq!(state.grants[0].connected, 1);
    assert!(shared_path.exists());
    assert!(server.client(bob).unwrap().close_after_flush);
    // Revocation is immediate: what the session sends after it, before Bob's outbox drains, is
    // not queued for him.
    server.broadcast_control(&ServerMessage::Renamed {
        session: "after-revoke".into(),
    });
    let bob_controls = outbox_controls(&server, bob);
    assert!(matches!(
        bob_controls.last(),
        Some(ServerMessage::Error { code, message, .. })
            if code == EVICTED_ERROR_CODE && message == "access to this session was revoked"
    ));
    assert!(outbox_controls(&server, charlie).iter().any(|m| matches!(
        m,
        ServerMessage::Renamed { session } if session == "after-revoke"
    )));
    // A revoked client that never reads its notice is closed anyway.
    server.expire_shared_connections(Instant::now() + Duration::from_secs(60));
    assert!(server.client(bob).is_none());
    assert!(server.client(charlie).is_some());

    // Revoke everyone: Charlie is evicted too, and the shared endpoint is unlinked.
    let mut broadcasts = Vec::new();
    let revoked_all = server.share_revoke_with(None, test_lookup(&[]), &mut broadcasts);
    for (target, msg) in broadcasts {
        server.enqueue(0, target, msg);
    }
    assert!(revoked_all.ok);
    let state_all: ShareState = serde_json::from_value(revoked_all.data.unwrap()).unwrap();
    assert!(state_all.endpoint.is_none());
    assert!(state_all.grants.is_empty());
    assert!(!shared_path.exists());
    assert!(server.client(charlie).unwrap().close_after_flush);
    assert_eq!(server.controller, None);
}

#[test]
fn owner_retakes_control_from_guest_with_takeover_disabled_and_guest_cannot_evict_owner() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = SessionServer::new_named_with_settings(
        "dev",
        ServerSettings {
            allow_takeover: false,
            ..ServerSettings::default()
        },
    );
    server.shares.set_test_shared_dir(dir.path().to_path_buf());
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Controller);

    // Guest attaches first and gets the lease.
    let (bob, _b_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    assert_eq!(server.controller, Some(bob));

    // Owner attaches later as a follower; even with allow_takeover = false, guest cannot evict owner.
    let (owner, _o_stream) = attach_client(&mut server);
    server.pump_attach_seeds();
    clear_outboxes(&mut server, &[bob, owner]);
    send_frame(
        &mut server,
        bob,
        ClientMessage::EvictClient { target: owner },
    );
    assert!(!server.client(owner).unwrap().close_after_flush);
    assert!(server.client_attached(owner));
    assert!(matches!(
        outbox_controls(&server, bob).as_slice(),
        [ServerMessage::Error { code, .. }] if code == PERMISSION_DENIED_ERROR_CODE
    ));

    // The owner removes a guest without holding the lease the other guest keeps.
    grant_user(&mut server, charlie_uid(), "charlie", SessionRole::Editor);
    let (charlie, _c_stream) = attach_guest(&mut server, charlie_uid(), "alice/dev", false);
    send_frame(
        &mut server,
        owner,
        ClientMessage::EvictClient { target: charlie },
    );
    assert_eq!(server.controller, Some(bob));
    assert!(server.client(charlie).unwrap().close_after_flush);

    // Owner's RequestControl takes control from the guest immediately despite allow_takeover = false.
    send_frame(&mut server, owner, ClientMessage::RequestControl);
    assert_eq!(server.controller, Some(owner));
}

#[test]
fn share_commands_validate_arguments_and_require_session_sharing_capability() {
    let (mut server, _dir) = shared_test_server("dev");

    // Client without Capability::SessionSharing gets Unsupported refusal.
    let no_cap = server.handle_session_control(
        1,
        "dev".into(),
        PROTOCOL_VERSION,
        PROTOCOL_VERSION,
        None,
        control_req(ControlCommand::ShareList),
    );
    assert!(matches!(
        no_cap.as_slice(),
        [(
            Target::Sender,
            ServerMessage::SessionControlResult {
                response: ControlResponse {
                    ok: false,
                    code: Some(ControlErrorCode::Unsupported),
                    ..
                },
                ..
            }
        )]
    ));

    // Granting Owner role or sharing with oneself returns InvalidRequest.
    let owner_role = grant_user(&mut server, bob_uid(), "bob", SessionRole::Owner);
    assert_eq!(owner_role.code, Some(ControlErrorCode::InvalidRequest));

    let self_grant = grant_user(&mut server, current_user_id(), "alice", SessionRole::Viewer);
    assert_eq!(self_grant.code, Some(ControlErrorCode::InvalidRequest));

    let mut broadcasts = Vec::new();
    let unknown = server.share_grant_with(
        "nobody",
        SessionRole::Viewer,
        test_lookup(&[]),
        &mut broadcasts,
    );
    assert_eq!(unknown.code, Some(ControlErrorCode::InvalidArgument));
}

#[test]
fn rename_rebinds_and_shutdown_unlinks_shared_endpoint() {
    let _dirs = crate::test_support::isolate_user_dirs();
    let (mut server, dir) = shared_test_server("alpha");
    let (_owner, _o_stream) = attach_client(&mut server);
    let (_endpoint_guard_dir, _priv_ep) = {
        let ep = session_endpoint("alpha").unwrap();
        let bound = ep.bind().unwrap();
        let _listener = bound.into_listener();
        server.endpoint = Some(ep.clone());
        ((), ep)
    };
    assert!(grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer).ok);
    let old_shared = dir.path().join("session-alpha.sock");
    let new_shared = dir.path().join("session-beta.sock");
    assert!(old_shared.exists());

    let renamed = server.rename_session("beta".into());
    assert!(
        matches!(renamed, ServerMessage::Renamed { ref session } if session == "beta"),
        "{renamed:?}"
    );
    assert!(!old_shared.exists());
    assert!(new_shared.exists());

    server.shutdown_listener().unwrap();
    assert!(!new_shared.exists());
}

#[test]
fn guest_handoff_respects_role_changes_and_read_only_clamp_persists_across_upgrades() {
    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Controller);
    grant_user(&mut server, charlie_uid(), "charlie", SessionRole::Editor);

    // Charlie attaches with read_only: true -> clamped to Viewer, stays Viewer on upgrade to Controller.
    let (charlie, _c_stream) = attach_guest(&mut server, charlie_uid(), "alice/dev", true);
    assert_eq!(
        server.client(charlie).unwrap().access.role,
        SessionRole::Viewer
    );
    grant_user(
        &mut server,
        charlie_uid(),
        "charlie",
        SessionRole::Controller,
    );
    assert_eq!(
        server.client(charlie).unwrap().access.role,
        SessionRole::Viewer
    );
    assert!(server.client_read_only(charlie));

    // Bob attaches as Controller, holds lease, prepares/commits/departs handoff.
    let (bob, _b_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    assert_eq!(server.controller, Some(bob));
    let replies = server.handle_message(
        bob,
        ClientMessage::PrepareHandoff {
            request_id: 1,
            hold_ms: 0,
        },
    );
    let token = match replies.as_slice() {
        [
            (
                Target::Sender,
                ServerMessage::HandoffPrepared {
                    token, error: None, ..
                },
            ),
        ] => token.clone(),
        other => panic!("expected HandoffPrepared, got {other:?}"),
    };
    server.handle_message(
        bob,
        ClientMessage::CommitHandoff {
            request_id: 2,
            token: token.clone(),
        },
    );
    server.handle_message(
        bob,
        ClientMessage::DepartForHandoff {
            token: token.clone(),
        },
    );
    server.remove_client(bob);
    assert_eq!(server.controller, Some(bob));

    // While Bob is in transit, downgrade Bob to Editor.
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);

    // Bob reattaches with the handoff token: slot claims, but Bob cannot hold the lease anymore!
    let (bob2, _b2_stream) = add_guest_client(&mut server, bob_uid());
    send_frame(
        &mut server,
        bob2,
        ClientMessage::Attach {
            capabilities: Some(Capabilities::current()),
            session: "alice/dev".into(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: "ignored".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
            handoff_token: Some(token),
        },
    );
    let ServerMessage::Attached {
        handoff_claimed,
        controller,
        ..
    } = attached_reply(&server, bob2)
    else {
        unreachable!()
    };
    assert_eq!(handoff_claimed, Some(true));
    assert_eq!(controller, None);
    assert_eq!(server.controller, None);
    assert_eq!(
        server.client(bob2).unwrap().access.role,
        SessionRole::Editor
    );
}

#[test]
fn attached_control_share_grant_succeeds_for_owner_and_refuses_guest_controller() {
    let (mut server, _dir) = shared_test_server("dev");
    let (owner, _o_stream) = attach_client(&mut server);
    let bob = bob_uid();
    let bob_spec = bob.to_string();

    clear_outboxes(&mut server, &[owner]);
    send_frame(
        &mut server,
        owner,
        ClientMessage::AttachedControl {
            request_id: 11,
            request: control_req(ControlCommand::ShareGrant {
                user: bob_spec.clone(),
                role: SessionRole::Controller,
            }),
        },
    );
    let reply = outbox_controls(&server, owner);
    let [
        ServerMessage::AttachedControlResult {
            request_id: 11,
            response,
        },
    ] = reply.as_slice()
    else {
        panic!("expected AttachedControlResult, got {reply:?}");
    };
    assert!(response.ok, "{response:?}");
    let share_state: ShareState = serde_json::from_value(response.data.clone().unwrap()).unwrap();
    assert_eq!(share_state.grants.len(), 1);
    assert_eq!(share_state.grants[0].uid, bob.as_raw());
    assert_eq!(share_state.grants[0].role, SessionRole::Controller);

    // Attach Bob as guest controller and verify AttachedControl ShareGrant is refused.
    let (guest_ctrl, _g_stream) = attach_guest(&mut server, bob, "alice/dev", false);
    assert_eq!(
        server.client(guest_ctrl).unwrap().access.role,
        SessionRole::Controller
    );
    let charlie = charlie_uid();
    clear_outboxes(&mut server, &[guest_ctrl]);
    send_frame(
        &mut server,
        guest_ctrl,
        ClientMessage::AttachedControl {
            request_id: 12,
            request: control_req(ControlCommand::ShareGrant {
                user: charlie.to_string(),
                role: SessionRole::Viewer,
            }),
        },
    );
    assert!(matches!(
        outbox_controls(&server, guest_ctrl).as_slice(),
        [ServerMessage::AttachedControlResult {
            request_id: 12,
            response: ControlResponse {
                ok: false,
                code: Some(ControlErrorCode::NotPermitted),
                ..
            }
        }]
    ));
    assert!(server.shares.grant_for(charlie).is_none());
}

#[test]
fn editor_downgraded_mid_upload_gets_the_refusal_as_an_attachment_ack() {
    use crate::session::protocol::AttachmentOperation;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let (mut server, _dir) = shared_test_server("dev");
    let pty = TerminalPty::spawn(
        TerminalPtyConfig::new("/bin/sh").arg("-c").arg("cat"),
        |_| {},
    )
    .unwrap();
    let mut pane = test_pane(2);
    pane.pty = Some(pty);
    server.panes.insert(1, pane);
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    let (bob, _stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);

    send_frame(
        &mut server,
        bob,
        ClientMessage::Attachment {
            transfer_id: 1,
            operation: AttachmentOperation::Begin {
                pane_id: 1,
                generation: 2,
                local: false,
                name: "notes.txt".into(),
                size: 4,
            },
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.attachments.is_busy((bob, 1)) {
        assert!(Instant::now() < deadline);
        server.poll_attachment_work();
        std::thread::yield_now();
    }
    assert!(server.attachments.has_pending());

    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);
    clear_outboxes(&mut server, &[bob]);
    send_frame(
        &mut server,
        bob,
        ClientMessage::Attachment {
            transfer_id: 1,
            operation: AttachmentOperation::Chunk {
                data: STANDARD.encode(b"data"),
            },
        },
    );
    assert!(matches!(
        outbox_controls(&server, bob).as_slice(),
        [ServerMessage::AttachmentAck {
            transfer_id: 1,
            error: Some(_),
        }]
    ));
    assert!(!server.attachments.has_pending());
    server.panes[&1].pty.as_ref().unwrap().kill().unwrap();
}

#[test]
fn shared_endpoint_bounds_connections_and_closes_silent_ones() {
    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);

    // Accounts without a grant share a few slots; a granted account is not crowded out by them.
    let strangers: Vec<_> = std::iter::from_fn(|| {
        server
            .shared_peer_has_room(charlie_uid())
            .then(|| add_guest_client(&mut server, charlie_uid()))
    })
    .take(64)
    .collect();
    assert!(strangers.len() < 64);
    assert!(server.shared_peer_has_room(bob_uid()));

    // A connection that never makes a request is closed once its handshake window passes, even if
    // it sends other frames; one that has attached is not.
    server.process_client_frame(strangers[0].0, Frame::PresentationAcks(Vec::new()));
    let (bob, _stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    server.expire_shared_connections(Instant::now() + Duration::from_secs(60));
    assert!(server.client(bob).is_some());
    for (stranger, _) in &strangers {
        assert!(server.client(*stranger).is_none());
    }
    assert!(server.shared_peer_has_room(charlie_uid()));
}

#[test]
fn an_answer_held_across_a_downgrade_is_authorized_again_before_it_goes() {
    use crate::control::{AgentTarget, AgentWaitCondition};
    use crate::session::protocol::{AgentIdentity, DetectedAgent, DetectedAgentState};

    let (mut server, _dir) = shared_test_server("dev");
    let mut pane = test_pane(7);
    pane.runtime.detected_agent = Some(DetectedAgent {
        agent: AgentIdentity::new("claude", "Claude Code").into(),
        state: DetectedAgentState::Working,
    });
    pane.agent.sync_references(&pane.runtime);
    server.panes.insert(3, pane);
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    let (bob, _stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    clear_outboxes(&mut server, &[bob]);

    // Two answers held at once under one request id, by different parts of the server: the
    // second, which a viewer may read, must not lend its permission to the first.
    for command in [
        ControlCommand::AgentWait {
            target: AgentTarget::Pane(3),
            until: AgentWaitCondition::Idle,
            timeout_ms: None,
        },
        ControlCommand::ExtensionsStatus,
    ] {
        send_frame(
            &mut server,
            bob,
            ClientMessage::AttachedControl {
                request_id: 7,
                request: control_req(command),
            },
        );
    }
    let held = outbox_controls(&server, bob);
    assert!(held.is_empty(), "both answers are held: {held:?}");

    grant_user(&mut server, bob_uid(), "bob", SessionRole::Viewer);
    clear_outboxes(&mut server, &[bob]);
    let pane = server.panes.get_mut(&3).unwrap();
    pane.runtime.detected_agent.as_mut().unwrap().state = DetectedAgentState::Idle;
    pane.agent.sync_references(&pane.runtime);
    server.resolve_agent_waits();

    let controls = outbox_controls(&server, bob);
    let [ServerMessage::AttachedControlResult { response, .. }] = controls.as_slice() else {
        panic!("{controls:?}");
    };
    assert_eq!(response.code, Some(ControlErrorCode::NotPermitted));
    assert!(response.data.is_none());
}

/// A guest's pane runs as the owner and outlives the guest, so the owner's config decides its shell
/// and runner and the guest's own environment stays behind. What the guest asked for - the command,
/// where it runs, and values of its own - still arrives. The owner's own panes are unchanged.
#[test]
fn a_guest_pane_runs_with_the_owner_shell_runner_and_environment() {
    crate::test_support::isolate_user_dirs();
    let _config = crate::test_support::lock_config_file();
    std::fs::write(
        crate::config::config_path(),
        "shell = [\"/bin/sh\"]\ncommand_shell = [\"/bin/sh\", \"-c\"]\n",
    )
    .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cwd = cwd.path().to_str().unwrap().to_string();
    let spawn = |pane_id, shell: &str, runner: &str| ClientMessage::SpawnPane {
        placement: None,
        pane_id,
        local: true,
        generation: 1,
        launch: Some(crate::pane::launch::PaneLaunch::shell("sleep 5")),
        cwd: Some(cwd.clone()),
        cols: 20,
        rows: 5,
        keep_open: false,
        env: vec![
            ("CLIENT_AMBIENT".into(), "1".into()),
            ("ROZI_BIN".into(), "/home/client/bin/rozi".into()),
            ("ROZI_SOCKET".into(), "/run/client/rozi.sock".into()),
            ("PER_SPAWN".into(), "a file name".into()),
        ],
        title: None,
        palette: crate::session::protocol::WirePalette::from(
            tui_lipan::TerminalColorPalette::default(),
        ),
        shell: vec![shell.to_string()],
        command_shell: vec![runner.to_string(), "-c".to_string()],
        cell_width: 0,
        cell_height: 0,
        // A client can name what the ambient environment would have carried as values of its own.
        spawn_env: Some(vec![
            ("PER_SPAWN".into(), "a file name".into()),
            ("ROZI_FILE".into(), "/srv/notes.md".into()),
            ("ROZI_BIN".into(), "/home/client/bin/rozi".into()),
            ("ROZI_SOCKET".into(), "/run/client/rozi.sock".into()),
            ("ROZI_EXTENSION".into(), "tools".into()),
            ("WAYLAND_DISPLAY".into(), "wayland-client".into()),
        ]),
    };
    let value = |env: &[(String, String)], key: &str| {
        env.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };

    let (mut server, _dir) = shared_test_server("dev");
    grant_user(&mut server, bob_uid(), "bob", SessionRole::Editor);
    let (bob, _b_stream) = attach_guest(&mut server, bob_uid(), "alice/dev", false);
    clear_outboxes(&mut server, &[bob]);
    // The guest's runner does not exist: the spawn can only succeed with the owner's.
    send_frame(
        &mut server,
        bob,
        spawn(1, "/home/bob/bin/fish", "/home/bob/bin/runner"),
    );
    assert!(
        outbox_controls(&server, bob)
            .iter()
            .any(|reply| matches!(reply, ServerMessage::SpawnResult { ok: true, .. }))
    );
    let pane = &server.local_panes[&(bob, 1)];
    assert_eq!(pane.shell, ["/bin/sh"]);
    assert_eq!(pane.cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(
        value(&pane.env, "PER_SPAWN").as_deref(),
        Some("a file name")
    );
    assert_eq!(value(&pane.env, "ROZI_PANE").as_deref(), Some("1"));
    assert_eq!(
        value(&pane.env, "ROZI_FILE").as_deref(),
        Some("/srv/notes.md")
    );
    assert_ne!(
        value(&pane.env, "WAYLAND_DISPLAY").as_deref(),
        Some("wayland-client")
    );
    for client_only in [
        "CLIENT_AMBIENT",
        "ROZI_BIN",
        "ROZI_SOCKET",
        "ROZI_EXTENSION",
    ] {
        assert_eq!(value(&pane.env, client_only), None, "{client_only}");
    }

    let (owner, _o_stream) = attach_client(&mut server);
    send_frame(&mut server, owner, spawn(2, "/bin/bash", "/bin/sh"));
    let pane = &server.local_panes[&(owner, 2)];
    assert_eq!(pane.shell, ["/bin/bash"]);
    assert_eq!(value(&pane.env, "CLIENT_AMBIENT").as_deref(), Some("1"));
    assert_eq!(
        value(&pane.env, "ROZI_BIN").as_deref(),
        Some("/home/client/bin/rozi")
    );

    for pane in server.local_panes.values() {
        if let Some(pty) = &pane.pty {
            let _ = pty.kill();
        }
    }
}
