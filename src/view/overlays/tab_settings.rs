use super::*;

use crate::config::{SettingKind, SettingSpec, SettingsOwner};
use crate::ops::tab_settings::{TAB_SETTING_CHOICES_KEY, TAB_SETTING_PROMPT_KEY, TAB_SETTINGS_KEY};

fn tab_label(ctx: &Context<AppRoot>, id: &str) -> String {
    ctx.state
        .config
        .sidebar
        .tabs
        .iter()
        .find(|tab| tab.id().as_str() == id)
        .map_or_else(|| id.to_string(), |tab| tab.label().to_string())
}

/// What Enter does to a setting, as the footer names it.
fn activate_verb(spec: &SettingSpec) -> &'static str {
    match spec.kind {
        SettingKind::Toggle => "toggle",
        SettingKind::Choice(_) | SettingKind::Flags(_) => "choose",
        _ => "edit",
    }
}

/// How many pickers the settings list stacks on: Sidebar tabs when it was opened from there, none
/// when it came from the command palette. A choice list or prompt sits one layer higher.
fn base_depth(ctx: &Context<AppRoot>) -> u16 {
    u16::from(ctx.state.sidebar_manager.is_some())
}

/// The open choice list, stacked on the settings list, if a choice setting is being edited.
pub(crate) fn tab_setting_choice_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.tab_settings.as_ref() else {
        return Text::new("").into();
    };
    let group = crate::ops::tab_settings::open_group(&ctx.state);
    let (Some(key), Some(group)) = (&picker.choosing, group) else {
        return Text::new("").into();
    };
    let Some(spec) = group.spec(key) else {
        return Text::new("").into();
    };
    choice_overlay(
        ctx,
        &tab_label(ctx, &picker.tab),
        spec,
        group.value(key),
        picker.choice_highlight.as_deref(),
        base_depth(ctx) + 1,
    )
}

pub(crate) fn tab_settings_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.tab_settings.as_ref() else {
        return Text::new("").into();
    };
    let label = tab_label(ctx, &picker.tab);
    let group = crate::ops::tab_settings::open_group(&ctx.state);
    let specs = group
        .map(|group| group.specs.as_slice())
        .unwrap_or_default();
    let entries = specs
        .iter()
        .map(|spec| {
            let value = group
                .and_then(|group| group.value(&spec.key))
                .unwrap_or(&spec.default);
            let mut item = SearchItem::new(spec.label.clone(), spec.key.clone())
                .alias(spec.key.clone())
                .description(picker_description(spec.display(value)));
            if let Some(description) = &spec.description {
                item = item.alias(description.clone());
            }
            SearchEntry::Item(item)
        })
        .collect::<Vec<_>>();
    let selected_spec = picker
        .selected
        .as_deref()
        .and_then(|key| specs.iter().find(|spec| spec.key == key));
    let selected_index = picker
        .selected
        .as_deref()
        .and_then(|key| specs.iter().position(|spec| spec.key == key));
    let overridden = picker
        .selected
        .as_deref()
        .is_some_and(|key| group.is_some_and(|group| group.overridden.contains(key)));
    let mut actions = Vec::new();
    if let Some(spec) = selected_spec {
        actions.push(
            OverlayAction::new(
                "enter",
                activate_verb(spec),
                Msg::TabSettingsActivate(spec.key.clone()),
                true,
            )
            .hint_only(),
        );
    }
    actions.push(OverlayAction::new(
        "ctrl-r",
        "reset",
        Msg::TabSettingsReset,
        overridden,
    ));
    let detail = selected_spec
        .and_then(|spec| spec.description.as_deref())
        .map(|description| {
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(description)
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(fg_only(&ctx.state.theme.muted)),
                )
                .into()
        });
    // An extension tab's settings are the extension's own; saying whose keeps a change from looking
    // as if it touched only this tab.
    let header = group.and_then(|group| match &group.owner {
        SettingsOwner::Extension(id) => Some(format!("from {id}")),
        SettingsOwner::Worktrees => Some("in [worktrees]".to_string()),
        SettingsOwner::SidebarTab(_) => None,
    });
    let mut palette = OverlayPalette::new(
        format!("{label} settings"),
        TAB_SETTINGS_KEY,
        Msg::TabSettingsBack,
        56,
    )
    .entries(entries)
    .actions(actions)
    .placeholder("Search settings…")
    .empty_text("No settings")
    .preserve_groups(false)
    .selected(selected_index)
    .initial_query(picker.query.clone())
    .detail(detail)
    .nested(base_depth(ctx))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::TabSettingsQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::TabSettingsSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::TabSettingsActivate(event.item.value)),
    );
    if let Some(header) = header {
        palette = palette.header_right(header);
    }
    palette.render(ctx)
}

/// A choice setting's values, stacked on the settings until one is chosen or Esc goes
/// back. The current value is highlighted and marked.
fn choice_overlay(
    ctx: &Context<AppRoot>,
    label: &str,
    spec: &SettingSpec,
    value: Option<&crate::config::SettingValue>,
    highlight: Option<&str>,
    depth: u16,
) -> Element {
    let (SettingKind::Choice(choices) | SettingKind::Flags(choices)) = &spec.kind else {
        return Text::new("").into();
    };
    let flags = matches!(spec.kind, SettingKind::Flags(_));
    let current = value
        .and_then(crate::config::SettingValue::as_str)
        .or_else(|| spec.default.as_str());
    let default = spec.default.as_str();
    let members = match value.unwrap_or(&spec.default) {
        crate::config::SettingValue::List(members) => members.as_slice(),
        _ => &[],
    };
    let entries = choices
        .iter()
        .map(|choice| {
            let mut item = SearchItem::new(choice.label.clone(), choice.value.clone())
                .alias(choice.value.clone());
            let status = if flags {
                if members.contains(&choice.value) {
                    "on"
                } else {
                    "off"
                }
            } else {
                match (
                    Some(choice.value.as_str()) == current,
                    Some(choice.value.as_str()) == default,
                ) {
                    (true, true) => "current  default",
                    (true, false) => "current",
                    (false, true) => "default",
                    (false, false) => "",
                }
            };
            if !status.is_empty() {
                item = item.description(picker_description(status));
            }
            SearchEntry::Item(item)
        })
        .collect::<Vec<_>>();
    // The footer's `apply` is clickable, so it applies the live highlight, not the value the list
    // opened on.
    let highlight = highlight.or(current).or_else(|| {
        flags
            .then(|| choices.first().map(|choice| choice.value.as_str()))
            .flatten()
    });
    let selected = choices
        .iter()
        .position(|choice| Some(choice.value.as_str()) == highlight);
    let mut actions = Vec::new();
    if let Some(highlight) = highlight {
        actions.push(
            OverlayAction::new(
                "enter",
                if flags { "toggle" } else { "apply" },
                Msg::TabSettingsChoose(highlight.to_string()),
                true,
            )
            .hint_only(),
        );
    }
    OverlayPalette::new(
        format!("{label} · {}", spec.label),
        TAB_SETTING_CHOICES_KEY,
        Msg::TabSettingsBack,
        40,
    )
    .entries(entries)
    .actions(actions)
    .placeholder("Search values…")
    .nested(depth)
    .preserve_groups(false)
    .selected(selected)
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::TabSettingsChoiceSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::TabSettingsChoose(event.item.value)),
    )
    .render(ctx)
}

/// The typed value of an integer or text setting, stacked on the settings list.
pub(crate) fn tab_setting_prompt_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.tab_settings.as_ref() else {
        return Text::new("").into();
    };
    let Some(prompt) = picker.prompt.as_ref() else {
        return Text::new("").into();
    };
    let spec =
        crate::ops::tab_settings::open_group(&ctx.state).and_then(|group| group.spec(&prompt.key));
    let title = spec.map_or_else(|| prompt.key.clone(), |spec| spec.label.clone());
    let placeholder = spec
        .and_then(SettingSpec::range_hint)
        .unwrap_or_else(|| "Value".to_string());
    prompt_overlay(
        ctx,
        PromptChrome::new(&title, &placeholder, &[("apply", "enter")])
            .caption(prompt.error.as_deref().map(PromptCaption::Note))
            .over_picker(base_depth(ctx) + 1),
        &prompt.input,
        TAB_SETTING_PROMPT_KEY,
        Msg::TabSettingPromptChanged,
        Msg::TabSettingPromptClose,
        Msg::TabSettingPromptSubmit,
    )
}
