use std::collections::HashMap;
use std::fs;
use std::io;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control;
use crate::session::protocol::{
    self, AttachedPane, ClientMessage, PROTOCOL_VERSION, ServerMessage, WirePalette,
    WireSearchMatch, WireSnapshot,
};
use crate::state::PaneId;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;
const DEFAULT_SCROLLBACK: usize = 5000;
const EXITED_NO_CLIENT_GRACE: Duration = Duration::from_secs(30);

pub struct SessionServer {
    panes: HashMap<PaneId, ServerPane>,
    next_generation: u64,
    layout_blob: Option<String>,
    event_rx: mpsc::Receiver<ServerEvent>,
    event_tx: mpsc::Sender<ServerEvent>,
    shutdown: bool,
    session_name: String,
}

pub struct ServerPane {
    pub generation: u64,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub pty: Option<ServerPty>,
    pub screen: TerminalScreen,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
}

pub enum ServerPty {
    Managed(TerminalPty),
    #[cfg(unix)]
    Adopted(AdoptedPty),
}

#[cfg(unix)]
pub struct AdoptedPty {
    fd: OwnedFd,
    writer: Mutex<std::fs::File>,
    active: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    pid: Option<u32>,
}

#[cfg(unix)]
impl AdoptedPty {
    fn new(
        fd: OwnedFd,
        pid: Option<u32>,
        on_event: impl Fn(TerminalPtyEvent) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let reader_fd = dup_raw_fd(fd.as_raw_fd())?;
        let writer_fd = dup_raw_fd(fd.as_raw_fd())?;
        let active = Arc::new(AtomicBool::new(true));
        let exited = Arc::new(AtomicBool::new(false));
        let thread_active = active.clone();
        let thread_exited = exited.clone();
        let on_event = Arc::new(on_event);
        std::thread::spawn(move || {
            let mut reader = std::fs::File::from(reader_fd);
            let mut buffer = [0_u8; 8192];
            while thread_active.load(Ordering::Acquire) {
                if !wait_readable(reader.as_raw_fd(), &thread_active) {
                    break;
                }
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        thread_exited.store(true, Ordering::Release);
                        on_event(TerminalPtyEvent::Exited(-1));
                        break;
                    }
                    Ok(read) => {
                        if thread_active.load(Ordering::Acquire) {
                            on_event(TerminalPtyEvent::Output(buffer[..read].to_vec().into()));
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) if is_pty_eof_error(&err) => {
                        thread_exited.store(true, Ordering::Release);
                        on_event(TerminalPtyEvent::Exited(-1));
                        break;
                    }
                    Err(err) => {
                        on_event(TerminalPtyEvent::Error(err.to_string().into()));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            fd,
            writer: Mutex::new(std::fs::File::from(writer_fd)),
            active,
            exited,
            pid,
        })
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("adopted pty writer lock poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = libc::winsize {
            ws_row: rows.max(1),
            ws_col: cols.max(1),
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn kill(&self) -> io::Result<()> {
        self.active.store(false, Ordering::Release);
        if self.exited.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(pid) = self.pid {
            let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };
            if rc != 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for AdoptedPty {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

impl ServerPty {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Managed(pty) => pty.write(bytes),
            #[cfg(unix)]
            Self::Adopted(pty) => pty.write(bytes),
        }
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        match self {
            Self::Managed(pty) => pty.resize(cols, rows),
            #[cfg(unix)]
            Self::Adopted(pty) => pty.resize(cols, rows),
        }
    }

    fn kill(&self) -> io::Result<()> {
        match self {
            Self::Managed(pty) => pty.kill(),
            #[cfg(unix)]
            Self::Adopted(pty) => pty.kill(),
        }
    }

    fn pid(&self) -> Option<u32> {
        match self {
            Self::Managed(pty) => pty.pid(),
            #[cfg(unix)]
            Self::Adopted(pty) => pty.pid,
        }
    }
}

enum ServerEvent {
    Pty(PaneId, u64, TerminalPtyEvent),
}

impl SessionServer {
    pub fn new_named(session_name: impl Into<String>) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            panes: HashMap::new(),
            next_generation: 1,
            layout_blob: None,
            event_rx,
            event_tx,
            shutdown: false,
            session_name: session_name.into(),
        }
    }

    pub fn run_listener(&mut self, listener: UnixListener) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        let mut exited_idle_since: Option<Instant> = None;
        while !self.shutdown {
            while let Ok(event) = self.event_rx.try_recv() {
                let _ = self.handle_event(event);
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    exited_idle_since = None;
                    let _ = self.handle_client(stream);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if self.all_panes_exited() {
                        let since = exited_idle_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= EXITED_NO_CLIENT_GRACE {
                            self.shutdown = true;
                        }
                    } else {
                        exited_idle_since = None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    fn handle_client(&mut self, mut stream: UnixStream) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut attached = false;
        let mut decoder = protocol::FrameDecoder::default();
        loop {
            while let Ok(event) = self.event_rx.try_recv() {
                if let Some(message) = self.handle_event(event)
                    && attached
                    && write_frame_blocking(&mut stream, &message).is_err()
                {
                    return Ok(());
                }
            }

            match decoder.read_from_status(&mut stream) {
                Ok(protocol::FrameReadStatus::Eof) => return Ok(()),
                Ok(protocol::FrameReadStatus::Read(_) | protocol::FrameReadStatus::WouldBlock) => {}
                Err(err) => {
                    let _ = write_frame_blocking(
                        &mut stream,
                        &ServerMessage::Error {
                            code: "protocol-error".to_string(),
                            message: err.to_string(),
                        },
                    );
                    return Ok(());
                }
            }

            loop {
                let message = match decoder.next::<ClientMessage>() {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(err) => {
                        let _ = write_frame_blocking(
                            &mut stream,
                            &ServerMessage::Error {
                                code: "protocol-error".to_string(),
                                message: err.to_string(),
                            },
                        );
                        return Ok(());
                    }
                };

                if let Some(error) = attach_required_error(attached, &message) {
                    write_frame_blocking(&mut stream, &error)?;
                    return Ok(());
                }
                let detach = matches!(message, ClientMessage::Detach);
                let is_attach = matches!(message, ClientMessage::Attach { .. });
                let responses = self.handle_message(message);
                if is_attach {
                    attached = responses
                        .iter()
                        .any(|response| matches!(response, ServerMessage::Attached { .. }));
                }
                for response in responses {
                    write_frame_blocking(&mut stream, &response)?;
                }
                if detach || self.shutdown || (is_attach && !attached) {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn handle_message(&mut self, message: ClientMessage) -> Vec<ServerMessage> {
        match message {
            ClientMessage::Attach {
                session,
                protocol_version,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    vec![ServerMessage::Error {
                        code: "protocol-mismatch".to_string(),
                        message: format!(
                            "client protocol {protocol_version} is incompatible with server protocol {PROTOCOL_VERSION}"
                        ),
                    }]
                } else if session != self.session_name {
                    vec![ServerMessage::Error {
                        code: "session-mismatch".to_string(),
                        message: format!(
                            "client requested session {session:?}, but this server owns {:?}",
                            self.session_name
                        ),
                    }]
                } else {
                    vec![ServerMessage::Attached {
                        protocol_version: PROTOCOL_VERSION,
                        session,
                        panes: self.snapshots(),
                        layout_blob: self.layout_blob.clone(),
                    }]
                }
            }
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
                vec![self.spawn_pane(SpawnRequest {
                    pane_id,
                    generation,
                    command,
                    cwd,
                    title,
                    cols,
                    rows,
                    keep_open,
                    env,
                })]
            }
            ClientMessage::AdoptPane {
                pane_id,
                generation,
                cols,
                rows,
                pid,
                title,
                cwd,
                snapshot,
                socket_path,
            } => self.adopt_pane(AdoptRequest {
                pane_id,
                generation,
                cols,
                rows,
                pid,
                title,
                cwd,
                snapshot,
                socket_path,
            }),
            ClientMessage::Input {
                pane_id,
                generation,
                bytes,
            } => {
                if let Some(pane) = self.live_pane_mut(pane_id, generation)
                    && let Some(pty) = &pane.pty
                {
                    let _ = pty.write(&bytes);
                }
                Vec::new()
            }
            ClientMessage::Resize {
                pane_id,
                generation,
                cols,
                rows,
            } => {
                if let Some(pane) = self.live_pane_mut(pane_id, generation) {
                    pane.cols = cols.max(1);
                    pane.rows = rows.max(1);
                    pane.screen.resize(pane.rows, pane.cols);
                    if let Some(pty) = &pane.pty {
                        let _ = pty.resize(pane.cols, pane.rows);
                    }
                    return vec![ServerMessage::Snapshot {
                        pane_id,
                        generation,
                        snapshot: pane.snapshot(),
                    }];
                }
                Vec::new()
            }
            ClientMessage::Scroll {
                pane_id,
                generation,
                offset,
            } => {
                if let Some(pane) = self.live_pane_mut(pane_id, generation) {
                    pane.screen.set_scrollback(offset);
                    return vec![ServerMessage::Snapshot {
                        pane_id,
                        generation,
                        snapshot: pane.snapshot(),
                    }];
                }
                Vec::new()
            }
            ClientMessage::Kill {
                pane_id,
                generation,
            } => {
                if let Some(pane) = self.live_pane_mut(pane_id, generation)
                    && let Some(pty) = &pane.pty
                {
                    let _ = pty.kill();
                }
                Vec::new()
            }
            ClientMessage::Search {
                request_id,
                pane_id,
                generation,
                query,
            } => {
                let matches = self
                    .panes
                    .get_mut(&pane_id)
                    .filter(|pane| pane.generation == generation)
                    .map(|pane| search_visible(pane, &query))
                    .unwrap_or_default();
                vec![ServerMessage::SearchResult {
                    request_id,
                    pane_id,
                    generation,
                    query,
                    matches,
                }]
            }
            ClientMessage::SetPalette {
                pane_id,
                generation,
                palette,
            } => self.apply_palette(pane_id, generation, palette),
            ClientMessage::ConfigurePane {
                pane_id,
                generation,
                palette,
                title,
                cwd,
            } => {
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
                    return vec![ServerMessage::Snapshot {
                        pane_id,
                        generation,
                        snapshot: pane.snapshot(),
                    }];
                }
                Vec::new()
            }
            ClientMessage::PushLayout { blob } => {
                self.layout_blob = Some(blob);
                Vec::new()
            }
            ClientMessage::Detach => Vec::new(),
            ClientMessage::Shutdown => {
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

    fn spawn_pane(&mut self, request: SpawnRequest) -> ServerMessage {
        let id = request.pane_id;
        if self.panes.contains_key(&id) {
            return ServerMessage::SpawnResult {
                pane_id: id,
                generation: request.generation,
                ok: false,
                error: Some(format!("pane {id} already exists")),
            };
        }
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
                let _ = pty.resize(cols.max(1), rows.max(1));
                screen.resize(rows.max(1), cols.max(1));
                self.panes.insert(
                    id,
                    ServerPane {
                        generation,
                        title: request.title,
                        cwd: request.cwd,
                        pty: Some(ServerPty::Managed(pty)),
                        screen,
                        cols: cols.max(1),
                        rows: rows.max(1),
                        exited: None,
                    },
                );
                ServerMessage::SpawnResult {
                    pane_id: id,
                    generation,
                    ok: true,
                    error: None,
                }
            }
            Err(err) => ServerMessage::SpawnResult {
                pane_id: id,
                generation,
                ok: false,
                error: Some(err.to_string()),
            },
        }
    }

    #[cfg(unix)]
    fn adopt_pane(&mut self, request: AdoptRequest) -> Vec<ServerMessage> {
        let id = request.pane_id;
        if self.panes.contains_key(&id) {
            return vec![ServerMessage::SpawnResult {
                pane_id: id,
                generation: request.generation,
                ok: false,
                error: Some(format!("pane {id} already exists")),
            }];
        }
        if let Err(err) = validate_adopt_socket_path(&request.socket_path) {
            return vec![ServerMessage::SpawnResult {
                pane_id: id,
                generation: request.generation,
                ok: false,
                error: Some(format!("invalid pty adoption socket: {err}")),
            }];
        }
        let stream = match connect_adopt_socket(&request.socket_path) {
            Ok(stream) => stream,
            Err(err) => {
                return vec![ServerMessage::SpawnResult {
                    pane_id: id,
                    generation: request.generation,
                    ok: false,
                    error: Some(format!("pty adoption socket failed: {err}")),
                }];
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let fd = match crate::session::fdpass::recv_fd(&stream) {
            Ok(fd) => fd,
            Err(err) => {
                return vec![ServerMessage::SpawnResult {
                    pane_id: id,
                    generation: request.generation,
                    ok: false,
                    error: Some(format!("pty fd receive failed: {err}")),
                }];
            }
        };
        let generation = request.generation;
        self.next_generation = self.next_generation.max(generation.saturating_add(1));
        let tx = self.event_tx.clone();
        let pty = match AdoptedPty::new(fd, request.pid, move |event| {
            let _ = tx.send(ServerEvent::Pty(id, generation, event));
        }) {
            Ok(pty) => pty,
            Err(err) => {
                return vec![ServerMessage::SpawnResult {
                    pane_id: id,
                    generation,
                    ok: false,
                    error: Some(format!("pty adoption failed: {err}")),
                }];
            }
        };
        let cols = request.cols.max(1);
        let rows = request.rows.max(1);
        let _ = pty.resize(cols, rows);
        let screen = seed_screen_from_snapshot(rows, cols, request.snapshot.clone());
        let snapshot = request.snapshot;
        self.panes.insert(
            id,
            ServerPane {
                generation,
                title: request.title,
                cwd: request.cwd,
                pty: Some(ServerPty::Adopted(pty)),
                screen,
                cols,
                rows,
                exited: None,
            },
        );
        vec![
            ServerMessage::SpawnResult {
                pane_id: id,
                generation,
                ok: true,
                error: None,
            },
            ServerMessage::Snapshot {
                pane_id: id,
                generation,
                snapshot,
            },
        ]
    }

    #[cfg(not(unix))]
    fn adopt_pane(&mut self, request: AdoptRequest) -> Vec<ServerMessage> {
        vec![ServerMessage::SpawnResult {
            pane_id: request.pane_id,
            generation: request.generation,
            ok: false,
            error: Some("pty adoption is only supported on Unix".to_string()),
        }]
    }

    fn handle_event(&mut self, event: ServerEvent) -> Option<ServerMessage> {
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
                        Some(ServerMessage::Snapshot {
                            pane_id: id,
                            generation,
                            snapshot: pane.snapshot(),
                        })
                    }
                    TerminalPtyEvent::Exited(code) => {
                        pane.exited = Some(code);
                        pane.pty = None;
                        Some(ServerMessage::Exited {
                            pane_id: id,
                            generation,
                            code,
                        })
                    }
                    TerminalPtyEvent::Error(message) => Some(ServerMessage::SpawnResult {
                        pane_id: id,
                        generation,
                        ok: false,
                        error: Some(message.to_string()),
                    }),
                }
            }
        }
    }

    fn live_pane_mut(&mut self, id: PaneId, generation: u64) -> Option<&mut ServerPane> {
        self.panes
            .get_mut(&id)
            .filter(|pane| pane.generation == generation && pane.exited.is_none())
    }

    fn snapshots(&mut self) -> Vec<AttachedPane> {
        self.panes
            .iter_mut()
            .map(|(pane_id, pane)| AttachedPane {
                pane_id: *pane_id,
                generation: pane.generation,
                snapshot: pane.snapshot(),
                exited: pane.exited,
            })
            .collect()
    }

    fn all_panes_exited(&self) -> bool {
        !self.panes.is_empty() && self.panes.values().all(|pane| pane.exited.is_some())
    }

    fn apply_palette(
        &mut self,
        id: PaneId,
        generation: u64,
        palette: WirePalette,
    ) -> Vec<ServerMessage> {
        if let Some(pane) = self.live_pane_mut(id, generation) {
            pane.screen.set_palette(palette.into());
            return vec![ServerMessage::Snapshot {
                pane_id: id,
                generation,
                snapshot: pane.snapshot(),
            }];
        }
        Vec::new()
    }
}

fn attach_required_error(attached: bool, message: &ClientMessage) -> Option<ServerMessage> {
    (!attached && !matches!(message, ClientMessage::Attach { .. })).then(|| ServerMessage::Error {
        code: "attach-required".to_string(),
        message: "first client message must be attach".to_string(),
    })
}

fn write_frame_blocking(stream: &mut UnixStream, message: &ServerMessage) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let result = protocol::write_frame(stream, message);
    let restore = stream.set_nonblocking(true);
    result.and(restore)
}

#[cfg(unix)]
fn connect_adopt_socket(path: &str) -> io::Result<UnixStream> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(unix)]
fn validate_adopt_socket_path(path: &str) -> io::Result<()> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must be absolute",
        ));
    }
    let runtime_dir = control::runtime_dir()?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    if parent != runtime_dir {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is outside hyprmux runtime directory",
        ));
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| !name.starts_with("adopt-") || !name.ends_with(".sock"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected adoption socket name",
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "adoption socket must not be a symlink",
        ));
    }
    Ok(())
}

fn seed_screen_from_snapshot(rows: u16, cols: u16, snapshot: WireSnapshot) -> TerminalScreen {
    let mut screen = TerminalScreen::new(rows, cols, DEFAULT_SCROLLBACK);
    for (index, line) in snapshot.text.lines().enumerate() {
        if index > 0 {
            screen.process_bytes(b"\r\n");
        }
        screen.process_bytes(line.as_bytes());
    }
    screen.set_scrollback(snapshot.scrollback_offset);
    screen
}

#[cfg(unix)]
fn dup_raw_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(dup) })
}

#[cfg(unix)]
fn wait_readable(fd: RawFd, active: &AtomicBool) -> bool {
    while active.load(Ordering::Acquire) {
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pollfd, 1, 100) };
        if rc > 0 {
            return pollfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
        }
        if rc == 0 {
            continue;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
    false
}

#[cfg(unix)]
fn is_pty_eof_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EIO)
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

struct AdoptRequest {
    pane_id: PaneId,
    generation: u64,
    cols: u16,
    rows: u16,
    pid: Option<u32>,
    title: Option<String>,
    cwd: Option<String>,
    snapshot: WireSnapshot,
    socket_path: String,
}

impl ServerPane {
    fn snapshot(&mut self) -> WireSnapshot {
        let snapshot = self.screen.render_snapshot();
        WireSnapshot::from_snapshot(self.effective_title(), self.effective_cwd(), &snapshot)
    }

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

fn search_visible(pane: &mut ServerPane, query: &str) -> Vec<WireSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let original = pane.screen.scrollback_offset();
    let max_offset = pane.screen.total_scrollback_rows();
    let step = usize::from(pane.rows.max(1));
    let mut matches = Vec::new();
    let mut seen_matches = std::collections::HashMap::new();
    let mut offset = max_offset;
    loop {
        pane.screen.set_scrollback(offset);
        let snapshot = pane.screen.render_snapshot();
        for (line, text) in snapshot.text.lines().enumerate() {
            let logical_line = line as isize - offset as isize;
            for (start_col, end_col) in search_match_ranges(text, query) {
                let matched = WireSearchMatch {
                    offset,
                    line,
                    start_col,
                    end_col,
                    text: text.to_string(),
                };
                let key = (logical_line, start_col, end_col);
                if let Some(index) = seen_matches.get(&key).copied() {
                    matches[index] = matched;
                } else {
                    seen_matches.insert(key, matches.len());
                    matches.push(matched);
                }
            }
        }
        if offset == 0 {
            break;
        }
        offset = offset.saturating_sub(step);
    }
    pane.screen.set_scrollback(original);
    matches
}

fn search_match_ranges(line: &str, query: &str) -> Vec<(usize, usize)> {
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let haystack = line.to_ascii_lowercase();
    let mut ranges = Vec::new();
    let mut search_from = 0usize;
    while search_from < haystack.len() {
        let Some(relative_start) = haystack[search_from..].find(&needle) else {
            break;
        };
        let start = search_from + relative_start;
        let end = start + needle.len();
        let start_col = haystack[..start].chars().count();
        let end_col = haystack[..end].chars().count();
        if start_col < end_col {
            ranges.push((start_col, end_col));
        }
        search_from = end;
    }
    ranges
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
    let result = SessionServer::new_named(name).run_listener(listener);
    let _ = fs::remove_file(path);
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
    use std::io::Write;

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

    #[cfg(unix)]
    #[test]
    fn linux_pty_eio_maps_to_eof() {
        let err = io::Error::from_raw_os_error(libc::EIO);
        assert!(is_pty_eof_error(&err));
    }

    #[test]
    fn search_visible_returns_matching_lines() {
        let mut pane = ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            pty: None,
            screen: TerminalScreen::new(5, 20, 100),
            cols: 20,
            rows: 5,
            exited: None,
        };
        pane.screen.process_bytes(b"alpha\r\nbeta\r\nalphabet");
        let matches = search_visible(&mut pane, "alpha");
        assert!(matches.iter().any(|item| item.text.contains("alpha")));
        assert!(
            matches
                .iter()
                .any(|item| item.start_col == 0 && item.end_col == 5)
        );
    }

    #[test]
    fn search_is_case_insensitive_dedupes_and_restores_offset() {
        let mut pane = ServerPane {
            generation: 2,
            title: None,
            cwd: None,
            pty: None,
            screen: TerminalScreen::new(2, 20, 100),
            cols: 20,
            rows: 2,
            exited: None,
        };
        pane.screen.process_bytes(b"Alpha\r\nfiller\r\nalpha\r\n");
        pane.screen.set_scrollback(1);
        let original = pane.screen.scrollback_offset();

        let matches = search_visible(&mut pane, "ALPHA");

        assert_eq!(pane.screen.scrollback_offset(), original);
        assert!(matches.len() >= 2, "matches: {matches:?}");
        let unique: std::collections::HashSet<_> = matches
            .iter()
            .map(|m| (m.offset as isize - m.line as isize, m.start_col, m.end_col))
            .collect();
        assert_eq!(
            unique.len(),
            matches.len(),
            "duplicate logical matches: {matches:?}"
        );
    }

    #[test]
    fn attach_reports_protocol_mismatch() {
        let mut server = SessionServer::new_named("dev");
        let responses = server.handle_message(ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION + 1,
        });
        assert!(
            matches!(responses.as_slice(), [ServerMessage::Error { code, .. }] if code == "protocol-mismatch")
        );
    }

    #[test]
    fn mismatched_attach_closes_connection_without_authorizing_later_messages() {
        let (mut client, server_stream) = UnixStream::pair().unwrap();
        protocol::write_frame(
            &mut client,
            &ClientMessage::Attach {
                session: "dev".into(),
                protocol_version: PROTOCOL_VERSION + 1,
            },
        )
        .unwrap();
        protocol::write_frame(
            &mut client,
            &ClientMessage::PushLayout { blob: "bad".into() },
        )
        .unwrap();
        let mut server = SessionServer::new_named("dev");

        server.handle_client(server_stream).unwrap();
        let response: ServerMessage = protocol::read_frame(&mut client).unwrap();

        assert!(
            matches!(response, ServerMessage::Error { code, .. } if code == "protocol-mismatch")
        );
        assert_eq!(server.layout_blob, None);
    }

    #[test]
    fn session_mismatch_closes_connection_without_authorizing_later_messages() {
        let (mut client, server_stream) = UnixStream::pair().unwrap();
        protocol::write_frame(
            &mut client,
            &ClientMessage::Attach {
                session: "other".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        protocol::write_frame(
            &mut client,
            &ClientMessage::PushLayout { blob: "bad".into() },
        )
        .unwrap();
        let mut server = SessionServer::new_named("dev");

        server.handle_client(server_stream).unwrap();
        let response: ServerMessage = protocol::read_frame(&mut client).unwrap();

        assert!(
            matches!(response, ServerMessage::Error { code, .. } if code == "session-mismatch")
        );
        assert_eq!(server.session_name, "dev");
        assert_eq!(server.layout_blob, None);
    }

    #[test]
    fn pre_attach_events_are_not_sent_to_wrong_session_client() {
        let (mut client, server_stream) = UnixStream::pair().unwrap();
        protocol::write_frame(
            &mut client,
            &ClientMessage::Attach {
                session: "other".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(
            7,
            ServerPane {
                generation: 3,
                title: Some("secret".into()),
                cwd: None,
                pty: None,
                screen: TerminalScreen::new(2, 20, 10),
                cols: 20,
                rows: 2,
                exited: None,
            },
        );
        server
            .event_tx
            .send(ServerEvent::Pty(
                7,
                3,
                TerminalPtyEvent::Output(b"secret snapshot".to_vec().into()),
            ))
            .unwrap();

        server.handle_client(server_stream).unwrap();
        let response: ServerMessage = protocol::read_frame(&mut client).unwrap();

        assert!(
            matches!(&response, ServerMessage::Error { code, .. } if code == "session-mismatch"),
            "wrong-session client received pre-auth data: {response:?}"
        );
        let text = server
            .panes
            .get_mut(&7)
            .unwrap()
            .screen
            .render_snapshot()
            .text
            .to_string();
        assert!(text.contains("secret snapshot"));
    }

    #[test]
    fn malformed_frame_does_not_destroy_server_state() {
        let (mut client, server_stream) = UnixStream::pair().unwrap();
        client
            .write_all(&((protocol::MAX_FRAME_SIZE as u32) + 1).to_be_bytes())
            .unwrap();
        let mut server = SessionServer::new_named("dev");
        server.layout_blob = Some("keep".into());

        server.handle_client(server_stream).unwrap();

        assert_eq!(server.layout_blob.as_deref(), Some("keep"));
    }

    #[test]
    fn client_connect_then_drop_returns_from_handle_client() {
        let (client, server_stream) = UnixStream::pair().unwrap();
        drop(client);
        let mut server = SessionServer::new_named("dev");
        server.handle_client(server_stream).unwrap();
    }

    #[test]
    fn client_partial_frame_then_drop_returns_from_handle_client() {
        let (mut client, server_stream) = UnixStream::pair().unwrap();
        client.write_all(&10_u32.to_be_bytes()).unwrap();
        client.write_all(b"abc").unwrap();
        drop(client);
        let mut server = SessionServer::new_named("dev");
        server.handle_client(server_stream).unwrap();
    }

    #[test]
    fn attach_after_spawned_state_and_layout_persists() {
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
        server.handle_message(ClientMessage::PushLayout {
            blob: "layout-v1".into(),
        });

        let responses = server.handle_message(ClientMessage::Attach {
            session: "dev".into(),
            protocol_version: PROTOCOL_VERSION,
        });

        let [
            ServerMessage::Attached {
                session,
                panes,
                layout_blob,
                ..
            },
        ] = responses.as_slice()
        else {
            panic!("unexpected responses: {responses:?}");
        };
        assert_eq!(session, "dev");
        assert_eq!(layout_blob.as_deref(), Some("layout-v1"));
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, 4);
        assert_eq!(panes[0].generation, 8);
        assert_eq!(panes[0].snapshot.title.as_deref(), Some("editor"));
    }

    #[test]
    fn set_palette_updates_screen_and_returns_snapshot() {
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
        let palette = WirePalette::from(TerminalColorPalette::default());
        let responses = server.handle_message(ClientMessage::SetPalette {
            pane_id: 1,
            generation: 2,
            palette,
        });
        assert!(matches!(
            responses.as_slice(),
            [ServerMessage::Snapshot { .. }]
        ));
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
    fn stale_generation_palette_is_ignored() {
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
        let responses = server.handle_message(ClientMessage::SetPalette {
            pane_id: 1,
            generation: 99,
            palette: WirePalette::from(TerminalColorPalette::default()),
        });
        assert!(responses.is_empty());
    }

    #[test]
    fn non_attach_first_client_message_gets_error() {
        let response =
            attach_required_error(false, &ClientMessage::PushLayout { blob: "x".into() })
                .expect("non-attach rejected");
        assert!(matches!(response, ServerMessage::Error { code, .. } if code == "attach-required"));
    }
}
