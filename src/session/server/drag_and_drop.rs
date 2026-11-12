//! A pane's drag-and-drop conversation (OSC 72), between its program and one client.
//!
//! The pane's screen answers the program's support query and keeps its registration; the
//! registration reaches every client in the pane's runtime state. A drag itself happens on one
//! client, the one whose host terminal the user is dragging over, so the program's answers go to
//! that client only: the last one to write drag-and-drop escape codes to the pane. The server
//! follows the conversation closely enough to end it for the program if that client goes away,
//! and answers for nobody when no client owns the pane, as the protocol requires.

use tui_lipan::dnd::{
    DndError, ErrorCode, EventDecoder, ProgramCommand, TaggedCommand, TerminalEvent,
};

use super::{ClientId, PaneId, ServerMessage, SessionServer, Target, wire_local};

/// Who a pane's drag belongs to, and how far it has got.
#[derive(Debug, Default)]
pub(super) struct PaneDragAndDrop {
    owner: Option<ClientId>,
    /// Reads what the owner wrote, to follow the drag.
    written: EventDecoder,
    /// A drag is over the pane.
    hovering: bool,
    /// The pane took a drop and has not said it is done reading it.
    dropped: bool,
    /// The pane's program offered a drag the host has not finished.
    dragging: bool,
}

impl PaneDragAndDrop {
    fn busy(&self) -> bool {
        self.hovering || self.dropped || self.dragging
    }

    fn observe_written(&mut self, escape_codes: &[u8]) {
        for (_, body) in tui_lipan::dnd::OscScanner::default().scan(escape_codes) {
            for tagged in self.written.push_body(&body) {
                match tagged.event {
                    TerminalEvent::DragOver { .. } => self.hovering = true,
                    TerminalEvent::DragLeave => {
                        self.hovering = false;
                        self.dropped = false;
                    }
                    TerminalEvent::Drop { .. } => {
                        self.hovering = false;
                        self.dropped = true;
                    }
                    TerminalEvent::DragFailed(_) | TerminalEvent::DragFinished { .. } => {
                        self.dragging = false;
                    }
                    _ => {}
                }
            }
        }
    }

    fn observe_command(&mut self, command: &ProgramCommand) {
        match command {
            ProgramCommand::FinishDrop { .. } => self.dropped = false,
            ProgramCommand::StartDrag => self.dragging = true,
            ProgramCommand::CancelDrag | ProgramCommand::DragError(_) => self.dragging = false,
            _ => {}
        }
    }

    /// What tells the program its drag is over, when its owner has gone mid-way.
    fn abandon(&mut self, drop_id: Option<u32>, drag_id: Option<u32>) -> Vec<u8> {
        let mut bytes = Vec::new();
        if self.hovering || self.dropped {
            TerminalEvent::DragLeave.encode(drop_id, &mut bytes);
        }
        if self.dragging {
            TerminalEvent::DragFinished { canceled: true }.encode(drag_id, &mut bytes);
        }
        *self = Self::default();
        bytes
    }
}

/// A command from a program that no client owns, answered as the protocol asks of a terminal.
fn unowned_reply(command: &TaggedCommand) -> Option<TerminalEvent> {
    let refused = |why: &str| DndError::with_description(ErrorCode::PermissionDenied, why);
    Some(match &command.command {
        ProgramCommand::RequestData(target) => TerminalEvent::DataError {
            target: Some(*target),
            error: refused("drop data can only be requested after a drop"),
        },
        ProgramCommand::Offer { .. } | ProgramCommand::StartDrag => {
            TerminalEvent::DragFailed(refused("no drag gesture to answer"))
        }
        _ => return None,
    })
}

impl SessionServer {
    /// Write a client's drag-and-drop escape codes to a pane, making the client its drag's owner.
    pub(super) fn accept_drag_and_drop(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        escape_codes: String,
    ) {
        if !self.may_drag_and_drop(client_id, pane_id, local, generation) {
            self.refuse_drag_and_drop(client_id, pane_id, local, generation, &escape_codes);
            return;
        }
        if self.defer_history_drag_and_drop(client_id, pane_id, local, generation, &escape_codes) {
            return;
        }
        let bytes = escape_codes.into_bytes();
        let owner = local.then_some(client_id);
        let Some(pane) = self.live_pane_mut(owner, pane_id, generation) else {
            return;
        };
        if pane.drag_and_drop.owner != Some(client_id) {
            pane.drag_and_drop = PaneDragAndDrop {
                owner: Some(client_id),
                ..PaneDragAndDrop::default()
            };
        }
        pane.drag_and_drop.observe_written(&bytes);
        if let Some(pty) = &pane.pty {
            let _ = pty.write(&bytes);
        }
    }

    fn may_drag_and_drop(
        &self,
        client: ClientId,
        pane_id: PaneId,
        local: bool,
        generation: u64,
    ) -> bool {
        let authorized = if local {
            self.client_attached(client) && !self.client_read_only(client)
        } else {
            self.client_may_input(client)
        };
        let Some(pane) = self.pane(local.then_some(client), pane_id) else {
            return false;
        };
        authorized
            && pane.generation == generation
            && pane.exited.is_none()
            && !self.drag_owned_by_other_client(&pane.drag_and_drop, client)
    }

    fn drag_owned_by_other_client(&self, drag: &PaneDragAndDrop, client: ClientId) -> bool {
        drag.owner.is_some_and(|current| {
            current != client && drag.busy() && self.client_attached(current)
        })
    }

    /// Reply to the requesting router, without disturbing another client's conversation.
    pub(super) fn refuse_drag_and_drop(
        &mut self,
        client: ClientId,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        escape_codes: &str,
    ) {
        let mut decoder = EventDecoder::default();
        let mut replies = Vec::new();
        for (_, body) in tui_lipan::dnd::OscScanner::default().scan(escape_codes.as_bytes()) {
            for tagged in decoder.push_body(&body) {
                let command = match tagged.event {
                    TerminalEvent::Drop { .. } => ProgramCommand::FinishDrop {
                        operation: tui_lipan::dnd::DropOperation::None,
                    },
                    TerminalEvent::DragOver { .. } => ProgramCommand::DropStatus {
                        operation: tui_lipan::dnd::DropOperation::None,
                        mimes: None,
                    },
                    _ => continue,
                };
                command.encode(tagged.client_id, &mut replies);
            }
        }
        if replies.is_empty() {
            return;
        }
        self.enqueue(
            client,
            Target::Client(client),
            ServerMessage::PaneDragAndDrop {
                pane_id,
                local,
                generation,
                escape_codes: String::from_utf8(replies).expect("OSC 72 commands are ASCII"),
            },
        );
    }

    /// Send what a pane's program said about drag and drop to its drag's owner, after its output.
    pub(super) fn route_drag_and_drop(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
    ) {
        let attached: Vec<ClientId> = self
            .clients
            .iter()
            .filter(|client| client.attached)
            .map(|client| client.id)
            .collect();
        let Some(pane) = self
            .pane_mut(owner, pane_id)
            .filter(|pane| pane.generation == generation)
        else {
            return;
        };
        let commands = pane.screen_without_change().drain_drag_and_drop_commands();
        if commands.is_empty() {
            return;
        }
        let session_owner = pane
            .drag_and_drop
            .owner
            .filter(|client| attached.contains(client));
        let Some(client) = session_owner else {
            let mut replies = Vec::new();
            for command in &commands {
                if let Some(reply) = unowned_reply(command) {
                    reply.encode(command.client_id, &mut replies);
                }
            }
            self.write_drag_reply(owner, pane_id, generation, replies);
            return;
        };
        let mut escape_codes = Vec::new();
        for command in &commands {
            pane.drag_and_drop.observe_command(&command.command);
            command.command.encode(command.client_id, &mut escape_codes);
        }
        let Ok(escape_codes) = String::from_utf8(escape_codes) else {
            return;
        };
        self.enqueue(
            client,
            Target::Client(client),
            ServerMessage::PaneDragAndDrop {
                pane_id,
                local: wire_local(owner),
                generation,
                escape_codes,
            },
        );
    }

    /// Input-lock and controller changes can revoke a connected owner's authority mid-drop.
    /// End both sides now, so neither the host nor another collaborator waits on that owner.
    pub(super) fn cancel_unauthorized_drags(&mut self) {
        let revoked: Vec<_> = self
            .panes
            .iter()
            .filter_map(|(&id, pane)| {
                let client = pane.drag_and_drop.owner?;
                (!self.client_may_input(client)).then_some((id, client))
            })
            .collect();
        for (pane_id, client) in revoked {
            self.cancel_pane_drag(pane_id, client);
        }
    }

    fn cancel_pane_drag(&mut self, pane_id: PaneId, client: ClientId) {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        let generation = pane.generation;
        let registration = pane.screen().drag_and_drop_registration().clone();
        let drop_id = registration.drops.as_ref().and_then(|r| r.client_id);
        let drag_id = registration.drags.as_ref().and_then(|r| r.client_id);
        let bytes = pane.drag_and_drop.abandon(drop_id, drag_id);
        // These commands only affect a conversation this client's router still owns.
        // CancelDrag also clears an offered drag that has not reached StartDrag yet.
        let mut replies = Vec::new();
        for (command, tag) in [
            (
                ProgramCommand::DropStatus {
                    operation: tui_lipan::dnd::DropOperation::None,
                    mimes: None,
                },
                drop_id,
            ),
            (
                ProgramCommand::FinishDrop {
                    operation: tui_lipan::dnd::DropOperation::None,
                },
                drop_id,
            ),
            (ProgramCommand::CancelDrag, drag_id),
        ] {
            command.encode(tag, &mut replies);
        }
        self.enqueue(
            client,
            Target::Client(client),
            ServerMessage::PaneDragAndDrop {
                pane_id,
                local: false,
                generation,
                escape_codes: String::from_utf8(replies).expect("OSC 72 commands are ASCII"),
            },
        );
        self.write_drag_reply(None, pane_id, generation, bytes);
    }

    /// A client has gone: end every drag it owned, so no program waits on it forever.
    pub(super) fn drop_client_drags(&mut self, client_id: ClientId) {
        let panes = self
            .panes
            .values_mut()
            .chain(self.local_panes.values_mut())
            .filter(|pane| pane.drag_and_drop.owner == Some(client_id));
        for pane in panes {
            let registration = pane.screen().drag_and_drop_registration().clone();
            let drop_id = registration
                .drops
                .as_ref()
                .and_then(|drops| drops.client_id);
            let drag_id = registration
                .drags
                .as_ref()
                .and_then(|drags| drags.client_id);
            let bytes = pane.drag_and_drop.abandon(drop_id, drag_id);
            if !bytes.is_empty()
                && let Some(pty) = &pane.pty
            {
                let _ = pty.write(&bytes);
            }
        }
    }

    /// The pane's program has gone: forget what it registered and whose drag it was in.
    pub(super) fn forget_pane_drag_and_drop(pane: &mut super::ServerPane) {
        pane.screen_without_change()
            .clear_drag_and_drop_registration();
        pane.drag_and_drop = PaneDragAndDrop::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::dnd::{DataTarget, DragPosition, DropOperations};

    fn written(events: &[TerminalEvent]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in events {
            event.encode(Some(4), &mut bytes);
        }
        bytes
    }

    #[test]
    fn the_drag_is_followed_from_what_the_owner_writes_and_the_program_answers() {
        let mut state = PaneDragAndDrop::default();
        let position = DragPosition::default();
        state.observe_written(&written(&[TerminalEvent::DragOver {
            position,
            operations: DropOperations::COPY,
            mimes: Some(vec!["text/plain".into()]),
        }]));
        assert!(state.busy());
        state.observe_written(&written(&[TerminalEvent::Drop {
            position,
            operations: DropOperations::COPY,
            mimes: vec!["text/plain".into()],
        }]));
        assert!(state.dropped && !state.hovering);
        state.observe_command(&ProgramCommand::FinishDrop {
            operation: tui_lipan::dnd::DropOperation::Copy,
        });
        assert!(!state.busy());

        state.observe_command(&ProgramCommand::StartDrag);
        assert!(state.busy());
        state.observe_written(&written(&[TerminalEvent::DragFinished { canceled: false }]));
        assert!(!state.busy());
    }

    #[test]
    fn a_lost_owner_leaves_the_program_told_the_drag_is_over() {
        let mut state = PaneDragAndDrop {
            dropped: true,
            dragging: true,
            ..PaneDragAndDrop::default()
        };
        let bytes = state.abandon(Some(4), Some(7));
        let mut decoder = EventDecoder::default();
        let events: Vec<_> = tui_lipan::dnd::OscScanner::default()
            .scan(&bytes)
            .into_iter()
            .flat_map(|(_, body)| decoder.push_body(&body))
            .map(|tagged| (tagged.client_id, tagged.event))
            .collect();
        assert_eq!(
            events,
            [
                (Some(4), TerminalEvent::DragLeave),
                (Some(7), TerminalEvent::DragFinished { canceled: true }),
            ]
        );
        assert!(!state.busy());
        assert!(state.abandon(None, None).is_empty(), "nothing left to end");
    }

    #[cfg(unix)]
    #[test]
    fn authority_loss_cancels_drags_and_late_cleanup_cannot_touch_the_new_owner() {
        use super::super::tests::{attach_client, test_pane};
        use crate::session::protocol::{ClientMessage, ControllerChangeReason};
        let hover = TerminalEvent::DragOver {
            position: Default::default(),
            operations: DropOperations::COPY,
            mimes: None,
        };
        for cleanup in [
            TerminalEvent::DragLeave,
            TerminalEvent::DragFinished { canceled: true },
            TerminalEvent::DragFailed(DndError::with_description(
                ErrorCode::PermissionDenied,
                "canceled",
            )),
        ] {
            let mut server = SessionServer::new_named("dnd-cleanup");
            let (first, _a) = attach_client(&mut server);
            let (second, _b) = attach_client(&mut server);
            server.panes.insert(1, test_pane(1));
            server.assign_controller(first, ControllerChangeReason::Granted);
            server.accept_drag_and_drop(
                first,
                1,
                false,
                1,
                String::from_utf8(written(std::slice::from_ref(&hover))).unwrap(),
            );
            if !matches!(cleanup, TerminalEvent::DragLeave) {
                let drag = &mut server.panes.get_mut(&1).unwrap().drag_and_drop;
                drag.hovering = false;
                drag.observe_command(&ProgramCommand::StartDrag);
            }
            server.handle_message(first, ClientMessage::SetInputLock { locked: true });
            server.handle_message(second, ClientMessage::RequestControl);
            assert!(!server.client_may_input(first));
            assert!(server.client_may_input(second));
            assert!(!server.panes[&1].drag_and_drop.busy());
            assert_eq!(server.panes[&1].drag_and_drop.owner, None);
            server.accept_drag_and_drop(
                second,
                1,
                false,
                1,
                String::from_utf8(written(std::slice::from_ref(&hover))).unwrap(),
            );
            assert_eq!(server.panes[&1].drag_and_drop.owner, Some(second));
            server.accept_drag_and_drop(
                first,
                1,
                false,
                1,
                String::from_utf8(written(&[cleanup])).unwrap(),
            );
            assert!(server.panes[&1].drag_and_drop.busy());
            assert_eq!(server.panes[&1].drag_and_drop.owner, Some(second));
        }
    }

    #[test]
    fn an_unowned_program_is_refused_what_the_protocol_refuses() {
        let request = TaggedCommand {
            client_id: Some(2),
            command: ProgramCommand::RequestData(DataTarget::Mime { index: 1 }),
        };
        assert!(matches!(
            unowned_reply(&request),
            Some(TerminalEvent::DataError { error, .. }) if error.code == ErrorCode::PermissionDenied
        ));
        let status = TaggedCommand {
            client_id: None,
            command: ProgramCommand::DropStatus {
                operation: tui_lipan::dnd::DropOperation::Copy,
                mimes: None,
            },
        };
        assert_eq!(unowned_reply(&status), None);
    }
}
