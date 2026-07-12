use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::ops::focus::{focus_pane, request_current_pane_focus};
use crate::ops::resize_move::resize_focused_in_direction;
use crate::state::{Direction, Mode, PaneId};
use crate::view;

/// Route a key that reached hyprmux's own handling: app command chords (leader/modifier) are
/// resolved natively by tui-lipan before this runs (see `App::key_dispatch_policy` /
/// `terminal_key_policy` in `main.rs`), so by the time a key gets here it is either a
/// `Resize`/`Copy` mode key or plain PTY input.
pub(crate) fn handle_key_routing(
    ctx: &mut Context<HyprmuxApp>,
    key: KeyEvent,
    source_pane: Option<PaneId>,
) -> (bool, Update) {
    match ctx.state.mode {
        Mode::Normal => {
            if let Some(id) = source_pane {
                return (true, crate::pty_events::forward_key_to_pane(ctx, id, key));
            }
            (false, Update::none())
        }
        Mode::Resize => handle_resize_mode_key(ctx, key),
        Mode::Copy => crate::copy_mode::handle_copy_key(ctx, key),
        Mode::Hint => crate::hints::handle_hint_key(ctx, key),
    }
}

fn handle_resize_mode_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Esc) || key.is(KeyCode::Enter) {
        ctx.state.mode = Mode::Normal;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
        return (true, Update::full());
    }

    let direction = match key.code {
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => Some(Direction::Left),
        KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => Some(Direction::Down),
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => Some(Direction::Up),
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => Some(Direction::Right),
        _ => None,
    };

    if let Some(direction) = direction {
        resize_focused_in_direction(ctx, direction);
        return (true, Update::full());
    }

    (true, Update::none())
}

fn framework_focused_pane(ctx: &Context<HyprmuxApp>) -> Option<PaneId> {
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .find(|pane| ctx.has_focus_within_key(view::pane_window_key(pane.id)))
        .map(|pane| pane.id)
}

pub(crate) fn sync_focus_from_framework(ctx: &mut Context<HyprmuxApp>) {
    if let Some(id) = ctx.state.focused_pane
        && ctx.state.workspaces[ctx.state.active_workspace]
            .panes
            .iter()
            .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
    {
        return;
    }

    let framework_focus = framework_focused_pane(ctx);
    if let Some(id) = framework_focus {
        focus_pane(&mut ctx.state, id);
    }
}
