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
    if ctx
        .state
        .popup
        .as_ref()
        .is_some_and(|pane| matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_)))
        && crate::popup::dismisses_completed(key)
    {
        return (true, crate::popup::close(ctx));
    }

    // The sidebar's row list and file tree are ordinary focusable widgets, so they consume their
    // own movement keys and only what they ignore reaches here. `FileTree` has no key-handler prop,
    // so claiming these at the root is also the one place that covers both widgets identically.
    if ctx.state.sidebar.focused
        && let Some(update) = handle_sidebar_key(ctx, key)
    {
        return (true, update);
    }

    match ctx.state.mode {
        Mode::Normal => {
            if let Some(id) = source_pane {
                return (true, crate::pty_events::forward_key_to_pane(ctx, id, key));
            }
            (false, Update::none())
        }
        Mode::Resize => handle_resize_mode_key(ctx, key),
        Mode::Copy => {
            // While a copy-mode `/` search overlay is open, let the focused search input handle
            // keys instead of consuming them in handle_copy_key.
            if ctx.state.search.is_some() {
                (false, Update::none())
            } else {
                crate::copy_mode::handle_copy_key(ctx, key)
            }
        }
        Mode::Hint => crate::hints::handle_hint_key(ctx, key),
    }
}

/// Keys hyprmux claims while the sidebar body has focus.
///
/// The file tree is a widget that navigates itself, so only the tab-level keys are taken there; the
/// composed row lists have no widget behind them, so hyprmux owns their cursor too. `PAGE_ROWS` is a
/// fixed step rather than a viewport-derived one — the row list is short and the view follows the
/// cursor anyway, so measuring the viewport buys nothing.
const PAGE_ROWS: isize = 5;

fn handle_sidebar_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> Option<Update> {
    use crate::update::sidebar;
    match key.code {
        KeyCode::Esc => return Some(sidebar::blur_body(ctx)),
        // Tab cycles sidebar tabs rather than the focus ring: the sidebar is deliberately outside
        // that ring, and this leaves the arrow keys free for the tree's expand/collapse.
        KeyCode::Tab if !key.mods.shift => return Some(sidebar::cycle_tab(ctx, true)),
        KeyCode::BackTab | KeyCode::Tab => return Some(sidebar::cycle_tab(ctx, false)),
        _ => {}
    }
    if tree_tab_active(ctx) {
        return None;
    }
    // `j`/`k` alongside the arrows, matching resize and copy mode — and matching the file tree,
    // whose widget keymap has included the vim keys all along.
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(sidebar::move_cursor(ctx, -1)),
        KeyCode::Down | KeyCode::Char('j') => Some(sidebar::move_cursor(ctx, 1)),
        KeyCode::PageUp => Some(sidebar::move_cursor(ctx, -PAGE_ROWS)),
        KeyCode::PageDown => Some(sidebar::move_cursor(ctx, PAGE_ROWS)),
        KeyCode::Home | KeyCode::Char('g') => Some(sidebar::move_cursor(ctx, isize::MIN)),
        KeyCode::End | KeyCode::Char('G') => Some(sidebar::move_cursor(ctx, isize::MAX)),
        KeyCode::Enter => Some(sidebar::activate_cursor(ctx)),
        _ => None,
    }
}

/// The file tree owns its own keyboard navigation; the composed row lists do not.
fn tree_tab_active(ctx: &Context<HyprmuxApp>) -> bool {
    matches!(
        crate::view::sidebar::active_tab(ctx),
        Some(crate::config::SidebarTab::Tree { .. })
    )
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
