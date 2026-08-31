pub(crate) fn pick_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Text::new("").into();
    };

    let title = pick.title.clone();
    let placeholder = pick.placeholder.clone();
    let width = pick.width;
    let restore_query = pick.restore_query.clone();
    let rows = pick.rows.clone();
    let has_groups = rows.iter().any(|r| r.group.is_some());
    let selected_index = Some(pick.selected);

    let entries = if has_groups {
        let mut groups: Vec<(String, Vec<SearchEntry<usize>>)> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let group_name = row.group.as_deref().unwrap_or("Other").to_string();
            let mut item = SearchItem::new(row.label.clone(), index).active(row.active);
            if let Some(priority) = row.priority {
                item = item.priority(priority);
            }
            let description = row
                .disabled
                .as_deref()
                .or(row.description.as_deref())
                .unwrap_or("");
            if !description.is_empty() {
                item = item.description(ItemDescription::new().right(description.to_string()));
            }
            let entry = SearchEntry::Item(item);
            if let Some((_, items)) = groups.iter_mut().find(|(name, _)| *name == group_name) {
                items.push(entry);
            } else {
                groups.push((group_name, vec![entry]));
            }
        }
        search_entries_with_groups(groups)
    } else {
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let mut item = SearchItem::new(row.label.clone(), index).active(row.active);
                if let Some(priority) = row.priority {
                    item = item.priority(priority);
                }
                let description = row
                    .disabled
                    .as_deref()
                    .or(row.description.as_deref())
                    .unwrap_or("");
                if !description.is_empty() {
                    item = item.description(ItemDescription::new().right(description.to_string()));
                }
                SearchEntry::Item(item)
            })
            .collect::<Vec<_>>()
    };

    let disabled_style = fg_only(&ctx.state.theme.muted);
    let item_style = fg_only(&ctx.state.theme.primary);
    let description_style = fg_only(&ctx.state.theme.muted);
    // An armed `confirm` action paints its row exactly like the session picker's armed kill, so
    // the affordance is one the user has already learned somewhere else.
    let armed = pick.pending_action.clone().and_then(|(index, row)| {
        pick.actions
            .get(index)
            .map(|action| (row, format!("again to {}", action.label)))
    });
    let error_bg = ctx.state.theme.status.error;

    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder(placeholder)
        // Rebuilt, not un-hidden: the picker unmounts while a prompt is up, so it comes back
        // seeded with the filter that was typed before.
        .initial_query(restore_query)
        .on_query_change(
            ctx.link()
                .callback(|query: std::sync::Arc<str>| Msg::PickQueryChanged(query.to_string())),
        )
        .preserve_groups(has_groups)
        .initial_selected_item_index(selected_index)
        .sync_selection(true)
        .render_item(Arc::new(
            move |item: &SearchItem<usize>, _highlight| {
                let row = &rows[item.value];
                if let Some((armed_row, cue)) = armed.as_ref()
                    && row.id.as_ref().unwrap_or(&row.label) == armed_row
                {
                    return render_pending_confirm_item(
                        item.label.as_ref(),
                        error_bg,
                        cue,
                        true,
                    )
                    .into();
                }
                let disabled_reason = row.disabled.as_deref();
                let status = disabled_reason.or(row.description.as_deref()).unwrap_or("");
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
        .on_select(ctx.link().callback(|event: SearchEvent<usize>| {
            Msg::PickSelect(event.item.value)
        }))
        .on_activate(ctx.link().callback(|event: SearchEvent<usize>| {
            Msg::PickActivate(event.item.value)
        }));

    let palette = palette.input_key_interceptor(pick_key_interceptor(ctx));

    let body = VStack::new()
        .height(Length::Auto)
        .child(palette)
        .child(pick_hints(ctx));

    action_palette(ctx, &title, pick_key(), Msg::ClosePick, body, width)
}

/// Footer chords: select, plus whatever the caller declared. Built the same way every other picker
/// builds its own, so a caller-driven one is not visibly a second-class citizen.
fn pick_hints(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let mut row = hint_row();
    if let Some(pick) = ctx.state.pick.as_ref() {
        if pick
            .rows
            .get(pick.selected)
            .is_some_and(|row| row.disabled.is_none())
        {
            row = row.child(hint_pill(theme, "select", "enter"));
        }
        for action in &pick.actions {
            row = row.child(hint_pill(
                theme,
                &action.label,
                &KeyBinding::from_str(&action.key)
                    .map(|binding| crate::view::keys_display::format_binding(&binding))
                    .unwrap_or_else(|_| action.key.clone()),
            ));
        }
    }
    // No `esc` pill: cancelling is the app-wide default for every overlay, and the built-in
    // pickers do not spend footer space restating it.
    row.into()
}

/// Match a declared action chord against the key the palette's input did not consume.
fn pick_key_interceptor(ctx: &Context<AppRoot>) -> KeyHandler {
    let actions: Vec<(usize, KeyBinding)> = ctx
        .state
        .pick
        .as_ref()
        .map(|pick| {
            pick.actions
                .iter()
                .enumerate()
                .filter_map(|(index, action)| {
                    KeyBinding::from_str(&action.key)
                        .ok()
                        .map(|binding| (index, binding))
                })
                .collect()
        })
        .unwrap_or_default();
    ctx.link().key_handler(move |key| {
        actions
            .iter()
            .find(|(_, binding)| binding.matches_sequence(&[key]))
            .map(|(index, _)| Msg::PickActionKey(*index))
    })
}

/// The text prompt an action raised. Rendered above the picker, which stays mounted underneath so
/// cancelling returns to the list with its query and highlight intact.
pub(crate) fn pick_prompt_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(prompt) = ctx.state.pick.as_ref().and_then(|pick| pick.prompt.as_ref()) else {
        return Text::new("").into();
    };
    prompt_overlay(
        ctx,
        PromptChrome::new(&prompt.title, "", &[("submit", "enter")]),
        &prompt.input,
        pick_prompt_input_key(),
        Msg::PickPromptChanged,
        Msg::PickPromptCancel,
        Msg::PickPromptSubmit,
    )
}
