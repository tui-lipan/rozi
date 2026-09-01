pub(crate) fn profile_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.profile_picker.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let selected = selected_profile(picker);
    let can_replace = crate::ops::profile::can_replace_session(&ctx.state);
    let mut actions = Vec::new();
    if picker.apply_mode {
        actions.push(
            OverlayAction::new(
                "enter",
                "replace",
                Msg::ProfilePickerApply,
                selected.is_some() && can_replace,
            )
            .hint_only()
            .confirm_if(
                picker.pending_apply == Some(picker.selected),
                "again to confirm (replaces panes)",
                theme.status.warning,
                false,
            ),
        );
        actions.push(OverlayAction::new(
            "esc",
            "cancel",
            Msg::CloseProfilePicker,
            true,
        ));
    } else {
        let current = selected.is_some_and(|entry| {
            ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        });
        let running = selected.is_some_and(|entry| {
            matches!(
                picker.running.get(&entry.name),
                Some(crate::session::discovery::DiscoveredSessionStatus::Running { .. })
            )
        });
        actions.push(
            OverlayAction::new(
                "enter",
                if running { "attach" } else { "launch" },
                Msg::SelectProfile(picker.selected),
                selected.is_some() && !current,
            )
            .hint_only()
            .confirm_if(
                picker.pending_open == Some(picker.selected),
                "again to confirm (ends ephemeral)",
                theme.status.warning,
                false,
            ),
        );
        actions.push(OverlayAction::new(
            "ctrl-o",
            "launch as",
            Msg::ProfilePickerOpenAs,
            selected.is_some(),
        ));
        actions.push(
            OverlayAction::new(
                "ctrl-r",
                "replace",
                Msg::ProfilePickerApply,
                selected.is_some() && can_replace,
            )
            .confirm_if(
                picker.pending_apply == Some(picker.selected),
                "again to confirm (replaces panes)",
                theme.status.warning,
                false,
            ),
        );
        actions.push(OverlayAction::new(
            "ctrl-f",
            "default",
            Msg::ProfilePickerSetDefault,
            selected.is_some(),
        ));
        actions.push(OverlayAction::new(
            "ctrl-n",
            "new",
            Msg::ProfilePickerNew,
            true,
        ));
        actions.push(
            OverlayAction::new(
                "ctrl-d",
                "delete",
                Msg::ProfilePickerDelete,
                selected.is_some(),
            )
            .confirm_if(
                picker.pending_delete == Some(picker.selected),
                "again to confirm",
                theme.status.error,
                true,
            ),
        );
    }
    let armed_row = picker
        .pending_delete
        .or(picker.pending_open)
        .or(picker.pending_apply);

    OverlayPalette::new(
        if picker.apply_mode {
            "Replace session with profile"
        } else {
            "Profiles"
        },
        profile_picker_key(),
        Msg::CloseProfilePicker,
        64,
    )
    .entries(profile_picker_entries(ctx, picker))
    .actions(actions)
    .armed_row(armed_row)
    .placeholder("Search profiles...")
    .initial_query(picker.input.text().to_string())
    .preserve_groups(false)
    .selected(Some(picker.selected))
    .empty_text(profile_picker_empty_text(picker))
    .on_query_change(
        ctx.link().callback(|query: std::sync::Arc<str>| {
            Msg::ProfilePickerQueryChanged(query.to_string())
        }),
    )
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
    }))
    .render(ctx)
}

fn profile_picker_entries(
    ctx: &Context<AppRoot>,
    picker: &ProfilePickerState,
) -> Vec<SearchEntry<usize>> {
    let default_name = ctx.state.config.profile.default.as_deref();
    let query = picker.input.text().trim().to_ascii_lowercase();
    picker
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
        .map(|(index, entry)| {
            let mut item = SearchEntry::item(entry.name.clone(), index);
            let status = if ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
            {
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
            item = item.description(picker_description(description));
            item
        })
        .collect()
}

fn profile_picker_empty_text(picker: &ProfilePickerState) -> String {
    let query = picker.input.text().trim().to_ascii_lowercase();
    if picker.entries.is_empty() {
        "No saved profiles - save one first".to_string()
    } else if query.is_empty() {
        "Type to filter profiles".to_string()
    } else {
        format!("No profiles match `{query}`")
    }
}

/// The currently highlighted profile, if it is still on screen after filtering.
fn selected_profile(picker: &ProfilePickerState) -> Option<&crate::config::ProfileEntry> {
    let query = picker.input.text().trim().to_ascii_lowercase();
    picker
        .entries
        .get(picker.selected)
        .filter(|entry| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
}

fn render_ephemeral_session_item(
    item: &SearchItem<usize>,
    label_style: &Style,
    description_style: &Style,
) -> ListItem {
    let label = item.label.as_ref();
    let suffix = label.strip_prefix("ephemeral").unwrap_or_default();
    let label_spans = vec![
        Span::new("ephemeral").style(*label_style),
        Span::new(suffix),
    ];
    let description = item
        .description
        .as_ref()
        .and_then(|desc| desc.right.clone())
        .unwrap_or_default();
    let description = description.strip_prefix("  ").unwrap_or(&description);
    picker_row(label_spans, description.to_string(), *description_style)
}
