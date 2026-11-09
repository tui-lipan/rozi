//! A client relaunching into a new process keeps its slot: the lease waits for the replacement,
//! nobody sees it leave and come back, and a slot nobody claims is released as a normal departure.
use super::*;

fn prepare(server: &mut SessionServer, client: ClientId) -> String {
    let replies = server.handle_message(client, ClientMessage::PrepareHandoff { request_id: 7 });
    match replies.as_slice() {
        [
            (
                Target::Sender,
                ServerMessage::HandoffPrepared {
                    request_id: 7,
                    token,
                    expires_in_ms,
                    error: None,
                },
            ),
        ] => {
            assert!(token.len() >= 32, "token {token:?}");
            assert_eq!(*expires_in_ms, 120_000);
            token.clone()
        }
        other => panic!("unexpected reply: {other:?}"),
    }
}

fn claim_with(
    server: &mut SessionServer,
    token: &str,
    capabilities: Option<protocol::Capabilities>,
) -> (ClientId, UnixStream, Option<bool>) {
    let (id, stream) = add_client(server);
    server.handle_message(
        id,
        ClientMessage::Attach {
            capabilities,
            session: server.session_name.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_VERSION,
            label: format!("client-{id}"),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
            handoff_token: Some(token.to_string()),
        },
    );
    let ServerMessage::Attached {
        handoff_claimed, ..
    } = attached_reply(server, id)
    else {
        unreachable!("attached_reply returns Attached");
    };
    (id, stream, handoff_claimed)
}

fn claim(server: &mut SessionServer, token: &str) -> (ClientId, UnixStream, Option<bool>) {
    claim_with(server, token, Some(protocol::Capabilities::current()))
}

fn membership_events(server: &mut SessionServer) -> Vec<crate::events::EventKind> {
    server.flush_session_events();
    server
        .session_events
        .iter()
        .map(|event| event.kind)
        .filter(|kind| {
            matches!(
                kind,
                crate::events::EventKind::ClientJoined | crate::events::EventKind::ClientLeft
            )
        })
        .collect()
}

#[test]
fn the_relaunched_client_keeps_the_lease_its_previous_process_held() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let before = membership_events(&mut server).len();
    let token = prepare(&mut server, old);

    server.remove_client(old);
    assert_eq!(
        server.controller,
        Some(old),
        "the follower is not promoted while the controller relaunches"
    );
    assert!(server.handoffs.any_in_transit());

    let (new, _new_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(true));
    assert_eq!(server.controller, Some(new));
    assert!(!server.handoffs.any_in_transit());
    assert!(server.client_attached(follower));
    assert_eq!(
        membership_events(&mut server).len(),
        before,
        "nobody is told the client left and joined"
    );
}

#[test]
fn a_token_claims_only_once() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let token = prepare(&mut server, old);
    server.remove_client(old);
    let (first, _first_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(true));

    let (second, _second_stream, claimed_again) = claim(&mut server, &token);
    assert_eq!(claimed_again, Some(false));
    assert_eq!(server.controller, Some(first));
    assert!(
        server.client_attached(second),
        "a spent token is an ordinary attach"
    );
}

#[test]
fn an_unknown_token_attaches_as_an_ordinary_follower() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _stream) = attach_client(&mut server);
    let (joined, _joined_stream, claimed) = claim(&mut server, "not-a-token");
    assert_eq!(claimed, Some(false));
    assert_eq!(server.controller, Some(controller));
    assert!(server.client_attached(joined));
}

#[test]
fn a_claim_that_beats_the_old_connection_closing_takes_the_slot_over_whole() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let (_follower, _follower_stream) = attach_client(&mut server);
    let token = prepare(&mut server, old);

    // The replacement attaches before the server has read the old connection's close.
    let (new, _new_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(true));
    assert!(!server.client_attached(old));
    assert_eq!(server.controller, Some(new));
}

#[test]
fn an_unclaimed_slot_expires_into_an_ordinary_departure() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    let token = prepare(&mut server, old);
    server.remove_client(old);
    let before = membership_events(&mut server);

    server
        .handoffs
        .age_for_test(crate::session::server::handoff::HANDOFF_HOLD);
    server.expire_handoffs();

    assert_eq!(server.controller, Some(follower));
    let after = membership_events(&mut server);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last(), Some(&crate::events::EventKind::ClientLeft));
    let (_late, _late_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(false));
}

#[test]
fn a_reservation_the_client_never_used_does_not_hold_its_later_departure() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let (follower, _follower_stream) = attach_client(&mut server);
    prepare(&mut server, old);

    // The relaunch was abandoned; the client stays and leaves long after.
    server
        .handoffs
        .age_for_test(crate::session::server::handoff::HANDOFF_HOLD);
    server.remove_client(old);

    assert_eq!(server.controller, Some(follower));
    assert!(!server.handoffs.any_in_transit());
}

#[test]
fn a_parked_client_comes_back_parked_and_without_the_lease() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (parked, _parked_stream) = attach_client(&mut server);
    server.handle_message(parked, ClientMessage::SetParked { parked: true });
    let token = prepare(&mut server, parked);
    server.remove_client(parked);
    assert_eq!(server.controller, Some(controller));

    let (new, _new_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(true));
    assert!(server.client_parked(new));
    assert_eq!(server.controller, Some(controller));
}

#[test]
fn an_ephemeral_session_waits_for_a_client_in_transit() {
    let mut server = SessionServer::new_named("eph-handoff-test");
    let (old, _old_stream) = attach_client(&mut server);
    prepare(&mut server, old);
    server.remove_client(old);

    let mut no_client_since = Some(Instant::now() - EPHEMERAL_NO_CLIENT_GRACE);
    server.update_ephemeral_lifetime(&mut no_client_since);
    assert!(!server.shutdown);
    assert_eq!(no_client_since, None);

    server
        .handoffs
        .age_for_test(crate::session::server::handoff::HANDOFF_HOLD);
    server.expire_handoffs();
    server.update_ephemeral_lifetime(&mut no_client_since);
    assert!(
        no_client_since.is_some(),
        "the countdown starts once the slot is released"
    );
}

#[test]
fn an_evicted_client_cannot_come_back_through_its_reservation() {
    let mut server = SessionServer::new_named("dev");
    let (controller, _controller_stream) = attach_client(&mut server);
    let (target, _target_stream) = attach_client(&mut server);
    let token = prepare(&mut server, target);
    server.handle_message(controller, ClientMessage::EvictClient { target });
    server.remove_client(target);

    let (_back, _back_stream, claimed) = claim(&mut server, &token);
    assert_eq!(claimed, Some(false));
}

#[test]
fn a_client_without_the_capability_cannot_claim() {
    let mut server = SessionServer::new_named("dev");
    let (old, _old_stream) = attach_client(&mut server);
    let token = prepare(&mut server, old);
    server.remove_client(old);

    claim_with(&mut server, &token, None);
    assert_eq!(server.controller, Some(old), "the slot stays reserved");
}
