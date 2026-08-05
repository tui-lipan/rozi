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
        if picker.apply_mode {
            "Replace session with profile"
        } else {
            "Profiles"
        },
        profile_picker_key(),
        Msg::CloseProfilePicker,
        body,
        64,
    )
}

fn profile_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let Some(picker) = ctx.state.profile_picker.as_ref() else {
        return Text::new("").into();
    };
    let query = picker.input.text().trim().to_ascii_lowercase();
    let selected = picker.entries.get(picker.selected).filter(|entry| {
        query.is_empty() || entry.name.to_ascii_lowercase().contains(&query)
    });
    if picker.apply_mode {
        let mut hints = hint_row();
        if selected.is_some() {
            hints = hints.child(hint_pill(theme, "replace", "enter"));
        }
        return hints
            .child(hint_pill(theme, "cancel", "esc"))
            .into();
    }
    let mut hints = hint_row();
    if let Some(entry) = selected {
        let current = ctx.state.current().session_name.as_deref() == Some(entry.name.as_str());
        if !current {
            let running = matches!(
                picker.running.get(&entry.name),
                Some(crate::session::discovery::DiscoveredSessionStatus::Running { .. })
            );
            hints = hints.child(hint_pill(
                theme,
                if running { "attach" } else { "launch" },
                "enter",
            ));
        }
        hints = hints.child(hint_pill(theme, "open as", "ctrl+o"));
        hints = hints.child(hint_pill(theme, "replace", "ctrl+r"));
        hints = hints.child(hint_pill(theme, "default", "ctrl+f"));
    }
    hints = hints.child(hint_pill(theme, "new", "ctrl+n"));
    if selected.is_some() {
        hints = hints.child(hint_pill(theme, "delete", "ctrl+d"));
    }
    hints.into()
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
            let mut item = SearchEntry::item(entry.name.clone(), index);
            let status = if ctx.state.current().session_name.as_deref() == Some(entry.name.as_str()) {
                Some("• attached")
            } else if matches!(
                picker.running.get(&entry.name),
                Some(crate::session::discovery::DiscoveredSessionStatus::Running { .. })
            ) {
                Some("• running")
            } else {
                None
            };
            let description = match (default_name == Some(entry.name.as_str()), status) {
                (true, Some(status)) => format!("default  {status}"),
                (true, None) => "default".to_string(),
                (false, Some(status)) => status.to_string(),
                (false, None) => String::new(),
            };
            item = item.description(ItemDescription::new().right(description));
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
    let pending_apply = picker.pending_apply;
    let error_bg = theme.status.error;
    let warn_bg = theme.status.warning;
    let selection_style = picker_selection_style(
        theme,
        pending_delete.is_some().then_some(error_bg).or_else(|| {
            (pending_open.is_some() || pending_apply.is_some()).then_some(warn_bg)
        }),
    );

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
        .on_activate(ctx.link().callback({
            let apply_mode = picker.apply_mode;
            move |event: SearchEvent<usize>| {
                if apply_mode {
                    Msg::ProfilePickerApply
                } else {
                    Msg::SelectProfile(event.item.value)
                }
            }
        }));

    if pending_delete.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            (pending_delete == Some(item.value)).then(|| {
                render_pending_confirm_item(item.label.as_ref(), error_bg, "again to confirm", true)
            })
        }));
    }
    if pending_open.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            (pending_open == Some(item.value)).then(|| {
                render_pending_confirm_item(
                    item.label.as_ref(),
                    warn_bg,
                    "again to confirm (ends ephemeral)",
                    false,
                )
            })
        }));
    }
    if pending_apply.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            (pending_apply == Some(item.value)).then(|| {
                render_pending_confirm_item(
                    item.label.as_ref(),
                    warn_bg,
                    "again to confirm (replaces ephemeral)",
                    false,
                )
            })
        }));
    }

    palette
}

fn profile_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')) {
            Some(Msg::ProfilePickerOpenAs)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            Some(Msg::ProfilePickerNew)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D')) {
            Some(Msg::ProfilePickerDelete)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F')) {
            Some(Msg::ProfilePickerSetDefault)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            Some(Msg::ProfilePickerApply)
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

/// Armed second-press row for session/profile/collaborator pickers.
///
/// `strike` marks destructive removal (kill/delete/kick): error/warning fill with a struck label.
/// Without it the row survives the confirm (restart/open) and reads as a bold warning highlight.
fn render_pending_confirm_item(
    label: &str,
    accent: Color,
    cue: &str,
    strike: bool,
) -> ListItem {
    let fg = readable_text_color(None, accent);
    let label_style = if strike {
        Style::new().fg(fg).strikethrough()
    } else {
        Style::new().fg(fg).bold()
    };
    ListItem::from_spans(vec![Span::new(label).style(label_style)])
        .description(cue)
        .description_style(Style::new().fg(fg).italic())
        .style(Style::new().bg(accent).fg(fg))
}

fn render_ephemeral_session_item<T>(
    item: &SearchItem<T>,
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
