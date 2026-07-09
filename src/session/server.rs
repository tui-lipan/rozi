use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control;
use crate::session::protocol::{
    self, ClientMessage, Frame, PROTOCOL_VERSION, PaneMeta, ServerMessage, WirePalette,
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
    pub pty: Option<TerminalPty>,
    pub screen: TerminalScreen,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
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
                if let Some(outbound) = self.handle_event(event)
                    && attached
                    && write_server_frame_blocking(&mut stream, &outbound).is_err()
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
                let frame = match decoder.next_frame::<ClientMessage>() {
                    Ok(Some(frame)) => frame,
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

                let message = match frame {
                    Frame::Control(message) => message,
                    Frame::PaneBytes {
                        pane_id,
                        generation,
                        bytes,
                    } => {
                        if !attached {
                            write_frame_blocking(
                                &mut stream,
                                &ServerMessage::Error {
                                    code: "attach-required".to_string(),
                                    message: "first client message must be attach".to_string(),
                                },
                            )?;
                            return Ok(());
                        }
                        self.handle_pane_input(pane_id, generation, &bytes);
                        continue;
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
                if is_attach && attached {
                    self.write_attach_seeds(&mut stream)?;
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
                        panes: self.pane_meta(),
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
                    return vec![ServerMessage::Resized {
                        pane_id,
                        generation,
                        cols: pane.cols,
                        rows: pane.rows,
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
                pid: None,
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

    fn write_attach_seeds(&mut self, stream: &mut UnixStream) -> io::Result<()> {
        const SEED_CHUNK: usize = 256 * 1024;
        stream.set_nonblocking(false)?;
        let mut result = Ok(());
        for (pane_id, pane) in &mut self.panes {
            if pane.exited.is_some() {
                continue;
            }
            let bytes = pane.screen.export_replay_bytes();
            for chunk in bytes.chunks(SEED_CHUNK) {
                result =
                    protocol::write_pane_output_frame(stream, *pane_id, pane.generation, chunk);
                if result.is_err() {
                    break;
                }
            }
            if result.is_err() {
                break;
            }
        }
        let restore = stream.set_nonblocking(true);
        result.and(restore)
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

fn write_server_frame_blocking(
    stream: &mut UnixStream,
    outbound: &ServerOutbound,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let result = match outbound {
        ServerOutbound::Control(message) => protocol::write_frame(stream, message),
        ServerOutbound::PaneOutput {
            pane_id,
            generation,
            bytes,
        } => protocol::write_pane_output_frame(stream, *pane_id, *generation, bytes),
    };
    let restore = stream.set_nonblocking(true);
    result.and(restore)
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
        assert_eq!(panes[0].title.as_deref(), Some("editor"));
        assert_eq!(panes[0].cols, 20);
        assert_eq!(panes[0].rows, 5);
    }

    #[test]
    fn set_palette_updates_screen_without_response() {
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
        assert!(responses.is_empty());
    }

    #[test]
    fn resize_updates_screen_and_returns_ordered_ack() {
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

        let responses = server.handle_message(ClientMessage::Resize {
            pane_id: 1,
            generation: 2,
            cols: 80,
            rows: 24,
        });

        assert!(matches!(
            responses.as_slice(),
            [ServerMessage::Resized {
                pane_id: 1,
                generation: 2,
                cols: 80,
                rows: 24,
            }]
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
