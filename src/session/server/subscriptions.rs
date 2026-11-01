//! Recording session events and delivering them: to attached UIs, and to `subscribe` connections.

use std::collections::HashSet;
use std::result::Result;

use super::headless::ReplyTo;
use super::*;
use crate::control::{ControlErrorCode, ControlResponse};
use crate::session::protocol::MAX_FRAME_SIZE;

/// A connection that turned into a stream of session records.
#[derive(Clone, Debug)]
pub(super) struct SessionSubscriber {
    /// The events it named; `None` means all of them.
    filter: Option<HashSet<events::EventKind>>,
}

impl SessionSubscriber {
    fn wants(&self, kind: events::EventKind) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|filter| filter.contains(&kind))
    }
}

/// The options a `subscribe` request carries.
pub(super) struct SubscribeOptions {
    pub(super) events: Vec<String>,
    pub(super) snapshot: bool,
    pub(super) after: Option<String>,
}

fn parse_event_filter(names: &[String]) -> Result<SessionSubscriber, ControlResponse> {
    if names.is_empty() {
        return Ok(SessionSubscriber { filter: None });
    }
    let mut filter = HashSet::new();
    for name in names {
        match events::EventKind::parse(name) {
            Some(kind) if kind.is_session_owned() => {
                filter.insert(kind);
            }
            Some(_) => {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::InvalidArgument,
                    format!(
                        "`{name}` is a UI event; a session server does not raise it (subscribe to the UI instead)"
                    ),
                ));
            }
            None => {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::UnknownEvent,
                    format!("unknown event `{name}`"),
                ));
            }
        }
    }
    Ok(SessionSubscriber {
        filter: Some(filter),
    })
}

fn parse_cursor(cursor: &str) -> Result<(protocol::SessionInstanceId, u64), ControlResponse> {
    let invalid = || {
        ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            format!("cursor must be INSTANCE:SEQ, got `{cursor}`"),
        )
    };
    let (instance, seq) = cursor.split_once(':').ok_or_else(invalid)?;
    let instance = protocol::SessionInstanceId::from_env_value(instance).ok_or_else(invalid)?;
    let seq = seq.parse().map_err(|_| invalid())?;
    Ok((instance, seq))
}

impl SessionServer {
    pub(super) fn record_session_event(
        &mut self,
        kind: events::EventKind,
        fields: Vec<(&'static str, String)>,
    ) {
        debug_assert!(kind.is_session_owned(), "{} is a UI event", kind.id());
        let event = self.session_events.record(kind, fields);
        self.dispatch_session_subscribers(&event);
        self.undelivered_ui_events.push_back(event);
    }

    /// Send recorded events to attached UIs, once the state messages they describe are queued.
    ///
    /// An event is recorded where the server's state changes, and the message telling clients about
    /// that change usually goes out after it. A UI that heard the event first would run a hook on a
    /// pane it has not been told about, and would decide who runs that hook from a controller it
    /// has not yet heard has changed.
    ///
    /// Runs before anything reaches a socket, and before an attach, whose reply already shows the
    /// new client these changes.
    pub(super) fn flush_session_events(&mut self) {
        // Delivery can expire a slow client, which records `client-left` onto this same queue.
        while let Some(event) = self.undelivered_ui_events.pop_front() {
            self.broadcast_control(&ServerMessage::SessionEvent { event });
        }
    }

    pub(super) fn is_session_subscriber(&self, client_id: ClientId) -> bool {
        self.session_subscribers.contains_key(&client_id)
    }

    fn dispatch_session_subscribers(&mut self, event: &events::SessionEvent) {
        let targets: Vec<ClientId> = self
            .session_subscribers
            .iter()
            .filter(|(_, subscriber)| subscriber.wants(event.kind))
            .map(|(&id, _)| id)
            .collect();
        if targets.is_empty() {
            return;
        }
        let message = ServerMessage::SessionSubscribeRecord {
            record: events::SessionSubscribeRecord::Event {
                event: event.clone(),
            },
        };
        let Some(bytes) = encode_control(&message) else {
            return;
        };
        let max_backlog = self.max_backlog;
        for client_id in targets {
            let result = self.client_mut(client_id).map(|client| {
                client.push_delta(Arc::clone(&bytes), SeedDelta::Control, max_backlog, None)
            });
            if matches!(result, Some(ClientFrameResult::Overflow)) {
                self.end_lagged_subscriber(client_id);
            }
        }
    }

    /// Stop streaming to a subscriber that fell behind, and say so as its last record. The record
    /// goes past the backlog limit, or the reader would never learn why the stream stopped.
    fn end_lagged_subscriber(&mut self, client_id: ClientId) {
        self.session_subscribers.remove(&client_id);
        let lagged = ServerMessage::SessionSubscribeRecord {
            record: events::SessionSubscribeRecord::Lagged,
        };
        if let Some(bytes) = encode_control(&lagged)
            && let Some(client) = self.client_mut(client_id)
        {
            client.push_delta(bytes, SeedDelta::Control, usize::MAX, None);
        }
        self.set_close_after_flush(client_id);
    }

    /// Answer a `subscribe` request. `None` means the answer has already been queued, followed by
    /// the stream's first records; `Some` is a refusal for the caller to send.
    pub(super) fn handle_subscribe_command(
        &mut self,
        client_id: ClientId,
        options: SubscribeOptions,
        reply: &ReplyTo,
    ) -> Option<ControlResponse> {
        self.open_subscription(client_id, options, reply).err()
    }

    /// Everything from the snapshot or replay to registering the subscriber happens in this one
    /// call, so no event can fall between what the subscriber is sent now and what follows.
    fn open_subscription(
        &mut self,
        client_id: ClientId,
        options: SubscribeOptions,
        reply: &ReplyTo,
    ) -> Result<(), ControlResponse> {
        if !matches!(reply, ReplyTo::StreamingHeadless { .. }) {
            return Err(ControlResponse::error_with(
                ControlErrorCode::InvalidRequest,
                "an attached client hears session events on its own connection",
            ));
        }
        if options.snapshot && options.after.is_some() {
            return Err(ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "snapshot and after cannot be combined",
            ));
        }
        let subscriber = parse_event_filter(&options.events)?;
        let replay = match &options.after {
            Some(cursor) => self.replay_after(cursor)?,
            None => Vec::new(),
        };
        let snapshot = options
            .snapshot
            .then(|| self.subscribe_snapshot())
            .transpose()?;

        self.enqueue(
            client_id,
            Target::Sender,
            reply.message(ControlResponse::empty()),
        );
        if let Some(snapshot) = snapshot {
            self.enqueue(client_id, Target::Sender, snapshot);
        }
        for event in replay {
            if subscriber.wants(event.kind) {
                self.enqueue(
                    client_id,
                    Target::Sender,
                    ServerMessage::SessionSubscribeRecord {
                        record: events::SessionSubscribeRecord::Event { event },
                    },
                );
            }
        }
        // Queuing the replay can overflow and drop the connection, leaving nothing to stream to.
        if self.client_mut(client_id).is_some() {
            self.session_subscribers.insert(client_id, subscriber);
        }
        Ok(())
    }

    fn replay_after(&self, cursor: &str) -> Result<Vec<events::SessionEvent>, ControlResponse> {
        let (instance, seq) = parse_cursor(cursor)?;
        self.session_events
            .after(&instance, seq)
            .map_err(|expired| {
                let mut response = ControlResponse::error_with(
                    ControlErrorCode::CursorExpired,
                    expired.to_string(),
                );
                response.data = serde_json::to_value(&expired).ok();
                response
            })
    }

    /// The opening snapshot, or a refusal when it would not fit a frame. It is refused here, while
    /// that can still be said: a frame too large vanishes on the way out, leaving a stream that
    /// looks healthy and never sent the state it promised.
    fn subscribe_snapshot(&self) -> Result<ServerMessage, ControlResponse> {
        let layout = self
            .session_layout_report(None)
            .data
            .and_then(|data| serde_json::from_value(data).ok());
        let message = ServerMessage::SessionSubscribeRecord {
            record: events::SessionSubscribeRecord::Snapshot(events::SessionSubscribeSnapshot {
                session_instance: self.instance_id.clone(),
                session: self.session_name.clone(),
                seq: self.session_events.latest_seq(),
                controller: self.controller,
                client_count: self.attached_count(),
                layout_rev: self.layout_rev,
                layout,
                panes: self.session_pane_report(),
            }),
        };
        let encoded = serde_json::to_vec(&message).map_or(usize::MAX, |body| body.len());
        if encoded >= MAX_FRAME_SIZE {
            return Err(ControlResponse::error_with(
                ControlErrorCode::MessageTooLarge,
                format!(
                    "the session's snapshot is {} KiB, over the {} KiB one record can carry; subscribe without --snapshot and read `list-panes --format json` and `layout get` instead",
                    encoded / 1024,
                    MAX_FRAME_SIZE / 1024
                ),
            ));
        }
        Ok(message)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::super::tests::{add_client, decode_outbox_controls, test_pane};
    use super::*;
    use crate::session::protocol::{Capabilities, PROTOCOL_VERSION};
    use events::{EventKind, SessionSubscribeRecord};

    fn streaming() -> ReplyTo {
        ReplyTo::StreamingHeadless {
            capabilities: Capabilities::current(),
            effective_protocol: PROTOCOL_VERSION,
        }
    }

    fn options(events: &[&str], snapshot: bool, after: Option<String>) -> SubscribeOptions {
        SubscribeOptions {
            events: events.iter().map(ToString::to_string).collect(),
            snapshot,
            after,
        }
    }

    fn subscribe(server: &mut SessionServer, options: SubscribeOptions) -> (ClientId, UnixStream) {
        let (client, stream) = add_client(server);
        let refused = server.handle_subscribe_command(client, options, &streaming());
        assert_eq!(refused.map(|response| response.error), None);
        (client, stream)
    }

    fn refusal(server: &mut SessionServer, options: SubscribeOptions) -> Option<ControlErrorCode> {
        let (client, _stream) = add_client(server);
        let refused = server
            .handle_subscribe_command(client, options, &streaming())
            .expect("the subscription is refused");
        assert!(!server.is_session_subscriber(client));
        refused.code
    }

    /// What the subscriber has been sent after its acknowledgement.
    fn records(server: &SessionServer, client: ClientId) -> Vec<SessionSubscribeRecord> {
        let mut sent = decode_outbox_controls(server.client(client).unwrap()).into_iter();
        assert!(matches!(
            sent.next(),
            Some(ServerMessage::SessionControlResult { response, .. }) if response.ok
        ));
        sent.map(|message| match message {
            ServerMessage::SessionSubscribeRecord { record } => record,
            other => panic!("expected a record, got {other:?}"),
        })
        .collect()
    }

    fn event_seqs(records: &[SessionSubscribeRecord]) -> Vec<u64> {
        records
            .iter()
            .filter_map(|record| match record {
                SessionSubscribeRecord::Event { event } => Some(event.seq),
                _ => None,
            })
            .collect()
    }

    fn record_panes(server: &mut SessionServer, count: u32) {
        for pane in 1..=count {
            server.record_session_event(EventKind::PaneSpawned, vec![("pane", pane.to_string())]);
        }
    }

    #[test]
    fn a_subscriber_hears_later_events_in_sequence() {
        let mut server = SessionServer::new_named("dev");
        let (client, _stream) = subscribe(&mut server, options(&[], false, None));

        record_panes(&mut server, 2);

        assert_eq!(event_seqs(&records(&server, client)), [1, 2]);
    }

    #[test]
    fn a_snapshot_says_where_the_events_after_it_begin() {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(1, test_pane(1));
        record_panes(&mut server, 2);
        let (client, _stream) = subscribe(&mut server, options(&[], true, None));

        record_panes(&mut server, 1);

        let sent = records(&server, client);
        let SessionSubscribeRecord::Snapshot(snapshot) = &sent[0] else {
            panic!("the snapshot comes first: {sent:?}");
        };
        assert_eq!(snapshot.session, "dev");
        assert_eq!(snapshot.seq, 2);
        assert_eq!(snapshot.panes[0].id, 1);
        assert_eq!(event_seqs(&sent), [3]);
    }

    #[test]
    fn a_cursor_replays_what_followed_it_then_carries_on_live() {
        let mut server = SessionServer::new_named("dev");
        record_panes(&mut server, 3);
        let cursor = format!("{}:1", server.instance_id);
        let (client, _stream) = subscribe(&mut server, options(&[], false, Some(cursor)));

        record_panes(&mut server, 1);

        assert_eq!(event_seqs(&records(&server, client)), [2, 3, 4]);
    }

    #[test]
    fn a_filter_narrows_both_replay_and_live_events_without_renumbering() {
        let mut server = SessionServer::new_named("dev");
        record_panes(&mut server, 1);
        server.record_session_event(EventKind::Bell, vec![("pane", "1".to_string())]);
        let cursor = format!("{}:0", server.instance_id);
        let (client, _stream) =
            subscribe(&mut server, options(&["pane-spawned"], false, Some(cursor)));

        server.record_session_event(EventKind::Bell, vec![("pane", "1".to_string())]);
        record_panes(&mut server, 1);

        assert_eq!(event_seqs(&records(&server, client)), [1, 4]);
    }

    #[test]
    fn what_a_subscription_could_never_honour_is_refused() {
        let mut server = SessionServer::new_named("dev");
        let current = format!("{}:0", server.instance_id);
        let cases = [
            (
                options(&["focus-changed"], false, None),
                ControlErrorCode::InvalidArgument,
            ),
            (
                options(&["no-such-event"], false, None),
                ControlErrorCode::UnknownEvent,
            ),
            (
                options(&[], true, Some(current)),
                ControlErrorCode::InvalidArgument,
            ),
            (
                options(&[], false, Some("no-separator".to_string())),
                ControlErrorCode::InvalidArgument,
            ),
            (
                options(&[], false, Some(":1".to_string())),
                ControlErrorCode::InvalidArgument,
            ),
            (
                options(&[], false, Some("another-instance:1".to_string())),
                ControlErrorCode::CursorExpired,
            ),
        ];
        for (options, code) in cases {
            let named = (options.events.clone(), options.after.clone());
            assert_eq!(refusal(&mut server, options), Some(code), "{named:?}");
        }
    }

    #[test]
    fn an_expired_cursor_reports_the_range_still_retained() {
        let mut server = SessionServer::new_named("dev");
        record_panes(&mut server, 2);
        let (client, _stream) = add_client(&mut server);

        let refused = server
            .handle_subscribe_command(
                client,
                options(&[], false, Some("another-instance:1".to_string())),
                &streaming(),
            )
            .expect("the cursor is refused");

        let data = refused.data.expect("the refusal carries the range");
        assert_eq!(data["current_instance"], server.instance_id.as_str());
        assert_eq!(data["oldest_seq"], 1);
        assert_eq!(data["latest_seq"], 2);
    }

    #[test]
    fn an_attached_connection_cannot_become_a_stream() {
        let mut server = SessionServer::new_named("dev");
        let (client, _stream) = add_client(&mut server);

        let refused = server
            .handle_subscribe_command(
                client,
                options(&[], false, None),
                &ReplyTo::Attached { request_id: 1 },
            )
            .expect("the request is refused");

        assert_eq!(refused.code, Some(ControlErrorCode::InvalidRequest));
        assert!(!server.is_session_subscriber(client));
    }

    #[test]
    fn a_snapshot_too_large_to_send_is_refused() {
        let mut server = SessionServer::new_named("dev");
        let mut pane = test_pane(1);
        pane.runtime.cwd = Some("x".repeat(MAX_FRAME_SIZE));
        server.panes.insert(1, pane);

        assert_eq!(
            refusal(&mut server, options(&[], true, None)),
            Some(ControlErrorCode::MessageTooLarge)
        );
    }

    #[test]
    fn a_subscription_that_overflows_while_opening_leaves_nothing_behind() {
        let mut server = SessionServer::new_named("dev");
        server.max_backlog = 0;

        let (client, _stream) = subscribe(&mut server, options(&[], true, None));

        assert!(server.client(client).is_none(), "the connection is dropped");
        assert!(server.session_subscribers.is_empty());
    }

    #[test]
    fn a_subscriber_that_falls_behind_is_told_and_closed() {
        let mut server = SessionServer::new_named("dev");
        let (client, _stream) = subscribe(&mut server, options(&[], false, None));

        server.max_backlog = 0;
        record_panes(&mut server, 1);

        assert!(!server.is_session_subscriber(client));
        assert!(server.client(client).unwrap().close_after_flush);
        assert_eq!(
            records(&server, client),
            [SessionSubscribeRecord::Lagged],
            "the record is delivered even though the backlog is full"
        );
    }
}
