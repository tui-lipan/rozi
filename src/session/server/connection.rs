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
                local,
                generation,
                bytes,
            } => {
                let owner = local.then_some(id);
                let may_operate = if local {
                    self.client_attached(id) && !self.client_read_only(id)
                } else {
                    self.client_may_input(id)
                };
                if may_operate {
                    self.handle_pane_input(owner, pane_id, generation, &bytes);
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
                min_protocol_version,
                label,
                read_only,
            } => self.handle_attach(
                client_id,
                session,
                protocol_version,
                min_protocol_version,
                label,
                read_only,
            ),
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
                    self.mark_dirty();
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
                min_protocol_version,
            } => self.handle_query(session, protocol_version, min_protocol_version),
            ClientMessage::SetPaneLogging {
                pane_id,
                local,
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
                            local,
                            generation,
                            enabled: false,
                            path: None,
                            error: Some("read-only client".to_string()),
                        },
                    )]
                } else {
                    let owner = local.then_some(client_id);
                    let message = self.set_pane_logging(owner, pane_id, generation, enabled);
                    vec![(owner.map_or(Target::Broadcast, Target::Client), message)]
                }
            }
            ClientMessage::SetPaneStatus {
                pane_id,
                local,
                generation,
                status,
                reason,
            } => {
                match self.set_pane_status(client_id, pane_id, generation, local, status, reason) {
                    Ok(Some(state)) => vec![(
                        if local {
                            Target::Client(client_id)
                        } else {
                            Target::Broadcast
                        },
                        ServerMessage::PaneRuntimeChanged {
                            pane_id,
                            local,
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
                }
            }
            ClientMessage::ReportPaneRows {
                pane_id,
                local,
                generation,
                rows,
            } => match self.report_pane_rows(client_id, pane_id, generation, local, rows) {
                Ok(Some(state)) => vec![(
                    if local {
                        Target::Client(client_id)
                    } else {
                        Target::Broadcast
                    },
                    ServerMessage::PaneRuntimeChanged {
                        pane_id,
                        local,
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
                local,
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
                cell_width,
                cell_height,
            } => {
                if local && self.client_read_only(client_id) {
                    return vec![(
                        Target::Sender,
                        ServerMessage::SpawnResult {
                            pane_id,
                            local,
                            generation,
                            pid: None,
                            ok: false,
                            error: Some("read-only client".to_string()),
                        },
                    )];
                }
                if !local && !self.is_controller(client_id) {
                    return vec![(
                        Target::Sender,
                        ServerMessage::SpawnResult {
                            pane_id,
                            local,
                            generation,
                            pid: None,
                            ok: false,
                            error: Some("not controller".to_string()),
                        },
                    )];
                }
                let initial_seed = !local
                    && self.created_from_profile.is_none()
                    && self.origin_seed_client.is_none()
                    && self.panes.is_empty()
                    && self.layout.is_none();
                let message = self.spawn_pane(SpawnRequest {
                    pane_id,
                    owner: local.then_some(client_id),
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
                    cell: cell_size(cell_width, cell_height),
                });
                if initial_seed && matches!(message, ServerMessage::SpawnResult { ok: true, .. }) {
                    self.origin_seed_client = Some(client_id);
                }
                vec![(Target::Sender, message)]
            }
            ClientMessage::Resize {
                pane_id,
                local,
                generation,
                cols,
                rows,
                cell_width,
                cell_height,
            } => {
                let owner = local.then_some(client_id);
                if owner.is_none() && !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(owner, pane_id, generation) {
                    pane.cols = cols.max(1);
                    pane.rows = rows.max(1);
                    let (rows, cols) = (pane.rows, pane.cols);
                    pane.screen_mut().resize(rows, cols);
                    // The controller's cell size is canonical alongside its pane size: the child
                    // reads it out of the PTY to decide how many cells a picture needs, and the
                    // pane that renders that picture is measuring against the same value.
                    if let Some(cell) = cell_size(cell_width, cell_height) {
                        pane.cell = cell;
                        pane.screen_mut().set_cell_size(cell);
                    }
                    if let Some(pty) = &pane.pty {
                        let _ = pty.resize_with_cell_size(pane.cols, pane.rows, pane.cell);
                    }
                    // Broadcast so every client's parser reshapes at the same byte position.
                    return vec![(
                        owner.map_or(Target::Broadcast, Target::Client),
                        ServerMessage::Resized {
                            pane_id,
                            local,
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
                local,
                generation,
            } => {
                let owner = local.then_some(client_id);
                if owner.is_none() && !self.is_controller(client_id) {
                    return Vec::new();
                }
                if self
                    .pane(owner, pane_id)
                    .is_some_and(|pane| pane.generation == generation)
                    && let Some(pane) = match owner {
                        Some(owner) => self.local_panes.remove(&(owner, pane_id)),
                        None => self.panes.remove(&pane_id),
                    }
                {
                    if let Some(pty) = &pane.pty {
                        let _ = pty.kill();
                    }
                    if owner.is_none() {
                        self.mark_dirty();
                    }
                }
                Vec::new()
            }
            ClientMessage::SetPalette {
                pane_id,
                local,
                generation,
                palette,
            } => {
                let owner = local.then_some(client_id);
                if owner.is_none() && !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(owner, pane_id, generation) {
                    pane.screen_mut().set_palette(palette.into());
                }
                Vec::new()
            }
            ClientMessage::ConfigurePane {
                pane_id,
                local,
                generation,
                palette,
                title,
                cwd,
            } => {
                let owner = local.then_some(client_id);
                if owner.is_none() && !self.is_controller(client_id) {
                    return Vec::new();
                }
                if let Some(pane) = self.live_pane_mut(owner, pane_id, generation) {
                    if let Some(title) = title {
                        pane.title = Some(title);
                    }
                    if let Some(cwd) = cwd {
                        pane.cwd = Some(cwd);
                    }
                    if let Some(palette) = palette {
                        pane.screen_mut().set_palette(palette.into());
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
            ClientMessage::SetControlTakeover { allowed } => {
                if !self.clients.iter().any(|client| client.id == client_id) {
                    return Vec::new();
                }
                self.handle_set_control_takeover(client_id, allowed)
            }
            ClientMessage::SetParked { parked } => self.handle_set_parked(client_id, parked),
            ClientMessage::GrantControl { to } => self.handle_grant_control(client_id, to),
            ClientMessage::DeclineControl { to } => self.handle_decline_control(client_id, to),
            ClientMessage::EvictClient { target } => self.handle_evict_client(client_id, target),
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
            // Read-only filesystem queries: allowed from any client, including read-only ones and
            // followers, since browsing changes nothing and is per-client view state.
            ClientMessage::ListDirectory { path, show_hidden } => self
                .request_browse(BrowseRequest::Directory {
                    client_id,
                    path,
                    show_hidden,
                })
                .map(|message| vec![(Target::Client(client_id), message)])
                .unwrap_or_default(),
            ClientMessage::ListChanges { root } => self
                .request_browse(BrowseRequest::Changes { client_id, root })
                .map(|message| vec![(Target::Client(client_id), message)])
                .unwrap_or_default(),
            ClientMessage::RequestRuntimeMetrics => {
                let known = self.clients.iter().any(|client| client.id == client_id);
                if known {
                    vec![(
                        Target::Client(client_id),
                        ServerMessage::RuntimeMetrics {
                            metrics: self.runtime_metrics(),
                        },
                    )]
                } else {
                    Vec::new()
                }
            }
            ClientMessage::Detach => Vec::new(),
            ClientMessage::Shutdown => {
                if !self.client_attached(client_id) || self.client_read_only(client_id) {
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

    fn request_browse(&mut self, request: BrowseRequest) -> Option<ServerMessage> {
        if request.path_len() > MAX_BROWSE_PATH_BYTES {
            return Some(ServerMessage::Error {
                code: "browse-path-too-long".to_string(),
                message: "browse path exceeds the server limit".to_string(),
            });
        }

        let key = request.key();
        if let Some(state) = self.browse_in_flight.get_mut(&key) {
            if state.submitted {
                state.rerun = Some(request);
            } else {
                state.request = request;
            }
            return None;
        }
        if self.browse_in_flight.len() >= MAX_BROWSE_PENDING {
            return Some(request.error_response("too many browse requests are pending"));
        }
        let client_id = request.client_id();
        if self
            .browse_in_flight
            .values()
            .filter(|state| state.request.client_id() == client_id)
            .count()
            >= MAX_BROWSE_PENDING_PER_CLIENT
        {
            return Some(request.error_response("too many client browse requests are pending"));
        }

        if self.browse_worker.is_none() {
            self.browse_worker = Some(BrowseWorker::new(Arc::clone(&self.events)));
        }
        let Some(worker) = self.browse_worker.as_ref() else {
            return Some(request.error_response("browse worker unavailable"));
        };
        match worker.try_submit(request.clone()) {
            Ok(()) => {
                self.browse_in_flight.insert(
                    key,
                    BrowseState {
                        request,
                        rerun: None,
                        submitted: true,
                    },
                );
                None
            }
            Err(mpsc::TrySendError::Full(request)) => {
                self.browse_in_flight.insert(
                    key,
                    BrowseState {
                        request,
                        rerun: None,
                        submitted: false,
                    },
                );
                None
            }
            Err(mpsc::TrySendError::Disconnected(request)) => {
                Some(request.error_response("browse worker unavailable"))
            }
        }
    }

    pub(super) fn retry_browse_requests(&mut self) {
        let pending: Vec<_> = self
            .browse_in_flight
            .iter()
            .filter_map(|(key, state)| (!state.submitted).then_some(key.clone()))
            .collect();
        for key in pending {
            let Some(state) = self.browse_in_flight.get(&key) else {
                continue;
            };
            let client_id = state.request.client_id();
            if !self.client_attached(client_id) {
                self.browse_in_flight.remove(&key);
                continue;
            }
            let request = state.request.clone();
            let submitted = if let Some(worker) = self.browse_worker.as_ref() {
                worker.try_submit(request)
            } else {
                Err(mpsc::TrySendError::Disconnected(request))
            };
            match submitted {
                Ok(()) => {
                    if let Some(state) = self.browse_in_flight.get_mut(&key) {
                        state.submitted = true;
                    }
                }
                Err(mpsc::TrySendError::Full(request)) => {
                    if let Some(state) = self.browse_in_flight.get_mut(&key) {
                        state.request = request;
                    }
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(request)) => {
                    self.browse_in_flight.remove(&key);
                    self.enqueue_browse_response(
                        client_id,
                        request.error_response("browse worker unavailable"),
                    );
                }
            }
        }
    }

    pub(super) fn enqueue_browse_response(&mut self, client_id: ClientId, message: ServerMessage) {
        if !self.client_attached(client_id) {
            return;
        }
        if encode_control(&message).is_some() {
            self.enqueue(client_id, Target::Client(client_id), message);
            return;
        }

        let fallback = match message {
            ServerMessage::DirectoryListing { path, .. } => ServerMessage::DirectoryListing {
                path,
                entries: Vec::new(),
                error: Some("browse result was too large for the protocol".to_string()),
            },
            ServerMessage::ChangeListing { root, .. } => ServerMessage::ChangeListing {
                root,
                changes: Vec::new(),
                error: Some("browse result was too large for the protocol".to_string()),
            },
            _ => ServerMessage::Error {
                code: "browse-result-encode-failed".to_string(),
                message: "browse result could not be encoded".to_string(),
            },
        };
        if encode_control(&fallback).is_some() {
            self.enqueue(client_id, Target::Client(client_id), fallback);
        } else {
            self.enqueue(
                client_id,
                Target::Client(client_id),
                ServerMessage::Error {
                    code: "browse-result-encode-failed".to_string(),
                    message: "browse result could not be encoded".to_string(),
                },
            );
        }
    }

    pub(super) fn handle_attach(
        &mut self,
        client_id: ClientId,
        session: String,
        protocol_version: u32,
        min_protocol_version: u32,
        label: String,
        read_only: bool,
    ) -> Vec<(Target, ServerMessage)> {
        let effective = match protocol::negotiate_protocol(
            protocol_version,
            min_protocol_version,
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
        ) {
            Ok(effective) => effective,
            Err(mismatch) => {
                return vec![(
                    Target::Sender,
                    ServerMessage::Error {
                        code: "protocol-mismatch".to_string(),
                        message: mismatch.message(),
                    },
                )];
            }
        };
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
            client.effective_protocol = effective;
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
            effective_protocol: effective,
            session,
            client_id,
            panes: self.pane_meta(),
            layout_rev: self.layout_rev,
            layout: self.layout.clone(),
            controller: self.controller,
            clients,
            input_locked: self.input_locked,
            allow_takeover: self.allow_takeover,
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
        min_protocol_version: u32,
    ) -> Vec<(Target, ServerMessage)> {
        let effective = match protocol::negotiate_protocol(
            protocol_version,
            min_protocol_version,
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
        ) {
            Ok(effective) => effective,
            Err(mismatch) => {
                return vec![(
                    Target::Sender,
                    ServerMessage::Error {
                        code: "protocol-mismatch".to_string(),
                        message: mismatch.message(),
                    },
                )];
            }
        };
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
                effective_protocol: effective,
                created_from_profile: self.created_from_profile.clone(),
            },
        )]
    }
}
