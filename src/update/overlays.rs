use tui_lipan::prelude::*;

use crate::actions::{execute_action, execute_palette_action};
use crate::input::Action;
use crate::ops::focus::{
    request_current_pane_focus, request_rename_focus, request_rename_session_focus,
    request_search_focus,
};
use crate::ops::theme::{
    cancel_theme_picker, preview_theme as preview, select_theme as select, theme_tick as tick,
};
use crate::pty_events::error_toast;
use crate::{HyprmuxApp, Msg};

fn valid_padding_text(value: &str) -> bool {
    value.is_empty()
        || (value.len() == 1
            && value.as_bytes()[0].is_ascii_digit()
            && u16::from(value.as_bytes()[0] - b'0') <= crate::config::MAX_PANE_PADDING)
}

fn padding_value(value: &str) -> Option<u16> {
    valid_padding_text(value)
        .then(|| value.parse().ok())
        .flatten()
}

fn padding_error(ctx: &mut Context<HyprmuxApp>) {
    ctx.toast().push(error_toast(
        &ctx.state.theme,
        "Invalid padding",
        "Enter one digit",
    ));
}

pub(super) fn command_link_ready(ctx: &mut Context<HyprmuxApp>, link: CommandLink<Msg>) -> Update {
    ctx.state.command_link = Some(link);
    crate::update::sidebar::request_sessions_refresh(ctx);
    crate::update::sidebar::request_command_poll(ctx);
    Update::none()
}

pub(super) fn hangup(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::exit::detach_on_hangup(ctx)
}

pub(super) fn run_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update {
    // Return before overlay cleanup and focus restoration: blocked actions must leave the
    // scratchpad terminal as the focused layer.
    if crate::actions::is_blocked_by_scratchpad(&ctx.state, action) {
        return Update::none();
    }
    if matches!(
        action,
        Action::OpenAppearance | Action::TogglePalette | Action::ToggleHelp
    ) {
        ctx.state.pane_padding_editor = None;
    }
    let cycle_layout_in_palette = matches!(action, Action::ToggleLayout) && ctx.state.show_palette;
    let from_palette = ctx.state.show_palette;
    if !cycle_layout_in_palette {
        ctx.state.show_palette = false;
    }
    let update = if from_palette {
        execute_palette_action(ctx, action)
    } else {
        execute_action(ctx, action)
    };
    match action {
        Action::OpenSearch => request_search_focus(ctx),
        Action::RenamePane => request_rename_focus(ctx),
        Action::RenameWorkspace | Action::RenameSession => request_rename_session_focus(ctx),
        Action::OpenAppearance | Action::OpenThemePicker => {}
        Action::SaveProfile
        | Action::OpenProfilePicker
        | Action::ApplyProfile
        | Action::OpenSessionPicker
        | Action::OpenClientList => {}
        // The scratchpad manages its own focus (the scratch terminal on show, the previously
        // focused pane on hide); don't override it.
        Action::ToggleScratchpad => {}
        Action::ToggleLayout if cycle_layout_in_palette => {}
        _ => request_current_pane_focus(ctx),
    }
    update
}

pub(super) fn close_palette(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_palette = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn close_help(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_help = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn close_appearance(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_appearance = false;
    ctx.state.pane_padding_editor = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn appearance_activate(
    ctx: &mut Context<HyprmuxApp>,
    action: crate::state::AppearanceAction,
) -> Update {
    // A greyed row (its parent feature is off) is inert: keep the overlay open and focused but
    // change nothing. Otherwise dispatch the row's underlying action.
    if action.disabled_reason(&ctx.state.config.pane).is_some() {
        ctx.request_focus(crate::view::appearance_palette_key());
    } else {
        match action {
            crate::state::AppearanceAction::Theme => {
                execute_action(ctx, Action::OpenThemePicker);
            }
            crate::state::AppearanceAction::EditPadding => {
                ctx.state.pane_padding_editor = Some(crate::state::PanePaddingEditorState::new(
                    ctx.state.config.pane.padding,
                ));
                ctx.request_focus(crate::view::pane_padding_vertical_key());
            }
            crate::state::AppearanceAction::ToggleTitles => {
                execute_action(ctx, Action::ToggleTitles);
            }
            crate::state::AppearanceAction::ToggleWorkbar => {
                execute_action(ctx, Action::ToggleWorkbar);
            }
            crate::state::AppearanceAction::ToggleWorkbarGap => {
                execute_action(ctx, Action::ToggleWorkbarGap);
            }
            crate::state::AppearanceAction::ToggleWorkbarPosition => {
                execute_action(ctx, Action::ToggleWorkbarPosition);
            }
            crate::state::AppearanceAction::ToggleWorkbarPowerline => {
                execute_action(ctx, Action::ToggleWorkbarPowerline);
            }
            crate::state::AppearanceAction::ToggleAnimations => {
                execute_action(ctx, Action::ToggleAnimations);
            }
            crate::state::AppearanceAction::ToggleHighlightFocusedBackground => {
                execute_action(ctx, Action::ToggleHighlightFocusedBackground);
            }
            crate::state::AppearanceAction::ToggleHighlightFocusedBorder => {
                execute_action(ctx, Action::ToggleHighlightFocusedBorder);
            }
            crate::state::AppearanceAction::ToggleBorderMerge => {
                execute_action(ctx, Action::ToggleBorderMerge);
            }
            crate::state::AppearanceAction::ToggleBackgroundFollowsTerminal => {
                execute_action(ctx, Action::ToggleBackgroundFollowsTerminal);
            }
            crate::state::AppearanceAction::CycleBorderStyle => {
                execute_action(ctx, Action::CycleBorderStyle);
            }
            crate::state::AppearanceAction::CycleTitleStyle => {
                execute_action(ctx, Action::CycleTitleStyle);
            }
            crate::state::AppearanceAction::CycleWorkbarBadgeStyle => {
                execute_action(ctx, Action::CycleWorkbarBadgeStyle);
            }
            crate::state::AppearanceAction::CycleWorkbarTabStyle => {
                execute_action(ctx, Action::CycleWorkbarTabStyle);
            }
            crate::state::AppearanceAction::CycleWorkbarStyle => {
                execute_action(ctx, Action::CycleWorkbarStyle);
            }
        };
        if !matches!(
            action,
            crate::state::AppearanceAction::Theme | crate::state::AppearanceAction::EditPadding
        ) {
            ctx.state.show_appearance = true;
            ctx.request_focus(crate::view::appearance_palette_key());
        }
    }
    Update::full()
}

pub(super) fn close_pane_padding_editor(ctx: &mut Context<HyprmuxApp>) -> Update {
    if ctx.state.pane_padding_editor.is_none() {
        return Update::none();
    }
    ctx.state.pane_padding_editor = None;
    if ctx.state.show_appearance {
        ctx.request_focus(crate::view::appearance_palette_key());
    }
    Update::full()
}

pub(super) fn pane_padding_vertical_changed(
    ctx: &mut Context<HyprmuxApp>,
    event: InputEvent,
) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
        return Update::none();
    };
    if valid_padding_text(&event.value) {
        event.apply_to(&mut editor.vertical);
    }
    ctx.request_focus(crate::view::pane_padding_vertical_key());
    Update::full()
}

pub(super) fn pane_padding_horizontal_changed(
    ctx: &mut Context<HyprmuxApp>,
    event: InputEvent,
) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
        return Update::none();
    };
    if valid_padding_text(&event.value) {
        event.apply_to(&mut editor.horizontal);
    }
    ctx.request_focus(crate::view::pane_padding_horizontal_key());
    Update::full()
}

pub(super) fn advance_pane_padding(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_ref() else {
        return Update::none();
    };
    if padding_value(editor.vertical.text()).is_some() {
        ctx.request_focus(crate::view::pane_padding_horizontal_key());
    } else {
        padding_error(ctx);
        ctx.request_focus(crate::view::pane_padding_vertical_key());
    }
    Update::full()
}

pub(super) fn submit_pane_padding(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_ref() else {
        return Update::none();
    };
    let Some(vertical) = padding_value(editor.vertical.text()) else {
        padding_error(ctx);
        ctx.request_focus(crate::view::pane_padding_vertical_key());
        return Update::full();
    };
    let Some(horizontal) = padding_value(editor.horizontal.text()) else {
        padding_error(ctx);
        ctx.request_focus(crate::view::pane_padding_horizontal_key());
        return Update::full();
    };
    ctx.state.config.pane.padding = (vertical, horizontal, vertical, horizontal);
    if let Err(error) = crate::config::persist_pane_padding(vertical, horizontal) {
        ctx.toast()
            .push(error_toast(&ctx.state.theme, "Padding not saved", error));
    }
    ctx.state.pane_padding_editor = None;
    if ctx.state.show_appearance {
        ctx.request_focus(crate::view::appearance_palette_key());
    }
    Update::full()
}

pub(super) fn close_theme_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    cancel_theme_picker(ctx);
    request_current_pane_focus(ctx);
    Update::full()
}
pub(super) fn preview_theme(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    preview(ctx, index)
}
pub(super) fn select_theme(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    select(ctx, index)
}
pub(super) fn theme_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    tick(ctx)
}
pub(super) fn config_file_changed(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::config::config_file_changed(ctx)
}

pub(super) fn workbar_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    // Reschedule only while a clock segment is configured.
    if !ctx.state.config.workbar.has_clock() {
        return Update::none();
    }
    let command = crate::schedule_workbar_tick();
    // The tick is 1s, but `clock_format` defaults to minute resolution. Comparing against the text
    // the view last rendered turns ~59 of every 60 ticks into a bare reschedule instead of a
    // full-app render that would redraw an identical badge.
    let current = format!(
        " {} ",
        chrono::Local::now().format(&ctx.state.config.workbar.clock_format)
    );
    let changed = ctx
        .state
        .last_clock_text
        .borrow()
        .as_ref()
        .is_none_or(|rendered| *rendered != current);
    if changed {
        Update::with_command(command)
    } else {
        Update::command_only(command)
    }
}

pub(super) fn workbar_command_output(
    ctx: &mut Context<HyprmuxApp>,
    command: String,
    output: String,
) -> Update {
    // Command segments re-run on a timer and usually report the same string (a battery percentage,
    // a branch name). Only the runs that actually change the badge are worth a frame.
    if ctx.state.workbar_command_output.get(&command) == Some(&output) {
        return Update::none();
    }
    ctx.state.workbar_command_output.insert(command, output);
    Update::full()
}

pub(super) fn theme_error(ctx: &mut Context<HyprmuxApp>, message: String) -> Update {
    ctx.toast().push(error_toast(
        &ctx.state.theme,
        "Theme reload failed",
        message,
    ));
    Update::full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_input_accepts_empty_or_one_ascii_digit_in_range() {
        assert!(valid_padding_text(""));
        assert!(valid_padding_text("8"));
        assert!(!valid_padding_text("9"));
        assert!(!valid_padding_text("12"));
        assert!(!valid_padding_text("８"));
    }
}
