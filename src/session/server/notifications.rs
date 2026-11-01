//! Session notification storage, independent of client presentation and read state.
use super::{SessionServer, events::EventKind};
use crate::control::{ControlErrorCode, ControlResponse, NotifyLevel, SessionNotification};

const INBOX_COUNT: usize = 64;
const INBOX_BYTES: usize = 256 * 1024;
const MESSAGE_BYTES: usize = 8 * 1024;

impl SessionServer {
    pub(super) fn session_notify(
        &mut self,
        message: String,
        title: Option<String>,
        level: NotifyLevel,
    ) -> ControlResponse {
        let message = message.trim().to_string();
        if message.is_empty()
            || message.len() + title.as_ref().map_or(0, String::len) > MESSAGE_BYTES
        {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "notify requires a nonempty message and at most 8192 bytes of message and title",
            );
        }
        let seq = self.session_events.latest_seq() + 1;
        let notification = SessionNotification {
            id: format!("{}:{seq}", self.instance_id),
            seq,
            level,
            title,
            message,
        };
        self.record_session_event(
            EventKind::SessionNotification,
            vec![
                ("id", notification.id.clone()),
                (
                    "level",
                    match level {
                        NotifyLevel::Info => "info",
                        NotifyLevel::Error => "error",
                    }
                    .into(),
                ),
                ("title", notification.title.clone().unwrap_or_default()),
                ("message", notification.message.clone()),
                ("session", self.session_name.clone()),
            ],
        );
        self.notifications.push_back(notification.clone());
        while self.notifications.len() > INBOX_COUNT
            || self
                .notifications
                .iter()
                .map(|notification| {
                    notification.message.len() + notification.title.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
                > INBOX_BYTES
        {
            self.notifications.pop_front();
        }
        ControlResponse::ok(notification)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::session::protocol::{
        Capabilities, ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
    };
    use crate::session::server::{
        events::SessionSubscribeRecord,
        headless::ReplyTo,
        tests::{add_client, attach_client, decode_outbox_controls},
    };

    #[test]
    fn detached_notifications_reach_snapshots_late_attach_and_every_live_client() {
        let mut server = SessionServer::new_named("inbox");
        let response = server.session_notify(
            "detached message".into(),
            Some("Build".into()),
            NotifyLevel::Error,
        );
        assert!(response.ok);
        let (subscriber, _stream) = add_client(&mut server);
        assert!(
            server
                .handle_subscribe_command(
                    subscriber,
                    Vec::new(),
                    true,
                    None,
                    ReplyTo::StreamingHeadless {
                        capabilities: Capabilities::current(),
                        effective_protocol: PROTOCOL_VERSION
                    }
                )
                .is_none()
        );
        let snapshot = decode_outbox_controls(server.client(subscriber).unwrap());
        let retained = snapshot.iter().find_map(|message| match message {
            ServerMessage::SessionSubscribeRecord {
                record: SessionSubscribeRecord::Snapshot(snapshot),
            } => Some(&snapshot.notifications),
            _ => None,
        });
        let retained = retained.expect("a snapshot record");
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].message, "detached message");
        let (late, _late_stream) = add_client(&mut server);
        server.handle_message(
            late,
            ClientMessage::Attach {
                capabilities: None,
                session: "inbox".into(),
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: MIN_SUPPORTED_PROTOCOL,
                label: "late".into(),
                read_only: false,
                shares_filesystem: true,
                expected_server_nonce: None,
            },
        );
        let inbox = decode_outbox_controls(server.client(late).unwrap());
        let inbox = inbox.iter().find_map(|message| match message {
            ServerMessage::SessionNotifications { notifications, .. } => Some(notifications),
            _ => None,
        });
        assert_eq!(inbox.expect("the retained inbox").len(), 1);
        let (other, _other_stream) = attach_client(&mut server);
        server.session_notify("live message".into(), None, NotifyLevel::Info);
        let mut sequences = Vec::new();
        for client in [late, other] {
            let events = decode_outbox_controls(server.client(client).unwrap());
            let event = events
                .iter()
                .find_map(|message| match message {
                    ServerMessage::SessionEvent { event }
                        if event.kind == EventKind::SessionNotification =>
                    {
                        Some(event)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(event.data["message"], "live message");
            sequences.push(event.seq);
        }
        assert_eq!(sequences[0], sequences[1]);
    }

    #[test]
    fn inbox_is_bounded_by_bytes_and_invalid_messages_consume_no_sequence() {
        let mut server = SessionServer::new_named("inbox-limits");
        for _ in 0..70 {
            assert!(
                server
                    .session_notify("x".repeat(5000), None, NotifyLevel::Info)
                    .ok
            );
        }
        assert!(server.notifications.len() <= INBOX_COUNT);
        assert!(server.notifications.len() * 5000 <= INBOX_BYTES);
        assert!(server.notifications.front().unwrap().seq > 1);
        let seq = server.session_events.latest_seq();
        assert!(
            !server
                .session_notify(" ".into(), None, NotifyLevel::Info)
                .ok
        );
        assert!(
            !server
                .session_notify("x".repeat(MESSAGE_BYTES + 1), None, NotifyLevel::Info)
                .ok
        );
        assert_eq!(server.session_events.latest_seq(), seq);
    }
}
