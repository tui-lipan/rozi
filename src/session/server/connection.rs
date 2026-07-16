use super::*;

impl SessionServer {
    pub(super) fn accept_new(&mut self, listener: &IpcListener) -> io::Result<()> {
        loop {
            match listener.accept() {
                Ok(stream) => {
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

    pub(super) fn pump_clients(&mut self) {
        let mut inbound: Vec<(ClientId, Frame<ClientMessage>)> = Vec::new();
        let mut dead: Vec<ClientId> = Vec::new();
        for client in &mut self.clients {
            for _ in 0..16 {
                match client.decoder.read_from_status(&mut client.stream) {
                    Ok(protocol::FrameReadStatus::Read(_)) => {}
                    Ok(protocol::FrameReadStatus::WouldBlock) => break,
                    Ok(protocol::FrameReadStatus::Eof) | Err(_) => {
                        dead.push(client.id);
                        break;
                    }
                }
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

    pub(super) fn process_client_frame(&mut self, id: ClientId, frame: Frame<ClientMessage>) {
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

    pub(super) fn handle_message(
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
            ClientMessage::SetSessionOrigin { profile } => {
                if self.created_from_profile.is_none()
                    && self.origin_seed_client == Some(client_id)
                    && !self.panes.is_empty()
                    && crate::session::discovery::valid_session_name(&profile)
                    && self
                        .client_mut(client_id)
                        .is_some_and(|client| !client.read_only)
                {
                    self.created_from_profile = Some(profile);
                    self.origin_seed_client = None;
                    self.dirty = true;
                    return vec![(
                        Target::Broadcast,
                        ServerMessage::SessionOriginSet {
                            created_from_profile: self
                                .created_from_profile
                                .clone()
                                .expect("origin set above"),
                        },
                    )];
                }
                Vec::new()
            }
            ClientMessage::Query {
                session,
                protocol_version,
            } => self.handle_query(session, protocol_version),
            ClientMessage::SetPaneLogging {
                pane_id,
                generation,
                enabled,
            } => {
                if self
                    .client_mut(client_id)
                    .is_some_and(|client| client.read_only)
                {
                    vec![(
                        Target::Sender,
                        ServerMessage::PaneLoggingChanged {
                            pane_id,
                            generation,
                            enabled: false,
                            path: None,
                            error: Some("read-only client".to_string()),
                        },
                    )]
                } else {
                    let message = self.set_pane_logging(pane_id, generation, enabled);
                    vec![(Target::Broadcast, message)]
                }
            }
            ClientMessage::SetPaneStatus {
                pane_id,
                generation,
                status,
                reason,
            } => match self.set_pane_status(client_id, pane_id, generation, status, reason) {
                Ok(Some(state)) => vec![(
                    Target::Broadcast,
                    ServerMessage::PaneRuntimeChanged {
                        pane_id,
                        generation,
                        state,
                    },
                )],
                Ok(None) => Vec::new(),
                Err((code, message)) => vec![(
                    Target::Sender,
                    ServerMessage::Error {
                        code: code.to_string(),
                        message,
                    },
                )],
            },
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
                palette,
                shell,
                command_shell,
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
                let initial_seed = self.created_from_profile.is_none()
                    && self.origin_seed_client.is_none()
                    && self.panes.is_empty()
                    && self.layout.is_none();
                let message = self.spawn_pane(SpawnRequest {
                    pane_id,
                    generation,
                    command,
                    cwd,
                    title,
                    cols,
                    rows,
                    keep_open,
                    env,
                    palette,
                    shell,
                    command_shell,
                });
                if initial_seed && matches!(message, ServerMessage::SpawnResult { ok: true, .. }) {
                    self.origin_seed_client = Some(client_id);
                }
                vec![(Target::Sender, message)]
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
                let responses = self.handle_commit_layout(client_id, base_rev, layout);
                if self.created_from_profile.is_none()
                    && responses.iter().any(|(_, message)| {
                        matches!(message, ServerMessage::LayoutCommitted { .. })
                    })
                {
                    self.origin_seed_client = None;
                }
                responses
            }
            ClientMessage::RequestControl => self.handle_request_control(client_id),
            ClientMessage::GrantControl { to } => self.handle_grant_control(client_id, to),
            ClientMessage::DeclineControl { to } => self.handle_decline_control(client_id, to),
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
                self.forget_snapshot = true;
                for pane in self.panes.values() {
                    if let Some(pty) = &pane.pty {
                        let _ = pty.kill();
                    }
                }
                Vec::new()
            }
        }
    }

    pub(super) fn handle_attach(
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
            created_from_profile: self.created_from_profile.clone(),
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

    pub(super) fn handle_query(
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
                created_from_profile: self.created_from_profile.clone(),
            },
        )]
    }
}
