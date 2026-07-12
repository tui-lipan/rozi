use super::*;

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
        // Seed the palette before the PTY spawns so the child's startup OSC 4/10/11 color queries
        // are answered against the theme palette instead of the screen default.
        screen.set_palette(request.palette.into());
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
            })
            .collect()
    }

    pub(super) fn apply_palette(&mut self, id: PaneId, generation: u64, palette: WirePalette) {
        if let Some(pane) = self.live_pane_mut(id, generation) {
            pane.screen.set_palette(palette.into());
        }
    }
}
