use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::UserCommandAction;
use crate::input::Action;
use crate::ops::focus::{
    cycle_focus_in_tiled_order, focus_in_direction, focus_in_direction_no_wrap,
    move_focused_to_workspace, promote_focused_to_master, relocate_active_workspace,
    request_current_pane_focus, request_palette_focus, request_pane_focus, switch_workspace,
};
use crate::ops::identity::open_rename_pane;
use crate::ops::profile::{open_profile_picker, open_save_profile_prompt};
use crate::ops::resize_move::{
    adjust_focused_split_ratio, move_focused_in_direction, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};
use crate::ops::search::open_search;
use crate::ops::theme::{apply_terminal_palette_to_state, open_theme_picker};
use crate::pane_lifecycle::{find_pane, spawn_pane};
use crate::state::{Direction, Mode, PaneIdentity, ToastChannel};

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
    let modes = find_pane(&ctx.state, id).map_or(TerminalKeyModes::default(), |pane| {
        pane.terminal.snapshot().key_modes
    });
    if let Err(err) = crate::pty_events::send_pane_bytes(ctx, id, encode_paste(&text, modes)) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Paste failed",
            err,
        ));
    }
    Update::full()
}

fn toggle_pane_logging(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::none();
    };
    let Some(pane) = crate::pane_lifecycle::find_pane(&ctx.state, id) else {
        return Update::none();
    };
    let generation = pane.pty_generation;
    let enabled = !pane.logging;
    if let Some(client) = &ctx.state.current().session_client {
        client.set_pane_logging(id, generation, enabled);
    }
    Update::none()
}

/// Dispatch a `[keys]`-defined user command: `Run` opens a new pane running the shell command
/// (the same `identity.command` hook the scratchpad and control socket's `NewPane` use), `Send`
/// writes the literal text straight to the focused pane's PTY.
fn run_user_command(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(command) = ctx.state.config.user_commands.get(index).cloned() else {
        return Update::none();
    };
    execute_user_command_action(ctx, &command.action)
}

pub(crate) fn execute_user_command_action(
    ctx: &mut Context<HyprmuxApp>,
    action: &UserCommandAction,
) -> Update {
    execute_user_command_action_with_env(ctx, action, Vec::new())
}

/// Run a user command with extra environment for this spawn only.
///
/// `env` is how a caller hands an untrusted value — a filename from the file tree, say — to a
/// command line without ever putting it *in* that command line. The command references it as
/// `"$VAR"`, which the shell expands as a single word rather than re-parsing for command syntax.
/// `Send` ignores it: it starts no process, it only types text into an existing one.
pub(crate) fn execute_user_command_action_with_env(
    ctx: &mut Context<HyprmuxApp>,
    action: &UserCommandAction,
    env: Vec<(String, String)>,
) -> Update {
    match action {
        UserCommandAction::Run { command, keep_open } => {
            let identity = PaneIdentity {
                command: Some(command.clone()),
                keep_open: *keep_open,
                // `cargo build` means "build the project I am looking at"; without this the command
                // runs wherever the session server was started.
                cwd: crate::pane_lifecycle::focused_spawn_cwd(&ctx.state),
                env,
                ..PaneIdentity::default()
            };
            crate::pane_lifecycle::spawn_interactive_pane(
                ctx,
                ctx.state.active_workspace,
                None,
                identity,
            )
            .1
        }
        UserCommandAction::Send(text) => {
            let Some(id) = ctx.state.focused_pane else {
                return Update::full();
            };
            if let Err(err) = crate::pty_events::send_pane_bytes(ctx, id, text.as_bytes().to_vec())
            {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Command failed",
                    err,
                ));
            }
            Update::full()
        }
        UserCommandAction::Popup { command, keep_open } => crate::popup::open(
            ctx,
            command.clone(),
            None,
            None,
            None,
            None,
            *keep_open,
            env,
        )
        .unwrap_or_else(|error| {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Popup failed",
                error,
            ));
            Update::full()
        }),
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
        crate::pty_events::replace_toast(
            ctx,
            ToastChannel::PreferenceSave,
            crate::pty_events::error_toast(&ctx.state.theme, "Preference not saved", err),
        );
    }
}

macro_rules! toggle_pane_flag {
    ($ctx:ident, $field:ident) => {{
        $ctx.state.config.pane.$field = !$ctx.state.config.pane.$field;
        persist_pane_toggle($ctx, stringify!($field), $ctx.state.config.pane.$field);
        Update::full()
    }};
}

fn persist_animation_toggle(ctx: &mut Context<HyprmuxApp>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_animation_flag(key, value) {
        crate::pty_events::replace_toast(
            ctx,
            ToastChannel::PreferenceSave,
            crate::pty_events::error_toast(&ctx.state.theme, "Preference not saved", err),
        );
    }
}

fn persist_pane_string_or_toast(ctx: &mut Context<HyprmuxApp>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_pane_string(key, value) {
        crate::pty_events::replace_toast(
            ctx,
            ToastChannel::PreferenceSave,
            crate::pty_events::error_toast(&ctx.state.theme, "Preference not saved", err),
        );
    }
}

/// Whether `action` changes the shared window-manager layout (pane membership/order, tiling,
/// geometry, workspace/pane identity). Followers are blocked from these until they take control;
/// focus/workspace-switch/copy/search/palette/theme and terminal input stay local and are allowed.
pub(crate) fn is_layout_mutating(state: &crate::state::State, action: Action) -> bool {
    match action {
        Action::Spawn
        | Action::RespawnPane
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
        | Action::KillWorkspace => true,
        // Showing can create the server-owned scratch PTY; hiding is a local overlay change.
        Action::ToggleScratchpad => !state.scratch_visible,
        // A user `Run` command spawns a pane (structural); `Send` only writes to the PTY (local).
        Action::RunUserCommand(index) => matches!(
            state.config.user_commands.get(index).map(|cmd| &cmd.action),
            Some(UserCommandAction::Run { .. } | UserCommandAction::Popup { .. })
        ),
        _ => false,
    }
}

/// The scratchpad is a focused modal terminal: workspace and application actions must not run
/// behind it. Its own toggle remains available so the same shortcut can dismiss it.
pub(crate) fn is_blocked_by_scratchpad(state: &crate::state::State, action: Action) -> bool {
    state.scratch_visible
        && !matches!(
            action,
            Action::ToggleScratchpad | Action::ToggleSidebar | Action::ToggleDevtools
        )
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
    if is_blocked_by_scratchpad(&ctx.state, action) {
        return Update::none();
    }
    // Any action can flip a dynamic label (a toggle, layout cycling) or the `commands_active`
    // gate (mode/overlay changes). Marking dirty unconditionally here covers both the
    // `Msg::RunAction` path and control-socket `RunAction` requests
    // (`ops::control::run_action`), which call this directly.
    ctx.state.commands_dirty = true;
    // Followers may not mutate the shared layout: intercept before dispatch and nudge toward
    // taking control. Focus, workspace switching, copy/search/palette, and terminal input are all
    // local and fall through.
    if is_layout_mutating(&ctx.state, action) && crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    if !crate::commands::command_available(action, &ctx.state) {
        return Update::full();
    }
    match action {
        Action::Spawn => spawn_pane(ctx),
        Action::RespawnPane => crate::pane_lifecycle::respawn_focused_pane(ctx),
        Action::TogglePaneLogging => toggle_pane_logging(ctx),
        Action::Close => {
            if ctx.state.popup.is_some() {
                return crate::popup::close(ctx);
            }
            crate::ops::exit::close_focused_pane_with_confirmation(ctx, confirmations_enabled)
        }
        Action::Focus(direction) => {
            let viewport = ctx.viewport();
            if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Action::FocusNoWrap(direction) => {
            let viewport = ctx.viewport();
            if let Some(id) = focus_in_direction_no_wrap(&mut ctx.state, direction, viewport) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Action::SmartFocus(direction) => smart_focus(ctx, direction),
        Action::FocusNextBlockedPane => {
            if let Some(id) = crate::ops::focus::next_blocked_pane(&ctx.state) {
                crate::ops::focus::focus_pane_anywhere(ctx, id);
                Update::full()
            } else {
                Update::none()
            }
        }
        Action::Move(direction) => {
            move_focused_in_direction(ctx, direction);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::Swap(direction) => {
            swap_focused_in_direction(ctx, direction);
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
        Action::RenameWorkspace => crate::ops::identity::open_rename_workspace(ctx),
        Action::Paste => paste_from_focused_pane(ctx),
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
        Action::EnterHintMode => crate::hints::enter(ctx),
        Action::ToggleScratchpad => crate::scratchpad::toggle(ctx),
        Action::OpenSearch => open_search(ctx),
        Action::SaveProfile => open_save_profile_prompt(ctx),
        Action::OpenProfilePicker => open_profile_picker(ctx),
        Action::ApplyProfile => crate::ops::profile::open_apply_profile_picker(ctx),
        Action::OpenSessionPicker => crate::ops::session::open_session_picker(ctx),
        Action::OpenClientList => crate::ops::session::open_client_list(ctx),
        Action::RenameSession => crate::ops::session::open_rename_session(ctx),
        Action::NewTemporarySession => {
            if ctx.state.current().session_attached
                && crate::ops::session::may_shutdown_ephemeral(&ctx.state)
                && confirmations_enabled
                && ctx.state.config.confirm.new_temporary_session
                && !crate::ops::exit::confirm_new_temporary_session(ctx)
            {
                return Update::full();
            }
            crate::ops::session::release_current_session(ctx);
            let update = crate::ops::session::swap_to_fresh_ephemeral(ctx);
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                "Started a fresh temporary session",
            ));
            update
        }
        Action::RequestControl => crate::ops::session::request_control(ctx),
        Action::GrantControl => crate::ops::session::grant_control_to_requester(ctx),
        Action::ToggleInputLock => crate::ops::session::toggle_input_lock(ctx),
        Action::Detach => crate::ops::session::detach_current_session(ctx),
        Action::Quit => crate::ops::exit::quit_client(ctx, confirmations_enabled),
        Action::KillWorkspace => {
            crate::ops::exit::kill_workspace_with_confirmation(ctx, confirmations_enabled)
        }
        Action::KillSession => {
            crate::ops::exit::kill_session_with_confirmation(ctx, confirmations_enabled)
        }
        Action::OpenThemePicker => open_theme_picker(ctx),
        Action::OpenAppearance => {
            ctx.state.pane_padding_editor = None;
            ctx.state.show_appearance = true;
            ctx.state.show_palette = false;
            ctx.state.show_help = false;
            ctx.state.show_theme_picker = false;
            ctx.state.commands_dirty = true;
            ctx.request_focus(crate::view::appearance_palette_key());
            Update::full()
        }
        Action::TogglePalette => {
            ctx.state.pane_padding_editor = None;
            ctx.state.show_palette = !ctx.state.show_palette;
            if ctx.state.show_palette {
                ctx.state.show_help = false;
                ctx.state.show_appearance = false;
                request_palette_focus(ctx);
            }
            Update::full()
        }
        Action::ToggleHelp => {
            ctx.state.pane_padding_editor = None;
            ctx.state.show_help = !ctx.state.show_help;
            if ctx.state.show_help {
                ctx.state.show_palette = false;
                ctx.state.show_appearance = false;
            }
            Update::full()
        }
        Action::ToggleDevtools => {
            ctx.toggle_devtools();
            Update::none()
        }
        Action::ToggleTitles => toggle_pane_flag!(ctx, show_titles),
        Action::ToggleWorkbar => toggle_pane_flag!(ctx, show_workbar),
        Action::ToggleWorkbarGap => toggle_pane_flag!(ctx, workbar_gap),
        Action::ToggleWorkbarPosition => toggle_pane_flag!(ctx, workbar_at_bottom),
        Action::ToggleWorkbarPowerline => toggle_pane_flag!(ctx, workbar_powerline),
        Action::ToggleSidebar => {
            ctx.state.sidebar_visible = !ctx.state.sidebar_visible;
            crate::update::sidebar::visibility_changed(ctx)
        }
        Action::FocusSidebar => crate::update::sidebar::focus_body(ctx),
        Action::SidebarNextTab => {
            if ctx.state.sidebar_visible {
                ctx.state.sidebar.cycle(&ctx.state.config.sidebar, true);
                crate::update::sidebar::visibility_changed(ctx)
            } else {
                Update::none()
            }
        }
        Action::SidebarPrevTab => {
            if ctx.state.sidebar_visible {
                ctx.state.sidebar.cycle(&ctx.state.config.sidebar, false);
                crate::update::sidebar::visibility_changed(ctx)
            } else {
                Update::none()
            }
        }
        Action::ToggleAnimations => {
            ctx.state.config.animations.enabled = !ctx.state.config.animations.enabled;
            persist_animation_toggle(ctx, "enabled", ctx.state.config.animations.enabled);
            Update::full()
        }
        Action::ToggleFocusOnHover => toggle_pane_flag!(ctx, focus_on_hover),
        Action::ToggleHighlightFocusedBackground => {
            toggle_pane_flag!(ctx, highlight_focused_background);
            apply_terminal_palette_to_state(&mut ctx.state);
            Update::full()
        }
        Action::ToggleHighlightFocusedBorder => {
            toggle_pane_flag!(ctx, highlight_focused_border)
        }
        Action::ToggleBorderMerge => toggle_pane_flag!(ctx, merge_borders),
        Action::ToggleBackgroundFollowsTerminal => {
            toggle_pane_flag!(ctx, background_follows_terminal);
            crate::ops::theme::reapply_active_theme(ctx)
        }
        Action::CycleBorderStyle => {
            let next = ctx.state.config.pane.border_style.next();
            ctx.state.config.pane.border_style = next;
            persist_pane_string_or_toast(ctx, "border_style", next.id());
            Update::full()
        }
        Action::CycleTitleStyle => {
            let next = ctx.state.config.pane.title_style.next();
            ctx.state.config.pane.title_style = next;
            persist_pane_string_or_toast(ctx, "title_style", next.id());
            Update::full()
        }
        Action::CycleWorkbarBadgeStyle => {
            let next = ctx.state.config.pane.workbar_badge_style.next_badge();
            ctx.state.config.pane.workbar_badge_style = next;
            persist_pane_string_or_toast(ctx, "workbar_badge_style", next.id());
            Update::full()
        }
        Action::CycleWorkbarTabStyle => {
            let next = ctx.state.config.pane.workbar_tab_style.next_badge();
            ctx.state.config.pane.workbar_tab_style = next;
            persist_pane_string_or_toast(ctx, "workbar_tab_style", next.id());
            Update::full()
        }
        Action::CycleWorkbarStyle => {
            let next = ctx.state.config.pane.workbar_style.next();
            ctx.state.config.pane.workbar_style = next;
            persist_pane_string_or_toast(ctx, "workbar_style", next.id());
            Update::full()
        }
        Action::RunUserCommand(index) => run_user_command(ctx, index),
        Action::OpenConfigFile => crate::ops::config::open_config_file(ctx),
        Action::EditScrollback => crate::ops::scrollback::edit_scrollback(ctx),
        Action::CopyLastOutput => crate::ops::last_output::copy_last_output(ctx),
        Action::TogglePaneSynchronization => {
            let synchronized = {
                let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
                workspace.synchronized = !workspace.synchronized;
                workspace.synchronized
            };
            crate::pty_events::replace_toast(
                ctx,
                ToastChannel::PaneSynchronization,
                crate::pty_events::info_toast(
                    &ctx.state.theme,
                    if synchronized { "Sync on" } else { "Sync off" },
                ),
            );
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
            Action::KillWorkspace,
            Action::ToggleScratchpad,
        ] {
            assert!(
                is_layout_mutating(&state, action),
                "{action:?} should be gated"
            );
        }
        // Local view actions and whole-session teardown stay allowed for followers.
        for action in [
            Action::Focus(Direction::Left),
            Action::SwitchWorkspace(1),
            Action::EnterCopyMode,
            Action::EnterHintMode,
            Action::TogglePalette,
            Action::Detach,
            Action::KillSession,
        ] {
            assert!(
                !is_layout_mutating(&state, action),
                "{action:?} should not be gated"
            );
        }
    }

    #[test]
    fn scratchpad_allows_its_toggle_and_sidebar_toggle() {
        let mut state =
            crate::state::State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.scratch_visible = true;

        for action in [
            Action::Spawn,
            Action::Close,
            Action::Focus(Direction::Left),
            Action::SwitchWorkspace(1),
            Action::TogglePalette,
            Action::RunUserCommand(0),
            Action::Quit,
        ] {
            assert!(
                is_blocked_by_scratchpad(&state, action),
                "{action:?} should be blocked"
            );
        }
        assert!(!is_blocked_by_scratchpad(&state, Action::ToggleScratchpad));
        assert!(!is_blocked_by_scratchpad(&state, Action::ToggleSidebar));
        assert!(is_blocked_by_scratchpad(&state, Action::SidebarNextTab));
        assert!(!is_layout_mutating(&state, Action::ToggleScratchpad));
        assert!(!is_layout_mutating(&state, Action::ToggleSidebar));
    }

    #[test]
    fn scratchpad_blocks_spawn_without_changing_focus() {
        use crate::Msg;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.state_mut().scratch_visible = true;
                let before_panes = backend.state().workspaces[0].panes.len();
                let before_focus = backend.state().focused_pane;

                backend
                    .dispatch(Msg::RunAction(Action::Spawn))
                    .expect("dispatch blocked spawn");

                assert_eq!(backend.state().workspaces[0].panes.len(), before_panes);
                assert_eq!(backend.state().focused_pane, before_focus);
            })
            .expect("spawn scratchpad action test thread")
            .join()
            .expect("scratchpad action test thread completes");
    }

    #[test]
    fn focus_next_blocked_action_switches_workspace_and_focuses_pane() {
        use crate::Msg;
        use crate::state::Pane;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let mut blocked = Pane::new(
                    2,
                    100,
                    FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 40.0,
                        h: 20.0,
                    },
                );
                blocked.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
                    value: "BLOCKED".to_string(),
                    reason: None,
                    set_at: 1,
                });
                backend.state_mut().workspaces[1].panes.push(blocked);

                backend
                    .dispatch(Msg::RunAction(Action::FocusNextBlockedPane))
                    .expect("focus blocked pane");
                assert_eq!(backend.state().active_workspace, 1);
                assert_eq!(backend.state().focused_pane, Some(2));
            })
            .expect("spawn blocked focus action test")
            .join()
            .expect("blocked focus action test completes");
    }

    #[test]
    fn sidebar_actions_toggle_cycle_and_publish_only_controller_canvas() {
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let viewport = Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                };
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(viewport);
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    state.current_mut().shared = Some(shared);
                }

                backend
                    .dispatch(Msg::RunAction(Action::ToggleSidebar))
                    .expect("toggle controller sidebar");
                assert!(backend.state().sidebar_visible);
                assert_eq!(
                    backend
                        .state()
                        .canvas_bounds_from_terminal_viewport(viewport)
                        .w,
                    68.0
                );
                backend
                    .dispatch(Msg::RunAction(Action::SidebarNextTab))
                    .expect("cycle sidebar tab");
                assert_eq!(
                    backend.state().sidebar.active_tab,
                    Some(crate::config::SidebarTabId::new("panes"))
                );
                backend
                    .dispatch(Msg::FlushLayoutCommit { epoch: 0 })
                    .expect("flush controller layout");
                let committed = rx
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::Control(ClientMessage::CommitLayout { layout, .. }) => {
                            Some((layout.canvas_cols, layout.canvas_rows))
                        }
                        _ => None,
                    })
                    .last();
                assert_eq!(committed, Some((68, 29)));

                let (follower_client, follower_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_client = Some(follower_client);
                    state.sidebar_visible = false;
                    state.current_mut().shared.as_mut().unwrap().controller = Some(2);
                }
                backend
                    .dispatch(Msg::RunAction(Action::ToggleSidebar))
                    .expect("toggle follower sidebar");
                backend
                    .dispatch(Msg::FlushLayoutCommit { epoch: 0 })
                    .expect("attempt follower flush");
                assert!(follower_rx.try_iter().all(|message| !matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::CommitLayout { .. })
                )));
            })
            .expect("spawn sidebar action test thread")
            .join()
            .expect("sidebar action test thread completes");
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
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2); // follower
                    state.current_mut().shared = Some(shared);
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

    #[test]
    fn repeated_state_toasts_replace_only_their_own_channel() {
        use crate::Msg;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());

                backend
                    .dispatch(Msg::RunAction(Action::ToggleLayout))
                    .expect("dispatch first layout cycle");
                let first_layout_toast = *backend
                    .state()
                    .replaceable_toasts
                    .get(&ToastChannel::LayoutMode)
                    .expect("layout toast is tracked");

                backend
                    .dispatch(Msg::RunAction(Action::ToggleLayout))
                    .expect("dispatch second layout cycle");
                let second_layout_toast = *backend
                    .state()
                    .replaceable_toasts
                    .get(&ToastChannel::LayoutMode)
                    .expect("replacement layout toast is tracked");
                assert_ne!(first_layout_toast, second_layout_toast);
                assert_eq!(backend.state().replaceable_toasts.len(), 1);

                backend
                    .dispatch(Msg::RunAction(Action::TogglePaneSynchronization))
                    .expect("dispatch synchronization toggle");
                assert_eq!(backend.state().replaceable_toasts.len(), 2);
                assert_eq!(
                    backend
                        .state()
                        .replaceable_toasts
                        .get(&ToastChannel::LayoutMode),
                    Some(&second_layout_toast),
                    "an independent toast channel must not replace the layout toast",
                );
            })
            .expect("spawn toast replacement test thread")
            .join()
            .expect("toast replacement test thread completes");
    }

    #[test]
    fn control_run_action_clears_stale_padding_editor() {
        use crate::Msg;
        use crate::control::{ControlCommand, ControlEnvelope, ControlRequest};
        use crate::state::PanePaddingEditorState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.state_mut().show_appearance = true;
                backend.state_mut().pane_padding_editor =
                    Some(PanePaddingEditorState::new((1, 1, 1, 1)));
                let (reply, replies) = std::sync::mpsc::channel();

                // Control requests call `execute_action` directly, bypassing `Msg::RunAction`.
                backend
                    .dispatch(Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::RunAction {
                                action: "command-palette".into(),
                            },
                            source_pane: None,
                        },
                        reply,
                    }))
                    .expect("dispatch control action");

                assert!(replies.recv().expect("control response").ok);
                assert!(backend.state().show_palette);
                assert!(backend.state().pane_padding_editor.is_none());
            })
            .expect("spawn direct action test thread")
            .join()
            .expect("direct action test thread completes");
    }
}
