use super::*;

/// What a client says about itself when it attaches.
pub(super) struct AttachRequest {
    pub capabilities: Option<protocol::Capabilities>,
    pub session: String,
    pub protocol_version: u32,
    pub min_protocol_version: u32,
    pub label: String,
    pub read_only: bool,
    pub shares_filesystem: bool,
    pub expected_server_nonce: Option<String>,
}

fn valid_origin(origin: &SessionOrigin) -> bool {
    !origin.is_empty()
        && origin
            .profile
            .as_deref()
            .is_none_or(crate::session::discovery::valid_session_name)
        && origin.worktree.as_ref().is_none_or(|worktree| {
            worktree.path.len() <= MAX_BROWSE_PATH_BYTES
                && std::path::Path::new(&worktree.path).is_absolute()
        })
}

/// Canonical pane geometry reported by a client.
pub(super) struct PaneResizeRequest {
    pub pane_id: PaneId,
    pub local: bool,
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl SessionServer {
    pub(super) fn accept_new(&mut self, listener: &IpcListener) -> io::Result<bool> {
        let mut accepted = false;
        loop {
            match listener.accept() {
                Ok(stream) => {
                    accepted = true;
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    self.clients.push(ClientConn::new(id, stream));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(accepted),
                Err(err) => return Err(err),
            }
        }
    }

    pub(super) fn pump_clients(&mut self) -> bool {
        let mut activity = self.poll_attachment_work();
        let mut inbound: Vec<(ClientId, Frame<ClientMessage>)> = Vec::new();
        let mut dead: Vec<ClientId> = Vec::new();
        for client in &mut self.clients {
            if client.transport_closed {
                continue;
            }
            for _ in 0..16 {
                match client.decoder.read_from_status(&mut client.stream) {
                    Ok(protocol::FrameReadStatus::Read(_)) => activity = true,
                    Ok(protocol::FrameReadStatus::WouldBlock) => break,
                    Ok(protocol::FrameReadStatus::Eof) | Err(_) => {
                        activity = true;
                        dead.push(client.id);
                        break;
                    }
                }
            }
            loop {
                match client.decoder.next_frame::<ClientMessage>() {
                    Ok(Some(frame)) => {
                        activity = true;
                        inbound.push((client.id, frame));
                    }
                    Ok(None) => break,
                    Err(_) => {
                        activity = true;
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
            self.disconnect_after_controls(id);
        }
        activity
    }

    pub(super) fn disconnect_after_controls(&mut self, id: ClientId) {
        // EOF can arrive in the same read as a final layout commit and Detach. Keep the
        // attached identity until its already-decoded control stream has drained.
        if self.media.client_fenced(id) {
            if let Some(client) = self.client_mut(id) {
                client.transport_closed = true;
            }
        } else {
            self.remove_client(id);
        }
    }

    /// Write a client's pane input, if that client may type into the pane.
    fn accept_pane_input(
        &mut self,
        id: ClientId,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        bytes: &[u8],
        latency_sensitive: bool,
    ) {
        let owner = local.then_some(id);
        let may_operate = if local {
            self.client_attached(id) && !self.client_read_only(id)
        } else {
            self.client_may_input(id)
        };
        if may_operate {
            if latency_sensitive {
                self.handle_pane_input(owner, pane_id, generation, bytes);
            } else {
                self.write_pane_input(owner, pane_id, generation, bytes);
            }
        }
    }

    fn accept_presentation_acks(&mut self, id: ClientId, acks: Vec<protocol::PresentationAck>) {
        if !self.client_attached(id) {
            self.set_close_after_flush(id);
            return;
        }
        for ack in acks {
            let key = PresentationKey {
                local: ack.local,
                pane_id: ack.pane_id,
                generation: ack.generation,
            };
            if !self
                .pane(ack.local.then_some(id), ack.pane_id)
                .is_some_and(|pane| {
                    pane.generation == ack.generation && ack.sequence <= pane.output_sequence
                })
            {
                continue;
            }
            let latency = self
                .pane(ack.local.then_some(id), ack.pane_id)
                .and_then(|pane| {
                    pane.output_times
                        .iter()
                        .rev()
                        .find(|(sequence, _)| *sequence == ack.sequence)
                        .map(|(_, time)| crate::runtime_metrics::duration_micros(time.elapsed()))
                });
            let advanced = self.client_mut(id).is_some_and(|client| {
                let presented = client.presented_sequences.entry(key).or_default();
                if ack.sequence <= *presented {
                    return false;
                }
                *presented = ack.sequence;
                client.presented_at.insert(key, Instant::now());
                true
            });
            if advanced {
                self.graphics_metrics.presentation_acks += 1;
                if let Some(latency) = latency {
                    self.graphics_metrics.presentation_ack_latency_samples += 1;
                    self.graphics_metrics.presentation_ack_latency_us += latency;
                    self.graphics_metrics.presentation_ack_latency_max_us = self
                        .graphics_metrics
                        .presentation_ack_latency_max_us
                        .max(latency);
                }
            }
        }
        self.prune_presentation_state(id);
    }

    /// Forget a client's presentation state for pane generations that no longer exist, so its
    /// maps stay bounded by the panes it can still see.
    fn prune_presentation_state(&mut self, id: ClientId) {
        let live: HashSet<PresentationKey> = self
            .clients
            .iter()
            .filter(|client| client.id == id)
            .flat_map(|client| {
                client
                    .presentation_interest
                    .keys()
                    .chain(client.presented_sequences.keys())
                    .copied()
            })
            .filter(|key| {
                self.pane(key.local.then_some(id), key.pane_id)
                    .is_some_and(|pane| pane.generation == key.generation)
            })
            .collect();
        if let Some(client) = self.client_mut(id) {
            client
                .presentation_interest
                .retain(|key, _| live.contains(key));
            client
                .presented_sequences
                .retain(|key, _| live.contains(key));
            client.presented_at.retain(|key, _| live.contains(key));
        }
    }

    fn accept_presentation_interest(
        &mut self,
        id: ClientId,
        interest: Vec<protocol::PanePresentationInterest>,
    ) {
        if !self.client_attached(id) {
            self.set_close_after_flush(id);
            return;
        }
        for pane in interest {
            if self
                .pane(pane.local.then_some(id), pane.pane_id)
                .is_none_or(|current| current.generation != pane.generation)
            {
                continue;
            }
            if let Some(client) = self.client_mut(id) {
                client.presentation_interest.insert(
                    PresentationKey {
                        local: pane.local,
                        pane_id: pane.pane_id,
                        generation: pane.generation,
                    },
                    pane,
                );
            }
        }
        self.prune_presentation_state(id);
    }

    pub(super) fn process_client_frame(&mut self, id: ClientId, frame: Frame<ClientMessage>) {
        match frame {
            Frame::PaneOutput { .. } => self.set_close_after_flush(id),
            Frame::AcquisitionAcks(acks) => {
                if self
                    .client_mut(id)
                    .is_some_and(|c| !c.acquisitions.acknowledge(&acks))
                {
                    self.disconnect_after_controls(id);
                }
            }
            Frame::PresentationAcks(acks) => self.accept_presentation_acks(id, acks),
            Frame::PresentationInterest(interest) => {
                self.accept_presentation_interest(id, interest)
            }
            Frame::PaneBytes {
                latency_sensitive,
                pane_id,
                local,
                generation,
                bytes,
            } => self.accept_pane_input(id, pane_id, local, generation, &bytes, latency_sensitive),
            Frame::Control(message) => {
                let message = match self.defer_media_barrier(id, message) {
                    Ok(message) => message,
                    Err(responses) => {
                        for (target, message) in responses {
                            self.enqueue(id, target, message);
                        }
                        return;
                    }
                };
                self.process_client_control_now(id, message);
            }
        }
    }

    /// Execute a control only after its client stream fence has released it. Lifecycle effects
    /// such as Detach must run here too, not when the deferred frame was first received.
    pub(super) fn process_client_control_now(&mut self, id: ClientId, message: ClientMessage) {
        let is_attach = matches!(message, ClientMessage::Attach { .. });
        // Both answer one question and hang up: neither ever becomes a client, so both
        // skip the attach requirement below and close once the reply has flushed.
        let is_query = matches!(
            message,
            ClientMessage::Query { .. } | ClientMessage::SessionControl { .. }
        );
        if !self.accepts_client_control(id, &message) {
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
        if is_query
            && !self.holds_agent_wait(id)
            && !self.capture_waits.contains_key(&id)
            && !self.holds_recording_reply(id)
        {
            self.set_close_after_flush(id);
        }
        if detach {
            self.remove_client(id);
        }
    }

    fn accepts_client_control(&mut self, id: ClientId, message: &ClientMessage) -> bool {
        if matches!(
            message,
            ClientMessage::Attach { .. }
                | ClientMessage::Query { .. }
                | ClientMessage::SessionControl { .. }
        ) || self.client_attached(id)
        {
            return true;
        }
        self.enqueue(
            id,
            Target::Sender,
            ServerMessage::Error {
                code: "attach-required".to_string(),
                message: "first client message must be attach".to_string(),
            },
        );
        self.set_close_after_flush(id);
        false
    }

    /// Spawn policy describes the *next* pane and `[recording]` the next recording, so a request
    /// that opens one re-reads its part of the config rather than using whatever this server
    /// started with - which for a session nobody has attached to in a week is config from a week
    /// ago. Only those requests pay the read; a polling `list-panes` must not stat the config file
    /// every few seconds. See [`SessionServer::reload_spawn_policy`].
    fn reload_config_for(&mut self, command: &control::ControlCommand) {
        match command {
            control::ControlCommand::NewPane { .. } => self.reload_spawn_policy(),
            control::ControlCommand::RecordStart { .. } => self.reload_recording_defaults(),
            _ => {}
        }
    }

    // Keep the fallible PTY operation injectable so retry behavior can be tested without
    // depending on an OS producing a transient ioctl/ConPTY failure.
    pub(super) fn handle_resize_with(
        &mut self,
        client_id: ClientId,
        request: PaneResizeRequest,
        resize_pty: impl FnOnce(&ServerPane) -> io::Result<()>,
    ) -> Vec<(Target, ServerMessage)> {
        let PaneResizeRequest {
            pane_id,
            local,
            generation,
            cols,
            rows,
            cell_width,
            cell_height,
        } = request;
        let owner = local.then_some(client_id);
        if owner.is_none() && !self.is_controller(client_id) {
            return Vec::new();
        }
        let image_media = self.image_media();
        let boundary = self.pane(owner, pane_id).is_some_and(|pane| {
            pane.generation == generation
                && (pane.cols != cols.max(1)
                    || pane.rows != rows.max(1)
                    || protocol::wire_cell(cell_width, cell_height)
                        .is_some_and(|cell| cell != pane.cell))
        });
        if boundary {
            if let Some(pane) = self.pane_mut(owner, pane_id) {
                pane.graphics_pacer.reset();
                if let Some(pty) = &pane.pty {
                    pty.read_gate().resume();
                }
                pane.screen_without_change()
                    .flush_output_transaction_boundary();
                pane.output_coalesce_epoch = pane.output_coalesce_epoch.saturating_add(1);
            }
            self.send_outbound(
                owner,
                &ServerOutbound::PaneOutput {
                    media: Vec::new(),
                    metadata: protocol::OutputMetadata::default(),
                    pane_id,
                    local,
                    generation,
                    bytes: Vec::new(),
                },
            );
        }
        if let Some(pane) = self.live_pane_mut(owner, pane_id, generation) {
            let (cols, rows) = (cols.max(1), rows.max(1));
            let reported_cell = protocol::wire_cell(cell_width, cell_height);
            let dimensions_changed = cols != pane.cols || rows != pane.rows;
            let cell_changed = reported_cell.is_some_and(|cell| cell != pane.cell);
            if !dimensions_changed && !cell_changed {
                if pane.pty_resize_pending {
                    pane.pty_resize_pending = resize_pty(pane).is_err();
                }
                return Vec::new();
            }
            pane.cols = cols;
            pane.rows = rows;
            // The controller's cell size is canonical alongside its pane size: the child
            // reads it out of the PTY to decide how many cells a picture needs, and the
            // pane that renders that picture is measuring against the same value.
            if let Some(cell) = reported_cell {
                pane.cell = cell;
            }
            if pane.output_seen {
                if dimensions_changed {
                    pane.screen_mut().resize(rows, cols);
                }
                if cell_changed {
                    let cell = pane.cell;
                    pane.screen_mut().set_cell_size(cell);
                }
            } else if dimensions_changed {
                // The spawn request uses a fallback geometry before the client's layout is
                // available. Rebuild the still-empty parser at its authoritative size so a
                // width reflow cannot discard the spare history-allocation slot.
                let mut screen = crate::pane::new_terminal_screen(rows, cols, pane.scrollback);
                screen.set_cell_size(pane.cell);
                screen.set_image_storage_enabled(true);
                screen.set_image_budget(crate::pane::PANE_IMAGE_BUDGET_BYTES);
                image_media.apply(&mut screen);
                screen.set_palette(pane.palette.into());
                pane.replace_empty_screen(screen);
            } else if cell_changed {
                let cell = pane.cell;
                pane.screen_mut().set_cell_size(cell);
            }
            pane.pty_resize_pending = resize_pty(pane).is_err();
            // Broadcast so every client's parser reshapes at the same byte position.
            return vec![(
                owner.map_or(Target::Broadcast, Target::Client),
                ServerMessage::Resized {
                    pane_id,
                    local,
                    generation,
                    cols: pane.cols,
                    rows: pane.rows,
                    cell_width: pane.cell.width,
                    cell_height: pane.cell.height,
                },
            )];
        }
        Vec::new()
    }

    pub(super) fn handle_message(
        &mut self,
        client_id: ClientId,
        message: ClientMessage,
    ) -> Vec<(Target, ServerMessage)> {
        match message {
            ClientMessage::Attach {
                capabilities,
                session,
                protocol_version,
                min_protocol_version,
                label,
                read_only,
                shares_filesystem,
                expected_server_nonce,
            } => self.handle_attach(
                client_id,
                AttachRequest {
                    capabilities,
                    session,
                    protocol_version,
                    min_protocol_version,
                    label,
                    read_only,
                    shares_filesystem,
                    expected_server_nonce,
                },
            ),
            ClientMessage::SetSessionOrigin { origin } => {
                if self.origin.is_empty()
                    && self.origin_seed_client == Some(client_id)
                    && !self.panes.is_empty()
                    && valid_origin(&origin)
                    && self
                        .client_mut(client_id)
                        .is_some_and(|client| !client.read_only)
                {
                    self.origin = origin;
                    self.origin_seed_client = None;
                    self.mark_dirty();
                    return vec![(
                        Target::Broadcast,
                        ServerMessage::SessionOriginSet {
                            origin: self.origin.clone(),
                        },
                    )];
                }
                Vec::new()
            }
            ClientMessage::Query {
                capabilities,
                session,
                protocol_version,
                min_protocol_version,
            } => self.handle_query(
                session,
                protocol_version,
                min_protocol_version,
                capabilities,
            ),
            ClientMessage::SessionControl {
                capabilities,
                session,
                protocol_version,
                min_protocol_version,
                request,
            } => {
                self.reload_config_for(&request.command);
                self.handle_session_control(
                    client_id,
                    session,
                    protocol_version,
                    min_protocol_version,
                    capabilities,
                    request,
                )
            }
            ClientMessage::AttachedControl {
                request_id,
                request,
            } => {
                self.reload_config_for(&request.command);
                self.handle_attached_control(client_id, request_id, request)
            }
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
                            agents: self.agent_records(local.then_some(client_id), pane_id),
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
            ClientMessage::ReportAgent {
                request_id,
                pane_id,
                local,
                generation,
                agent,
                integration,
                state,
                reason,
                native_session,
                seq,
            } => {
                if self.client_read_only(client_id) {
                    return vec![(
                        Target::Sender,
                        ServerMessage::AgentReportResult {
                            request_id,
                            response: control::ControlResponse::error_with(
                                control::ControlErrorCode::ReadOnly,
                                "read-only client",
                            ),
                        },
                    )];
                }
                let owner = local.then_some(client_id);
                match self.apply_agent_integration(
                    owner,
                    pane_id,
                    generation,
                    runtime::AgentIntegrationUpdate {
                        agent,
                        integration,
                        state,
                        reason,
                        native_session,
                        seq,
                    },
                ) {
                    Ok(Some(state)) => vec![
                        (
                            owner.map_or(Target::Broadcast, Target::Client),
                            ServerMessage::PaneRuntimeChanged {
                                pane_id,
                                local,
                                generation,
                                agents: self.agent_records(owner, pane_id),
                                state,
                            },
                        ),
                        (
                            Target::Sender,
                            ServerMessage::AgentReportResult {
                                request_id,
                                response: control::ControlResponse::empty(),
                            },
                        ),
                    ],
                    Ok(None) => vec![(
                        Target::Sender,
                        ServerMessage::AgentReportResult {
                            request_id,
                            response: control::ControlResponse::empty(),
                        },
                    )],
                    Err((code, message)) => vec![(
                        Target::Sender,
                        ServerMessage::AgentReportResult {
                            request_id,
                            response: control::ControlResponse::error_with(
                                runtime::integration_error_code(code),
                                message,
                            ),
                        },
                    )],
                }
            }
            ClientMessage::ReloadSleepPolicy => {
                if self.is_controller(client_id) && !self.client_read_only(client_id) {
                    self.reload_sleep_policy();
                }
                Vec::new()
            }
            ClientMessage::ReloadSpawnPolicy => {
                if self.is_controller(client_id) && !self.client_read_only(client_id) {
                    self.reload_spawn_policy();
                }
                Vec::new()
            }
            ClientMessage::ReloadAgents => {
                if self.is_controller(client_id) && !self.client_read_only(client_id) {
                    self.reload_agent_definitions();
                }
                Vec::new()
            }
            ClientMessage::ReportPaneRows {
                pane_id,
                local,
                generation,
                publisher,
                kind,
                rows,
            } => match self.report_pane_rows(
                client_id,
                pane_id,
                generation,
                local,
                runtime::PanePublisher {
                    id: publisher,
                    kind,
                },
                rows,
            ) {
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
                        agents: self.agent_records(local.then_some(client_id), pane_id),
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
                placement,
                pane_id,
                local,
                generation,
                launch,
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
                let initial_seed = !local
                    && self.origin.is_empty()
                    && self.origin_seed_client.is_none()
                    && self.panes.is_empty()
                    && self.layout.is_none();
                let request = SpawnRequest {
                    pane_id,
                    owner: local.then_some(client_id),
                    generation,
                    launch,
                    agent_resume: None,
                    cwd,
                    title,
                    cols,
                    rows,
                    keep_open,
                    env,
                    palette,
                    shell,
                    command_shell,
                    cell: protocol::wire_cell(cell_width, cell_height),
                };
                let mut messages = Vec::new();
                let message = if local {
                    if placement.is_some() {
                        return Vec::new();
                    }
                    self.spawn_pane(request)
                } else {
                    self.spawn_shared_pane(request, placement, client_id, &mut messages)
                };
                if initial_seed && matches!(message, ServerMessage::SpawnResult { ok: true, .. }) {
                    self.origin_seed_client = Some(client_id);
                }
                if local
                    && matches!(message, ServerMessage::SpawnResult { ok: true, .. })
                    && let Some(metadata) = self.pane_metadata_message(Some(client_id), pane_id)
                {
                    messages.push((Target::Sender, metadata));
                }
                if !local
                    && matches!(message, ServerMessage::SpawnResult { ok: false, .. })
                    && !messages
                        .iter()
                        .any(|(_, message)| matches!(message, ServerMessage::LayoutRejected { .. }))
                {
                    messages.push((
                        Target::Sender,
                        ServerMessage::LayoutRejected {
                            current_rev: self.layout_rev,
                            layout: self.layout.clone(),
                        },
                    ));
                }
                messages.insert(0, (Target::Sender, message));
                messages
            }
            ClientMessage::Resize {
                pane_id,
                local,
                generation,
                cols,
                rows,
                cell_width,
                cell_height,
            } => self.handle_resize_with(
                client_id,
                PaneResizeRequest {
                    pane_id,
                    local,
                    generation,
                    cols,
                    rows,
                    cell_width,
                    cell_height,
                },
                |pane| {
                    pane.pty.as_ref().map_or(Ok(()), |pty| {
                        pty.resize_with_cell_size(pane.cols, pane.rows, pane.cell)
                    })
                },
            ),
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
                self.resolve_agent_waits();
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
            ClientMessage::CommitLayout {
                base_rev,
                layout,
                live,
            } => {
                let responses = self.handle_commit_layout(client_id, base_rev, layout, live);
                if self.origin.is_empty()
                    && responses.iter().any(|(_, message)| {
                        matches!(message, ServerMessage::LayoutCommitted { .. })
                    })
                {
                    self.origin_seed_client = None;
                }
                responses
            }
            // Pure relay: a drag is presence, not state. Keeping none of it here is what makes a
            // controller that vanishes mid-gesture self-healing - there is no stale lift for a new
            // controller to inherit, and the `ControllerChanged` every disconnect already
            // broadcasts is what tells followers to stop lifting.
            ClientMessage::DragUpdate { pane_id, rect } => {
                if !self.is_controller(client_id) || self.client_read_only(client_id) {
                    return Vec::new();
                }
                vec![(
                    Target::Broadcast,
                    ServerMessage::DragChanged {
                        author: client_id,
                        drag: Some(crate::state::RemoteDrag { pane_id, rect }),
                    },
                )]
            }
            ClientMessage::DragEnd => {
                if !self.is_controller(client_id) || self.client_read_only(client_id) {
                    return Vec::new();
                }
                vec![(
                    Target::Broadcast,
                    ServerMessage::DragChanged {
                        author: client_id,
                        drag: None,
                    },
                )]
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
            ClientMessage::MarkedInput {
                pane_id,
                local,
                generation,
                bytes,
                token,
            } => {
                // The mark divides the pane's output for the sender's wait, so output after it must
                // not replace independent graphics queued ahead of it, in any outbox or mailbox.
                if let Some(pane) = self
                    .pane_mut(local.then_some(client_id), pane_id)
                    .filter(|pane| pane.generation == generation)
                {
                    pane.output_coalesce_epoch = pane.output_coalesce_epoch.saturating_add(1);
                }
                self.accept_pane_input(client_id, pane_id, local, generation, &bytes, true);
                vec![(Target::Sender, ServerMessage::InputMarked { token })]
            }
            ClientMessage::Pong { seq: _ } => {
                if let Some(client) = self.client_mut(client_id) {
                    client.last_pong = Instant::now();
                }
                Vec::new()
            }
            ClientMessage::Attachment {
                transfer_id,
                operation,
            } => self.attachment_step(client_id, transfer_id, operation),
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
            ClientMessage::Worktree {
                request_id,
                request,
            } => self
                .request_worktree(client_id, request_id, request)
                .map(|message| vec![(Target::Client(client_id), message)])
                .unwrap_or_default(),
            ClientMessage::RequestRuntimeMetrics => {
                let known = self.clients.iter().any(|client| {
                    client.id == client_id
                        && client.capabilities.supports(protocol::RUNTIME_METRICS)
                });
                if known {
                    vec![(
                        Target::Client(client_id),
                        ServerMessage::RuntimeMetrics {
                            metrics: Box::new(self.runtime_metrics()),
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

    fn request_worktree(
        &mut self,
        client_id: ClientId,
        request_id: u64,
        request: protocol::WorktreeRequest,
    ) -> Option<ServerMessage> {
        let failed = |message: &str| ServerMessage::WorktreeResult {
            request_id,
            result: protocol::WorktreeResult::Failed {
                message: message.to_string(),
            },
        };
        if !self.client_attached(client_id) {
            return Some(failed("client is not attached"));
        }
        if !matches!(
            &request,
            protocol::WorktreeRequest::Branches { .. }
                | protocol::WorktreeRequest::List { .. }
                | protocol::WorktreeRequest::Preview { .. }
                | protocol::WorktreeRequest::Status { .. }
        ) && self.client_read_only(client_id)
        {
            return Some(failed("worktree mutation requires a writable client"));
        }
        let job = WorktreeJob {
            client_id,
            request_id,
            request,
        };
        if worktree::request_too_large(&job) {
            return Some(failed("worktree request exceeds the server limit"));
        }
        if self.worktree_worker.is_none() {
            self.worktree_worker = Some(WorktreeWorker::new(
                self.settings.worktree_directory.clone(),
            ));
        }
        match self.worktree_worker.as_ref()?.try_submit(job) {
            Ok(()) => None,
            Err(mpsc::TrySendError::Full(_)) => {
                Some(failed("too many worktree requests are pending"))
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Some(failed("worktree worker unavailable")),
        }
    }

    pub(super) fn drain_worktree_results(&mut self) {
        let Some(worker) = self.worktree_worker.as_ref() else {
            return;
        };
        for done in worker.drain() {
            if !self.client_attached(done.client_id) {
                continue;
            }
            let mut response = ServerMessage::WorktreeResult {
                request_id: done.request_id,
                result: done.result,
            };
            if encode_control(&response).is_none() {
                response = ServerMessage::WorktreeResult {
                    request_id: done.request_id,
                    result: protocol::WorktreeResult::Failed {
                        message: "worktree result exceeds the protocol limit".to_string(),
                    },
                };
            }
            self.enqueue(done.client_id, Target::Client(done.client_id), response);
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
        request: AttachRequest,
    ) -> Vec<(Target, ServerMessage)> {
        let AttachRequest {
            capabilities,
            session,
            protocol_version,
            min_protocol_version,
            label,
            read_only,
            shares_filesystem,
            expected_server_nonce,
        } = request;
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
        if let Some(expected) = expected_server_nonce
            && self.settings.startup_nonce.as_deref() != Some(expected.as_str())
        {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "server-identity-mismatch".to_string(),
                    message: "the session endpoint belongs to a different server".to_string(),
                },
            )];
        }
        // Complete output encoded for the existing recipients, named files or frame edits, before
        // this reader joins the common representation. The new reader's baseline replay carries
        // pixels.
        while self.media.has_reader_specific_output() {
            self.pump_media_output();
        }
        let capabilities = protocol::Capabilities::negotiated(capabilities.as_ref());
        if let Some(client) = self.client_mut(client_id) {
            client.capabilities = capabilities.clone();
            client.attached = true;
            client.label = Some(label);
            client.read_only = read_only;
            client.shares_filesystem = shares_filesystem;
            client.last_pong = Instant::now();
            client.effective_protocol = effective;
        }
        self.sync_image_media_policy();
        // First attacher is auto-granted the layout-control lease.
        let granted = if self.controller.is_none() && !read_only {
            self.controller = Some(client_id);
            true
        } else {
            false
        };
        let clients = self.client_roster();
        let attached = ServerMessage::Attached {
            capabilities: Some(capabilities),
            session_instance: self.instance_id.clone(),
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
            origin: self.origin.clone(),
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
        capabilities: Option<protocol::Capabilities>,
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
        let capabilities = protocol::Capabilities::negotiated(capabilities.as_ref());
        let agents = if capabilities.supports(protocol::AGENT_SUMMARIES) {
            self.agent_summaries()
        } else {
            Vec::new()
        };
        let projects = if capabilities.supports(protocol::PROJECT_SUMMARIES) {
            self.project_summaries()
        } else {
            Vec::new()
        };
        vec![(
            Target::Sender,
            ServerMessage::SessionInfo {
                capabilities: Some(capabilities),
                agents,
                projects,
                session,
                panes,
                clients: self.attached_count(),
                has_layout: self.layout.is_some(),
                effective_protocol: effective,
                origin: self.origin.clone(),
                instance: Some(self.instance_id.clone()),
            },
        )]
    }
}
