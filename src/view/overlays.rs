use std::sync::Arc;

use tui_lipan::Justify::SpaceBetween;
use tui_lipan::prelude::*;
use tui_lipan::utils::color_contrast::readable_text_color;

use crate::input::{Action, CommandBinding};
use crate::state::{
    ProfilePickerState, ScrollbackMatch, ScrollbackSearchState, SessionPickerState,
};
use crate::{HyprmuxApp, Msg};

use super::keys::{
    help_scroll_key, palette_key, profile_picker_key, rename_input_key, rename_workspace_input_key,
    save_profile_key, search_input_key, session_picker_key, theme_picker_key,
};
use super::{
    action_palette_modal, fg_only, modal_scrollbar_config, shared_search_palette, styled_modal,
};

pub(crate) fn help_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let prefix = ctx.state.config.input.prefix.to_string();
    let modifier = ctx.state.config.input.modifier.label();

    // Group bindings by category (first-seen order), so a category with non-contiguous
    // entries in the table still gets a single header - matching the command palette.
    let mut groups: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
    for binding in &crate::input::command_bindings() {
        let row = (
            help_keys(ctx, binding),
            crate::input::command_label(binding.action, &ctx.state),
        );
        match groups
            .iter_mut()
            .find(|(name, _)| *name == binding.category)
        {
            Some((_, rows)) => rows.push(row),
            None => groups.push((binding.category, vec![row])),
        }
    }
    // Workspace digits and mouse gestures aren't in the command table; append them.
    groups.push((
        "Workspaces",
        vec![
            ("1-9".to_string(), "Switch to workspace".to_string()),
            (
                "Shift+1-9".to_string(),
                "Move pane to workspace".to_string(),
            ),
        ],
    ));
    groups.push((
        "Mouse",
        vec![
            ("mod-drag".to_string(), "Move pane (left-drag)".to_string()),
            (
                "mod-right-drag".to_string(),
                "Resize pane from corner".to_string(),
            ),
            ("drag gap".to_string(), "Resize a tiled split".to_string()),
        ],
    ));
    // Copy mode's internal keys aren't discrete actions, so they aren't in the command table.
    groups.push((
        "Copy mode",
        vec![
            ("hjkl / arrows".to_string(), "Move cursor".to_string()),
            (
                "w / b / e".to_string(),
                "Word forward / back / end".to_string(),
            ),
            (
                "W / B / E".to_string(),
                "WORD forward / back / end".to_string(),
            ),
            (
                "0 / ^ / $".to_string(),
                "Line start / first non-blank / end".to_string(),
            ),
            (
                "g / G".to_string(),
                "Top / bottom of scrollback".to_string(),
            ),
            (
                "Ctrl+u / Ctrl+d".to_string(),
                "Half page up / down".to_string(),
            ),
            ("v / Space".to_string(), "Start selection".to_string()),
            ("y / Enter".to_string(), "Copy selection & exit".to_string()),
            ("Esc / q".to_string(), "Exit copy mode".to_string()),
        ],
    ));

    let mut list = VStack::new();
    for (index, (category, rows)) in groups.iter().enumerate() {
        list = list.child(help_section(category, theme, index > 0));
        for (keys, label) in rows {
            list = list.child(help_row(keys, label, theme));
        }
    }

    let body = VStack::new()
        .child(
            Text::new(format!(
                "Prefix keys with {prefix}, or hold {modifier} with any listed key. Scroll for more · Esc closes."
            ))
            .style(theme.muted)
            .overflow(Overflow::Wrap)
            .height(Length::Auto),
        )
        .child(Text::new("").height(Length::Px(1)))
        .child(
            ScrollView::new()
                .children(vec![list.into()])
                .focusable(true)
                .scroll_wheel(true)
                .scrollbar(true)
                .scrollbar_config(modal_scrollbar_config(theme))
                .height(Length::Flex(1))
                .key(help_scroll_key()),
        );

    styled_modal(ctx, "Keybindings", 50)
        .height(Length::Percent(70))
        .padding((1, 1, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseHelp))
        .child(body)
        .into()
}

pub(crate) fn search_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(search) = ctx.state.search.as_ref() else {
        return Text::new("").into();
    };

    action_palette_modal(
        ctx,
        &format!("Search · {} · Tab: scope", search.scope.label()),
    )
    .on_close(ctx.link().callback(|_| Msg::CloseSearch))
    .child(scrollback_search_palette(ctx, search))
    .key(search_input_key())
}

fn scrollback_search_palette(
    ctx: &Context<HyprmuxApp>,
    search: &ScrollbackSearchState,
) -> SearchPalette<usize> {
    let current = search.current;
    let query = search.input.text().trim();
    let entries = search
        .matches
        .iter()
        .enumerate()
        .map(|(index, matched)| {
            SearchEntry::item(search_match_label(matched), index)
                .description(search_match_description(matched))
                .active(index == current)
        })
        .collect::<Vec<_>>();

    let empty_text = if query.is_empty() {
        format!("Type to search scrollback ({})", search.scope.label())
    } else {
        format!("No matches for `{query}`")
    };

    shared_search_palette::<usize>(ctx, Length::Auto, true)
        .entries(entries)
        .placeholder("Search scrollback...")
        .initial_query(query.to_string())
        .preserve_groups(true)
        .initial_selected_item_index(Some(current))
        .sync_selection(true)
        .description_placement(DescriptionPlacement::Right)
        .empty_text(empty_text)
        .input_key_interceptor(scrollback_search_key_interceptor(ctx))
        .on_query_change(
            ctx.link()
                .callback(|query: std::sync::Arc<str>| Msg::SearchQueryChanged(query.to_string())),
        )
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SearchSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SearchActivate(event.item.value)),
        )
}

fn scrollback_search_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSearch)
        } else if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            Some(Msg::SearchCycleScope)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            Some(Msg::SearchNext(false))
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P')) {
            Some(Msg::SearchNext(true))
        } else {
            None
        }
    })
}

fn search_match_label(matched: &ScrollbackMatch) -> String {
    let label = matched.text.trim();
    if label.is_empty() {
        "(blank line)".to_string()
    } else {
        label.to_string()
    }
}

fn search_match_description(matched: &ScrollbackMatch) -> String {
    format!(
        "pane {} · row {} · col {}",
        matched.pane,
        matched.line + 1,
        matched.start_col + 1
    )
}

pub(crate) fn rename_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = Input::bound(&rename.input)
        .placeholder("Pane name, empty clears custom title")
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .on_change(ctx.link().callback(Msg::RenamePaneChanged))
        .on_key(ctx.link().key_handler(|key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseRenamePane)
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(Msg::SubmitRenamePane)
            } else {
                None
            }
        }));

    styled_modal(ctx, &format!("Rename pane {}", rename.target), 56)
        .padding((1, 2, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseRenamePane))
        .child(input.key(rename_input_key()))
        .into()
}

pub(crate) fn rename_workspace_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename_workspace.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = Input::bound(&rename.input)
        .placeholder("Workspace name, empty clears it")
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .on_change(ctx.link().callback(Msg::RenameWorkspaceChanged))
        .on_key(ctx.link().key_handler(|key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseRenameWorkspace)
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(Msg::SubmitRenameWorkspace)
            } else {
                None
            }
        }));

    styled_modal(ctx, &format!("Rename workspace {}", rename.target + 1), 56)
        .padding((1, 2, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseRenameWorkspace))
        .child(input.key(rename_workspace_input_key()))
        .into()
}

pub(crate) fn save_profile_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(prompt) = ctx.state.save_profile_prompt.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = Input::bound(&prompt.input)
        .placeholder("Profile name")
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .on_change(ctx.link().callback(Msg::SaveProfileNameChanged))
        .on_key(ctx.link().key_handler(|key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseSaveProfile)
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(Msg::SubmitSaveProfile)
            } else {
                None
            }
        }));

    styled_modal(ctx, "Save profile", 56)
        .padding((1, 2, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseSaveProfile))
        .child(input.key(save_profile_key()))
        .into()
}

pub(crate) fn profile_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(picker) = ctx.state.profile_picker.as_ref() else {
        return Text::new("").into();
    };

    let palette = profile_picker_palette(ctx, picker);
    let body = VStack::new()
        .child(palette)
        .child(profile_picker_hints(ctx));

    action_palette_modal(ctx, "Profiles")
        .on_close(ctx.link().callback(|_| Msg::CloseProfilePicker))
        .child(body)
        .key(profile_picker_key())
}

fn profile_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let hint = |label: &str, key: &str| -> Element {
        HStack::new()
            .gap(1)
            .width(Length::Auto)
            .child(Text::new(label).style(fg_only(&theme.primary).bold()))
            .child(Text::new(key).style(fg_only(&theme.muted)))
            .into()
    };

    HStack::new()
        .padding((1, 1, 0, 1))
        .justify(Justify::SpaceBetween)
        .gap(2)
        .child(hint("open", "enter"))
        .child(hint("default", "ctrl+f"))
        .child(hint("delete", "ctrl+d"))
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
    let error_bg = theme.status.error;
    let selection_style = if pending_delete.is_some() {
        Style::new()
            .bg(theme.status.error)
            .fg(readable_text_color(None, error_bg))
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    } else {
        Style::new()
            .fg(theme.surface.backdrop)
            .bg(theme.border_active)
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    };

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

fn render_pending_delete_item(item: &SearchItem<usize>, error_bg: Color) -> ListItem {
    let fg = readable_text_color(None, error_bg);
    ListItem::from_spans(vec![
        Span::new(item.label.as_ref()).style(Style::new().fg(fg).strikethrough()),
    ])
    .description("again to confirm")
    .description_style(Style::new().fg(fg).italic())
    .style(Style::new().bg(error_bg).fg(fg))
}

pub(crate) fn session_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Text::new("").into();
    };
    let body = VStack::new()
        .child(session_picker_palette(ctx, picker))
        .child(session_picker_hints(ctx));

    action_palette_modal(ctx, "Sessions")
        .on_close(ctx.link().callback(|_| Msg::CloseSessionPicker))
        .child(body)
        .key(session_picker_key())
}

/// The footer hint row only advertises keys that would actually act on the current state, so a
/// hint never lies: `detach` appears only while attached, `kill`/`open` only for a selectable
/// non-current session, and `new` only once the query is a valid name that isn't already listed.
fn session_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let hint = |label: &str, key: &str| -> Element {
        HStack::new()
            .gap(1)
            .width(Length::Auto)
            .child(Text::new(label).style(fg_only(&theme.primary).bold()))
            .child(Text::new(key).style(fg_only(&theme.muted)))
            .into()
    };

    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Text::new("").into();
    };
    let query = picker.input.text().trim();
    let query_lower = query.to_ascii_lowercase();
    let current = ctx.state.session_name.as_deref();
    let visible = |entry: &crate::session::discovery::DiscoveredSession| {
        query_lower.is_empty() || entry.name.to_ascii_lowercase().contains(&query_lower)
    };
    let selected = picker
        .entries
        .get(picker.selected)
        .filter(|entry| visible(entry));
    // Opening or killing the session you are already attached to is a no-op, so only offer them
    // for some other session.
    let selected_actionable = selected.is_some_and(|entry| current != Some(entry.name.as_str()));
    let can_new = !query.is_empty()
        && crate::session::discovery::valid_session_name(query)
        && !picker.entries.iter().any(|entry| entry.name == query);

    let mut row = HStack::new()
        .padding((1, 1, 0, 1))
        .justify(Justify::Start)
        .gap(3);
    if selected_actionable {
        row = row.child(hint("open", "enter"));
    }
    if can_new {
        row = row.child(hint("new", "ctrl+n"));
    }
    if ctx.state.session_attached {
        row = row.child(hint("detach", "ctrl+d"));
    }
    if selected_actionable {
        row = row.child(hint("kill", "ctrl+k"));
    }
    row = row.child(hint("refresh", "ctrl+r"));
    row.into()
}

fn session_picker_palette(
    ctx: &Context<HyprmuxApp>,
    picker: &SessionPickerState,
) -> SearchPalette<usize> {
    let theme = &ctx.state.theme;
    let query = picker.input.text().trim().to_ascii_lowercase();
    let current = ctx.state.session_name.as_deref();
    let entries = picker
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
        .map(|(index, entry)| {
            let mut label = entry.name.clone();
            if current == Some(entry.name.as_str()) {
                label.push_str("  • current");
            }
            SearchEntry::item(label, index)
                .description(session_description(entry))
                .active(index == picker.selected)
        })
        .collect::<Vec<_>>();
    let empty_text = if picker.entries.is_empty() {
        "No sessions - type a name and press Ctrl+N".to_string()
    } else if query.is_empty() {
        "Type to filter sessions, or enter a new name".to_string()
    } else {
        format!("No sessions match `{query}` - Ctrl+N creates it")
    };

    let pending_kill = picker.pending_kill;
    let error_bg = theme.status.error;
    let selection_style = if pending_kill.is_some() {
        Style::new()
            .bg(theme.status.error)
            .fg(readable_text_color(None, error_bg))
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    } else {
        Style::new()
            .fg(theme.surface.backdrop)
            .bg(theme.border_active)
            .bold()
            .contrast_policy(ContrastPolicy::BlackOrWhite)
    };

    let mut palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search or name session...")
        .initial_query(picker.input.text().to_string())
        .initial_selected_item_index(Some(picker.selected))
        .sync_selection(true)
        .empty_text(empty_text)
        .description_placement(DescriptionPlacement::Right)
        .list_selection_style(selection_style)
        .list_unfocused_selection_style(selection_style)
        .input_key_interceptor(session_picker_key_interceptor(ctx))
        .on_query_change(
            ctx.link()
                .callback(|query: Arc<str>| Msg::SessionPickerQueryChanged(query.to_string())),
        )
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SessionPickerSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SessionPickerActivate(event.item.value)),
        );
    if pending_kill.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            if pending_kill == Some(item.value) {
                Some(render_pending_delete_item(item, error_bg))
            } else {
                None
            }
        }));
    }
    palette
}

fn session_description(entry: &crate::session::discovery::DiscoveredSession) -> ItemDescription {
    use crate::session::discovery::DiscoveredSessionStatus;
    match &entry.status {
        DiscoveredSessionStatus::Running { panes, has_layout } => {
            ItemDescription::new().right(format!(
                "running · panes={panes} · layout={}",
                if *has_layout { "yes" } else { "no" }
            ))
        }
        DiscoveredSessionStatus::Busy => ItemDescription::new().right("busy"),
        DiscoveredSessionStatus::Unknown => ItemDescription::new().right("unknown"),
    }
}

fn session_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSessionPicker)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            Some(Msg::SessionPickerCreateFromQuery)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D')) {
            Some(Msg::SessionPickerDetachCurrent)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K')) {
            Some(Msg::SessionPickerKillSelected)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            Some(Msg::SessionPickerRefresh)
        } else {
            None
        }
    })
}

pub(crate) fn palette_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    // Commands are sourced from the single binding table; only entries flagged for
    // the palette appear here. The help overlay remains the full keybinding reference.
    // Group by category (first-seen order) so each category header appears once even
    // when bindings of the same category are not declared contiguously.
    let mut groups: Vec<(&'static str, Vec<SearchEntry<Action>>)> = Vec::new();
    for binding in crate::input::command_bindings()
        .into_iter()
        .filter(|binding| binding.palette)
    {
        let label = crate::input::command_label(binding.action, &ctx.state);
        let mut entry = SearchEntry::item(label, binding.action);
        let keys = palette_keys(ctx, &binding);
        if !keys.is_empty() {
            entry = entry.description(ItemDescription::new().right(keys));
        }
        match groups
            .iter_mut()
            .find(|(category, _)| *category == binding.category)
        {
            Some((_, items)) => items.push(entry),
            None => groups.push((binding.category, vec![entry])),
        }
    }
    let mut entries: Vec<SearchEntry<Action>> = Vec::new();
    for (category, items) in groups {
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }

    let palette = action_search_palette(ctx, entries, "Search commands…");

    action_palette_modal(ctx, "Commands")
        .on_close(ctx.link().callback(|_| Msg::ClosePalette))
        .child(palette)
        .key(palette_key())
}

fn action_search_palette(
    ctx: &Context<HyprmuxApp>,
    entries: Vec<SearchEntry<Action>>,
    placeholder: &str,
) -> SearchPalette<Action> {
    shared_search_palette::<Action>(ctx, Length::Auto, true)
        .entries(entries)
        .placeholder(placeholder)
        .preserve_groups(true)
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<Action>| Msg::RunAction(event.item.value)),
        )
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

    action_palette_modal(ctx, "Choose theme")
        .on_close(ctx.link().callback(|_| Msg::CloseThemePicker))
        .child(palette)
        .key(theme_picker_key())
}

/// Display keys in the command palette: user overrides or real defaults only.
/// Palette-only commands intentionally do not show a fake key hint here.
fn palette_keys(ctx: &Context<HyprmuxApp>, binding: &CommandBinding) -> String {
    ctx.state
        .config
        .keymap
        .keys_for(binding.action)
        .unwrap_or_else(|| binding.keys.to_string())
}

/// Display keys in the help overlay. Empty active keys render as `not set`.
fn help_keys(ctx: &Context<HyprmuxApp>, binding: &CommandBinding) -> String {
    ctx.state
        .config
        .keymap
        .keys_for(binding.action)
        .unwrap_or_else(|| binding.keys.to_string())
}

fn help_section(title: &str, theme: &Theme, spaced: bool) -> Element {
    // A horizontal rule with the section title on it - the title in the accent color, the
    // line muted - so groups read as clear dividers without competing with the key text.
    let divider = Divider::horizontal()
        .label(
            Text::new(format!(" {} ", title.to_uppercase())).style(fg_only(&theme.accent).bold()),
        )
        .label_alignment(Align::Center)
        .label_padding(1)
        .style(fg_only(&theme.muted))
        .width(Length::Flex(1));
    if spaced {
        VStack::new()
            .child(Text::new("").height(Length::Px(1)))
            .child(divider)
            .into()
    } else {
        divider.into()
    }
}

fn help_row(keys: &str, desc: &str, theme: &Theme) -> Element {
    let (keys_text, keys_style) = if keys.is_empty() {
        ("not set".to_string(), fg_only(&theme.muted))
    } else {
        (
            keys.to_string(),
            Style::new().fg(theme.border_active).bold(),
        )
    };
    HStack::new()
        .gap(2)
        .height(Length::Px(1))
        .justify(SpaceBetween)
        .child(
            Text::new(keys_text)
                .style(keys_style)
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .child(
            Text::new(desc.to_string())
                .style(fg_only(&theme.primary))
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .into()
}
