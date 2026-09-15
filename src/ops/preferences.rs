use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::PaneConfig;
use crate::state::{
    PaneBorderStyle, ToastChannel, cap_style_id, next_badge_cap_style, next_cap_style,
};

fn persist_pane_toggle(ctx: &mut Context<AppRoot>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_pane_flag(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_sidebar_toggle(ctx: &mut Context<AppRoot>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_sidebar_flag(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_sidebar_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_sidebar_string(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
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

fn persist_animation_toggle(ctx: &mut Context<AppRoot>, key: &str, value: bool) {
    if let Err(err) = crate::config::persist_animation_flag(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_nerd_icons_toggle(ctx: &mut Context<AppRoot>, value: bool) {
    if let Err(err) = crate::config::persist_top_level_flag("nerd_icons", value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_pane_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_pane_string(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_workbar_alert_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_workbar_alert_string(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

pub(crate) fn toggle_titles(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, show_titles)
}

pub(crate) fn cycle_titlebar(ctx: &mut Context<AppRoot>) -> Update {
    let next = ctx.state.config.pane.titlebar.next();
    ctx.state.config.pane.titlebar = next;
    persist_pane_string_or_toast(ctx, "titlebar", next.id());
    Update::full()
}

pub(crate) fn toggle_workbar(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, show_workbar)
}

pub(crate) fn toggle_workbar_gap(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, workbar_gap)
}

pub(crate) fn toggle_workbar_background(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, workbar_background)
}

pub(crate) fn toggle_sidebar_gap(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.config.sidebar.gap = !ctx.state.config.sidebar.gap;
    persist_sidebar_toggle(ctx, "gap", ctx.state.config.sidebar.gap);
    Update::full()
}

pub(crate) fn toggle_sidebar_background(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.config.sidebar.background = !ctx.state.config.sidebar.background;
    persist_sidebar_toggle(ctx, "background", ctx.state.config.sidebar.background);
    Update::full()
}

pub(crate) fn toggle_sidebar_background_follows_terminal(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.config.sidebar.background_follows_terminal =
        !ctx.state.config.sidebar.background_follows_terminal;
    persist_sidebar_toggle(
        ctx,
        "background_follows_terminal",
        ctx.state.config.sidebar.background_follows_terminal,
    );
    Update::full()
}

pub(crate) fn cycle_sidebar_tab_style(ctx: &mut Context<AppRoot>) -> Update {
    let next = next_badge_cap_style(ctx.state.config.sidebar.tab_style);
    ctx.state.config.sidebar.tab_style = next;
    persist_sidebar_string_or_toast(ctx, "tab_style", cap_style_id(next));
    Update::full()
}

pub(crate) fn toggle_workbar_position(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, workbar_at_bottom)
}

pub(crate) fn toggle_workbar_powerline(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, workbar_powerline)
}

pub(crate) fn toggle_animations(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.config.animations.enabled = !ctx.state.config.animations.enabled;
    persist_animation_toggle(ctx, "enabled", ctx.state.config.animations.enabled);
    Update::full()
}

pub(crate) fn toggle_nerd_icons(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.config.nerd_icons = !ctx.state.config.nerd_icons;
    persist_nerd_icons_toggle(ctx, ctx.state.config.nerd_icons);
    Update::full()
}

pub(crate) fn toggle_focus_on_hover(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, focus_on_hover)
}

pub(crate) fn toggle_highlight_focused_background(ctx: &mut Context<AppRoot>) -> Update {
    let update = toggle_pane_flag!(ctx, highlight_focused_background);
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
    update
}

pub(crate) fn toggle_highlight_focused_border(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, highlight_focused_border)
}

pub(crate) fn toggle_highlight_focused_titlebar(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, highlight_focused_titlebar)
}

pub(crate) fn cycle_border_mode(ctx: &mut Context<AppRoot>) -> Update {
    let next = ctx.state.config.pane.border_mode.next();
    ctx.state.config.pane.border_mode = next;
    persist_pane_string_or_toast(ctx, "border_mode", next.id());
    Update::full()
}

pub(crate) fn cycle_alert_border(ctx: &mut Context<AppRoot>) -> Update {
    let next = ctx.state.config.pane.alert_border.next();
    ctx.state.config.pane.alert_border = next;
    persist_pane_string_or_toast(ctx, "alert_border", next.id());
    Update::full()
}

pub(crate) fn cycle_workbar_alert(ctx: &mut Context<AppRoot>) -> Update {
    let next = ctx.state.config.workbar.alert.mode.next();
    ctx.state.config.workbar.alert.mode = next;
    persist_workbar_alert_string_or_toast(ctx, "mode", next.id());
    Update::full()
}

pub(crate) fn cycle_workbar_alert_paint(ctx: &mut Context<AppRoot>) -> Update {
    let next = ctx.state.config.workbar.alert.paint.next();
    ctx.state.config.workbar.alert.paint = next;
    persist_workbar_alert_string_or_toast(ctx, "paint", next.id());
    Update::full()
}

pub(crate) fn toggle_background_follows_terminal(ctx: &mut Context<AppRoot>) -> Update {
    toggle_pane_flag!(ctx, background_follows_terminal);
    crate::ops::theme::reapply_active_theme(ctx)
}

pub(crate) fn cycle_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(ctx, |pane| &mut pane.border_style, "border_style", false)
}

pub(crate) fn reverse_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(ctx, |pane| &mut pane.border_style, "border_style", true)
}

pub(crate) fn cycle_float_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.float_border_style,
        "float_border_style",
        false,
    )
}

pub(crate) fn reverse_float_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.float_border_style,
        "float_border_style",
        true,
    )
}

pub(crate) fn cycle_scratch_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.scratch_border_style,
        "scratch_border_style",
        false,
    )
}

pub(crate) fn reverse_scratch_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.scratch_border_style,
        "scratch_border_style",
        true,
    )
}

pub(crate) fn cycle_fullscreen_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.fullscreen_border_style,
        "fullscreen_border_style",
        false,
    )
}

pub(crate) fn reverse_fullscreen_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.fullscreen_border_style,
        "fullscreen_border_style",
        true,
    )
}

pub(crate) fn cycle_picker_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.picker_border_style,
        "picker_border_style",
        false,
    )
}

pub(crate) fn reverse_picker_border_style(ctx: &mut Context<AppRoot>) -> Update {
    step_pane_border_style(
        ctx,
        |pane| &mut pane.picker_border_style,
        "picker_border_style",
        true,
    )
}

fn step_pane_border_style(
    ctx: &mut Context<AppRoot>,
    select: fn(&mut PaneConfig) -> &mut PaneBorderStyle,
    key: &str,
    reverse: bool,
) -> Update {
    let next = {
        let current = *select(&mut ctx.state.config.pane);
        if reverse {
            current.prev()
        } else {
            current.next()
        }
    };
    *select(&mut ctx.state.config.pane) = next;
    persist_pane_string_or_toast(ctx, key, next.id());
    Update::full()
}

pub(crate) fn cycle_title_style(ctx: &mut Context<AppRoot>) -> Update {
    let next = next_cap_style(ctx.state.config.pane.title_style);
    ctx.state.config.pane.title_style = next;
    persist_pane_string_or_toast(ctx, "title_style", cap_style_id(next));
    Update::full()
}

pub(crate) fn cycle_workbar_badge_style(ctx: &mut Context<AppRoot>) -> Update {
    let next = next_badge_cap_style(ctx.state.config.pane.workbar_badge_style);
    ctx.state.config.pane.workbar_badge_style = next;
    persist_pane_string_or_toast(ctx, "workbar_badge_style", cap_style_id(next));
    Update::full()
}

pub(crate) fn cycle_workbar_tab_style(ctx: &mut Context<AppRoot>) -> Update {
    let next = next_badge_cap_style(ctx.state.config.pane.workbar_tab_style);
    ctx.state.config.pane.workbar_tab_style = next;
    persist_pane_string_or_toast(ctx, "workbar_tab_style", cap_style_id(next));
    Update::full()
}

pub(crate) fn cycle_workbar_style(ctx: &mut Context<AppRoot>) -> Update {
    let next = next_cap_style(ctx.state.config.pane.workbar_style);
    ctx.state.config.pane.workbar_style = next;
    persist_pane_string_or_toast(ctx, "workbar_style", cap_style_id(next));
    Update::full()
}
