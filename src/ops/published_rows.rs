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
use crate::config::SessionScope;
use crate::pane::id::PaneId;
use crate::session::protocol::{PublishedRow, PublisherKind, RowKey, SessionInstanceId};
use crate::state::{PublishOrigin, PublishStreamKey, PublisherIdentity};

/// What a `publish` request asked for, beside the channels that carry its stream.
pub(crate) struct PublishOpen {
    pub stream_id: u64,
    pub requested_pane: Option<PaneId>,
    /// The caller's `source_session`: the session its pane runs in, when it said.
    pub source_session: Option<SessionInstanceId>,
    pub kind: PublisherKind,
    pub name: Option<String>,
    pub extension: Option<crate::control::ExtensionProvenance>,
    pub worker: Option<crate::control::WorkerId>,
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
                    if worker.session_scope == SessionScope::Attached
                        && let Some(instance) = &source_session
                    {
                        return crate::ops::extension_workers::admit_attached_publish(
                            &ctx.state,
                            worker,
                            instance,
                            requested_pane,
                        )
                        .map(|()| (worker.binding.clone(), worker.session_scope));
                    }
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
                        .map(|_| (worker.binding.clone(), worker.session_scope))
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
    // A session off screen has no focus to fall back on, and the focused pane is another session's.
    if requested_pane.is_none()
        && source_session
            .as_ref()
            .is_some_and(|instance| ctx.state.current().session_instance.as_ref() != Some(instance))
    {
        let _ = ack.send(Err(ControlResponse::error_with(
            crate::control::ControlErrorCode::TargetRequired,
            "publishing into a session off screen requires a pane",
        )));
        return Update::none();
    }
    let session_scope = binding
        .as_ref()
        .map_or_else(SessionScope::default, |(_, scope)| *scope);
    let binding = binding.map(|(binding, _)| binding);
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
        .is_some_and(|binding| !worker_may_publish(&ctx.state, binding, session_scope, &origin))
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
            last_responses: Vec::new(),
            sender,
            extension,
            binding,
            session_scope,
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
///
/// A service declaring [`SessionScope::Attached`] keeps a session's rows current off screen too:
/// the stream names that session, so the rows can only land there, and it must still lie within
/// the binding.
fn worker_may_publish(
    state: &crate::state::State,
    binding: &crate::state::WorkerBinding,
    session_scope: SessionScope,
    origin: &PublishOrigin,
) -> bool {
    if session_scope == SessionScope::Attached
        && let PublishOrigin::Session(instance) = origin
    {
        return attachment_for_session(state, instance).is_some_and(|attachment| {
            binding.admits(
                &crate::state::HostKey::of(attachment.remote_target.as_ref()),
                Some(instance),
            )
        });
    }
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
    responses: Vec<crate::control::PublishedAgentResponse>,
) -> Update {
    let Some(key) = live_stream(&ctx.state, stream_id, pane_id) else {
        return Update::none();
    };
    let generation = target(&ctx.state, &key.origin, pane_id)
        .map(|(generation, _, _)| generation)
        .unwrap_or(u64::MAX);
    let Some(stream) = ctx.state.publish_streams.get_mut(&key) else {
        return Update::none();
    };
    // An update whose own `ReportPaneRows` frame exceeds `MAX_FRAME_SIZE` cannot travel on the
    // session wire at all: neither replace `last_rows` / `last_responses` with it nor push it into
    // the outbound transport where it would disconnect the client.
    if report_wire_bytes(
        pane_id,
        generation,
        &stream.identity,
        stream.id,
        &rows,
        responses.clone(),
    ) > crate::session::protocol::MAX_FRAME_SIZE
    {
        return Update::none();
    }
    if retain_responses(stream, pane_id, generation, &rows, &responses) {
        stream.last_rows = rows;
        send_latest(&mut ctx.state, &key, false, None);
    } else {
        send_latest(&mut ctx.state, &key, false, Some((rows, responses)));
    }
    Update::none()
}

/// On-wire byte length of a `ReportPaneRows` control frame carrying `rows` and `responses`,
/// using the exact accounting of [`crate::session::protocol::control_frame_wire_bytes`].
fn report_wire_bytes(
    pane_id: PaneId,
    generation: u64,
    publisher: &PublisherIdentity,
    epoch: u64,
    rows: &[PublishedRow],
    responses: Vec<crate::control::PublishedAgentResponse>,
) -> usize {
    crate::session::protocol::control_frame_wire_bytes(
        &crate::session::protocol::ClientMessage::ReportPaneRows {
            epoch,
            responses,
            pane_id,
            local: false,
            generation,
            publisher: publisher.id.clone(),
            kind: publisher.kind,
            rows: rows.to_vec(),
        },
    )
}

/// Retain `responses` for replay, if the whole update passes the session server's validation.
///
/// Caller has already verified that `(rows, responses)` fits [`crate::session::protocol::MAX_FRAME_SIZE`].
/// An update is one report, accepted or rejected whole, and so is its replay: nothing of an
/// invalid update (nor its rows) is kept, or a later rows-only update could combine with it or a
/// reconnect could present part of it as valid. A retained response is dropped once its row is
/// gone from the canonical set or has changed status or `native_session`. A response replaces the
/// retained one for its id only with a greater `seq`, as the session server orders them.
///
/// The cache holds at most one update's budget and keeps the complete `ReportPaneRows` frame
/// within [`crate::session::protocol::MAX_FRAME_SIZE`], evicting older cached responses first, so
/// every valid update that fits the transport is retained and a reconnect is always a single valid
/// frame.
fn retain_responses(
    stream: &mut crate::state::PublishStreamState,
    pane_id: PaneId,
    generation: u64,
    rows: &[PublishedRow],
    responses: &[crate::control::PublishedAgentResponse],
) -> bool {
    use crate::session::protocol::{
        MAX_AGENT_RESPONSES_UPDATE_BYTES, MAX_FRAME_SIZE, MAX_RESPONSES_PER_UPDATE,
        canonical_published_row_ids, canonical_row_response_scope, validate_published_responses,
    };
    if validate_published_responses(stream.identity.kind, rows, responses).is_err() {
        return false;
    }
    let canonical = canonical_published_row_ids(rows);
    stream.last_responses.retain(|kept| {
        canonical.contains(&kept.response.id)
            && rows
                .iter()
                .find(|row| row.id == kept.response.id)
                .and_then(canonical_row_response_scope)
                .is_some_and(|(status, native_session)| {
                    status == kept.status && native_session == kept.native_session
                })
    });
    for response in responses {
        let Some((status, native_session)) = rows
            .iter()
            .find(|row| row.id == response.id)
            .and_then(canonical_row_response_scope)
        else {
            continue;
        };
        let new = crate::state::RetainedResponse {
            response: response.clone(),
            status,
            native_session,
            delivered: false,
        };
        let kept = &mut stream.last_responses;
        match kept.iter().position(|old| old.response.id == response.id) {
            Some(index) if kept[index].response.seq >= response.seq => {}
            Some(index) => {
                kept.remove(index);
                kept.push(new);
            }
            None => kept.push(new),
        }
    }
    while !stream.last_responses.is_empty()
        && (stream.last_responses.len() > MAX_RESPONSES_PER_UPDATE
            || stream
                .last_responses
                .iter()
                .map(|kept| kept.response.wire_len())
                .sum::<usize>()
                > MAX_AGENT_RESPONSES_UPDATE_BYTES
            || report_wire_bytes(
                pane_id,
                generation,
                &stream.identity,
                stream.id,
                rows,
                stream
                    .last_responses
                    .iter()
                    .map(|kept| kept.response.clone())
                    .collect(),
            ) > MAX_FRAME_SIZE)
    {
        // Never evict a response belonging to the current update; `(rows, responses)` already fits
        // `MAX_FRAME_SIZE` on its own, so evicting older cached responses is always enough.
        let Some(index) = stream
            .last_responses
            .iter()
            .position(|kept| !responses.iter().any(|fresh| fresh.id == kept.response.id))
        else {
            break;
        };
        stream.last_responses.remove(index);
    }
    true
}

/// Send a stream's latest snapshot to the session it is bound to, if it may reach it now, as one
/// report.
///
/// `raw` is an update that was not retained; it goes exactly as published, alone, so the server
/// rejects it whole. Otherwise the report carries the retained responses not yet delivered, or
/// every retained one when `replay` is set.
fn send_latest(
    state: &mut crate::state::State,
    key: &PublishStreamKey,
    replay: bool,
    raw: Option<(
        Vec<PublishedRow>,
        Vec<crate::control::PublishedAgentResponse>,
    )>,
) {
    let Some(stream) = state.publish_streams.get(key) else {
        return;
    };
    if stream.binding.as_ref().is_some_and(|binding| {
        !worker_may_publish(state, binding, stream.session_scope, &key.origin)
    }) {
        return;
    }
    let sends_raw = raw.is_some();
    let (rows, responses) = raw.unwrap_or_else(|| {
        (
            stream.last_rows.clone(),
            stream
                .last_responses
                .iter()
                .filter(|kept| replay || !kept.delivered)
                .map(|kept| kept.response.clone())
                .collect(),
        )
    });
    let sent = report_rows(state, key, &stream.identity, rows, stream.id, responses);
    if sent
        && !sends_raw
        && let Some(stream) = state.publish_streams.get_mut(key)
    {
        for kept in &mut stream.last_responses {
            kept.delivered = true;
        }
    }
}

/// Report every live stream's latest snapshot again, after this client attached to a session.
///
/// The server keeps a client's rows only while that client is attached, so a reconnect would
/// otherwise leave every live publisher's rows withdrawn until it next changed them. A stream
/// whose rows are unchanged on a server that still has them costs the server one comparison.
pub(crate) fn resend_rows(state: &mut crate::state::State) {
    let keys: Vec<_> = state.publish_streams.keys().cloned().collect();
    for key in &keys {
        send_latest(state, key, true, None);
    }
}

/// Send one publisher's rows to the session its stream is bound to. A session no longer attached
/// here gets nothing: the rows must never land on whichever pane shares the id now.
fn report_rows(
    state: &crate::state::State,
    key: &PublishStreamKey,
    publisher: &PublisherIdentity,
    rows: Vec<PublishedRow>,
    epoch: u64,
    responses: Vec<crate::control::PublishedAgentResponse>,
) -> bool {
    if let Some((generation, Some(client), local)) = target(state, &key.origin, key.pane) {
        client.report_pane_rows(
            key.pane, generation, local, publisher, rows, epoch, responses,
        );
        return true;
    }
    false
}

/// Drop one stream and withdraw its rows, leaving the pane's other publishers alone.
fn drop_stream(state: &mut crate::state::State, key: &PublishStreamKey) {
    if let Some(stream) = state.publish_streams.remove(key) {
        report_rows(
            state,
            key,
            &stream.identity,
            Vec::new(),
            stream.id,
            Vec::new(),
        );
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
    // The server qualifies each publisher by the client whose stream it is.
    let Some((owner, publisher)) = crate::session::protocol::publisher_owner(&row.publisher) else {
        return;
    };
    let local = crate::pane::lifecycle::pane_is_local(state, pane_id);
    let origin = if local {
        // A scratch or popup pane is this client's alone, on whichever connection it used.
        PublishOrigin::Local
    } else {
        current_origin(state)
    };
    let own = local
        || state
            .current()
            .shared
            .as_ref()
            .is_some_and(|shared| shared.client_id == owner);
    let attached = state.current().shared.is_some();
    let key = PublishStreamKey {
        origin,
        pane: pane_id,
        publisher: publisher.to_string(),
    };
    let Some(stream) = state.publish_streams.get(&key) else {
        return;
    };
    // Another client's row has no stream here, whatever its publisher is called in that client -
    // unless it is an agent publisher. Then this stream is this client's copy of the one speaking
    // for the pane, listing the same conversations, and it is the one that can act in this client.
    let copy = attached && stream.identity.kind == PublisherKind::Agent;
    if !(own || copy) {
        return;
    }
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
        crate::test_support::on_large_stack(move || {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            body(&mut backend);
        });
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
        pane: Option<crate::pane::id::PaneId>,
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
    ) -> std::result::Result<crate::pane::id::PaneId, crate::control::ControlResponse> {
        let (ack, reply) = mpsc::channel();
        let extension = open
            .extension
            .map(|id| crate::control::ExtensionProvenance {
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
                responses: Vec::new(),
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
            epoch: _,
            responses: _,
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

    /// The pane lists one window's copy of an agent publisher. Selecting its row here reaches this
    /// window's copy, which lists the same conversations and can act on this window's behalf.
    #[test]
    fn activating_another_windows_agent_row_reaches_this_windows_copy() {
        with_backend(|backend| {
            let _outbound = attach(backend);
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            open_stream(
                backend,
                Open {
                    kind: PublisherKind::Agent,
                    ..Open::anonymous(1)
                },
                tx,
            );
            super::request_activation(
                backend.state_mut(),
                1,
                &row_key(CLIENT + 1, "publish:default", "ses_b"),
            );
            assert_eq!(rx.try_recv().unwrap().trim(), r#"{"activate":"ses_b"}"#);
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

    /// A service declaring `session_scope = "attached"` keeps naming a session's rows while another
    /// is on screen. Pane ids repeat, so it names the session and the pane; without its declaration,
    /// or for a session outside its placement, the same stream is refused.
    #[test]
    fn an_attached_scope_keeps_publishing_into_a_named_session_off_screen() {
        use crate::config::{Placement, SessionScope};
        use crate::control::ExtensionProvenance;
        use crate::session::remote::RemoteTarget;
        use crate::state::{HostKey, WorkerBinding, WorkerKind, WorkerRuntime};
        with_backend(|backend| {
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
            // `parked` is off screen on `beta`, `elsewhere` off screen on `alpha`, and `shown` on
            // screen on `beta`. All three have a pane 1.
            let mut outbound = Vec::new();
            for (instance, host) in [
                ("parked", "beta"),
                ("elsewhere", "alpha"),
                ("shown", "beta"),
            ] {
                let (client, rx) = SessionClient::test_channel();
                let mut attachment = crate::state::Attachment::new();
                attachment.session_attached = true;
                attachment.session_client = Some(client);
                attachment.remote_target = Some(RemoteTarget::Alias(host.to_string()));
                attachment.session_instance = Some(SessionInstanceId::for_test(instance));
                attachment.workspaces[0].panes.push(pane());
                outbound.push(rx);
                let state = backend.state_mut();
                let epoch = state.runtime_epoch;
                state.park_current(epoch, attachment);
                state.runtime_epoch = epoch + 1;
            }
            let state = backend.state_mut();
            state
                .extension_generations
                .insert("probe".to_string(), "g1".to_string());
            let provenance = || ExtensionProvenance {
                id: "probe".to_string(),
                generation: "g1".to_string(),
            };
            let mut issue = |scope| {
                state
                    .extension_workers
                    .issue_scoped(
                        provenance(),
                        Placement::EachHost,
                        WorkerBinding {
                            host: HostKey::Remote(RemoteTarget::Alias("beta".to_string())),
                            session: None,
                        },
                        scope,
                        WorkerRuntime(1),
                        WorkerKind::Service {
                            name: "probe.watch".to_string(),
                        },
                    )
                    .id
            };
            let narrow = issue(SessionScope::Active);
            let wide = issue(SessionScope::Attached);
            let open = |backend: &mut TestBackend<crate::AppRoot>,
                        worker,
                        stream_id: u64,
                        session: &str,
                        requested_pane| {
                let (tx, _rx) = std::sync::mpsc::sync_channel(4);
                let (ack, reply) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::PublishStreamOpen {
                        stream_id,
                        requested_pane,
                        source_session: Some(SessionInstanceId::for_test(session)),
                        kind: PublisherKind::Agent,
                        name: None,
                        extension: Some(provenance()),
                        sender: tx,
                        ack,
                        worker: Some(worker),
                    })
                    .expect("dispatch stream open");
                reply.recv().unwrap().map_err(|response| response.code)
            };
            use crate::control::ControlErrorCode::{OutOfScope, TargetRequired};
            assert_eq!(
                open(backend, narrow, 1, "parked", Some(1)),
                Err(Some(OutOfScope)),
                "the wider reach is declared, never assumed"
            );
            assert_eq!(
                open(backend, wide, 2, "elsewhere", Some(1)),
                Err(Some(OutOfScope)),
                "another host is outside the placement"
            );
            assert_eq!(
                open(backend, wide, 3, "parked", None),
                Err(Some(TargetRequired)),
                "the focused pane belongs to the session on screen"
            );
            assert_eq!(open(backend, wide, 4, "parked", Some(1)), Ok(1));
            assert_eq!(open(backend, wide, 5, "shown", Some(1)), Ok(1));

            report(backend, 4, vec![row("r", "working")]);
            assert_eq!(
                reports(&outbound[0]),
                [("probe".into(), PublisherKind::Agent, vec!["r".to_string()])],
                "the rows reach the session off screen"
            );
            assert!(reports(&outbound[2]).is_empty(), "and not the one showing");
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
                    crate::control::ExtensionProvenance {
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
                        extension: Some(crate::control::ExtensionProvenance {
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
    /// The responses carried by every `ReportPaneRows` sent so far, as `(row id, seq)`.
    fn response_reports(outbound: &mpsc::Receiver<ClientOutbound>) -> Vec<Vec<(String, u64)>> {
        outbound
            .try_iter()
            .filter_map(|message| match message {
                ClientOutbound::Control(ClientMessage::ReportPaneRows { responses, .. }) => {
                    Some(responses.into_iter().map(|r| (r.id, r.seq)).collect())
                }
                _ => None,
            })
            .collect()
    }

    fn report_with_response(
        backend: &mut TestBackend<crate::AppRoot>,
        rows: Vec<PublishedRow>,
        id: &str,
        seq: u64,
    ) {
        backend
            .dispatch(crate::Msg::PublishedRowsReported {
                responses: vec![crate::control::PublishedAgentResponse {
                    id: id.into(),
                    seq,
                    response: crate::control::AgentResponsePayload {
                        text: "answer".into(),
                        turn_id: None,
                        source: crate::control::AgentResponseSource::NativeApi,
                        fidelity: crate::control::AgentResponseFidelity::Semantic,
                        complete: true,
                        truncated: false,
                    },
                }],
                stream_id: 1,
                pane_id: 1,
                rows,
            })
            .expect("dispatch rows");
    }

    fn open_agent_stream(
        backend: &mut TestBackend<crate::AppRoot>,
        stream_id: u64,
        sender: std::sync::mpsc::SyncSender<String>,
    ) {
        open_stream(
            backend,
            Open {
                kind: PublisherKind::Agent,
                ..Open::anonymous(stream_id)
            },
            sender,
        );
    }

    #[test]
    fn a_response_is_replayed_after_an_attach_while_its_row_keeps_its_status() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_agent_stream(backend, 1, tx);
            report_with_response(backend, vec![row("a", "done")], "a", 1);
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("a".to_string(), 1)]]
            );
            // An update that adds nothing does not repeat what was already delivered.
            report(backend, 1, vec![row("a", "done")]);
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );
            // A reconnect is a new client to the server, so the answer goes again.
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("a".to_string(), 1)]]
            );
            // A newer turn ends the answer's lifetime: it is never replayed for it.
            report(backend, 1, vec![row("a", "working")]);
            report(backend, 1, vec![row("a", "done")]);
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );
            // Switching `native_session` on the same row id and status also ends the answer's
            // lifetime so an older conversation's response cannot cross into the new one.
            let mut s1 = row("a", "done");
            s1.native_session = Some("s1".into());
            report_with_response(backend, vec![s1], "a", 2);
            outbound.try_iter().for_each(drop);
            let mut s2 = row("a", "done");
            s2.native_session = Some("s2".into());
            report(backend, 1, vec![s2]);
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );
        });
    }

    #[test]
    fn a_response_published_out_of_reach_goes_with_the_next_report() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_agent_stream(backend, 1, tx);
            let key = backend
                .state()
                .publish_streams
                .keys()
                .next()
                .unwrap()
                .clone();
            let stream = backend.state_mut().publish_streams.get_mut(&key).unwrap();
            stream.last_responses.push(crate::state::RetainedResponse {
                response: crate::control::PublishedAgentResponse {
                    id: "a".into(),
                    seq: 1,
                    response: crate::control::AgentResponsePayload {
                        text: "answer".into(),
                        turn_id: None,
                        source: crate::control::AgentResponseSource::NativeApi,
                        fidelity: crate::control::AgentResponseFidelity::Semantic,
                        complete: true,
                        truncated: false,
                    },
                },
                status: "done".into(),
                native_session: None,
                delivered: false,
            });
            report(backend, 1, vec![row("a", "done")]);
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("a".to_string(), 1)]]
            );
        });
    }

    fn report_responses(
        backend: &mut TestBackend<crate::AppRoot>,
        rows: Vec<PublishedRow>,
        responses: &[(&str, u64, usize)],
    ) {
        backend
            .dispatch(crate::Msg::PublishedRowsReported {
                responses: responses
                    .iter()
                    .map(|(id, seq, bytes)| crate::control::PublishedAgentResponse {
                        id: (*id).into(),
                        seq: *seq,
                        response: crate::control::AgentResponsePayload {
                            text: "x".repeat(*bytes),
                            turn_id: None,
                            source: crate::control::AgentResponseSource::NativeApi,
                            fidelity: crate::control::AgentResponseFidelity::Semantic,
                            complete: true,
                            truncated: false,
                        },
                    })
                    .collect(),
                stream_id: 1,
                pane_id: 1,
                rows,
            })
            .expect("dispatch rows");
    }

    #[test]
    fn a_replay_is_one_report_within_the_update_and_frame_budgets() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_agent_stream(backend, 1, tx);
            let big = crate::session::protocol::MAX_AGENT_RESPONSE_BYTES;
            let ids: Vec<String> = (0..10).map(|n| format!("r{n}")).collect();
            let rows: Vec<_> = ids.iter().map(|id| row(id, "done")).collect();
            for id in &ids {
                report_responses(backend, rows.clone(), &[(id, 1, big)]);
            }
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            let sent = response_reports(&outbound);
            assert_eq!(sent.len(), 1, "a reconnect is a single report");
            assert!(sent[0].len() < 10, "the oldest responses are evicted");
            assert!(
                sent[0].iter().any(|(id, _)| id == "r9"),
                "the newest survive"
            );
            assert!(
                sent[0].len() * big <= crate::session::protocol::MAX_AGENT_RESPONSES_UPDATE_BYTES
            );

            // A later rows-only update with large rows evicts enough retained responses so the
            // complete serialized `ReportPaneRows` still fits within the session frame budget.
            let mut large_rows = rows;
            large_rows[0].cwd = Some("x".repeat(5 * 1024 * 1024));
            report(backend, 1, large_rows);
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            let frames: Vec<(usize, Vec<String>)> = outbound
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::Control(
                        ref message @ ClientMessage::ReportPaneRows { ref responses, .. },
                    ) => Some((
                        crate::session::protocol::control_frame_wire_bytes(message),
                        responses.iter().map(|r| r.id.clone()).collect(),
                    )),
                    _ => None,
                })
                .collect();
            assert_eq!(frames.len(), 1);
            assert!(
                frames[0].0 <= crate::session::protocol::MAX_FRAME_SIZE,
                "frame len {} exceeded MAX_FRAME_SIZE",
                frames[0].0
            );
            assert_eq!(
                frames[0].1,
                vec!["r8".to_string(), "r9".to_string()],
                "oldest responses evicted so large rows + responses fit one frame"
            );

            // An update near the 8 MiB ceiling that still fits `MAX_FRAME_SIZE` is retained whole
            // without any 64 KiB dead zone, evicting older cached responses instead of its own.
            let mut near_ceiling_rows =
                vec![row("r0", "done"), row("r1", "done"), row("r9", "done")];
            near_ceiling_rows[0].cwd = Some("x".repeat(6 * 1024 * 1024 - 8 * 1024));
            report_responses(
                backend,
                near_ceiling_rows,
                &[("r0", 2, big), ("r1", 2, big)],
            );
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("r0".to_string(), 2), ("r1".to_string(), 2)]],
                "current update responses fit MAX_FRAME_SIZE and are sent together"
            );
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("r0".to_string(), 2), ("r1".to_string(), 2)]],
                "update fitting MAX_FRAME_SIZE was retained and evicted older r9"
            );

            // An update whose own `ReportPaneRows` exceeds `MAX_FRAME_SIZE` (whether with
            // responses or rows-only) is neither sent into the session transport nor allowed to
            // poison `last_rows` / `last_responses`.
            let mut over_with_responses = vec![row("r0", "done"), row("r1", "done")];
            over_with_responses[0].cwd = Some("x".repeat(6 * 1024 * 1024 + 64 * 1024));
            report_responses(
                backend,
                over_with_responses,
                &[("r0", 3, big), ("r1", 3, big)],
            );
            assert!(
                response_reports(&outbound).is_empty(),
                "oversized update with responses must not be pushed to session transport"
            );
            let mut over_rows_only = vec![row("r0", "done"), row("r1", "done")];
            over_rows_only[0].cwd = Some("x".repeat(crate::session::protocol::MAX_FRAME_SIZE));
            report(backend, 1, over_rows_only);
            assert!(
                response_reports(&outbound).is_empty(),
                "oversized rows-only update must not be pushed to session transport"
            );
            super::resend_rows(backend.state_mut());
            let replayed: Vec<(usize, Vec<(String, u64)>)> = outbound
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::Control(
                        ref message @ ClientMessage::ReportPaneRows { ref responses, .. },
                    ) => Some((
                        crate::session::protocol::control_frame_wire_bytes(message),
                        responses.iter().map(|r| (r.id.clone(), r.seq)).collect(),
                    )),
                    _ => None,
                })
                .collect();
            assert_eq!(replayed.len(), 1);
            assert!(replayed[0].0 <= crate::session::protocol::MAX_FRAME_SIZE);
            assert_eq!(
                replayed[0].1,
                vec![("r0".to_string(), 2), ("r1".to_string(), 2)],
                "oversized updates did not poison last_rows or last_responses"
            );
        });
    }

    #[test]
    fn a_rejected_update_leaves_no_valid_part_to_replay_even_after_later_row_fixes() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_agent_stream(backend, 1, tx);
            // `b` names no row, so the server rejects the whole update; `a` must not outlive it.
            report_responses(backend, vec![row("a", "done")], &[("a", 1, 1), ("b", 1, 1)]);
            let sent = response_reports(&outbound);
            assert_eq!(sent.len(), 1);
            assert_eq!(
                sent[0].len(),
                2,
                "the server sees the update exactly as published"
            );
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );

            // A post-sanitizing collision is rejected by the server; fixing the rows in a later
            // rows-only update must not resurrect the rejected response on reconnect.
            report_responses(
                backend,
                vec![row("a", "done"), row(" a ", "done")],
                &[("a", 1, 1)],
            );
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("a".to_string(), 1)]]
            );
            report(backend, 1, vec![row("a", "done")]);
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );

            // Over the aggregate budget is just as invalid, and just as unretained.
            let big = crate::session::protocol::MAX_AGENT_RESPONSE_BYTES;
            let rows: Vec<_> = (0..5).map(|n| row(&format!("r{n}"), "done")).collect();
            let heavy: Vec<(String, u64, usize)> =
                (0..5).map(|n| (format!("r{n}"), 1, big)).collect();
            let heavy: Vec<(&str, u64, usize)> = heavy
                .iter()
                .map(|(id, seq, len)| (id.as_str(), *seq, *len))
                .collect();
            report_responses(backend, rows, &heavy);
            assert_eq!(response_reports(&outbound)[0].len(), 5);
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![Vec::<(String, u64)>::new()]
            );
        });
    }

    #[test]
    fn a_stale_sequence_cannot_replace_a_newer_retained_response() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_agent_stream(backend, 1, tx);
            report_responses(backend, vec![row("a", "done")], &[("a", 2, 1)]);
            report_responses(backend, vec![row("a", "done")], &[("a", 1, 1)]);
            outbound.try_iter().for_each(drop);
            super::resend_rows(backend.state_mut());
            assert_eq!(
                response_reports(&outbound),
                vec![vec![("a".to_string(), 2)]]
            );
        });
    }

    #[test]
    fn live_streams_resend_their_rows_after_an_attach() {
        with_backend(|backend| {
            let outbound = attach(backend);
            let (tx, _rx) = std::sync::mpsc::sync_channel(1);
            open_stream(backend, Open::anonymous(1), tx);
            report(backend, 1, vec![row("a", "working")]);
            assert_eq!(reports(&outbound).len(), 1);
            super::resend_rows(backend.state_mut());
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
                    extension: Some(crate::control::ExtensionProvenance {
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
