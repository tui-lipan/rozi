//! Session-owned, temporary input leases for rendered agent results.
//!
//! All PTYs belong to the server and client input passes through `accept_pane_input`. A lease
//! queues that input and geometry changes through restoration; headless automation gets busy.
use tui_lipan::core::event::{MouseEvent, MouseKind};

use super::connection::PaneResizeRequest;
use super::headless::ReplyTo;
use super::responses::{AgentResponseFence, MAX_RESPONSE_BYTES};
use super::*;
use crate::control::{
    AgentResponse, AgentResponsePayload, AgentResponseSource, AgentWaitResult, ControlResponse,
};

const MAX_QUEUED_INPUT: usize = 1024 * 1024;

pub(super) struct HistoryWait {
    pub consumer: HistoryConsumer,
    pub deadline: Option<Instant>,
    pub fence: AgentResponseFence,
}

/// Who a history read answers.
pub(super) enum HistoryConsumer {
    /// A prompt wait holding its client's reply open.
    Reply {
        key: waits::WaitKey,
        reply: ReplyTo,
        result: Box<AgentWaitResult>,
    },
    /// An agent request, answered through the request store.
    Request {
        id: String,
        reference: protocol::AgentRef,
    },
}

impl HistoryWait {
    fn reference(&self) -> &protocol::AgentRef {
        match &self.consumer {
            HistoryConsumer::Reply { result, .. } => {
                result.reference.as_ref().expect("history has an agent")
            }
            HistoryConsumer::Request { reference, .. } => reference,
        }
    }

    fn client(&self) -> Option<ClientId> {
        match &self.consumer {
            HistoryConsumer::Reply { key, .. } => Some(key.0),
            HistoryConsumer::Request { .. } => None,
        }
    }
}

pub(super) struct PendingHistoryRead {
    wait: HistoryWait,
    /// The screen when the turn resolved, returned when no history can be reconstructed.
    visible: String,
    reader: AltScreenHistoryReader,
    mouse: MouseModeState,
    mutations: Vec<DeferredMutation>,
    input_bytes: usize,
    cancelled: bool,
    error: Option<ControlResponse>,
    action_at: Option<Instant>,
}

enum DeferredMutation {
    Input(DeferredInput),
    Control(ClientId, Box<ClientMessage>),
    Resize(ClientId, PaneResizeRequest),
    DragReply { generation: u64, bytes: Vec<u8> },
}

struct DeferredInput {
    client: ClientId,
    generation: u64,
    bytes: Vec<u8>,
    latency_sensitive: bool,
}

impl PendingHistoryRead {
    fn reference(&self) -> &protocol::AgentRef {
        self.wait.reference()
    }

    fn cancel(&mut self) {
        if !self.cancelled {
            self.cancelled = true;
            self.reader.interrupt(Instant::now());
        }
    }
}

/// The active grid's text: retained history on the normal screen, the viewport on the alternate.
fn bounded_terminal_text(screen: &TerminalScreen) -> String {
    use std::ops::ControlFlow;
    let mut text = String::new();
    let _ = screen.try_for_each_text_line(0, screen.total_text_lines(), |row, line| {
        if row > 0 {
            text.push('\n');
        }
        text.push_str(line);
        if text.len() > MAX_RESPONSE_BYTES {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    text.truncate(text.trim_end().len());
    text
}

fn rendered_payload(
    mut text: String,
    source: AgentResponseSource,
    complete: bool,
) -> AgentResponsePayload {
    let truncated = text.len() > MAX_RESPONSE_BYTES;
    if truncated && source == AgentResponseSource::AlternateScreenHistory {
        // History is read upward from the newest row, so its newest rows are kept.
        let mut start = text.len() - MAX_RESPONSE_BYTES;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
    } else if truncated {
        let mut end = MAX_RESPONSE_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    AgentResponsePayload {
        text,
        turn_id: None,
        source,
        fidelity: crate::control::AgentResponseFidelity::Rendered,
        complete: complete && !truncated,
        truncated,
    }
}

impl SessionServer {
    pub(super) fn handoff_agent_history(&mut self, handoff: &super::responses::AgentHandoff) {
        let (old, new) = (&handoff.old, &handoff.new);
        for read in self.agent_history.values_mut() {
            if read.wait.reference() != old {
                continue;
            }
            handoff.rename_session(&mut read.wait.fence.native_session);
            match &mut read.wait.consumer {
                HistoryConsumer::Request { reference, .. } => *reference = new.clone(),
                HistoryConsumer::Reply { result, .. } => {
                    result.reference = Some(new.clone());
                    if let Some(agent) = &mut result.agent
                        && agent.reference == *old
                    {
                        agent.reference = new.clone();
                    }
                }
            }
        }
    }

    /// Answer `wait` from what `subject`'s turn left on its terminal: the alternate screen's
    /// history through a leased traversal when the pane qualifies, else the visible screen. Without
    /// a subject, `wait` is answered with no terminal text.
    pub(super) fn answer_from_screen(
        &mut self,
        wait: HistoryWait,
        subject: Option<protocol::AgentRef>,
    ) {
        let Some(reference) = subject else {
            return self.complete_history(wait, None, None);
        };
        let Some(pane) = self.panes.get_mut(&reference.pane.pane_id) else {
            return self.complete_history(wait, None, None);
        };
        let screen = pane.screen_without_change();
        let visible = bounded_terminal_text(screen);
        let reader = if screen.is_alternate_screen() && self.may_read_history(&wait, &reference) {
            let screen = self
                .panes
                .get_mut(&reference.pane.pane_id)
                .expect("validated pane")
                .screen_without_change();
            let mouse = screen.render_snapshot().mouse_mode;
            AltScreenHistoryReader::new(screen.history_read_frame(), Instant::now())
                .ok()
                .map(|reader| (reader, mouse))
        } else {
            None
        };
        let Some((reader, mouse)) = reader else {
            let response = self.store_terminal_response(&reference, &wait.fence, visible);
            return self.complete_history(wait, None, response);
        };
        self.agent_history.insert(
            reference.pane.pane_id,
            PendingHistoryRead {
                wait,
                visible,
                reader,
                mouse,
                mutations: Vec::new(),
                input_bytes: 0,
                cancelled: false,
                error: None,
                action_at: None,
            },
        );
    }

    /// Hand a finished read's outcome to whoever it answers.
    fn complete_history(
        &mut self,
        wait: HistoryWait,
        error: Option<ControlResponse>,
        response: Option<AgentResponse>,
    ) {
        match wait.consumer {
            HistoryConsumer::Reply {
                key,
                reply,
                mut result,
            } => {
                if response.is_some() {
                    result.response = response;
                }
                let answer = error.unwrap_or_else(|| ControlResponse::ok(*result));
                self.answer_held(key.0, &reply, answer);
            }
            HistoryConsumer::Request { id, reference } => {
                self.finish_request_history(&reference, &id, error, response);
            }
        }
    }

    /// Whether request `id`'s answer is being read from history.
    pub(super) fn reading_request_history(&self, id: &str) -> bool {
        self.agent_history.values().any(|read| {
            matches!(&read.wait.consumer, HistoryConsumer::Request { id: reading, .. } if reading == id)
        })
    }

    /// Whether the read's consumer still wants an answer: a request answered by its agent in the
    /// meantime does not.
    fn history_wanted(&self, wait: &HistoryWait) -> bool {
        match &wait.consumer {
            HistoryConsumer::Reply { .. } => true,
            HistoryConsumer::Request { id, .. } => self.request_active(id),
        }
    }

    fn may_read_history(&mut self, wait: &HistoryWait, reference: &protocol::AgentRef) -> bool {
        if self.agent_history.contains_key(&reference.pane.pane_id)
            || !self.history_authorized(wait)
        {
            return false;
        }
        // Only a turn that ended idle or done, for an agent the pane itself detected.
        if reference.slot.is_some() || !self.history_subject_idle(reference) {
            return false;
        }
        let Some(pane) = self.panes.get_mut(&reference.pane.pane_id) else {
            return false;
        };
        if pane.pty.is_none()
            || pane.exited.is_some()
            || pane.generation != reference.pane.generation
            // Reconstruction assumes the viewport size is the child's real size.
            || pane.pty_resize_pending
        {
            return false;
        }
        let screen = pane.screen_without_change();
        let snapshot = screen.render_snapshot();
        let eligible = screen.scrollback_offset() == 0
            && snapshot.mouse_mode.mode != MouseMode::None
            && snapshot.images.is_empty();
        eligible && self.history_allowed(reference)
    }

    /// Synthetic wheel input is only ever sent to a recognized agent whose definition has not
    /// opted out.
    fn history_allowed(&self, reference: &protocol::AgentRef) -> bool {
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return false;
        };
        self.agent_runtimes_for(reference.pane.pane_id, pane)
            .into_iter()
            .find(|agent| agent.reference == *reference)
            .and_then(|agent| {
                self.settings
                    .agents
                    .by_id(&agent.identity.id)
                    .map(|d| d.history)
            })
            .is_some_and(|history| history == crate::agent_detection::AgentHistory::MouseScroll)
    }

    fn history_authorized(&self, wait: &HistoryWait) -> bool {
        match &wait.consumer {
            HistoryConsumer::Reply {
                key,
                reply: ReplyTo::Attached { .. },
                ..
            } => self.client_may_input(key.0),
            _ => !self.input_locked,
        }
    }

    /// Store rendered terminal text, which never claims to be a complete response, and return the
    /// turn's response. Nothing is stored for an empty screen.
    fn store_terminal_response(
        &mut self,
        reference: &protocol::AgentRef,
        fence: &AgentResponseFence,
        text: String,
    ) -> Option<AgentResponse> {
        if text.is_empty() {
            return None;
        }
        let content = rendered_payload(text, AgentResponseSource::Terminal, false);
        self.store_rendered(reference, fence, content)
    }

    fn store_rendered(
        &mut self,
        reference: &protocol::AgentRef,
        fence: &AgentResponseFence,
        content: AgentResponsePayload,
    ) -> Option<AgentResponse> {
        // A semantic reply arriving during traversal always takes precedence.
        if self.agent_responses.after(reference, fence).is_none() {
            let native = self.response_native_session(reference);
            self.agent_responses
                .insert(reference.clone(), native, content);
        }
        self.agent_responses.after(reference, fence).cloned()
    }

    /// Poll from the normal server loop. Never block the pump waiting for a TUI redraw.
    pub(super) fn pump_agent_history(&mut self) {
        let panes = self.agent_history.keys().copied().collect::<Vec<_>>();
        for id in panes {
            let mut read = self.agent_history.remove(&id).expect("listed read");
            self.check_history_wait(&mut read);
            let step = self.observe_history(&mut read);
            match step {
                HistoryReadStep::Finished(result) => self.finish_history(read, result),
                HistoryReadStep::Wait => {
                    self.agent_history.insert(id, read);
                }
                action => {
                    if !self.write_history_action(&read, action) {
                        self.finish_history(read, Err(HistoryReadError::RestoreUnproven));
                    } else {
                        read.action_at = Some(Instant::now());
                        self.agent_history.insert(id, read);
                    }
                }
            }
        }
    }

    fn check_history_wait(&self, read: &mut PendingHistoryRead) {
        if let Some(error) = self.history_wait_error(read) {
            read.error = Some(error);
            read.cancel();
        }
        if !self.history_subject_idle(read.reference())
            || !self.history_authorized(&read.wait)
            || !self.history_wanted(&read.wait)
            || self
                .agent_responses
                .after(read.reference(), &read.wait.fence)
                .is_some()
        {
            read.cancel();
        }
    }

    fn history_wait_error(&self, read: &PendingHistoryRead) -> Option<ControlResponse> {
        let reference = read.reference();
        if let Err(error) =
            self.resolve_agent_wait_target(crate::control::AgentTarget::Ref(reference.clone()))
        {
            return Some(error);
        }
        if read.wait.fence.native_session.is_some()
            && self.response_native_session(reference) != read.wait.fence.native_session
        {
            return Some(ControlResponse::error_with(
                crate::control::ControlErrorCode::AgentReplaced,
                "agent changed its native session during history traversal",
            ));
        }
        read.wait
            .deadline
            .filter(|deadline| Instant::now() >= *deadline)
            .map(|_| {
                ControlResponse::error_with(
                    crate::control::ControlErrorCode::Timeout,
                    "timed out reading agent history; viewport restoration attempted",
                )
            })
    }

    fn history_subject_idle(&self, reference: &protocol::AgentRef) -> bool {
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return false;
        };
        self.agent_runtimes_for(reference.pane.pane_id, pane)
            .into_iter()
            .find(|agent| agent.reference == *reference)
            .is_some_and(|agent| {
                matches!(
                    agent.state,
                    protocol::AgentState::Idle | protocol::AgentState::Done
                )
            })
    }

    fn observe_history(&mut self, read: &mut PendingHistoryRead) -> HistoryReadStep {
        let reference = read.reference();
        let Some(pane) = self
            .panes
            .get_mut(&reference.pane.pane_id)
            .filter(|pane| pane.generation == reference.pane.generation && pane.exited.is_none())
        else {
            return HistoryReadStep::Finished(Err(HistoryReadError::RestoreUnproven));
        };
        let screen = pane.screen_without_change();
        let snapshot = screen.render_snapshot();
        // Without an alternate screen or a mouse mode there is no way left to scroll back.
        if !screen.is_alternate_screen() || snapshot.mouse_mode.mode == MouseMode::None {
            return HistoryReadStep::Finished(Err(HistoryReadError::RestoreUnproven));
        }
        // An image or a changed mouse protocol makes the pages untrustworthy, but the viewport
        // still has to be restored: stop reconstructing and let the reader scroll back, using
        // the protocol the application now speaks.
        if !snapshot.images.is_empty() || snapshot.mouse_mode != read.mouse {
            read.mouse = snapshot.mouse_mode;
            read.cancel();
        }
        read.reader
            .observe(screen.history_read_frame(), Instant::now())
    }

    fn write_history_action(&self, read: &PendingHistoryRead, action: HistoryReadStep) -> bool {
        let reference = read.reference();
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return false;
        };
        let (kind, ticks) = match action {
            HistoryReadStep::ScrollUp(ticks) => (MouseKind::ScrollUp, ticks),
            HistoryReadStep::ScrollDown(ticks) => (MouseKind::ScrollDown, ticks),
            _ => return false,
        };
        // Pixel mouse reports share SGR framing, with coordinates measured in pixels.
        let (x, y, encoding) = if read.mouse.encoding == MouseEncoding::SgrPixels {
            (
                (pane.cols / 2).saturating_mul(pane.cell.width),
                (pane.rows / 2).saturating_mul(pane.cell.height),
                MouseEncoding::Sgr,
            )
        } else {
            (pane.cols / 2, pane.rows / 2, read.mouse.encoding)
        };
        let event = MouseEvent {
            x,
            y,
            kind,
            mods: KeyMods::default(),
        };
        let Some(tick) = mouse_event_to_bytes(event, encoding, Default::default()) else {
            return false;
        };
        // All ticks in one write, so the application sees them as one burst.
        let bytes = tick.repeat(usize::from(ticks));
        pane.pty
            .as_ref()
            .is_some_and(|pty| pty.write(&bytes).is_ok())
    }

    fn finish_history(
        &mut self,
        mut read: PendingHistoryRead,
        history: std::result::Result<HistoryRead, HistoryReadError>,
    ) {
        let reference = read.reference().clone();
        let response = match history {
            _ if !self.history_wanted(&read.wait) => None,
            Ok(history) => {
                let content = rendered_payload(
                    history.text,
                    AgentResponseSource::AlternateScreenHistory,
                    history.complete,
                );
                self.store_rendered(&reference, &read.wait.fence, content)
            }
            // A failed request publishes nothing; a traversal that just ended without text
            // still answers with the screen the turn ended on.
            Err(_) if read.error.is_none() => {
                let visible = std::mem::take(&mut read.visible);
                self.store_terminal_response(&reference, &read.wait.fence, visible)
            }
            Err(_) => None,
        };
        self.complete_history(read.wait, read.error, response);
        // Release before replay, and recheck each client's current authority and incarnation.
        for mutation in read.mutations {
            self.replay_history_mutation(reference.pane.pane_id, mutation);
        }
        self.pump_agent_requests();
    }

    fn replay_history_mutation(&mut self, pane_id: PaneId, mutation: DeferredMutation) {
        match mutation {
            DeferredMutation::Input(input) => self.accept_pane_input(
                input.client,
                pane_id,
                false,
                input.generation,
                &input.bytes,
                input.latency_sensitive,
            ),
            DeferredMutation::Control(client, message) => {
                self.process_client_control_now(client, *message);
            }
            DeferredMutation::DragReply { generation, bytes } => {
                self.write_drag_reply(None, pane_id, generation, bytes);
            }
            DeferredMutation::Resize(client, resize) => {
                let replies = self.handle_resize_with(client, resize, |pane| {
                    pane.pty.as_ref().map_or(Ok(()), |pty| {
                        pty.resize_with_cell_size(pane.cols, pane.rows, pane.cell)
                    })
                });
                for (target, message) in replies {
                    self.enqueue(client, target, message);
                }
            }
        }
    }

    /// Client input is deferred through restoration, never interleaved with wheel ticks.
    pub(super) fn defer_history_input(
        &mut self,
        client: ClientId,
        pane: PaneId,
        local: bool,
        generation: u64,
        bytes: &[u8],
        latency_sensitive: bool,
    ) -> bool {
        if local {
            return false;
        }
        let Some(read) = self.agent_history.get_mut(&pane) else {
            return false;
        };
        if read.reference().pane.generation != generation {
            return false;
        }
        if bytes.is_empty() {
            return true;
        }
        read.cancel();
        self.queue_history_mutation(
            pane,
            client,
            bytes.len(),
            DeferredMutation::Input(DeferredInput {
                client,
                generation,
                bytes: bytes.to_vec(),
                latency_sensitive,
            }),
        );
        true
    }

    fn queue_history_mutation(
        &mut self,
        pane: PaneId,
        client: ClientId,
        bytes: usize,
        mutation: DeferredMutation,
    ) {
        let read = self.agent_history.get_mut(&pane).expect("active lease");
        if read.input_bytes.saturating_add(bytes) > MAX_QUEUED_INPUT || read.mutations.len() >= 4096
        {
            self.enqueue(client, Target::Sender, ServerMessage::Error {
                code: "conflict".into(),
                message: "pane is restoring agent history; input queue is full, retry after restoration".into(),
                peer: None,
            });
            return;
        }
        read.input_bytes += bytes;
        read.mutations.push(mutation);
    }

    /// Server-generated replies stay ordered behind history restoration and before the
    /// next owner's input. Cancellation must not be rejected by the user-input queue's capacity limit.
    pub(super) fn write_drag_reply(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    ) {
        if bytes.is_empty() {
            return;
        }
        if owner.is_none()
            && let Some(read) = self.agent_history.get_mut(&pane_id)
            && read.reference().pane.generation == generation
        {
            read.cancel();
            read.mutations
                .push(DeferredMutation::DragReply { generation, bytes });
            return;
        }
        if let Some(pane) = self.live_pane_mut(owner, pane_id, generation)
            && let Some(pty) = &pane.pty
        {
            let _ = pty.write(&bytes);
        }
    }

    pub(super) fn defer_history_drag_and_drop(
        &mut self,
        client: ClientId,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        escape_codes: &str,
    ) -> bool {
        if local {
            return false;
        }
        let Some(read) = self.agent_history.get_mut(&pane_id) else {
            return false;
        };
        if read.reference().pane.generation != generation {
            return false;
        }
        // A refused drop must still release the host's pending conversation.
        if read.input_bytes.saturating_add(escape_codes.len()) > MAX_QUEUED_INPUT
            || read.mutations.len() >= 4096
        {
            self.refuse_drag_and_drop(client, pane_id, local, generation, escape_codes);
            return true;
        }
        read.cancel();
        self.queue_history_mutation(
            pane_id,
            client,
            escape_codes.len(),
            DeferredMutation::Control(
                client,
                Box::new(ClientMessage::DragAndDrop {
                    pane_id,
                    local,
                    generation,
                    escape_codes: escape_codes.to_owned(),
                }),
            ),
        );
        true
    }

    pub(super) fn defer_history_marked_input(
        &mut self,
        client: ClientId,
        message: &ClientMessage,
    ) -> bool {
        let ClientMessage::MarkedInput {
            pane_id,
            local: false,
            generation,
            bytes,
            ..
        } = message
        else {
            return false;
        };
        if !self.client_may_input(client) {
            return false;
        }
        let Some(read) = self.agent_history.get_mut(pane_id) else {
            return false;
        };
        if read.reference().pane.generation != *generation {
            return false;
        }
        read.cancel();
        self.queue_history_mutation(
            *pane_id,
            client,
            bytes.len(),
            DeferredMutation::Control(client, Box::new(message.clone())),
        );
        true
    }

    pub(super) fn defer_history_resize(
        &mut self,
        client: ClientId,
        request: PaneResizeRequest,
    ) -> bool {
        if request.local {
            return false;
        }
        let Some(read) = self.agent_history.get_mut(&request.pane_id) else {
            return false;
        };
        if read.reference().pane.generation != request.generation {
            return false;
        }
        // Compare with where the pane will be once the queue replays, not where it is now:
        // A -> B -> A has to queue both transitions, and a request that changes nothing must not
        // reach the pane at all, or its pending-resize retry would bypass the lease.
        let Some(pane) = self.panes.get(&request.pane_id) else {
            return true;
        };
        let (mut cols, mut rows, mut cell) = (pane.cols, pane.rows, pane.cell);
        for mutation in &read.mutations {
            if let DeferredMutation::Resize(_, queued) = mutation {
                (cols, rows) = (queued.cols.max(1), queued.rows.max(1));
                if let Some(queued_cell) =
                    protocol::wire_cell(queued.cell_width, queued.cell_height)
                {
                    cell = queued_cell;
                }
            }
        }
        let changes = cols != request.cols.max(1)
            || rows != request.rows.max(1)
            || protocol::wire_cell(request.cell_width, request.cell_height)
                .is_some_and(|requested| requested != cell);
        if changes {
            read.cancel();
            self.queue_history_mutation(
                request.pane_id,
                client,
                0,
                DeferredMutation::Resize(client, request),
            );
        }
        true
    }

    pub(super) fn note_history_output(&mut self, pane: PaneId, generation: u64) {
        if let Some(read) = self.agent_history.get_mut(&pane)
            && read.reference().pane.generation == generation
            // Only redraws within the reader's wheel-response window are expected.
            && read.action_at.is_none_or(|action| action.elapsed() >= Duration::from_millis(600))
        {
            read.cancel();
        }
    }

    pub(super) fn cancel_client_history(&mut self, client: ClientId) {
        for read in self
            .agent_history
            .values_mut()
            .filter(|read| read.wait.client() == Some(client))
        {
            read.cancel();
        }
    }

    pub(super) fn holds_agent_history(&self, client: ClientId) -> bool {
        self.agent_history
            .values()
            .any(|read| read.wait.client() == Some(client))
    }
}

/// The agent whose terminal answers a prompted turn that ended with no response. A published
/// conversation qualifies too: a prompt only types into the conversation on screen, and a
/// publisher that sends its own response sends it with the completed row.
pub(super) fn history_subject(result: &AgentWaitResult) -> Option<protocol::AgentRef> {
    let reference = result.reference.as_ref()?;
    let agent = result.agent.as_ref()?;
    (result.response.is_none()
        && matches!(
            agent.state,
            protocol::AgentState::Idle | protocol::AgentState::Done
        ))
    .then(|| reference.clone())
}

#[cfg(all(test, unix))]
mod tests;
