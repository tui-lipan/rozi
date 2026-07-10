use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::UserCommandAction;
use crate::focus_ops::{
    cycle_focus_in_tiled_order, focus_in_direction, move_focused_to_workspace,
    promote_focused_to_master, relocate_active_workspace, request_current_pane_focus,
    request_palette_focus, request_pane_focus, switch_workspace,
};
use crate::identity_ops::open_rename_pane;
use crate::input::Action;
use crate::pane_lifecycle::{find_pane, spawn_pane, spawn_pane_in_workspace};
use crate::profile_ops::{open_profile_picker, open_save_profile_prompt};
use crate::resize_move_ops::{
    adjust_focused_split_ratio, move_focused_in_direction, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};
use crate::search_ops::open_search;
use crate::state::{Direction, Mode, PaneIdentity};
use crate::theme_ops::{apply_terminal_palette_to_state, open_theme_picker};

/// Read the system clipboard and send it to the focused pane's PTY, bracketed-paste wrapped so
/// shells/editors that opt in treat it as one paste instead of simulated keystrokes.
fn paste_from_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let text = match ctx.clipboard().read() {
        Ok(text) => text,
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Paste failed",
                err.to_string(),
            ));
            return Update::full();
        }
    };
    if text.is_empty() {
        return Update::full();
    }
    if let Err(err) = crate::pty_events::send_pane_bytes(ctx, id, wrap_bracketed_paste(&text)) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Paste failed",
            err,
        ));
    }
    Update::full()
}

/// Dispatch a `[keys]`-defined user command: `Run` opens a new pane running the shell command
/// (the same `identity.command` hook the scratchpad and control socket's `NewPane` use), `Send`
/// writes the literal text straight to the focused pane's PTY.
fn run_user_command(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(command) = ctx.state.config.user_commands.get(index).cloned() else {
        return Update::none();
    };
    match command.action {
        UserCommandAction::Run(command) => {
            let workspace_index = ctx.state.active_workspace;
            let previous_focused = ctx.state.workspaces[workspace_index].focused_pane;
            let identity = PaneIdentity {
                command: Some(command),
                ..PaneIdentity::default()
            };
            spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity).1
        }
        UserCommandAction::Send(text) => {
            let Some(id) = ctx.state.focused_pane else {
                return Update::full();
            };
            if let Err(err) = crate::pty_events::send_pane_bytes(ctx, id, text.into_bytes()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Command failed",
                    err,
                ));
            }
            Update::full()
        }
    }
}

/// vim-tmux-navigator-style directional focus: if the focused pane runs a split-aware program
/// (see `[navigation] editors`), forward the matching `Ctrl-h/j/k/l` so that program moves its
/// own split; otherwise move hyprmux pane focus. The forwarded program is expected to hand focus
/// back at its split edge via the control socket (`hyprmux run-action focus-<dir>`), which yields
/// the seamless "one keymap crosses both" behavior.
fn smart_focus(ctx: &mut Context<HyprmuxApp>, direction: Direction) -> Update {
    if let Some(id) = ctx.state.focused_pane
        && focused_pane_forwards_navigation(&ctx.state, id)
    {
        return crate::pty_events::forward_key_to_pane(ctx, id, navigation_key(direction));
    }

    let viewport = ctx.viewport();
    if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
        request_pane_focus(ctx, id);
    }
    Update::full()
}

/// Whether the pane's foreground program is one that should receive navigation keys itself.
fn focused_pane_forwards_navigation(state: &crate::state::State, id: crate::state::PaneId) -> bool {
    find_pane(state, id)
        .and_then(|pane| pane.terminal.foreground_command())
        .is_some_and(|command| state.config.navigation.is_split_editor(&command))
}

/// The `Ctrl-h/j/k/l` key a split-aware program expects for the given navigation direction,
/// matching vim-tmux-navigator's default mappings.
fn navigation_key(direction: Direction) -> KeyEvent {
    let ch = match direction {
        Direction::Left => 'h',
        Direction::Down => 'j',
        Direction::Up => 'k',
        Direction::Right => 'l',
    };
    KeyEvent {
        code: KeyCode::Char(ch),
        mods: KeyMods {
            ctrl: true,
            ..KeyMods::NONE
        },
    }
}

fn persist_pane_toggle(ctx: &mut Context<HyprmuxApp>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_pane_flag(key, value) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Preference not saved",
            err,
        ));
    }
}

fn persist_animation_toggle(ctx: &mut Context<HyprmuxApp>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_animation_flag(key, value) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Preference not saved",
            err,
        ));
    }
}

/// Whether `action` changes the shared window-manager layout (pane membership/order, tiling,
/// geometry, workspace/pane identity). Followers are blocked from these until they take control;
/// focus/workspace-switch/copy/search/palette/theme and terminal input stay local and are allowed.
pub(crate) fn is_layout_mutating(state: &crate::state::State, action: Action) -> bool {
    match action {
        Action::Spawn
        | Action::Close
        | Action::Move(_)
        | Action::Swap(_)
        | Action::PromoteToMaster
        | Action::ToggleFloat
        | Action::ToggleFullscreen
        | Action::FlipSplit
        | Action::AdjustRatio(_)
        | Action::EnterResizeMode
        | Action::ToggleLayout
        | Action::MoveToWorkspace(_)
        | Action::RelocateWorkspace(_)
        | Action::TogglePaneSynchronization
        | Action::RenamePane
        | Action::RenameWorkspace
        | Action::ToggleScratchpad => true,
        // A user `Run` command spawns a pane (structural); `Send` only writes to the PTY (local).
        Action::RunUserCommand(index) => matches!(
            state.config.user_commands.get(index).map(|cmd| &cmd.action),
            Some(UserCommandAction::Run(_))
        ),
        _ => false,
    }
}

pub(crate) fn execute_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update {
    execute_action_inner(ctx, action, true)
}

pub(crate) fn execute_palette_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update {
    execute_action_inner(ctx, action, false)
}

fn execute_action_inner(
    ctx: &mut Context<HyprmuxApp>,
    action: Action,
    confirmations_enabled: bool,
) -> Update {
    // Any action can flip a dynamic label (a toggle, layout cycling) or the `commands_active`
    // gate (mode/overlay changes). Marking dirty unconditionally here covers both the
    // `Msg::RunAction` path and control-socket `RunAction` requests
    // (`control_ops::run_action`), which call this directly.
    ctx.state.commands_dirty = true;
    // Followers may not mutate the shared layout: intercept before dispatch and nudge toward
    // taking control. Focus, workspace switching, copy/search/palette, and terminal input are all
    // local and fall through.
    if is_layout_mutating(&ctx.state, action) && crate::session_ops::nudge_if_follower(ctx) {
        return Update::full();
    }
    match action {
        Action::Spawn => spawn_pane(ctx),
        Action::Close => {
            crate::exit_ops::close_focused_pane_with_confirmation(ctx, confirmations_enabled)
        }
        Action::Focus(direction) => {
            let viewport = ctx.viewport();
            if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Action::SmartFocus(direction) => smart_focus(ctx, direction),
        Action::Move(direction) => {
            move_focused_in_direction(ctx, direction);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::SwitchWorkspace(index) => {
            switch_workspace(&mut ctx.state, index);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::MoveToWorkspace(index) => {
            move_focused_to_workspace(&mut ctx.state, index);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::RelocateWorkspace(index) => {
            relocate_active_workspace(&mut ctx.state, index);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::ToggleFloat => {
            toggle_tiling(ctx);
            Update::full()
        }
        Action::ToggleFullscreen => toggle_fullscreen(ctx),
        Action::RenamePane => open_rename_pane(ctx),
        Action::RenameWorkspace => crate::identity_ops::open_rename_workspace(ctx),
        Action::Paste => paste_from_focused_pane(ctx),
        Action::Swap(direction) => {
            swap_focused_in_direction(ctx, direction);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::CycleFocus(forward) => {
            if let Some(id) = cycle_focus_in_tiled_order(&mut ctx.state, forward) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Action::PromoteToMaster => {
            if promote_focused_to_master(&mut ctx.state) {
                ctx.state.animation = crate::anim::GeometryAnimation::AxisChange;
            }
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::FlipSplit => {
            toggle_focused_split_axis(&mut ctx.state);
            Update::full()
        }
        Action::AdjustRatio(delta) => {
            adjust_focused_split_ratio(&mut ctx.state, delta);
            Update::full()
        }
        Action::EnterResizeMode => {
            ctx.state.mode = Mode::Resize;
            ctx.state.show_help = false;
            ctx.state.show_palette = false;
            Update::full()
        }
        Action::ToggleLayout => {
            toggle_layout(ctx, !ctx.state.show_palette);
            Update::full()
        }
        Action::EnterCopyMode => crate::copy_mode::enter(ctx),
        Action::ToggleScratchpad => crate::scratchpad::toggle(ctx),
        Action::OpenSearch => open_search(ctx),
        Action::SaveProfile => open_save_profile_prompt(ctx),
        Action::OpenProfilePicker => open_profile_picker(ctx),
        Action::OpenSessionPicker => crate::session_ops::open_session_picker(ctx),
        Action::RenameSession => crate::session_ops::open_rename_session(ctx),
        Action::TakeControl => crate::session_ops::take_control(ctx),
        Action::Detach => crate::exit_ops::detach(ctx),
        Action::Quit => crate::exit_ops::quit_client(ctx, confirmations_enabled),
        Action::KillWorkspace => {
            crate::exit_ops::kill_workspace_with_confirmation(ctx, confirmations_enabled)
        }
        Action::KillSession => {
            crate::exit_ops::kill_session_with_confirmation(ctx, confirmations_enabled)
        }
        Action::OpenThemePicker => open_theme_picker(ctx),
        Action::OpenAppearance => {
            ctx.state.show_appearance = true;
            ctx.state.show_palette = false;
            ctx.state.show_help = false;
            ctx.state.show_theme_picker = false;
            ctx.state.commands_dirty = true;
            ctx.request_focus(crate::view::appearance_palette_key());
            Update::full()
        }
        Action::TogglePalette => {
            ctx.state.show_palette = !ctx.state.show_palette;
            if ctx.state.show_palette {
                ctx.state.show_help = false;
                ctx.state.show_appearance = false;
                request_palette_focus(ctx);
            }
            Update::full()
        }
        Action::ToggleHelp => {
            ctx.state.show_help = !ctx.state.show_help;
            if ctx.state.show_help {
                ctx.state.show_palette = false;
                ctx.state.show_appearance = false;
            }
            Update::full()
        }
        Action::ToggleTitles => {
            ctx.state.config.pane.show_titles = !ctx.state.config.pane.show_titles;
            persist_pane_toggle(ctx, "show_titles", ctx.state.config.pane.show_titles);
            Update::full()
        }
        Action::ToggleWorkbar => {
            ctx.state.config.pane.show_workbar = !ctx.state.config.pane.show_workbar;
            persist_pane_toggle(ctx, "show_workbar", ctx.state.config.pane.show_workbar);
            Update::full()
        }
        Action::ToggleWorkbarGap => {
            ctx.state.config.pane.workbar_gap = !ctx.state.config.pane.workbar_gap;
            persist_pane_toggle(ctx, "workbar_gap", ctx.state.config.pane.workbar_gap);
            Update::full()
        }
        Action::ToggleWorkbarPosition => {
            ctx.state.config.pane.workbar_at_bottom = !ctx.state.config.pane.workbar_at_bottom;
            persist_pane_toggle(
                ctx,
                "workbar_at_bottom",
                ctx.state.config.pane.workbar_at_bottom,
            );
            Update::full()
        }
        Action::ToggleAnimations => {
            ctx.state.config.animations.enabled = !ctx.state.config.animations.enabled;
            persist_animation_toggle(ctx, "enabled", ctx.state.config.animations.enabled);
            Update::full()
        }
        Action::ToggleFocusOnHover => {
            ctx.state.config.pane.focus_on_hover = !ctx.state.config.pane.focus_on_hover;
            persist_pane_toggle(ctx, "focus_on_hover", ctx.state.config.pane.focus_on_hover);
            Update::full()
        }
        Action::ToggleHighlightFocusedBackground => {
            ctx.state.config.pane.highlight_focused_background =
                !ctx.state.config.pane.highlight_focused_background;
            persist_pane_toggle(
                ctx,
                "highlight_focused_background",
                ctx.state.config.pane.highlight_focused_background,
            );
            apply_terminal_palette_to_state(&mut ctx.state);
            Update::full()
        }
        Action::ToggleHighlightFocusedBorder => {
            ctx.state.config.pane.highlight_focused_border =
                !ctx.state.config.pane.highlight_focused_border;
            persist_pane_toggle(
                ctx,
                "highlight_focused_border",
                ctx.state.config.pane.highlight_focused_border,
            );
            Update::full()
        }
        Action::ToggleBorderMerge => {
            ctx.state.config.pane.merge_borders = !ctx.state.config.pane.merge_borders;
            persist_pane_toggle(ctx, "merge_borders", ctx.state.config.pane.merge_borders);
            Update::full()
        }
        Action::CycleBorderStyle => {
            let next = ctx.state.config.pane.border_style.next();
            ctx.state.config.pane.border_style = next;
            if let Err(err) = crate::config::persist_pane_string("border_style", next.id()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Preference not saved",
                    err,
                ));
            }
            Update::full()
        }
        Action::CycleTitleStyle => {
            let next = ctx.state.config.pane.title_style.next();
            ctx.state.config.pane.title_style = next;
            if let Err(err) = crate::config::persist_pane_string("title_style", next.id()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Preference not saved",
                    err,
                ));
            }
            Update::full()
        }
        Action::CycleWorkbarBadgeStyle => {
            let next = ctx.state.config.pane.workbar_badge_style.next_badge();
            ctx.state.config.pane.workbar_badge_style = next;
            if let Err(err) = crate::config::persist_pane_string("workbar_badge_style", next.id()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Preference not saved",
                    err,
                ));
            }
            Update::full()
        }
        Action::CycleWorkbarTabStyle => {
            let next = ctx.state.config.pane.workbar_tab_style.next_badge();
            ctx.state.config.pane.workbar_tab_style = next;
            if let Err(err) = crate::config::persist_pane_string("workbar_tab_style", next.id()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Preference not saved",
                    err,
                ));
            }
            Update::full()
        }
        Action::CycleWorkbarStyle => {
            let next = ctx.state.config.pane.workbar_style.next();
            ctx.state.config.pane.workbar_style = next;
            if let Err(err) = crate::config::persist_pane_string("workbar_style", next.id()) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Preference not saved",
                    err,
                ));
            }
            Update::full()
        }
        Action::RunUserCommand(index) => run_user_command(ctx, index),
        Action::OpenConfigFile => crate::config_ops::open_config_file(ctx),
        Action::TogglePaneSynchronization => {
            let synchronized = {
                let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
                workspace.synchronized = !workspace.synchronized;
                workspace.synchronized
            };
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                if synchronized { "Sync on" } else { "Sync off" },
            ));
            Update::full()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_mutating_classification_gates_structure_not_navigation() {
        let state =
            crate::state::State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        // Structural / geometry actions are gated for followers.
        for action in [
            Action::Spawn,
            Action::Close,
            Action::Move(Direction::Left),
            Action::ToggleFloat,
            Action::EnterResizeMode,
            Action::MoveToWorkspace(1),
            Action::ToggleScratchpad,
        ] {
            assert!(
                is_layout_mutating(&state, action),
                "{action:?} should be gated"
            );
        }
        // Local view actions stay allowed for followers.
        for action in [
            Action::Focus(Direction::Left),
            Action::SwitchWorkspace(1),
            Action::EnterCopyMode,
            Action::TogglePalette,
            Action::Detach,
        ] {
            assert!(
                !is_layout_mutating(&state, action),
                "{action:?} should not be gated"
            );
        }
    }

    #[test]
    fn follower_layout_action_emits_no_frame_but_focus_still_works() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2); // follower
                    state.shared = Some(shared);
                }
                backend.render();
                let before = backend.state_mut().workspaces[0].panes.len();

                backend
                    .dispatch(Msg::RunAction(Action::Spawn))
                    .expect("dispatch spawn");

                assert_eq!(
                    backend.state_mut().workspaces[0].panes.len(),
                    before,
                    "a follower's spawn is a no-op"
                );
                let spawns = rx
                    .try_iter()
                    .filter(|msg| {
                        matches!(
                            msg,
                            ClientOutbound::Control(ClientMessage::SpawnPane { .. })
                        )
                    })
                    .count();
                assert_eq!(spawns, 0, "a gated action must not emit a frame");

                // Focus is local and still works for a follower.
                backend.dispatch(Msg::FocusPane(1)).expect("dispatch focus");
                assert_eq!(backend.state_mut().focused_pane, Some(1));
            })
            .expect("spawn gate test thread")
            .join()
            .expect("gate test thread completes");
    }

    #[test]
    fn navigation_key_maps_directions_to_ctrl_hjkl() {
        let ctrl = |ch| KeyEvent {
            code: KeyCode::Char(ch),
            mods: KeyMods {
                ctrl: true,
                ..KeyMods::NONE
            },
        };
        assert_eq!(navigation_key(Direction::Left), ctrl('h'));
        assert_eq!(navigation_key(Direction::Down), ctrl('j'));
        assert_eq!(navigation_key(Direction::Up), ctrl('k'));
        assert_eq!(navigation_key(Direction::Right), ctrl('l'));
    }
}
