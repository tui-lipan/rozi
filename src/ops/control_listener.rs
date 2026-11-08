//! The control socket's server side: accepting connections, authorizing callers and turning each
//! request into a `Msg` for the app. The request and response vocabulary lives in
//! [`crate::control`]; this half depends on the app, so it sits with the other operations.

use std::io::{self, BufReader, Write};
use std::sync::mpsc;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::control::OVERSIZED_CONTROL_MESSAGE;
use crate::control::*;
use crate::events::{EventHub, EventKind};
use crate::platform::ipc::{IpcConnection, IpcListener};

pub fn run_listener(listener: IpcListener, link: CommandLink<Msg>, event_hub: EventHub) {
    listener
        .set_nonblocking(false)
        .expect("control listener supports blocking accept");
    // Accept failures repeat every retry while the condition lasts, so only the first of a run is
    // reported. stderr is not an option: it is the terminal the TUI is drawing on.
    let mut failing = false;
    loop {
        match listener.accept() {
            Ok(stream) => {
                failing = false;
                let link = link.clone();
                let event_hub = event_hub.clone();
                std::thread::spawn(move || handle_connection(stream, link, event_hub));
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                if !failing {
                    failing = true;
                    link.send(Msg::BackgroundError {
                        title: "Control endpoint accept failed",
                        message: err.to_string(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// How many unread activations a publisher may accumulate before rozi stops keeping them.
///
/// Activations are user clicks, so this is generous relative to how fast anyone can produce them;
/// a publisher that has stopped reading is wedged rather than busy.
const PUBLISH_ACTIVATION_BACKLOG: usize = 32;

/// Clone the reader for a long-lived control stream without the handshake deadline.
/// Timeout settings may be copied by a transport's `try_clone`, so clear the actual reader.
pub(crate) fn control_stream_reader(stream: &IpcConnection) -> io::Result<IpcConnection> {
    let reader = stream.try_clone()?;
    reader.set_read_timeout(None)?;
    Ok(reader)
}

/// Serve one pane's `publish` stream until its publisher goes away.
///
/// The only bidirectional command: after the acknowledgement, the publisher writes one row list
/// per line and rozi writes one activation per line back. Both directions run for the life of
/// the connection, so a writer thread carries activations while this thread reads.
fn run_publish_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    open: crate::ops::published_rows::PublishOpen,
) {
    let stream_id = open.stream_id;
    let Ok(reader_stream) = control_stream_reader(&stream) else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel::<String>(PUBLISH_ACTIVATION_BACKLOG);
    let (ack_tx, ack_rx) = mpsc::channel();
    link.send(Msg::PublishStreamOpen {
        stream_id,
        requested_pane: open.requested_pane,
        source_session: open.source_session,
        kind: open.kind,
        name: open.name,
        extension: open.extension,
        worker: open.worker,
        sender: tx,
        ack: ack_tx,
    });
    let pane_id = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(pane_id)) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::empty()).unwrap()
            );
            pane_id
        }
        Ok(Err(response)) => {
            let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
            return;
        }
        Err(_) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error_with(
                    ControlErrorCode::RequestTimeout,
                    "publish request timed out",
                ))
                .unwrap()
            );
            return;
        }
    };
    // The dedicated reader allows silence between state changes, which can be minutes apart.
    let writer = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                return;
            }
        }
    });

    for line in control_lines(reader_stream) {
        let line = match line {
            Ok(line) => line,
            Err(error) if is_oversized_control_line(&error) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ControlResponse::error_with(
                        ControlErrorCode::MessageTooLarge,
                        OVERSIZED_CONTROL_MESSAGE,
                    ))
                    .unwrap()
                );
                break;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        // A malformed line is the publisher's bug, not a reason to drop its rows; skip it and keep
        // the stream open so the next good list still lands.
        if let Ok(report) = serde_json::from_str::<PublishReport>(&line) {
            link.send(Msg::PublishedRowsReported {
                stream_id,
                pane_id,
                rows: report.rows,
                responses: report.responses,
            });
        }
    }

    // Reaching here means EOF or a read error: the publisher is gone, so its rows go with it.
    link.send(Msg::PublishStreamClosed { stream_id, pane_id });
    drop(stream);
    let _ = writer.join();
}

/// How many unread events a sidebar stream producer may accumulate before rozi drops them. Like
/// publish activations, these follow user gestures; a producer this far behind has stopped reading.
const SIDEBAR_STREAM_BACKLOG: usize = 64;

/// Serve one `sidebar-publish` stream until its producer goes away.
///
/// The same two-way shape as [`run_publish_stream`]: after the acknowledgement the producer writes
/// one snapshot per line and rozi writes one event per line back, each direction on its own thread.
fn run_sidebar_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    open: crate::ops::sidebar_streams::StreamOpen,
) {
    let stream_id = open.stream_id;
    let Ok(reader_stream) = control_stream_reader(&stream) else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel::<String>(SIDEBAR_STREAM_BACKLOG);
    let (ack_tx, ack_rx) = mpsc::channel();
    link.send(Msg::SidebarStreamOpen {
        open: Box::new(open),
        sender: tx,
        ack: ack_tx,
    });
    let (tab, producer) = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(owner)) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::empty()).unwrap()
            );
            owner
        }
        Ok(Err(response)) => {
            let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
            return;
        }
        Err(_) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error_with(
                    ControlErrorCode::RequestTimeout,
                    "sidebar-publish request timed out",
                ))
                .unwrap()
            );
            return;
        }
    };
    let writer = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                return;
            }
        }
    });
    for line in control_lines(reader_stream) {
        let line = match line {
            Ok(line) => line,
            Err(error) if is_oversized_control_line(&error) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ControlResponse::error_with(
                        ControlErrorCode::MessageTooLarge,
                        OVERSIZED_CONTROL_MESSAGE,
                    ))
                    .unwrap()
                );
                break;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        // A malformed line is the producer's bug; its last good snapshot stays on screen.
        if let Ok(snapshot) = serde_json::from_str::<crate::state::SidebarSnapshot>(&line) {
            link.send(Msg::SidebarStreamSnapshot {
                stream_id,
                tab: tab.clone(),
                producer: producer.clone(),
                snapshot,
            });
        }
    }
    link.send(Msg::SidebarStreamClosed {
        stream_id,
        tab,
        producer,
    });
    drop(stream);
    let _ = writer.join();
}

fn next_pick_id() -> u64 {
    static NEXT_PICK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_PICK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Serve one `pick` or `prompt` stream until the user makes a choice, cancels, or the caller
/// disconnects. `open` builds the message that raises the modal.
///
/// After acknowledging the request on the UI thread, a picker's caller may write row updates, each
/// replacing the previous set. When the modal closes, rozi writes exactly one terminal JSON line
/// (`{"selected":"…"}`, `{"input":"…"}`, or `{"cancelled":true}`).
///
/// Note: `IpcConnection` exposes no `shutdown()`, so after writing the terminal line rozi cannot
/// force the connection closed from this end; the reader thread reaps once the client closes.
fn run_pick_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    id: u64,
    open: impl FnOnce(crate::state::PickReply, mpsc::Sender<ControlResponse>) -> Msg,
) {
    let Ok(reader_stream) = control_stream_reader(&stream) else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };

    let (ack_tx, ack_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = crate::state::PickReply::channel();

    link.send(open(reply_tx, ack_tx));

    let ack_response = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => res,
        Err(_) => {
            ControlResponse::error_with(ControlErrorCode::RequestTimeout, "pick request timed out")
        }
    };

    let _ = writeln!(stream, "{}", serde_json::to_string(&ack_response).unwrap());

    if !ack_response.ok {
        return;
    }

    // Every reply line goes out, not just the first: actions and tab switches keep the picker
    // open and report again later. The loop ends after the terminal line, or when the picker is
    // dropped without one.
    let writer = std::thread::spawn(move || {
        while let Some(line) = reply_rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
    });

    for line in control_lines(reader_stream) {
        let line = match line {
            Ok(line) => line,
            Err(error) if is_oversized_control_line(&error) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ControlResponse::error_with(
                        ControlErrorCode::MessageTooLarge,
                        OVERSIZED_CONTROL_MESSAGE,
                    ))
                    .unwrap()
                );
                break;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<PickReport>(&line) {
            let mut rows = report.rows;
            rows.truncate(MAX_PICK_ROWS);
            link.send(Msg::PickRowsReported {
                id,
                tab: report.tab,
                rows,
            });
        }
    }

    link.send(Msg::PickStreamClosed { id });
    drop(stream);
    let _ = writer.join();
}

/// One line written by a `publish` publisher.
#[derive(Debug, serde::Deserialize)]
struct PublishReport {
    #[serde(default)]
    responses: Vec<crate::control::PublishedAgentResponse>,
    #[serde(default)]
    rows: Vec<crate::session::protocol::PublishedRow>,
}

/// One line written by a `pick` publisher.
#[derive(Debug, serde::Deserialize)]
struct PickReport {
    /// The tab these rows fill. Required when the picker declared tabs, absent otherwise.
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    rows: Vec<crate::control::pick::PickRow>,
}

fn write_control_response(stream: &mut IpcConnection, response: &ControlResponse) {
    let _ = writeln!(stream, "{}", serde_json::to_string(response).unwrap());
}

/// Establish who sent `request`, or answer it with the reason it is refused.
///
/// A request with neither provenance nor a credential, from this machine, is an ordinary local
/// caller and needs nothing. Anything else is decided by the UI, which alone knows which
/// generations and worker credentials are live.
fn authorize_request(
    stream: &mut IpcConnection,
    link: &CommandLink<Msg>,
    request: &ControlRequest,
    origin: RequestOrigin,
) -> Option<Option<ExtensionGrant>> {
    if request.extension.is_none() && request.credential.is_none() && origin == RequestOrigin::Local
    {
        return Some(None);
    }
    let (reply, authorized) = mpsc::channel();
    link.send(Msg::AuthorizeExtensionControl {
        provenance: request.extension.clone(),
        credential: request.credential.clone(),
        origin,
        reply,
    });
    let response = match authorized.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(grant)) => return Some(grant),
        Ok(Err(response)) => response,
        Err(_) => ControlResponse::error_with(
            ControlErrorCode::ExtensionInactive,
            "extension generation is not active",
        ),
    };
    write_control_response(stream, &response);
    None
}

fn run_subscription(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    event_hub: EventHub,
    events: &[String],
    extension: Option<crate::control::ExtensionProvenance>,
    worker: Option<crate::control::WorkerId>,
) {
    let mut kinds = std::collections::HashSet::new();
    for id in events {
        let Some(kind) = EventKind::parse(id) else {
            write_control_response(
                &mut stream,
                &ControlResponse::error_with(
                    ControlErrorCode::UnknownEvent,
                    format!("unknown event `{id}`"),
                ),
            );
            return;
        };
        kinds.insert(kind);
    }

    static NEXT_SUBSCRIPTION_ID: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1);
    let owned_subscription = extension.map(|provenance| {
        let id = NEXT_SUBSCRIPTION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (cancel, cancelled) = mpsc::sync_channel(1);
        let (reply, opened) = mpsc::channel();
        link.send(Msg::ExtensionSubscriptionOpen {
            id,
            provenance,
            worker,
            cancel,
            reply,
        });
        (id, cancelled, opened)
    });
    // A placed worker only hears what happens while the session on screen lies within its
    // binding: events describe that session's panes, and a worker on another machine has no claim
    // on a session somewhere else.
    let mut binding = None;
    if let Some((_, _, opened)) = &owned_subscription {
        match opened.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(bound)) => binding = bound,
            Ok(Err(response)) => {
                write_control_response(&mut stream, &response);
                return;
            }
            Err(_) => {
                write_control_response(
                    &mut stream,
                    &ControlResponse::error_with(
                        ControlErrorCode::ExtensionInactive,
                        "extension generation is not active",
                    ),
                );
                return;
            }
        }
    }

    let rx = event_hub.subscribe_bound((!kinds.is_empty()).then_some(kinds), binding);
    write_control_response(&mut stream, &ControlResponse::empty());
    let _ = stream.set_read_timeout(None);
    loop {
        if owned_subscription
            .as_ref()
            .is_some_and(|(_, cancelled, _)| cancelled.try_recv().is_ok())
        {
            break;
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                if writeln!(stream, "{event}").is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if caller_hung_up(&mut stream) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some((id, _, _)) = owned_subscription {
        link.send(Msg::ExtensionSubscriptionClosed { id });
    }
}

fn control_lines(stream: IpcConnection) -> impl Iterator<Item = io::Result<String>> {
    ControlLines {
        reader: BufReader::new(stream),
    }
}

struct ControlLines<R> {
    reader: BufReader<R>,
}

impl<R: io::Read> Iterator for ControlLines<R> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        match read_control_line(&mut self.reader) {
            Ok(Some(line)) => Some(Ok(line)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

/// How long a UI control connection waits for the app to answer.
///
/// An ordinary request is answered on the app's next update, so ten seconds is a wedged UI. A pane
/// wait is answered when its condition resolves or its own deadline passes, and the UI enforces
/// that deadline itself, so the connection outlasts it by the same margin rather than cutting
/// every wait off at ten seconds. An agent wait, alone or after a prompt, is held by the session
/// server until its own deadline, with the same margin. A `record-stop` is answered once the session server has finished
/// the file, so it gets the budget a session caller gets, and a `record-ui-stop` once the UI has.
fn control_reply_timeout(command: &ControlCommand) -> Duration {
    const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
    if matches!(
        command,
        ControlCommand::RecordStop { .. } | ControlCommand::RecordUiStop
    ) {
        return crate::session::headless::RECORDING_STOP_TIMEOUT;
    }
    if let ControlCommand::Invoke { timeout_ms, .. } = command {
        return Duration::from_millis(longest_invoke_ms(*timeout_ms)).saturating_add(REPLY_TIMEOUT);
    }
    if let ControlCommand::AgentWait {
        timeout_ms: Some(timeout_ms),
        ..
    }
    | ControlCommand::AgentPrompt {
        wait: Some(_),
        timeout_ms: Some(timeout_ms),
        ..
    } = command
    {
        return Duration::from_millis(*timeout_ms).saturating_add(REPLY_TIMEOUT);
    }
    command.pane_wait().map_or(REPLY_TIMEOUT, |wait| {
        Duration::from_millis(wait.timeout_ms).saturating_add(REPLY_TIMEOUT)
    })
}

fn handle_connection(stream: IpcConnection, link: CommandLink<Msg>, event_hub: EventHub) {
    serve_connection(stream, link, event_hub, RequestOrigin::Local);
}

/// Serve one control connection, local or relayed from a remote runtime's extension bridge. Both
/// speak exactly the same protocol; only what the UI is told about the caller differs.
pub(crate) fn serve_connection(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    event_hub: EventHub,
    origin: RequestOrigin,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let line = match read_control_line(&mut reader) {
        Ok(Some(line)) => line,
        Ok(None) => return,
        Err(error) => {
            if error.kind() == io::ErrorKind::InvalidData {
                let code = if is_oversized_control_line(&error) {
                    ControlErrorCode::MessageTooLarge
                } else {
                    ControlErrorCode::InvalidRequest
                };
                write_control_response(
                    &mut stream,
                    &ControlResponse::error_with(code, error.to_string()),
                );
            }
            return;
        }
    };
    let mut request = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(request) => request,
        Err(err) => {
            write_control_response(
                &mut stream,
                &ControlResponse::error_with(
                    ControlErrorCode::InvalidRequest,
                    format!("invalid request: {err}"),
                ),
            );
            return;
        }
    };
    let Some(grant) = authorize_request(&mut stream, &link, &request, origin) else {
        return;
    };
    let worker = grant.as_ref().and_then(|grant| grant.worker);
    if let Some(grant) = grant {
        // The credential, not the request, says which extension and generation this is.
        request.extension = Some(grant.provenance);
    }
    if let ControlCommand::Subscribe {
        events,
        snapshot,
        after,
    } = &request.command
    {
        if *snapshot || after.is_some() {
            write_control_response(
                &mut stream,
                &ControlResponse::error_with(
                    ControlErrorCode::InvalidArgument,
                    "snapshot and after need a session endpoint; a UI keeps no event history",
                ),
            );
            return;
        }
        run_subscription(
            stream,
            link,
            event_hub,
            events,
            request.extension.clone(),
            worker,
        );
        return;
    }
    if let ControlCommand::Publish { kind, name } = &request.command {
        static NEXT_PUBLISH_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let stream_id = NEXT_PUBLISH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_publish_stream(
            stream,
            link,
            crate::ops::published_rows::PublishOpen {
                stream_id,
                requested_pane: request.source_pane,
                source_session: request.source_session.clone(),
                kind: *kind,
                name: name.clone(),
                extension: request.extension.clone(),
                worker,
            },
        );
        return;
    }
    if let ControlCommand::SidebarPublish { tab, name } = &request.command {
        static NEXT_SIDEBAR_STREAM_ID: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let stream_id = NEXT_SIDEBAR_STREAM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_sidebar_stream(
            stream,
            link,
            crate::ops::sidebar_streams::StreamOpen {
                stream_id,
                tab: tab.clone(),
                name: name.clone(),
                extension: request.extension.clone(),
                worker,
            },
        );
        return;
    }
    if let ControlCommand::Pick {
        title,
        placeholder,
        empty,
        width,
        actions,
        tabs,
        tab,
    } = &request.command
    {
        let id = next_pick_id();
        let open = |sender, ack| Msg::PickStreamOpen {
            id,
            title: title.clone(),
            placeholder: placeholder.clone(),
            empty: empty.clone(),
            width: *width,
            actions: actions.clone(),
            tabs: tabs.clone(),
            tab: tab.clone(),
            extension: request.extension.clone(),
            worker,
            sender,
            ack,
        };
        run_pick_stream(stream, link, id, open);
        return;
    }
    if let ControlCommand::Prompt {
        title,
        placeholder,
        value,
        masked,
    } = &request.command
    {
        let id = next_pick_id();
        let open = |sender, ack| Msg::PromptStreamOpen {
            id,
            prompt: crate::control::pick::PickPromptFields {
                title: title.clone(),
                placeholder: placeholder.clone(),
                value: value.clone(),
                from_query: false,
                masked: *masked,
            },
            extension: request.extension.clone(),
            worker,
            sender,
            ack,
        };
        run_pick_stream(stream, link, id, open);
        return;
    }
    let reply_timeout = control_reply_timeout(&request.command);
    let caller = matches!(request.command, ControlCommand::Invoke { .. })
        .then(|| NEXT_WATCHED_CALLER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let (tx, rx) = mpsc::channel();
    link.send(Msg::ControlRequest(ControlEnvelope {
        request,
        reply: tx,
        worker,
        caller,
    }));
    let response = if let Some(caller) = caller {
        let Ok(response) = reply_unless_hung_up(&mut stream, &rx, reply_timeout) else {
            link.send(Msg::ControlCallerGone { caller });
            return;
        };
        response
    } else {
        rx.recv_timeout(reply_timeout).ok()
    };
    let response = response.unwrap_or_else(|| {
        ControlResponse::error_with(
            ControlErrorCode::RequestTimeout,
            "control request timed out",
        )
    });
    write_control_response(&mut stream, &response);
}

static NEXT_WATCHED_CALLER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Whether a caller that sends nothing after its request has closed the connection.
fn caller_hung_up(stream: &mut IpcConnection) -> bool {
    let mut probe = [0u8; 8];
    let _ = stream.set_nonblocking(true);
    let hung_up = match std::io::Read::read(stream, &mut probe) {
        Ok(0) => true,
        Ok(_) => false,
        Err(err) => err.kind() != std::io::ErrorKind::WouldBlock,
    };
    let _ = stream.set_nonblocking(false);
    hung_up
}

/// The caller closed its connection before its reply came.
struct CallerHungUp;

/// Wait for a reply while watching the caller. `Ok(None)` means no reply came in time.
fn reply_unless_hung_up(
    stream: &mut IpcConnection,
    replies: &mpsc::Receiver<ControlResponse>,
    timeout: Duration,
) -> std::result::Result<Option<ControlResponse>, CallerHungUp> {
    const PROBE_INTERVAL: Duration = Duration::from_millis(100);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match replies.recv_timeout(left.min(PROBE_INTERVAL)) {
            Ok(response) => return Ok(Some(response)),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) if left <= PROBE_INTERVAL => return Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if caller_hung_up(stream) {
                    return Err(CallerHungUp);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{CaptureRender, ControlCommand, PaneWait};

    #[test]
    fn control_stream_reader_outlives_the_handshake_deadline() {
        use crate::platform::ipc::IpcEndpoint;
        use std::io::{Read, Write};

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("private");
        crate::platform::fs_security::ensure_private_dir(&dir).unwrap();
        let endpoint = IpcEndpoint::at_path(dir.join("stream.sock"));
        let listener = endpoint.bind().unwrap().into_listener();
        let server = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(1)))
                .unwrap();
            let mut reader = control_stream_reader(&stream).unwrap();
            let (ready, started) = mpsc::channel();
            let (result, completed) = mpsc::channel();
            let reading = std::thread::spawn(move || {
                ready.send(()).unwrap();
                let mut byte = [0];
                result
                    .send(reader.read_exact(&mut byte).map(|()| byte))
                    .unwrap();
            });
            started.recv().unwrap();
            (stream, reading, completed)
        });
        let mut client = endpoint.connect().unwrap();
        let (_stream, reading, completed) = server.join().unwrap();
        let early = completed.recv_timeout(Duration::from_millis(20));
        client.write_all(b"x").unwrap();
        drop(client);
        reading.join().unwrap();
        assert!(
            matches!(early, Err(mpsc::RecvTimeoutError::Timeout)),
            "idle reader exited: {early:?}"
        );
        assert_eq!(completed.recv().unwrap().unwrap(), *b"x");
    }

    #[test]
    fn a_ui_wait_is_given_its_own_deadline_to_answer_in() {
        let capture = |wait| ControlCommand::CapturePane {
            target: None,
            scrollback: None,
            render: CaptureRender::Text,
            scale: None,
            image_pixels: false,
            wait,
        };
        assert_eq!(
            control_reply_timeout(&capture(None)),
            Duration::from_secs(10)
        );
        assert_eq!(
            control_reply_timeout(&capture(Some(PaneWait {
                text: Some("ok".to_string()),
                settle_ms: None,
                timeout_ms: 60_000,
            }))),
            Duration::from_secs(70)
        );
    }
}
