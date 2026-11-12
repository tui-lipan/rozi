//! Drag and drop between the host terminal and the programs in panes.
//!
//! The host terminal sees rozi as one program. [`tui_lipan::dnd::Router`] carries a drag over
//! rozi to the pane under it, in that pane's own coordinates; the pane's answers come back from
//! its session server and go out to the host. A pane whose program does not speak the protocol
//! gets dropped paths and text pasted, as a terminal does.

use tui_lipan::dnd::{Hit, HostEvent};
use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::id::PaneId;
use crate::state::DropPane;

#[derive(Clone, Debug)]
pub enum Msg {
    /// An event from the host terminal.
    Host(HostEvent),
    /// Drag-and-drop escape codes a pane's program wrote, relayed by its session server.
    Pane {
        epoch: u64,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        escape_codes: String,
    },
}

pub(crate) fn handle(ctx: &mut Context<AppRoot>, msg: Msg) -> Update {
    match msg {
        Msg::Host(event) => host_event(ctx, event),
        Msg::Pane {
            epoch,
            pane_id,
            local,
            generation,
            escape_codes,
        } => pane_commands(ctx, epoch, pane_id, local, generation, escape_codes),
    }
}

fn host_event(ctx: &mut Context<AppRoot>, event: HostEvent) -> Update {
    if !ctx.state.config.clipboard.drag_and_drop {
        return Update::none();
    }
    let reset = crate::ops::drag_and_drop::forget_other_attachment(ctx);
    let hit = event.targets.iter().find_map(|target| {
        let pane = crate::ops::drag_and_drop::pane_by_terminal_key(&ctx.state, &target.key)?;
        Some(Hit {
            key: crate::ops::drag_and_drop::drop_pane(&ctx.state, pane),
            position: target.position,
            registration: (&pane.terminal.drag_and_drop).into(),
        })
    });
    let dropped_on = matches!(event.event, tui_lipan::dnd::TerminalEvent::Drop { .. })
        .then(|| hit.as_ref().map(|hit| hit.key))
        .flatten();
    let tagged = tui_lipan::dnd::TaggedEvent {
        client_id: event.client_id,
        event: event.event,
    };
    let outputs = ctx
        .state
        .drag_and_drop
        .router
        .host_event(tagged, |_| hit.clone());
    // What the user dropped on is what they work on next.
    let focused = dropped_on.is_some_and(|target| {
        if target.local {
            if ctx
                .state
                .popup
                .as_ref()
                .is_none_or(|pane| pane.id != target.pane_id)
            {
                ctx.state.scratch.focused_pane = Some(target.pane_id);
            }
        } else {
            crate::ops::focus::focus_pane_anywhere(ctx, target.pane_id);
        }
        if let Some(pane) =
            crate::pane::lifecycle::find_pane_in_namespace(&ctx.state, target.pane_id, target.local)
        {
            let key = pane.keys.terminal.clone();
            ctx.state.sidebar.focused = false;
            ctx.request_focus(key);
        }
        true
    });
    let update = reset.merge(crate::ops::drag_and_drop::perform(ctx, outputs));
    if focused {
        update.merge(Update::full())
    } else {
        update
    }
}

fn pane_commands(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    escape_codes: String,
) -> Update {
    if epoch != ctx.state.runtime_epoch && !local {
        return Update::none();
    }
    let reset = crate::ops::drag_and_drop::forget_other_attachment(ctx);
    let key = DropPane {
        pane_id,
        local,
        generation,
        attachment_epoch: ctx.state.runtime_epoch,
    };
    let Some(pane) = crate::pane::lifecycle::find_pane_in_namespace(&ctx.state, pane_id, local)
        .filter(|pane| pane.pty_generation == generation)
    else {
        return Update::none();
    };
    let registration: tui_lipan::dnd::ChildRegistration = (&pane.terminal.drag_and_drop).into();
    let decoder = ctx.state.drag_and_drop.decoders.entry(key).or_default();
    let commands: Vec<_> = tui_lipan::dnd::OscScanner::default()
        .scan(escape_codes.as_bytes())
        .into_iter()
        .flat_map(|(_, body)| decoder.push_body(&body))
        .collect();
    let mut outputs = Vec::new();
    for command in commands {
        outputs.extend(
            ctx.state
                .drag_and_drop
                .router
                .child_command(&key, &registration, command),
        );
    }
    reset.merge(crate::ops::drag_and_drop::perform(ctx, outputs))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use tui_lipan::TestBackend;
    use tui_lipan::dnd::{
        CommandDecoder, DataTarget, DragPosition, DropOperation, DropOperations, EventDecoder,
        OscScanner, ProgramCommand, TaggedEvent, TerminalEvent,
    };
    use tui_lipan::prelude::encode_paste;

    use super::*;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientMessage, PaneDragAndDrop, PaneDropRegistration};

    struct Harness {
        backend: TestBackend<AppRoot>,
        outbound: Receiver<ClientOutbound>,
        pane: PaneId,
        generation: u64,
        rect: Rect,
    }

    fn harness() -> Harness {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_host_drag_and_drop_supported(true);
        let (client, outbound) = SessionClient::test_channel();
        let (pane, generation, key) = {
            let attachment = backend.state_mut().current_mut();
            attachment.session_client = Some(client);
            let pane = &mut attachment.workspaces[0].panes[0];
            pane.opening = false;
            pane.terminal_active = true;
            (pane.id, pane.pty_generation, pane.keys.terminal.clone())
        };
        backend.render();
        let rect = backend
            .rect_of_key(&key)
            .expect("the pane's terminal is on screen");
        while outbound.try_recv().is_ok() {}
        let _ = backend.take_host_drag_and_drop_output();
        Harness {
            backend,
            outbound,
            pane,
            generation,
            rect,
        }
    }

    impl Harness {
        /// A point inside the pane's terminal, `x` and `y` cells from its corner.
        fn at(&self, x: u16, y: u16) -> DragPosition {
            DragPosition {
                cell_x: u32::from(self.rect.x as u16 + x),
                cell_y: u32::from(self.rect.y as u16 + y),
                pixel_x: 0,
                pixel_y: 0,
            }
        }

        fn host(&mut self, event: TerminalEvent) {
            self.backend
                .host_drag_and_drop(TaggedEvent {
                    client_id: None,
                    event,
                })
                .unwrap();
        }

        /// What rozi sent the host since the last call.
        fn sent_to_host(&mut self) -> Vec<ProgramCommand> {
            let bytes = self.backend.take_host_drag_and_drop_output();
            let mut decoder = CommandDecoder::default();
            OscScanner::default()
                .scan(&bytes)
                .into_iter()
                .flat_map(|(_, body)| decoder.push_body(&body))
                .map(|tagged| tagged.command)
                .collect()
        }

        /// What rozi sent the pane's program, through the session, since the last call.
        fn sent_to_pane(&mut self) -> Vec<(Option<u32>, TerminalEvent)> {
            let mut decoder = EventDecoder::default();
            self.outbound
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::Control(ClientMessage::DragAndDrop {
                        escape_codes, ..
                    }) => Some(escape_codes),
                    _ => None,
                })
                .flat_map(|codes| OscScanner::default().scan(codes.as_bytes()))
                .flat_map(|(_, body)| decoder.push_body(&body))
                .map(|tagged| (tagged.client_id, tagged.event))
                .collect()
        }

        fn pane_says(&mut self, command: ProgramCommand) {
            let mut codes = Vec::new();
            command.encode(Some(5), &mut codes);
            self.backend
                .dispatch(crate::Msg::DragAndDrop(Msg::Pane {
                    epoch: self.backend.state().runtime_epoch,
                    pane_id: self.pane,
                    local: false,
                    generation: self.generation,
                    escape_codes: String::from_utf8(codes).unwrap(),
                }))
                .unwrap();
        }

        fn register(&mut self, registration: PaneDragAndDrop) {
            self.backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .drag_and_drop = registration;
        }
    }

    fn files() -> Vec<std::sync::Arc<str>> {
        vec!["text/uri-list".into(), "text/plain".into()]
    }

    #[test]
    fn a_drop_on_a_plain_pane_pastes_the_dropped_paths() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            let position = h.at(2, 1);
            h.host(TerminalEvent::DragOver {
                position,
                operations: DropOperations::EITHER,
                mimes: Some(files()),
            });
            assert_eq!(
                h.backend
                    .state()
                    .drag_and_drop
                    .hovered
                    .map(|hovered| hovered.pane_id),
                Some(h.pane)
            );
            assert!(h.sent_to_host().contains(&ProgramCommand::DropStatus {
                operation: DropOperation::Copy,
                mimes: Some(files()),
            }));
            h.host(TerminalEvent::Drop {
                position,
                operations: DropOperations::EITHER,
                mimes: files(),
            });
            assert_eq!(h.backend.state().drag_and_drop.hovered, None);
            assert_eq!(
                h.sent_to_host(),
                [ProgramCommand::RequestData(DataTarget::Mime { index: 1 })]
            );
            #[cfg(unix)]
            let (uris, pasted) = (
                b"file:///tmp/my%20photo.png\r\nfile:///tmp/notes.txt\r\n".as_slice(),
                "\"/tmp/my photo.png\" /tmp/notes.txt ",
            );
            #[cfg(windows)]
            let (uris, pasted) = (
                b"file:///C:/tmp/my%20photo.png\r\nfile:///C:/tmp/notes.txt\r\n".as_slice(),
                "\"C:/tmp/my photo.png\" C:/tmp/notes.txt ",
            );
            h.host(TerminalEvent::Data {
                target: DataTarget::Mime { index: 1 },
                kind: None,
                remote: false,
                bytes: uris.to_vec(),
                end: true,
            });
            let modes = h.backend.state().current().workspaces[0].panes[0]
                .terminal
                .snapshot()
                .key_modes;
            let input: Vec<_> = h
                .outbound
                .try_iter()
                .filter_map(|message| match message {
                    ClientOutbound::PaneInput { pane_id, bytes, .. } if pane_id == h.pane => {
                        Some(bytes)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(input, [encode_paste(pasted, modes)]);
            assert_eq!(
                h.sent_to_host(),
                [ProgramCommand::FinishDrop {
                    operation: DropOperation::Copy
                }]
            );
        });
    }

    #[test]
    fn a_program_that_speaks_the_protocol_gets_the_drag_in_its_own_cells() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            h.register(PaneDragAndDrop {
                drops: Some(PaneDropRegistration {
                    client_id: Some(5),
                    ..PaneDropRegistration::default()
                }),
                drags: None,
            });
            h.host(TerminalEvent::DragOver {
                position: h.at(3, 2),
                operations: DropOperations::COPY,
                mimes: Some(files()),
            });
            let delivered = h.sent_to_pane();
            assert!(
                matches!(
                    &delivered[..],
                    [(Some(5), TerminalEvent::DragOver { position, mimes: Some(mimes), .. })]
                        if position.cell_x == 3 && position.cell_y == 2 && *mimes == files()
                ),
                "{delivered:?}"
            );
            assert!(h.sent_to_host().contains(&ProgramCommand::DropStatus {
                operation: DropOperation::None,
                mimes: None,
            }));
            h.pane_says(ProgramCommand::DropStatus {
                operation: DropOperation::Copy,
                mimes: Some(vec!["text/uri-list".into()]),
            });
            assert_eq!(
                h.sent_to_host(),
                [ProgramCommand::DropStatus {
                    operation: DropOperation::Copy,
                    mimes: Some(vec!["text/uri-list".into()]),
                }]
            );
            h.host(TerminalEvent::Drop {
                position: h.at(3, 2),
                operations: DropOperations::COPY,
                mimes: files(),
            });
            assert!(matches!(
                &h.sent_to_pane()[..],
                [(Some(5), TerminalEvent::Drop { .. })]
            ));
            h.pane_says(ProgramCommand::RequestData(DataTarget::Mime { index: 1 }));
            assert_eq!(
                h.sent_to_host(),
                [ProgramCommand::RequestData(DataTarget::Mime { index: 1 })]
            );
            h.pane_says(ProgramCommand::FinishDrop {
                operation: DropOperation::Copy,
            });
            assert_eq!(
                h.sent_to_host(),
                [ProgramCommand::FinishDrop {
                    operation: DropOperation::Copy
                }]
            );
        });
    }

    #[test]
    fn late_drop_data_cannot_cross_an_attachment_switch() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            h.host(TerminalEvent::Drop {
                position: h.at(2, 1),
                operations: DropOperations::COPY,
                mimes: files(),
            });
            h.sent_to_host();
            h.backend.state_mut().runtime_epoch += 1;
            h.host(TerminalEvent::Data {
                target: DataTarget::Mime { index: 1 },
                kind: None,
                remote: false,
                bytes: b"file:///tmp/old-session.txt".to_vec(),
                end: true,
            });
            assert!(h.outbound.try_iter().all(|message| !matches!(
                message,
                ClientOutbound::PaneInput { .. }
                    | ClientOutbound::Control(ClientMessage::DragAndDrop { .. })
            )));
            assert!(h.sent_to_host().contains(&ProgramCommand::FinishDrop {
                operation: DropOperation::None,
            }));
        });
    }

    #[test]
    fn a_shared_drop_keeps_its_namespace_when_a_local_pane_has_the_same_id() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            let local = crate::state::Pane::new_local(h.pane, 100, FloatRect::default());
            h.backend.state_mut().scratch.panes.push(local);
            let pane = &h.backend.state().current().workspaces[0].panes[0];
            assert!(!crate::ops::drag_and_drop::drop_pane(h.backend.state(), pane).local);
            h.host(TerminalEvent::Drop {
                position: h.at(2, 1),
                operations: DropOperations::COPY,
                mimes: vec!["text/plain".into()],
            });
            h.host(TerminalEvent::Data {
                target: DataTarget::Mime { index: 1 },
                kind: None,
                remote: false,
                bytes: b"shared".to_vec(),
                end: true,
            });
            assert!(h.outbound.try_iter().any(|message| matches!(message,
                ClientOutbound::PaneInput { local: false, bytes, .. } if bytes == b"shared"
            )));
            h.register(PaneDragAndDrop {
                drops: Some(PaneDropRegistration {
                    client_id: Some(5),
                    ..Default::default()
                }),
                drags: None,
            });
            h.host(TerminalEvent::DragOver {
                position: h.at(2, 1),
                operations: DropOperations::COPY,
                mimes: Some(files()),
            });
            assert!(h.outbound.try_iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::DragAndDrop { local: false, .. })
            )));
        });
    }

    #[test]
    fn scratch_program_replies_reach_the_host_through_the_private_mailbox() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            let (client, outbound) = SessionClient::test_channel();
            let mut pane = crate::state::Pane::new_local(h.pane, 100, FloatRect::default());
            pane.opening = false;
            pane.terminal_active = true;
            pane.pty_generation = h.generation;
            pane.terminal.drag_and_drop.drops = Some(PaneDropRegistration {
                client_id: Some(5),
                ..Default::default()
            });
            let key = pane.keys.terminal.clone();
            let state = h.backend.state_mut();
            state.scratch.panes.push(pane);
            state.scratch.focused_pane = Some(h.pane);
            state.scratch_visible = true;
            state.scratch_runtime =
                Some(crate::scratchpad::runtime::ScratchRuntime::from_test_client(client));
            h.outbound = outbound;
            h.backend.render();
            h.backend.advance(std::time::Duration::from_secs(1));
            h.backend.render();
            h.rect = h
                .backend
                .rect_of_key(&key)
                .expect("scratch terminal is mounted");
            h.sent_to_host();
            h.host(TerminalEvent::DragOver {
                position: h.at(1, 1),
                operations: DropOperations::COPY,
                mimes: Some(files()),
            });
            let delivered = h.sent_to_pane();
            assert!(
                matches!(&delivered[..], [(Some(5), TerminalEvent::DragOver { .. })]),
                "{delivered:?}"
            );

            h.sent_to_host();
            let reply = |h: &mut Harness, command: ProgramCommand| {
                let mut codes = Vec::new();
                command.encode(Some(5), &mut codes);
                let message = crate::scratchpad::runtime::message_for_frame(
                    h.backend.state().runtime_epoch,
                    crate::session::protocol::Frame::Control(
                        crate::session::protocol::ServerMessage::PaneDragAndDrop {
                            pane_id: h.pane,
                            local: true,
                            generation: h.generation,
                            escape_codes: String::from_utf8(codes).unwrap(),
                        },
                    ),
                )
                .expect("scratch protocol reply reaches the UI");
                h.backend.dispatch(message).unwrap();
            };
            let status = ProgramCommand::DropStatus {
                operation: DropOperation::Copy,
                mimes: None,
            };
            reply(&mut h, status.clone());
            assert_eq!(h.sent_to_host(), [status]);
            h.host(TerminalEvent::Drop {
                position: h.at(1, 1),
                operations: DropOperations::COPY,
                mimes: files(),
            });
            let request = ProgramCommand::RequestData(DataTarget::Mime { index: 1 });
            reply(&mut h, request.clone());
            assert_eq!(h.sent_to_host(), [request]);
        });
    }

    #[test]
    fn a_popup_drop_preserves_the_hidden_scratch_focus() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            let scratch = crate::state::Pane::new_local(7, 100, FloatRect::default());
            h.backend.state_mut().scratch.panes.push(scratch);
            h.backend.state_mut().scratch.focused_pane = Some(7);
            let popup = crate::state::Pane::new_local(
                crate::pane::id::POPUP_PANE_ID,
                100,
                FloatRect::default(),
            );
            let key = popup.keys.terminal.clone();
            h.backend.state_mut().popup = Some(popup);
            h.backend
                .dispatch(crate::Msg::DragAndDrop(Msg::Host(HostEvent {
                    client_id: None,
                    event: TerminalEvent::Drop {
                        position: Default::default(),
                        operations: DropOperations::COPY,
                        mimes: files(),
                    },
                    targets: vec![tui_lipan::dnd::HostTarget {
                        key,
                        rect: h.rect,
                        position: Default::default(),
                    }],
                })))
                .unwrap();
            assert_eq!(h.backend.state().scratch.focused_pane, Some(7));
        });
    }

    #[test]
    fn rozi_registers_with_the_host_and_offers_drags_while_a_pane_does() {
        crate::test_support::on_large_stack(|| {
            let mut h = harness();
            h.backend.dispatch(crate::Msg::RefreshPaintLayers).unwrap();
            let registered = h.sent_to_host();
            assert!(
                matches!(&registered[..], [ProgramCommand::AcceptDrops { mimes }, ..] if mimes.is_empty()),
                "{registered:?}"
            );
            assert!(
                registered[1..]
                    .iter()
                    .all(|command| matches!(command, ProgramCommand::DropMachineId(_))),
                "the machine id follows, where the platform has one"
            );
            h.backend.dispatch(crate::Msg::RefreshPaintLayers).unwrap();
            assert!(h.sent_to_host().is_empty(), "registered once");
            h.register(PaneDragAndDrop {
                drops: None,
                drags: Some(Default::default()),
            });
            h.backend.dispatch(crate::Msg::RefreshPaintLayers).unwrap();
            assert!(matches!(
                &h.sent_to_host()[..],
                [ProgramCommand::OfferDrags { .. }]
            ));
            h.register(PaneDragAndDrop::default());
            h.backend.state_mut().config.clipboard.drag_and_drop = false;
            h.backend.dispatch(crate::Msg::RefreshPaintLayers).unwrap();
            assert_eq!(
                h.sent_to_host(),
                [
                    ProgramCommand::StopAcceptingDrops,
                    ProgramCommand::StopOfferingDrags
                ]
            );
        });
    }
}
