use super::*;

use std::collections::HashSet;

const SETTINGS_MAX_HEIGHT_PERCENT: u16 = 70;
const SETTINGS_MODAL_WIDTH: u16 = 64;
const SETTINGS_CHOICE_WIDTH: u16 = 40;

type SettingEntry = SearchEntry<(SettingsAction, String)>;
type SettingGroup = (&'static str, Vec<SettingEntry>);

fn settings_groups(ctx: &Context<AppRoot>) -> Vec<SettingGroup> {
    use SettingsAction::*;

    let pane = &ctx.state.config.pane;
    vec![
        settings_group(
            "General",
            vec![
                ("Theme", current_theme_label(ctx), Theme),
                (
                    "Animations",
                    enabled_status(ctx.state.config.animations.enabled),
                    ToggleAnimations,
                ),
                (
                    "Workspace switching animation",
                    enabled_status(ctx.state.config.animations.workspace),
                    ToggleWorkspaceAnimation,
                ),
                (
                    "Nerd icons",
                    enabled_status(ctx.state.config.nerd_icons),
                    ToggleNerdIcons,
                ),
                (
                    "Which-key",
                    ctx.state.config.input.which_key.label().to_string(),
                    CycleWhichKey,
                ),
                (
                    "Focus on hover",
                    enabled_status(pane.focus_on_hover),
                    ToggleFocusOnHover,
                ),
            ],
        ),
        settings_group(
            "Pickers",
            vec![
                (
                    "Border",
                    pane.picker_border_style.label().to_string(),
                    CyclePickerBorderStyle,
                ),
                (
                    "Tab strip",
                    enabled_status(pane.picker_tab_background),
                    TogglePickerTabBackground,
                ),
                (
                    "Tab style",
                    cap_style_label(pane.picker_tab_style).to_string(),
                    CyclePickerTabStyle,
                ),
                (
                    "Selection style",
                    cap_style_label(pane.picker_selection_style).to_string(),
                    CyclePickerSelectionStyle,
                ),
            ],
        ),
        settings_group(
            "Titlebar",
            vec![
                (
                    "Show titlebar",
                    enabled_status(pane.show_titles),
                    ToggleTitles,
                ),
                ("Layout", pane.titlebar.label().to_string(), CycleTitlebar),
                (
                    "Style",
                    cap_style_label(pane.title_style).to_string(),
                    CycleTitleStyle,
                ),
            ],
        ),
        settings_group(
            "Workbar",
            vec![
                (
                    "Show workbar",
                    enabled_status(pane.show_workbar),
                    ToggleWorkbar,
                ),
                (
                    "Position",
                    if pane.workbar_at_bottom {
                        "Bottom"
                    } else {
                        "Top"
                    }
                    .to_string(),
                    ToggleWorkbarPosition,
                ),
                ("Gap", enabled_status(pane.workbar_gap), ToggleWorkbarGap),
                (
                    "Background",
                    enabled_status(pane.workbar_background),
                    ToggleWorkbarBackground,
                ),
                (
                    "Style",
                    cap_style_label(pane.workbar_style).to_string(),
                    CycleWorkbarStyle,
                ),
                (
                    "Badge style",
                    cap_style_label(pane.workbar_badge_style).to_string(),
                    CycleWorkbarBadgeStyle,
                ),
                (
                    "Tab style",
                    cap_style_label(pane.workbar_tab_style).to_string(),
                    CycleWorkbarTabStyle,
                ),
                (
                    "Powerline",
                    enabled_status(pane.workbar_powerline),
                    ToggleWorkbarPowerline,
                ),
            ],
        ),
        settings_group(
            "Panes",
            vec![
                (
                    "Background follows terminal",
                    enabled_status(pane.background_follows_terminal),
                    ToggleBackgroundFollowsTerminal,
                ),
                (
                    "Terminal padding",
                    padding_summary(pane.padding),
                    EditPadding,
                ),
                (
                    "Focused background",
                    enabled_status(pane.highlight_focused_background),
                    ToggleHighlightFocusedBackground,
                ),
                (
                    "Focused border",
                    enabled_status(pane.highlight_focused_border),
                    ToggleHighlightFocusedBorder,
                ),
                (
                    "Focused titlebar",
                    enabled_status(pane.highlight_focused_titlebar),
                    ToggleHighlightFocusedTitlebar,
                ),
                (
                    "Border mode",
                    pane.border_mode.label().to_string(),
                    CycleBorderMode,
                ),
                (
                    "Border style",
                    pane.border_style.label().to_string(),
                    CycleBorderStyle,
                ),
                (
                    "Floating border",
                    pane.float_border_style.label().to_string(),
                    CycleFloatBorderStyle,
                ),
                (
                    "Scratchpad border",
                    pane.scratch_border_style.label().to_string(),
                    CycleScratchBorderStyle,
                ),
                (
                    "Fullscreen border",
                    pane.fullscreen_border_style.label().to_string(),
                    CycleFullscreenBorderStyle,
                ),
                (
                    "Open/close animation",
                    ctx.state.config.animations.pane_style.label().to_string(),
                    CyclePaneAnimation,
                ),
            ],
        ),
        settings_group(
            "Sidebar",
            vec![
                (
                    "Position",
                    ctx.state.config.sidebar.position.label().to_string(),
                    ToggleSidebarPosition,
                ),
                (
                    "Background follows canvas",
                    enabled_status(ctx.state.config.sidebar.background_follows_canvas),
                    ToggleSidebarBackgroundFollowsCanvas,
                ),
                (
                    "Gap",
                    enabled_status(ctx.state.config.sidebar.gap),
                    ToggleSidebarGap,
                ),
                (
                    "Tab strip",
                    enabled_status(ctx.state.config.sidebar.background),
                    ToggleSidebarBackground,
                ),
                (
                    "Tab style",
                    cap_style_label(ctx.state.config.sidebar.tab_style).to_string(),
                    CycleSidebarTabStyle,
                ),
            ],
        ),
        settings_group(
            "Alerts",
            vec![
                (
                    "Bell urgency",
                    enabled_status(ctx.state.config.notifications.bell),
                    ToggleBellUrgency,
                ),
                (
                    "Pane border effect",
                    pane.alert_border.status_label(
                        ctx.state.config.animations.enabled,
                        ctx.state.config.animations.focus_chrome,
                    ),
                    CycleAlertBorder,
                ),
                (
                    "Workspace tab effect",
                    ctx.state.config.workbar.alert.mode.status_label(
                        ctx.state.config.animations.enabled,
                        ctx.state.config.animations.focus_chrome,
                    ),
                    CycleWorkbarAlert,
                ),
                (
                    "Workspace tab highlight",
                    ctx.state.config.workbar.alert.paint.label().to_string(),
                    CycleWorkbarAlertPaint,
                ),
                (
                    "Bell mark",
                    enabled_status(ctx.state.config.workbar.alert.bell),
                    ToggleMarkBell,
                ),
                (
                    "Blocked mark",
                    enabled_status(ctx.state.config.workbar.alert.blocked),
                    ToggleMarkBlocked,
                ),
                (
                    "Finished mark",
                    enabled_status(ctx.state.config.workbar.alert.finished),
                    ToggleMarkFinished,
                ),
                (
                    "Working mark",
                    enabled_status(ctx.state.config.workbar.alert.working),
                    ToggleMarkWorking,
                ),
                (
                    "Idle mark",
                    enabled_status(ctx.state.config.workbar.alert.idle),
                    ToggleMarkIdle,
                ),
            ],
        ),
        settings_group(
            "Desktop notifications",
            vec![
                (
                    "Show notifications",
                    enabled_status(ctx.state.config.notifications.enabled),
                    ToggleDesktopEnabled,
                ),
                (
                    "Blocked",
                    enabled_status(ctx.state.config.notifications.pane_blocked),
                    ToggleDesktopBlocked,
                ),
                (
                    "Finished",
                    enabled_status(ctx.state.config.notifications.pane_done),
                    ToggleDesktopDone,
                ),
                (
                    "Exit",
                    enabled_status(ctx.state.config.notifications.pane_exit),
                    ToggleDesktopExit,
                ),
                (
                    "Exit with error",
                    enabled_status(ctx.state.config.notifications.pane_exit_error),
                    ToggleDesktopExitError,
                ),
            ],
        ),
        settings_group(
            "Sounds",
            vec![
                (
                    "Play sounds",
                    enabled_status(ctx.state.config.sounds.enabled),
                    ToggleSoundEnabled,
                ),
                (
                    "Bell",
                    enabled_status(ctx.state.config.sounds.bell),
                    ToggleSoundBell,
                ),
                (
                    "Blocked",
                    enabled_status(ctx.state.config.sounds.blocked),
                    ToggleSoundBlocked,
                ),
                (
                    "Finished",
                    enabled_status(ctx.state.config.sounds.done),
                    ToggleSoundDone,
                ),
                (
                    "Exit with error",
                    enabled_status(ctx.state.config.sounds.error),
                    ToggleSoundError,
                ),
            ],
        ),
        // Last group: unlike everything above, these change what a *later* launch or server does, so
        // there is nothing on screen to inspect after stepping them.
        settings_group(
            "Sessions",
            vec![
                (
                    "Startup mode",
                    ctx.state.config.session.startup.label().to_string(),
                    CycleStartupMode,
                ),
                (
                    "Layout autosave",
                    enabled_status(ctx.state.config.session.autosave),
                    ToggleSessionAutosave,
                ),
                (
                    "Resurrect named sessions",
                    enabled_status(ctx.state.config.session.resurrect),
                    ToggleSessionResurrect,
                ),
                (
                    "Restored running commands",
                    ctx.state
                        .config
                        .session
                        .resurrect_foreground
                        .label()
                        .to_string(),
                    CycleResurrectForeground,
                ),
            ],
        ),
    ]
}

fn setting_category(group: &str) -> crate::state::SettingsTab {
    use crate::state::SettingsTab;
    match group {
        "General" | "Pickers" => SettingsTab::General,
        "Panes" | "Titlebar" => SettingsTab::Panes,
        "Workbar" | "Sidebar" => SettingsTab::Bars,
        "Alerts" | "Desktop notifications" | "Sounds" => SettingsTab::Alerts,
        "Sessions" => SettingsTab::Sessions,
        _ => unreachable!("unknown settings group"),
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
    let mut groups = settings_groups(ctx);
    groups.sort_by_key(|(group, _)| (setting_category(group).index(), *group == "Titlebar"));
    search_entries_with_groups(groups.into_iter().filter_map(|(group, entries)| {
        let category = setting_category(group);
        if tab != crate::state::SettingsTab::All && tab != category {
            return None;
        }
        let heading = if searching && category.label() != group {
            format!("{} › {group}", category.label())
        } else {
            group.to_string()
        };
        Some((heading, entries))
    }))
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
    ListItem::header(title).style(fg_only(&theme.accent).bold())
}

fn settings_list_rows(ctx: &Context<AppRoot>, rows: usize) -> u16 {
    const ABOVE_LIST: u16 = 7;
    let cap = (ctx.viewport().h * SETTINGS_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(ABOVE_LIST)
        .max(3);
    u16::try_from(rows).unwrap_or(u16::MAX).clamp(1, cap)
}

fn stepped_settings_row(
    targets: &[Option<SettingsAction>],
    current: Option<usize>,
    delta: isize,
) -> Option<usize> {
    let rows = targets
        .iter()
        .enumerate()
        .filter_map(|(index, target)| target.map(|_| index))
        .collect::<Vec<_>>();
    let last = rows.len().checked_sub(1)?;
    let position = current
        .and_then(|current| rows.iter().position(|row| *row == current))
        .unwrap_or(0);
    Some(rows[position.saturating_add_signed(delta).min(last)])
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
    ctx.link().key_handler(move |key| {
        let plain = !key.mods.ctrl && !key.mods.alt && !key.mods.super_key;
        let navigation = match key.code {
            code if plain && !key.mods.shift => {
                let delta = match code {
                    KeyCode::Up => Some(-1),
                    KeyCode::Down => Some(1),
                    KeyCode::PageUp => Some(-page),
                    KeyCode::PageDown => Some(page),
                    KeyCode::Home => Some(isize::MIN),
                    KeyCode::End => Some(isize::MAX),
                    _ => None,
                };
                delta.and_then(|delta| {
                    let index = stepped_settings_row(&targets, selected, delta)?;
                    targets[index].map(Msg::SettingsSelect)
                })
            }
            _ => None,
        };
        navigation.or_else(|| {
            actions
                .iter()
                .find(|(binding, _)| binding.matches_sequence(&[key]))
                .map(|(_, msg)| msg.clone())
        })
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
                let disabled_reason = item.value.0.disabled_reason(&config);
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
                items.push(picker_row(
                    [Span::new(label).style(style)],
                    status,
                    if disabled_reason.is_some() {
                        disabled_style
                    } else {
                        description_style
                    },
                ));
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
    let nested = ctx.state.pane_padding_editor.is_some() || ctx.state.settings_choice.is_some();
    let dim_progress = ctx.transition::<f32>(
        "rozi-settings-padding-dim",
        if nested { 1.0 } else { 0.0 },
        crate::view::animation::scratch_transition_config(ctx),
    );
    let panel: Element = if dim_progress > 0.0 {
        Animated::new(panel)
            .opacity(crate::scratchpad::backdrop_dim(dim_progress))
            .opacity_target(ctx.state.theme.surface.backdrop)
            .transition(crate::layout::anim::instant_transition())
            .into()
    } else {
        panel
    };

    Modal::new()
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
        .into()
}

fn settings_group(
    group: &'static str,
    rows: Vec<(&'static str, String, SettingsAction)>,
) -> (&'static str, Vec<SearchEntry<(SettingsAction, String)>>) {
    let entries = rows
        .into_iter()
        .map(|(label, status, action)| {
            let mut aliases = settings_palette_aliases(group, action);
            aliases.push(Arc::from(setting_category(group).label()));
            SearchEntry::Item(SearchItem::new(label, (action, status)).aliases(aliases))
        })
        .collect();
    (group, entries)
}

fn settings_actions(
    ctx: &Context<AppRoot>,
    selected: Option<SettingsAction>,
) -> Vec<OverlayAction> {
    let can_change =
        selected.is_some_and(|action| action.disabled_reason(&ctx.state.config).is_none());
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
                action.disabled_reason(&ctx.state.config).is_none()
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

pub(crate) fn pane_padding_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(editor) = ctx.state.pane_padding_editor.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    // A labeled, fixed-width numeric field: "Label [ 0 ]". Kept narrow so both axes sit on one row.
    let field = |field: crate::state::PanePaddingField,
                 label: &str,
                 state: &TextInput,
                 key,
                 changed: fn(InputEvent) -> Msg,
                 submit: Msg| {
        {
            let focused = editor.focus == field;
            let input = Input::bound(state)
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
                .on_change(ctx.link().callback(changed))
                .on_focus(ctx.link().callback(move |_| Msg::PanePaddingFocus(field)))
                .on_key(ctx.link().key_handler(move |event| {
                    if event.is(KeyCode::Esc) {
                        Some(Msg::ClosePanePaddingEditor)
                    } else if event.code == KeyCode::Enter
                        && !event.mods.ctrl
                        && !event.mods.alt
                        && !event.mods.super_key
                    {
                        Some(submit.clone())
                    } else {
                        None
                    }
                }))
                .key(key);
            HStack::new()
                .width(Length::Auto)
                .height(Length::Auto)
                .gap(1)
                .child(
                    // The same marker the host editor wears, for the same reason: two borderless
                    // fields side by side otherwise say nothing about which one Enter is aimed at.
                    Text::new(if focused { "›" } else { " " })
                        .width(Length::Px(1))
                        .style(fg_only(&theme.accent)),
                )
                .child(Text::new(label.to_string()).style(if focused {
                    fg_only(&theme.primary).bold()
                } else {
                    fg_only(&theme.muted)
                }))
                .child(input)
        }
    };
    let fields = HStack::new()
        .height(Length::Auto)
        .padding((0, 1))
        .justify(Justify::SpaceBetween)
        .child(field(
            crate::state::PanePaddingField::Vertical,
            "Vertical",
            &editor.vertical,
            pane_padding_vertical_key(),
            Msg::PanePaddingVerticalChanged,
            Msg::AdvancePanePadding,
        ))
        .child(field(
            crate::state::PanePaddingField::Horizontal,
            "Horizontal",
            &editor.horizontal,
            pane_padding_horizontal_key(),
            Msg::PanePaddingHorizontalChanged,
            Msg::SubmitPanePadding,
        ));
    // gap(0): the fields sit under the modal's own top padding, and `hint_row` carries its own
    // leading blank line, so an extra VStack gap would double the spacing.
    let mut body = VStack::new()
        .height(Length::Auto)
        .padding((1, 0, 0, 0))
        .gap(0)
        .child(fields);
    if editor.normalizes_asymmetric {
        // Compact structured status: applying this editor always writes its two-axis form.
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((0, 1))
                .justify(Justify::SpaceBetween)
                .child(Text::new("Apply").style(fg_only(&theme.muted)))
                .child(Text::new("Symmetric").style(fg_only(&theme.primary))),
        );
    }
    let body = body.child(
        hint_row()
            .child(hint_pill(theme, "next / apply", "enter"))
            // Both keys land back in Settings whenever it is the dialog behind this one.
            .child(hint_pill(
                theme,
                if ctx.state.show_settings {
                    "back"
                } else {
                    "cancel"
                },
                "esc",
            )),
    );
    nested_action_palette_modal(ctx, "Terminal padding", SETTINGS_MAX_HEIGHT_PERCENT)
        .width(Length::Auto)
        .on_close(ctx.link().callback(|_| Msg::ClosePanePaddingEditor))
        .child(body)
        .into()
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
            let mut entry = SearchEntry::item(*label, index);
            if index == editor.original_index {
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
        .backdrop_style(Style::new().tint_by(theme.surface.backdrop, BACKDROP_RECESSION))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::SettingsChoiceCancel))
        .child(palette)
        .key(settings_choice_key())
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
