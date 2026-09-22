use tui_lipan::prelude::*;

use crate::actions::{execute_action, execute_palette_action};
use crate::config::UserCommandAction;
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
    crate::pane::pty_events::notify_error(ctx, "Invalid padding", "Enter one digit");
}

pub(super) fn command_link_ready(ctx: &mut Context<AppRoot>, link: CommandLink<Msg>) -> Update {
    ctx.state.command_link = Some(link.clone());
    // Started here rather than at the first remote operation: every ssh this client spawns has to
    // find the endpoint already bound, and the ops that spawn one run on worker threads with no
    // route back to the UI of their own.
    crate::session::remote::askpass::start(link);
    crate::update::sidebar::request_sessions_refresh(ctx);
    crate::update::sidebar::request_command_poll(ctx);
    crate::update::workbar::request_command_polls(ctx);
    crate::ops::services::start_services(ctx)
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
            | Action::OpenExtensions
            | Action::OpenAppearance
            | Action::OpenAlerts
            | Action::TogglePalette
            | Action::ToggleHelp
    ) {
        ctx.state.pane_padding_editor = None;
        discard_settings_choice(ctx);
    }
    let cycle_layout_in_palette = matches!(action, Action::ToggleLayout) && ctx.state.show_palette;
    let from_palette = ctx.state.show_palette;
    let handoff_palette = from_palette && command_palette_handoff_action(ctx, action);
    if handoff_palette {
        ctx.state.command_palette_handoff_epoch =
            ctx.state.command_palette_handoff_epoch.wrapping_add(1);
        ctx.state.command_palette_handoff = Some(ctx.state.command_palette_handoff_epoch);
    } else if !cycle_layout_in_palette {
        ctx.state.show_palette = false;
        ctx.state.command_palette_sidebar_query = false;
        ctx.state.command_palette_handoff = None;
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
        | Action::OpenExtensions
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

fn command_palette_handoff_action(ctx: &Context<AppRoot>, action: Action) -> bool {
    let Action::RunNamedCommand(index) = action else {
        return false;
    };
    let Some(command) = ctx.state.config.commands.get(index) else {
        return false;
    };
    if !command.env.iter().any(|(key, _)| key == "ROZI_EXTENSION") {
        return false;
    }
    matches!(
        command.action,
        UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. }
    )
}

pub(super) fn command_palette_handoff_finished(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if ctx.state.command_palette_handoff != Some(epoch) {
        return Update::none();
    }
    ctx.state.command_palette_handoff = None;
    if ctx.state.show_palette && !ctx.state.show_pick {
        ctx.state.show_palette = false;
        ctx.state.command_palette_sidebar_query = false;
        ctx.state.commands_dirty = true;
        request_current_pane_focus(ctx);
        return Update::full();
    }
    Update::none()
}

pub(super) fn close_palette(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_palette = false;
    ctx.state.command_palette_sidebar_query = false;
    ctx.state.command_palette_handoff = None;
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
    ctx.state.keybindings = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

/// A new query re-ranks the rows, so the highlight returns to the first match, as in every picker.
pub(super) fn help_query_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    let Some(keybindings) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    event.apply_to(&mut keybindings.query);
    keybindings.selected = None;
    Update::full()
}

pub(super) fn help_tab_selected(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(keybindings) = ctx.state.keybindings.as_mut() else {
        return Update::none();
    };
    keybindings.tab = crate::state::HelpTab::from_index(index);
    keybindings.selected = None;
    ctx.request_focus(crate::view::help_filter_key());
    Update::full()
}

pub(super) fn settings_escape(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.settings_navigation.query.text().is_empty() {
        return close_settings(ctx);
    }
    ctx.state.settings_navigation.query = TextInput::new("");
    sync_settings_query_selection(ctx, true);
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

pub(super) fn settings_query_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    let was_empty = ctx.state.settings_navigation.query.text().is_empty();
    event.apply_to(&mut ctx.state.settings_navigation.query);
    sync_settings_query_selection(ctx, was_empty);
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

fn sync_settings_query_selection(ctx: &mut Context<AppRoot>, was_empty: bool) {
    let is_empty = ctx.state.settings_navigation.query.text().is_empty();
    if was_empty && !is_empty {
        ctx.state.settings_navigation.browse_selected = ctx.state.settings_selected;
    }
    if is_empty {
        ctx.state.settings_selected = ctx.state.settings_navigation.browse_selected.take();
    }
    let selected = crate::view::settings_query_selection(ctx);
    crate::state::assign_settings_selection(&mut ctx.state, selected);
}

pub(super) fn settings_tab_selected(
    ctx: &mut Context<AppRoot>,
    tab: crate::state::SettingsTab,
) -> Update {
    let from_tab = ctx.state.settings_navigation.tab;
    if ctx.state.settings_navigation.query.text().is_empty() {
        let selected = ctx.state.settings_selected;
        ctx.state.settings_navigation.remember(from_tab, selected);
    }
    ctx.state.settings_navigation.tab = tab;
    ctx.state.settings_navigation.query = TextInput::new("");
    ctx.state.settings_navigation.browse_selected = None;
    ctx.state.settings_selected = ctx.state.settings_navigation.remembered(tab);
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

pub(super) fn close_settings(ctx: &mut Context<AppRoot>) -> Update {
    discard_settings_choice(ctx);
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
    crate::state::assign_settings_selection(&mut ctx.state, Some(action));
    Update::full()
}

pub(super) fn settings_activate(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    settings_apply(ctx, action)
}

pub(super) fn settings_cycle_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    cycle_settings_choice(ctx, action)
}

pub(super) fn settings_choice_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let action = {
        let Some(editor) = ctx.state.settings_choice.as_mut() else {
            return Update::none();
        };
        if index >= editor.options.len() {
            return Update::none();
        }
        if editor.index == index {
            ctx.request_focus(crate::view::settings_choice_key());
            return Update::full();
        }
        editor.index = index;
        editor.action
    };
    apply_settings_choice(ctx, action, index, false);
    ctx.request_focus(crate::view::settings_choice_key());
    Update::full()
}

pub(super) fn settings_choice_pick(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(editor) = ctx.state.settings_choice.as_mut() else {
        return Update::none();
    };
    if index >= editor.options.len() {
        return Update::none();
    }
    editor.index = index;
    settings_choice_save(ctx)
}

pub(super) fn settings_choice_save(ctx: &mut Context<AppRoot>) -> Update {
    let Some(editor) = ctx.state.settings_choice.take() else {
        return Update::none();
    };
    apply_settings_choice(ctx, editor.action, editor.index, true);
    ctx.state.show_settings = true;
    crate::state::assign_settings_selection(&mut ctx.state, Some(editor.action));
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

pub(super) fn settings_choice_cancel(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.settings_choice.is_none() {
        return Update::none();
    }
    discard_settings_choice(ctx);
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

fn open_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    if action.disabled_reason(&ctx.state.config).is_some() {
        ctx.request_focus(crate::view::settings_palette_key());
        return Update::full();
    }
    let Some(ring) = action.choice_ring(&ctx.state.config) else {
        return Update::none();
    };
    ctx.state.settings_choice = Some(crate::state::SettingsChoiceEditor::from_ring(action, ring));
    crate::state::assign_settings_selection(&mut ctx.state, Some(action));
    ctx.request_focus(crate::view::settings_choice_key());
    Update::full()
}

fn settings_apply(ctx: &mut Context<AppRoot>, action: crate::state::SettingsAction) -> Update {
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
        CycleTitlebar
        | CycleTitleStyle
        | CycleSidebarTabStyle
        | CycleWorkbarStyle
        | CycleWorkbarBadgeStyle
        | CycleWorkbarTabStyle
        | CyclePaneAnimation
        | CycleWhichKey
        | CycleCopyOnSelect
        | CycleMiddleClickPaste
        | CycleRightClickClipboard
        | CycleBorderMode
        | CycleBorderStyle
        | CycleFloatBorderStyle
        | CycleScratchBorderStyle
        | CycleFullscreenBorderStyle
        | CyclePickerBorderStyle
        | CyclePickerTabStyle
        | CyclePickerSelectionStyle
        | CycleAlertBorder
        | CycleWorkbarAlert
        | CycleStartupMode
        | CycleResurrectForeground => {
            return open_settings_choice(ctx, action);
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
        ToggleWorkbarBackground => {
            execute_action(ctx, Action::ToggleWorkbarBackground);
        }
        TogglePickerTabBackground => {
            execute_action(ctx, Action::TogglePickerTabBackground);
        }
        ToggleSidebarGap => {
            execute_action(ctx, Action::ToggleSidebarGap);
        }
        ToggleSidebarPosition => {
            execute_action(ctx, Action::ToggleSidebarPosition);
        }
        ToggleSidebarBackground => {
            execute_action(ctx, Action::ToggleSidebarBackground);
        }
        ToggleSidebarBackgroundFollowsCanvas => {
            execute_action(ctx, Action::ToggleSidebarBackgroundFollowsCanvas);
        }
        ToggleWorkbarPowerline => {
            execute_action(ctx, Action::ToggleWorkbarPowerline);
        }
        ToggleWorkspaceAnimation => {
            let value = !ctx.state.config.animations.workspace;
            ctx.state.config.animations.workspace = value;
            if let Err(err) = crate::config::persist_animation_flag("workspace", value) {
                preference_error(ctx, err);
            }
        }
        ToggleAnimations => {
            execute_action(ctx, Action::ToggleAnimations);
        }
        ToggleNerdIcons => {
            execute_action(ctx, Action::ToggleNerdIcons);
        }
        ToggleOsc52 => {
            ctx.state.config.clipboard.enable_osc52 = !ctx.state.config.clipboard.enable_osc52;
            ctx.set_clipboard_config(crate::app::clipboard_config(&ctx.state.config));
            persisted = Some((
                "clipboard",
                "enable_osc52",
                ctx.state.config.clipboard.enable_osc52,
            ));
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
        ToggleBellUrgency => {
            ctx.state.config.notifications.bell = !ctx.state.config.notifications.bell;
            persisted = Some(("notifications", "bell", ctx.state.config.notifications.bell));
        }
        CycleWorkbarAlertPaint => {
            let value = ctx.state.config.workbar.alert.paint.next();
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
            "input" => crate::config::persist_input_flag(key, value),
            "clipboard" => crate::config::persist_clipboard_flag(key, value),
            _ => crate::config::persist_workbar_alert_flag(key, value),
        };
        if let Err(err) = result {
            preference_error(ctx, err);
        }
    }
    if !matches!(action, Theme | EditPadding) {
        ctx.state.show_settings = true;
        crate::state::assign_settings_selection(&mut ctx.state, Some(action));
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

fn preference_error(ctx: &mut Context<AppRoot>, err: String) {
    crate::pane::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::PreferenceSave,
        Some("Preference not saved".to_string()),
        err,
    );
}

fn persist_pane_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_pane_string(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            crate::state::ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn persist_sidebar_string_or_toast(ctx: &mut Context<AppRoot>, key: &str, value: &str) {
    if let Err(err) = crate::config::persist_sidebar_string(key, value) {
        crate::pane::pty_events::notify_on(
            ctx,
            crate::state::ToastChannel::PreferenceSave,
            Some("Preference not saved".to_string()),
            err,
        );
    }
}

fn cycle_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    if action.disabled_reason(&ctx.state.config).is_some() {
        ctx.request_focus(crate::view::settings_palette_key());
        return Update::full();
    }
    let Some(ring) = action.choice_ring(&ctx.state.config) else {
        return Update::none();
    };
    let count = ring.options.len();
    if count == 0 {
        return Update::none();
    }
    apply_settings_choice(ctx, action, (ring.index + 1) % count, true);
    ctx.state.show_settings = true;
    crate::state::assign_settings_selection(&mut ctx.state, Some(action));
    ctx.request_focus(crate::view::settings_palette_key());
    Update::full()
}

fn discard_settings_choice(ctx: &mut Context<AppRoot>) {
    let clipboard_changed = ctx.state.settings_choice.as_ref().is_some_and(|editor| {
        matches!(
            editor.action,
            crate::state::SettingsAction::CycleCopyOnSelect
                | crate::state::SettingsAction::CycleMiddleClickPaste
                | crate::state::SettingsAction::CycleRightClickClipboard
        )
    });
    if let Some(delay) = crate::state::abandon_settings_choice(&mut ctx.state) {
        ctx.set_command_chord_reveal_delay(delay);
    }
    if clipboard_changed {
        ctx.set_clipboard_config(crate::app::clipboard_config(&ctx.state.config));
    }
}

fn apply_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
    index: usize,
    persist: bool,
) {
    if !action.apply_choice(&mut ctx.state.config, index) {
        return;
    }
    if matches!(action, crate::state::SettingsAction::CycleWhichKey) {
        ctx.set_command_chord_reveal_delay(ctx.state.config.input.which_key.reveal_delay());
    }
    if matches!(
        action,
        crate::state::SettingsAction::CycleCopyOnSelect
            | crate::state::SettingsAction::CycleMiddleClickPaste
            | crate::state::SettingsAction::CycleRightClickClipboard
    ) {
        ctx.set_clipboard_config(crate::app::clipboard_config(&ctx.state.config));
    }
    if persist {
        persist_applied_settings_choice(ctx, action);
    }
}

fn persist_applied_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) {
    use crate::state::SettingsAction::*;
    match action {
        CycleWhichKey => {
            if let Err(err) = crate::config::persist_input_string(
                "which_key",
                ctx.state.config.input.which_key.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleCopyOnSelect => {
            if let Err(err) = crate::config::persist_clipboard_string(
                "copy_on_select",
                ctx.state.config.clipboard.copy_on_select.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleMiddleClickPaste => {
            if let Err(err) = crate::config::persist_clipboard_string(
                "middle_click_paste",
                ctx.state.config.clipboard.middle_click_paste.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleRightClickClipboard => {
            if let Err(err) = crate::config::persist_clipboard_string(
                "right_click",
                ctx.state.config.clipboard.right_click.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CyclePickerBorderStyle => persist_pane_string_or_toast(
            ctx,
            "picker_border_style",
            ctx.state.config.pane.picker_border_style.id(),
        ),
        CyclePickerTabStyle => persist_pane_string_or_toast(
            ctx,
            "picker_tab_style",
            crate::state::cap_style_id(ctx.state.config.pane.picker_tab_style),
        ),
        CyclePickerSelectionStyle => persist_pane_string_or_toast(
            ctx,
            "picker_selection_style",
            crate::state::cap_style_id(ctx.state.config.pane.picker_selection_style),
        ),
        CycleTitlebar => {
            persist_pane_string_or_toast(ctx, "titlebar", ctx.state.config.pane.titlebar.id())
        }
        CycleTitleStyle => persist_pane_string_or_toast(
            ctx,
            "title_style",
            crate::state::cap_style_id(ctx.state.config.pane.title_style),
        ),
        CycleWorkbarStyle => persist_pane_string_or_toast(
            ctx,
            "workbar_style",
            crate::state::cap_style_id(ctx.state.config.pane.workbar_style),
        ),
        CycleWorkbarBadgeStyle => persist_pane_string_or_toast(
            ctx,
            "workbar_badge_style",
            crate::state::cap_style_id(ctx.state.config.pane.workbar_badge_style),
        ),
        CycleWorkbarTabStyle => persist_pane_string_or_toast(
            ctx,
            "workbar_tab_style",
            crate::state::cap_style_id(ctx.state.config.pane.workbar_tab_style),
        ),
        CycleBorderMode => {
            persist_pane_string_or_toast(ctx, "border_mode", ctx.state.config.pane.border_mode.id())
        }
        CycleBorderStyle => persist_pane_string_or_toast(
            ctx,
            "border_style",
            ctx.state.config.pane.border_style.id(),
        ),
        CycleFloatBorderStyle => persist_pane_string_or_toast(
            ctx,
            "float_border_style",
            ctx.state.config.pane.float_border_style.id(),
        ),
        CycleScratchBorderStyle => persist_pane_string_or_toast(
            ctx,
            "scratch_border_style",
            ctx.state.config.pane.scratch_border_style.id(),
        ),
        CycleFullscreenBorderStyle => persist_pane_string_or_toast(
            ctx,
            "fullscreen_border_style",
            ctx.state.config.pane.fullscreen_border_style.id(),
        ),
        CyclePaneAnimation => {
            if let Err(err) = crate::config::persist_animation_string(
                "pane_style",
                ctx.state.config.animations.pane_style.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleSidebarTabStyle => persist_sidebar_string_or_toast(
            ctx,
            "tab_style",
            crate::state::cap_style_id(ctx.state.config.sidebar.tab_style),
        ),
        CycleAlertBorder => persist_pane_string_or_toast(
            ctx,
            "alert_border",
            ctx.state.config.pane.alert_border.id(),
        ),
        CycleWorkbarAlert => {
            if let Err(err) = crate::config::persist_workbar_alert_string(
                "mode",
                ctx.state.config.workbar.alert.mode.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleStartupMode => {
            if let Err(err) = crate::config::persist_session_string(
                "startup",
                ctx.state.config.session.startup.id(),
            ) {
                preference_error(ctx, err);
            }
        }
        CycleResurrectForeground => {
            if let Err(err) = crate::config::persist_session_string(
                "resurrect_foreground",
                ctx.state.config.session.resurrect_foreground.as_str(),
            ) {
                preference_error(ctx, err);
            }
        }
        _ => {}
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
    editor.focus = crate::state::PanePaddingField::Vertical;
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
    editor.focus = crate::state::PanePaddingField::Horizontal;
    ctx.request_focus(crate::view::pane_padding_horizontal_key());
    Update::full()
}

/// Record where focus landed, so the dialog can mark the active field. Reported by the field
/// itself, which covers a click and `Tab` traversal alike.
pub(super) fn pane_padding_focus(
    ctx: &mut Context<AppRoot>,
    field: crate::state::PanePaddingField,
) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
        return Update::none();
    };
    editor.focus = field;
    Update::full()
}

pub(super) fn advance_pane_padding(ctx: &mut Context<AppRoot>) -> Update {
    let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
        return Update::none();
    };
    if padding_value(editor.vertical.text()).is_some() {
        editor.focus = crate::state::PanePaddingField::Horizontal;
        ctx.request_focus(crate::view::pane_padding_horizontal_key());
    } else {
        editor.focus = crate::state::PanePaddingField::Vertical;
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
        crate::pane::pty_events::notify_error(ctx, "Padding not saved", error);
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

/// Re-check for a newer release. Turning `[updates] check` off stops the loop here rather than
/// cancelling the tick in flight; `crate::ops::config` arms a fresh one when it comes back on.
pub(super) fn update_check_tick(ctx: &mut Context<AppRoot>) -> Update {
    match ctx.state.update_check_interval() {
        Some(interval) => Update::command_only(crate::check_for_update_and_reschedule(interval)),
        None => Update::none(),
    }
}

/// Remember a newer release for Commands, and raise its toast if this client claimed it.
///
/// The toast is the one-time announcement; the remembered update is what lasts. It keeps
/// **Update rozi** in Commands for the rest of this client's life, so a toast that went by while
/// the user looked elsewhere does not lose the news.
pub(super) fn update_available(
    ctx: &mut Context<AppRoot>,
    update: crate::ops::update_check::AvailableUpdate,
    announce: bool,
) -> Update {
    let known = ctx.state.available_update.as_ref() == Some(&update);
    if !known {
        // A newer release than the one this client already offered is a fresh offer, even if the
        // user had started updating to the previous one.
        ctx.state.update_started = false;
        ctx.state.available_update = Some(update.clone());
        ctx.state.commands_dirty = true;
    }
    if !announce {
        return if known {
            Update::none()
        } else {
            Update::full()
        };
    }
    let in_commands = crate::commands::command_available(Action::UpdateRozi, &ctx.state);
    crate::pane::pty_events::notify_update(
        ctx,
        update.toast_title(),
        update.toast_body(in_commands),
        update.needs_caution(),
    );
    Update::full()
}

pub(super) fn theme_error(ctx: &mut Context<AppRoot>, message: String) -> Update {
    crate::pane::pty_events::notify_error(ctx, "Theme reload failed", message);
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

    /// The text `notify_*` tracked for the toast it raised, title and body joined by NUL.
    fn last_toast(backend: &TestBackend<AppRoot>) -> String {
        let tracked = backend
            .state()
            .replaceable_toasts
            .values()
            .next()
            .expect("a toast was raised");
        tracked.content().replace('\u{0}', " ")
    }

    fn update_to(version: &str, contracts: Option<(u32, u32)>) -> Msg {
        Msg::UpdateAvailable {
            update: crate::ops::update_check::AvailableUpdate::for_test(
                version,
                crate::platform::install_source::InstallSource::Managed,
                contracts,
            ),
            announce: true,
        }
    }

    #[test]
    fn both_update_toasts_lead_with_the_new_version() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.dispatch(update_to("9.9.9", None)).unwrap();
            let toast = last_toast(&backend);
            assert!(toast.starts_with("rozi v9.9.9 available"), "{toast}");
            assert!(toast.contains("→ v9.9.9"), "{toast}");
            // Commands holds the row in a local client, so the toast says where to find it again.
            assert!(toast.contains("or use command Update rozi"), "{toast}");

            backend.state_mut().replaceable_toasts.clear();
            let protocol = crate::session::protocol::PROTOCOL_VERSION;
            let extension_api = crate::config::EXTENSION_API_VERSION;
            backend
                .dispatch(update_to("9.9.10", Some((extension_api, protocol + 1))))
                .unwrap();
            let toast = last_toast(&backend);
            // The regression this guards: the compatibility toast used to open with
            // "Compatibility change in v9.9.9" and never say an update existed.
            assert!(toast.starts_with("rozi v9.9.10 available"), "{toast}");
            assert!(toast.contains("session protocol"), "{toast}");
            assert!(toast.contains("run `rozi update`"), "{toast}");
        });
    }

    /// Only one client per release raises the toast, but every client that found the release keeps
    /// it in Commands - which is what makes a missed toast recoverable.
    #[test]
    fn an_unannounced_update_still_offers_update_rozi() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            assert!(!crate::commands::command_available(
                Action::UpdateRozi,
                backend.state()
            ));
            let Msg::UpdateAvailable { update, .. } = update_to("9.9.9", None) else {
                unreachable!()
            };
            backend
                .dispatch(Msg::UpdateAvailable {
                    update,
                    announce: false,
                })
                .unwrap();

            assert!(backend.state().replaceable_toasts.is_empty());
            assert!(crate::commands::command_available(
                Action::UpdateRozi,
                backend.state()
            ));
            assert_eq!(
                crate::ops::update_check::update_command_here(backend.state()),
                Some("rozi update")
            );
        });
    }

    /// The popup runs on the session's server, so a remote session in front would update the host.
    /// Once run, the same release is not offered again; a newer one is.
    #[test]
    fn update_rozi_is_withheld_on_a_remote_session_and_after_it_ran() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.dispatch(update_to("9.9.9", None)).unwrap();

            backend.state_mut().current_mut().remote_target = Some(
                crate::session::remote::RemoteTarget::Alias("workbox".into()),
            );
            assert!(!crate::commands::command_available(
                Action::UpdateRozi,
                backend.state()
            ));
            let toast_on_remote = {
                backend.state_mut().replaceable_toasts.clear();
                backend.dispatch(update_to("9.9.10", None)).unwrap();
                last_toast(&backend)
            };
            assert!(!toast_on_remote.contains("Commands"), "{toast_on_remote}");
            backend.state_mut().current_mut().remote_target = None;

            backend.state_mut().update_started = true;
            assert!(!crate::commands::command_available(
                Action::UpdateRozi,
                backend.state()
            ));
            backend.dispatch(update_to("9.9.10", None)).unwrap();
            assert!(
                backend.state().update_started,
                "the same release stays withdrawn"
            );
            backend.dispatch(update_to("9.9.11", None)).unwrap();
            assert!(crate::commands::command_available(
                Action::UpdateRozi,
                backend.state()
            ));
        });
    }

    #[test]
    fn a_client_that_may_not_check_stays_off_the_network_whatever_the_config_says() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            // What an integration app looks like: checking is on in config, denied by the client.
            assert!(backend.state().config.updates.check);
            assert!(!backend.state().update_checks_allowed);
            assert!(backend.state().update_check_interval().is_none());

            backend.state_mut().update_checks_allowed = true;
            assert_eq!(
                backend.state().update_check_interval(),
                Some(std::time::Duration::from_secs(6 * 3600))
            );

            backend.state_mut().config.updates.check = false;
            assert!(backend.state().update_check_interval().is_none());
        });
    }

    #[test]
    fn settings_enter_opens_alert_picker_and_shift_enter_cycles() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend.state_mut().config.pane.show_workbar = true;
            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleAlertBorder,
                ))
                .unwrap();
            assert!(backend.state().settings_choice.is_some());
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Pulse
            );
            backend.dispatch(Msg::SettingsChoiceCancel).unwrap();

            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleAlertBorder,
                ))
                .unwrap();
            assert!(backend.state().settings_choice.is_none());
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Off
            );

            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleAlertBorder,
                ))
                .unwrap();
            backend.dispatch(Msg::SettingsChoiceSelect(1)).unwrap();
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Static
            );
            assert!(backend.state().settings_choice.is_some());
            backend.dispatch(Msg::SettingsChoiceCancel).unwrap();
            assert!(backend.state().settings_choice.is_none());
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Off
            );
            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleAlertBorder,
                ))
                .unwrap();
            backend.dispatch(Msg::SettingsChoicePick(1)).unwrap();
            assert_eq!(
                backend.state().config.pane.alert_border,
                crate::state::AlertMode::Static
            );
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::CycleAlertBorder)
            );

            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleWorkbarAlert,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.workbar.alert.mode,
                crate::state::AlertMode::Off
            );
            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleWorkbarAlert,
                ))
                .unwrap();
            backend.dispatch(Msg::SettingsChoiceSelect(1)).unwrap();
            assert_eq!(
                backend.state().config.workbar.alert.mode,
                crate::state::AlertMode::Static
            );
            assert!(backend.state().settings_choice.is_some());
            backend.dispatch(Msg::SettingsChoicePick(1)).unwrap();
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
    fn help_filter_uses_picker_style_body_chrome() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 96,
                h: 40,
            });
            backend.state_mut().keybindings = Some(crate::state::KeybindingsState::default());
            backend.render();

            let placeholder = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .into_iter()
                .find(|line| line.contains("Search keybindings"))
                .expect("help search row");
            assert!(placeholder.contains("│ Search keybindings…"));
            assert!(placeholder.contains("57/57 │"));

            help_state(&mut backend).query = TextInput::new("here i am quite long");
            backend.render();
            let populated = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .into_iter()
                .find(|line| line.contains("here i am quite long"))
                .expect("populated help search row");
            assert!(
                populated.contains("here i am quite long"),
                "growing search input should stay inside the modal: {populated}"
            );

            help_state(&mut backend).query =
                TextInput::new("here i am quite long and it is moving left");
            backend.render();
            let overflowing = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .into_iter()
                .find(|line| line.contains("and it is moving left"))
                .expect("overflowing help search row");
            assert!(
                overflowing.contains("and it is moving left"),
                "overflowing search input should keep its tail visible: {overflowing}"
            );
        });
    }

    fn help_state(backend: &mut TestBackend<AppRoot>) -> &mut crate::state::KeybindingsState {
        backend
            .state_mut()
            .keybindings
            .as_mut()
            .expect("keybindings overlay is open")
    }

    fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
        backend
            .send_key(KeyEvent {
                code,
                mods: KeyMods::NONE,
            })
            .expect("send key");
        backend.render();
    }

    #[test]
    fn keybindings_search_owns_every_key_and_escape_closes() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 96,
                h: 40,
            });
            backend
                .dispatch(Msg::RunAction(Action::ToggleHelp))
                .expect("open help");
            backend.render();
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::help_filter_key())
            );
            let pane_id = backend.state().focused_pane().expect("focused pane");
            let epoch = backend.state().runtime_epoch;
            let generation = backend
                .state()
                .current()
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.panes.iter())
                .find(|pane| pane.id == pane_id)
                .expect("focused pane record")
                .pty_generation;
            backend
                .dispatch(Msg::ActivatePane(epoch, pane_id, generation))
                .expect("pane activate while help is open");
            backend.render();
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::help_filter_key()),
                "the search field reclaims focus"
            );
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(frame.contains("Keybindings"));
            assert!(
                frame.contains("╭Keybindings─"),
                "title should sit flush on the border like other modals: {frame}"
            );
            assert!(frame.contains("│ Search keybindings…"));
            assert!(
                !frame.contains('├') && !frame.contains('┤'),
                "the search divider must not join the frame, like other pickers: {frame}"
            );
            assert!(
                frame.contains("────────"),
                "the search divider should still draw a muted rule: {frame}"
            );
            // The first row is the prefix scheme row, which has no unbind/reset to advertise.
            assert!(frame.contains("change Enter"), "{frame}");
            assert!(!frame.contains("switch tabs ←/→"), "{frame}");
            assert!(!frame.contains("unbind Ctrl+U"), "{frame}");
            assert!(!frame.contains("edit e"), "no separate edit mode:\n{frame}");
            assert!(!frame.contains("╭─ Keybindings"));
            assert!(!frame.contains("Keybindings · Edit"));
            assert!(frame.contains("Search"));
            assert!(frame.contains("Global"));
            assert!(frame.contains("Ctrl+A"));
            assert!(frame.contains("Prefix · then key"));
            assert!(frame.contains("Alt"));
            assert!(frame.contains("Mod · hold + key"));
            assert!(!frame.contains("Prefix keys with"));
            assert!(!frame.contains("Mod Alt"));
            assert!(
                !frame.contains("Edit scrollback"),
                "Global tab hides unbound commands: {frame}"
            );
            assert!(
                !frame.contains("SIDEBAR FOCUSED"),
                "Global tab hides direct mode keys: {frame}"
            );
            // Former mode keys are ordinary query text now.
            for code in [KeyCode::Char('/'), KeyCode::Char('e')] {
                press(&mut backend, code);
            }
            assert_eq!(help_state(&mut backend).query.text(), "/e");
            help_state(&mut backend).query = TextInput::new("");

            press(&mut backend, KeyCode::Right);
            assert_eq!(help_state(&mut backend).tab, crate::state::HelpTab::Modes);
            let modes = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(modes.contains("COPY MODE"));
            assert!(modes.contains("SIDEBAR FOCUSED"));
            assert!(modes.contains("DIRECT"));
            assert!(modes.contains("Exit copy mode"));
            assert!(
                !modes.contains("Prefix · then key"),
                "Modes omits scheme rows: {modes}"
            );
            assert!(
                !modes.contains("Mod · hold + key"),
                "Modes omits scheme rows: {modes}"
            );
            backend
                .dispatch(Msg::HelpTabSelected(2))
                .expect("show unbound bindings");
            backend.render();
            let unbound = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(unbound.contains("Edit scrollback"));
            assert!(unbound.contains("—"));
            assert!(!unbound.contains("not set"));
            backend
                .dispatch(Msg::HelpTabSelected(3))
                .expect("show all bindings");
            backend.render();
            let all = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(all.contains("Edit scrollback"));
            assert!(all.contains("Prefix · then key"));
            assert!(all.contains("Mod · hold + key"));
            assert_eq!(help_state(&mut backend).tab, crate::state::HelpTab::All);
            press(&mut backend, KeyCode::Char('z'));
            assert_eq!(help_state(&mut backend).query.text(), "z");
            press(&mut backend, KeyCode::Esc);
            assert!(
                backend.state().keybindings.is_none(),
                "Esc closes, query or not"
            );
            assert!(!backend.modifier_key_reporting_enabled());

            // Reopening starts fresh.
            backend
                .dispatch(Msg::RunAction(Action::ToggleHelp))
                .expect("reopen help");
            let reopened = help_state(&mut backend);
            assert!(reopened.query.text().is_empty());
            assert_eq!(reopened.tab, crate::state::HelpTab::Global);
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
                state.keybindings = Some(crate::state::KeybindingsState::default());
                state.overlay_return = Some(crate::state::OverlayOrigin::Settings);
            }

            backend
                .dispatch(Msg::RunAction(Action::OpenAlerts))
                .unwrap();

            assert!(!backend.state().show_session_picker);
            assert!(backend.state().session_picker.is_none());
            assert!(backend.state().keybindings.is_none());
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

    /// The startup row is a value ring: Shift+Enter cycles immediately. What reaches `[session]` is
    /// pinned deterministically in `config::persist` instead - these tests share one scratch config
    /// file and run in parallel, so reading it back here would race a sibling's write.
    #[test]
    fn settings_shift_enter_cycles_startup_mode() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Ephemeral
            );
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Last
            );
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::CycleStartupMode)
            );
            assert!(backend.state().show_settings, "the dialog stays open");

            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            backend.dispatch(Msg::SettingsChoicePick(0)).unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Picker
            );
        });
    }

    #[test]
    fn settings_shift_enter_cycles_foreground_restore() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleResurrectForeground,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.resurrect_foreground,
                crate::config::ForegroundRestore::Never
            );
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleResurrectForeground,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.resurrect_foreground,
                crate::config::ForegroundRestore::Hold
            );
            assert_eq!(
                backend.state().settings_selected,
                Some(crate::state::SettingsAction::CycleResurrectForeground)
            );
            assert!(backend.state().show_settings, "the dialog stays open");

            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::CycleResurrectForeground,
                ))
                .unwrap();
            backend.dispatch(Msg::SettingsChoicePick(2)).unwrap();
            assert_eq!(
                backend.state().config.session.resurrect_foreground,
                crate::config::ForegroundRestore::Auto
            );
        });
    }

    /// `profile` mode has nothing to open without a default profile, so the row does not stop there
    /// until one is set. Stepping back from `picker` is the shortest way to reach it.
    #[test]
    fn settings_offers_profile_startup_only_once_a_default_profile_exists() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            assert!(backend.state().config.profile.default.is_none());
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Ephemeral
            );
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Picker,
                "with no default profile the ring wraps straight past `profile`"
            );

            backend.state_mut().config.profile.default = Some("dev".to_string());
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleStartupMode,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Profile,
                "starring a profile puts the mode back in the ring"
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
