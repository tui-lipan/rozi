pub(crate) fn profile_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(picker) = ctx.state.profile_picker.as_ref() else {
        return Text::new("").into();
    };

    let body = VStack::new()
        .height(Length::Auto)
        .child(profile_picker_palette(ctx, picker))
        .child(profile_picker_hints(ctx));

    action_palette(
        ctx,
        "Profiles",
        profile_picker_key(),
        Msg::CloseProfilePicker,
        body,
        60,
    )
}

fn profile_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    hint_row()
        .justify(Justify::SpaceBetween)
        .gap(2)
        .child(hint_pill(theme, "open", "enter"))
        .child(hint_pill(theme, "default", "ctrl+f"))
        .child(hint_pill(theme, "delete", "ctrl+d"))
        .into()
}

fn profile_picker_palette(
    ctx: &Context<HyprmuxApp>,
    picker: &ProfilePickerState,
) -> SearchPalette<usize> {
    let theme = &ctx.state.theme;
    let default_name = ctx.state.config.profile.default.as_deref();
    let query = picker.input.text().trim().to_ascii_lowercase();
    let entries = picker
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
        .map(|(index, entry)| {
            let mut item =
                SearchEntry::item(entry.name.clone(), index).active(index == picker.selected);
            if default_name == Some(entry.name.as_str()) {
                item = item.description(ItemDescription::new().right("default"));
            }
            item
        })
        .collect::<Vec<_>>();

    let empty_text = if picker.entries.is_empty() {
        "No saved profiles - save one first".to_string()
    } else if query.is_empty() {
        "Type to filter profiles".to_string()
    } else {
        format!("No profiles match `{query}`")
    };

    let pending_delete = picker.pending_delete;
    let pending_open = picker.pending_open;
    let error_bg = theme.status.error;
    let warn_bg = theme.status.warning;
    let selection_style =
        picker_selection_style(theme, pending_delete.is_some().then_some(error_bg).or_else(|| pending_open.is_some().then_some(warn_bg)));

    let mut palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search profiles...")
        .initial_query(picker.input.text().to_string())
        .preserve_groups(false)
        .initial_selected_item_index(Some(picker.selected))
        .sync_selection(true)
        .empty_text(empty_text)
        .list_selection_style(selection_style)
        .list_unfocused_selection_style(selection_style)
        .input_key_interceptor(profile_picker_key_interceptor(ctx))
        .on_query_change(ctx.link().callback(|query: std::sync::Arc<str>| {
            Msg::ProfilePickerQueryChanged(query.to_string())
        }))
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::ProfilePickerSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SelectProfile(event.item.value)),
        );

    if pending_delete.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            if pending_delete == Some(item.value) {
                Some(render_pending_delete_item(item, error_bg))
            } else {
                None
            }
        }));
    }
    if pending_open.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            (pending_open == Some(item.value)).then(|| render_pending_open_item(item, warn_bg))
        }));
    }

    palette
}

fn profile_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D')) {
            Some(Msg::ProfilePickerDelete)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F')) {
            Some(Msg::ProfilePickerSetDefault)
        } else {
            None
        }
    })
}

/// Selection highlight for the profile/session pickers. While an action awaits its confirming
/// second press the selected row adopts `pending_accent` (error for a kill/delete, warning for the
/// cautionary open-discards-ephemeral guard); otherwise it uses the normal accent.
fn picker_selection_style(theme: &Theme, pending_accent: Option<Color>) -> Style {
    if let Some(accent) = pending_accent {
        Style::new()
            .bg(accent)
            .fg(readable_text_color(None, accent))
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    } else {
        Style::new()
            .fg(theme.surface.backdrop)
            .bg(theme.border_active)
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    }
}

fn render_pending_delete_item(item: &SearchItem<usize>, error_bg: Color) -> ListItem {
    let fg = readable_text_color(None, error_bg);
    ListItem::from_spans(vec![
        Span::new(item.label.as_ref()).style(Style::new().fg(fg).strikethrough()),
    ])
    .description("again to confirm")
    .description_style(Style::new().fg(fg).italic())
    .style(Style::new().bg(error_bg).fg(fg))
}

/// The target row while an open awaits its confirming second Enter. Unlike a pending kill (which
/// strikes the row through, since the row itself is going away), the target survives - the cost is
/// to the *current* ephemeral session - so it reads as a warning-colored highlight whose hint spells
/// out the trade rather than a deletion.
fn render_pending_open_item(item: &SearchItem<usize>, warn_bg: Color) -> ListItem {
    let fg = readable_text_color(None, warn_bg);
    ListItem::from_spans(vec![
        Span::new(item.label.as_ref()).style(Style::new().fg(fg).bold()),
    ])
    .description("again to confirm (ends ephemeral)")
    .description_style(Style::new().fg(fg).italic())
    .style(Style::new().bg(warn_bg).fg(fg))
}

fn render_ephemeral_session_item(
    item: &SearchItem<usize>,
    label_style: &Style,
    description_style: &Style,
) -> ListItem {
    let label = item.label.as_ref();
    let suffix = label.strip_prefix("ephemeral").unwrap_or_default();
    let mut row = ListItem::from_spans(vec![
        Span::new("ephemeral").style(*label_style),
        Span::new(suffix),
    ]);
    if let Some(description) = item
        .description
        .as_ref()
        .and_then(|desc| desc.right.clone())
    {
        row = row
            .description(description)
            .description_style(*description_style);
    }
    row
}
