use tui_lipan::prelude::*;

use crate::pty_events::{
    handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
    handle_prune_closed, handle_pty_event, handle_pty_ready,
};
use crate::{HyprmuxApp, Msg};

pub(crate) fn handle_msg(_app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update {
    match msg {
        Msg::PruneClosed(id) => handle_prune_closed(ctx, id),
        Msg::PtyReady(id, pty) => handle_pty_ready(ctx, id, pty),
        Msg::PtyEvent(id, event) => handle_pty_event(ctx, id, event),
        Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
        Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
        Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
        Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
        _ => unreachable!("update::handle_msg is wired after operation modules are extracted"),
    }
}
