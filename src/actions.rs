use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::UserCommandAction;
use crate::focus_ops::{
    cycle_focus_in_tiled_order, focus_in_direction, move_focused_to_workspace,
    promote_focused_to_master, relocate_active_workspace, request_current_pane_focus,
    request_pane_focus, switch_workspace,
};
use crate::identity_ops::open_rename_pane;
use crate::input::Action;
use crate::pane_lifecycle::{
    close_focused_pane, find_pane_mut, spawn_pane, spawn_pane_in_workspace,
};
use crate::profile_ops::{open_profile_picker, open_save_profile_prompt};
use crate::resize_move_ops::{
    adjust_focused_split_ratio, move_focused_in_direction, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};
use crate::search_ops::open_search;
use crate::state::{Mode, PaneIdentity};
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
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && let Err(err) = pane.terminal.send_bytes(&wrap_bracketed_paste(&text))
    {
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
            if let Some(pane) = find_pane_mut(&mut ctx.state, id)
                && let Err(err) = pane.terminal.send_bytes(text.as_bytes())
            {
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

fn persist_pane_toggle(ctx: &mut Context<HyprmuxApp>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_pane_flag(key, value) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Preference not saved",
            err,
        ));
    }
}

pub(crate) fn execute_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update {
    match action {
        Action::Spawn => spawn_pane(ctx),
        Action::Close => close_focused_pane(ctx),
        Action::Focus(direction) => {
            let viewport = ctx.viewport();
            if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
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
        Action::DetachSession => crate::session_ops::detach_current_session(ctx),
        Action::OpenThemePicker => open_theme_picker(ctx),
        Action::TogglePalette => {
            ctx.state.show_palette = !ctx.state.show_palette;
            if ctx.state.show_palette {
                ctx.state.show_help = false;
            }
            Update::full()
        }
        Action::ToggleHelp => {
            ctx.state.show_help = !ctx.state.show_help;
            if ctx.state.show_help {
                ctx.state.show_palette = false;
            }
            Update::full()
        }
        Action::ToggleTitles => {
            ctx.state.config.pane.show_titles = !ctx.state.config.pane.show_titles;
            persist_pane_toggle(ctx, "show_titles", ctx.state.config.pane.show_titles);
            Update::full()
        }
        Action::ToggleTopBar => {
            ctx.state.config.pane.show_top_bar = !ctx.state.config.pane.show_top_bar;
            persist_pane_toggle(ctx, "show_top_bar", ctx.state.config.pane.show_top_bar);
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
        Action::RunUserCommand(index) => run_user_command(ctx, index),
        Action::ReloadConfig => crate::config_ops::reload_config(ctx),
        Action::OpenConfigFile => crate::config_ops::open_config_file(ctx),
        Action::TogglePaneSynchronization => {
            let synchronized = {
                let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
                workspace.synchronized = !workspace.synchronized;
                workspace.synchronized
            };
            ctx.toast()
                .push(crate::pty_events::info_toast(if synchronized {
                    "Sync on"
                } else {
                    "Sync off"
                }));
            Update::full()
        }
    }
}
