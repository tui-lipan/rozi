use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::input::Action;
use crate::ops::focus::{focus_pane, request_current_pane_focus};
use crate::ops::resize_move::resize_focused_in_direction;
use crate::state::{Direction, Mode, PaneId, State};
use crate::view;

/// Route a key that reached rozi's own handling: app command chords (leader/modifier) are
/// resolved natively by tui-lipan before this runs (see `App::key_dispatch_policy` /
/// `terminal_key_policy` in `main.rs`), so by the time a key gets here it is either a
/// `Resize`/`Copy` mode key or plain PTY input.
pub(crate) fn handle_key_routing(
    ctx: &mut Context<AppRoot>,
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
            if launcher_start_key(&ctx.state, key) {
                return (true, crate::actions::execute_action(ctx, Action::Spawn));
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

/// Whether a bare `Enter` should start a shell: only in the launcher, where no pane owns the
/// keyboard and the panel advertises exactly one thing to do. The configured `spawn` chord stays
/// bound everywhere and is unaffected; this only gives the launcher's single offer the obvious key.
///
/// `commands_active` is the same gate app chords use, so an overlay raised over the launcher (the
/// session picker, a rename prompt) keeps `Enter` for itself.
fn launcher_start_key(state: &State, key: KeyEvent) -> bool {
    key.is(KeyCode::Enter) && state.is_launcher() && crate::commands::commands_active(state)
}

/// Keys rozi claims while the sidebar body has focus.
///
/// The file tree is a widget that navigates itself, so only the tab-level keys are taken there; the
/// composed row lists have no widget behind them, so rozi owns their cursor too. Page movement
/// uses the active row list's measured viewport size, with a small fallback before its first layout.
fn handle_sidebar_key(ctx: &mut Context<AppRoot>, key: KeyEvent) -> Option<Update> {
    use crate::update::sidebar;
    // App commands run before focused widgets. Let the explorer consume its own Escape so `/`
    // returns to the tree; pointer-entered explorer focus never sets `sidebar.focused`, so its
    // escape callback takes the separate path that restores the pane.
    if key.is(KeyCode::Esc) && ctx.state.sidebar.explorer_entered_from_tree {
        return None;
    }
    if key.mods.ctrl && key.mods.shift {
        return match key.code {
            KeyCode::Left => Some(sidebar::reorder_active_tab(ctx, false)),
            KeyCode::Right => Some(sidebar::reorder_active_tab(ctx, true)),
            KeyCode::Up => Some(sidebar::move_active_tab_to_panel(ctx, false)),
            KeyCode::Down => Some(sidebar::move_active_tab_to_panel(ctx, true)),
            _ => None,
        };
    }
    if key.mods.ctrl {
        return match key.code {
            KeyCode::Up => Some(sidebar::focus_panel(ctx, false)),
            KeyCode::Down => Some(sidebar::focus_panel(ctx, true)),
            _ => None,
        };
    }
    if key.mods.shift {
        return match key.code {
            KeyCode::Left => Some(sidebar::resize_width(ctx, false)),
            KeyCode::Right => Some(sidebar::resize_width(ctx, true)),
            KeyCode::Up => Some(sidebar::resize_panel_split(ctx, false)),
            KeyCode::Down => Some(sidebar::resize_panel_split(ctx, true)),
            KeyCode::BackTab | KeyCode::Tab => Some(sidebar::cycle_tab(ctx, false)),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Esc => return Some(sidebar::blur_body(ctx)),
        // Tab cycles sidebar tabs rather than the focus ring. FileTree owns bare Left/Right for
        // directory navigation; h/l and Space retain the equivalent tree operations.
        KeyCode::Tab => return Some(sidebar::cycle_tab(ctx, true)),
        KeyCode::BackTab => return Some(sidebar::cycle_tab(ctx, false)),
        KeyCode::Char('s') => return Some(sidebar::toggle_split(ctx)),
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
        KeyCode::PageUp => Some(sidebar::move_cursor_page(ctx, false)),
        KeyCode::PageDown => Some(sidebar::move_cursor_page(ctx, true)),
        KeyCode::Home | KeyCode::Char('g') => Some(sidebar::move_cursor(ctx, isize::MIN)),
        KeyCode::End | KeyCode::Char('G') => Some(sidebar::move_cursor(ctx, isize::MAX)),
        KeyCode::Enter => Some(sidebar::activate_cursor(ctx)),
        _ => None,
    }
}

/// The file tree owns its own keyboard navigation; the composed row lists do not.
fn tree_tab_active(ctx: &Context<AppRoot>) -> bool {
    matches!(
        crate::view::sidebar::active_tab(ctx),
        Some(crate::config::SidebarTab::Tree { .. })
    )
}

fn handle_resize_mode_key(ctx: &mut Context<AppRoot>, key: KeyEvent) -> (bool, Update) {
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

fn framework_focused_pane(ctx: &Context<AppRoot>) -> Option<PaneId> {
    let workspace = ctx.state.active_workspace_ref();
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .find(|pane| ctx.has_focus_within_key(view::pane_window_key(pane.id, pane.pty_generation)))
        .map(|pane| pane.id)
}

pub(crate) fn sync_focus_from_framework(ctx: &mut Context<AppRoot>) {
    let workspace = ctx.state.active_workspace_ref();
    if let Some(id) = ctx.state.focused_pane()
        && workspace
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::Attachment;
    use tui_lipan::prelude::{KeyMods, Theme};

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    /// A state in the launcher: what dismissing the startup picker leaves behind.
    fn launcher_state() -> State {
        let mut state = State::new(Config::default(), Theme::default());
        *state.current_mut() = Attachment::new();
        assert!(state.is_launcher());
        state
    }

    #[test]
    fn bare_enter_starts_a_shell_only_in_an_unobstructed_launcher() {
        let mut state = launcher_state();
        assert!(launcher_start_key(
            &state,
            key(KeyCode::Enter, KeyMods::NONE)
        ));

        // The picker over the launcher owns Enter — that is how a session row is activated.
        state.show_session_picker = true;
        assert!(!launcher_start_key(
            &state,
            key(KeyCode::Enter, KeyMods::NONE)
        ));
        state.show_session_picker = false;

        // Modified Enter is the configured `spawn` chord, resolved before key routing sees it.
        assert!(!launcher_start_key(
            &state,
            key(KeyCode::Enter, KeyMods::ALT)
        ));
        assert!(!launcher_start_key(
            &state,
            key(KeyCode::Esc, KeyMods::NONE)
        ));
    }

    /// With a session attached, Enter is the shell's, not the app's.
    #[test]
    fn bare_enter_is_not_claimed_outside_the_launcher() {
        let state = State::new(Config::default(), Theme::default());
        assert!(!state.is_launcher());
        assert!(!launcher_start_key(
            &state,
            key(KeyCode::Enter, KeyMods::NONE)
        ));
    }
}
