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
        ctx.state.settings_number_editor = None;
        discard_settings_choice(ctx);
    }
    let cycle_layout_in_palette = matches!(action, Action::ToggleLayout) && ctx.state.show_palette;
    let from_palette = ctx.state.show_palette;
    let handoff_label = from_palette
        .then(|| command_palette_handoff_label(ctx, action))
        .flatten();
    if let Some(label) = handoff_label {
        ctx.state.command_palette_handoff_epoch =
            ctx.state.command_palette_handoff_epoch.wrapping_add(1);
        let epoch = ctx.state.command_palette_handoff_epoch;
        ctx.state.command_palette_handoff = Some(crate::state::CommandPaletteHandoff {
            epoch,
            label,
            running: false,
        });
        if let Some(link) = ctx.state.command_link.clone() {
            link.send_after(
                super::COMMAND_PALETTE_HANDOFF_QUIET,
                Msg::CommandPaletteHandoffRunning { epoch },
            );
        }
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
        Action::MarkPaneRecording | Action::MarkUiRecording
            if ctx.state.recording_mark.is_some() =>
        {
            crate::ops::focus::request_recording_mark_focus(ctx)
        }
        Action::RenameWorkspace | Action::RenameSession => request_rename_session_focus(ctx),
        Action::OpenSettings
        | Action::OpenExtensions
        | Action::OpenAppearance
        | Action::OpenAlerts
        | Action::OpenThemePicker
        | Action::OpenLayoutPicker
        | Action::SidebarTabs => {}
        Action::SaveProfile
        | Action::OpenProfilePicker
        | Action::OpenWorktrees
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

/// The label of the extension command `action` runs, when Commands should hand off to it: a
/// detached extension command, which may open a picker of its own.
fn command_palette_handoff_label(ctx: &Context<AppRoot>, action: Action) -> Option<String> {
    let Action::RunNamedCommand(index) = action else {
        return None;
    };
    let command = ctx.state.config.commands.get(index)?;
    if !command.env.iter().any(|(key, _)| key == "ROZI_EXTENSION") {
        return None;
    }
    matches!(
        command.action,
        UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. }
    )
    .then(|| command.label())
}

fn is_current_handoff(ctx: &Context<AppRoot>, epoch: u64) -> bool {
    ctx.state
        .command_palette_handoff
        .as_ref()
        .is_some_and(|handoff| handoff.epoch == epoch)
}

/// Commands is still waiting on the command a moment after it started, so it says what it is
/// waiting on. A command that opens its picker or exits at once never gets here, and goes straight
/// from Commands to its picker or back to the panes.
pub(super) fn command_palette_handoff_running(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if !ctx.state.show_palette || !is_current_handoff(ctx, epoch) {
        return Update::none();
    }
    if let Some(handoff) = ctx.state.command_palette_handoff.as_mut() {
        handoff.running = true;
    }
    Update::full()
}

pub(super) fn command_palette_handoff_finished(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if !is_current_handoff(ctx, epoch) {
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
    ctx.state.settings_number_editor = None;
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
    if action.previews_choice() {
        apply_settings_choice(ctx, action, index, false);
    }
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
    if action.disabled_reason_for_state(&ctx.state).is_some() {
        ctx.request_focus(crate::view::settings_palette_key());
        return Update::full();
    }
    let Some(ring) = action.choice_ring(&ctx.state.config) else {
        return Update::none();
    };
    ctx.state.settings_choice = Some(crate::state::SettingsChoiceEditor::from_ring(
        action,
        ring,
        &ctx.state.config,
    ));
    crate::state::assign_settings_selection(&mut ctx.state, Some(action));
    ctx.request_focus(crate::view::settings_choice_key());
    Update::full()
}

fn settings_apply(ctx: &mut Context<AppRoot>, action: crate::state::SettingsAction) -> Update {
    use crate::state::{SettingControl, SettingsAction::*};

    if action.disabled_reason_for_state(&ctx.state).is_some() {
        ctx.request_focus(crate::view::settings_palette_key());
        return Update::full();
    }
    match &action.spec().control {
        SettingControl::Toggle { field, key, effect } => {
            let slot = (field.slot)(&mut ctx.state.config);
            *slot = !*slot;
            let value = *slot;
            let saved = key.save_flag(value);
            setting_saved(ctx, *effect, saved);
        }
        SettingControl::ActionToggle { action, .. } => {
            execute_action(ctx, *action);
        }
        SettingControl::Choice(_) => return open_settings_choice(ctx, action),
        SettingControl::Cycle(_) => return cycle_settings_choice(ctx, action),
        SettingControl::Custom => match action {
            Theme => {
                execute_action(ctx, Action::OpenThemePicker);
            }
            EditPadding | EditScrollMultiplier => {
                ctx.state.settings_number_editor =
                    crate::state::SettingsNumberEditor::for_action(action, &ctx.state.config);
                ctx.request_focus(crate::view::settings_number_field_key(0));
            }
            SidebarTabs => return crate::update::sidebar::open_manager(ctx),
            ToggleSidebarStartup => {
                ctx.state.config.sidebar.startup = !ctx.state.config.sidebar.startup;
                crate::update::sidebar::save_layout(ctx);
            }
            _ => return open_settings_choice(ctx, action),
        },
    }
    if !matches!(action, Theme | EditPadding | EditScrollMultiplier) {
        ctx.state.show_settings = true;
        crate::state::assign_settings_selection(&mut ctx.state, Some(action));
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

/// Bring runtime state in line with a changed setting.
fn apply_setting_effect(ctx: &mut Context<AppRoot>, effect: crate::state::SettingEffect) {
    use crate::state::SettingEffect;

    match effect {
        SettingEffect::Clipboard => {
            ctx.set_clipboard_config(crate::app::clipboard_config(&ctx.state.config));
        }
        SettingEffect::WhichKeyDelay => {
            ctx.set_command_chord_reveal_delay(ctx.state.config.input.which_key.reveal_delay());
        }
        SettingEffect::SleepPolicy => crate::ops::config::reload_sleep_policy(ctx),
    }
}

/// Finish a saved setting change: report a failed save, and run the setting's effect, which an
/// effect that reads the saved file only does once the save went through.
fn setting_saved(
    ctx: &mut Context<AppRoot>,
    effect: Option<crate::state::SettingEffect>,
    saved: std::result::Result<std::path::PathBuf, String>,
) {
    let ok = saved.is_ok();
    if let Err(err) = saved {
        preference_error(ctx, err);
    }
    if let Some(effect) = effect
        && (ok || !effect.after_save())
    {
        apply_setting_effect(ctx, effect);
    }
}

fn preference_error(ctx: &mut Context<AppRoot>, err: String) {
    crate::pane::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::PreferenceSave,
        Some("Preference not saved".to_string()),
        err,
    );
}

fn persist_titlebar_choice(ctx: &mut Context<AppRoot>) {
    let pane = &ctx.state.config.pane;
    if pane.show_titles {
        let titlebar = format!("\"{}\"", pane.titlebar.id());
        if let Err(err) = crate::config::persist_section_values(
            "pane",
            &[("show_titles", "true"), ("titlebar", &titlebar)],
        ) {
            preference_error(ctx, err);
        }
    } else if let Err(err) = crate::config::persist_pane_flag("show_titles", false) {
        preference_error(ctx, err);
    }
}

fn persist_workbar_choice(ctx: &mut Context<AppRoot>) {
    let pane = &ctx.state.config.pane;
    if pane.show_workbar {
        let position = if pane.workbar_at_bottom {
            "true"
        } else {
            "false"
        };
        if let Err(err) = crate::config::persist_section_values(
            "pane",
            &[("show_workbar", "true"), ("workbar_at_bottom", position)],
        ) {
            preference_error(ctx, err);
        }
    } else if let Err(err) = crate::config::persist_pane_flag("show_workbar", false) {
        preference_error(ctx, err);
    }
}

fn cycle_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
) -> Update {
    if action.disabled_reason_for_state(&ctx.state).is_some() {
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
    if let Some(delay) = crate::state::abandon_settings_choice(&mut ctx.state) {
        ctx.set_command_chord_reveal_delay(delay);
    }
}

fn apply_settings_choice(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
    index: usize,
    persist: bool,
) {
    use crate::state::SettingsAction::*;

    if !action.apply_choice(&mut ctx.state.config, index) {
        return;
    }
    if let Some(choice) = action.choice() {
        let effect = choice.effect();
        if let Some(effect) = effect.filter(|effect| !effect.after_save()) {
            apply_setting_effect(ctx, effect);
        }
        if persist {
            let saved = choice.save(&ctx.state.config);
            setting_saved(ctx, effect.filter(|effect| effect.after_save()), saved);
        }
        return;
    }
    if !persist {
        return;
    }
    match action {
        LeftSidebarPanels | RightSidebarPanels => {
            let side = if action == LeftSidebarPanels {
                crate::config::SidebarPosition::Left
            } else {
                crate::config::SidebarPosition::Right
            };
            crate::update::sidebar::set_panel_count(ctx, side, index + 1);
            let _ = crate::update::sidebar::visibility_changed(ctx);
        }
        SidebarLayoutPreset => crate::update::sidebar::apply_layout_preset(ctx, index),
        ChooseTitlebar => persist_titlebar_choice(ctx),
        ChooseWorkbar => persist_workbar_choice(ctx),
        _ => {}
    }
}

pub(super) fn close_settings_number_editor(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.settings_number_editor.is_none() {
        return Update::none();
    }
    ctx.state.settings_number_editor = None;
    if ctx.state.show_settings {
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

pub(super) fn settings_number_changed(
    ctx: &mut Context<AppRoot>,
    field: usize,
    event: InputEvent,
) -> Update {
    let Some(editor) = ctx.state.settings_number_editor.as_mut() else {
        return Update::none();
    };
    let Some(slot) = editor.fields.get_mut(field) else {
        return Update::none();
    };
    if slot.accepts(&event.value) {
        event.apply_to(&mut slot.input);
    }
    editor.focus = field;
    ctx.request_focus(crate::view::settings_number_field_key(field));
    Update::full()
}

/// Record where focus landed, so the dialog can mark the active field. Reported by the field
/// itself, which covers a click and `Tab` traversal alike.
pub(super) fn settings_number_focus(ctx: &mut Context<AppRoot>, field: usize) -> Update {
    let Some(editor) = ctx.state.settings_number_editor.as_mut() else {
        return Update::none();
    };
    if field < editor.fields.len() {
        editor.focus = field;
    }
    Update::full()
}

/// Enter on a field: refuse an empty or out-of-range value, otherwise move to the next field, or
/// apply every field from the last one.
pub(super) fn settings_number_enter(ctx: &mut Context<AppRoot>, field: usize) -> Update {
    let Some(editor) = ctx.state.settings_number_editor.as_mut() else {
        return Update::none();
    };
    let Some(slot) = editor.fields.get(field) else {
        return Update::none();
    };
    if slot.value().is_none() {
        let (title, hint) = (editor.invalid_title, slot.range_hint());
        editor.focus = field;
        crate::pane::pty_events::notify_error(ctx, title, hint);
        ctx.request_focus(crate::view::settings_number_field_key(field));
        return Update::full();
    }
    if !editor.is_last(field) {
        editor.focus = field + 1;
        ctx.request_focus(crate::view::settings_number_field_key(field + 1));
        return Update::full();
    }
    // Every field before this one passed its own Enter, but a click can land on the last field
    // first, so check them all again.
    if let Some(invalid) = editor.fields.iter().position(|slot| slot.value().is_none()) {
        let (title, hint) = (editor.invalid_title, editor.fields[invalid].range_hint());
        editor.focus = invalid;
        crate::pane::pty_events::notify_error(ctx, title, hint);
        ctx.request_focus(crate::view::settings_number_field_key(invalid));
        return Update::full();
    }
    let action = editor.action;
    let values: Vec<u16> = editor
        .fields
        .iter()
        .filter_map(|slot| slot.value())
        .collect();
    apply_settings_number(ctx, action, &values);
    ctx.state.settings_number_editor = None;
    if ctx.state.show_settings {
        ctx.request_focus(crate::view::settings_palette_key());
    }
    Update::full()
}

fn apply_settings_number(
    ctx: &mut Context<AppRoot>,
    action: crate::state::SettingsAction,
    values: &[u16],
) {
    use crate::state::SettingsAction::*;
    match (action, values) {
        (EditPadding, &[vertical, horizontal]) => {
            ctx.state.config.pane.padding = (vertical, horizontal, vertical, horizontal);
            if let Err(error) = crate::config::persist_pane_padding(vertical, horizontal) {
                crate::pane::pty_events::notify_error(ctx, "Padding not saved", error);
            }
        }
        (EditScrollMultiplier, &[multiplier]) => {
            ctx.state.config.input.scroll_multiplier = multiplier;
            ctx.set_scroll_wheel_multiplier(multiplier);
            if let Err(err) = crate::config::persist_section_values(
                "input",
                &[("scroll_multiplier", &multiplier.to_string())],
            ) {
                preference_error(ctx, err);
            }
        }
        _ => {}
    }
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
    let command = crate::schedule_workbar_tick(ctx);
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

pub(super) fn background_error(
    ctx: &mut Context<AppRoot>,
    title: &'static str,
    message: String,
) -> Update {
    crate::pane::pty_events::notify_error(ctx, title, message);
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

    fn frame(backend: &mut TestBackend<AppRoot>) -> String {
        backend.render();
        backend.capture_frame().to_fixed_grid_lines().join("\n")
    }

    fn sized(backend: &mut TestBackend<AppRoot>) {
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
    }

    #[test]
    fn the_clock_tick_keeps_its_wall_anchor_on_a_controlled_clock() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            sized(&mut backend);
            backend
                .state_mut()
                .config
                .workbar
                .right
                .push(crate::config::WorkbarItem {
                    segment: crate::config::WorkbarSegment::Clock,
                    color: None,
                });
            backend.dispatch(Msg::WorkbarTick).unwrap();
            let anchor = backend.state().wall_anchor.get();
            assert!(anchor.is_some(), "the tick read the wall clock's phase");
            // Each advanced second fires one tick and leaves the real wall clock behind the runtime
            // clock, which must not read as a jump that retakes the anchor.
            for _ in 0..5 {
                backend.advance(std::time::Duration::from_secs(1));
                backend.pump().unwrap();
                assert_eq!(backend.state().wall_anchor.get(), anchor);
            }
        });
    }

    #[test]
    fn a_long_handoff_says_what_commands_is_waiting_on() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            sized(&mut backend);
            backend
                .dispatch(Msg::RunAction(Action::TogglePalette))
                .unwrap();
            backend.render();
            backend.advance(std::time::Duration::from_millis(200));
            {
                let state = backend.state_mut();
                state.command_palette_handoff = Some(crate::state::CommandPaletteHandoff {
                    epoch: 3,
                    label: "Install hooks".to_string(),
                    running: false,
                });
            }
            // A timer from an earlier selection changes nothing.
            backend
                .dispatch(Msg::CommandPaletteHandoffRunning { epoch: 2 })
                .unwrap();
            assert!(!frame(&mut backend).contains("Running"));
            backend
                .dispatch(Msg::CommandPaletteHandoffRunning { epoch: 3 })
                .unwrap();
            let shown = frame(&mut backend);
            assert!(shown.contains("Running “Install hooks”…"), "{shown}");
            assert!(shown.contains("the command keeps running"), "{shown}");
            // Leaving does not wait for the command.
            backend
                .send_key(tui_lipan::prelude::KeyEvent {
                    code: tui_lipan::prelude::KeyCode::Esc,
                    mods: tui_lipan::prelude::KeyMods::NONE,
                })
                .unwrap();
            assert!(!backend.state().show_palette);
            assert!(backend.state().command_palette_handoff.is_none());
            // The command finishing later finds nothing to close.
            backend
                .dispatch(Msg::CommandPaletteHandoffFinished { epoch: 3 })
                .unwrap();
            assert!(!backend.state().show_palette);
        });
    }

    // Needs a command that is still running when the assertions run, and `sleep` is the portable
    // way to have one on Unix.
    #[cfg(unix)]
    #[test]
    fn an_extension_command_started_from_commands_is_handed_off_under_its_label() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            sized(&mut backend);
            {
                let state = backend.state_mut();
                state.config.commands = vec![crate::config::NamedCommand {
                    id: "tools.slow".to_string(),
                    label: Some("Slow probe".to_string()),
                    action: crate::config::UserCommandAction::ExecDirect {
                        argv: vec!["sleep".to_string(), "2".to_string()],
                    },
                    category: "Tools".to_string(),
                    env: vec![("ROZI_EXTENSION".to_string(), "tools".to_string())],
                    default_key: None,
                    hidden: false,
                }];
                state.commands_dirty = true;
            }
            backend
                .dispatch(Msg::RunAction(Action::TogglePalette))
                .unwrap();
            backend.render();
            for ch in "slow".chars() {
                backend
                    .send_key(tui_lipan::prelude::KeyEvent {
                        code: tui_lipan::prelude::KeyCode::Char(ch),
                        mods: tui_lipan::prelude::KeyMods::NONE,
                    })
                    .unwrap();
            }
            backend.advance(std::time::Duration::from_millis(200));
            let listed = frame(&mut backend);
            assert!(listed.contains("Slow probe"), "{listed}");
            backend
                .dispatch(Msg::RunAction(Action::RunNamedCommand(0)))
                .unwrap();
            assert!(
                backend.state().show_palette,
                "Commands waits on the command"
            );
            let handoff = backend
                .state()
                .command_palette_handoff
                .as_ref()
                .expect("handed off");
            assert_eq!(handoff.label, "Slow probe");
            assert!(!handoff.running, "quiet until the command has had a moment");
        });
    }

    #[test]
    fn padding_input_accepts_empty_or_one_ascii_digit_in_range() {
        let editor = crate::state::SettingsNumberEditor::padding((0, 0, 0, 0));
        let field = &editor.fields[0];
        assert!(field.accepts(""));
        assert!(field.accepts("8"));
        assert!(!field.accepts("9"));
        assert!(!field.accepts("12"));
        assert!(!field.accepts("８"));
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
    fn a_background_error_becomes_a_toast() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend
                .dispatch(Msg::BackgroundError {
                    title: "Control endpoint accept failed",
                    message: "too many open files".to_string(),
                })
                .unwrap();
            let toast = last_toast(&backend);
            assert!(toast.contains("Control endpoint accept failed"), "{toast}");
            assert!(toast.contains("too many open files"), "{toast}");
        });
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
                crate::state::AlertMode::Off
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
                crate::state::AlertMode::Off
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
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
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
            assert!(placeholder.contains("54/54 │"), "{placeholder}");

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
    fn keybindings_navigation_visits_every_row_in_both_directions() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 96,
                h: 40,
            });
            backend
                .dispatch(Msg::RunAction(Action::ToggleHelp))
                .unwrap();
            for tab in crate::state::HelpTab::ORDER {
                backend.dispatch(Msg::HelpTabSelected(tab.index())).unwrap();
                backend.render();
                press(&mut backend, KeyCode::End);
                let last = help_state(&mut backend).selected.clone().unwrap();
                press(&mut backend, KeyCode::Home);
                let first = help_state(&mut backend).selected.clone().unwrap();
                let mut visited = std::collections::HashSet::new();
                loop {
                    let id = help_state(&mut backend).selected.clone().unwrap();
                    assert!(
                        visited.insert(id.clone()),
                        "navigation stuck in {tab:?} at {id}"
                    );
                    if id == last {
                        break;
                    }
                    press(&mut backend, KeyCode::Down);
                }
                assert!(visited.len() > 1);
                press(&mut backend, KeyCode::Down);
                assert_eq!(help_state(&mut backend).selected.as_ref(), Some(&first));
                press(&mut backend, KeyCode::Up);
                assert_eq!(help_state(&mut backend).selected.as_ref(), Some(&last));
                loop {
                    let id = help_state(&mut backend).selected.clone().unwrap();
                    assert!(
                        visited.remove(&id),
                        "reverse navigation stuck in {tab:?} at {id}"
                    );
                    if id == first {
                        break;
                    }
                    press(&mut backend, KeyCode::Up);
                }
                assert!(visited.is_empty());
            }
        });
    }

    #[test]
    fn keybindings_search_owns_every_key_and_escape_closes() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
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

    #[test]
    fn cursor_blink_is_saved_from_settings() {
        on_large_stack(|| {
            let _config = crate::test_support::lock_config_file();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            let action = crate::state::SettingsAction::CycleCursorBlink;
            backend.update_level(Msg::SettingsActivate(action)).unwrap();
            assert_eq!(
                backend.state().settings_choice.as_ref().unwrap().options,
                ["Inherit", "Static", "Blinking"]
            );
            backend.update_level(Msg::SettingsChoicePick(1)).unwrap();
            assert_eq!(
                backend.state().config.cursor.blink,
                crate::config::CursorBlink::Static
            );
            assert_eq!(
                crate::config::load_config().config.cursor.blink,
                crate::config::CursorBlink::Static
            );
            assert!(backend.state().show_settings);
        });
    }

    #[test]
    fn sessions_opening_preference_is_saved_from_settings() {
        on_large_stack(|| {
            let _config = crate::test_support::lock_config_file();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            backend.state_mut().config.session.picker_open_on =
                crate::config::SessionPickerOpenOn::CurrentHost;
            let action = crate::state::SettingsAction::CycleSessionPickerOpenOn;
            backend.update_level(Msg::SettingsActivate(action)).unwrap();
            let editor = backend.state().settings_choice.as_ref().unwrap();
            assert_eq!(editor.options, ["Current host", "All sessions"]);
            backend.update_level(Msg::SettingsChoicePick(1)).unwrap();
            assert_eq!(
                backend.state().config.session.picker_open_on,
                crate::config::SessionPickerOpenOn::All
            );
            assert_eq!(
                crate::config::load_config().config.session.picker_open_on,
                crate::config::SessionPickerOpenOn::All
            );
            assert!(backend.state().show_settings);
            backend
                .update_level(Msg::SettingsCycleChoice(action))
                .unwrap();
            assert_eq!(
                crate::config::load_config().config.session.picker_open_on,
                crate::config::SessionPickerOpenOn::CurrentHost
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
    fn settings_shared_attach_cycles_and_persists() {
        on_large_stack(|| {
            let _guard = crate::test_support::lock_config_file();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            let action = crate::state::SettingsAction::CycleSessionOnAttach;
            use crate::config::SessionOnAttach::{Ask, Follow, Take};
            for expected in [Follow, Take, Ask] {
                backend.dispatch(Msg::SettingsCycleChoice(action)).unwrap();
                assert_eq!(backend.state().config.session.on_attach, expected);
                assert_eq!(backend.state().settings_selected, Some(action));
                assert!(backend.state().show_settings);
                let saved = crate::config::load_config();
                assert_eq!(saved.config.session.on_attach, expected);
            }
            backend.dispatch(Msg::SettingsActivate(action)).unwrap();
            backend.dispatch(Msg::SettingsChoicePick(2)).unwrap();
            assert_eq!(backend.state().config.session.on_attach, Take);
            assert_eq!(crate::config::load_config().config.session.on_attach, Take);
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
                crate::config::ForegroundRestore::Auto
            );
            backend
                .dispatch(Msg::SettingsCycleChoice(
                    crate::state::SettingsAction::CycleResurrectForeground,
                ))
                .unwrap();
            assert_eq!(
                backend.state().config.session.resurrect_foreground,
                crate::config::ForegroundRestore::Never
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

    #[test]
    fn settings_scroll_multiplier_applies_and_saves_a_typed_value() {
        on_large_stack(|| {
            use tui_lipan::core::event::{KeyMods, MouseEvent, MouseKind};

            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 96,
                h: 40,
            });
            let tab_id = crate::config::SidebarTabId::new("launcher");
            {
                let state = backend.state_mut();
                state.sidebar.shown = [true, false];
                state.config.animations.sidebar = false;
                state.config.sidebar.layout.left.panel_count = 1;
                state.config.sidebar.tabs = vec![crate::config::SidebarTab::Launcher {
                    name: tab_id.clone(),
                    label: "Launch".to_string(),
                    entries: (0..40)
                        .map(|i| crate::config::SidebarLauncherEntry {
                            label: format!("entry-{i:02}"),
                            group: None,
                            action: crate::config::UserCommandAction::Send(format!("{i}")),
                        })
                        .collect(),
                    env: Vec::new(),
                }];
                state.sidebar.panels[0].tabs = vec![tab_id.clone()];
                state.sidebar.panels[0].active_tab = Some(tab_id);
            }
            backend.render();

            let wheel_down = MouseEvent {
                x: 4,
                y: 4,
                kind: MouseKind::ScrollDown,
                mods: KeyMods::NONE,
            };
            assert!(backend.send_mouse(wheel_down).unwrap());
            let after_default = backend.capture_frame().to_fixed_grid();
            // Each launcher entry is 2 lines tall, so offset 3 scrolls past entry-00 and the
            // title line of entry-01, leaving entry-02 as the first full entry in view.
            assert!(
                !after_default.contains("entry-01") && after_default.contains("entry-02"),
                "default multiplier 3 should advance sidebar by 3 lines:\n{after_default}"
            );

            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsActivate(
                    crate::state::SettingsAction::EditScrollMultiplier,
                ))
                .unwrap();
            backend.render();
            let field = |backend: &TestBackend<AppRoot>| {
                backend
                    .state()
                    .settings_number_editor
                    .as_ref()
                    .unwrap()
                    .fields[0]
                    .input
                    .text()
                    .to_string()
            };
            assert_eq!(
                backend
                    .state()
                    .settings_number_editor
                    .as_ref()
                    .unwrap()
                    .title,
                "Scroll multiplier"
            );
            assert_eq!(field(&backend), "3");

            backend.send_paste("51").unwrap();
            assert_eq!(field(&backend), "3", "above the maximum is refused");

            backend.send_paste("0").unwrap();
            assert_eq!(field(&backend), "0", "typing replaces the selected value");
            press(&mut backend, KeyCode::Enter);
            assert!(
                backend.state().settings_number_editor.is_some(),
                "below the minimum keeps the editor open"
            );
            assert_eq!(backend.state().config.input.scroll_multiplier, 3);

            press(&mut backend, KeyCode::Backspace);
            backend.send_paste("7").unwrap();
            press(&mut backend, KeyCode::Enter);
            assert!(backend.state().settings_number_editor.is_none());
            assert_eq!(backend.state().config.input.scroll_multiplier, 7);
            assert!(backend.state().show_settings, "Settings stays open");

            let saved = std::fs::read_to_string(crate::config::config_path()).unwrap();
            assert!(saved.contains("[input]\nscroll_multiplier = 7"), "{saved}");

            backend.state_mut().show_settings = false;
            backend.render();
            assert!(backend.send_mouse(wheel_down).unwrap());
            let after_updated = backend.capture_frame().to_fixed_grid();
            assert!(
                !after_updated.contains("entry-04") && after_updated.contains("entry-05"),
                "updated multiplier 7 should advance sidebar from line 3 to line 10 (entry-05):\n{after_updated}"
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
    fn sleep_policy_settings_cannot_change_local_config_while_attached_remotely() {
        on_large_stack(|| {
            let _config = crate::test_support::lock_config_file();
            crate::config::persist_session_flag("keep_awake_while_agents_work", false).unwrap();
            let before = std::fs::read(crate::config::config_path()).unwrap();
            let mut backend = TestBackend::new(AppRoot::default());
            backend
                .state_mut()
                .config
                .session
                .keep_awake_while_agents_work = false;
            backend.state_mut().current_mut().remote_target = Some(
                crate::session::remote::RemoteTarget::Alias("workbox".into()),
            );
            let action = crate::state::SettingsAction::ToggleKeepAwakeWhileAgentsWork;
            for message in [
                Msg::SettingsActivate(action),
                Msg::SettingsCycleChoice(action),
            ] {
                backend.dispatch(message).unwrap();
                assert!(!backend.state().config.session.keep_awake_while_agents_work);
                assert_eq!(std::fs::read(crate::config::config_path()).unwrap(), before);
            }
            backend.state_mut().current_mut().remote_target = None;
            backend.dispatch(Msg::SettingsActivate(action)).unwrap();
            assert!(backend.state().config.session.keep_awake_while_agents_work);
            assert!(
                crate::config::load_config()
                    .config
                    .session
                    .keep_awake_while_agents_work
            );
        });
    }

    #[test]
    fn settings_toggles_session_flags() {
        on_large_stack(|| {
            let _config = crate::test_support::lock_config_file();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().show_settings = true;
            for (action, key) in [
                (
                    crate::state::SettingsAction::ToggleSessionAutosave,
                    "autosave",
                ),
                (
                    crate::state::SettingsAction::ToggleKeepAwakeWhileAgentsWork,
                    "keep_awake_while_agents_work",
                ),
                (
                    crate::state::SettingsAction::ToggleSessionResurrect,
                    "resurrect",
                ),
            ] {
                let before = match key {
                    "autosave" => backend.state().config.session.autosave,
                    "keep_awake_while_agents_work" => {
                        backend.state().config.session.keep_awake_while_agents_work
                    }
                    _ => backend.state().config.session.resurrect,
                };
                backend.dispatch(Msg::SettingsActivate(action)).unwrap();
                let after = match key {
                    "autosave" => backend.state().config.session.autosave,
                    "keep_awake_while_agents_work" => {
                        backend.state().config.session.keep_awake_while_agents_work
                    }
                    _ => backend.state().config.session.resurrect,
                };
                assert_eq!(after, !before, "{key} should toggle");
                let saved = crate::config::load_config();
                if key == "keep_awake_while_agents_work" {
                    assert_eq!(saved.config.session.keep_awake_while_agents_work, after);
                }
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
