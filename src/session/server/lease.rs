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
        self.dirty = true;
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
        if self.origin_seed_client == Some(id) {
            self.origin_seed_client = None;
        }
        let Some(index) = self.clients.iter().position(|client| client.id == id) else {
            return;
        };
        let removed = self.clients.remove(index);
        if !removed.attached {
            return;
        }
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

    pub(super) fn heartbeat(&mut self) {
        let now = Instant::now();
        let mut timed_out: Vec<ClientId> = Vec::new();
        let mut pings: Vec<(ClientId, u64)> = Vec::new();
        for client in &mut self.clients {
            if !client.attached {
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

    pub(super) fn flush_clients(&mut self) {
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
            }
            if disconnect {
                dead.push(client.id);
            }
        }
        for id in dead {
            self.remove_client(id);
        }
    }

    pub(super) fn enqueue(&mut self, sender_id: ClientId, target: Target, message: ServerMessage) {
        let Some(bytes) = encode_control(&message) else {
            return;
        };
        let max_backlog = self.max_backlog;
        match target {
            Target::Sender => {
                let accepted = self
                    .client_mut(sender_id)
                    .is_some_and(|client| client.try_push(bytes, max_backlog));
                if !accepted {
                    self.remove_client(sender_id);
                }
            }
            Target::Client(id) => {
                let accepted = self
                    .client_mut(id)
                    .filter(|client| client.attached)
                    .is_none_or(|client| client.try_push(bytes, max_backlog));
                if !accepted {
                    self.remove_client(id);
                }
            }
            Target::Broadcast => {
                self.push_to_attached(bytes);
            }
        }
        self.note_outbox_high_water();
    }

    /// Queue one shared encoded frame on every attached client. Each client retains its own write
    /// offset, while the immutable payload allocation is shared.
    pub(super) fn push_to_attached(&mut self, bytes: Arc<[u8]>) {
        let mut slow = Vec::new();
        for client in &mut self.clients {
            if !client.attached {
                continue;
            }
            if !client.try_push(Arc::clone(&bytes), self.max_backlog) {
                slow.push(client.id);
            }
        }
        self.note_outbox_high_water();
        for id in slow {
            self.remove_client(id);
        }
    }

    pub(super) fn broadcast_control(&mut self, message: &ServerMessage) {
        let Some(bytes) = encode_control(message) else {
            return;
        };
        self.push_to_attached(bytes);
    }

    pub(super) fn broadcast_outbound(&mut self, outbound: &ServerOutbound) {
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
    pub(super) fn enqueue_attach_seeds(&mut self, id: ClientId) {
        if let Some(client) = self.client_mut(id) {
            client.seeding = true;
        }
        let panes: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.exited.is_none())
            .map(|(pane_id, pane)| (*pane_id, pane.generation))
            .collect();
        for (pane_id, generation) in panes {
            let bytes = self
                .panes
                .get_mut(&pane_id)
                .expect("pane id came from map")
                .screen
                .export_replay_bytes();
            for chunk in bytes.chunks(SEED_CHUNK) {
                let Some(frame) = encode_pane_output(pane_id, generation, chunk) else {
                    continue;
                };
                let max_backlog = self.max_backlog;
                let accepted = self
                    .client_mut(id)
                    .is_some_and(|client| client.try_push(frame, max_backlog));
                if !accepted {
                    self.remove_client(id);
                    return;
                }
                self.note_outbox_high_water();
            }
        }
    }
}
