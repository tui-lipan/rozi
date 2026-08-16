#![allow(clippy::too_many_arguments)]

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::platform::ipc::{IpcConnection, IpcEndpoint};
use crate::runtime_metrics::{
    ByteBufferMetrics, CachedServerRuntimeMetrics, QueueMetrics, TimedServerRuntimeMetrics,
};
use crate::session::protocol::Frame;
use crate::session::protocol::{
    self, ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage, WirePalette,
};
use crate::session::queue::{ByteQueue, PushError};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

const MAX_CLIENT_INBOUND_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIENT_OUTBOUND_BYTES: usize = 8 * 1024 * 1024;

/// Explicit transport lifecycle owner. When the final `SessionClient` handle for a session
/// is dropped, `ClientTransport` shuts down the stream, signals termination to reader/writer
/// worker threads, and closes the outbound queue.
struct ClientTransport {
    outbound: Arc<ByteQueue<ClientOutbound>>,
    shutdown_stream: Mutex<Option<crate::platform::ipc::IpcConnection>>,
    shutdown_signal: Arc<AtomicBool>,
}

impl ClientTransport {
    fn disconnect(&self) {
        self.shutdown_signal.store(true, Ordering::SeqCst);
        self.outbound.close();
        if let Ok(mut guard) = self.shutdown_stream.lock()
            && let Some(stream) = guard.take()
        {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for ClientTransport {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[derive(Clone)]
pub struct SessionClient {
    /// RAII owner for the transport's connection and worker threads. Dropping the final
    /// reference closes the queue and socket.
    #[allow(dead_code)]
    transport: Option<Arc<ClientTransport>>,
    outbound: Arc<ByteQueue<ClientOutbound>>,
    transport_failed: Arc<AtomicBool>,
    inbound: Option<Arc<InboundMailbox>>,
    latest_server_metrics: Arc<Mutex<Option<TimedServerRuntimeMetrics>>>,
    metrics_request_pending: Arc<AtomicBool>,
    piped_buffer: Option<crate::platform::ipc::PipedBufferStatsHandle>,
    #[cfg(test)]
    test_observer: Option<mpsc::Sender<ClientOutbound>>,
    server_pid: Option<u32>,
    /// Wire version agreed with this server. Gates messages added after the minimum supported
    /// version so an older server never receives a variant it cannot deserialize.
    effective_protocol: u32,
    /// This client's host cell size in pixels, sent with the canonical PTY size so the server's
    /// PTYs report pixel dimensions the child can size images against. Read once: it is a
    /// property of the terminal this process is attached to, not of any one pane.
    cell: tui_lipan::TerminalCellSize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRuntimeStats {
    pub inbound: Option<QueueMetrics>,
    pub outbound: QueueMetrics,
    pub piped_remote: Option<ByteBufferMetrics>,
    pub server: Option<CachedServerRuntimeMetrics>,
}

/// A structured request to spawn a pane on the session server.
#[derive(Clone, Debug)]
pub struct SpawnPaneRequest {
    pub pane_id: PaneId,
    pub local: bool,
    pub generation: u64,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub keep_open: bool,
    pub env: Vec<(String, String)>,
    pub title: Option<String>,
    pub palette: TerminalColorPalette,
    pub shell: Vec<String>,
    pub command_shell: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
// `ClientMessage::SpawnPane` (with its `shell`/`command_shell` argv, cross-platform plan Phase 4)
// makes `Control` noticeably larger than `PaneInput`. Every outbound message already goes through
// an `mpsc` channel send regardless (one heap-ish allocation either way), and this is a
// per-user-action/per-input-chunk channel, not a hot per-byte loop, so boxing `ClientMessage` here
// to shrink the enum is not worth the churn across every `ClientOutbound::Control(...)` construction
// and match site.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ClientOutbound {
    Control(ClientMessage),
    PaneInput {
        pane_id: PaneId,
        local: bool,
        generation: u64,
        bytes: Vec<u8>,
    },
}

impl SessionClient {
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Receiver<ClientOutbound>) {
        let outbound = Arc::new(ByteQueue::new(MAX_CLIENT_OUTBOUND_BYTES));
        let (test_tx, test_rx) = mpsc::channel();
        (
            Self {
                transport: None,
                outbound: Arc::clone(&outbound),
                transport_failed: Arc::new(AtomicBool::new(false)),
                inbound: None,
                latest_server_metrics: Arc::new(Mutex::new(None)),
                metrics_request_pending: Arc::new(AtomicBool::new(false)),
                piped_buffer: None,
                test_observer: Some(test_tx),
                cell: tui_lipan::TerminalCellSize::default(),
                server_pid: None,
                effective_protocol: PROTOCOL_VERSION,
            },
            test_rx,
        )
    }

    pub fn connect(
        endpoint: &IpcEndpoint,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        let stream = endpoint.connect()?;
        Self::from_stream(stream, session, inbound)
    }

    pub fn connect_attached(
        endpoint: &IpcEndpoint,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        let stream = endpoint.connect()?;
        Self::from_stream_attached(stream, session, inbound, read_only)
    }

    pub fn from_stream(
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
    ) -> io::Result<Self> {
        Ok(Self::from_stream_attached(stream, session, inbound, false)?.0)
    }

    pub fn from_stream_attached(
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: mpsc::Sender<Frame<ServerMessage>>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        Self::from_stream_attached_target(
            stream,
            session,
            InboundTarget::Channel(inbound),
            read_only,
        )
    }

    pub(crate) fn connect_attached_mailbox(
        endpoint: &IpcEndpoint,
        session: impl Into<String>,
        inbound: Arc<InboundMailbox>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        Self::from_stream_attached_target(
            endpoint.connect()?,
            session,
            InboundTarget::Mailbox(inbound),
            read_only,
        )
    }

    pub(crate) fn from_stream_attached_mailbox(
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: Arc<InboundMailbox>,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        Self::from_stream_attached_target(
            stream,
            session,
            InboundTarget::Mailbox(inbound),
            read_only,
        )
    }

    fn from_stream_attached_target(
        stream: IpcConnection,
        session: impl Into<String>,
        inbound: InboundTarget,
        read_only: bool,
    ) -> io::Result<(Self, ServerMessage)> {
        let mut stream = stream;
        let server_pid = stream.peer_pid();
        let piped_buffer = stream.piped_buffer_stats_handle();
        let mut reader = stream.try_clone()?;
        reader.set_read_timeout(Some(Duration::from_secs(2)))?;
        protocol::write_frame(
            &mut stream,
            &protocol::attach_message(
                session,
                crate::platform::user::current_user_label(),
                read_only,
            ),
        )?;
        let attached = protocol::read_frame::<_, ServerMessage>(&mut reader)?;
        reader.set_read_timeout(None)?;
        #[cfg(windows)]
        // A pending synchronous ReadFile on a duplicated named-pipe handle can hold up WriteFile on
        // its sibling, delaying both keys and heartbeat pongs. Polling keeps the duplex path live.
        reader.set_nonblocking(true)?;
        let effective_protocol = validate_attached(&attached)?;
        let outbound = Arc::new(ByteQueue::<ClientOutbound>::new(MAX_CLIENT_OUTBOUND_BYTES));
        let shutdown_signal = Arc::new(AtomicBool::new(false));
        // The worker threads may be blocked in platform I/O. Do not construct a transport without
        // the duplicate handle used to interrupt those operations.
        let shutdown_stream = stream.try_clone()?;
        let transport = Arc::new(ClientTransport {
            outbound: Arc::clone(&outbound),
            shutdown_stream: Mutex::new(Some(shutdown_stream)),
            shutdown_signal: Arc::clone(&shutdown_signal),
        });
        let client_inbound = inbound.mailbox();
        let writer_outbound = Arc::clone(&outbound);
        let writer_inbound = client_inbound.clone();
        let writer_shutdown_signal = Arc::clone(&shutdown_signal);
        thread::spawn(move || {
            while let Some(message) = writer_outbound.pop_blocking() {
                if writer_shutdown_signal.load(Ordering::Relaxed) {
                    break;
                }
                let result = match message {
                    ClientOutbound::Control(message) => {
                        protocol::write_frame(&mut stream, &message)
                    }
                    ClientOutbound::PaneInput {
                        pane_id,
                        local,
                        generation,
                        bytes,
                    } => protocol::write_pane_input_frame(
                        &mut stream,
                        pane_id,
                        generation,
                        local,
                        &bytes,
                    ),
                };
                if result.is_err() {
                    if !writer_shutdown_signal.load(Ordering::Relaxed)
                        && let Some(inbound) = &writer_inbound
                    {
                        inbound.fail("session writer disconnected".to_string());
                    }
                    break;
                }
            }
            writer_outbound.close();
        });
        let heartbeat_outbound = Arc::clone(&outbound);
        let reader_outbound = Arc::clone(&outbound);
        let latest_server_metrics = Arc::new(Mutex::new(None));
        let reader_metrics = Arc::clone(&latest_server_metrics);
        let metrics_request_pending = Arc::new(AtomicBool::new(false));
        let reader_metrics_request_pending = Arc::clone(&metrics_request_pending);
        let reader_shutdown_signal = Arc::clone(&shutdown_signal);
        thread::spawn(move || {
            forward_inbound(
                &mut reader,
                &inbound,
                Some(&heartbeat_outbound),
                Some(&reader_metrics),
                Some(&reader_metrics_request_pending),
                true,
                Some(&reader_shutdown_signal),
            );
            reader_outbound.close();
        });
        let client = Self {
            transport: Some(transport),
            outbound,
            transport_failed: Arc::new(AtomicBool::new(false)),
            inbound: client_inbound,
            latest_server_metrics,
            metrics_request_pending,
            piped_buffer,
            #[cfg(test)]
            test_observer: None,
            server_pid,
            effective_protocol,
            cell: tui_lipan::host_cell_size(),
        };
        client.request_runtime_metrics();
        Ok((client, attached))
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.server_pid
    }

    /// Negotiated wire version for this connection.
    pub fn effective_protocol(&self) -> u32 {
        self.effective_protocol
    }

    pub fn request_runtime_metrics(&self) {
        try_enqueue_runtime_metrics_request(&self.outbound, &self.metrics_request_pending);
    }

    pub fn runtime_stats(&self) -> ClientRuntimeStats {
        let queue_metrics = |stats: crate::session::queue::QueueStats| QueueMetrics {
            bytes: ByteBufferMetrics::new(stats.bytes, stats.high_water_bytes, stats.capacity),
            queued_items: stats.len as u64,
        };
        ClientRuntimeStats {
            inbound: self
                .inbound
                .as_ref()
                .map(|inbound| queue_metrics(inbound.queue.stats())),
            outbound: queue_metrics(self.outbound.stats()),
            piped_remote: self
                .piped_buffer
                .as_ref()
                .and_then(|handle| handle.stats())
                .map(|stats| {
                    ByteBufferMetrics::new(stats.current, stats.high_water, stats.capacity)
                }),
            server: self
                .latest_server_metrics
                .lock()
                .expect("server metrics cache poisoned")
                .as_ref()
                .map(TimedServerRuntimeMetrics::cached),
        }
    }

    /// Tell the server whether this client is parked — attached with its screens kept live, but not
    /// displaying the session. A parked client gives up the layout-control lease, so keeping a
    /// session open in the background never makes it look occupied to the next client to attach.
    pub fn set_parked(&self, parked: bool) {
        self.send_control(ClientMessage::SetParked { parked });
    }

    /// Ask the server to list one directory on its own host.
    pub fn list_directory(&self, path: String, show_hidden: bool) {
        self.send_control(ClientMessage::ListDirectory { path, show_hidden });
    }

    /// Ask the server to scan a repository on its own host for changed paths.
    pub fn list_changes(&self, root: String) {
        self.send_control(ClientMessage::ListChanges { root });
    }

    pub fn spawn_pane(&self, request: SpawnPaneRequest) {
        self.send_control(ClientMessage::SpawnPane {
            pane_id: request.pane_id,
            local: request.local,
            generation: request.generation,
            command: request.command,
            cwd: request.cwd,
            cols: request.cols,
            rows: request.rows,
            keep_open: request.keep_open,
            env: request.env,
            title: request.title,
            palette: WirePalette::from(request.palette),
            shell: request.shell,
            command_shell: request.command_shell,
            cell_width: self.cell.width,
            cell_height: self.cell.height,
        });
    }

    pub fn send_input(&self, pane_id: PaneId, generation: u64, local: bool, bytes: Vec<u8>) {
        self.send(ClientOutbound::PaneInput {
            pane_id,
            local,
            generation,
            bytes,
        });
    }
    pub fn resize(&self, pane_id: PaneId, generation: u64, local: bool, cols: u16, rows: u16) {
        self.send_control(ClientMessage::Resize {
            pane_id,
            local,
            generation,
            cols,
            rows,
            cell_width: self.cell.width,
            cell_height: self.cell.height,
        });
    }
    pub fn kill(&self, pane_id: PaneId, generation: u64, local: bool) {
        self.send_control(ClientMessage::Kill {
            pane_id,
            local,
            generation,
        });
    }
    pub fn set_palette(
        &self,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        palette: TerminalColorPalette,
    ) {
        self.send_control(ClientMessage::SetPalette {
            pane_id,
            local,
            generation,
            palette: WirePalette::from(palette),
        });
    }
    pub fn set_pane_logging(&self, pane_id: PaneId, generation: u64, local: bool, enabled: bool) {
        self.send_control(ClientMessage::SetPaneLogging {
            pane_id,
            local,
            generation,
            enabled,
        });
    }
    pub fn set_pane_status(
        &self,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        status: Option<String>,
        reason: Option<String>,
    ) {
        self.send_control(ClientMessage::SetPaneStatus {
            pane_id,
            local,
            generation,
            status,
            reason,
        });
    }

    /// Replace a pane's published agent slots; an empty list withdraws them.
    pub fn report_pane_rows(
        &self,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        rows: Vec<crate::session::protocol::PublishedRow>,
    ) {
        self.send_control(ClientMessage::ReportPaneRows {
            pane_id,
            local,
            generation,
            rows,
        });
    }
    /// Commit a new shared layout, optimistically based on `base_rev`. The server accepts it only
    /// while this client holds the lease and `base_rev` matches the current revision.
    pub fn commit_layout(&self, base_rev: u64, layout: SharedLayout) {
        self.send_control(ClientMessage::CommitLayout { base_rev, layout });
    }
    /// Ask the current controller for the layout-control lease. The server auto-grants only when no
    /// controller holds it; otherwise it flags the request for the controller to grant or decline.
    pub fn request_control(&self) {
        self.send_control(ClientMessage::RequestControl);
    }
    pub fn set_control_takeover(&self, allowed: bool) {
        self.send_control(ClientMessage::SetControlTakeover { allowed });
    }
    pub fn grant_control(&self, to: ClientId) {
        self.send_control(ClientMessage::GrantControl { to });
    }
    pub fn decline_control(&self, to: ClientId) {
        self.send_control(ClientMessage::DeclineControl { to });
    }
    /// Controller-only: remove another client from the session.
    pub fn evict_client(&self, target: ClientId) {
        self.send_control(ClientMessage::EvictClient { target });
    }
    pub fn set_input_lock(&self, locked: bool) {
        self.send_control(ClientMessage::SetInputLock { locked });
    }

    pub fn set_session_origin(&self, profile: String) {
        self.send_control(ClientMessage::SetSessionOrigin { profile });
    }
    /// Reply to a server heartbeat.
    pub fn pong(&self, seq: u64) {
        self.send_control(ClientMessage::Pong { seq });
    }
    pub fn rename(&self, name: String) {
        self.send_control(ClientMessage::Rename { name });
    }
    pub fn detach(&self) {
        self.send_control(ClientMessage::Detach);
    }

    pub fn shutdown(&self) {
        self.send_control(ClientMessage::Shutdown);
    }

    /// Explicit local transport teardown: closes the outbound queue, signals reader/writer
    /// threads, and shuts down the local socket/pipe without requesting server termination.
    pub fn disconnect_transport(&self) {
        if let Some(transport) = &self.transport {
            transport.disconnect();
        }
    }

    fn send_control(&self, message: ClientMessage) {
        self.send(ClientOutbound::Control(message));
    }

    fn send(&self, message: ClientOutbound) {
        #[cfg(test)]
        if let Some(observer) = &self.test_observer {
            let _ = observer.send(message);
            return;
        }
        if self.transport_failed.load(Ordering::Acquire) {
            return;
        }
        let bytes = message.wire_bytes();
        if self.outbound.try_push(message, bytes).is_err() {
            self.transport_failed.store(true, Ordering::Release);
            self.outbound.close();
            if let Some(inbound) = &self.inbound {
                inbound.fail("session outbound queue exceeded 8 MiB".to_string());
            }
        }
    }
}

enum InboundTarget {
    Channel(mpsc::Sender<Frame<ServerMessage>>),
    Mailbox(Arc<InboundMailbox>),
}

impl InboundTarget {
    fn mailbox(&self) -> Option<Arc<InboundMailbox>> {
        match self {
            Self::Channel(_) => None,
            Self::Mailbox(mailbox) => Some(Arc::clone(mailbox)),
        }
    }

    fn send(&self, frame: Frame<ServerMessage>) -> std::result::Result<(), ()> {
        match self {
            Self::Channel(channel) => channel.send(frame).map_err(|_| ()),
            Self::Mailbox(mailbox) => mailbox.push(frame),
        }
    }

    fn disconnected(&self) {
        if let Self::Mailbox(mailbox) = self {
            mailbox.disconnected();
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum InboundEvent {
    Frame(Box<Frame<ServerMessage>>),
    Disconnected,
    Failed(String),
}

pub struct InboundMailbox {
    queue: ByteQueue<InboundEvent>,
    scheduled: AtomicBool,
    active: AtomicBool,
    ended: AtomicBool,
    epoch: u64,
    session_name: String,
    link: CommandLink<crate::Msg>,
}

impl InboundMailbox {
    pub(crate) fn new(
        epoch: u64,
        session_name: String,
        link: CommandLink<crate::Msg>,
    ) -> Arc<Self> {
        Arc::new(Self {
            queue: ByteQueue::new(MAX_CLIENT_INBOUND_BYTES),
            scheduled: AtomicBool::new(false),
            active: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            epoch,
            session_name,
            link,
        })
    }

    fn push(self: &Arc<Self>, frame: Frame<ServerMessage>) -> std::result::Result<(), ()> {
        let bytes = inbound_frame_bytes(&frame);
        // Parsing terminal graphics can briefly take longer than the socket reader needs to fill
        // this bounded mailbox. Backpressure that reader instead of tearing down a healthy local
        // session: the server still owns per-client outbox isolation, so a genuinely slow client
        // cannot stall broadcasts to everyone else.
        //
        // Keep all adjacent output for one pane in one entry, up to the mailbox's byte cap. A
        // browser redraw is hundreds of KiB split across many server frames; delivering every
        // 64-KiB piece as a separate UI message forces redundant paints and can make the reader
        // outrun the parser even though one batched pass catches up.
        let result = self.queue.push_blocking_with(
            InboundEvent::Frame(Box::new(frame)),
            bytes,
            coalesce_inbound,
        );
        if let Err(error) = result {
            let message = match error {
                PushError::TooLarge(_) => "session inbound frame exceeded 8 MiB",
                PushError::Closed(_) => "session inbound queue closed",
                // Blocking insertion waits for capacity, so `Full` is not produced.
                PushError::Full(_) => "session inbound queue unavailable",
            };
            self.fail(message.to_string());
            return Err(());
        }
        self.schedule();
        Ok(())
    }

    pub(crate) fn activate(self: &Arc<Self>) {
        self.active.store(true, Ordering::Release);
        self.schedule();
    }

    pub(crate) fn pop(&self) -> Option<InboundEvent> {
        self.queue.try_pop()
    }

    pub(crate) fn session_name(&self) -> String {
        self.session_name.clone()
    }

    pub(crate) fn finish_drain(self: &Arc<Self>) {
        self.scheduled.store(false, Ordering::Release);
        if self.queue.stats().len > 0 {
            self.schedule();
        }
    }

    fn disconnected(self: &Arc<Self>) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            let _ = self.queue.try_push(InboundEvent::Disconnected, 0);
            self.schedule();
        }
    }

    pub(crate) fn fail(self: &Arc<Self>, message: String) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            let _ = self.queue.try_push(InboundEvent::Failed(message), 0);
            self.schedule();
        }
    }

    fn schedule(self: &Arc<Self>) {
        if self.active.load(Ordering::Acquire)
            && self
                .scheduled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.link.send(crate::Msg::DrainSessionFrames {
                epoch: self.epoch,
                mailbox: Arc::clone(self),
            });
        }
    }
}

fn inbound_frame_bytes(frame: &Frame<ServerMessage>) -> usize {
    match frame {
        Frame::PaneBytes { bytes, .. } => bytes.len().saturating_add(protocol::PANE_FRAME_OVERHEAD),
        Frame::Control(message) => serde_json::to_vec(message)
            .map(|bytes| bytes.len().saturating_add(5))
            .unwrap_or(5),
    }
}

fn coalesce_inbound(back: &mut InboundEvent, next: &InboundEvent) -> bool {
    let (InboundEvent::Frame(back), InboundEvent::Frame(next)) = (back, next) else {
        return false;
    };
    let (
        Frame::PaneBytes {
            pane_id,
            local,
            generation,
            bytes,
        },
        Frame::PaneBytes {
            pane_id: next_pane,
            local: next_local,
            generation: next_generation,
            bytes: next_bytes,
        },
    ) = (back.as_mut(), next.as_ref())
    else {
        return false;
    };
    if pane_id != next_pane || local != next_local || generation != next_generation {
        return false;
    }
    bytes.extend_from_slice(next_bytes);
    true
}

impl ClientOutbound {
    fn wire_bytes(&self) -> usize {
        match self {
            Self::Control(message) => serde_json::to_vec(message)
                .map(|bytes| bytes.len().saturating_add(5))
                .unwrap_or(5),
            Self::PaneInput { bytes, .. } => {
                bytes.len().saturating_add(protocol::PANE_FRAME_OVERHEAD)
            }
        }
    }
}

/// Validate the attach reply and return the negotiated wire version.
fn validate_attached(attached: &ServerMessage) -> io::Result<u32> {
    if let ServerMessage::Error { code, message } = attached {
        // A version skew (an older server still running an earlier wire protocol) is the common
        // cause here; give the user something actionable instead of a debug dump.
        let detail = if code == "protocol-mismatch" {
            format!("runs an incompatible rozi version ({message}); kill it and start a new one")
        } else {
            message.clone()
        };
        return Err(io::Error::new(io::ErrorKind::InvalidData, detail));
    }
    let ServerMessage::Attached {
        protocol_version: server_max,
        effective_protocol,
        ..
    } = attached
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("attach handshake failed: {attached:?}"),
        ));
    };
    // Pre-negotiation servers omit effective_protocol (serde default 0) and echo their own
    // protocol_version; treat that echo as the effective version.
    let effective = if *effective_protocol == 0 {
        *server_max
    } else {
        *effective_protocol
    };
    if !(MIN_SUPPORTED_PROTOCOL..=PROTOCOL_VERSION).contains(&effective) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "runs an incompatible rozi version (negotiated protocol {effective}; client supports {MIN_SUPPORTED_PROTOCOL}-{PROTOCOL_VERSION}); kill it and start a new one"
            ),
        ));
    }
    Ok(effective)
}

fn forward_inbound<R: std::io::Read>(
    reader: &mut R,
    inbound: &InboundTarget,
    outbound: Option<&Arc<ByteQueue<ClientOutbound>>>,
    latest_server_metrics: Option<&Arc<Mutex<Option<TimedServerRuntimeMetrics>>>>,
    metrics_request_pending: Option<&Arc<AtomicBool>>,
    request_metrics_on_heartbeat: bool,
    shutdown_signal: Option<&Arc<AtomicBool>>,
) {
    let mut decoder = protocol::FrameDecoder::default();
    'read: loop {
        if shutdown_signal.is_some_and(|sig| sig.load(Ordering::Relaxed)) {
            break;
        }
        let would_block = match decoder.read_from_status(reader) {
            Ok(protocol::FrameReadStatus::Eof) => break,
            Ok(protocol::FrameReadStatus::Read(_)) => false,
            Ok(protocol::FrameReadStatus::WouldBlock) => true,
            Err(_) => break,
        };
        loop {
            if shutdown_signal.is_some_and(|sig| sig.load(Ordering::Relaxed)) {
                break 'read;
            }
            match decoder.next_frame::<ServerMessage>() {
                Ok(Some(frame)) => {
                    if let Frame::Control(ServerMessage::Ping { seq }) = &frame
                        && let Some(outbound) = outbound
                    {
                        if shutdown_signal.is_some_and(|sig| sig.load(Ordering::Relaxed)) {
                            break 'read;
                        }
                        let pong = ClientOutbound::Control(ClientMessage::Pong { seq: *seq });
                        let bytes = pong.wire_bytes();
                        if outbound.try_push(pong, bytes).is_err() {
                            break 'read;
                        }
                        if request_metrics_on_heartbeat
                            && let Some(pending) = metrics_request_pending
                        {
                            try_enqueue_runtime_metrics_request(outbound, pending);
                        }
                        continue;
                    }
                    if let Frame::Control(ServerMessage::RuntimeMetrics { metrics }) = &frame
                        && let Some(cache) = latest_server_metrics
                    {
                        *cache.lock().expect("server metrics cache poisoned") =
                            Some(TimedServerRuntimeMetrics::received(metrics.clone()));
                        if let Some(pending) = metrics_request_pending {
                            pending.store(false, Ordering::Release);
                        }
                    }
                    if inbound.send(frame).is_err() {
                        break 'read;
                    }
                }
                Ok(None) => break,
                Err(_) => break 'read,
            }
        }
        if would_block {
            thread::sleep(Duration::from_millis(1));
        }
    }
    inbound.disconnected();
}

fn try_enqueue_runtime_metrics_request(outbound: &ByteQueue<ClientOutbound>, pending: &AtomicBool) {
    if pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let request = ClientOutbound::Control(ClientMessage::RequestRuntimeMetrics);
    let bytes = request.wire_bytes();
    // Instrumentation is best-effort and must never fail the transport.
    if outbound.try_push(request, bytes).is_err() {
        pending.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::io::Read;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    fn attached_message() -> ServerMessage {
        ServerMessage::Attached {
            protocol_version: PROTOCOL_VERSION,
            effective_protocol: PROTOCOL_VERSION,
            session: "test".to_string(),
            client_id: 7,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(7),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            created_from_profile: None,
        }
    }

    #[test]
    #[cfg(unix)]
    fn attached_stream_decodes_control_and_pane_frames() {
        let (mut client_stream, mut server_stream) = UnixStream::pair().expect("socket pair");
        let server = std::thread::spawn(move || {
            protocol::write_frame(&mut server_stream, &ServerMessage::Ping { seq: 11 })
                .expect("write control frame");
            protocol::write_pane_output_frame(&mut server_stream, 3, 5, false, b"ready\n")
                .expect("write pane frame");
        });

        let (inbound_tx, inbound_rx) = mpsc::channel();
        forward_inbound(
            &mut client_stream,
            &InboundTarget::Channel(inbound_tx),
            None,
            None,
            None,
            false,
            None,
        );
        assert_eq!(
            inbound_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("control frame"),
            Frame::Control(ServerMessage::Ping { seq: 11 })
        );
        assert_eq!(
            inbound_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("pane frame"),
            Frame::PaneBytes {
                pane_id: 3,
                local: false,
                generation: 5,
                bytes: b"ready\n".to_vec(),
            }
        );
        server.join().expect("server thread");
    }

    #[test]
    fn attach_error_is_returned_without_starting_client_threads() {
        let error = validate_attached(&ServerMessage::Error {
            code: "protocol-mismatch".to_string(),
            message: "server uses protocol 2".to_string(),
        })
        .expect_err("attach must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("incompatible rozi version"));
        assert!(validate_attached(&attached_message()).is_ok());
        assert!(validate_attached(&ServerMessage::Ping { seq: 1 }).is_err());
    }

    #[test]
    fn transport_replies_to_ping_without_waiting_for_ui_dispatch() {
        let mut bytes = Vec::new();
        protocol::write_frame(&mut bytes, &ServerMessage::Ping { seq: 42 }).unwrap();
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let outbound = Arc::new(ByteQueue::new(MAX_CLIENT_OUTBOUND_BYTES));

        forward_inbound(
            &mut std::io::Cursor::new(bytes),
            &InboundTarget::Channel(inbound_tx),
            Some(&outbound),
            None,
            None,
            false,
            None,
        );

        assert_eq!(
            outbound.try_pop().unwrap(),
            ClientOutbound::Control(ClientMessage::Pong { seq: 42 })
        );
        assert!(inbound_rx.try_recv().is_err());
    }

    #[test]
    fn set_pane_status_queues_control_message() {
        let (client, outbound) = SessionClient::test_channel();
        client.set_pane_status(
            3,
            5,
            false,
            Some("blocked".to_string()),
            Some("needs approval".to_string()),
        );
        assert_eq!(
            outbound.try_recv().unwrap(),
            ClientOutbound::Control(ClientMessage::SetPaneStatus {
                pane_id: 3,
                local: false,
                generation: 5,
                status: Some("blocked".to_string()),
                reason: Some("needs approval".to_string()),
            })
        );
    }

    #[test]
    fn large_paste_counts_bytes_and_overflow_fails_transport_explicitly() {
        let capacity = 64;
        let outbound = Arc::new(ByteQueue::new(capacity));
        let client = SessionClient {
            transport: None,
            outbound: Arc::clone(&outbound),
            transport_failed: Arc::new(AtomicBool::new(false)),
            inbound: None,
            latest_server_metrics: Arc::new(Mutex::new(None)),
            metrics_request_pending: Arc::new(AtomicBool::new(false)),
            piped_buffer: None,
            test_observer: None,
            server_pid: None,
            effective_protocol: PROTOCOL_VERSION,
            cell: tui_lipan::TerminalCellSize::default(),
        };

        client.send_input(
            1,
            1,
            false,
            vec![b'x'; capacity - protocol::PANE_FRAME_OVERHEAD],
        );
        assert_eq!(outbound.stats().bytes, capacity);
        let stats = client.runtime_stats().outbound;
        assert_eq!(stats.bytes.current_bytes, capacity as u64);
        assert_eq!(stats.bytes.high_water_bytes, capacity as u64);
        assert_eq!(stats.bytes.capacity_bytes, capacity as u64);
        assert_eq!(stats.queued_items, 1);
        assert!(!client.transport_failed.load(Ordering::Acquire));
        client.send_input(1, 1, false, vec![b'y']);
        assert!(client.transport_failed.load(Ordering::Acquire));
        assert!(outbound.stats().closed);
        assert_eq!(outbound.stats().high_water_bytes, capacity);
    }

    #[test]
    fn metrics_refresh_is_best_effort_and_coalesces_while_pending() {
        let outbound = ByteQueue::new(1024);
        let pending = AtomicBool::new(false);
        try_enqueue_runtime_metrics_request(&outbound, &pending);
        try_enqueue_runtime_metrics_request(&outbound, &pending);
        assert_eq!(outbound.stats().len, 1);
        assert!(pending.load(Ordering::Acquire));

        let full = ByteQueue::new(1);
        let full_pending = AtomicBool::new(false);
        try_enqueue_runtime_metrics_request(&full, &full_pending);
        assert_eq!(full.stats().len, 0);
        assert!(!full_pending.load(Ordering::Acquire));
    }

    #[test]
    fn inbound_coalescing_preserves_transcript_and_control_boundaries() {
        let mut output = InboundEvent::Frame(Box::new(Frame::PaneBytes {
            pane_id: 1,
            local: false,
            generation: 2,
            bytes: b"alpha".to_vec(),
        }));
        let continuation = InboundEvent::Frame(Box::new(Frame::PaneBytes {
            pane_id: 1,
            local: false,
            generation: 2,
            bytes: b"beta".to_vec(),
        }));
        assert!(coalesce_inbound(&mut output, &continuation));
        assert_eq!(
            output,
            InboundEvent::Frame(Box::new(Frame::PaneBytes {
                pane_id: 1,
                local: false,
                generation: 2,
                bytes: b"alphabeta".to_vec(),
            }))
        );
        assert!(!coalesce_inbound(
            &mut output,
            &InboundEvent::Frame(Box::new(Frame::PaneBytes {
                pane_id: 1,
                local: true,
                generation: 2,
                bytes: b"gamma".to_vec(),
            }))
        ));
        assert!(!coalesce_inbound(
            &mut output,
            &InboundEvent::Frame(Box::new(Frame::Control(ServerMessage::Ping { seq: 1 })))
        ));
    }

    #[test]
    fn inbound_coalescing_keeps_large_browser_redraws_in_one_ui_delivery() {
        let first = vec![b'a'; 64 * 1024];
        let second = vec![b'b'; 512 * 1024];
        let mut output = InboundEvent::Frame(Box::new(Frame::PaneBytes {
            pane_id: 1,
            local: false,
            generation: 2,
            bytes: first,
        }));
        let continuation = InboundEvent::Frame(Box::new(Frame::PaneBytes {
            pane_id: 1,
            local: false,
            generation: 2,
            bytes: second,
        }));

        assert!(coalesce_inbound(&mut output, &continuation));
        let InboundEvent::Frame(frame) = output else {
            panic!("coalesced pane output");
        };
        let Frame::PaneBytes { bytes, .. } = *frame else {
            panic!("coalesced pane bytes");
        };
        assert_eq!(bytes.len(), 576 * 1024);
    }

    #[test]
    #[cfg(windows)]
    fn windows_duplex_transport_delivers_pong_while_reader_polls() {
        let endpoint = IpcEndpoint::at_path(
            std::env::temp_dir().join(format!("rozi-client-duplex-{}.sock", std::process::id())),
        );
        endpoint.remove_stale();
        let listener = endpoint.bind().unwrap().into_listener();
        listener.set_nonblocking(true).unwrap();

        let server = thread::spawn(move || {
            let mut stream = loop {
                match listener.accept() {
                    Ok(stream) => break stream,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(err) => panic!("accept failed: {err}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            assert!(matches!(
                protocol::read_frame::<_, ClientMessage>(&mut stream).unwrap(),
                ClientMessage::Attach { .. }
            ));
            protocol::write_frame(&mut stream, &attached_message()).unwrap();
            protocol::write_frame(&mut stream, &ServerMessage::Ping { seq: 77 }).unwrap();
            // The client drives its own traffic too - a runtime-metrics request goes out on attach
            // - so the pong is not necessarily the next frame on the wire. What this test is about
            // is that the pong arrives at all while the reader is polling, not that it arrives
            // first. The read timeout above bounds the loop.
            let pong = loop {
                let message = protocol::read_frame::<_, ClientMessage>(&mut stream).unwrap();
                if matches!(message, ClientMessage::Pong { .. }) {
                    break message;
                }
            };
            assert_eq!(pong, ClientMessage::Pong { seq: 77 });
        });

        let (inbound_tx, _inbound_rx) = mpsc::channel();
        let (_client, attached) =
            SessionClient::connect_attached(&endpoint, "test", inbound_tx, false).unwrap();
        assert!(matches!(attached, ServerMessage::Attached { .. }));
        server.join().unwrap();
        endpoint.remove_stale();
    }

    #[test]
    #[cfg(unix)]
    fn dropping_final_client_owner_shuts_down_transport() {
        let (client_stream, mut server_stream) = UnixStream::pair().expect("socket pair");
        let (inbound_tx, _inbound_rx) = mpsc::channel();
        let attached = attached_message();

        let server = thread::spawn(move || {
            let msg: ClientMessage = protocol::read_frame(&mut server_stream).expect("read attach");
            assert!(matches!(msg, ClientMessage::Attach { .. }));
            protocol::write_frame(&mut server_stream, &attached).expect("write attached");

            // Consume possible metrics request
            let _ = protocol::read_frame::<_, ClientMessage>(&mut server_stream);

            // Wait for EOF once client drops
            let mut buf = [0u8; 16];
            let read = server_stream.read(&mut buf);
            assert!(matches!(read, Ok(0) | Err(_)));
        });

        let (client, _attached) = SessionClient::from_stream_attached(
            crate::platform::ipc::IpcConnection::from_unix(client_stream),
            "test",
            inbound_tx,
            false,
        )
        .expect("from_stream_attached");

        // Clone simulating background/parked attachment
        let parked_clone = client.clone();
        drop(client);
        assert!(parked_clone.transport.is_some());
        thread::sleep(Duration::from_millis(50));

        // Dropping final clone shuts down the stream
        drop(parked_clone);
        server.join().expect("server thread finished on eof");
    }
}
