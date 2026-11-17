//! `publish` streams the session server holds itself, for panes with no UI to publish through: a
//! pane a guest opened, a restored pane, one a headless `new-pane` opened, or one on a remote
//! session. A UI holds the stream whenever the pane can reach one; see `rozi publish`.
//!
//! Each stream speaks for one publisher in one pane generation. Its rows are qualified by
//! [`protocol::SESSION_PUBLISHER_NAMESPACE`] rather than by the connection, so a publisher that
//! restarts opens a new connection and takes over the same rows. Only the connection that holds a
//! publisher now can change or withdraw them.

use super::headless::ReplyTo;
use super::runtime::PanePublisher;
use super::*;
use crate::control::{ControlCommand, ControlErrorCode, ControlRequest, ControlResponse};
use protocol::{PublisherKind, RowKey, SESSION_PUBLISHER_NAMESPACE};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct StreamKey {
    pane: PaneId,
    publisher: String,
}

struct Stream {
    connection: ClientId,
    generation: u64,
    kind: PublisherKind,
}

#[derive(Default)]
pub(super) struct SessionPublishers {
    streams: HashMap<StreamKey, Stream>,
    by_connection: HashMap<ClientId, StreamKey>,
}

impl SessionPublishers {
    /// Whether `connection` holds a `publish` stream.
    pub(super) fn holds(&self, connection: ClientId) -> bool {
        self.by_connection.contains_key(&connection)
    }
}

impl SessionServer {
    /// Open a `publish` stream for the calling pane on this connection.
    ///
    /// The caller names its pane with `ROZI_PANE` and proves it means this session with
    /// `ROZI_SESSION_INSTANCE`: pane ids repeat across sessions, so a bare id is not enough.
    pub(super) fn open_session_publisher(
        &mut self,
        client_id: ClientId,
        request: &ControlRequest,
        reply: &ReplyTo,
    ) -> ControlResponse {
        let ControlCommand::Publish { kind, name } = &request.command else {
            return ControlResponse::error_with(ControlErrorCode::InvalidRequest, "not a publish");
        };
        let refuse = |code, message: &str| ControlResponse::error_with(code, message.to_string());
        if !matches!(reply, ReplyTo::StreamingHeadless { .. }) {
            return refuse(
                ControlErrorCode::InvalidRequest,
                "an attached client's panes publish through its own UI",
            );
        }
        if self.session_publishers.holds(client_id) {
            return refuse(
                ControlErrorCode::InvalidRequest,
                "this connection already holds a publish stream",
            );
        }
        if request.source_session.as_ref() != Some(&self.instance_id) {
            return refuse(
                ControlErrorCode::InvalidRequest,
                "publish runs inside a pane of this session; ROZI_SESSION_INSTANCE does not name it",
            );
        }
        let Some(pane_id) = request.source_pane else {
            return refuse(
                ControlErrorCode::InvalidRequest,
                "publish runs inside a pane; ROZI_PANE is not set",
            );
        };
        if name
            .as_deref()
            .is_some_and(|name| !protocol::publisher_name_is_valid(name))
        {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                format!(
                    "publisher name must be 1-{} lowercase letters, digits, `-`, or `_`",
                    protocol::MAX_PUBLISHER_NAME
                ),
            );
        }
        let Some(generation) = self
            .panes
            .get(&pane_id)
            .filter(|pane| pane.exited.is_none())
            .map(|pane| pane.generation)
        else {
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {pane_id} not found"),
            );
        };
        let key = StreamKey {
            pane: pane_id,
            publisher: protocol::publisher_id(None, name.as_deref()),
        };
        // The same limit the UI holds a pane to, and refused as it does - when the stream opens,
        // so a publisher over it knows at once rather than at its first rows.
        let open = self
            .session_publishers
            .streams
            .iter()
            .filter(|(other, stream)| {
                other.pane == pane_id && **other != key && stream.generation == generation
            })
            .count();
        if open >= protocol::MAX_PANE_PUBLISHERS {
            return ControlResponse::error_with(
                ControlErrorCode::Conflict,
                format!(
                    "pane {pane_id} already has {} publishers",
                    protocol::MAX_PANE_PUBLISHERS
                ),
            );
        }
        // A second stream for the same publisher means the first is stale - a publisher that
        // restarted, say. It takes over the rows; the old connection is closed and can no longer
        // touch them.
        let previous = self.session_publishers.streams.insert(
            key.clone(),
            Stream {
                connection: client_id,
                generation,
                kind: *kind,
            },
        );
        if let Some(previous) = previous {
            self.session_publishers
                .by_connection
                .remove(&previous.connection);
            self.set_close_after_flush(previous.connection);
        }
        self.session_publishers.by_connection.insert(client_id, key);
        ControlResponse::empty()
    }

    /// One row update from the stream `client_id` holds.
    pub(super) fn session_publisher_rows(
        &mut self,
        client_id: ClientId,
        rows: Vec<protocol::PublishedRow>,
        responses: Vec<crate::control::PublishedAgentResponse>,
    ) -> Vec<(Target, ServerMessage)> {
        let refusal = |code: &str, message: &str| {
            vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: code.to_string(),
                    message: message.to_string(),
                    peer: None,
                },
            )]
        };
        let Some(key) = self
            .session_publishers
            .by_connection
            .get(&client_id)
            .cloned()
        else {
            return refusal("invalid-request", "this connection holds no publish stream");
        };
        let Some(stream) = self.session_publishers.streams.get(&key) else {
            return Vec::new();
        };
        let (generation, kind) = (stream.generation, stream.kind);
        let current = self
            .panes
            .get(&key.pane)
            .is_some_and(|pane| pane.generation == generation && pane.exited.is_none());
        if !current {
            // The pane this stream spoke for has exited or been replaced; it speaks for nothing
            // now, and must never speak for the pane's next generation.
            self.close_session_publisher(client_id);
            self.set_close_after_flush(client_id);
            return refusal(
                "stale-generation",
                "the pane this stream publishes for has exited or been replaced",
            );
        }
        let publisher = PanePublisher {
            epoch: client_id,
            responses,
            id: protocol::qualified_publisher(SESSION_PUBLISHER_NAMESPACE, &key.publisher),
            kind,
        };
        match self.apply_pane_rows(
            SESSION_PUBLISHER_NAMESPACE,
            None,
            key.pane,
            generation,
            publisher,
            rows,
        ) {
            Ok(Some(message)) => vec![(Target::Broadcast, message)],
            Ok(None) => Vec::new(),
            Err((code, message)) => refusal(code, &message),
        }
    }

    /// The stream `client_id` held is gone. Its rows go with it, unless a newer stream for the same
    /// publisher already took them over.
    pub(super) fn close_session_publisher(&mut self, client_id: ClientId) {
        let Some(key) = self.session_publishers.by_connection.remove(&client_id) else {
            return;
        };
        let Some(stream) = self
            .session_publishers
            .streams
            .remove_entry(&key)
            .filter(|(_, stream)| stream.connection == client_id)
            .map(|(_, stream)| stream)
        else {
            return;
        };
        let still_there = self
            .panes
            .get(&key.pane)
            .is_some_and(|pane| pane.generation == stream.generation);
        if !still_there {
            return;
        }
        let publisher = PanePublisher {
            epoch: client_id,
            responses: Vec::new(),
            id: protocol::qualified_publisher(SESSION_PUBLISHER_NAMESPACE, &key.publisher),
            kind: stream.kind,
        };
        if let Ok(Some(message)) = self.apply_pane_rows(
            SESSION_PUBLISHER_NAMESPACE,
            None,
            key.pane,
            stream.generation,
            publisher,
            Vec::new(),
        ) {
            self.broadcast_control(&message);
        }
    }

    /// Pane `pane_id` is ending - it exited, or it is being closed - and every stream that spoke
    /// for it is over. Called while the pane is still live where it can be, so the rows go the
    /// ordinary way and its exit stays the last thing said about it.
    pub(super) fn retire_session_publishers(&mut self, pane_id: PaneId) {
        let connections: Vec<ClientId> = self
            .session_publishers
            .streams
            .iter()
            .filter(|(key, _)| key.pane == pane_id)
            .map(|(_, stream)| stream.connection)
            .collect();
        for connection in connections {
            self.close_session_publisher(connection);
            self.enqueue(
                connection,
                Target::Sender,
                ServerMessage::Error {
                    code: "pane-exited".to_string(),
                    message: format!("pane {pane_id} has exited"),
                    peer: None,
                },
            );
            self.set_close_after_flush(connection);
        }
    }

    /// Pass a row an attached client activated to the stream that published it.
    ///
    /// Only a row that is in the pane now, published by a stream this server holds for that very
    /// generation, reaches a publisher: a client cannot make one act on a row it invented. A client
    /// that may not type - a viewer, or anyone but the controller under input lock - cannot make a
    /// program in the pane act either.
    pub(super) fn activate_published_row(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        generation: u64,
        row: RowKey,
    ) {
        if !self.client_may_input(client_id) {
            return;
        }
        let Some((SESSION_PUBLISHER_NAMESPACE, publisher)) =
            protocol::publisher_owner(&row.publisher)
        else {
            return;
        };
        let shown = self.panes.get(&pane_id).is_some_and(|pane| {
            pane.generation == generation
                && pane
                    .runtime
                    .rows
                    .iter()
                    .any(|shown| shown.publisher == row.publisher && shown.id == row.id)
        });
        if !shown {
            return;
        }
        let key = StreamKey {
            pane: pane_id,
            publisher: publisher.to_string(),
        };
        let Some(connection) = self
            .session_publishers
            .streams
            .get(&key)
            .filter(|stream| stream.generation == generation)
            .map(|stream| stream.connection)
        else {
            return;
        };
        self.enqueue(
            connection,
            Target::Sender,
            ServerMessage::PublishActivation { row: row.id },
        );
    }
}
