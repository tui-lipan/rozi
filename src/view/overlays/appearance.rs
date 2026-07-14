pub(crate) fn appearance_overlay(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let pane = &ctx.state.config.pane;
    // Dependent rows (Titlebar style, Workbar gap/position) always stay in the list; when their
    // parent feature is off they render greyed and non-activating (see `disabled_reason` and the
    // `render_item` below) rather than disappearing. Grouped so each control sits next to the
    // toggle it depends on.
    let entries = vec![
        appearance_entry("Theme", current_theme_label(ctx), AppearanceAction::Theme),
        appearance_entry(
            "Terminal padding",
            padding_summary(pane.padding),
            AppearanceAction::EditPadding,
        ),
        appearance_entry(
            "Titlebar",
            enabled_status(pane.show_titles),
            AppearanceAction::ToggleTitles,
        ),
        appearance_entry(
            "Titlebar style",
            pane.title_style.label().to_string(),
            AppearanceAction::CycleTitleStyle,
        ),
        appearance_entry(
            "Workbar",
            enabled_status(pane.show_workbar),
            AppearanceAction::ToggleWorkbar,
        ),
        appearance_entry(
            "Workbar gap",
            enabled_status(pane.workbar_gap),
            AppearanceAction::ToggleWorkbarGap,
        ),
        appearance_entry(
            "Workbar position",
            if pane.workbar_at_bottom {
                "Bottom"
            } else {
                "Top"
            }
            .to_string(),
            AppearanceAction::ToggleWorkbarPosition,
        ),
        appearance_entry(
            "Workbar style",
            pane.workbar_style.label().to_string(),
            AppearanceAction::CycleWorkbarStyle,
        ),
        appearance_entry(
            "Workbar badge style",
            pane.workbar_badge_style.label().to_string(),
            AppearanceAction::CycleWorkbarBadgeStyle,
        ),
        appearance_entry(
            "Workbar powerline",
            enabled_status(pane.workbar_powerline),
            AppearanceAction::ToggleWorkbarPowerline,
        ),
        appearance_entry(
            "Workbar tab style",
            pane.workbar_tab_style.label().to_string(),
            AppearanceAction::CycleWorkbarTabStyle,
        ),
        appearance_entry(
            "Animations",
            enabled_status(ctx.state.config.animations.enabled),
            AppearanceAction::ToggleAnimations,
        ),
        appearance_entry(
            "Focused pane background",
            enabled_status(pane.highlight_focused_background),
            AppearanceAction::ToggleHighlightFocusedBackground,
        ),
        appearance_entry(
            "Focused pane border",
            enabled_status(pane.highlight_focused_border),
            AppearanceAction::ToggleHighlightFocusedBorder,
        ),
        appearance_entry(
            "Border merging",
            enabled_status(pane.merge_borders),
            AppearanceAction::ToggleBorderMerge,
        ),
        appearance_entry(
            "Border style",
            pane.border_style.label().to_string(),
            AppearanceAction::CycleBorderStyle,
        ),
        appearance_entry(
            "Background follows terminal",
            enabled_status(pane.background_follows_terminal),
            AppearanceAction::ToggleBackgroundFollowsTerminal,
        ),
    ];

    let pane_flags = ctx.state.config.pane;
    let disabled_style = fg_only(&ctx.state.theme.muted);
    let palette = shared_search_palette::<AppearanceAction>(ctx, Length::Auto, true)
        .entries(entries)
        .placeholder("Search appearance…")
        .description_placement(DescriptionPlacement::Right)
        .render_item(Arc::new(
            move |item: &SearchItem<AppearanceAction>, _highlight| {
                item.value.disabled_reason(&pane_flags).map(|reason| {
                    ListItem::from_spans(vec![Span::new(item.label.as_ref()).style(disabled_style)])
                        .description(reason)
                        .description_style(disabled_style)
                })
            },
        ))
        .on_activate(ctx.link().callback(|event: SearchEvent<AppearanceAction>| {
            Msg::AppearanceActivate(event.item.value)
        }));

    let panel: Element = Frame::new()
        .title("Change appearance")
        .title_style(ctx.state.theme.accent.bold())
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
            .child(hint_pill(theme, "cancel", "esc")),
    );
    action_palette_modal(ctx, "Terminal padding")
        .width(Length::Auto)
        .on_close(ctx.link().callback(|_| Msg::ClosePanePaddingEditor))
        .child(action_palette_frame(body))
        .into()
}

fn appearance_entry(
    label: impl Into<Arc<str>>,
    status: String,
    action: AppearanceAction,
) -> SearchEntry<AppearanceAction> {
    SearchEntry::Item(SearchItem::new(label, action).aliases(appearance_palette_aliases(action)))
        .description(ItemDescription::new().right(status))
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
    shared_search_palette::<Callback<()>>(ctx, Length::Auto, true)
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
    let initial_selected = choices.iter().position(|choice| &choice.id() == current);

    let entries: Vec<SearchEntry<usize>> = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let mut entry = SearchEntry::item(choice.label(), index);
            if Some(index) == initial_selected {
                entry = entry.description(ItemDescription::new().right("current"));
            }
            entry
        })
        .collect();

    // Mirror the command palette so theme selection reuses the same fuzzy-search UX.
    let palette = shared_search_palette::<usize>(ctx, Length::Auto, true)
        .entries(entries)
        .placeholder("Search themes…")
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
