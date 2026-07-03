use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::focus_ops::{
    cycle_focus_in_tiled_order, focus_in_direction, move_focused_to_workspace,
    promote_focused_to_master, request_current_pane_focus, request_pane_focus, switch_workspace,
};
use crate::identity_ops::open_rename_pane;
use crate::input::Action;
use crate::pane_lifecycle::{close_focused_pane, spawn_pane};
use crate::profile_ops::{open_profile_picker, open_save_profile_prompt};
use crate::resize_move_ops::{
    adjust_focused_split_ratio, move_focused_in_direction, swap_focused_in_direction,
    toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling,
};
use crate::search_ops::open_search;
use crate::state::Mode;
use crate::theme_ops::{open_theme_picker, select_theme};

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
        Action::ToggleFloat => {
            toggle_tiling(ctx);
            Update::full()
        }
        Action::ToggleFullscreen => toggle_fullscreen(ctx),
        Action::RenamePane => open_rename_pane(ctx),
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
            toggle_layout(ctx);
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
        Action::SelectTheme(preset) => {
            select_theme(ctx, preset);
            Update::full()
        }
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
            ctx.state.show_titles = !ctx.state.show_titles;
            Update::full()
        }
        Action::ToggleFocusOnHover => {
            ctx.state.config.pane.focus_on_hover = !ctx.state.config.pane.focus_on_hover;
            Update::full()
        }
        Action::TogglePaneSynchronization => {
            let synchronized = {
                let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
                workspace.synchronized = !workspace.synchronized;
                workspace.synchronized
            };
            ctx.toast()
                .push(crate::pty_events::info_toast(if synchronized {
                    "Pane synchronization enabled"
                } else {
                    "Pane synchronization disabled"
                }));
            Update::full()
        }
    }
}
