use tui_lipan::prelude::*;

use crate::input::{self, Action};
use crate::search_ops::open_search;
use crate::state::Mode;
use crate::theme_ops::{open_theme_picker, select_theme};
use crate::{
    HyprmuxApp, Msg, adjust_focused_split_ratio, close_focused_pane, focus_in_direction,
    move_focused_in_direction, move_focused_to_workspace, request_current_pane_focus,
    request_pane_focus, spawn_pane, switch_workspace, toggle_focused_split_axis, toggle_fullscreen,
    toggle_layout, toggle_tiling,
};

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
        Action::OpenSearch => open_search(ctx),
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
    }
}

pub(crate) fn register_commands(ctx: &mut Context<HyprmuxApp>) {
    let registry = ctx.command_registry();
    for binding in input::command_bindings()
        .into_iter()
        .filter(|binding| binding.palette)
    {
        let action = binding.action;
        let link = ctx.link().clone();
        registry.register(
            CommandEntry::builder(binding.id)
                .label(binding.label)
                .category(binding.category)
                .keybinding(binding.keys)
                .handler(Callback::new(move |_| link.send(Msg::RunAction(action))))
                .build(),
        );
    }
}
