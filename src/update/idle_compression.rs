//! The client's history compression timer: passive maintenance that never requests a frame.

use std::time::Instant;

use tui_lipan::prelude::*;

use crate::pane::idle_compression::POLL_INTERVAL;
use crate::{AppRoot, Msg};

pub(super) fn tick(ctx: &mut Context<AppRoot>) -> Update {
    compress_state(&mut ctx.state);
    Update::command_only(Command::after(POLL_INTERVAL, |link| {
        link.send(Msg::IdleCompressionTick);
    }))
}

fn compress_state(state: &mut crate::state::State) {
    let now = Instant::now();
    let attachments = std::iter::once(&mut state.attachment).chain(state.background.values_mut());
    let mut panes: Vec<_> = attachments
        .flat_map(|attachment| &mut attachment.workspaces)
        .flat_map(|workspace| &mut workspace.panes)
        .chain(&mut state.scratch.panes)
        .chain(state.popup.iter_mut())
        .collect();
    crate::pane::idle_compression::poll(&mut panes, &mut state.idle_compression_cursor, |pane| {
        pane.terminal.poll_idle_compression(now);
    });
}
