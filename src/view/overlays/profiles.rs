use super::*;

use crate::view::vocabulary::SessionPresence;

pub(crate) fn profile_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.profile_picker.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let status_styles = crate::view::vocabulary::SessionStatusStyles::from_theme(theme);
    let statuses: Vec<_> = picker
        .entries
        .iter()
        .map(|entry| profile_session_status(ctx, picker, &entry.name))
        .collect();
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
        actions.push(OverlayAction::destructive(
            ctx,
            "delete",
            Msg::ProfilePickerDelete,
            selected.is_some(),
            picker.pending_delete == Some(picker.selected),
        ));
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
    .item_gutter(Arc::new(move |item: &SearchItem<usize>, _hl| {
        // A profile with no session gets the quiet dot, so the column reads as a status column
        // rather than ragged indentation.
        Some(match statuses.get(item.value)? {
            Some(presence) => presence.gutter(status_styles),
            None => crate::view::vocabulary::picker_marker_gutter("·", status_styles.label),
        })
    }))
    .actions(actions)
    .armed_row(armed_row)
    .placeholder("Search profiles…")
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
            let presence = profile_session_status(ctx, picker, &entry.name);
            let description = crate::view::vocabulary::fields([
                if default_name == Some(entry.name.as_str()) {
                    "Default"
                } else {
                    ""
                },
                presence.map_or("", SessionPresence::label),
            ]);
            item = item.description(picker_description(description));
            item
        })
        .collect()
}

/// The standing of the session a profile launches, if one exists.
fn profile_session_status(
    ctx: &Context<AppRoot>,
    picker: &ProfilePickerState,
    name: &str,
) -> Option<SessionPresence> {
    // Profiles launch local sessions. A held one keeps its exact standing, reconnecting or lost
    // included; otherwise discovery says what is there, the same as a Sessions picker row.
    let presence = crate::view::vocabulary::session_presence_for(&ctx.state, name, None);
    if presence.is_held() {
        return Some(presence);
    }
    picker
        .running
        .get(name)
        .map(|status| presence.discovered(status))
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

pub(super) fn render_ephemeral_session_item(
    item: &SearchItem<usize>,
    label_style: &Style,
    description_style: &Style,
) -> ListItem {
    let label = item.label.as_ref();
    let suffix = label
        .strip_prefix(crate::view::vocabulary::EPHEMERAL)
        .unwrap_or_default();
    let label_spans = vec![
        Span::new(crate::view::vocabulary::EPHEMERAL).style(*label_style),
        Span::new(suffix),
    ];
    let description = item
        .description
        .as_ref()
        .and_then(|desc| desc.right.clone())
        .unwrap_or_default();
    picker_row(label_spans, description, *description_style)
}
