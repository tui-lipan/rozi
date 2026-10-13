//! Snapshot transport policy. Protocol records remain owned by the framework terminal.
use super::{ClientId, PaneId, ServerMessage, ServerOutbound, SessionServer, wire_local};
use std::time::{Duration, Instant};
use tui_lipan::ProgramStatusSnapshot;

const COALESCE: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct ProgramStatusSync {
    pub(super) sent: ProgramStatusSnapshot,
    pub(super) pending: bool,
    pub(super) last_sent: Option<Instant>,
}

fn semantic_changed(before: &ProgramStatusSnapshot, after: &ProgramStatusSnapshot) -> bool {
    before.records.len() != after.records.len()
        || before.records.iter().zip(&after.records).any(|(a, b)| {
            a.id != b.id
                || a.state != b.state
                || a.kind != b.kind
                || a.app != b.app
                || a.title != b.title
        })
}

fn effective_changed(before: &ProgramStatusSnapshot, after: &ProgramStatusSnapshot) -> bool {
    crate::session::protocol::program_status_summary(before).map(|r| &r.id)
        != crate::session::protocol::program_status_summary(after).map(|r| &r.id)
        || semantic_changed(before, after)
        || before
            .records
            .iter()
            .zip(&after.records)
            .any(|(a, b)| a.msg != b.msg || a.progress != b.progress)
}

impl SessionServer {
    /// A child can report before the controller's layout commit introduces it to followers.
    /// Repeat its current snapshot after that commit, where every recipient has its generation.
    pub(super) fn program_status_for_new_layout_panes(
        &self,
        layout: &crate::layout::shared::SharedLayout,
    ) -> Vec<(super::Target, ServerMessage)> {
        let known: std::collections::HashSet<_> = self
            .layout
            .iter()
            .flat_map(|layout| &layout.workspaces)
            .flat_map(|workspace| &workspace.panes)
            .map(|pane| (pane.pane_id, pane.generation))
            .collect();
        layout
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panes)
            .filter(|pane| !known.contains(&(pane.pane_id, pane.generation)))
            .filter_map(|identity| {
                let pane = self
                    .panes
                    .get(&identity.pane_id)
                    .filter(|pane| pane.generation == identity.generation)?;
                let snapshot = pane.screen().program_status_snapshot();
                (snapshot.revision > 0).then_some((
                    super::Target::Broadcast,
                    ServerMessage::ProgramStatusChanged {
                        pane_id: identity.pane_id,
                        local: false,
                        generation: identity.generation,
                        snapshot,
                    },
                ))
            })
            .collect()
    }

    pub(super) fn sync_program_status(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
        generation: u64,
        force: bool,
    ) {
        let Some(pane) = self
            .pane_mut(owner, id)
            .filter(|p| p.generation == generation)
        else {
            return;
        };
        let changed = !pane
            .screen_without_change()
            .drain_program_status_events()
            .is_empty();
        if !changed && !pane.program_status_sync.pending {
            return;
        }
        let snapshot = pane.screen().program_status_snapshot();
        let sync = &mut pane.program_status_sync;
        if !effective_changed(&sync.sent, &snapshot) {
            sync.pending = false;
            return;
        }
        if !force
            && !semantic_changed(&sync.sent, &snapshot)
            && sync.last_sent.is_some_and(|time| time.elapsed() < COALESCE)
        {
            sync.pending = true;
            return;
        }
        sync.sent = snapshot.clone();
        sync.pending = false;
        sync.last_sent = Some(Instant::now());
        self.send_outbound(
            owner,
            &ServerOutbound::control(ServerMessage::ProgramStatusChanged {
                pane_id: id,
                local: wire_local(owner),
                generation,
                snapshot,
            }),
        );
    }

    pub(super) fn flush_program_status(&mut self) {
        let mut pending: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, p)| p.program_status_sync.pending)
            .map(|(&id, p)| (None, id, p.generation))
            .collect();
        pending.extend(
            self.local_panes
                .iter()
                .filter(|(_, p)| p.program_status_sync.pending)
                .map(|(&(owner, id), p)| (Some(owner), id, p.generation)),
        );
        for (owner, id, generation) in pending {
            self.sync_program_status(owner, id, generation, false);
        }
    }
}
