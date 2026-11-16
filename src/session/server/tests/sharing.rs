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
    assert!(outbox_controls(&server, bob).iter().any(|m| matches!(
        m,
        ServerMessage::Error { code, message, .. }
            if code == EVICTED_ERROR_CODE && message == "access to this session was revoked"
    )));

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
