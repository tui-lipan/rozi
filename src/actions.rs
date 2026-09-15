use tui_lipan::prelude::*;

use crate::AppRoot;
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
use crate::ops::theme::open_theme_picker;
use crate::ops::user_command;
use crate::pane::lifecycle::{find_pane, spawn_floating_pane_at_cursor, spawn_pane};
use crate::state::{Direction, Mode};

/// Read the system clipboard and send it to the focused pane's PTY, bracketed-paste wrapped so
/// shells/editors that opt in treat it as one paste instead of simulated keystrokes.
fn paste_from_focused_pane(ctx: &mut Context<AppRoot>) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::full();
    };
    let text = match ctx.clipboard().read() {
        Ok(text) => text,
        Err(err) => {
            crate::pane::pty_events::notify_error(ctx, "Paste failed", err.to_string());
            return Update::full();
        }
    };
    if text.is_empty() {
        return Update::full();
    }
    let modes = find_pane(&ctx.state, id).map_or(TerminalKeyModes::default(), |pane| {
        pane.terminal.snapshot().key_modes
    });
    if let Err(err) = crate::pane::pty_events::send_pane_bytes(ctx, id, encode_paste(&text, modes))
    {
        crate::pane::pty_events::notify_error(ctx, "Paste failed", err);
    }
    // The app-level paste action writes straight to the PTY, so it misses the widget input path
    // that acknowledges an ordinary paste. Same act, same answer.
    crate::ops::focus::acknowledge_pane_input(&mut ctx.state, id);
    Update::full()
}

fn toggle_pane_logging(ctx: &mut Context<AppRoot>) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::none();
    };
    let Some(pane) = crate::pane::lifecycle::find_pane(&ctx.state, id) else {
        return Update::none();
    };
    let generation = pane.pty_generation;
    let enabled = !pane.logging;
    if let Some(client) = ctx.state.pty_client_for_pane(id) {
        client.set_pane_logging(
            id,
            generation,
            crate::pane::lifecycle::pane_is_local(&ctx.state, id),
            enabled,
        );
    }
    Update::none()
}

/// Dispatch a `[keys]`-defined user command: `Run` opens a new pane running the shell launch
/// (the same `PaneIdentity::launch` path the scratchpad and control socket's `NewPane` use), `Send`
/// writes the literal text straight to the focused pane's PTY.
fn run_user_command(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(command) = ctx.state.config.user_commands.get(index).cloned() else {
        return Update::none();
    };
    user_command::execute(ctx, &command.action)
}

fn run_named_command(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(command) = ctx.state.config.commands.get(index).cloned() else {
        return Update::none();
    };
    user_command::execute_with_env(ctx, &command.action, command.env)
}

/// Split-aware directional focus: if the focused pane runs a program in the resolved navigation
/// target set, forward the matching `Ctrl-h/j/k/l` so that program moves its own split; otherwise
/// move rozi pane focus. An editor-specific integration hands focus back at its outer edge through
/// `rozi run-action focus-<dir>`.
fn smart_focus(ctx: &mut Context<AppRoot>, direction: Direction) -> Update {
    if let Some(id) = ctx.state.focused_pane()
        && focused_pane_forwards_navigation(&ctx.state, id)
    {
        return crate::pane::pty_events::forward_key_to_pane(ctx, id, navigation_key(direction));
    }

    let viewport = ctx.viewport();
    if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
        request_pane_focus(ctx, id);
    }
    Update::full()
}

/// Whether any process eligible to read from the pane is a split-aware navigation target.
fn focused_pane_forwards_navigation(state: &crate::state::State, id: crate::state::PaneId) -> bool {
    let Some(pane) = find_pane(state, id) else {
        return false;
    };
    pane.terminal
        .foreground_commands()
        .any(|program| state.config.navigation.is_split_editor(program))
}

/// The `Ctrl-h/j/k/l` convention shared by the split-aware editor integrations.
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

/// Whether `action` changes the shared window-manager layout (pane membership/order, tiling,
/// geometry, workspace/pane identity). Followers are blocked from these until they take control;
/// focus/workspace-switch/copy/search/palette/theme and terminal input stay local and are allowed.
pub(crate) fn is_layout_mutating(state: &crate::state::State, action: Action) -> bool {
    match action {
        Action::Spawn
        | Action::SpawnFloat
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
        | Action::KillWorkspace
        | Action::OpenConfigFile
        | Action::EditScrollback => true,
        // Scratch and popup PTYs are protocol-level client-local panes, not shared layout.
        Action::ToggleScratchpad => false,
        // A user `Run` command spawns a shared pane; popup/send remain client-local.
        Action::RunUserCommand(index) => matches!(
            state.config.user_commands.get(index).map(|cmd| &cmd.action),
            Some(UserCommandAction::Run { .. })
        ),
        Action::RunNamedCommand(index) => matches!(
            state.config.commands.get(index).map(|cmd| &cmd.action),
            Some(UserCommandAction::Run { .. })
        ),
        _ => false,
    }
}

/// Scratch panes support ordinary pane-local actions. Attachment workspace/session/profile
/// operations remain blocked so the local dropdown cannot mutate the hidden shared workspace.
pub(crate) fn is_blocked_by_scratchpad(state: &crate::state::State, action: Action) -> bool {
    state.scratch_visible
        && matches!(
            action,
            Action::SwitchWorkspace(_)
                | Action::MoveToWorkspace(_)
                | Action::RelocateWorkspace(_)
                | Action::RenameWorkspace
                | Action::KillWorkspace
                | Action::SaveProfile
                | Action::OpenProfilePicker
                | Action::ApplyProfile
                | Action::OpenSessionPicker
                | Action::OpenAgentPicker
                | Action::OpenCollaborators
                | Action::RenameSession
                | Action::NewTemporarySession
                | Action::RequestControl
                | Action::GrantControl
                | Action::ToggleControlTakeover
                | Action::ToggleInputLock
                | Action::KillSession
                | Action::RestartSession
                | Action::TogglePaneSynchronization
        )
}

pub(crate) fn execute_action(ctx: &mut Context<AppRoot>, action: Action) -> Update {
    execute_action_inner(ctx, action, true)
}

pub(crate) fn execute_palette_action(ctx: &mut Context<AppRoot>, action: Action) -> Update {
    execute_action_inner(ctx, action, false)
}

fn execute_action_inner(
    ctx: &mut Context<AppRoot>,
    action: Action,
    confirmations_enabled: bool,
) -> Update {
    if is_blocked_by_scratchpad(&ctx.state, action) {
        return Update::none();
    }
    if closes_settings(action) {
        ctx.state.show_settings = false;
        ctx.state.settings_selected = None;
        ctx.state.pane_padding_editor = None;
    }
    // Any action can flip a dynamic label (a toggle, layout cycling) or the `commands_active`
    // gate (mode/overlay changes). Marking dirty unconditionally here covers both the
    // `Msg::RunAction` path and control-socket `RunAction` requests
    // (`ops::control::run_action`), which call this directly.
    ctx.state.commands_dirty = true;
    // Followers may not mutate the shared layout: intercept before dispatch and nudge toward
    // taking control. Focus, workspace switching, copy/search/palette, and terminal input are all
    // local and fall through.
    if is_layout_mutating(&ctx.state, action) && !ctx.state.scratch_visible {
        if crate::ops::session::nudge_if_follower(ctx) {
            return Update::full();
        }
        // Reshaping a session by hand is the clearest form of using it: from here on it is the
        // user's, not a disposable one the client made for them.
        ctx.state.current_mut().engaged = true;
    }
    if !crate::commands::command_available(action, &ctx.state) {
        return Update::full();
    }
    match action {
        // In the launcher (or any no-client resting state) there is no session to spawn into, and
        // queueing the spawn against a client that will never arrive would look like a hang. Asking
        // for a shell there is the explicit "start a new session" the launcher advertises.
        Action::Spawn | Action::SpawnFloat
            if crate::ops::session::needs_session_for_pty(&ctx.state) =>
        {
            crate::ops::session::start_launcher_shell(ctx)
        }
        Action::Spawn => spawn_pane(ctx),
        Action::SpawnFloat => spawn_floating_pane_at_cursor(ctx),
        Action::RespawnPane => crate::pane::lifecycle::respawn_focused_pane(ctx),
        Action::TogglePaneLogging => toggle_pane_logging(ctx),
        Action::Close => {
            if ctx.state.popup.is_some() {
                return crate::ops::popup::close(ctx);
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
                ctx.state.animation = crate::layout::anim::GeometryAnimation::AxisChange;
            }
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::FlipSplit => {
            toggle_focused_split_axis(&mut ctx.state);
            Update::full()
        }
        Action::AdjustRatio(grow) => {
            adjust_focused_split_ratio(ctx, grow);
            Update::full()
        }
        Action::EnterResizeMode => {
            ctx.state.mode = Mode::Resize;
            ctx.state.show_help = false;
            ctx.state.show_palette = false;
            Update::full()
        }
        Action::ToggleLayout => {
            // The palette's own entry already renders the active layout, so a toast beside it would
            // be the duplicate this whole pass is about removing.
            toggle_layout(ctx, !ctx.state.show_palette);
            Update::full()
        }
        Action::OpenLayoutPicker => crate::ops::layout_picker::open_layout_picker(ctx),
        Action::EnterCopyMode => crate::input::copy_mode::enter(ctx),
        Action::EnterHintMode => crate::ops::hints::enter(ctx),
        Action::ToggleScratchpad => crate::scratchpad::toggle(ctx),
        Action::OpenSearch => open_search(ctx),
        Action::SaveProfile => open_save_profile_prompt(ctx),
        Action::OpenProfilePicker => open_profile_picker(ctx),
        Action::ApplyProfile => crate::ops::profile::open_apply_profile_picker(ctx),
        Action::OpenSessionPicker => crate::ops::session::open_session_picker(ctx),
        Action::OpenAgentPicker => crate::ops::agents::open_agent_picker(ctx),
        Action::OpenCollaborators => crate::ops::session::open_collaborators(ctx),
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
            crate::ops::session::swap_to_fresh_ephemeral(ctx)
        }
        Action::RequestControl => crate::ops::session::request_control(ctx),
        Action::GrantControl => crate::ops::session::grant_control_to_requester(ctx),
        Action::ToggleControlTakeover => crate::ops::session::toggle_control_takeover(ctx),
        Action::ToggleInputLock => crate::ops::session::toggle_input_lock(ctx),
        // One way out. `detach` and `quit` differed only in what they did to sessions as a side
        // effect, which stopped being a property of *how* you leave once a client could hold
        // several sessions: it is decided per session now, by `leave_client`.
        Action::Detach | Action::Quit => crate::ops::exit::leave_client(ctx),
        Action::KillWorkspace => {
            crate::ops::exit::kill_workspace_with_confirmation(ctx, confirmations_enabled)
        }
        Action::KillSession => {
            crate::ops::exit::kill_session_with_confirmation(ctx, confirmations_enabled)
        }
        Action::RestartSession => {
            crate::ops::exit::restart_session_with_confirmation(ctx, confirmations_enabled)
        }
        Action::OpenThemePicker => open_theme_picker(ctx),
        Action::OpenSettings | Action::OpenAppearance => {
            open_settings(ctx, crate::state::SettingsAction::Theme)
        }
        Action::OpenExtensions => {
            ctx.state.overlay_return = None;
            crate::ops::extensions_manager::open(ctx)
        }
        Action::OpenAlerts => open_settings(ctx, crate::state::SettingsAction::ToggleBellUrgency),
        Action::ToggleDoNotDisturb => {
            // A persistent in-session mode earns a workbar chip, not a redundant toast.
            ctx.state.do_not_disturb = !ctx.state.do_not_disturb;
            Update::full()
        }
        Action::TogglePalette => {
            ctx.state.pane_padding_editor = None;
            ctx.state.show_palette = !ctx.state.show_palette;
            ctx.state.command_palette_sidebar_query = false;
            if ctx.state.show_palette {
                ctx.state.show_help = false;
                request_palette_focus(ctx);
            }
            Update::full()
        }
        Action::ToggleHelp => {
            ctx.state.pane_padding_editor = None;
            ctx.state.show_help = !ctx.state.show_help;
            if ctx.state.show_help {
                ctx.state.show_palette = false;
                ctx.state.help_query = TextInput::new("");
                ctx.state.help_tab = crate::state::HelpTab::Global;
                ctx.request_focus(crate::view::help_scroll_key());
            } else {
                ctx.state.help_query = TextInput::new("");
                ctx.state.help_tab = crate::state::HelpTab::Global;
                request_current_pane_focus(ctx);
            }
            Update::full()
        }
        Action::ToggleDevtools => {
            ctx.toggle_devtools();
            Update::none()
        }
        Action::ToggleTitles => crate::ops::preferences::toggle_titles(ctx),
        Action::CycleTitlebar => crate::ops::preferences::cycle_titlebar(ctx),
        Action::ToggleWorkbar => crate::ops::preferences::toggle_workbar(ctx),
        Action::ToggleWorkbarGap => crate::ops::preferences::toggle_workbar_gap(ctx),
        Action::ToggleWorkbarBackground => crate::ops::preferences::toggle_workbar_background(ctx),
        Action::ToggleWorkbarPosition => crate::ops::preferences::toggle_workbar_position(ctx),
        Action::ToggleWorkbarPowerline => crate::ops::preferences::toggle_workbar_powerline(ctx),
        Action::ToggleSidebar => crate::update::sidebar::toggle_visible(ctx),
        Action::ToggleSidebarSplit => crate::update::sidebar::toggle_split(ctx),
        Action::ToggleSidebarGap => crate::ops::preferences::toggle_sidebar_gap(ctx),
        Action::ToggleSidebarBackground => crate::ops::preferences::toggle_sidebar_background(ctx),
        Action::ToggleSidebarBackgroundFollowsTerminal => {
            crate::ops::preferences::toggle_sidebar_background_follows_terminal(ctx)
        }
        Action::FocusSidebar => crate::update::sidebar::focus_body(ctx),
        Action::SidebarNextTab => {
            if ctx.state.sidebar_visible {
                crate::update::sidebar::cycle_tab(ctx, true)
            } else {
                Update::none()
            }
        }
        Action::SidebarPrevTab => {
            if ctx.state.sidebar_visible {
                crate::update::sidebar::cycle_tab(ctx, false)
            } else {
                Update::none()
            }
        }
        Action::ToggleAnimations => crate::ops::preferences::toggle_animations(ctx),
        Action::ToggleNerdIcons => crate::ops::preferences::toggle_nerd_icons(ctx),
        Action::ToggleFocusOnHover => crate::ops::preferences::toggle_focus_on_hover(ctx),
        Action::ToggleHighlightFocusedBackground => {
            crate::ops::preferences::toggle_highlight_focused_background(ctx)
        }
        Action::ToggleHighlightFocusedBorder => {
            crate::ops::preferences::toggle_highlight_focused_border(ctx)
        }
        Action::ToggleHighlightFocusedTitlebar => {
            crate::ops::preferences::toggle_highlight_focused_titlebar(ctx)
        }
        Action::CycleBorderMode => crate::ops::preferences::cycle_border_mode(ctx),
        Action::CycleAlertBorder => crate::ops::preferences::cycle_alert_border(ctx),
        Action::CycleWorkbarAlert => crate::ops::preferences::cycle_workbar_alert(ctx),
        Action::CycleWorkbarAlertPaint => crate::ops::preferences::cycle_workbar_alert_paint(ctx),
        Action::ToggleBackgroundFollowsTerminal => {
            crate::ops::preferences::toggle_background_follows_terminal(ctx)
        }
        Action::CycleBorderStyle => crate::ops::preferences::cycle_border_style(ctx),
        Action::CycleTitleStyle => crate::ops::preferences::cycle_title_style(ctx),
        Action::CycleWorkbarBadgeStyle => crate::ops::preferences::cycle_workbar_badge_style(ctx),
        Action::CycleWorkbarTabStyle => crate::ops::preferences::cycle_workbar_tab_style(ctx),
        Action::CycleWorkbarStyle => crate::ops::preferences::cycle_workbar_style(ctx),
        Action::RunUserCommand(index) => run_user_command(ctx, index),
        Action::RunNamedCommand(index) => run_named_command(ctx, index),
        Action::OpenConfigFile => crate::ops::config::open_config_file(ctx),
        Action::ReloadExtensions => crate::ops::config::reload_extensions(ctx),
        Action::EditScrollback => crate::ops::scrollback::edit_scrollback(ctx),
        Action::CopyLastOutput => crate::ops::last_output::copy_last_output(ctx),
        Action::TogglePaneSynchronization => {
            // No toast: synchronization is a persistent mode that silently multiplies every
            // keystroke across panes, so it needs a permanent `SYNC` chip in the workbar rather
            // than a 3s announcement that leaves the dangerous state unmarked afterwards.
            let workspace = ctx.state.active_workspace_mut();
            workspace.synchronized = !workspace.synchronized;
            Update::full()
        }
    }
}

/// Theme is the one child that records Settings as its parent before hiding it. Every unrelated
/// overlay opener closes Settings immediately so no modal remains interactive underneath another.
fn closes_settings(action: Action) -> bool {
    matches!(
        action,
        Action::TogglePalette
            | Action::ToggleHelp
            | Action::OpenLayoutPicker
            | Action::OpenSearch
            | Action::RenamePane
            | Action::RenameWorkspace
            | Action::RenameSession
            | Action::SaveProfile
            | Action::OpenProfilePicker
            | Action::ApplyProfile
            | Action::OpenSessionPicker
            | Action::OpenAgentPicker
            | Action::OpenCollaborators
            | Action::OpenExtensions
    )
}

fn open_settings(ctx: &mut Context<AppRoot>, selected: crate::state::SettingsAction) -> Update {
    clear_non_settings_overlays(ctx);
    ctx.state.show_settings = true;
    ctx.state.settings_selected = Some(selected);
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

/// A top-level Settings opening abandons any other modal instead of rendering over it.
fn clear_non_settings_overlays(ctx: &mut Context<AppRoot>) {
    crate::ops::theme::cancel_theme_picker(ctx);
    if ctx.state.show_layout_picker || ctx.state.layout_picker.is_some() {
        let _ = crate::ops::layout_picker::cancel_layout_picker(ctx);
    }
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.show_settings = false;
    ctx.state.settings_selected = None;
    ctx.state.pane_padding_editor = None;
    ctx.state.search = None;
    ctx.state.rename = None;
    ctx.state.rename_session = None;
    ctx.state.save_profile_prompt = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.profile_picker_epoch = ctx.state.profile_picker_epoch.wrapping_add(1);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    ctx.state.agent_picker = None;
    ctx.state.extensions = None;
    ctx.state.collaboration = None;
    ctx.state.follow_prompt = None;
    ctx.state.overlay_return = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::lifecycle::find_pane_mut;
    use crate::state::ToastChannel;

    #[test]
    fn layout_mutating_classification_gates_structure_not_navigation() {
        let state = crate::state::State::new(crate::config::Config::default(), Theme::default());
        // Structural / geometry actions are gated for followers.
        for action in [
            Action::Spawn,
            Action::SpawnFloat,
            Action::Close,
            Action::Move(Direction::Left),
            Action::ToggleFloat,
            Action::EnterResizeMode,
            Action::MoveToWorkspace(1),
            Action::KillWorkspace,
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
            Action::KillSession,
            Action::ToggleScratchpad,
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
    fn smart_focus_recognizes_an_editor_behind_a_foreground_wrapper() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        let pane_id = state.focused_pane().expect("default pane is focused");
        let pane = find_pane_mut(&mut state, pane_id).expect("focused pane exists");
        pane.terminal.foreground_program = Some("npm".into());
        pane.terminal.foreground_programs = vec!["npm".into(), "nvim".into()];

        assert!(focused_pane_forwards_navigation(&state, pane_id));
    }

    #[test]
    fn devtools_toggle_is_framework_owned_and_render_neutral() {
        use crate::Msg;
        use tui_lipan::{TestBackend, UpdateLevel};

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                assert_eq!(
                    backend
                        .update_level(Msg::RunAction(Action::ToggleDevtools))
                        .expect("toggle devtools update"),
                    UpdateLevel::None
                );
            })
            .expect("spawn devtools action test thread")
            .join()
            .expect("devtools action test thread completes");
    }

    #[test]
    fn scratchpad_allows_pane_actions_and_blocks_attachment_actions() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        state.scratch_visible = true;

        for action in [
            Action::SwitchWorkspace(1),
            Action::MoveToWorkspace(1),
            Action::RenameWorkspace,
            Action::KillWorkspace,
            Action::SaveProfile,
            Action::OpenSessionPicker,
            Action::KillSession,
        ] {
            assert!(
                is_blocked_by_scratchpad(&state, action),
                "{action:?} should be blocked"
            );
        }
        assert!(!is_blocked_by_scratchpad(&state, Action::ToggleScratchpad));
        assert!(!is_blocked_by_scratchpad(&state, Action::ToggleSidebar));
        for action in [
            Action::Spawn,
            Action::SpawnFloat,
            Action::Close,
            Action::Focus(Direction::Left),
            Action::Move(Direction::Right),
            Action::ToggleFloat,
            Action::ToggleFullscreen,
            Action::RenamePane,
            Action::EnterCopyMode,
            Action::OpenSearch,
            Action::Paste,
            Action::TogglePalette,
            Action::Quit,
        ] {
            assert!(
                !is_blocked_by_scratchpad(&state, action),
                "{action:?} should target scratch or remain globally available"
            );
        }
        assert!(!is_layout_mutating(&state, Action::ToggleScratchpad));
        assert!(!is_layout_mutating(&state, Action::ToggleSidebar));
    }

    #[test]
    fn visible_scratch_workspace_is_the_active_pane_target() {
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.state_mut().scratch_visible = true;
                backend
                    .state_mut()
                    .scratch
                    .panes
                    .push(crate::state::Pane::new(42, 100, FloatRect::default()));
                backend.state_mut().scratch.focused_pane = Some(42);
                assert_eq!(backend.state().focused_pane(), Some(42));
                assert_eq!(backend.state().active_workspace_ref().panes[0].id, 42);
                assert_eq!(backend.state().current().focused_pane, Some(1));
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
                let mut backend = TestBackend::new(AppRoot::default());
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
                backend.state_mut().current_mut().workspaces[1]
                    .panes
                    .push(blocked);

                backend
                    .dispatch(Msg::RunAction(Action::FocusNextBlockedPane))
                    .expect("focus blocked pane");
                assert_eq!(backend.state().current().active_workspace, 1);
                assert_eq!(backend.state().current().focused_pane, Some(2));
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
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(viewport);
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    // The sidebar reserves its columns only once its slide lands, so the canonical
                    // canvas asserted below is the settled one; this test is about the actions and
                    // the commit, not about the frames in between.
                    state.config.animations.sidebar = false;
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
                // Toggling is client-local: it must not write the config default back.
                assert!(!backend.state().config.sidebar.visible);
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
                    backend.state().sidebar.active_tab(),
                    Some(&crate::config::SidebarTabId::new("panes"))
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
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
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
                let before = backend.state_mut().current_mut().workspaces[0].panes.len();

                backend
                    .dispatch(Msg::RunAction(Action::Spawn))
                    .expect("dispatch spawn");

                assert_eq!(
                    backend.state_mut().current_mut().workspaces[0].panes.len(),
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
                assert_eq!(backend.state_mut().current_mut().focused_pane, Some(1));
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

    /// Run `body` on a thread with enough stack for a `TestBackend`-hosted app.
    fn with_backend(body: impl FnOnce(tui_lipan::TestBackend<AppRoot>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || body(tui_lipan::TestBackend::new(AppRoot::default())))
            .expect("spawn toast test thread")
            .join()
            .expect("toast test thread completes");
    }

    /// The slot a content-keyed toast lands in, so a test can look it up the way `notify` does.
    fn content_slot(message: &str) -> crate::pane::pty_events::ToastKey {
        crate::pane::pty_events::ToastKey::Content(
            crate::pane::pty_events::notifications::content_key(message),
        )
    }

    #[test]
    fn theme_picker_remembers_the_highlighted_row_across_its_lifetime() {
        use crate::Msg;

        with_backend(|mut backend| {
            backend
                .dispatch(Msg::RunAction(Action::OpenThemePicker))
                .expect("open theme picker");
            // Opens highlighting the active theme, which drives the palette's initial selection.
            assert!(
                backend.state().theme_picker_selected.is_some(),
                "the picker opens with a remembered selection",
            );

            // Previewing (what highlight changes emit) moves the remembered row, so a subsequent
            // filter preserves this selection instead of snapping back to the active theme.
            backend
                .dispatch(Msg::PreviewTheme(0))
                .expect("preview the first theme");
            assert_eq!(backend.state().theme_picker_selected, Some(0));

            backend
                .dispatch(Msg::CloseThemePicker)
                .expect("close theme picker");
            assert_eq!(
                backend.state().theme_picker_selected,
                None,
                "closing clears the remembered selection",
            );
        });
    }

    #[test]
    fn layout_picker_opens_on_current_and_switches_the_active_workspace() {
        use crate::Msg;
        use crate::state::LayoutKind;

        with_backend(|mut backend| {
            // A fresh backend starts every workspace in the default (dwindle, index 0).
            backend
                .dispatch(Msg::RunAction(Action::OpenLayoutPicker))
                .expect("open layout picker");
            assert!(backend.state().show_layout_picker);
            assert_eq!(
                backend
                    .state()
                    .layout_picker
                    .as_ref()
                    .expect("picker state present")
                    .selected,
                0,
                "the picker highlights the workspace's current layout",
            );

            let grid = LayoutKind::all()
                .iter()
                .position(|kind| *kind == LayoutKind::Grid)
                .expect("grid is a layout");
            backend
                .dispatch(Msg::SelectLayout(grid))
                .expect("select grid layout");

            assert!(
                !backend.state().show_layout_picker,
                "selecting a layout closes the picker",
            );
            assert!(backend.state().layout_picker.is_none());
            let active = backend.state().current().active_workspace;
            assert_eq!(
                backend.state().current().workspaces[active].layout_kind,
                LayoutKind::Grid,
            );
        });
    }

    #[test]
    fn layout_picker_previews_on_highlight_and_reverts_on_cancel() {
        use crate::Msg;
        use crate::state::LayoutKind;

        with_backend(|mut backend| {
            backend
                .dispatch(Msg::RunAction(Action::OpenLayoutPicker))
                .expect("open layout picker");

            let columns = LayoutKind::all()
                .iter()
                .position(|kind| *kind == LayoutKind::Columns)
                .expect("columns is a layout");
            backend
                .dispatch(Msg::LayoutPickerSelect(columns))
                .expect("highlight columns");
            let active = backend.state().current().active_workspace;
            assert_eq!(
                backend.state().current().workspaces[active].layout_kind,
                LayoutKind::Columns,
                "highlighting a row previews that layout live",
            );

            // Cancelling without Enter restores the layout the picker opened on.
            backend
                .dispatch(Msg::CloseLayoutPicker)
                .expect("close layout picker");
            assert!(!backend.state().show_layout_picker);
            assert_eq!(
                backend.state().current().workspaces[active].layout_kind,
                LayoutKind::Dwindle,
            );
        });
    }

    // Both rejections depend only on `session_attached`, which is deterministically false in a
    // fresh backend. Anything gated on a focused pane would race the async first spawn, and
    // anything `command_available` gates (RequestControl, GrantControl, ...) never reaches its
    // handler at all here, since no shared session exists to enable it.
    const NOT_ATTACHED: &str = "Not attached to a session";
    const COMMAND_FAILED: &str = "Command failed\0toast test failure";

    #[test]
    fn a_channel_replaces_its_own_toast_when_the_state_behind_it_changes() {
        use crate::Msg;

        with_backend(|mut backend| {
            let layout_slot = crate::pane::pty_events::ToastKey::Channel(ToastChannel::LayoutMode);

            backend
                .dispatch(Msg::RunAction(Action::ToggleLayout))
                .expect("dispatch first layout cycle");
            let first = backend
                .state()
                .replaceable_toasts
                .get(&layout_slot)
                .expect("the layout toast is tracked")
                .id();

            backend
                .dispatch(Msg::RunAction(Action::ToggleLayout))
                .expect("dispatch second layout cycle");
            let second = backend
                .state()
                .replaceable_toasts
                .get(&layout_slot)
                .expect("the replacement layout toast is tracked")
                .id();

            // A new overlay id, unlike the renew case: the layout name changed, so the channel's
            // previous message is superseded rather than kept alive.
            assert_ne!(
                first, second,
                "changed text in a channel must replace, not renew",
            );
        });
    }

    #[test]
    fn layout_cycle_stays_quiet_when_the_workbar_names_the_layout() {
        use crate::Msg;

        with_backend(|mut backend| {
            backend
                .state_mut()
                .config
                .workbar
                .left
                .push(crate::config::WorkbarItem {
                    segment: crate::config::WorkbarSegment::Layout,
                    color: None,
                });

            backend
                .dispatch(Msg::RunAction(Action::ToggleLayout))
                .expect("dispatch layout cycle");

            let layout_slot = crate::pane::pty_events::ToastKey::Channel(ToastChannel::LayoutMode);
            assert!(
                !backend
                    .state()
                    .replaceable_toasts
                    .contains_key(&layout_slot),
                "the workbar already reports the active layout"
            );
        });
    }

    #[test]
    fn an_identical_toast_renews_the_one_already_on_screen() {
        use crate::Msg;

        with_backend(|mut backend| {
            backend
                .dispatch(Msg::RunAction(Action::KillSession))
                .expect("dispatch first kill-session");
            let first = backend
                .state()
                .replaceable_toasts
                .get(&content_slot(NOT_ATTACHED))
                .expect("the rejection toast is tracked")
                .id();

            backend
                .dispatch(Msg::RunAction(Action::KillSession))
                .expect("dispatch repeat kill-session");
            let second = backend
                .state()
                .replaceable_toasts
                .get(&content_slot(NOT_ATTACHED))
                .expect("the rejection toast is still tracked")
                .id();

            // Same overlay id means the toast was renewed in place rather than dismissed and
            // re-pushed, which is what keeps a repeat from blinking or jumping position.
            assert_eq!(first, second, "a repeat must renew, not stack");
        });
    }

    #[test]
    fn different_messages_occupy_independent_slots() {
        use crate::Msg;

        with_backend(|mut backend| {
            backend
                .dispatch(Msg::RunAction(Action::KillSession))
                .expect("dispatch kill-session");
            let output_toast = backend
                .state()
                .replaceable_toasts
                .get(&content_slot(NOT_ATTACHED))
                .expect("the rejection toast is tracked")
                .id();

            backend
                .dispatch(Msg::UserCommandFailed {
                    message: "toast test failure".to_string(),
                })
                .expect("dispatch command failure");

            // Asserted per slot rather than on the map size: startup spawns can raise their own
            // toasts asynchronously, and a total count would make this test depend on them.
            assert_eq!(
                backend
                    .state()
                    .replaceable_toasts
                    .get(&content_slot(NOT_ATTACHED))
                    .map(crate::pane::pty_events::TrackedToast::id),
                Some(output_toast),
                "an unrelated message must not disturb another slot",
            );
            assert!(
                backend
                    .state()
                    .replaceable_toasts
                    .contains_key(&content_slot(COMMAND_FAILED)),
                "the second message gets its own slot",
            );
        });
    }

    #[test]
    fn an_armed_confirmation_never_dedups_against_its_own_repeat() {
        use crate::Msg;

        with_backend(|mut backend| {
            // Kill-pane arms on the first press and *executes* on the second, so the two presses are
            // different events. Collapsing them would misreport the confirm window as still open,
            // which is why confirm toasts are pushed directly and never enter the tracking map.
            // Asserted against that one slot rather than the whole map, so an unrelated async
            // toast (a failed startup spawn, say) cannot make this pass or fail by accident.
            backend
                .dispatch(Msg::RunAction(Action::Close))
                .expect("dispatch first close-pane");
            backend
                .dispatch(Msg::RunAction(Action::Close))
                .expect("dispatch repeat close-pane");
            assert!(
                !backend
                    .state()
                    .replaceable_toasts
                    .contains_key(&content_slot("Press again to kill pane")),
                "confirm toasts must never be tracked for de-duplication",
            );
        });
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
                let mut backend = TestBackend::new(AppRoot::default());
                backend.state_mut().show_settings = true;
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
                            extension: None,
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
