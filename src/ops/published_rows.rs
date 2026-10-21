//! The pane side of published activity rows.
//!
//! A program that runs several agents or background tasks behind one terminal opens a `publish`
//! stream and keeps it open: it writes a row list whenever its own state changes, and reads back the
//! activations a user clicks in the sidebar. The stream's lifetime *is* the rows' lifetime, so a
//! publisher that exits or crashes withdraws its rows without needing to say so.
//!
//! A pane may have several publishers at once - an agent's session list beside a CI watcher, say.
//! Each owns only its own rows: opening, closing, crashing, or being dropped for backpressure never
//! touches another publisher's.

use tui_lipan::prelude::*;

use crate::app::AppRoot;
use crate::session::protocol::{PublishedRow, PublisherKind, RowKey, SessionInstanceId};
use crate::state::{PaneId, PublishOrigin, PublishStreamKey, PublisherIdentity};

/// What a `publish` request asked for, beside the channels that carry its stream.
pub(crate) struct PublishOpen {
    pub stream_id: u64,
    pub requested_pane: Option<PaneId>,
    /// The caller's `source_session`: the session its pane runs in, when it said.
    pub source_session: Option<SessionInstanceId>,
    pub kind: PublisherKind,
    pub name: Option<String>,
    pub extension: Option<crate::config::ExtensionProvenance>,
    pub worker: Option<crate::state::WorkerId>,
}

/// Longest `--name` a publisher may give itself.
const MAX_PUBLISHER_NAME: usize = 64;

/// Whether `name` is an acceptable publisher name: the same alphabet as extension ids.
pub(crate) fn publisher_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PUBLISHER_NAME
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

/// The stable identity a publisher has within its pane. See [`PublisherIdentity`].
pub(crate) fn publisher_id(extension: Option<&str>, name: Option<&str>) -> String {
    match (extension, name) {
        (Some(extension), None) => extension.to_string(),
        (Some(extension), Some(name)) => format!("{extension}.{name}"),
        (None, name) => format!("publish:{}", name.unwrap_or("default")),
    }
}

/// Register a newly opened stream, replacing any stream the same publisher had open in that pane.
///
/// A second stream with the same identity means the first is stale - a publisher that restarted
/// inside a `keep_open` pane, say. Dropping the old sender closes it. Streams of other publishers in
/// the pane are untouched.
pub(crate) fn stream_opened(
    ctx: &mut Context<AppRoot>,
    open: PublishOpen,
    sender: std::sync::mpsc::SyncSender<String>,
    ack: std::sync::mpsc::Sender<std::result::Result<PaneId, crate::control::ControlResponse>>,
) -> Update {
    use crate::control::ControlResponse;
    let PublishOpen {
        stream_id,
        requested_pane,
        source_session,
        kind,
        name,
        extension,
        worker,
    } = open;
    if extension.as_ref().is_some_and(|provenance| {
        !crate::config::provenance_is_active(&ctx.state.extension_generations, provenance)
    }) {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::ExtensionInactive,
            "extension is not active",
        )));
        return Update::none();
    }
    if name
        .as_deref()
        .is_some_and(|name| !publisher_name_is_valid(name))
    {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::InvalidArgument,
            format!(
                "publisher name must be 1-{MAX_PUBLISHER_NAME} lowercase letters, digits, `-`, or `_`"
            ),
        )));
        return Update::none();
    }
    // A placed worker publishes only into a pane of a session within its binding, and its rows
    // keep flowing only while that session is the one on screen: pane ids repeat across sessions,
    // and a later report must never land in whichever session happens to be showing then.
    let binding = match worker {
        None => None,
        Some(worker) => {
            let admitted =
                crate::ops::extension_workers::live_worker(&ctx.state, worker).and_then(|worker| {
                    let request = crate::control::ControlRequest {
                        command: crate::control::ControlCommand::Publish {
                            kind,
                            name: name.clone(),
                        },
                        source_pane: requested_pane,
                        source_session: None,
                        extension: None,
                        caller_context: None,
                        credential: None,
                    };
                    crate::ops::extension_workers::admit(&ctx.state, worker, &request)
                        .map(|_| worker.binding.clone())
                });
            match admitted {
                Ok(binding) => Some(binding),
                Err(response) => {
                    let _ = ack.send(Err(response));
                    return Update::none();
                }
            }
        }
    };
    let Some(pane_id) = requested_pane.or_else(|| ctx.state.focused_pane()) else {
        let _ = ack.send(Err(ControlResponse::error("publish requires a live pane")));
        return Update::none();
    };
    let origin = match publish_origin(&ctx.state, pane_id, source_session) {
        Ok(origin) => origin,
        Err(response) => {
            let _ = ack.send(Err(response));
            return Update::none();
        }
    };
    // A placed worker's binding covers where its rows land, not just the session on screen when it
    // asked: a caller-named session could otherwise be another session, even on another host.
    if binding
        .as_ref()
        .is_some_and(|binding| !worker_may_publish(&ctx.state, binding, &origin))
    {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::OutOfScope,
            "a placed process may only publish into the session on screen within its placement",
        )));
        return Update::none();
    }
    if target(&ctx.state, &origin, pane_id).is_none() {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::PaneNotFound,
            format!("pane {pane_id} not found"),
        )));
        return Update::none();
    }
    let identity = PublisherIdentity {
        id: publisher_id(
            extension.as_ref().map(|provenance| provenance.id.as_str()),
            name.as_deref(),
        ),
        kind,
    };
    let key = PublishStreamKey {
        origin,
        pane: pane_id,
        publisher: identity.id.clone(),
    };
    // Admission counts open streams, not streams that have published: a stream refused here hears
    // so before its acknowledgement, where a refusal of its rows later could not reach it at all.
    // A publisher replacing its own stream takes no new place.
    let open = ctx
        .state
        .publish_streams
        .keys()
        .filter(|other| {
            other.origin == key.origin && other.pane == pane_id && other.publisher != key.publisher
        })
        .count();
    if open >= crate::session::protocol::MAX_PANE_PUBLISHERS {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::Conflict,
            format!(
                "pane {pane_id} already has {} publishers",
                crate::session::protocol::MAX_PANE_PUBLISHERS
            ),
        )));
        return Update::none();
    }
    ctx.state.publish_streams.insert(
        key,
        crate::state::PublishStreamState {
            id: stream_id,
            identity,
            last_rows: Vec::new(),
            sender,
            extension,
            binding,
        },
    );
    let _ = ack.send(Ok(pane_id));
    Update::none()
}

/// The namespace a new stream for `pane_id` belongs to.
///
/// A caller that names its session is bound to it, current or in the background, and refused when
/// that session is not attached here: guessing would put its rows on another session's pane with
/// the same id. A caller that names none - a supervised service, say - is bound to the session on
/// screen when it opens, and keeps that session after the user switches away.
fn publish_origin(
    state: &crate::state::State,
    pane_id: PaneId,
    source_session: Option<SessionInstanceId>,
) -> std::result::Result<PublishOrigin, crate::control::ControlResponse> {
    if let Some(instance) = source_session {
        return if attachment_for_session(state, &instance).is_some() {
            Ok(PublishOrigin::Session(instance))
        } else {
            Err(crate::control::ControlResponse::error_with(
                crate::control::ControlErrorCode::SessionNotAttached,
                "the calling pane's session is not attached to this rozi",
            ))
        };
    }
    if crate::pane::lifecycle::pane_is_local(state, pane_id) {
        return Ok(PublishOrigin::Local);
    }
    Ok(current_origin(state))
}

/// Whether a placed worker's rows may land in `origin`.
///
/// A placed process acts on a session only while that session is on screen and within its
/// placement, the rule every pane command it sends follows. So the destination must be the session
/// on screen, and the session on screen must lie within the binding - checked against where the
/// stream is bound, never just against whatever is showing, so neither a caller-named background
/// session nor a later switch to another in-scope session redirects its rows.
fn worker_may_publish(
    state: &crate::state::State,
    binding: &crate::state::WorkerBinding,
    origin: &PublishOrigin,
) -> bool {
    let on_screen = match origin {
        PublishOrigin::Local => false,
        PublishOrigin::Session(instance) => {
            state.current().session_instance.as_ref() == Some(instance)
        }
        PublishOrigin::Attachment(epoch) => *epoch == state.runtime_epoch,
    };
    on_screen && crate::ops::extension_workers::in_scope(state, binding)
}

/// The namespace of the session on screen.
fn current_origin(state: &crate::state::State) -> PublishOrigin {
    match &state.current().session_instance {
        Some(instance) => PublishOrigin::Session(instance.clone()),
        None => PublishOrigin::Attachment(state.runtime_epoch),
    }
}

fn attachment_for_session<'a>(
    state: &'a crate::state::State,
    instance: &SessionInstanceId,
) -> Option<&'a crate::state::Attachment> {
    std::iter::once(state.current())
        .chain(state.background.values())
        .find(|attachment| attachment.session_instance.as_ref() == Some(instance))
}

/// Where a stream's rows go: the pane's generation, the connection to its session server when
/// there is one, and whether the pane is this client's own. `None` once the pane or its session is
/// gone from here.
fn target(
    state: &crate::state::State,
    origin: &PublishOrigin,
    pane_id: PaneId,
) -> Option<(u64, Option<crate::session::client::SessionClient>, bool)> {
    let attachment = match origin {
        PublishOrigin::Local => {
            let pane = crate::pane::lifecycle::find_pane_in_namespace(state, pane_id, true)?;
            return Some((
                pane.pty_generation,
                state.pty_client_for_pane(pane_id),
                true,
            ));
        }
        PublishOrigin::Session(instance) => attachment_for_session(state, instance)?,
        PublishOrigin::Attachment(epoch) => state.attachment_for_epoch(*epoch)?,
    };
    let pane = attachment
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == pane_id && !pane.closing)?;
    Some((
        pane.pty_generation,
        attachment.session_client.clone(),
        false,
    ))
}

/// The key of the live stream `stream_id` in `pane_id`, if it has not been replaced or dropped.
fn live_stream(
    state: &crate::state::State,
    stream_id: u64,
    pane_id: PaneId,
) -> Option<PublishStreamKey> {
    state
        .publish_streams
        .iter()
        .find(|(key, stream)| key.pane == pane_id && stream.id == stream_id)
        .map(|(key, _)| key.clone())
}

/// Publish one publisher's rows to the session server, which owns their run clocks and broadcasts
/// them to every attached client.
pub(crate) fn rows_reported(
    ctx: &mut Context<AppRoot>,
    stream_id: u64,
    pane_id: PaneId,
    rows: Vec<PublishedRow>,
) -> Update {
    let Some(key) = live_stream(&ctx.state, stream_id, pane_id) else {
        return Update::none();
    };
    let Some(stream) = ctx.state.publish_streams.get_mut(&key) else {
        return Update::none();
    };
    stream.last_rows = rows;
    send_latest(&ctx.state, &key);
    Update::none()
}

/// Send a stream's latest snapshot to the session it is bound to, if it may reach it now.
fn send_latest(state: &crate::state::State, key: &PublishStreamKey) {
    let Some(stream) = state.publish_streams.get(key) else {
        return;
    };
    if stream
        .binding
        .as_ref()
        .is_some_and(|binding| !worker_may_publish(state, binding, &key.origin))
    {
        return;
    }
    report_rows(state, key, &stream.identity, stream.last_rows.clone());
}

/// Report every live stream's latest snapshot again, after this client attached to a session.
///
/// The server keeps a client's rows only while that client is attached, so a reconnect would
/// otherwise leave every live publisher's rows withdrawn until it next changed them. A stream
/// whose rows are unchanged on a server that still has them costs the server one comparison.
pub(crate) fn resend_rows(state: &crate::state::State) {
    for key in state.publish_streams.keys() {
        send_latest(state, key);
    }
}

/// Send one publisher's rows to the session its stream is bound to. A session no longer attached
/// here gets nothing: the rows must never land on whichever pane shares the id now.
fn report_rows(
    state: &crate::state::State,
    key: &PublishStreamKey,
    publisher: &PublisherIdentity,
    rows: Vec<PublishedRow>,
) {
    if let Some((generation, Some(client), local)) = target(state, &key.origin, key.pane) {
        client.report_pane_rows(key.pane, generation, local, publisher, rows);
    }
}

/// Drop one stream and withdraw its rows, leaving the pane's other publishers alone.
fn drop_stream(state: &mut crate::state::State, key: &PublishStreamKey) {
    if let Some(stream) = state.publish_streams.remove(key) {
        report_rows(state, key, &stream.identity, Vec::new());
    }
}

/// Withdraw a publisher's rows because it went away.
pub(crate) fn stream_closed(ctx: &mut Context<AppRoot>, stream_id: u64, pane_id: PaneId) -> Update {
    if let Some(key) = live_stream(&ctx.state, stream_id, pane_id) {
        drop_stream(&mut ctx.state, &key);
    }
    Update::none()
}

/// Drop streams owned by extensions that disappeared or materially changed during reload.
pub(crate) fn unload_extensions(
    ctx: &mut Context<AppRoot>,
    stale_extensions: &std::collections::HashSet<String>,
) -> Update {
    let stale: Vec<_> = ctx
        .state
        .publish_streams
        .iter()
        .filter(|(_, stream)| {
            stream
                .extension
                .as_ref()
                .is_some_and(|provenance| stale_extensions.contains(&provenance.id))
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in &stale {
        drop_stream(&mut ctx.state, key);
    }
    if stale.is_empty() {
        Update::none()
    } else {
        Update::full()
    }
}

/// Ask the publisher that owns `row`, in a pane on screen, to bring it on screen.
///
/// Best-effort by design: rozi cannot move another program's view itself, and a publisher that
/// has stopped reading is indistinguishable from one that is slow. The pane is focused either way,
/// so the click is never a no-op.
pub(crate) fn request_activation(state: &mut crate::state::State, pane_id: PaneId, row: &RowKey) {
    // The server qualifies each publisher by the client whose stream it is. Another client's row
    // has no stream here, whatever its publisher is called in that client.
    let Some((owner, publisher)) = crate::session::protocol::publisher_owner(&row.publisher) else {
        return;
    };
    let origin = if crate::pane::lifecycle::pane_is_local(state, pane_id) {
        // A scratch or popup pane is this client's alone, on whichever connection it used.
        PublishOrigin::Local
    } else {
        if state
            .current()
            .shared
            .as_ref()
            .is_none_or(|shared| shared.client_id != owner)
        {
            return;
        }
        current_origin(state)
    };
    let key = PublishStreamKey {
        origin,
        pane: pane_id,
        publisher: publisher.to_string(),
    };
    let Some(stream) = state.publish_streams.get(&key) else {
        return;
    };
    let line = format!("{}\n", serde_json::json!({ "activate": row.id }));
    // A publisher that is not draining its activations is either wedged or gone; either way the
    // stream is no longer useful, so drop it rather than blocking the UI thread on it. Only its
    // own rows go with it.
    if stream.sender.try_send(line).is_err() {
        drop_stream(state, &key);
    }
}

#[cfg(test)]
mod tests {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{
        ClientMessage, PublishedRow, PublisherKind, RowKey, SessionInstanceId,
    };
    use crate::state::{PublishOrigin, PublishStreamKey};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn row(id: &str, status: &str) -> PublishedRow {
        PublishedRow {
            publisher: String::new(),
            kind: PublisherKind::Activity,
            live_session: None,
            reported: false,
            id: id.into(),
            title: format!("tab {id}"),
            status: status.into(),
            reason: None,
            active: true,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        }
    }

    /// Run on a worker thread with a large stack, matching the other `TestBackend` control tests.
    fn with_backend(body: impl FnOnce(&mut TestBackend<crate::AppRoot>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                body(&mut backend);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn attach(backend: &mut TestBackend<crate::AppRoot>) -> mpsc::Receiver<ClientOutbound> {
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        state.current_mut().session_instance = Some(SessionInstanceId::for_test("session-a"));
        state.current_mut().shared = Some(crate::state::SharedSessionState::new(CLIENT));
        state.current_mut().workspaces[0].panes[0].pty_generation = 4;
        outbound
    }

    /// This client's id in the session `attach` sets up.
    const CLIENT: u64 = 7;

    /// A row of `publisher` as the server reports it back: qualified by the client.
    fn row_key(client: u64, publisher: &str, id: &str) -> RowKey {
        RowKey::new(
            crate::session::protocol::qualified_publisher(client, publisher),
            id,
        )
    }

    /// The key of `publisher`'s stream in pane 1 of the session `attach` set up.
    fn key(publisher: &str) -> PublishStreamKey {
        PublishStreamKey {
            origin: PublishOrigin::Session(SessionInstanceId::for_test("session-a")),
            pane: 1,
            publisher: publisher.to_string(),
        }
    }

    struct Open<'a> {
        stream_id: u64,
        pane: Option<crate::state::PaneId>,
        session: Option<&'a str>,
        extension: Option<&'a str>,
        kind: PublisherKind,
        name: Option<&'a str>,
    }

    impl Open<'_> {
        fn anonymous(stream_id: u64) -> Self {
            Open {
                stream_id,
                pane: Some(1),
                session: None,
                extension: None,
                kind: PublisherKind::Activity,
                name: None,
            }
        }
    }

    fn try_open(
        backend: &mut TestBackend<crate::AppRoot>,
        open: Open<'_>,
        sender: std::sync::mpsc::SyncSender<String>,
    ) -> std::result::Result<crate::state::PaneId, crate::control::ControlResponse> {
        let (ack, reply) = mpsc::channel();
        let extension = open.extension.map(|id| crate::config::ExtensionProvenance {
            id: id.to_string(),
            generation: backend.state().extension_generations[id].clone(),
        });
        backend
            .dispatch(crate::Msg::PublishStreamOpen {
                stream_id: open.stream_id,
                requested_pane: open.pane,
                source_session: open.session.map(SessionInstanceId::for_test),
                kind: open.kind,
                name: open.name.map(str::to_string),
                extension,
                sender,
                ack,
                worker: None,
            })
            .expect("dispatch stream open");
        reply.recv().unwrap()
    }

    fn open_stream(
        backend: &mut TestBackend<crate::AppRoot>,
        open: Open<'_>,
        sender: std::sync::mpsc::SyncSender<String>,
    ) {
        assert_eq!(try_open(backend, open, sender), Ok(1));
    }

    fn report(backend: &mut TestBackend<crate::AppRoot>, stream_id: u64, rows: Vec<PublishedRow>) {
        backend
            .dispatch(crate::Msg::PublishedRowsReported {
                stream_id,
                pane_id: 1,
                rows,
            })
            .expect("dispatch rows");
    }

    /// Every `ReportPaneRows` sent so far, as `(publisher, kind, row ids)`.
    fn reports(
        outbound: &mpsc::Receiver<ClientOutbound>,
    ) -> Vec<(String, PublisherKind, Vec<String>)> {
        outbound
            .try_iter()
            .filter_map(|message| match message {
                ClientOutbound::Control(ClientMessage::ReportPaneRows {
                    publisher,
                    kind,
                    rows,
                    ..
                }) => Some((
                    publisher,
                    kind,
                    rows.into_iter().map(|row| row.id).collect(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_reported_row_list_reaches_the_session_server() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(1), tx);
            report(backend, 1, vec![row("a", "working")]);
            assert!(outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::ReportPaneRows {
                    pane_id: 1,
                    local: false,
                    generation: 4,
                    ref publisher,
                    kind: PublisherKind::Activity,
                    rows,
                }) if publisher == "publish:default" && rows.len() == 1 && rows[0].id == "a"
            )));
        });
    }

    #[test]
    fn publisher_identity_is_namespaced_by_extension_and_name() {
        assert_eq!(super::publisher_id(None, None), "publish:default");
        assert_eq!(super::publisher_id(None, Some("prs")), "publish:prs");
        assert_eq!(super::publisher_id(Some("git-tools"), None), "git-tools");
        assert_eq!(
            super::publisher_id(Some("git-tools"), Some("prs")),
            "git-tools.prs"
        );
        assert!(super::publisher_name_is_valid("ci_watch-2"));
        for name in ["", "Prs", "a.b", "a:b", "a b", &"x".repeat(65)] {
            assert!(!super::publisher_name_is_valid(name), "{name:?}");
        }
    }

    #[test]
    fn an_invalid_publisher_name_is_refused() {
        with_backend(|backend| {
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            let refused = try_open(
                backend,
                Open {
                    name: Some("Not.Valid"),
                    ..Open::anonymous(1)
                },
                tx,
            );
            assert_eq!(
                refused.unwrap_err().code,
                Some(crate::control::ControlErrorCode::InvalidArgument)
            );
            assert!(backend.state().publish_streams.is_empty());
        });
    }

    /// Closing the stream is how a publisher withdraws, including by dying, so it must reach the
    /// server as an explicit empty list rather than being left for a timeout to notice.
    #[test]
    fn closing_a_stream_withdraws_the_publishers_rows() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(1), tx);
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 1,
                    pane_id: 1,
                })
                .expect("dispatch stream close");
            assert_eq!(
                reports(&outbound),
                [(
                    "publish:default".into(),
                    PublisherKind::Activity,
                    Vec::new()
                )]
            );
        });
    }

    #[test]
    fn activating_a_row_writes_to_its_publisher() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            open_stream(backend, Open::anonymous(1), tx);
            // Another client's publisher of the same name has no stream here.
            super::request_activation(
                backend.state_mut(),
                1,
                &row_key(CLIENT + 1, "publish:default", "ses_b"),
            );
            assert!(
                rx.try_recv().is_err(),
                "another client's row is not this stream's"
            );
            super::request_activation(
                backend.state_mut(),
                1,
                &row_key(CLIENT, "publish:default", "ses_b"),
            );
            let line = rx.try_recv().expect("activation was written");
            assert_eq!(line.trim(), r#"{"activate":"ses_b"}"#);
        });
    }

    /// Two publishers in one pane, both using row id `1`: each activation reaches the stream that
    /// owns the row, and closing or evicting one never withdraws the other's rows.
    #[test]
    fn publishers_sharing_a_pane_own_their_rows_and_activations() {
        with_backend(|backend| {
            let outbound = attach(backend);
            backend.state_mut().extension_generations.insert(
                "codex-rozi-sessions".to_string(),
                "generation-a".to_string(),
            );
            let (agent_tx, agent_rx) = std::sync::mpsc::sync_channel(4);
            open_stream(
                backend,
                Open {
                    extension: Some("codex-rozi-sessions"),
                    kind: PublisherKind::Agent,
                    ..Open::anonymous(1)
                },
                agent_tx,
            );
            let (prs_tx, prs_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(
                backend,
                Open {
                    name: Some("prs"),
                    ..Open::anonymous(2)
                },
                prs_tx,
            );
            assert_eq!(backend.state().publish_streams.len(), 2);
            report(backend, 1, vec![row("1", "working")]);
            report(backend, 2, vec![row("1", "idle")]);
            assert_eq!(
                reports(&outbound),
                [
                    (
                        "codex-rozi-sessions".into(),
                        PublisherKind::Agent,
                        vec!["1".to_string()]
                    ),
                    (
                        "publish:prs".into(),
                        PublisherKind::Activity,
                        vec!["1".to_string()]
                    ),
                ]
            );

            super::request_activation(
                backend.state_mut(),
                1,
                &row_key(CLIENT, "codex-rozi-sessions", "1"),
            );
            assert_eq!(agent_rx.try_recv().unwrap().trim(), r#"{"activate":"1"}"#);
            assert!(
                prs_rx.try_recv().is_err(),
                "the other publisher hears nothing"
            );

            // The PR publisher stops draining: it is evicted, and only its rows are withdrawn.
            let prs = row_key(CLIENT, "publish:prs", "1");
            super::request_activation(backend.state_mut(), 1, &prs);
            super::request_activation(backend.state_mut(), 1, &prs);
            assert_eq!(
                reports(&outbound),
                [("publish:prs".into(), PublisherKind::Activity, Vec::new())]
            );
            assert!(
                backend
                    .state()
                    .publish_streams
                    .contains_key(&key("codex-rozi-sessions"))
            );

            // The agent publisher closing withdraws its own rows and nothing else.
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 1,
                    pane_id: 1,
                })
                .expect("close");
            assert_eq!(
                reports(&outbound),
                [(
                    "codex-rozi-sessions".into(),
                    PublisherKind::Agent,
                    Vec::new()
                )]
            );
            assert!(backend.state().publish_streams.is_empty());
            drop(prs_rx);
        });
    }

    /// The pane's publisher limit is enforced when a stream opens, counting streams that have not
    /// published anything yet, so the refusal reaches the publisher. A publisher replacing its own
    /// stream at the limit is still admitted.
    #[test]
    fn a_publisher_beyond_the_panes_limit_is_refused_when_it_opens() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let names: Vec<String> = (0..crate::session::protocol::MAX_PANE_PUBLISHERS)
                .map(|index| format!("p{index}"))
                .collect();
            let mut receivers = Vec::new();
            for (index, name) in names.iter().enumerate() {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                receivers.push(rx);
                open_stream(
                    backend,
                    Open {
                        name: Some(name),
                        ..Open::anonymous(index as u64 + 1)
                    },
                    tx,
                );
            }
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            let refused = try_open(
                backend,
                Open {
                    name: Some("one-too-many"),
                    ..Open::anonymous(100)
                },
                tx,
            );
            assert_eq!(
                refused.unwrap_err().code,
                Some(crate::control::ControlErrorCode::Conflict)
            );
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(
                backend,
                Open {
                    name: Some("p0"),
                    ..Open::anonymous(101)
                },
                tx,
            );
            assert_eq!(
                backend.state().publish_streams.len(),
                crate::session::protocol::MAX_PANE_PUBLISHERS
            );
        });
    }

    /// Pane ids repeat across sessions. A stream opened in session A's pane 1 keeps reporting to
    /// session A after the user switches to session B, which has a pane 1 of its own; B's publisher
    /// of the same identity is a different stream; and a caller naming a session that is not
    /// attached here is refused rather than guessed.
    #[test]
    fn a_stream_stays_with_the_session_it_was_opened_in() {
        with_backend(|backend| {
            let outbound_a = attach(backend);
            let (tx_a, _rx_a) = std::sync::mpsc::sync_channel(4);
            open_stream(
                backend,
                Open {
                    session: Some("session-a"),
                    ..Open::anonymous(1)
                },
                tx_a,
            );

            // Switch to session B, whose own pane 1 is now the one on screen.
            let (client_b, outbound_b) = SessionClient::test_channel();
            let mut b = crate::state::Attachment::new();
            b.session_attached = true;
            b.session_client = Some(client_b);
            b.session_instance = Some(SessionInstanceId::for_test("session-b"));
            b.shared = Some(crate::state::SharedSessionState::new(8));
            b.workspaces[0].panes.push(crate::state::Pane::new(
                1,
                100,
                tui_lipan::prelude::FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 20.0,
                    h: 10.0,
                },
            ));
            let state = backend.state_mut();
            let epoch = state.runtime_epoch;
            state.park_current(epoch, b);
            state.runtime_epoch = epoch + 1;

            report(backend, 1, vec![row("a", "working")]);
            assert_eq!(
                reports(&outbound_a),
                [(
                    "publish:default".into(),
                    PublisherKind::Activity,
                    vec!["a".to_string()]
                )],
                "rows go to the session the stream was opened in"
            );
            assert!(reports(&outbound_b).is_empty(), "never to B's pane 1");

            // B's publisher with the same identity is its own stream and replaces nothing in A.
            let (tx_b, rx_b) = std::sync::mpsc::sync_channel(4);
            open_stream(
                backend,
                Open {
                    session: Some("session-b"),
                    ..Open::anonymous(2)
                },
                tx_b,
            );
            assert_eq!(backend.state().publish_streams.len(), 2);
            // An activation from the screen reaches B's stream, the one on screen.
            super::request_activation(backend.state_mut(), 1, &row_key(8, "publish:default", "b"));
            assert_eq!(rx_b.try_recv().unwrap().trim(), r#"{"activate":"b"}"#);

            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            let refused = try_open(
                backend,
                Open {
                    session: Some("session-gone"),
                    ..Open::anonymous(3)
                },
                tx,
            );
            assert_eq!(
                refused.unwrap_err().code,
                Some(crate::control::ControlErrorCode::SessionNotAttached)
            );
        });
    }

    /// A placed worker bound to host B cannot reach session A, on another host, by naming A's
    /// instance; its stream opened in B stays with B, and its rows are dropped rather than
    /// redirected while another session is on screen.
    #[test]
    fn a_placed_worker_publishes_only_within_its_placement_on_screen() {
        use crate::session::remote::RemoteTarget;
        use crate::state::{HostKey, WorkerBinding, WorkerKind, WorkerRuntime};
        with_backend(|backend| {
            let host = |name: &str| HostKey::Remote(RemoteTarget::Alias(name.to_string()));
            let pane = || {
                crate::state::Pane::new(
                    1,
                    100,
                    tui_lipan::prelude::FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 20.0,
                        h: 10.0,
                    },
                )
            };
            // Session A on host `alpha`, parked in the background with its own pane 1.
            let outbound_a = attach(backend);
            backend.state_mut().current_mut().remote_target =
                Some(RemoteTarget::Alias("alpha".to_string()));
            // Session B on host `beta`, on screen, also with a pane 1.
            let (client_b, outbound_b) = SessionClient::test_channel();
            let mut b = crate::state::Attachment::new();
            b.session_attached = true;
            b.session_client = Some(client_b);
            b.remote_target = Some(RemoteTarget::Alias("beta".to_string()));
            b.session_instance = Some(SessionInstanceId::for_test("session-b"));
            b.workspaces[0].panes.push(pane());
            let state = backend.state_mut();
            let epoch = state.runtime_epoch;
            state.park_current(epoch, b);
            state.runtime_epoch = epoch + 1;
            state
                .extension_generations
                .insert("probe".to_string(), "g1".to_string());
            let worker = state
                .extension_workers
                .issue(
                    crate::config::ExtensionProvenance {
                        id: "probe".to_string(),
                        generation: "g1".to_string(),
                    },
                    crate::config::Placement::EachHost,
                    WorkerBinding {
                        host: host("beta"),
                        session: None,
                    },
                    WorkerRuntime(1),
                    WorkerKind::Service {
                        name: "probe.watch".to_string(),
                    },
                )
                .id;
            let open_as_worker = |backend: &mut TestBackend<crate::AppRoot>,
                                  stream_id: u64,
                                  session: Option<&str>| {
                let (tx, _rx) = std::sync::mpsc::sync_channel(4);
                let (ack, reply) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::PublishStreamOpen {
                        stream_id,
                        requested_pane: Some(1),
                        source_session: session.map(SessionInstanceId::for_test),
                        kind: PublisherKind::Agent,
                        name: None,
                        extension: Some(crate::config::ExtensionProvenance {
                            id: "probe".to_string(),
                            generation: "g1".to_string(),
                        }),
                        sender: tx,
                        ack,
                        worker: Some(worker),
                    })
                    .expect("dispatch stream open");
                reply.recv().unwrap()
            };

            let refused = open_as_worker(backend, 1, Some("session-a"));
            assert_eq!(
                refused.unwrap_err().code,
                Some(crate::control::ControlErrorCode::OutOfScope),
                "naming A's instance must not reach A on another host"
            );
            assert!(backend.state().publish_streams.is_empty());

            assert_eq!(open_as_worker(backend, 2, None), Ok(1));
            report(backend, 2, vec![row("r", "working")]);
            assert_eq!(
                reports(&outbound_b),
                [("probe".into(), PublisherKind::Agent, vec!["r".to_string()])]
            );

            // A is brought on screen: B's stream keeps its binding and says nothing to A.
            let state = backend.state_mut();
            let current = state.runtime_epoch;
            state.runtime_epoch = state.unpark(epoch, current).expect("switch back to A");
            report(backend, 2, vec![row("r", "idle")]);
            assert!(reports(&outbound_a).is_empty(), "never into A");
            assert!(reports(&outbound_b).is_empty(), "B is not on screen");
        });
    }

    /// The server withdraws a client's rows when it leaves, so attaching again resends every live
    /// stream's latest snapshot.
    #[test]
    fn live_streams_resend_their_rows_after_an_attach() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(1), tx);
            report(backend, 1, vec![row("a", "working")]);
            assert_eq!(reports(&outbound).len(), 1);
            super::resend_rows(backend.state());
            assert_eq!(
                reports(&outbound),
                [(
                    "publish:default".into(),
                    PublisherKind::Activity,
                    vec!["a".to_string()]
                )]
            );
        });
    }

    /// A publisher restarting in the same pane replaces its own stale stream, and only that one.
    #[test]
    fn a_restarted_publisher_replaces_only_its_own_stream() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (other_tx, other_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(
                backend,
                Open {
                    name: Some("ci"),
                    ..Open::anonymous(1)
                },
                other_tx,
            );
            let (old_tx, old_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(2), old_tx);
            let (new_tx, _new_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(3), new_tx);
            assert!(matches!(
                old_rx.recv_timeout(std::time::Duration::from_secs(1)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            assert!(matches!(
                other_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            let streams = &backend.state().publish_streams;
            assert_eq!(streams[&key("publish:default")].id, 3);
            assert_eq!(streams[&key("publish:ci")].id, 1);
            // The replaced stream's late close is ignored.
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 2,
                    pane_id: 1,
                })
                .expect("stale close");
            assert!(reports(&outbound).is_empty());
        });
    }

    #[test]
    fn stale_events_from_replaced_publisher_generation_are_ignored() {
        with_backend(|backend| {
            let outbound = attach(backend);
            backend
                .state_mut()
                .extension_generations
                .insert("dashboard".to_string(), "generation-a".to_string());
            let dashboard = |stream_id| Open {
                extension: Some("dashboard"),
                ..Open::anonymous(stream_id)
            };
            let (old_tx, old_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, dashboard(1), old_tx);
            let (new_tx, _new_rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, dashboard(2), new_tx);
            assert!(matches!(
                old_rx.recv_timeout(std::time::Duration::from_secs(1)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));

            report(backend, 1, vec![row("stale", "working")]);
            backend
                .dispatch(crate::Msg::PublishStreamClosed {
                    stream_id: 1,
                    pane_id: 1,
                })
                .expect("stale close");
            assert_eq!(backend.state().publish_streams[&key("dashboard")].id, 2);
            assert!(reports(&outbound).is_empty());

            report(backend, 2, vec![row("current", "working")]);
            assert_eq!(
                reports(&outbound),
                [(
                    "dashboard".into(),
                    PublisherKind::Activity,
                    vec!["current".to_string()]
                )]
            );
        });
    }

    #[test]
    fn service_publisher_uses_focused_pane_and_unloads_with_its_extension() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            backend
                .state_mut()
                .extension_generations
                .insert("dashboard".to_string(), "generation-a".to_string());
            open_stream(
                backend,
                Open {
                    pane: None,
                    extension: Some("dashboard"),
                    ..Open::anonymous(1)
                },
                tx,
            );
            assert_eq!(
                backend
                    .state_mut()
                    .publish_streams
                    .get(&key("dashboard"))
                    .and_then(|stream| stream.extension.as_ref())
                    .map(|provenance| provenance.id.as_str()),
                Some("dashboard")
            );
            backend
                .dispatch(crate::Msg::RunAction(
                    crate::input::Action::ReloadExtensions,
                ))
                .expect("reload without extension");
            assert!(matches!(
                rx.recv_timeout(std::time::Duration::from_secs(1)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            assert_eq!(
                reports(&outbound),
                [("dashboard".into(), PublisherKind::Activity, Vec::new())]
            );
        });
    }

    #[test]
    fn inactive_extension_cannot_open_a_publish_stream() {
        with_backend(|backend| {
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            let (ack, reply) = mpsc::channel();
            backend
                .dispatch(crate::Msg::PublishStreamOpen {
                    stream_id: 1,
                    requested_pane: None,
                    source_session: None,
                    kind: PublisherKind::Activity,
                    name: None,
                    extension: Some(crate::config::ExtensionProvenance {
                        id: "dashboard".to_string(),
                        generation: "retired".to_string(),
                    }),
                    sender: tx,
                    ack,
                    worker: None,
                })
                .expect("dispatch stream open");
            assert_eq!(
                reply.recv().unwrap().unwrap_err().code,
                Some(crate::control::ControlErrorCode::ExtensionInactive)
            );
            assert!(backend.state().publish_streams.is_empty());
        });
    }
}
