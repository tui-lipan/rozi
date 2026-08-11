pub(crate) fn appearance_overlay(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let pane = &ctx.state.config.pane;
    // Dependent rows (Titlebar Style, Workbar Gap/Position) always stay in the list; when their
    // parent feature is off they render greyed and non-activating (see `disabled_reason` and the
    // `render_item` below) rather than disappearing. Category headers mirror the Commands picker;
    // short labels rely on group context (and aliases for search).
    let row = |label: &str, status: String, action: AppearanceAction| {
        SearchEntry::Item(
            SearchItem::new(label, (action, status)).aliases(appearance_palette_aliases(action)),
        )
    };
    let entries = search_entries_with_groups([
        (
            "General",
            vec![
                row("Theme", current_theme_label(ctx), AppearanceAction::Theme),
                row(
                    "Terminal padding",
                    padding_summary(pane.padding),
                    AppearanceAction::EditPadding,
                ),
                row(
                    "Animations",
                    enabled_status(ctx.state.config.animations.enabled),
                    AppearanceAction::ToggleAnimations,
                ),
                row(
                    "Background follows terminal",
                    enabled_status(pane.background_follows_terminal),
                    AppearanceAction::ToggleBackgroundFollowsTerminal,
                ),
            ],
        ),
        (
            "Titlebar",
            vec![
                row(
                    "Show titlebar",
                    enabled_status(pane.show_titles),
                    AppearanceAction::ToggleTitles,
                ),
                row(
                    "Layout",
                    pane.titlebar.label().to_string(),
                    AppearanceAction::CycleTitlebar,
                ),
                row(
                    "Style",
                    cap_style_label(pane.title_style).to_string(),
                    AppearanceAction::CycleTitleStyle,
                ),
            ],
        ),
        (
            "Workbar",
            vec![
                row(
                    "Show workbar",
                    enabled_status(pane.show_workbar),
                    AppearanceAction::ToggleWorkbar,
                ),
                row(
                    "Position",
                    if pane.workbar_at_bottom {
                        "Bottom"
                    } else {
                        "Top"
                    }
                    .to_string(),
                    AppearanceAction::ToggleWorkbarPosition,
                ),
                row(
                    "Gap",
                    enabled_status(pane.workbar_gap),
                    AppearanceAction::ToggleWorkbarGap,
                ),
                row(
                    "Style",
                    cap_style_label(pane.workbar_style).to_string(),
                    AppearanceAction::CycleWorkbarStyle,
                ),
                row(
                    "Badge style",
                    cap_style_label(pane.workbar_badge_style).to_string(),
                    AppearanceAction::CycleWorkbarBadgeStyle,
                ),
                row(
                    "Tab style",
                    cap_style_label(pane.workbar_tab_style).to_string(),
                    AppearanceAction::CycleWorkbarTabStyle,
                ),
                row(
                    "Powerline",
                    enabled_status(pane.workbar_powerline),
                    AppearanceAction::ToggleWorkbarPowerline,
                ),
                row(
                    "Alert",
                    ctx.state.config.workbar.alert.mode.status_label(
                        ctx.state.config.animations.enabled,
                        ctx.state.config.animations.focus_chrome,
                    ),
                    AppearanceAction::CycleWorkbarAlert,
                ),
                row(
                    "Alert paint",
                    ctx.state.config.workbar.alert.paint.label().to_string(),
                    AppearanceAction::CycleWorkbarAlertPaint,
                ),
            ],
        ),
        (
            "Focused pane",
            vec![
                row(
                    "Background",
                    enabled_status(pane.highlight_focused_background),
                    AppearanceAction::ToggleHighlightFocusedBackground,
                ),
                row(
                    "Border",
                    enabled_status(pane.highlight_focused_border),
                    AppearanceAction::ToggleHighlightFocusedBorder,
                ),
                row(
                    "Titlebar",
                    enabled_status(pane.highlight_focused_titlebar),
                    AppearanceAction::ToggleHighlightFocusedTitlebar,
                ),
            ],
        ),
        (
            "Pane borders",
            vec![
                row(
                    "Mode",
                    pane.border_mode.label().to_string(),
                    AppearanceAction::CycleBorderMode,
                ),
                row(
                    "Style",
                    pane.border_style.label().to_string(),
                    AppearanceAction::CycleBorderStyle,
                ),
                row(
                    "Alert",
                    pane.alert_border.status_label(
                        ctx.state.config.animations.enabled,
                        ctx.state.config.animations.focus_chrome,
                    ),
                    AppearanceAction::CycleAlertBorder,
                ),
            ],
        ),
    ]);

    let pane_flags = ctx.state.config.pane;
    let item_style = fg_only(&ctx.state.theme.primary);
    let description_style = fg_only(&ctx.state.theme.muted);
    let disabled_style = fg_only(&ctx.state.theme.muted);
    let selected_index = ctx.state.appearance_selected.and_then(|selected| {
        entries
            .iter()
            .filter_map(|entry| match entry {
                SearchEntry::Item(item) => Some(item.value.0),
                _ => None,
            })
            .position(|action| action == selected)
    });
    let palette = shared_search_palette::<(AppearanceAction, String)>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search appearance…")
        .preserve_groups(true)
        .initial_selected_item_index(selected_index)
        .sync_selection(true)
        .input_key_interceptor(appearance_palette_key_interceptor(ctx))
        .render_item(Arc::new(
            move |item: &SearchItem<(AppearanceAction, String)>, _highlight| {
                let disabled_reason = item.value.0.disabled_reason(&pane_flags);
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
                .callback(|event: SearchEvent<(AppearanceAction, String)>| {
                    Msg::AppearanceSelect(event.item.value.0)
                }),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<(AppearanceAction, String)>| {
                    Msg::AppearanceActivate(event.item.value.0)
                }),
        );

    let panel: Element = Frame::new()
        .header_left("Change appearance")
        .header_style(ctx.state.theme.accent.bold())
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(action_palette_frame(palette))
        .into();
    let dim_progress = ctx.transition::<f32>(
        "hyprmux-appearance-padding-dim",
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
            .transition(crate::anim::instant_transition())
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
        .on_close(ctx.link().callback(|_| Msg::CloseAppearance))
        .child(panel)
        .key(appearance_palette_key())
}

fn appearance_palette_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    // Always claim bare Left/Right; `appearance_step` reads live selection so a stale
    // render-time capture cannot step the wrong row (or miss a move after Up/Down).
    ctx.link().key_handler(|key| {
        if key.mods != KeyMods::default() {
            return None;
        }
        match key.code {
            KeyCode::Left => Some(Msg::AppearanceStep { reverse: true }),
            KeyCode::Right => Some(Msg::AppearanceStep { reverse: false }),
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

pub(crate) fn pane_padding_overlay(ctx: &Context<HyprmuxApp>) -> Element {
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
        // Only worth a line when the current padding is uneven and applying will flatten it.
        body = body.child(
            Text::new("Sides differ; applying writes symmetric padding.")
                .style(fg_only(&theme.muted))
                .overflow(Overflow::Wrap),
        );
    }
    let body = body.child(
        hint_row()
            .child(hint_pill(theme, "next / apply", "enter"))
            // Both keys land back in Appearance whenever it is the dialog behind this one.
            .child(hint_pill(
                theme,
                if ctx.state.show_appearance {
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

fn current_theme_label(ctx: &Context<HyprmuxApp>) -> String {
    let current = &ctx.state.config.theme.name;
    crate::config::theme_choices()
        .into_iter()
        .find(|choice| &choice.id() == current)
        .map(|choice| choice.label())
        .unwrap_or_else(|| current.clone())
}

fn action_search_palette(
    ctx: &Context<HyprmuxApp>,
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

pub(crate) fn theme_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
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
                entries.push(SearchEntry::header(group));
            }
            previous_group = group;
        }

        let mut entry = SearchEntry::item(choice.label(), index);
        if Some(index) == current_index {
            entry = entry.description(ItemDescription::new().right("current"));
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
