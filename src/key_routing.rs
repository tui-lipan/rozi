use tui_lipan::prelude::*;

use crate::focus_ops::{focus_pane, request_current_pane_focus};
use crate::input;
use crate::resize_move_ops::resize_focused_in_direction;
use crate::state::{Direction, Mode, PaneId};
use crate::view;
use crate::{HyprmuxApp, execute_action};

pub(crate) fn handle_key_routing(
    ctx: &mut Context<HyprmuxApp>,
    key: KeyEvent,
    source_pane: Option<PaneId>,
) -> (bool, Update) {
    match ctx.state.mode {
        Mode::Normal => {
            if input::is_prefix_key(key, ctx.state.config.input) {
                ctx.state.mode = Mode::Prefix;
                return (true, Update::full());
            }

            if let Some(action) = input::action_for_held(key, ctx.state.config.input) {
                return (true, execute_action(ctx, action));
            }

            if let Some(id) = source_pane {
                return (true, crate::pty_events::forward_key_to_pane(ctx, id, key));
            }

            (false, Update::none())
        }
        Mode::Prefix => {
            ctx.state.mode = Mode::Normal;
            if input::is_prefix_key(key, ctx.state.config.input) {
                let id = source_pane.or(ctx.state.focused_pane);
                let update = id
                    .map(|id| crate::pty_events::forward_key_to_pane(ctx, id, key))
                    .unwrap_or_else(Update::none);
                return (true, update);
            }

            if key.is(KeyCode::Esc) {
                return (true, Update::full());
            }

            if let Some(action) = input::action_for_prefix(key) {
                return (true, execute_action(ctx, action));
            }

            let id = source_pane.or(ctx.state.focused_pane);
            let update = id
                .map(|id| crate::pty_events::forward_key_to_pane(ctx, id, key))
                .unwrap_or_else(Update::none);
            (true, update)
        }
        Mode::Resize => handle_resize_mode_key(ctx, key),
    }
}

pub(crate) fn handle_resize_mode_key(
    ctx: &mut Context<HyprmuxApp>,
    key: KeyEvent,
) -> (bool, Update) {
    if key.is(KeyCode::Esc) || key.is(KeyCode::Enter) {
        ctx.state.mode = Mode::Normal;
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

pub(crate) fn framework_focused_pane(ctx: &Context<HyprmuxApp>) -> Option<PaneId> {
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .find(|pane| ctx.has_focus_within_key(view::pane_window_key(pane.id)))
        .map(|pane| pane.id)
}

pub(crate) fn sync_focus_from_framework(ctx: &mut Context<HyprmuxApp>) {
    let framework_focus = framework_focused_pane(ctx);
    if let Some(id) = framework_focus {
        focus_pane(&mut ctx.state, id);
    }
}
