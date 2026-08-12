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
use crate::{AppRoot, Msg};

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

fn padding_error(ctx: &mut Context<AppRoot>) {
    crate::pty_events::notify_error(ctx, "Invalid padding", "Enter one digit");
}

pub(super) fn command_link_ready(ctx: &mut Context<AppRoot>, link: CommandLink<Msg>) -> Update {
    ctx.state.command_link = Some(link);
    crate::update::sidebar::request_sessions_refresh(ctx);
    crate::update::sidebar::request_command_poll(ctx);
    crate::update::workbar::request_command_polls(ctx);
    Update::none()
}

pub(super) fn hangup(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::exit::detach_on_hangup(ctx)
}

pub(super) fn run_action(ctx: &mut Context<AppRoot>, action: Action) -> Update {
    // Return before overlay cleanup and focus restoration: blocked actions must leave the
    // scratchpad terminal as the focused layer.
    if crate::actions::is_blocked_by_scratchpad(&ctx.state, action) {
        return Update::none();
    }
    if matches!(
        action,
        Action::OpenSettings
            | Action::OpenAppearance
            | Action::OpenAlerts
            | Action::TogglePalette
            | Action::ToggleHelp
    ) {
        ctx.state.pane_padding_editor = None;
    }
    let cycle_layout_in_palette = matches!(action, Action::ToggleLayout) && ctx.state.show_palette;
    let from_palette = ctx.state.show_palette;
    if !cycle_layout_in_palette {
        ctx.state.show_palette = false;
        ctx.state.command_palette_sidebar_query = false;
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
        Action::OpenSettings
        | Action::OpenAppearance
        | Action::OpenAlerts
        | Action::OpenThemePicker
        | Action::OpenLayoutPicker => {}
        Action::SaveProfile
        | Action::OpenProfilePicker
        | Action::ApplyProfile
        | Action::OpenSessionPicker
        | Action::OpenCollaborators => {}
        // The scratchpad manages its own focus (the scratch terminal on show, the previously
        // focused pane on hide); don't override it.
        Action::ToggleScratchpad => {}
        // `focus-sidebar` exists to move focus off the pane. Falling through to the catch-all would
        // hand it straight back and make the action look dead.
        Action::FocusSidebar => {}
        Action::ToggleLayout if cycle_layout_in_palette => {}
        _ => request_current_pane_focus(ctx),
    }
    update
}

pub(super) fn close_palette(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_palette = false;
    ctx.state.command_palette_sidebar_query = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn command_palette_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    let sidebar_query = query.trim().eq_ignore_ascii_case("sidebar");
    if ctx.state.command_palette_sidebar_query == sidebar_query {
        return Update::none();
    }
    ctx.state.command_palette_sidebar_query = sidebar_query;
    Update::full()
}

pub(super) fn close_help(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_help = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn close_settings(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_settings = false;
    ctx.state.settings_selected = None;
    ctx.state.pane_padding_editor = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn settings_select(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    ctx.state.settings_selected = Some(action);
    // The key interceptor is rebuilt from this live selection: Theme/Padding leave Left/Right to
    // the search caret, while value rows consume them for stepping.
    Update::full()
}

pub(super) fn settings_activate(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    settings_activate_dir(ctx, action, false)
}

pub(super) fn settings_step(ctx: &mut Context<AppRoot>, reverse: bool) -> Update {
    let Some(action) = ctx.state.settings_selected else {
        return Update::none();
    };
    if !action.steps_horizontally() {
        return Update::none();
    }
    settings_activate_dir(ctx, action, reverse)
}

fn settings_activate_dir(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
    reverse: bool,
) -> Update {
    if action.disabled_reason(&ctx.state.config).is_some() {
        ctx.request_focus(crate::view::settings_palette_key());
        return Update::full();
    }
    use crate::state::SettingsAction::*;
    let mut persisted: Option<(&str, &str, bool)> = None;
    match action {
        Theme => {
            execute_action(ctx, Action::OpenThemePicker);
        }
        EditPadding => {
            ctx.state.pane_padding_editor = Some(crate::state::PanePaddingEditorState::new(
                ctx.state.config.pane.padding,
            ));
            ctx.request_focus(crate::view::pane_padding_vertical_key());
        }
        ToggleTitles => {
            execute_action(ctx, Action::ToggleTitles);
        }
        CycleTitlebar if reverse => {
            let value = ctx.state.config.pane.titlebar.prev();
            ctx.state.config.pane.titlebar = value;
            persist_pane_string_or_toast(ctx, "titlebar", value.id());
        }
        CycleTitlebar => {
            execute_action(ctx, Action::CycleTitlebar);
        }
        CycleTitleStyle if reverse => {
            let value = crate::state::prev_cap_style(ctx.state.config.pane.title_style);
            ctx.state.config.pane.title_style = value;
            persist_pane_string_or_toast(ctx, "title_style", crate::state::cap_style_id(value));
        }
        CycleTitleStyle => {
            execute_action(ctx, Action::CycleTitleStyle);
        }
        ToggleWorkbar => {
            execute_action(ctx, Action::ToggleWorkbar);
        }
        ToggleWorkbarPosition => {
            execute_action(ctx, Action::ToggleWorkbarPosition);
        }
        ToggleWorkbarGap => {
            execute_action(ctx, Action::ToggleWorkbarGap);
        }
        CycleWorkbarStyle if reverse => {
            let value = crate::state::prev_cap_style(ctx.state.config.pane.workbar_style);
            ctx.state.config.pane.workbar_style = value;
            persist_pane_string_or_toast(ctx, "workbar_style", crate::state::cap_style_id(value));
        }
        CycleWorkbarStyle => {
            execute_action(ctx, Action::CycleWorkbarStyle);
        }
        CycleWorkbarBadgeStyle if reverse => {
            let value =
                crate::state::prev_badge_cap_style(ctx.state.config.pane.workbar_badge_style);
            ctx.state.config.pane.workbar_badge_style = value;
            persist_pane_string_or_toast(
                ctx,
                "workbar_badge_style",
                crate::state::cap_style_id(value),
            );
        }
        CycleWorkbarBadgeStyle => {
            execute_action(ctx, Action::CycleWorkbarBadgeStyle);
        }
        CycleWorkbarTabStyle if reverse => {
            let value = crate::state::prev_badge_cap_style(ctx.state.config.pane.workbar_tab_style);
            ctx.state.config.pane.workbar_tab_style = value;
            persist_pane_string_or_toast(
                ctx,
                "workbar_tab_style",
                crate::state::cap_style_id(value),
            );
        }
        CycleWorkbarTabStyle => {
            execute_action(ctx, Action::CycleWorkbarTabStyle);
        }
        ToggleWorkbarPowerline => {
            execute_action(ctx, Action::ToggleWorkbarPowerline);
        }
        ToggleAnimations => {
            execute_action(ctx, Action::ToggleAnimations);
        }
        ToggleFocusOnHover => {
            execute_action(ctx, Action::ToggleFocusOnHover);
        }
        ToggleBackgroundFollowsTerminal => {
            execute_action(ctx, Action::ToggleBackgroundFollowsTerminal);
        }
        ToggleHighlightFocusedBackground => {
            execute_action(ctx, Action::ToggleHighlightFocusedBackground);
        }
        ToggleHighlightFocusedBorder => {
            execute_action(ctx, Action::ToggleHighlightFocusedBorder);
        }
        ToggleHighlightFocusedTitlebar => {
            execute_action(ctx, Action::ToggleHighlightFocusedTitlebar);
        }
        CycleBorderMode if reverse => {
            let value = ctx.state.config.pane.border_mode.prev();
            ctx.state.config.pane.border_mode = value;
            persist_pane_string_or_toast(ctx, "border_mode", value.id());
        }
        CycleBorderMode => {
            execute_action(ctx, Action::CycleBorderMode);
        }
        CycleBorderStyle if reverse => {
            let value = ctx.state.config.pane.border_style.prev();
            ctx.state.config.pane.border_style = value;
            persist_pane_string_or_toast(ctx, "border_style", value.id());
        }
        CycleBorderStyle => {
            execute_action(ctx, Action::CycleBorderStyle);
        }
        ToggleBellUrgency => {
            ctx.state.config.notifications.bell = !ctx.state.config.notifications.bell;
            persisted = Some(("notifications", "bell", ctx.state.config.notifications.bell));
        }
        CycleAlertBorder => {
            let value = if reverse {
                ctx.state.config.pane.alert_border.prev()
            } else {
                ctx.state.config.pane.alert_border.next()
            };
            ctx.state.config.pane.alert_border = value;
            if let Err(err) = crate::config::persist_pane_string("alert_border", value.id()) {
                preference_error(ctx, err);
            }
        }
        CycleWorkbarAlert => {
            let value = if reverse {
                ctx.state.config.workbar.alert.mode.prev()
            } else {
                ctx.state.config.workbar.alert.mode.next()
            };
            ctx.state.config.workbar.alert.mode = value;
            if let Err(err) = crate::config::persist_workbar_alert_string("mode", value.id()) {
                preference_error(ctx, err);
            }
        }
        CycleWorkbarAlertPaint => {
            let value = if reverse {
                ctx.state.config.workbar.alert.paint.prev()
            } else {
                ctx.state.config.workbar.alert.paint.next()
            };
            ctx.state.config.workbar.alert.paint = value;
            if let Err(err) = crate::config::persist_workbar_alert_string("paint", value.id()) {
                preference_error(ctx, err);
            }
        }
        ToggleMarkBell => {
            ctx.state.config.workbar.alert.bell = !ctx.state.config.workbar.alert.bell;
            persisted = Some(("workbar.alert", "bell", ctx.state.config.workbar.alert.bell));
        }
        ToggleMarkBlocked => {
            ctx.state.config.workbar.alert.blocked = !ctx.state.config.workbar.alert.blocked;
            persisted = Some((
                "workbar.alert",
                "blocked",
                ctx.state.config.workbar.alert.blocked,
            ));
        }
        ToggleMarkFinished => {
            ctx.state.config.workbar.alert.finished = !ctx.state.config.workbar.alert.finished;
            persisted = Some((
                "workbar.alert",
                "finished",
                ctx.state.config.workbar.alert.finished,
            ));
        }
        ToggleMarkWorking => {
            ctx.state.config.workbar.alert.working = !ctx.state.config.workbar.alert.working;
            persisted = Some((
                "workbar.alert",
                "working",
                ctx.state.config.workbar.alert.working,
            ));
        }
        ToggleMarkIdle => {
            ctx.state.config.workbar.alert.idle = !ctx.state.config.workbar.alert.idle;
            persisted = Some(("workbar.alert", "idle", ctx.state.config.workbar.alert.idle));
        }
        ToggleDesktopEnabled => {
            ctx.state.config.notifications.enabled = !ctx.state.config.notifications.enabled;
            persisted = Some((
                "notifications",
                "enabled",
                ctx.state.config.notifications.enabled,
            ));
        }
        ToggleDesktopBlocked => {
            ctx.state.config.notifications.pane_blocked =
                !ctx.state.config.notifications.pane_blocked;
            persisted = Some((
                "notifications",
                "pane_blocked",
                ctx.state.config.notifications.pane_blocked,
            ));
        }
        ToggleDesktopDone => {
            ctx.state.config.notifications.pane_done = !ctx.state.config.notifications.pane_done;
            persisted = Some((
                "notifications",
                "pane_done",
                ctx.state.config.notifications.pane_done,
            ));
        }
        ToggleDesktopExit => {
            ctx.state.config.notifications.pane_exit = !ctx.state.config.notifications.pane_exit;
            persisted = Some((
                "notifications",
                "pane_exit",
                ctx.state.config.notifications.pane_exit,
            ));
        }
        ToggleDesktopExitError => {
            ctx.state.config.notifications.pane_exit_error =
                !ctx.state.config.notifications.pane_exit_error;
            persisted = Some((
                "notifications",
                "pane_exit_error",
                ctx.state.config.notifications.pane_exit_error,
            ));
        }
        ToggleSoundEnabled => {
            ctx.state.config.sounds.enabled = !ctx.state.config.sounds.enabled;
            persisted = Some(("sounds", "enabled", ctx.state.config.sounds.enabled));
        }
        ToggleSoundBell => {
            ctx.state.config.sounds.bell = !ctx.state.config.sounds.bell;
            persisted = Some(("sounds", "bell", ctx.state.config.sounds.bell));
        }
        ToggleSoundBlocked => {
            ctx.state.config.sounds.blocked = !ctx.state.config.sounds.blocked;
            persisted = Some(("sounds", "blocked", ctx.state.config.sounds.blocked));
        }
        ToggleSoundDone => {
            ctx.state.config.sounds.done = !ctx.state.config.sounds.done;
            persisted = Some(("sounds", "done", ctx.state.config.sounds.done));
        }
        ToggleSoundError => {
            ctx.state.config.sounds.error = !ctx.state.config.sounds.error;
            persisted = Some(("sounds", "error", ctx.state.config.sounds.error));
        }
        CycleStartupMode => {
            let value = if reverse {
                ctx.state.config.session.startup.prev()
            } else {
                ctx.state.config.session.startup.next()
            };
            ctx.state.config.session.startup = value;
            if let Err(err) = crate::config::persist_session_string("startup", value.id()) {
                preference_error(ctx, err);
            }
        }
        ToggleSessionAutosave => {
            ctx.state.config.session.autosave = !ctx.state.config.session.autosave;
            persisted = Some(("session", "autosave", ctx.state.config.session.autosave));
        }
        ToggleSessionResurrect => {
            ctx.state.config.session.resurrect = !ctx.state.config.session.resurrect;
            persisted = Some(("session", "resurrect", ctx.state.config.session.resurrect));
        }
    }
    if let Some((section, key, value)) = persisted {
        let result = match section {
            "notifications" => crate::config::persist_notification_flag(key, value),
            "sounds" => crate::config::persist_sound_flag(key, value),
            "session" => crate::config::persist_session_flag(key, value),
            _ => crate::config::persist_workbar_alert_flag(key, value),
        };
        if let Err(err) = result {
            preference_error(ctx, err);
        }
    }
    if !matches!(action, Theme | EditPadding) {
        ctx.state.show_settings = true;
        ctx.state.settings_selected = Some(action);
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

fn preference_error(ctx: &mut Context<AppRoot>, err: String) {
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::PreferenceSave,
        Some("Preference not saved".to_string()),
        err,
    );
}

fn persist_pane_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_pane_string(key, value) {
        crate::pty_events::notify_on(
            ctx,
            crate::state::ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

pub(super) fn close_pane_padding_editor(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.pane_padding_editor.is_none() {
        return Update::none();
    }
    ctx.state.pane_padding_editor = None;
    if ctx.state.show_settings {
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

pub(super) fn pane_padding_vertical_changed(
    ctx: &mut Context<AppRoot>,
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
    ctx: &mut Context<AppRoot>,
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

pub(super) fn advance_pane_padding(ctx: &mut Context<AppRoot>) -> Update {
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

pub(super) fn submit_pane_padding(ctx: &mut Context<AppRoot>) -> Update {
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
        crate::pty_events::notify_error(ctx, "Padding not saved", error);
    }
    ctx.state.pane_padding_editor = None;
    if ctx.state.show_settings {
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

pub(super) fn close_theme_picker(ctx: &mut Context<AppRoot>) -> Update {
    cancel_theme_picker(ctx);
    crate::ops::overlay_return::finish(ctx)
}
pub(super) fn preview_theme(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    preview(ctx, index)
}
pub(super) fn select_theme(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    select(ctx, index)
}
pub(super) fn theme_tick(ctx: &mut Context<AppRoot>) -> Update {
    tick(ctx)
}
pub(super) fn config_file_changed(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::config::config_file_changed(ctx)
}

pub(super) fn workbar_tick(ctx: &mut Context<AppRoot>) -> Update {
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

pub(super) fn theme_error(ctx: &mut Context<AppRoot>, message: String) -> Update {
    crate::pty_events::notify_error(ctx, "Theme reload failed", message);
    Update::full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::TestBackend;

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn padding_input_accepts_empty_or_one_ascii_digit_in_range() {
        assert!(valid_padding_text(""));
        assert!(valid_padding_text("8"));
        assert!(!valid_padding_text("9"));
        assert!(!valid_padding_text("12"));
        assert!(!valid_padding_text("８"));
    }

    #[test]
    fn settings_left_right_steps_alert_modes_and_keeps_selection() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend.state_mut().config.pane.show_workbar = true;
            backend.state_mut().settings_selected =
                Some(crate::state::SettingsAction::CycleAlertBorder);

            backend
                .dispatch(Msg::SettingsStep { reverse: true })
                .unwrap();
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Static
            );
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::CycleAlertBorder)
            );

            backend
                .dispatch(Msg::SettingsStep { reverse: false })
                .unwrap();
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Pulse
            );

            backend.state_mut().settings_selected =
                Some(crate::state::SettingsAction::CycleWorkbarAlert);
            backend
                .dispatch(Msg::SettingsStep { reverse: true })
                .unwrap();
            assert_eq!(
                backend.state().config.workbar.alert.mode,
                crate::state::AlertMode::Static
            );
        });
    }

    #[test]
    fn appearance_deep_link_reuses_settings() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::RunAction(Action::OpenAppearance))
                .unwrap();
            assert!(backend.state().show_settings);
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::Theme)
            );
        });
    }

    #[test]
    fn alerts_deep_link_clears_a_session_picker_and_selects_bell_urgency() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.show_session_picker = true;
                state.session_picker = Some(crate::state::SessionPickerState::new(Vec::new()));
                state.show_help = true;
                state.overlay_return = Some(crate::state::OverlayOrigin::Settings);
            }

            backend
                .dispatch(Msg::RunAction(Action::OpenAlerts))
                .unwrap();

            assert!(!backend.state().show_session_picker);
            assert!(backend.state().session_picker.is_none());
            assert!(!backend.state().show_help);
            assert!(backend.state().overlay_return.is_none());
            assert!(backend.state().show_settings);
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::ToggleBellUrgency)
            );
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::settings_palette_key())
            );
        });
    }

    #[test]
    fn settings_command_opens_at_theme() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend
                .dispatch(Msg::RunAction(Action::OpenSettings))
                .unwrap();
            assert!(backend.state().show_settings);
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::Theme)
            );
        });
    }

    /// The startup row is a value ring like the alert modes: both arrows work and the row keeps the
    /// highlight. What reaches `[session]` is pinned deterministically in `config::persist` instead -
    /// these tests share one scratch config file and run in parallel, so reading it back here would
    /// race a sibling's write.
    #[test]
    fn settings_steps_startup_mode_in_both_directions() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend.state_mut().settings_selected =
                Some(crate::state::SettingsAction::CycleStartupMode);
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Picker
            );

            backend
                .dispatch(Msg::SettingsStep { reverse: false })
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Ephemeral
            );
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::CycleStartupMode)
            );
            assert!(backend.state().show_settings, "the dialog stays open");

            backend
                .dispatch(Msg::SettingsStep { reverse: true })
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Picker
            );
        });
    }

    #[test]
    fn settings_toggles_session_flags() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            for (action, key) in [
                (
                    crate::state::SettingsAction::ToggleSessionAutosave,
                    "autosave",
                ),
                (
                    crate::state::SettingsAction::ToggleSessionResurrect,
                    "resurrect",
                ),
            ] {
                let before = match key {
                    "autosave" => backend.state().config.session.autosave,
                    _ => backend.state().config.session.resurrect,
                };
                backend.dispatch(Msg::SettingsActivate(action)).unwrap();
                let after = match key {
                    "autosave" => backend.state().config.session.autosave,
                    _ => backend.state().config.session.resurrect,
                };
                assert_eq!(after, !before, "{key} should toggle");
                assert_eq!(backend.state().settings_selected, Some(action));
                assert!(backend.state().show_settings, "the dialog stays open");
            }
        });
    }

    #[test]
    fn focus_on_hover_id_stays_a_direct_action() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let initial = backend.state().config.pane.focus_on_hover;
            backend
                .dispatch(Msg::RunAction(Action::ToggleFocusOnHover))
                .unwrap();
            assert_eq!(backend.state().config.pane.focus_on_hover, !initial);
            assert!(!backend.state().show_settings);
        });
    }
}
