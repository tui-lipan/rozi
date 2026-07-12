use super::*;
use std::fs::OpenOptions;

impl SessionServer {
    /// Rename this session in place: move the listening socket to the new name so the same server
    /// (and its live panes) becomes discoverable under `name` with zero pane movement. Rejects
    /// invalid names and collisions with an already-running session.
    pub(super) fn rename_session(&mut self, name: String) -> ServerMessage {
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

    pub(super) fn spawn_pane(&mut self, request: SpawnRequest) -> ServerMessage {
        self.spawn_pane_inner(request, None, false)
    }

    pub(super) fn spawn_pane_inner(
        &mut self,
        request: SpawnRequest,
        seed: Option<&[u8]>,
        retain_on_failure: bool,
    ) -> ServerMessage {
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
        // A replaced pane's log handle dies with it; tell every client so no stale
        // `[log]` badge survives a respawn.
        if let Some(old) = self.panes.remove(&id)
            && old.log.is_some()
        {
            self.broadcast_outbound(&ServerOutbound::Control(
                ServerMessage::PaneLoggingChanged {
                    pane_id: id,
                    generation: old.generation,
                    enabled: false,
                    path: None,
                    error: None,
                },
            ));
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
        // Seed the palette before the PTY spawns so the child's startup OSC 4/10/11 color queries
        // are answered against the theme palette instead of the screen default.
        screen.set_palette(request.palette.into());
        if let Some(seed) = seed {
            screen.process_bytes(seed);
            screen.drain_responses();
        }
        let mut config = pty_config(
            request.command.as_deref(),
            request.keep_open,
            &request.shell,
            &request.command_shell,
        );
        if let Some(cwd) = request.cwd.as_ref().filter(|cwd| Path::new(cwd).is_dir()) {
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
                        command: request.command,
                        keep_open: request.keep_open,
                        palette: request.palette,
                        pty: Some(pty),
                        screen,
                        cols: cols.max(1),
                        rows: rows.max(1),
                        exited: None,
                        log: None,
                    },
                );
                self.dirty = true;
                ServerMessage::SpawnResult {
                    pane_id: id,
                    generation,
                    pid,
                    ok: true,
                    error: None,
                }
            }
            Err(err) => {
                if retain_on_failure {
                    self.panes.insert(
                        id,
                        ServerPane {
                            generation,
                            title: request.title,
                            cwd: request.cwd,
                            command: request.command,
                            keep_open: request.keep_open,
                            palette: request.palette,
                            pty: None,
                            screen,
                            cols: cols.max(1),
                            rows: rows.max(1),
                            exited: Some(127),
                            log: None,
                        },
                    );
                }
                ServerMessage::SpawnResult {
                    pane_id: id,
                    generation,
                    pid: None,
                    ok: false,
                    error: Some(err.to_string()),
                }
            }
        }
    }

    pub(super) fn handle_pane_input(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        if let Some(pane) = self.live_pane_mut(pane_id, generation)
            && let Some(pty) = &pane.pty
        {
            let _ = pty.write(bytes);
        }
    }

    pub(super) fn handle_event(&mut self, event: ServerEvent) -> Option<ServerOutbound> {
        match event {
            ServerEvent::Pty(id, generation, event) => {
                let pane = self.panes.get_mut(&id)?;
                if pane.generation != generation {
                    return None;
                }
                match event {
                    TerminalPtyEvent::Output(bytes) => {
                        let log_error = pane
                            .log
                            .as_mut()
                            .and_then(|log| log.file.write_all(&bytes).err());
                        let logging_error = log_error.map(|error| {
                            pane.log = None;
                            format!("pane log write failed: {error}")
                        });
                        pane.screen.process_bytes(&bytes);
                        self.dirty = true;
                        if let Some(pty) = &pane.pty {
                            for response in pane.screen.drain_responses() {
                                let _ = pty.write(&response);
                            }
                        }
                        let output = ServerOutbound::PaneOutput {
                            pane_id: id,
                            generation,
                            bytes: bytes.to_vec(),
                        };
                        if let Some(error) = logging_error {
                            self.broadcast_outbound(&ServerOutbound::Control(
                                ServerMessage::PaneLoggingChanged {
                                    pane_id: id,
                                    generation,
                                    enabled: false,
                                    path: None,
                                    error: Some(error),
                                },
                            ));
                        }
                        Some(output)
                    }
                    TerminalPtyEvent::Exited(code) => {
                        pane.exited = Some(code);
                        pane.pty = None;
                        self.dirty = true;
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

    pub(super) fn live_pane_mut(&mut self, id: PaneId, generation: u64) -> Option<&mut ServerPane> {
        self.panes
            .get_mut(&id)
            .filter(|pane| pane.generation == generation && pane.exited.is_none())
    }

    pub(super) fn pane_meta(&self) -> Vec<PaneMeta> {
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
                logging: pane.log.is_some(),
            })
            .collect()
    }

    pub(super) fn apply_palette(&mut self, id: PaneId, generation: u64, palette: WirePalette) {
        if let Some(pane) = self.live_pane_mut(id, generation) {
            pane.screen.set_palette(palette.into());
        }
    }

    pub(super) fn set_pane_logging(
        &mut self,
        id: PaneId,
        generation: u64,
        enabled: bool,
    ) -> ServerMessage {
        let Some(pane) = self
            .panes
            .get_mut(&id)
            .filter(|pane| pane.generation == generation)
        else {
            return ServerMessage::PaneLoggingChanged {
                pane_id: id,
                generation,
                enabled: false,
                path: None,
                error: Some("pane not found".to_string()),
            };
        };
        if !enabled {
            pane.log = None;
            return ServerMessage::PaneLoggingChanged {
                pane_id: id,
                generation,
                enabled: false,
                path: None,
                error: None,
            };
        }
        let root = self.settings.log_dir.clone().or_else(default_log_dir);
        let result = root
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "state directory unavailable"))
            .and_then(|root| {
                fs::create_dir_all(&root)?;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
                let dir = root.join(&self.session_name);
                fs::create_dir_all(&dir)?;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
                let path = dir.join(format!("{id}-{generation}.log"));
                let file = OpenOptions::new().create(true).append(true).open(&path)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                Ok(PaneLog { file, path })
            });
        match result {
            Ok(log) => {
                let path = log.path.to_string_lossy().into_owned();
                pane.log = Some(log);
                ServerMessage::PaneLoggingChanged {
                    pane_id: id,
                    generation,
                    enabled: true,
                    path: Some(path),
                    error: None,
                }
            }
            Err(error) => ServerMessage::PaneLoggingChanged {
                pane_id: id,
                generation,
                enabled: false,
                path: None,
                error: Some(error.to_string()),
            },
        }
    }
}

fn default_log_dir() -> Option<PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    if env.home.is_none() && env.xdg_state_home.is_none() {
        return None;
    }
    Some(crate::platform::paths::state_dir(&env).join("logs"))
}
