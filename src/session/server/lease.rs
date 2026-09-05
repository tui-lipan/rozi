use super::*;

impl SessionServer {
    pub(super) fn credit_server_stall(&mut self, stalled_for: Duration) {
        if stalled_for < HEARTBEAT_STALL_THRESHOLD {
            return;
        }
        let now = Instant::now();
        for client in &mut self.clients {
            client.last_pong = client
                .last_pong
                .checked_add(stalled_for)
                .unwrap_or(now)
                .min(now);
            client.last_ping = client
                .last_ping
                .checked_add(stalled_for)
                .unwrap_or(now)
                .min(now);
        }
    }

    pub(super) fn validate_shared_layout_against_panes(&self, layout: &SharedLayout) -> bool {
        let mut layout_ids = std::collections::HashSet::new();
        for ws in &layout.workspaces {
            for pane in &ws.panes {
                if pane.pane_id == crate::state::POPUP_PANE_ID {
                    return false;
                }
                let Some(server_pane) = self.panes.get(&pane.pane_id) else {
                    return false;
                };
                if server_pane.exited.is_some() || server_pane.generation != pane.generation {
                    return false;
                }
                layout_ids.insert(pane.pane_id);
            }
        }
        self.panes
            .iter()
            .all(|(id, pane)| pane.exited.is_some() || layout_ids.contains(id))
    }

    pub(super) fn handle_commit_layout(
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
        if base_rev != self.layout_rev
            || layout.validate().is_err()
            || !self.validate_shared_layout_against_panes(&layout)
        {
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
        self.mark_dirty();
        vec![(
            Target::Broadcast,
            ServerMessage::LayoutCommitted {
                rev: self.layout_rev,
                author: client_id,
                layout,
            },
        )]
    }

    /// A request for the lease. Auto-grants when there is no controller (nobody to ask); otherwise
    /// takes immediately when takeover is enabled; otherwise flags the requester in the roster and
    /// notifies the controller, debounced per requester so a held key cannot spam with toasts.
    pub(super) fn handle_request_control(
        &mut self,
        client_id: ClientId,
    ) -> Vec<(Target, ServerMessage)> {
        // A parked client has nothing on screen to control; it takes the lease by unparking.
        if self.client_read_only(client_id)
            || !self.client_attached(client_id)
            || self.client_parked(client_id)
        {
            return Vec::new();
        }
        if self.controller == Some(client_id) {
            return Vec::new();
        }
        if self.controller.is_none() {
            return self.assign_controller(client_id, ControllerChangeReason::Granted);
        }
        if self.allow_takeover {
            return self.assign_controller(client_id, ControllerChangeReason::Granted);
        }
        let controller = self.controller;
        let mut responses = Vec::new();
        let Some(client) = self.client_mut(client_id) else {
            return responses;
        };
        let already = client.requesting_control;
        client.requesting_control = true;
        let notify = client
            .last_request_notify
            .is_none_or(|last| last.elapsed() >= REQUEST_NOTIFY_COOLDOWN);
        if notify {
            client.last_request_notify = Some(Instant::now());
        }
        if !already {
            responses.push((Target::Broadcast, self.clients_changed()));
        }
        if notify && let Some(controller) = controller {
            responses.push((
                Target::Client(controller),
                ServerMessage::ControlRequested { from: client_id },
            ));
        }
        responses
    }

    /// A client declaring whether it is parked — attached, but keeping the session in the
    /// background rather than using it.
    ///
    /// Parking releases the layout-control lease. Holding it from the background is what made a
    /// second client attach as a follower of someone who was not even looking at the session; a
    /// parked client has no view to keep in sync, so it has no claim on control. Unparking asks for
    /// the lease back and gets it when nobody else holds it, which is the common case: the client
    /// is returning to a session it left parked.
    pub(super) fn handle_set_parked(
        &mut self,
        client_id: ClientId,
        parked: bool,
    ) -> Vec<(Target, ServerMessage)> {
        let Some(client) = self
            .client_mut(client_id)
            .filter(|client| client.parked != parked)
        else {
            return Vec::new();
        };
        client.parked = parked;
        if parked {
            if self.controller == Some(client_id) {
                return self.release_controller();
            }
        } else if self.controller.is_none() && !self.client_read_only(client_id) {
            return self.assign_controller(client_id, ControllerChangeReason::Granted);
        }
        vec![(Target::Broadcast, self.clients_changed())]
    }

    /// Hand the lease to the next client entitled to it, or leave the session uncontrolled when
    /// there is none. Used when the controller gives it up rather than leaves.
    fn release_controller(&mut self) -> Vec<(Target, ServerMessage)> {
        self.controller = self.promotion_candidate();
        if let Some(promoted) = self.controller
            && let Some(client) = self.client_mut(promoted)
        {
            client.requesting_control = false;
            client.last_request_notify = None;
        }
        vec![
            (
                Target::Broadcast,
                ServerMessage::ControllerChanged {
                    controller: self.controller,
                    reason: if self.controller.is_some() {
                        ControllerChangeReason::Granted
                    } else {
                        ControllerChangeReason::Released
                    },
                },
            ),
            (Target::Broadcast, self.clients_changed()),
        ]
    }

    /// The client the lease falls to when the holder gives it up or disappears: the oldest attached
    /// writable client that is actually using the session. Parked clients are skipped — promoting
    /// one would hand control to a background connection and lock out the client in front of the
    /// user. Smallest id = earliest connect.
    fn promotion_candidate(&self) -> Option<ClientId> {
        self.clients
            .iter()
            .filter(|client| client.attached && !client.read_only && !client.parked)
            .map(|client| client.id)
            .min()
    }

    pub(super) fn handle_grant_control(
        &mut self,
        client_id: ClientId,
        to: ClientId,
    ) -> Vec<(Target, ServerMessage)> {
        if !self.is_controller(client_id)
            || !self.client_attached(to)
            || self.client_read_only(to)
            || self.client_parked(to)
        {
            return Vec::new();
        }
        self.assign_controller(to, ControllerChangeReason::Granted)
    }

    pub(super) fn handle_set_control_takeover(
        &mut self,
        client_id: ClientId,
        allowed: bool,
    ) -> Vec<(Target, ServerMessage)> {
        if !self.is_controller(client_id) || self.client_read_only(client_id) {
            return Vec::new();
        }
        self.allow_takeover = allowed;
        vec![(Target::Broadcast, self.clients_changed())]
    }

    /// Controller declines `to`'s pending request: clear its flag, refresh the roster, and tell it.
    pub(super) fn handle_decline_control(
        &mut self,
        client_id: ClientId,
        to: ClientId,
    ) -> Vec<(Target, ServerMessage)> {
        if !self.is_controller(client_id) {
            return Vec::new();
        }
        let Some(client) = self
            .client_mut(to)
            .filter(|client| client.requesting_control)
        else {
            return Vec::new();
        };
        client.requesting_control = false;
        client.last_request_notify = None;
        vec![
            (Target::Broadcast, self.clients_changed()),
            (Target::Client(to), ServerMessage::ControlDeclined),
        ]
    }

    /// Controller removes `target` from the session: tell it why, then close it once that reached
    /// the wire. The roster broadcast follows from the disconnect itself (see
    /// [`Self::flush_clients`]), and the target's own panes keep running — this ends one client's
    /// attachment, not the session.
    ///
    /// A read-only controller cannot evict: it holds the lease only because nobody else does, and
    /// nothing else it can do changes the session for other people either.
    pub(super) fn handle_evict_client(
        &mut self,
        client_id: ClientId,
        target: ClientId,
    ) -> Vec<(Target, ServerMessage)> {
        if !self.is_controller(client_id)
            || self.client_read_only(client_id)
            || target == client_id
            || !self.client_attached(target)
        {
            return Vec::new();
        }
        let by = self
            .clients
            .iter()
            .find(|client| client.id == client_id)
            .and_then(|client| client.label.as_deref())
            .map(|label| format!("{label} #{client_id}"))
            .unwrap_or_else(|| format!("client #{client_id}"));
        self.set_close_after_flush(target);
        vec![(
            Target::Client(target),
            ServerMessage::Error {
                code: protocol::EVICTED_ERROR_CODE.to_string(),
                message: format!("removed from the session by {by}"),
            },
        )]
    }

    /// Move the lease to `to`, clearing its pending request, and broadcast the controller change plus
    /// the refreshed roster (so any request badge on the new controller clears everywhere).
    pub(super) fn assign_controller(
        &mut self,
        to: ClientId,
        reason: ControllerChangeReason,
    ) -> Vec<(Target, ServerMessage)> {
        self.controller = Some(to);
        if let Some(client) = self.client_mut(to) {
            client.requesting_control = false;
            client.last_request_notify = None;
        }
        vec![
            (
                Target::Broadcast,
                ServerMessage::ControllerChanged {
                    controller: self.controller,
                    reason,
                },
            ),
            (Target::Broadcast, self.clients_changed()),
        ]
    }

    pub(super) fn is_controller(&self, client_id: ClientId) -> bool {
        self.controller == Some(client_id)
    }

    pub(super) fn client_mut(&mut self, id: ClientId) -> Option<&mut ClientConn> {
        self.clients.iter_mut().find(|client| client.id == id)
    }

    pub(super) fn client_attached(&self, id: ClientId) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == id && client.attached)
    }

    /// Whether `id` is keeping the session in the background rather than using it.
    pub(super) fn client_parked(&self, id: ClientId) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == id && client.attached && client.parked)
    }

    pub(super) fn client_read_only(&self, id: ClientId) -> bool {
        self.clients
            .iter()
            .find(|client| client.id == id && client.attached)
            .is_none_or(|client| client.read_only)
    }

    pub(super) fn client_may_input(&self, id: ClientId) -> bool {
        self.client_attached(id)
            && !self.client_read_only(id)
            && (!self.input_locked || self.is_controller(id))
    }

    pub(super) fn client_roster(&self) -> Vec<ClientInfo> {
        self.clients
            .iter()
            .filter(|client| client.attached)
            .map(|client| ClientInfo {
                id: client.id,
                label: client.label.clone().unwrap_or_else(|| "client".to_string()),
                read_only: client.read_only,
                requesting_control: client.requesting_control,
                parked: client.parked,
            })
            .collect()
    }

    pub(super) fn clients_changed(&self) -> ServerMessage {
        ServerMessage::ClientsChanged {
            clients: self.client_roster(),
            input_locked: self.input_locked,
            allow_takeover: self.allow_takeover,
        }
    }

    pub(super) fn attached_count(&self) -> u32 {
        self.clients.iter().filter(|client| client.attached).count() as u32
    }

    pub(super) fn set_close_after_flush(&mut self, id: ClientId) {
        if let Some(client) = self.client_mut(id) {
            client.close_after_flush = true;
        }
    }

    /// Remove a client, promoting the oldest surviving attached client to controller if the leaver
    /// held the lease, and broadcasting the resulting client/controller changes.
    pub(super) fn remove_client(&mut self, id: ClientId) {
        self.remove_client_with_reason(id, ControllerChangeReason::Granted);
    }

    fn remove_client_with_reason(
        &mut self,
        id: ClientId,
        promotion_reason: ControllerChangeReason,
    ) {
        self.clear_browse_requests(id);
        if self.origin_seed_client == Some(id) {
            self.origin_seed_client = None;
        }
        let Some(index) = self.clients.iter().position(|client| client.id == id) else {
            return;
        };
        let mut removed = self.clients.remove(index);
        if let Some(seed) = removed.seed.take() {
            self.finish_attach_seed(seed, true);
        }
        let local_ids: Vec<_> = self
            .local_panes
            .keys()
            .filter_map(|(owner, pane_id)| (*owner == id).then_some(*pane_id))
            .collect();
        for pane_id in local_ids {
            if let Some(pane) = self.local_panes.remove(&(id, pane_id))
                && let Some(pty) = pane.pty
            {
                let _ = pty.kill();
            }
        }
        if !removed.attached {
            return;
        }
        // The client that could not reach the filesystem may have just left, which puts naming a
        // file back on the table for the panes.
        self.sync_image_media_policy();
        let mut messages: Vec<ServerMessage> = Vec::new();
        if self.controller == Some(id) {
            self.controller = self.promotion_candidate();
            // A promoted client no longer needs its own pending request.
            if let Some(new_controller) = self.controller
                && let Some(client) = self.client_mut(new_controller)
            {
                client.requesting_control = false;
                client.last_request_notify = None;
            }
            messages.push(ServerMessage::ControllerChanged {
                controller: self.controller,
                reason: if promotion_reason == ControllerChangeReason::Expired {
                    ControllerChangeReason::Expired
                } else if self.controller.is_some() {
                    promotion_reason
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

    fn clear_browse_requests(&mut self, client_id: ClientId) {
        self.browse_in_flight.retain(|key, state| {
            let belongs_to_client = match key {
                BrowseRequestKey::Directory {
                    client_id: key_client_id,
                    ..
                }
                | BrowseRequestKey::Changes {
                    client_id: key_client_id,
                    ..
                } => *key_client_id == client_id,
            };
            if !belongs_to_client {
                return true;
            }
            if state.submitted {
                state.rerun = None;
                true
            } else {
                false
            }
        });
    }

    fn finish_attach_seed(&mut self, seed: AttachSeedState, disconnected: bool) {
        let duration = crate::runtime_metrics::duration_micros(seed.started.elapsed());
        self.attach_seed_totals.last_duration_us = duration;
        self.attach_seed_totals.max_duration_us =
            self.attach_seed_totals.max_duration_us.max(duration);
        if disconnected {
            self.attach_seed_totals.disconnected += 1;
            self.attach_seed_totals.last_disconnect_reason = Some(
                seed.disconnect_reason
                    .unwrap_or("connection-closed")
                    .to_string(),
            );
        } else {
            self.attach_seed_totals.completed += 1;
        }
    }

    pub(super) fn heartbeat(&mut self) {
        let now = Instant::now();
        let mut timed_out: Vec<ClientId> = Vec::new();
        let mut pings: Vec<(ClientId, u64)> = Vec::new();
        for client in &mut self.clients {
            if !client.attached {
                continue;
            }
            // A ping queued behind a replay cannot be answered yet. Start the heartbeat clock once
            // the baseline and its catch-up have reached the socket.
            if client.seed.is_some() {
                client.last_pong = now;
                client.last_ping = now;
                continue;
            }
            if now.duration_since(client.last_pong) >= self.settings.heartbeat_timeout {
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
            self.remove_client_with_reason(id, ControllerChangeReason::Expired);
        }
    }

    pub(super) fn flush_clients(&mut self) -> bool {
        let mut activity = false;
        let mut dead: Vec<ClientId> = Vec::new();
        let mut completed_seeds = Vec::new();
        let now = Instant::now();
        for client in &mut self.clients {
            let (client_activity, mut disconnect) = Self::flush_client_outbox(client);
            activity |= client_activity;
            if let Some(seed) = Self::take_completed_seed(client, now) {
                completed_seeds.push(seed);
            }
            if client.outbox.is_empty() && client.close_after_flush {
                disconnect = true;
            }
            if disconnect {
                dead.push(client.id);
            }
        }
        for seed in completed_seeds {
            self.finish_attach_seed(seed, false);
        }
        for id in dead {
            self.remove_client(id);
        }
        activity
    }

    fn flush_client_outbox(client: &mut ClientConn) -> (bool, bool) {
        let mut activity = false;
        while let Some(front) = client.outbox.front() {
            let class = front.class;
            let chunk = &front.bytes[client.front_offset..];
            match client.stream.write(chunk) {
                Ok(0) => {
                    if let Some(seed) = client.seed.as_mut() {
                        seed.disconnect_reason = Some("transport-closed");
                    }
                    return (true, true);
                }
                Ok(n) => {
                    activity = true;
                    client.front_offset += n;
                    client.outbox_bytes -= n;
                    match class {
                        OutboxClass::Normal => {}
                        OutboxClass::SeedReplay => client.seed_queued_bytes -= n,
                        OutboxClass::SeedCatchUp => client.seed_catch_up_queued_bytes -= n,
                    }
                    if client.front_offset >= front.bytes.len() {
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
                    return (activity, false);
                }
                Err(_) => {
                    if let Some(seed) = client.seed.as_mut() {
                        seed.disconnect_reason = Some("transport-write-failed");
                    }
                    return (true, true);
                }
            }
        }
        (activity, false)
    }

    fn take_completed_seed(client: &mut ClientConn, now: Instant) -> Option<AttachSeedState> {
        let complete = client.seed.as_ref().is_some_and(|seed| {
            seed.baseline_queued
                && client.seed_queued_bytes == 0
                && client.seed_catch_up_queued_bytes == 0
        });
        if !complete {
            return None;
        }
        client.last_pong = now;
        client.last_ping = now;
        client.seed.take()
    }

    pub(super) fn enqueue(&mut self, sender_id: ClientId, target: Target, message: ServerMessage) {
        let delta = seed_delta_for_control(&message);
        let Some(bytes) = encode_control(&message) else {
            return;
        };
        let max_backlog = self.max_backlog;
        match target {
            Target::Sender => {
                let result = self
                    .client_mut(sender_id)
                    .map(|client| client.push_delta(bytes, delta, max_backlog));
                if matches!(result, Some(ClientFrameResult::Overflow)) {
                    self.remove_client(sender_id);
                }
            }
            Target::Client(id) => {
                let result = self
                    .client_mut(id)
                    .filter(|client| client.attached)
                    .map(|client| client.push_delta(bytes, delta, max_backlog));
                if matches!(result, Some(ClientFrameResult::Overflow)) {
                    self.remove_client(id);
                }
            }
            Target::Broadcast => {
                self.push_delta_to_attached(bytes, delta);
            }
        }
        self.note_attach_seed_high_water();
        self.note_outbox_high_water();
    }

    /// Queue one shared encoded frame on every attached client. Each client retains its own write
    /// offset, while the immutable payload allocation is shared.
    #[cfg(test)]
    pub(super) fn push_to_attached(&mut self, bytes: Arc<[u8]>) {
        self.push_delta_to_attached(bytes, SeedDelta::Control);
    }

    fn push_delta_to_attached(&mut self, bytes: Arc<[u8]>, delta: SeedDelta) {
        let mut slow = Vec::new();
        for client in &mut self.clients {
            if !client.attached {
                continue;
            }
            if client.push_delta(Arc::clone(&bytes), delta, self.max_backlog)
                == ClientFrameResult::Overflow
            {
                slow.push(client.id);
            }
        }
        self.note_attach_seed_high_water();
        self.note_outbox_high_water();
        for id in slow {
            self.remove_client(id);
        }
    }

    pub(super) fn broadcast_control(&mut self, message: &ServerMessage) {
        let Some(bytes) = encode_control(message) else {
            return;
        };
        self.push_delta_to_attached(bytes, seed_delta_for_control(message));
    }

    pub(super) fn broadcast_outbound(&mut self, outbound: &ServerOutbound) {
        match outbound {
            ServerOutbound::Control(message) => self.broadcast_control(message),
            ServerOutbound::PaneOutput {
                pane_id,
                local,
                generation,
                bytes,
            } => {
                let Some(frame) = encode_pane_output(*pane_id, *generation, *local, bytes) else {
                    return;
                };
                let delta = if *local {
                    SeedDelta::Control
                } else {
                    SeedDelta::PaneOutput(PaneSeedKey {
                        pane_id: *pane_id,
                        generation: *generation,
                    })
                };
                let mut slow = Vec::new();
                for client in &mut self.clients {
                    if client.attached
                        && client.push_delta(Arc::clone(&frame), delta, self.max_backlog)
                            == ClientFrameResult::Overflow
                    {
                        slow.push(client.id);
                    }
                }
                self.note_attach_seed_high_water();
                self.note_outbox_high_water();
                for id in slow {
                    self.remove_client(id);
                }
            }
        }
    }

    pub(super) fn enqueue_outbound(&mut self, id: ClientId, outbound: &ServerOutbound) {
        let (bytes, delta) = match outbound {
            ServerOutbound::Control(message) => {
                (encode_control(message), seed_delta_for_control(message))
            }
            ServerOutbound::PaneOutput {
                pane_id,
                local,
                generation,
                bytes,
                ..
            } => (
                encode_pane_output(*pane_id, *generation, *local, bytes),
                if *local {
                    SeedDelta::Control
                } else {
                    SeedDelta::PaneOutput(PaneSeedKey {
                        pane_id: *pane_id,
                        generation: *generation,
                    })
                },
            ),
        };
        let Some(bytes) = bytes else { return };
        let max_backlog = self.max_backlog;
        let result = self
            .client_mut(id)
            .filter(|client| client.attached)
            .map(|client| client.push_delta(bytes, delta, max_backlog));
        if matches!(result, Some(ClientFrameResult::Overflow) | None) {
            self.remove_client(id);
        }
        self.note_attach_seed_high_water();
        self.note_outbox_high_water();
    }

    /// Establish the pane manifest for a freshly attached client.
    ///
    /// No replay bytes are exported here. [`Self::pump_attach_seeds`] exports at most one pane per
    /// server iteration and keeps only [`SEED_SEND_WINDOW`] encoded bytes queued. Output for a pane
    /// still pending export is already represented by that future snapshot and is suppressed;
    /// output after export is retained behind the complete baseline.
    pub(super) fn enqueue_attach_seeds(&mut self, id: ClientId) {
        let panes: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.exited.is_none())
            .map(|(pane_id, pane)| PaneSeedKey {
                pane_id: *pane_id,
                generation: pane.generation,
            })
            .collect();
        if let Some(client) = self.client_mut(id) {
            client.seed = Some(AttachSeedState::new(panes));
        }
        self.note_attach_seed_high_water();
    }

    /// Advance all active attach streams under one byte budget and one export budget.
    pub(super) fn pump_attach_seeds(&mut self) -> bool {
        let ids: Vec<_> = self
            .clients
            .iter()
            .filter(|client| {
                client
                    .seed
                    .as_ref()
                    .is_some_and(|seed| !seed.baseline_queued)
            })
            .map(|client| client.id)
            .collect();
        if ids.is_empty() {
            return false;
        }

        let start = self.attach_seed_cursor % ids.len();
        self.attach_seed_cursor = self.attach_seed_cursor.wrapping_add(1);
        let mut remaining = SEED_PUMP_BYTES_PER_TICK;
        let mut export_available = true;
        let mut activity = false;
        for offset in 0..ids.len() {
            if remaining <= protocol::PANE_FRAME_OVERHEAD {
                break;
            }
            let id = ids[(start + offset) % ids.len()];
            activity |= self.pump_client_attach_seed(id, &mut remaining, &mut export_available);
        }
        self.note_attach_seed_high_water();
        self.note_outbox_high_water();
        activity
    }

    fn pump_client_attach_seed(
        &mut self,
        id: ClientId,
        remaining: &mut usize,
        export_available: &mut bool,
    ) -> bool {
        let mut activity = false;
        loop {
            let progressed = match self.next_attach_seed_work(id) {
                AttachSeedWork::ReplayResize => self.pump_replay_resize(id, remaining),
                AttachSeedWork::FinishReplay => {
                    self.finish_pane_replay(id);
                    true
                }
                AttachSeedWork::ReplayChunk => self.pump_replay_chunk(id, remaining),
                AttachSeedWork::BeginPane(key) => {
                    if !*export_available {
                        return activity;
                    }
                    self.begin_pane_replay(id, key);
                    *export_available = false;
                    true
                }
                AttachSeedWork::CatchUp => self.pump_seed_catch_up(id, remaining),
                AttachSeedWork::Complete => {
                    self.complete_attach_baseline(id);
                    return true;
                }
                AttachSeedWork::Done => return activity,
            };
            if !progressed {
                return activity;
            }
            activity = true;
        }
    }

    fn next_attach_seed_work(&self, id: ClientId) -> AttachSeedWork {
        let Some(seed) = self
            .clients
            .iter()
            .find(|client| client.id == id)
            .and_then(|client| client.seed.as_ref())
        else {
            return AttachSeedWork::Done;
        };
        if seed.baseline_queued {
            return AttachSeedWork::Done;
        }
        if let Some(replay) = seed.replay.as_ref() {
            if replay.resize_frame.is_some() {
                return AttachSeedWork::ReplayResize;
            }
            if replay.offset >= replay.bytes.len() {
                return AttachSeedWork::FinishReplay;
            }
            return AttachSeedWork::ReplayChunk;
        }
        if let Some(key) = seed.pending.front() {
            return AttachSeedWork::BeginPane(*key);
        }
        if seed.catch_up.is_empty() {
            AttachSeedWork::Complete
        } else {
            AttachSeedWork::CatchUp
        }
    }

    fn pump_replay_resize(&mut self, id: ClientId, remaining: &mut usize) -> bool {
        let Some((frame, available)) = self.clients.iter().find_map(|client| {
            let replay = client.seed.as_ref()?.replay.as_ref()?;
            Some((
                Arc::clone(replay.resize_frame.as_ref()?),
                SEED_SEND_WINDOW.saturating_sub(client.outbox_bytes),
            ))
        }) else {
            return true;
        };
        if frame.len() > available || frame.len() > *remaining {
            return false;
        }
        let frame_len = frame.len();
        if !self.queue_attach_frame(
            id,
            frame,
            OutboxClass::SeedReplay,
            "attach-seed-outbox-overflow",
        ) {
            return true;
        }
        self.client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .and_then(|seed| seed.replay.as_mut())
            .expect("replay resize was present")
            .resize_frame = None;
        *remaining -= frame_len;
        true
    }

    fn finish_pane_replay(&mut self, id: ClientId) {
        let seed = self
            .client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .expect("active seed");
        let key = seed.replay.as_ref().expect("replay was present").key;
        seed.replay = None;
        seed.manifest.insert(key, PaneSeedState::CatchingUp);
    }

    fn pump_replay_chunk(&mut self, id: ClientId, remaining: &mut usize) -> bool {
        let Some((frame, payload_len, available)) = self.clients.iter().find_map(|client| {
            let replay = client.seed.as_ref()?.replay.as_ref()?;
            let available = SEED_SEND_WINDOW.saturating_sub(client.outbox_bytes);
            let payload_budget = available
                .min(*remaining)
                .saturating_sub(protocol::PANE_FRAME_OVERHEAD);
            let payload_len = SEED_CHUNK
                .min(payload_budget)
                .min(replay.bytes.len().saturating_sub(replay.offset));
            let frame = encode_pane_output(
                replay.key.pane_id,
                replay.key.generation,
                false,
                &replay.bytes[replay.offset..replay.offset + payload_len],
            );
            Some((frame, payload_len, available))
        }) else {
            return true;
        };
        if available <= protocol::PANE_FRAME_OVERHEAD || payload_len == 0 {
            return false;
        }
        let Some(frame) = frame else {
            self.advance_replay(id, payload_len);
            return true;
        };
        let frame_len = frame.len();
        if !self.queue_attach_frame(
            id,
            frame,
            OutboxClass::SeedReplay,
            "attach-seed-outbox-overflow",
        ) {
            return true;
        }
        self.advance_replay(id, payload_len);
        *remaining = remaining.saturating_sub(frame_len);
        true
    }

    fn advance_replay(&mut self, id: ClientId, bytes: usize) {
        self.client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .and_then(|seed| seed.replay.as_mut())
            .expect("replay was present")
            .offset += bytes;
    }

    fn begin_pane_replay(&mut self, id: ClientId, key: PaneSeedKey) {
        self.client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .expect("active seed")
            .pending
            .pop_front();
        let snapshot = self
            .panes
            .get_mut(&key.pane_id)
            .filter(|pane| pane.generation == key.generation && pane.exited.is_none())
            .map(|pane| {
                let cols = pane.cols;
                let rows = pane.rows;
                let bytes = pane.screen_without_change().export_replay_bytes();
                (bytes, cols, rows)
            });
        let replay_len = snapshot
            .as_ref()
            .map_or(0, |(bytes, _, _)| bytes.len() as u64);
        let seed = self
            .client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .expect("active seed");
        if let Some((bytes, cols, rows)) = snapshot {
            seed.manifest.insert(key, PaneSeedState::Replaying);
            seed.replay = Some(PaneSeedReplay {
                key,
                resize_frame: encode_control(&ServerMessage::Resized {
                    pane_id: key.pane_id,
                    local: false,
                    generation: key.generation,
                    cols,
                    rows,
                }),
                bytes,
                offset: 0,
            });
        } else {
            seed.manifest.insert(key, PaneSeedState::CatchingUp);
        }
        self.attach_seed_totals.replay_bytes += replay_len;
    }

    fn pump_seed_catch_up(&mut self, id: ClientId, remaining: &mut usize) -> bool {
        let Some((frame, available)) = self.clients.iter().find_map(|client| {
            let seed = client.seed.as_ref()?;
            Some((
                Arc::clone(seed.catch_up.front()?),
                SEED_SEND_WINDOW.saturating_sub(client.outbox_bytes),
            ))
        }) else {
            return true;
        };
        if frame.len() > available || frame.len() > *remaining {
            return false;
        }
        let frame_len = frame.len();
        if !self.queue_attach_frame(
            id,
            frame,
            OutboxClass::SeedCatchUp,
            "attach-catch-up-outbox-overflow",
        ) {
            return true;
        }
        let seed = self
            .client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .expect("active seed");
        seed.catch_up.pop_front();
        seed.catch_up_bytes -= frame_len;
        *remaining -= frame_len;
        true
    }

    fn queue_attach_frame(
        &mut self,
        id: ClientId,
        frame: Arc<[u8]>,
        class: OutboxClass,
        overflow_reason: &'static str,
    ) -> bool {
        let max_backlog = self.max_backlog;
        let Some(client) = self.client_mut(id) else {
            return false;
        };
        if client.try_push_class(frame, max_backlog, class) {
            return true;
        }
        if let Some(seed) = client.seed.as_mut() {
            seed.disconnect_reason = Some(overflow_reason);
        }
        self.remove_client(id);
        false
    }

    fn complete_attach_baseline(&mut self, id: ClientId) {
        let seed = self
            .client_mut(id)
            .and_then(|client| client.seed.as_mut())
            .expect("active seed");
        for state in seed.manifest.values_mut() {
            *state = PaneSeedState::Live;
        }
        seed.baseline_queued = true;
    }

    fn note_attach_seed_high_water(&mut self) {
        let queued = self
            .clients
            .iter()
            .map(|client| client.seed_queued_bytes)
            .sum::<usize>();
        let catch_up = self
            .clients
            .iter()
            .map(|client| {
                client.seed_catch_up_queued_bytes
                    + client.seed.as_ref().map_or(0, |seed| seed.catch_up_bytes)
            })
            .sum::<usize>();
        self.attach_seed_totals.peak_queued_bytes =
            self.attach_seed_totals.peak_queued_bytes.max(queued);
        self.attach_seed_totals.peak_catch_up_bytes =
            self.attach_seed_totals.peak_catch_up_bytes.max(catch_up);
    }
}

fn seed_delta_for_control(message: &ServerMessage) -> SeedDelta {
    match message {
        ServerMessage::Resized {
            pane_id,
            local: false,
            generation,
            ..
        } => SeedDelta::PaneResize(PaneSeedKey {
            pane_id: *pane_id,
            generation: *generation,
        }),
        _ => SeedDelta::Control,
    }
}
