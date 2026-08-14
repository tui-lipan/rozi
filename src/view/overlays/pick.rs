pub(crate) fn pick_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Text::new("").into();
    };

    let title = pick.title.clone();
    let placeholder = pick.placeholder.clone();
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

    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder(placeholder)
        .preserve_groups(has_groups)
        .initial_selected_item_index(selected_index)
        .sync_selection(true)
        .render_item(Arc::new(
            move |item: &SearchItem<usize>, _highlight| {
                let row = &rows[item.value];
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

    action_palette(ctx, &title, pick_key(), Msg::ClosePick, palette, 60)
}
