use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control;
use crate::session::protocol::{
    self, ClientInfo, ClientMessage, ControllerChangeReason, Frame, PROTOCOL_VERSION, PaneMeta,
    ServerMessage, WirePalette,
};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;
const DEFAULT_SCROLLBACK: usize = 5000;
/// How long an *ephemeral* session server survives with no client attached before it self-reaps,
/// regardless of pane state. This is only a crash/abnormal-exit backstop: a clean quit or normal
/// transition tears an ephemeral server down client-side (`ClientMessage::Shutdown`). A *named*
/// session never self-reaps from client absence — it is durable until explicitly killed.
const EPHEMERAL_NO_CLIENT_GRACE: Duration = Duration::from_secs(45);
/// How often the server pings each attached client.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long a client may go without a pong before it is disconnected (and its lease released). A
/// wedged UI loses control deliberately; a merely busy one has ample slack.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);
/// Minimum spacing between successful layout-control takeovers, to stop rapid steal ping-pong.
const TAKEOVER_COOLDOWN: Duration = Duration::from_secs(3);
/// Default per-client outbox cap; a client backed up past this is disconnected so it can never
/// stall the broadcast to everyone else.
const DEFAULT_MAX_BACKLOG: usize = 8 * 1024 * 1024;
/// Larger cap while a client is still receiving its initial replay seed.
const SEED_MAX_BACKLOG: usize = 64 * 1024 * 1024;
const SEED_CHUNK: usize = 256 * 1024;

pub struct SessionServer {
    panes: HashMap<PaneId, ServerPane>,
    next_generation: u64,
    layout: Option<SharedLayout>,
    layout_rev: u64,
    controller: Option<ClientId>,
    input_locked: bool,
    last_takeover: Option<Instant>,
    clients: Vec<ClientConn>,
    next_client_id: ClientId,
    max_backlog: usize,
    event_rx: mpsc::Receiver<ServerEvent>,
    event_tx: mpsc::Sender<ServerEvent>,
    shutdown: bool,
    session_name: String,
    /// The socket file this server currently listens on. Set by [`run_named_session`]; a rename
    /// moves this file in place so the running listener keeps serving under the new name.
    socket_path: Option<PathBuf>,
}

pub struct ServerPane {
    pub generation: u64,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub pty: Option<TerminalPty>,
    pub screen: TerminalScreen,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
}

/// One attached (or connecting) client. The stream is non-blocking; outbound frames are
/// pre-encoded and queued in `outbox`, flushed opportunistically so a slow reader never blocks the
/// server or the other clients.
struct ClientConn {
    id: ClientId,
    stream: UnixStream,
    decoder: protocol::FrameDecoder,
    outbox: VecDeque<Vec<u8>>,
    outbox_bytes: usize,
    /// Bytes of `outbox.front()` already written (non-blocking writes can be partial).
    front_offset: usize,
    attached: bool,
    label: Option<String>,
    read_only: bool,
    /// True while the initial replay seed is still queued; raises the backlog cap.
    seeding: bool,
    /// Close this connection once its outbox drains (query probes, rejected attaches).
    close_after_flush: bool,
    last_pong: Instant,
    last_ping: Instant,
    ping_seq: u64,
}

impl ClientConn {
    fn new(id: ClientId, stream: UnixStream) -> Self {
        let now = Instant::now();
        Self {
            id,
            stream,
            decoder: protocol::FrameDecoder::default(),
            outbox: VecDeque::new(),
            outbox_bytes: 0,
            front_offset: 0,
            attached: false,
            label: None,
            read_only: false,
            seeding: false,
            close_after_flush: false,
            last_pong: now,
            last_ping: now,
            ping_seq: 0,
        }
    }

    fn push(&mut self, bytes: Vec<u8>) {
        self.outbox_bytes += bytes.len();
        self.outbox.push_back(bytes);
    }

    fn backlog_cap(&self, default: usize) -> usize {
        if self.seeding {
            SEED_MAX_BACKLOG
        } else {
            default
        }
    }
}

enum ServerEvent {
    Pty(PaneId, u64, TerminalPtyEvent),
}

enum ServerOutbound {
    Control(ServerMessage),
    PaneOutput {
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
}

/// Where a response frame should go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Just the client that sent the triggering message.
    Sender,
    /// Every attached client.
    Broadcast,
}

impl SessionServer {
    pub fn new_named(session_name: impl Into<String>) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            panes: HashMap::new(),
            next_generation: 1,
            layout: None,
            layout_rev: 0,
            controller: None,
            input_locked: false,
            last_takeover: None,
            clients: Vec::new(),
            next_client_id: 1,
            max_backlog: DEFAULT_MAX_BACKLOG,
            event_rx,
            event_tx,
            shutdown: false,
            session_name: session_name.into(),
            socket_path: None,
        }
    }

    pub fn run_listener(&mut self, listener: UnixListener) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        // Tracks how long an ephemeral session has had no *attached* client. A named session
        // ignores this timer and is durable until explicitly shut down.
        let mut no_client_since: Option<Instant> = None;
        while !self.shutdown {
            self.accept_new(&listener)?;

            while let Ok(event) = self.event_rx.try_recv() {
                if let Some(outbound) = self.handle_event(event) {
                    self.broadcast_outbound(&outbound);
                }
            }

            self.pump_clients();
            self.heartbeat();
            self.flush_clients();

            let attached = self.attached_count();
            if attached == 0 {
                if crate::state::is_ephemeral_session_name(&self.session_name) {
                    let since = no_client_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= EPHEMERAL_NO_CLIENT_GRACE {
                        self.shutdown = true;
                    }
                }
            } else {
                no_client_since = None;
            }

            let idle = if self.clients.is_empty() { 20 } else { 6 };
            std::thread::sleep(Duration::from_millis(idle));
        }
        for pane in self.panes.values() {
            if let Some(pty) = &pane.pty {
                let _ = pty.kill();
            }
        }
        Ok(())
    }

    fn accept_new(&mut self, listener: &UnixListener) -> io::Result<()> {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    self.clients.push(ClientConn::new(id, stream));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    }

    fn pump_clients(&mut self) {
        let mut inbound: Vec<(ClientId, Frame<ClientMessage>)> = Vec::new();
        let mut dead: Vec<ClientId> = Vec::new();
        for client in &mut self.clients {
            match client.decoder.read_from_status(&mut client.stream) {
                Ok(protocol::FrameReadStatus::Eof) => dead.push(client.id),
                Ok(_) => {}
                Err(_) => dead.push(client.id),
            }
            loop {
                match client.decoder.next_frame::<ClientMessage>() {
                    Ok(Some(frame)) => inbound.push((client.id, frame)),
                    Ok(None) => break,
                    Err(_) => {
                        dead.push(client.id);
                        break;
                    }
                }
            }
        }

        for (id, frame) in inbound {
            self.process_client_frame(id, frame);
        }
        for id in dead {
            self.remove_client(id);
        }
    }

    fn process_client_frame(&mut self, id: ClientId, frame: Frame<ClientMessage>) {
        match frame {
            Frame::PaneBytes {
                pane_id,
                generation,
                bytes,
            } => {
                if self.client_may_input(id) {
                    self.handle_pane_input(pane_id, generation, &bytes);
                }
            }
            Frame::Control(message) => {
                let is_attach = matches!(message, ClientMessage::Attach { .. });
                let is_query = matches!(message, ClientMessage::Query { .. });
                if !is_attach && !is_query && !self.client_attached(id) {
                    self.enqueue(
                        id,
                        Target::Sender,
                        ServerMessage::Error {
                            code: "attach-required".to_string(),
                            message: "first client message must be attach".to_string(),
                        },
                    );
                    self.set_close_after_flush(id);
                    return;
                }
                let detach = matches!(message, ClientMessage::Detach);
                let responses = self.handle_message(id, message);
                for (target, msg) in responses {
                    self.enqueue(id, target, msg);
                }
                if is_attach {
                    if self.client_attached(id) {
                        self.enqueue_attach_seeds(id);
                    } else {
                        // A failed attach (protocol/session mismatch) sent an error; close it.
                        self.set_close_after_flush(id);
                    }
                }
                if is_query {
                    self.set_close_after_flush(id);
                }
                if detach {
                    self.remove_client(id);
                }
            }
        }
    }

    fn handle_message(
        &mut self,
        client_id: ClientId,
        message: ClientMessage,
    ) -> Vec<(Target, ServerMessage)> {
        match message {
            ClientMessage::Attach {
                session,
                protocol_version,
                label,
                read_only,
            } => self.handle_attach(client_id, session, protocol_version, label, read_only),
            ClientMessage::Query {
                session,
                protocol_version,
            } => self.handle_query(session, protocol_version),
            ClientMessage::SpawnPane {
                pane_id,
                generation,
                command,
                cwd,
                cols,
                rows,
                keep_open,
                env,
                title,
            } => {
                if !self.is_controller(client_id) {
                    return vec![(
                        Target::Sender,
                        ServerMessage::SpawnResult {
                            pane_id,
                            generation,
                            pid: None,
                            ok: false,
                            error: Some("not controller".to_string()),
                        },
                    )];
                }
                vec![(
                    Target::Sender,
                    self.spawn_pane(SpawnRequest {
                        pane_id,
                        generation,
                        command,
                        cwd,
                        title,
                        cols,
                        rows,
                        keep_open,
                        env,
                    }),
                )]
            }
            ClientMessage::Resize {
                pane_id,
                generation,
                cols,
                rows,
            } => {
                if !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(pane_id, generation) {
                    pane.cols = cols.max(1);
                    pane.rows = rows.max(1);
                    pane.screen.resize(pane.rows, pane.cols);
                    if let Some(pty) = &pane.pty {
                        let _ = pty.resize(pane.cols, pane.rows);
                    }
                    // Broadcast so every client's parser reshapes at the same byte position.
                    return vec![(
                        Target::Broadcast,
                        ServerMessage::Resized {
                            pane_id,
                            generation,
                            cols: pane.cols,
                            rows: pane.rows,
                        },
                    )];
                }
                Vec::new()
            }
            ClientMessage::Kill {
                pane_id,
                generation,
            } => {
                if !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(pane_id, generation)
                    && let Some(pty) = &pane.pty
                {
                    let _ = pty.kill();
                }
                Vec::new()
            }
            ClientMessage::SetPalette {
                pane_id,
                generation,
                palette,
            } => {
                if !self.is_controller(client_id) {
                    return Vec::new();
                }
                self.apply_palette(pane_id, generation, palette);
                Vec::new()
            }
            ClientMessage::ConfigurePane {
                pane_id,
                generation,
                palette,
                title,
                cwd,
            } => {
                if !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(pane_id, generation) {
                    if let Some(title) = title {
                        pane.title = Some(title);
                    }
                    if let Some(cwd) = cwd {
                        pane.cwd = Some(cwd);
                    }
                    if let Some(palette) = palette {
                        pane.screen.set_palette(palette.into());
                    }
                }
                Vec::new()
            }
            ClientMessage::CommitLayout { base_rev, layout } => {
                self.handle_commit_layout(client_id, base_rev, layout)
            }
            ClientMessage::TakeControl => self.handle_take_control(client_id),
            ClientMessage::GrantControl { to } => self.handle_grant_control(client_id, to),
            ClientMessage::SetInputLock { locked } => {
                if !self.is_controller(client_id) || self.client_read_only(client_id) {
                    return Vec::new();
                }
                self.input_locked = locked;
                vec![(Target::Broadcast, self.clients_changed())]
            }
            ClientMessage::Pong { seq: _ } => {
                if let Some(client) = self.client_mut(client_id) {
                    client.last_pong = Instant::now();
                }
                Vec::new()
            }
            ClientMessage::Rename { name } => {
                if !self.is_controller(client_id) {
                    return Vec::new();
                }
                vec![(Target::Broadcast, self.rename_session(name))]
            }
            ClientMessage::Detach => Vec::new(),
            ClientMessage::Shutdown => {
                if !self.is_controller(client_id) || self.client_read_only(client_id) {
                    return Vec::new();
                }
                self.shutdown = true;
                for pane in self.panes.values() {
                    if let Some(pty) = &pane.pty {
                        let _ = pty.kill();
                    }
                }
                Vec::new()
            }
        }
    }

    fn handle_attach(
        &mut self,
        client_id: ClientId,
        session: String,
        protocol_version: u32,
        label: String,
        read_only: bool,
    ) -> Vec<(Target, ServerMessage)> {
        if protocol_version != PROTOCOL_VERSION {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "protocol-mismatch".to_string(),
                    message: format!(
                        "client protocol {protocol_version} is incompatible with server protocol {PROTOCOL_VERSION}"
                    ),
                },
            )];
        }
        if session != self.session_name {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "session-mismatch".to_string(),
                    message: format!(
                        "client requested session {session:?}, but this server owns {:?}",
                        self.session_name
                    ),
                },
            )];
        }
        if let Some(client) = self.client_mut(client_id) {
            client.attached = true;
            client.label = Some(label);
            client.read_only = read_only;
            client.last_pong = Instant::now();
        }
        // First attacher is auto-granted the layout-control lease.
        let granted = if self.controller.is_none() && !read_only {
            self.controller = Some(client_id);
            true
        } else {
            false
        };
        let clients = self.client_roster();
        let attached = ServerMessage::Attached {
            protocol_version: PROTOCOL_VERSION,
            session,
            client_id,
            panes: self.pane_meta(),
            layout_rev: self.layout_rev,
            layout: self.layout.clone(),
            controller: self.controller,
            clients,
            input_locked: self.input_locked,
        };
        let mut responses = vec![(Target::Sender, attached)];
        responses.push((Target::Broadcast, self.clients_changed()));
        if granted {
            responses.push((
                Target::Broadcast,
                ServerMessage::ControllerChanged {
                    controller: self.controller,
                    reason: ControllerChangeReason::Granted,
                },
            ));
        }
        responses
    }

    fn handle_query(
        &mut self,
        session: String,
        protocol_version: u32,
    ) -> Vec<(Target, ServerMessage)> {
        if protocol_version != PROTOCOL_VERSION {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "protocol-mismatch".to_string(),
                    message: format!(
                        "client protocol {protocol_version} is incompatible with server protocol {PROTOCOL_VERSION}"
                    ),
                },
            )];
        }
        if session != self.session_name {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "session-mismatch".to_string(),
                    message: format!(
                        "client requested session {session:?}, but this server owns {:?}",
                        self.session_name
                    ),
                },
            )];
        }
        let panes = self
            .panes
            .values()
            .filter(|pane| pane.exited.is_none())
            .count();
        vec![(
            Target::Sender,
            ServerMessage::SessionInfo {
                session,
                panes,
                clients: self.attached_count(),
                has_layout: self.layout.is_some(),
            },
        )]
    }

    fn handle_commit_layout(
        &mut self,
        client_id: ClientId,
        base_rev: u64,
        layout: SharedLayout,
    ) -> Vec<(Target, ServerMessage)> {
        // Non-controller commits are silently dropped (client-side gating already blocks them;
        // this is defense in depth). The follower resyncs its base rev from ControllerChanged.
        if !self.is_controller(client_id) || self.client_read_only(client_id) {
            return Vec::new();
        }
        if base_rev != self.layout_rev {
            return vec![(
                Target::Sender,
                ServerMessage::LayoutRejected {
                    current_rev: self.layout_rev,
                    layout: self.layout.clone(),
                },
            )];
        }
        self.layout_rev += 1;
        self.layout = Some(layout.clone());
        vec![(
            Target::Broadcast,
            ServerMessage::LayoutCommitted {
                rev: self.layout_rev,
                author: client_id,
                layout,
            },
        )]
    }

    fn handle_take_control(&mut self, client_id: ClientId) -> Vec<(Target, ServerMessage)> {
        if self.client_read_only(client_id) {
            return Vec::new();
        }
        if self.controller == Some(client_id) {
            return Vec::new();
        }
        if let Some(last) = self.last_takeover
            && last.elapsed() < TAKEOVER_COOLDOWN
        {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "takeover-cooldown".to_string(),
                    message: "layout control was just taken; try again in a moment".to_string(),
                },
            )];
        }
        self.controller = Some(client_id);
        self.last_takeover = Some(Instant::now());
        vec![(
            Target::Broadcast,
            ServerMessage::ControllerChanged {
                controller: self.controller,
                reason: ControllerChangeReason::Taken,
            },
        )]
    }

    fn handle_grant_control(
        &mut self,
        client_id: ClientId,
        to: ClientId,
    ) -> Vec<(Target, ServerMessage)> {
        if !self.is_controller(client_id) || !self.client_attached(to) || self.client_read_only(to)
        {
            return Vec::new();
        }
        self.controller = Some(to);
        vec![(
            Target::Broadcast,
            ServerMessage::ControllerChanged {
                controller: self.controller,
                reason: ControllerChangeReason::Granted,
            },
        )]
    }

    fn is_controller(&self, client_id: ClientId) -> bool {
        self.controller == Some(client_id)
    }

    fn client_mut(&mut self, id: ClientId) -> Option<&mut ClientConn> {
        self.clients.iter_mut().find(|client| client.id == id)
    }

    fn client_attached(&self, id: ClientId) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == id && client.attached)
    }

    fn client_read_only(&self, id: ClientId) -> bool {
        self.clients
            .iter()
            .find(|client| client.id == id && client.attached)
            .is_none_or(|client| client.read_only)
    }

    fn client_may_input(&self, id: ClientId) -> bool {
        self.client_attached(id)
            && !self.client_read_only(id)
            && (!self.input_locked || self.is_controller(id))
    }

    fn client_roster(&self) -> Vec<ClientInfo> {
        self.clients
            .iter()
            .filter(|client| client.attached)
            .map(|client| ClientInfo {
                id: client.id,
                label: client.label.clone().unwrap_or_else(|| "client".to_string()),
                read_only: client.read_only,
            })
            .collect()
    }

    fn clients_changed(&self) -> ServerMessage {
        ServerMessage::ClientsChanged {
            clients: self.client_roster(),
            input_locked: self.input_locked,
        }
    }

    fn attached_count(&self) -> u32 {
        self.clients.iter().filter(|client| client.attached).count() as u32
    }

    fn set_close_after_flush(&mut self, id: ClientId) {
        if let Some(client) = self.client_mut(id) {
            client.close_after_flush = true;
        }
    }

    /// Remove a client, promoting the oldest surviving attached client to controller if the leaver
    /// held the lease, and broadcasting the resulting client/controller changes.
    fn remove_client(&mut self, id: ClientId) {
        let Some(index) = self.clients.iter().position(|client| client.id == id) else {
            return;
        };
        let removed = self.clients.remove(index);
        if !removed.attached {
            return;
        }
        let mut messages: Vec<ServerMessage> = Vec::new();
        if self.controller == Some(id) {
            // Auto-promote the oldest remaining attached client (smallest id = earliest connect).
            self.controller = self
                .clients
                .iter()
                .filter(|client| client.attached && !client.read_only)
                .map(|client| client.id)
                .min();
            messages.push(ServerMessage::ControllerChanged {
                controller: self.controller,
                reason: if self.controller.is_some() {
                    ControllerChangeReason::Granted
                } else {
                    ControllerChangeReason::Released
                },
            });
        }
        messages.push(self.clients_changed());
        for message in messages {
            self.broadcast_control(&message);
        }
    }

    fn heartbeat(&mut self) {
        let now = Instant::now();
        let mut timed_out: Vec<ClientId> = Vec::new();
        let mut pings: Vec<(ClientId, u64)> = Vec::new();
        for client in &mut self.clients {
            if !client.attached {
                continue;
            }
            if now.duration_since(client.last_pong) >= HEARTBEAT_TIMEOUT {
                timed_out.push(client.id);
                continue;
            }
            if now.duration_since(client.last_ping) >= HEARTBEAT_INTERVAL {
                client.last_ping = now;
                client.ping_seq += 1;
                pings.push((client.id, client.ping_seq));
            }
        }
        for (id, seq) in pings {
            self.enqueue(id, Target::Sender, ServerMessage::Ping { seq });
        }
        for id in timed_out {
            self.remove_client(id);
        }
    }

    fn flush_clients(&mut self) {
        let default_cap = self.max_backlog;
        let mut dead: Vec<ClientId> = Vec::new();
        for client in &mut self.clients {
            let mut disconnect = false;
            while let Some(front) = client.outbox.front() {
                let chunk = &front[client.front_offset..];
                match client.stream.write(chunk) {
                    Ok(0) => {
                        disconnect = true;
                        break;
                    }
                    Ok(n) => {
                        client.front_offset += n;
                        client.outbox_bytes -= n;
                        if client.front_offset >= front.len() {
                            client.outbox.pop_front();
                            client.front_offset = 0;
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        break;
                    }
                    Err(_) => {
                        disconnect = true;
                        break;
                    }
                }
            }
            if client.outbox.is_empty() {
                client.seeding = false;
                if client.close_after_flush {
                    disconnect = true;
                }
            } else if client.outbox_bytes > client.backlog_cap(default_cap) {
                // A client backed up past its cap is a liability; drop it so broadcasts never stall.
                disconnect = true;
            }
            if disconnect {
                dead.push(client.id);
            }
        }
        for id in dead {
            self.remove_client(id);
        }
    }

    fn enqueue(&mut self, sender_id: ClientId, target: Target, message: ServerMessage) {
        let Some(bytes) = encode_control(&message) else {
            return;
        };
        match target {
            Target::Sender => {
                if let Some(client) = self.client_mut(sender_id) {
                    client.push(bytes);
                }
            }
            Target::Broadcast => {
                self.push_to_attached(bytes);
            }
        }
    }

    /// Queue `bytes` on every attached client, cloning for all but the last recipient.
    fn push_to_attached(&mut self, bytes: Vec<u8>) {
        let last = self.clients.iter().rposition(|client| client.attached);
        let Some(last) = last else { return };
        for (index, client) in self.clients.iter_mut().enumerate() {
            if !client.attached {
                continue;
            }
            if index == last {
                client.push(bytes);
                return;
            }
            client.push(bytes.clone());
        }
    }

    fn broadcast_control(&mut self, message: &ServerMessage) {
        let Some(bytes) = encode_control(message) else {
            return;
        };
        self.push_to_attached(bytes);
    }

    fn broadcast_outbound(&mut self, outbound: &ServerOutbound) {
        let bytes = match outbound {
            ServerOutbound::Control(message) => encode_control(message),
            ServerOutbound::PaneOutput {
                pane_id,
                generation,
                bytes,
            } => encode_pane_output(*pane_id, *generation, bytes),
        };
        let Some(bytes) = bytes else {
            return;
        };
        self.push_to_attached(bytes);
    }

    /// Queue the initial replay seed for a freshly attached client: the exported screen of every
    /// live pane, in 256 KiB chunks, right after `Attached` and before any subsequent live output.
    fn enqueue_attach_seeds(&mut self, id: ClientId) {
        let mut seeds: Vec<Vec<u8>> = Vec::new();
        for (pane_id, pane) in &mut self.panes {
            if pane.exited.is_some() {
                continue;
            }
            let bytes = pane.screen.export_replay_bytes();
            for chunk in bytes.chunks(SEED_CHUNK) {
                if let Some(frame) = encode_pane_output(*pane_id, pane.generation, chunk) {
                    seeds.push(frame);
                }
            }
        }
        if let Some(client) = self.client_mut(id) {
            client.seeding = true;
            for frame in seeds {
                client.push(frame);
            }
        }
    }

    /// Rename this session in place: move the listening socket to the new name so the same server
    /// (and its live panes) becomes discoverable under `name` with zero pane movement. Rejects
    /// invalid names and collisions with an already-running session.
    fn rename_session(&mut self, name: String) -> ServerMessage {
        if !crate::session::discovery::valid_session_name(&name) {
            return ServerMessage::Error {
                code: "invalid-name".to_string(),
                message: format!("invalid session name {name:?}"),
            };
        }
        if name == self.session_name {
            return ServerMessage::Renamed { session: name };
        }
        let new_path = match session_socket_path(&name) {
            Ok(path) => path,
            Err(err) => {
                return ServerMessage::Error {
                    code: "rename-failed".to_string(),
                    message: err.to_string(),
                };
            }
        };
        if new_path.exists() {
            if UnixStream::connect(&new_path).is_ok() {
                return ServerMessage::Error {
                    code: "name-in-use".to_string(),
                    message: format!("session `{name}` already exists"),
                };
            }
            // A stale socket whose server is gone; clear it so the rename can take the name.
            let _ = fs::remove_file(&new_path);
        }
        if let Some(old_path) = self.socket_path.clone() {
            if let Err(err) = fs::rename(&old_path, &new_path) {
                return ServerMessage::Error {
                    code: "rename-failed".to_string(),
                    message: err.to_string(),
                };
            }
            let _ = fs::set_permissions(&new_path, fs::Permissions::from_mode(0o600));
        }
        self.socket_path = Some(new_path);
        self.session_name = name.clone();
        ServerMessage::Renamed { session: name }
    }

    fn spawn_pane(&mut self, request: SpawnRequest) -> ServerMessage {
        let id = request.pane_id;
        // A live pane with this id already exists; refuse. An *exited* pane is replaced in place
        // so keep-open respawn (client re-sends `SpawnPane` with a fresh generation) works.
        if self
            .panes
            .get(&id)
            .is_some_and(|pane| pane.exited.is_none())
        {
            return ServerMessage::SpawnResult {
                pane_id: id,
                generation: request.generation,
                pid: None,
                ok: false,
                error: Some(format!("pane {id} already exists")),
            };
        }
        self.panes.remove(&id);
        let cols = if request.cols == 0 {
            DEFAULT_COLS
        } else {
            request.cols
        };
        let rows = if request.rows == 0 {
            DEFAULT_ROWS
        } else {
            request.rows
        };
        let generation = request.generation;
        self.next_generation = self.next_generation.max(generation.saturating_add(1));
        let mut screen = TerminalScreen::new(rows.max(1), cols.max(1), DEFAULT_SCROLLBACK);
        let mut config = pty_config(request.command.as_deref(), request.keep_open);
        if let Some(cwd) = &request.cwd {
            config = config.cwd(cwd.clone());
        }
        for (key, value) in &request.env {
            config = config.env(key.clone(), value.clone());
        }
        let tx = self.event_tx.clone();
        match TerminalPty::spawn(config, move |event| {
            let _ = tx.send(ServerEvent::Pty(id, generation, event));
        }) {
            Ok(pty) => {
                let pid = pty.pid();
                let _ = pty.resize(cols.max(1), rows.max(1));
                screen.resize(rows.max(1), cols.max(1));
                self.panes.insert(
                    id,
                    ServerPane {
                        generation,
                        title: request.title,
                        cwd: request.cwd,
                        pty: Some(pty),
                        screen,
                        cols: cols.max(1),
                        rows: rows.max(1),
                        exited: None,
                    },
                );
                ServerMessage::SpawnResult {
                    pane_id: id,
                    generation,
                    pid,
                    ok: true,
                    error: None,
                }
            }
            Err(err) => ServerMessage::SpawnResult {
                pane_id: id,
                generation,
                pid: None,
                ok: false,
                error: Some(err.to_string()),
            },
        }
    }

    fn handle_pane_input(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        if let Some(pane) = self.live_pane_mut(pane_id, generation)
            && let Some(pty) = &pane.pty
        {
            let _ = pty.write(bytes);
        }
    }

    fn handle_event(&mut self, event: ServerEvent) -> Option<ServerOutbound> {
        match event {
            ServerEvent::Pty(id, generation, event) => {
                let pane = self.panes.get_mut(&id)?;
                if pane.generation != generation {
                    return None;
                }
                match event {
                    TerminalPtyEvent::Output(bytes) => {
                        pane.screen.process_bytes(&bytes);
                        if let Some(pty) = &pane.pty {
                            for response in pane.screen.drain_responses() {
                                let _ = pty.write(&response);
                            }
                        }
                        Some(ServerOutbound::PaneOutput {
                            pane_id: id,
                            generation,
                            bytes: bytes.to_vec(),
                        })
                    }
                    TerminalPtyEvent::Exited(code) => {
                        pane.exited = Some(code);
                        pane.pty = None;
                        Some(ServerOutbound::Control(ServerMessage::Exited {
                            pane_id: id,
                            generation,
                            code,
                        }))
                    }
                    TerminalPtyEvent::Error(message) => {
                        Some(ServerOutbound::Control(ServerMessage::SpawnResult {
                            pane_id: id,
                            generation,
                            pid: None,
                            ok: false,
                            error: Some(message.to_string()),
                        }))
                    }
                }
            }
        }
    }

    fn live_pane_mut(&mut self, id: PaneId, generation: u64) -> Option<&mut ServerPane> {
        self.panes
            .get_mut(&id)
            .filter(|pane| pane.generation == generation && pane.exited.is_none())
    }

    fn pane_meta(&self) -> Vec<PaneMeta> {
        self.panes
            .iter()
            .map(|(pane_id, pane)| PaneMeta {
                pane_id: *pane_id,
                generation: pane.generation,
                cols: pane.cols,
                rows: pane.rows,
                pid: pane.pty.as_ref().and_then(TerminalPty::pid),
                title: pane.effective_title(),
                cwd: pane.effective_cwd(),
                exited: pane.exited,
            })
            .collect()
    }

    fn apply_palette(&mut self, id: PaneId, generation: u64, palette: WirePalette) {
        if let Some(pane) = self.live_pane_mut(id, generation) {
            pane.screen.set_palette(palette.into());
        }
    }
}

fn encode_control(message: &ServerMessage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    protocol::write_frame(&mut buf, message).ok().map(|()| buf)
}

fn encode_pane_output(pane_id: PaneId, generation: u64, bytes: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    protocol::write_pane_output_frame(&mut buf, pane_id, generation, bytes)
        .ok()
        .map(|()| buf)
}

struct SpawnRequest {
    pane_id: PaneId,
    generation: u64,
    command: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    cols: u16,
    rows: u16,
    keep_open: bool,
    env: Vec<(String, String)>,
}

impl ServerPane {
    fn effective_title(&self) -> Option<String> {
        self.screen.title().or_else(|| self.title.clone())
    }

    fn effective_cwd(&self) -> Option<String> {
        self.pty
            .as_ref()
            .and_then(|pty| pty.pid())
            .and_then(cwd_for_pid)
            .or_else(|| self.cwd.clone())
    }
}

#[cfg(target_os = "linux")]
fn cwd_for_pid(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

#[cfg(not(target_os = "linux"))]
fn cwd_for_pid(_pid: u32) -> Option<String> {
    None
}

fn pty_config(command: Option<&str>, keep_open: bool) -> TerminalPtyConfig {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
        let command = if keep_open {
            format!("{command}; exec {shell}")
        } else {
            command.to_string()
        };
        TerminalPtyConfig::new(shell)
            .arg("-lc")
            .arg(command)
            .term("xterm-256color")
    } else {
        TerminalPtyConfig::new(shell).term("xterm-256color")
    }
}

pub fn session_socket_path(name: &str) -> io::Result<PathBuf> {
    Ok(control::runtime_dir()?.join(format!("session-{}.sock", sanitize_session_name(name))))
}

pub fn bind_session_socket(name: &str) -> io::Result<(UnixListener, PathBuf)> {
    let path = session_socket_path(name)?;
    bind_unix_socket(&path)?;
    let listener = UnixListener::bind(&path)?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok((listener, path))
}

fn bind_unix_socket(path: &Path) -> io::Result<()> {
    if path.exists() && UnixStream::connect(path).is_err() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn run_named_session(name: &str) -> io::Result<()> {
    let (listener, path) = bind_session_socket(name)?;
    let mut server = SessionServer::new_named(name);
    server.socket_path = Some(path);
    let result = server.run_listener(listener);
    // A rename moves the socket file, so unlink the current path rather than the original one.
    if let Some(path) = &server.socket_path {
        let _ = fs::remove_file(path);
    }
    result
}

fn sanitize_session_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Register a client backed by a socketpair and return its id plus the client-side stream.
    fn add_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        server_stream.set_nonblocking(true).unwrap();
        let id = server.next_client_id;
        server.next_client_id += 1;
        server.clients.push(ClientConn::new(id, server_stream));
        (id, client_stream)
    }

    /// Register and attach a client, returning its id and client-side stream.
    fn attach_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
        let (id, stream) = add_client(server);
        let responses = server.handle_message(
            id,
            ClientMessage::Attach {
                session: server.session_name.clone(),
                protocol_version: PROTOCOL_VERSION,
                label: format!("client-{id}"),
                read_only: false,
            },
        );
        assert!(
            responses
                .iter()
                .any(|(_, msg)| matches!(msg, ServerMessage::Attached { .. }))
        );
        (id, stream)
    }

    fn attach_read_only_client(server: &mut SessionServer) -> (ClientId, UnixStream) {
        let (id, stream) = add_client(server);
        server.handle_message(
            id,
            ClientMessage::Attach {
                session: server.session_name.clone(),
                protocol_version: PROTOCOL_VERSION,
                label: format!("viewer-{id}"),
                read_only: true,
            },
        );
        (id, stream)
    }

    #[test]
    fn session_socket_name_is_sanitized() {
        assert!(
            session_socket_path("dev/../../x")
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-dev_______x")
        );
    }

    #[test]
    fn rename_in_place_updates_session_name() {
        let mut server = SessionServer::new_named("eph-123");
        let response = server.rename_session("renametest-unlikely-xyz".into());
        assert!(
            matches!(response, ServerMessage::Renamed { session } if session == "renametest-unlikely-xyz")
        );
        assert_eq!(server.session_name, "renametest-unlikely-xyz");
    }

    #[test]
    fn rename_rejects_reserved_ephemeral_prefix() {
        let mut server = SessionServer::new_named("eph-123");
        let response = server.rename_session("eph-999".into());
        assert!(matches!(response, ServerMessage::Error { code, .. } if code == "invalid-name"));
        assert_eq!(server.session_name, "eph-123");
    }

    #[test]
    fn attach_reports_protocol_mismatch() {
        let mut server = SessionServer::new_named("dev");
        let (id, _stream) = add_client(&mut server);
        let responses = server.handle_message(
            id,
            ClientMessage::Attach {
                session: "dev".into(),
                protocol_version: PROTOCOL_VERSION + 1,
                label: "client".into(),
                read_only: false,
            },
        );
        assert!(
            matches!(responses.as_slice(), [(_, ServerMessage::Error { code, .. })] if code == "protocol-mismatch")
        );
    }

    #[test]
    fn first_attacher_is_granted_control() {
        let mut server = SessionServer::new_named("dev");
        let (id, _stream) = attach_client(&mut server);
        assert_eq!(server.controller, Some(id));
    }

    #[test]
    fn second_attacher_is_a_follower() {
        let mut server = SessionServer::new_named("dev");
        let (first, _s1) = attach_client(&mut server);
        let (second, _s2) = attach_client(&mut server);
        assert_eq!(server.controller, Some(first));
        assert_ne!(server.controller, Some(second));
        assert_eq!(server.attached_count(), 2);
    }

    #[test]
    fn query_registers_nothing_and_seeds_nothing() {
        let mut server = SessionServer::new_named("dev");
        let (id, _stream) = add_client(&mut server);
        let responses = server.handle_message(
            id,
            ClientMessage::Query {
                session: "dev".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        );
        assert!(matches!(
            responses.as_slice(),
            [(Target::Sender, ServerMessage::SessionInfo { .. })]
        ));
        assert_eq!(server.attached_count(), 0);
        assert!(server.client_mut(id).unwrap().outbox.is_empty());
    }

    #[test]
    fn non_controller_commit_is_ignored() {
        let mut server = SessionServer::new_named("dev");
        let (_controller, _s1) = attach_client(&mut server);
        let (follower, _s2) = attach_client(&mut server);
        let layout = SharedLayout {
            version: 1,
            canvas_cols: 80,
            canvas_rows: 24,
            workspaces: Vec::new(),
        };
        let responses = server.handle_message(
            follower,
            ClientMessage::CommitLayout {
                base_rev: 0,
                layout,
            },
        );
        assert!(responses.is_empty());
        assert_eq!(server.layout_rev, 0);
    }

    #[test]
    fn controller_commit_increments_rev_and_broadcasts_author() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let layout = SharedLayout {
            version: 1,
            canvas_cols: 80,
            canvas_rows: 24,
            workspaces: Vec::new(),
        };
        let responses = server.handle_message(
            controller,
            ClientMessage::CommitLayout {
                base_rev: 0,
                layout,
            },
        );
        assert_eq!(server.layout_rev, 1);
        let [(Target::Broadcast, ServerMessage::LayoutCommitted { rev, author, .. })] =
            responses.as_slice()
        else {
            panic!("expected broadcast commit, got {responses:?}");
        };
        assert_eq!(*rev, 1);
        assert_eq!(*author, controller);
    }

    #[test]
    fn stale_base_rev_is_rejected_with_authoritative_layout() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let layout = SharedLayout {
            version: 1,
            canvas_cols: 80,
            canvas_rows: 24,
            workspaces: Vec::new(),
        };
        server.handle_message(
            controller,
            ClientMessage::CommitLayout {
                base_rev: 0,
                layout: layout.clone(),
            },
        );
        let responses = server.handle_message(
            controller,
            ClientMessage::CommitLayout {
                base_rev: 0,
                layout,
            },
        );
        let [
            (
                Target::Sender,
                ServerMessage::LayoutRejected {
                    current_rev,
                    layout,
                },
            ),
        ] = responses.as_slice()
        else {
            panic!("expected rejection, got {responses:?}");
        };
        assert_eq!(*current_rev, 1);
        assert!(layout.is_some());
    }

    #[test]
    fn take_control_grants_and_broadcasts() {
        let mut server = SessionServer::new_named("dev");
        let (_first, _s1) = attach_client(&mut server);
        let (second, _s2) = attach_client(&mut server);
        let responses = server.handle_message(second, ClientMessage::TakeControl);
        assert_eq!(server.controller, Some(second));
        assert!(matches!(
            responses.as_slice(),
            [(
                Target::Broadcast,
                ServerMessage::ControllerChanged {
                    reason: ControllerChangeReason::Taken,
                    ..
                }
            )]
        ));
    }

    #[test]
    fn take_control_respects_cooldown() {
        let mut server = SessionServer::new_named("dev");
        let (_first, _s1) = attach_client(&mut server);
        let (second, _s2) = attach_client(&mut server);
        let (third, _s3) = attach_client(&mut server);
        server.handle_message(second, ClientMessage::TakeControl);
        let responses = server.handle_message(third, ClientMessage::TakeControl);
        assert!(
            matches!(responses.as_slice(), [(Target::Sender, ServerMessage::Error { code, .. })] if code == "takeover-cooldown")
        );
        assert_eq!(server.controller, Some(second));
    }

    #[test]
    fn removing_controller_promotes_oldest_survivor() {
        let mut server = SessionServer::new_named("dev");
        let (first, _s1) = attach_client(&mut server);
        let (second, _s2) = attach_client(&mut server);
        let (third, _s3) = attach_client(&mut server);
        assert_eq!(server.controller, Some(first));
        server.remove_client(first);
        assert_eq!(server.controller, Some(second));
        let _ = third;
    }

    #[test]
    fn spawn_from_follower_is_rejected() {
        let mut server = SessionServer::new_named("dev");
        let (_controller, _s1) = attach_client(&mut server);
        let (follower, _s2) = attach_client(&mut server);
        let responses = server.handle_message(
            follower,
            ClientMessage::SpawnPane {
                pane_id: 1,
                generation: 1,
                command: None,
                cwd: None,
                cols: 20,
                rows: 5,
                keep_open: false,
                env: Vec::new(),
                title: None,
            },
        );
        assert!(matches!(
            responses.as_slice(),
            [(Target::Sender, ServerMessage::SpawnResult { ok: false, error: Some(error), .. })]
                if error == "not controller"
        ));
        assert!(server.panes.is_empty());
    }

    #[test]
    fn read_only_and_locked_follower_input_is_denied() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let (follower, _s2) = attach_client(&mut server);
        let (viewer, _s3) = attach_read_only_client(&mut server);
        assert!(server.client_may_input(controller));
        assert!(server.client_may_input(follower));
        assert!(!server.client_may_input(viewer));

        server.handle_message(controller, ClientMessage::SetInputLock { locked: true });
        assert!(server.client_may_input(controller));
        assert!(!server.client_may_input(follower));
    }

    #[test]
    fn grant_control_validates_sender_and_target() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let (follower, _s2) = attach_client(&mut server);
        let (viewer, _s3) = attach_read_only_client(&mut server);

        assert!(
            server
                .handle_message(follower, ClientMessage::GrantControl { to: controller })
                .is_empty()
        );
        assert_eq!(server.controller, Some(controller));
        assert!(
            server
                .handle_message(controller, ClientMessage::GrantControl { to: viewer })
                .is_empty()
        );
        let responses =
            server.handle_message(controller, ClientMessage::GrantControl { to: follower });
        assert_eq!(server.controller, Some(follower));
        assert!(matches!(
            responses.as_slice(),
            [(
                Target::Broadcast,
                ServerMessage::ControllerChanged {
                    reason: ControllerChangeReason::Granted,
                    ..
                }
            )]
        ));
    }

    #[test]
    fn shutdown_requires_writable_controller() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let (follower, _s2) = attach_client(&mut server);
        let (viewer, _s3) = attach_read_only_client(&mut server);

        server.handle_message(follower, ClientMessage::Shutdown);
        assert!(!server.shutdown);
        server.handle_message(viewer, ClientMessage::Shutdown);
        assert!(!server.shutdown);
        server.handle_message(controller, ClientMessage::Shutdown);
        assert!(server.shutdown);
    }

    #[test]
    fn clients_changed_contains_roster_and_lock_state() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        let (viewer, _s2) = attach_read_only_client(&mut server);
        server.input_locked = true;
        let ServerMessage::ClientsChanged {
            clients,
            input_locked,
        } = server.clients_changed()
        else {
            panic!("expected clients changed");
        };
        assert!(input_locked);
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].id, controller);
        assert_eq!(clients[1].id, viewer);
        assert!(clients[1].read_only);
    }

    #[test]
    fn resize_updates_screen_and_broadcasts_ack() {
        let mut server = SessionServer::new_named("dev");
        let (controller, _s1) = attach_client(&mut server);
        server.panes.insert(
            1,
            ServerPane {
                generation: 2,
                title: None,
                cwd: None,
                pty: None,
                screen: TerminalScreen::new(5, 20, 100),
                cols: 20,
                rows: 5,
                exited: None,
            },
        );

        let responses = server.handle_message(
            controller,
            ClientMessage::Resize {
                pane_id: 1,
                generation: 2,
                cols: 80,
                rows: 24,
            },
        );

        assert!(matches!(
            responses.as_slice(),
            [(
                Target::Broadcast,
                ServerMessage::Resized {
                    pane_id: 1,
                    generation: 2,
                    cols: 80,
                    rows: 24,
                }
            )]
        ));
        let pane = server.panes.get_mut(&1).unwrap();
        assert_eq!((pane.cols, pane.rows), (80, 24));
        assert_eq!(pane.screen.render_snapshot().text.lines().count(), 24);
    }

    #[test]
    fn duplicate_spawn_is_rejected() {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(
            1,
            ServerPane {
                generation: 2,
                title: None,
                cwd: None,
                pty: None,
                screen: TerminalScreen::new(5, 20, 100),
                cols: 20,
                rows: 5,
                exited: None,
            },
        );
        let result = server.spawn_pane(SpawnRequest {
            pane_id: 1,
            generation: 3,
            command: None,
            cwd: None,
            title: None,
            cols: 20,
            rows: 5,
            keep_open: false,
            env: Vec::new(),
        });
        assert!(matches!(
            result,
            ServerMessage::SpawnResult { ok: false, .. }
        ));
    }

    #[test]
    fn exited_pane_can_be_respawned() {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(
            1,
            ServerPane {
                generation: 2,
                title: None,
                cwd: None,
                pty: None,
                screen: TerminalScreen::new(5, 20, 100),
                cols: 20,
                rows: 5,
                exited: Some(0),
            },
        );

        let result = server.spawn_pane(SpawnRequest {
            pane_id: 1,
            generation: 3,
            command: Some("true".into()),
            cwd: None,
            title: None,
            cols: 20,
            rows: 5,
            keep_open: false,
            env: Vec::new(),
        });

        assert!(matches!(
            result,
            ServerMessage::SpawnResult {
                pane_id: 1,
                generation: 3,
                ok: true,
                ..
            }
        ));
        assert_eq!(server.panes.get(&1).unwrap().generation, 3);
    }

    #[test]
    fn attach_reports_layout_and_panes() {
        let mut server = SessionServer::new_named("dev");
        let mut pane = ServerPane {
            generation: 8,
            title: Some("editor".into()),
            cwd: Some("/repo".into()),
            pty: None,
            screen: TerminalScreen::new(5, 20, 100),
            cols: 20,
            rows: 5,
            exited: None,
        };
        pane.screen.process_bytes(b"ready");
        server.panes.insert(4, pane);
        server.layout = Some(SharedLayout {
            version: 1,
            canvas_cols: 20,
            canvas_rows: 5,
            workspaces: Vec::new(),
        });
        server.layout_rev = 7;

        let (id, _stream) = add_client(&mut server);
        let responses = server.handle_message(
            id,
            ClientMessage::Attach {
                session: "dev".into(),
                protocol_version: PROTOCOL_VERSION,
                label: "client".into(),
                read_only: false,
            },
        );
        let Some((
            _,
            ServerMessage::Attached {
                session,
                panes,
                layout_rev,
                layout,
                controller,
                ..
            },
        )) = responses.first()
        else {
            panic!("unexpected responses: {responses:?}");
        };
        assert_eq!(session, "dev");
        assert_eq!(*layout_rev, 7);
        assert!(layout.is_some());
        assert_eq!(*controller, Some(id));
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, 4);
    }
}
