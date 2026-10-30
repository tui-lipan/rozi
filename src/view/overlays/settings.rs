use super::*;

use std::collections::HashSet;

use crate::state::SettingControl;

const SETTINGS_MAX_HEIGHT_PERCENT: u16 = 70;
const SETTINGS_MODAL_WIDTH: u16 = 64;
const SETTINGS_CHOICE_WIDTH: u16 = 40;

type SettingEntry = SearchEntry<(SettingsAction, String)>;
type SettingGroup = (crate::state::SettingsGroup, Vec<SettingEntry>);

fn settings_groups(ctx: &Context<AppRoot>) -> Vec<SettingGroup> {
    let config = &ctx.state.config;
    let mut groups: Vec<SettingGroup> = Vec::new();
    for spec in crate::state::SETTINGS {
        if spec.action == SettingsAction::SidebarLayoutPreset && config.sidebar.presets.is_empty() {
            continue;
        }
        let status = match &spec.control {
            SettingControl::Toggle { field, .. } => enabled_status((field.get)(config)),
            SettingControl::ActionToggle { value, .. } => enabled_status(value(config)),
            SettingControl::Choice(choice) | SettingControl::Cycle(choice) => choice.status(config),
            SettingControl::Custom => custom_status(ctx, spec.action),
        };
        let mut aliases: Vec<Arc<str>> = spec.aliases.iter().copied().map(Arc::from).collect();
        aliases.push(Arc::from(spec.group.label()));
        aliases.push(Arc::from(spec.group.tab().label()));
        let entry =
            SearchEntry::Item(SearchItem::new(spec.label, (spec.action, status)).aliases(aliases));
        match groups.last_mut() {
            Some((group, entries)) if *group == spec.group => entries.push(entry),
            _ => groups.push((spec.group, vec![entry])),
        }
    }
    groups
}

/// The value text of a row with an editor of its own.
fn custom_status(ctx: &Context<AppRoot>, action: SettingsAction) -> String {
    let config = &ctx.state.config;
    match action {
        SettingsAction::Theme => current_theme_label(ctx),
        SettingsAction::EditScrollMultiplier => {
            crate::state::scroll_multiplier_label(config.input.scroll_multiplier)
        }
        SettingsAction::EditPadding => padding_summary(config.pane.padding),
        SettingsAction::EditUnfocusedDim => {
            crate::state::unfocused_dim_label(config.pane.unfocused_dim)
        }
        SettingsAction::ChooseTitlebar | SettingsAction::ChooseWorkbar => {
            choice_status(ctx, action)
        }
        SettingsAction::LeftSidebarPanels => sidebar_dock_status(&config.sidebar.layout.left),
        SettingsAction::RightSidebarPanels => sidebar_dock_status(&config.sidebar.layout.right),
        SettingsAction::ToggleSidebarStartup => enabled_status(config.sidebar.startup),
        _ => String::new(),
    }
}

fn settings_query(ctx: &Context<AppRoot>) -> &str {
    ctx.state.settings_navigation.query.text()
}

fn settings_entries(ctx: &Context<AppRoot>) -> Vec<SettingEntry> {
    let searching = !settings_query(ctx).is_empty();
    let tab = if searching {
        crate::state::SettingsTab::All
    } else {
        ctx.state.settings_navigation.tab
    };
    settings_entries_for_tab(ctx, tab, searching)
}

fn settings_item_count(entries: &[SettingEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| matches!(entry, SearchEntry::Item(_)))
        .count()
}

fn visible_settings_entries(entries: &[SettingEntry], query: &str) -> Vec<SettingEntry> {
    if query.is_empty() {
        return entries.to_vec();
    }
    let items: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            SearchEntry::Item(item) => Some(item.clone()),
            _ => None,
        })
        .collect();
    let matched: HashSet<usize> = rank_search_palette_indices_with_mode(
        &items,
        query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    )
    .into_iter()
    .collect();
    let mut visible = Vec::new();
    let mut chrome = Vec::new();
    let mut item_index = 0;
    for entry in entries {
        match entry {
            SearchEntry::Spacer => {
                chrome.clear();
                chrome.push(entry.clone());
            }
            SearchEntry::Header(_) => chrome.push(entry.clone()),
            SearchEntry::Item(_) => {
                if matched.contains(&item_index) {
                    if visible.is_empty() {
                        chrome.retain(|entry| !matches!(entry, SearchEntry::Spacer));
                    }
                    visible.append(&mut chrome);
                    visible.push(entry.clone());
                }
                item_index += 1;
            }
        }
    }
    visible
}

fn settings_entries_for_tab(
    ctx: &Context<AppRoot>,
    tab: crate::state::SettingsTab,
    searching: bool,
) -> Vec<SettingEntry> {
    search_entries_with_groups(
        settings_groups(ctx)
            .into_iter()
            .filter_map(|(group, entries)| {
                let category = group.tab();
                if tab != crate::state::SettingsTab::All && tab != category {
                    return None;
                }
                let heading = if searching && category.label() != group.label() {
                    format!("{} › {}", category.label(), group.label())
                } else {
                    group.label().to_string()
                };
                Some((heading, entries))
            }),
    )
    .into_iter()
    .filter(|entry| match entry {
        SearchEntry::Header(title) if !searching => title.as_ref() != tab.label(),
        _ => true,
    })
    .collect()
}

pub(crate) fn settings_query_selection(ctx: &Context<AppRoot>) -> Option<SettingsAction> {
    let actions: Vec<_> = visible_settings_entries(&settings_entries(ctx), settings_query(ctx))
        .into_iter()
        .filter_map(|entry| match entry {
            SearchEntry::Item(item) => Some(item.value.0),
            _ => None,
        })
        .collect();
    actions
        .iter()
        .copied()
        .find(|action| Some(*action) == ctx.state.settings_selected)
        .or_else(|| actions.first().copied())
}

fn settings_section_item(title: &str, theme: &Theme) -> ListItem {
    ListItem::header(title).style(rozi_fg(theme).bold())
}

fn settings_list_rows(ctx: &Context<AppRoot>, rows: usize) -> u16 {
    const ABOVE_LIST: u16 = 7;
    let cap = (ctx.viewport().h * SETTINGS_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(ABOVE_LIST)
        .max(3);
    u16::try_from(rows).unwrap_or(u16::MAX).clamp(1, cap)
}

fn settings_key_handler(
    ctx: &Context<AppRoot>,
    targets: Arc<[Option<SettingsAction>]>,
    selected: Option<usize>,
    page: isize,
    actions: &[OverlayAction],
) -> KeyHandler {
    let actions = actions
        .iter()
        .filter(|action| action.enabled && action.intercept)
        .map(|action| (action.key.clone(), action.msg.clone()))
        .collect::<Vec<_>>();
    let handler = ctx.link().key_handler(move |key| {
        let direction = key.left_alt_arrow();
        let plain = direction.is_some() || (!key.mods.ctrl && !key.mods.alt && !key.mods.super_key);
        let navigation = match direction.unwrap_or(key.code) {
            code if plain && !key.mods.shift => {
                let step = match code {
                    KeyCode::Up => Some((-1, true)),
                    KeyCode::Down => Some((1, true)),
                    KeyCode::PageUp => Some((-page, false)),
                    KeyCode::PageDown => Some((page, false)),
                    KeyCode::Home => Some((isize::MIN, false)),
                    KeyCode::End => Some((isize::MAX, false)),
                    _ => None,
                };
                step.and_then(|(delta, wrap)| {
                    let index =
                        super::palette::stepped_selectable_row(&targets, selected, delta, wrap)?;
                    targets[index].map(Msg::SettingsSelect)
                })
            }
            _ => None,
        };
        navigation.or_else(|| {
            actions
                .iter()
                .find(|(binding, _)| {
                    binding.matches_sequence(&[KeyEvent {
                        code: direction.unwrap_or(key.code),
                        mods: if direction.is_some() {
                            KeyMods::NONE
                        } else {
                            key.mods
                        },
                    }])
                })
                .map(|(_, msg)| msg.clone())
        })
    });
    KeyHandler::new(move |key| {
        // Give picker actions first refusal, then keep empty-result navigation out of text input.
        handler.handle(key) || matches!(key.left_alt_arrow(), Some(KeyCode::Up | KeyCode::Down))
    })
}

fn settings_search(
    ctx: &Context<AppRoot>,
    matches: usize,
    total: usize,
    keys: KeyHandler,
) -> Element {
    let theme = &ctx.state.theme;
    Input::bound(&ctx.state.settings_navigation.query)
        .placeholder("Search settings…")
        .suffix(format!("{matches}/{total}"))
        .style(fg_only(&theme.muted))
        .focus_style(Style::new().fg(theme.border_active))
        .placeholder_style(fg_only(&theme.muted))
        .suffix_style(fg_only(&theme.primary))
        .focus_suffix_style(fg_only(&theme.primary))
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .border(false)
        .padding((0, 1))
        .on_change(ctx.link().callback(Msg::SettingsQueryChanged))
        .key_interceptor(keys)
        .key(settings_palette_key())
}

pub(crate) fn settings_overlay(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let query = settings_query(ctx);
    let entries = settings_entries(ctx);
    let total = settings_item_count(&entries);
    let visible = visible_settings_entries(&entries, query);
    let matches = settings_item_count(&visible);
    let config = ctx.state.config.clone();
    let item_style = fg_only(&theme.primary);
    let description_style = fg_only(&theme.muted);
    let disabled_style = fg_only(&theme.border);
    let mut items = Vec::new();
    let mut targets = Vec::new();
    for entry in &visible {
        match entry {
            SearchEntry::Spacer => {
                items.push(ListItem::spacer());
                targets.push(None);
            }
            SearchEntry::Header(title) => {
                items.push(settings_section_item(title, theme));
                targets.push(None);
            }
            SearchEntry::Item(item) => {
                let disabled_reason = item.value.0.disabled_reason_for_state(&ctx.state);
                let marked =
                    disabled_reason.is_none() && item.value.0.shows_choice_ellipsis(&config);
                let label = if marked {
                    format!("{}…", item.label)
                } else {
                    item.label.to_string()
                };
                let status = disabled_reason.unwrap_or(&item.value.1);
                let style = if disabled_reason.is_some() {
                    disabled_style
                } else {
                    item_style
                };
                let row = picker_row(
                    [Span::new(label).style(style)],
                    status,
                    if disabled_reason.is_some() {
                        disabled_style
                    } else {
                        description_style
                    },
                );
                items.push(if disabled_reason.is_some() {
                    row.pointer_shape(PointerShape::Default)
                } else {
                    row
                });
                targets.push(Some(item.value.0));
            }
        }
    }
    let selected_index = ctx
        .state
        .settings_selected
        .and_then(|selected| {
            targets
                .iter()
                .position(|target| target.is_some_and(|action| action == selected))
        })
        .or_else(|| targets.iter().position(Option::is_some));
    let selected = selected_index.and_then(|index| targets[index]);
    let actions = settings_actions(ctx, selected);
    let targets: Arc<[Option<SettingsAction>]> = targets.into();
    let select_targets = targets.clone();
    let activate_targets = targets.clone();
    let list_rows = settings_list_rows(ctx, targets.len());
    let page = isize::from(i16::try_from(list_rows.saturating_sub(1).max(1)).unwrap_or(i16::MAX));
    let keys = settings_key_handler(ctx, targets, selected_index, page, &actions);
    let search = settings_search(ctx, matches, total, keys);
    let (selection_left, selection_right) =
        crate::view::picker_selection_cap_glyphs(&ctx.state.config);
    let list = List::new()
        .items(items)
        .selected(selected_index)
        .border(false)
        .selection_symbol(Some(selection_left))
        .selection_symbol_right(Some(selection_right))
        .selection_symbol_style(crate::view::picker_selection_cap_style(
            theme,
            theme.border_active,
        ))
        .unselected_symbol(Some(""))
        .selection_full_width(true)
        .selection_style(picker_selection_style(theme, None))
        .unfocused_selection_style(picker_selection_style(theme, None))
        .item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
        .item_horizontal_padding((0, 1))
        .header_horizontal_padding((0, 1))
        .scroll_wheel(true)
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(theme))
        .empty_text("No matches")
        .empty_text_style(fg_only(&theme.muted))
        .height(Length::Px(list_rows))
        .focusable(false)
        .on_select(ctx.link().callback_opt(move |event: ListEvent| {
            select_targets.get(event.index)?.map(Msg::SettingsSelect)
        }))
        .on_activate(ctx.link().callback_opt(move |event: ListEvent| {
            activate_targets
                .get(event.index)?
                .map(Msg::SettingsActivate)
        }));
    let tabs = super::palette::picker_tabs(
        ctx,
        &crate::state::SettingsTab::ALL.map(|tab| tab.label()),
        if query.is_empty() {
            ctx.state.settings_navigation.tab.index()
        } else {
            crate::state::SettingsTab::All.index()
        },
        ctx.link().callback(|event: TabsEvent| {
            Msg::SettingsTabSelected(crate::state::SettingsTab::ALL[event.index])
        }),
    );
    let body = VStack::new()
        .height(Length::Auto)
        .child(search)
        .child(super::palette::picker_divider(theme))
        .child(tabs)
        .child(Spacer::new().height(Length::Px(1)))
        .child(list);
    let panel = super::palette::tabbed_picker_panel(ctx, "Settings", Length::Auto, body.into());
    let nested = ctx.state.settings_number_editor.is_some() || ctx.state.settings_choice.is_some();
    let panel: Element = Animated::new(panel)
        .opacity(crate::scratchpad::backdrop_dim(if nested {
            1.0
        } else {
            0.0
        }))
        .opacity_target(ctx.state.theme.surface.backdrop)
        .transition(crate::view::animation::scratch_transition_config(ctx))
        .into();

    Modal::new()
        .animation(crate::view::animation::picker_animation(
            ctx.state.config.animations,
        ))
        .width(Length::Px(SETTINGS_MODAL_WIDTH))
        .height(Length::Auto)
        .max_height(Length::Percent(SETTINGS_MAX_HEIGHT_PERCENT))
        .reserve_height(Length::Percent(SETTINGS_MAX_HEIGHT_PERCENT))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::CloseSettings))
        .child(panel)
        .key("rozi-settings-modal")
}

fn settings_actions(
    ctx: &Context<AppRoot>,
    selected: Option<SettingsAction>,
) -> Vec<OverlayAction> {
    let can_change =
        selected.is_some_and(|action| action.disabled_reason_for_state(&ctx.state).is_none());
    let tab = if settings_query(ctx).is_empty() {
        ctx.state.settings_navigation.tab
    } else {
        crate::state::SettingsTab::All
    };
    vec![
        OverlayAction::new("esc", "clear / close", Msg::SettingsEscape, true).hide_hint(),
        OverlayAction::new(
            "tab",
            "category",
            Msg::SettingsTabSelected(tab.stepped(false)),
            true,
        )
        .hide_hint(),
        OverlayAction::new(
            "shift-tab",
            "category",
            Msg::SettingsTabSelected(tab.stepped(true)),
            true,
        )
        .hide_hint(),
        OverlayAction::new(
            "left",
            "category",
            Msg::SettingsTabSelected(tab.stepped(true)),
            true,
        )
        .hide_hint(),
        OverlayAction::new(
            "right",
            "category",
            Msg::SettingsTabSelected(tab.stepped(false)),
            true,
        )
        .hide_hint(),
        OverlayAction::new(
            "shift-enter",
            "cycle",
            Msg::SettingsCycleChoice(selected.unwrap_or(SettingsAction::Theme)),
            selected.is_some_and(|action| {
                action.disabled_reason_for_state(&ctx.state).is_none()
                    && action.choice_ring(&ctx.state.config).is_some()
            }),
        )
        .hide_hint(),
        OverlayAction::new(
            "enter",
            "choose",
            Msg::SettingsActivate(selected.unwrap_or(SettingsAction::Theme)),
            can_change,
        )
        .hide_hint(),
    ]
}

fn padding_summary((top, right, bottom, left): (u16, u16, u16, u16)) -> String {
    if top == bottom && right == left {
        format!("V{top} · H{right}")
    } else {
        format!("T{top} R{right} B{bottom} L{left}")
    }
}

pub(crate) fn settings_number_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(editor) = ctx.state.settings_number_editor.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let several = editor.fields.len() > 1;
    // A labeled, fixed-width numeric field: "Label [ 0 ]". Kept narrow so a pair sits on one row.
    let field = |index: usize, field: &crate::state::SettingsNumberField| {
        let focused = editor.focus == index;
        let input = Input::bound(&field.input)
            .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
            .focus_style(
                Style::new()
                    .fg(theme.border_active)
                    .bg(theme.surface.element),
            )
            .selection_style(theme.text_selection)
            .width(Length::Px(6))
            .border(false)
            .padding((0, 1))
            .on_change(
                ctx.link()
                    .callback(move |event| Msg::SettingsNumberChanged(index, event)),
            )
            .on_focus(
                ctx.link()
                    .callback(move |_| Msg::SettingsNumberFocus(index)),
            )
            .on_key(ctx.link().key_handler(move |event| {
                if event.is(KeyCode::Esc) {
                    Some(Msg::CloseSettingsNumberEditor)
                } else if event.code == KeyCode::Enter
                    && !event.mods.ctrl
                    && !event.mods.alt
                    && !event.mods.super_key
                {
                    Some(Msg::SettingsNumberEnter(index))
                } else {
                    None
                }
            }))
            .key(settings_number_field_key(index));
        HStack::new()
            .width(Length::Auto)
            .height(Length::Auto)
            .gap(1)
            .child(
                // The same marker the host editor wears, for the same reason: a borderless field
                // otherwise says nothing about where Enter is aimed.
                Text::new(if focused { "›" } else { " " })
                    .width(Length::Px(1))
                    .style(fg_only(&theme.accent)),
            )
            .child(Text::new(field.label.to_string()).style(if focused {
                fg_only(&theme.primary).bold()
            } else {
                fg_only(&theme.muted)
            }))
            .child(input)
    };
    let fields = editor.fields.iter().enumerate().fold(
        HStack::new()
            .height(Length::Auto)
            .padding((0, 1))
            .justify(Justify::SpaceBetween),
        |row, (index, slot)| row.child(field(index, slot)),
    );
    // gap(0): the fields sit under the modal's own top padding, and `hint_row` carries its own
    // leading blank line, so an extra VStack gap would double the spacing.
    let mut body = VStack::new()
        .height(Length::Auto)
        .padding((1, 0, 0, 0))
        .gap(0)
        .child(fields);
    if let Some((label, value)) = editor.note {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((0, 1))
                .justify(Justify::SpaceBetween)
                .child(Text::new(label).style(fg_only(&theme.muted)))
                .child(Text::new(value).style(fg_only(&theme.primary))),
        );
    }
    let body = body.child(
        hint_row()
            // Enter advances from an earlier field and applies from the last, so the click follows
            // whichever field has focus, as the key does.
            .child(hint_button(
                ctx,
                if several { "next / apply" } else { "apply" },
                "enter",
                Msg::SettingsNumberEnter(editor.focus),
            ))
            // Both keys land back in Settings whenever it is the dialog behind this one.
            .child(hint_button(
                ctx,
                if ctx.state.show_settings {
                    "back"
                } else {
                    "cancel"
                },
                "esc",
                Msg::CloseSettingsNumberEditor,
            )),
    );
    nested_action_palette_modal(ctx, editor.title, SETTINGS_MAX_HEIGHT_PERCENT)
        .width(Length::Auto)
        .on_close(ctx.link().callback(|_| Msg::CloseSettingsNumberEditor))
        .child(body)
        .key("rozi-settings-number-modal")
}

pub(crate) fn settings_choice_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(editor) = ctx.state.settings_choice.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let entries: Vec<SearchEntry<usize>> = editor
        .options
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let mut entry = SearchEntry::item(label.clone(), index);
            if index == editor.original_index
                && editor.action != SettingsAction::SidebarLayoutPreset
            {
                entry = entry.description(picker_description("current"));
            }
            entry
        })
        .collect();
    let actions =
        vec![OverlayAction::new("esc", "cancel", Msg::SettingsChoiceCancel, true).hide_hint()];
    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search…")
        .initial_selected_item_index(Some(editor.index))
        .sync_selection(true)
        .input_key_interceptor(overlay_interceptor(ctx, &actions))
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SettingsChoiceSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SettingsChoicePick(event.item.value)),
        );
    nested_action_palette_modal(ctx, editor.title, SETTINGS_MAX_HEIGHT_PERCENT)
        .width(Length::Px(SETTINGS_CHOICE_WIDTH))
        .backdrop_style(receding_backdrop(theme))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::SettingsChoiceCancel))
        .child(palette)
        .key(settings_choice_key())
}

fn choice_status(ctx: &Context<AppRoot>, action: SettingsAction) -> String {
    action
        .choice_ring(&ctx.state.config)
        .and_then(|ring| ring.options.get(ring.index).cloned())
        .unwrap_or_default()
}

fn enabled_status(enabled: bool) -> String {
    if enabled { "Enabled" } else { "Disabled" }.to_string()
}

fn current_theme_label(ctx: &Context<AppRoot>) -> String {
    let current = &ctx.state.config.theme.name;
    crate::config::theme_choices()
        .into_iter()
        .find(|choice| &choice.id() == current)
        .map(|choice| choice.label())
        .unwrap_or_else(|| current.clone())
}

pub(super) fn action_search_palette(
    ctx: &Context<AppRoot>,
    entries: Vec<SearchEntry<Callback<()>>>,
    placeholder: &str,
) -> SearchPalette<Callback<()>> {
    shared_search_palette::<Callback<()>>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder(placeholder)
        // Score-order matches once the user types; category headers only show for an empty query.
        .preserve_groups(false)
        // Run the command's own handler directly rather than looking it up by id through
        // `CommandRegistry::execute`, since that call also enforces the `commands_active`
        // gate - which is false while this very palette is open (see `commands::sync`).
        .on_activate(Callback::new(|event: SearchEvent<Callback<()>>| {
            event.item.value.emit(());
        }))
}

pub(crate) fn theme_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    // Built-in presets plus every custom theme file, selected by index into the same list.
    let choices = crate::config::theme_choices();
    let current = &ctx.state.config.theme.name;
    let current_index = choices.iter().position(|choice| &choice.id() == current);
    // The highlight is user-owned once the picker opens: drive it from the remembered selection so
    // filtering preserves it (or falls to the first match) rather than snapping back to the active
    // theme. Fall back to the active theme only before the first selection is recorded.
    let initial_selected = ctx.state.theme_picker_selected.or(current_index);

    let mut entries = Vec::with_capacity(choices.len() + 4);
    let mut previous_group = None;
    for (index, choice) in choices.iter().enumerate() {
        let group = match choice {
            crate::config::ThemeChoice::System => None,
            crate::config::ThemeChoice::Builtin(preset) if preset.is_light() => Some("Light"),
            crate::config::ThemeChoice::Builtin(_) => Some("Dark"),
            crate::config::ThemeChoice::Custom { .. } => Some("Custom"),
        };
        if previous_group != group {
            if let Some(group) = group {
                if !entries.is_empty() {
                    entries.push(SearchEntry::spacer());
                }
                entries.push(SearchEntry::header(group));
            }
            previous_group = group;
        }

        let mut entry = SearchEntry::item(choice.label(), index);
        let signature = matches!(
            choice,
            crate::config::ThemeChoice::Builtin(
                crate::state::ThemePreset::Rozi | crate::state::ThemePreset::Lipan
            )
        );
        let description = match (Some(index) == current_index, signature) {
            (true, true) => Some("current · signature"),
            (true, false) => Some("current"),
            (false, true) => Some("signature"),
            (false, false) => None,
        };
        if let Some(description) = description {
            entry = entry.description(picker_description(description));
        }
        entries.push(entry);
    }

    // Mirror the command palette so theme selection reuses the same fuzzy-search UX.
    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search themes…")
        .preserve_groups(false)
        .initial_selected_item_index(initial_selected)
        .sync_selection(true)
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::PreviewTheme(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SelectTheme(event.item.value)),
        );

    action_palette(
        ctx,
        "Change theme",
        theme_picker_key(),
        Msg::CloseThemePicker,
        palette,
        60,
    )
}

fn sidebar_dock_status(dock: &crate::config::SidebarDock) -> String {
    if dock.panel_count == 1 {
        "1 panel".into()
    } else {
        format!("{} panels", dock.panel_count)
    }
}
