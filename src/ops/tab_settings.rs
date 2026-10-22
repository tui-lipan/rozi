//! A sidebar tab's settings picker: open it, edit one setting at a time, and write each change to
//! the table its owner keeps settings in.
//!
//! Every change is written to `config.toml` and then reloaded, rather than applied to the running
//! config by hand. That is the one path that applies a built-in tab's settings and an extension's
//! alike: a built-in tab rebuilds from its table, and an extension's processes restart with the
//! new value in their environment, exactly as if the user had edited the file.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{SettingKind, SettingSpec, SettingValue, SettingsGroup};
use crate::state::{TabSettingPrompt, TabSettingsState};

/// Focus key of the settings list.
pub(crate) const TAB_SETTINGS_KEY: &str = "tab-settings";
/// Focus key of a choice setting's value list. Distinct from the settings list so opening it
/// starts with an empty search rather than the settings query.
pub(crate) const TAB_SETTING_CHOICES_KEY: &str = "tab-setting-choices";
/// Focus key of the typed-value prompt.
pub(crate) const TAB_SETTING_PROMPT_KEY: &str = "tab-setting-prompt";

/// The settings group of an open picker's tab, if the tab still offers any.
pub(crate) fn open_group(state: &crate::state::State) -> Option<&SettingsGroup> {
    let picker = state.tab_settings.as_ref()?;
    state.config.sidebar.tab_settings.get(&picker.tab)
}

/// Whether `tab` has settings a picker can show.
pub(crate) fn tab_has_settings(state: &crate::state::State, tab: &str) -> bool {
    state
        .config
        .sidebar
        .tab_settings
        .get(tab)
        .is_some_and(|group| !group.specs.is_empty())
}

pub(crate) fn open(ctx: &mut Context<AppRoot>, tab: String) -> Update {
    let Some(first) = ctx
        .state
        .config
        .sidebar
        .tab_settings
        .get(&tab)
        .and_then(|group| group.specs.first())
        .map(|spec| spec.key.clone())
    else {
        return Update::none();
    };
    let mut picker = TabSettingsState::new(tab);
    picker.selected = Some(first);
    ctx.state.tab_settings = Some(picker);
    ctx.request_focus(TAB_SETTINGS_KEY);
    Update::full()
}

/// Esc: close an open choice list, otherwise leave the picker for Sidebar tabs.
pub(crate) fn back(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.tab_settings.as_mut() else {
        return Update::none();
    };
    if picker.choosing.take().is_some() {
        picker.choice_highlight = None;
        ctx.request_focus(TAB_SETTINGS_KEY);
        return Update::full();
    }
    let tab = picker.tab.clone();
    ctx.state.tab_settings = None;
    if let Some(manager) = ctx.state.sidebar_manager.as_mut() {
        manager.selected = Some(tab);
        ctx.request_focus("sidebar-manager");
    }
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.tab_settings.as_mut()
        && picker.choosing.is_none()
    {
        picker.query = query;
    }
    Update::none()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, key: String) -> Update {
    if let Some(picker) = ctx.state.tab_settings.as_mut()
        && picker.choosing.is_none()
    {
        picker.selected = Some(key);
    }
    Update::full()
}

pub(crate) fn activate(ctx: &mut Context<AppRoot>, key: String) -> Update {
    let Some(group) = open_group(&ctx.state) else {
        return Update::none();
    };
    let Some(spec) = group.spec(&key).cloned() else {
        return Update::none();
    };
    let value = group
        .value(&key)
        .cloned()
        .unwrap_or_else(|| spec.default.clone());
    if let Some(picker) = ctx.state.tab_settings.as_mut() {
        picker.selected = Some(key.clone());
    }
    match (&spec.kind, &value) {
        (SettingKind::Toggle, SettingValue::Bool(current)) => {
            write(ctx, &key, Some(SettingValue::Bool(!current)))
        }
        (SettingKind::Choice(_) | SettingKind::Flags(_), _) => {
            if let Some(picker) = ctx.state.tab_settings.as_mut() {
                picker.choosing = Some(key);
                picker.choice_highlight = value.as_str().map(str::to_string);
            }
            ctx.request_focus(TAB_SETTING_CHOICES_KEY);
            Update::full()
        }
        (SettingKind::Integer { .. } | SettingKind::Text, _) => {
            let text = match &value {
                SettingValue::Integer(value) => value.to_string(),
                SettingValue::String(value) => value.clone(),
                _ => String::new(),
            };
            if let Some(picker) = ctx.state.tab_settings.as_mut() {
                picker.prompt = Some(TabSettingPrompt::new(key, text));
            }
            ctx.request_focus(TAB_SETTING_PROMPT_KEY);
            Update::full()
        }
        // Lists are edited in config.toml; a tab never offers one.
        _ => Update::none(),
    }
}

pub(crate) fn reset(ctx: &mut Context<AppRoot>) -> Update {
    let Some(key) = ctx
        .state
        .tab_settings
        .as_ref()
        .and_then(|picker| picker.selected.clone())
    else {
        return Update::none();
    };
    if !open_group(&ctx.state).is_some_and(|group| group.overridden.contains(&key)) {
        return Update::none();
    }
    write(ctx, &key, None)
}

pub(crate) fn choose(ctx: &mut Context<AppRoot>, value: String) -> Update {
    let Some(picker) = ctx.state.tab_settings.as_mut() else {
        return Update::none();
    };
    picker.choice_highlight = None;
    let Some(key) = picker.choosing.take() else {
        return Update::none();
    };
    let flags = open_group(&ctx.state).and_then(|group| {
        match (&group.spec(&key)?.kind, group.value(&key)) {
            (SettingKind::Flags(_), Some(SettingValue::List(members))) => Some(members.clone()),
            _ => None,
        }
    });
    let Some(mut members) = flags else {
        return write(ctx, &key, Some(SettingValue::String(value)));
    };
    // A set is toggled one member at a time, and the list stays open on that member for the next.
    if let Some(at) = members.iter().position(|member| *member == value) {
        members.remove(at);
    } else {
        members.push(value.clone());
    }
    if let Some(picker) = ctx.state.tab_settings.as_mut() {
        picker.choosing = Some(key.clone());
        picker.choice_highlight = Some(value);
    }
    let update = write(ctx, &key, Some(SettingValue::List(members)));
    ctx.request_focus(TAB_SETTING_CHOICES_KEY);
    update
}

pub(crate) fn choice_select(ctx: &mut Context<AppRoot>, value: String) -> Update {
    if let Some(picker) = ctx.state.tab_settings.as_mut()
        && picker.choosing.is_some()
    {
        picker.choice_highlight = Some(value);
    }
    // The footer's `apply` carries the highlighted value.
    Update::full()
}

fn prompt_mut(ctx: &mut Context<AppRoot>) -> Option<&mut TabSettingPrompt> {
    ctx.state.tab_settings.as_mut()?.prompt.as_mut()
}

pub(crate) fn prompt_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(prompt) = prompt_mut(ctx) {
        event.apply_to(&mut prompt.input);
        prompt.error = None;
    }
    ctx.request_focus(TAB_SETTING_PROMPT_KEY);
    Update::full()
}

pub(crate) fn prompt_close(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(picker) = ctx.state.tab_settings.as_mut() {
        picker.prompt = None;
    }
    ctx.request_focus(TAB_SETTINGS_KEY);
    Update::full()
}

pub(crate) fn prompt_submit(ctx: &mut Context<AppRoot>) -> Update {
    let Some(prompt) = prompt_mut(ctx) else {
        return Update::none();
    };
    let key = prompt.key.clone();
    let text = prompt.input.text().to_string();
    let Some(spec) = open_group(&ctx.state).and_then(|group| group.spec(&key)) else {
        return prompt_close(ctx);
    };
    match typed_value(spec, &text) {
        Ok(value) => {
            prompt_close(ctx);
            write(ctx, &key, Some(value))
        }
        Err(error) => {
            if let Some(prompt) = prompt_mut(ctx) {
                prompt.error = Some(error);
            }
            ctx.request_focus(TAB_SETTING_PROMPT_KEY);
            Update::full()
        }
    }
}

/// Read a typed answer as a value of `spec`. Unlike a value in `config.toml`, an integer out of
/// range is refused rather than clamped: the user is still at the prompt and can fix it.
pub(crate) fn typed_value(
    spec: &SettingSpec,
    text: &str,
) -> std::result::Result<SettingValue, String> {
    let value = match spec.kind {
        SettingKind::Integer { .. } => {
            let range = spec
                .range_hint()
                .unwrap_or_else(|| "a whole number".to_string());
            let parsed = text
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("Enter {range}"))?;
            match spec.accept(SettingValue::Integer(parsed)) {
                Ok(crate::config::Checked::Value(value)) => value,
                _ => return Err(format!("Enter {range}")),
            }
        }
        _ => SettingValue::String(text.to_string()),
    };
    Ok(value)
}

/// Write one setting (or remove it with `None`), then reload so it takes effect everywhere. The
/// picker stays open on the same row.
fn write(ctx: &mut Context<AppRoot>, key: &str, value: Option<SettingValue>) -> Update {
    let Some(owner) = open_group(&ctx.state).map(|group| group.owner.clone()) else {
        return Update::none();
    };
    let update = match crate::config::persist_setting(&owner, key, value.as_ref()) {
        Ok(_) => crate::ops::config::reload_config_quiet(ctx),
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Setting not saved", error);
            Update::full()
        }
    };
    ctx.request_focus(TAB_SETTINGS_KEY);
    update
}
