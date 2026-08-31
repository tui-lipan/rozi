pub(crate) fn settings_overlay(app: &AppRoot, ctx: &Context<AppRoot>) -> Element {
    use SettingsAction::*;

    let pane = &ctx.state.config.pane;
    let entries = search_entries_with_groups([
        settings_group(
            "General",
            vec![
                ("Theme", current_theme_label(ctx), Theme),
                ("Terminal padding", padding_summary(pane.padding), EditPadding),
                (
                    "Animations",
                    enabled_status(ctx.state.config.animations.enabled),
                    ToggleAnimations,
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
                ("Focus on hover", enabled_status(pane.focus_on_hover), ToggleFocusOnHover),
                (
                    "Background follows terminal",
                    enabled_status(pane.background_follows_terminal),
                    ToggleBackgroundFollowsTerminal,
                ),
            ],
        ),
        settings_group(
            "Titlebar",
            vec![
                ("Show titlebar", enabled_status(pane.show_titles), ToggleTitles),
                ("Layout", pane.titlebar.label().to_string(), CycleTitlebar),
                ("Style", cap_style_label(pane.title_style).to_string(), CycleTitleStyle),
            ],
        ),
        settings_group(
            "Workbar",
            vec![
                ("Show workbar", enabled_status(pane.show_workbar), ToggleWorkbar),
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
                ("Style", cap_style_label(pane.workbar_style).to_string(), CycleWorkbarStyle),
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
                ("Powerline", enabled_status(pane.workbar_powerline), ToggleWorkbarPowerline),
            ],
        ),
        settings_group(
            "Panes",
            vec![
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
                ("Border mode", pane.border_mode.label().to_string(), CycleBorderMode),
                ("Border style", pane.border_style.label().to_string(), CycleBorderStyle),
                (
                    "Open/close animation",
                    ctx.state.config.animations.pane_style.label().to_string(),
                    CyclePaneAnimation,
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
                ("Bell mark", enabled_status(ctx.state.config.workbar.alert.bell), ToggleMarkBell),
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
                ("Idle mark", enabled_status(ctx.state.config.workbar.alert.idle), ToggleMarkIdle),
            ],
        ),
        settings_group(
            "Desktop notifications",
            vec![
                ("Show notifications", enabled_status(ctx.state.config.notifications.enabled), ToggleDesktopEnabled),
                ("Blocked", enabled_status(ctx.state.config.notifications.pane_blocked), ToggleDesktopBlocked),
                ("Finished", enabled_status(ctx.state.config.notifications.pane_done), ToggleDesktopDone),
                ("Exit", enabled_status(ctx.state.config.notifications.pane_exit), ToggleDesktopExit),
                ("Exit with error", enabled_status(ctx.state.config.notifications.pane_exit_error), ToggleDesktopExitError),
            ],
        ),
        settings_group(
            "Sounds",
            vec![
                ("Play sounds", enabled_status(ctx.state.config.sounds.enabled), ToggleSoundEnabled),
                ("Bell", enabled_status(ctx.state.config.sounds.bell), ToggleSoundBell),
                ("Blocked", enabled_status(ctx.state.config.sounds.blocked), ToggleSoundBlocked),
                ("Finished", enabled_status(ctx.state.config.sounds.done), ToggleSoundDone),
                ("Exit with error", enabled_status(ctx.state.config.sounds.error), ToggleSoundError),
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
            ],
        ),
    ]);

    let config = ctx.state.config.clone();
    let item_style = fg_only(&ctx.state.theme.primary);
    let description_style = fg_only(&ctx.state.theme.muted);
    let disabled_style = fg_only(&ctx.state.theme.muted);
    let selected_index = ctx.state.settings_selected.and_then(|selected| {
        entries
            .iter()
            .filter_map(|entry| match entry {
                SearchEntry::Item(item) => Some(item.value.0),
                _ => None,
            })
            .position(|action| action == selected)
    });
    let palette = shared_search_palette::<(SettingsAction, String)>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search settings…")
        .preserve_groups(true)
        .initial_selected_item_index(selected_index)
        .sync_selection(true)
        .input_key_interceptor(settings_palette_key_interceptor(ctx))
        .render_item(Arc::new(
            move |item: &SearchItem<(SettingsAction, String)>, _highlight| {
                let disabled_reason = item.value.0.disabled_reason(&config);
                let status = disabled_reason.unwrap_or(&item.value.1);
                let style = if disabled_reason.is_some() {
                    disabled_style
                } else {
                    item_style
                };
                ListItem::from_spans(vec![Span::new(item.label.as_ref()).style(style)])
                    .description(status)
                    .description_style(if disabled_reason.is_some() {
                        disabled_style
                    } else {
                        description_style
                    })
                    .into()
            },
        ))
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<(SettingsAction, String)>| {
                    Msg::SettingsSelect(event.item.value.0)
                }),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<(SettingsAction, String)>| {
                    Msg::SettingsActivate(event.item.value.0)
                }),
        );

    let panel: Element = Frame::new()
        .header_left("Settings")
        .header_style(ctx.state.theme.accent.bold())
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(action_palette_frame(palette))
        .into();
    let dim_progress = ctx.transition::<f32>(
        "rozi-settings-padding-dim",
        if ctx.state.pane_padding_editor.is_some() {
            1.0
        } else {
            0.0
        },
        app.scratch_transition_config(ctx),
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
        .width(Length::Px(60))
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(|_| Msg::CloseSettings))
        .child(panel)
        .key(settings_palette_key())
}

fn settings_group(
    group: &'static str,
    rows: Vec<(&'static str, String, SettingsAction)>,
) -> (&'static str, Vec<SearchEntry<(SettingsAction, String)>>) {
    let entries = rows
        .into_iter()
        .map(|(label, status, action)| {
            SearchEntry::Item(
                SearchItem::new(label, (action, status))
                    .aliases(settings_palette_aliases(group, action)),
            )
        })
        .collect();
    (group, entries)
}

fn settings_palette_key_interceptor(ctx: &Context<AppRoot>) -> KeyHandler {
    let selected = ctx.state.settings_selected;
    ctx.link().key_handler(move |key| {
        if key.mods != KeyMods::default() {
            return None;
        }
        if !selected.is_some_and(SettingsAction::steps_horizontally) {
            return None;
        }
        match key.code {
            KeyCode::Left => Some(Msg::SettingsStep { reverse: true }),
            KeyCode::Right => Some(Msg::SettingsStep { reverse: false }),
            _ => None,
        }
    })
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
    let field =
        |label: &str, state: &TextInput, key, changed: fn(InputEvent) -> Msg, submit: Msg| {
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
                .child(Text::new(label.to_string()).style(fg_only(&theme.muted)))
                .child(input)
        };
    let fields = HStack::new()
        .height(Length::Auto)
        .padding((0, 1))
        .justify(Justify::SpaceBetween)
        .child(field(
            "Vertical",
            &editor.vertical,
            pane_padding_vertical_key(),
            Msg::PanePaddingVerticalChanged,
            Msg::AdvancePanePadding,
        ))
        .child(field(
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
    action_palette_modal(ctx, "Terminal padding")
        .width(Length::Auto)
        .on_close(ctx.link().callback(|_| Msg::ClosePanePaddingEditor))
        .child(action_palette_frame(body))
        .into()
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

fn action_search_palette(
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
            entry = entry.description(ItemDescription::new().right(description));
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
