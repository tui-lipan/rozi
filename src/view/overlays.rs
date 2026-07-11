use std::sync::Arc;

use tui_lipan::Justify::SpaceBetween;
use tui_lipan::prelude::*;
use tui_lipan::utils::color_contrast::readable_text_color;

use crate::input::Action;
use crate::state::{
    AppearanceAction, ProfilePickerState, ScrollbackMatch, ScrollbackSearchState,
    SessionPickerState,
};
use crate::{HyprmuxApp, Msg};

use super::keys::{
    appearance_palette_key, client_list_key, help_scroll_key, palette_key,
    pane_padding_horizontal_key, pane_padding_vertical_key, profile_picker_key, rename_input_key,
    rename_session_input_key, save_profile_key, search_input_key, session_picker_key,
    theme_picker_key,
};
use super::{
    action_palette_frame, action_palette_modal, action_palette_modal_with_width, fg_only,
    modal_scrollbar_config, shared_search_palette, styled_modal,
};

pub(crate) fn help_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let prefix = ctx.state.config.input.prefix.to_string();
    let modifier = ctx.state.config.input.modifier.label();

    // Group commands by category (first-seen order), so a category with non-contiguous
    // entries still gets a single header - matching the command palette. Commands (labels,
    // categories, live keybinding hints) come straight from the registry `commands.rs` builds,
    // so this always reflects the actual active bindings, including `[keys]` overrides and
    // user-defined commands (registered under category "Custom").
    let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for entry in ctx.command_registry().entries() {
        // Workspace digit switches are described generically below rather than as 27 rows.
        if entry.id.as_str().starts_with("workspace.") {
            continue;
        }
        let category = entry.category.as_deref().unwrap_or("Other").to_string();
        let row = (
            entry.keybinding_hint.as_deref().unwrap_or("").to_string(),
            entry.label.to_string(),
        );
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((category, vec![row])),
        }
    }
    // Workspace digits and mouse gestures aren't individual registry entries; append them.
    groups.push((
        "Workspaces".to_string(),
        vec![
            ("1-9".to_string(), "Switch to workspace".to_string()),
            (
                "Shift+1-9".to_string(),
                "Move pane to workspace (follow)".to_string(),
            ),
            (
                "Ctrl+Shift+1-9".to_string(),
                "Move workspace to workspace (follow)".to_string(),
            ),
        ],
    ));
    groups.push((
        "Mouse".to_string(),
        vec![
            ("mod-drag".to_string(), "Move pane (left-drag)".to_string()),
            (
                "mod-right-drag".to_string(),
                "Resize pane from corner".to_string(),
            ),
            ("drag gap".to_string(), "Resize a tiled split".to_string()),
        ],
    ));
    // User `[keys]` commands are already covered by the registry loop above (category
    // "Custom"). Copy mode's internal keys aren't discrete commands, so they aren't registered.
    groups.push((
        "Copy mode".to_string(),
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
    .child(action_palette_frame(scrollback_search_palette(ctx, search)))
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

/// A single `label key` footer hint (e.g. `submit enter`), styled like the palette hint bar.
fn hint_pill(theme: &Theme, label: &str, key: &str) -> Element {
    HStack::new()
        .gap(1)
        .width(Length::Auto)
        .height(Length::Auto)
        .child(Text::new(label).style(fg_only(&theme.primary).bold()))
        .child(Text::new(key).style(fg_only(&theme.muted)))
        .into()
}

/// The base footer row shared by every overlay hint bar: content-height with a leading gap above
/// it. Callers add [`hint_pill`] children and may override justify/gap.
fn hint_row() -> HStack {
    HStack::new()
        .height(Length::Auto)
        .padding((1, 1, 0, 1))
        .justify(Justify::Start)
        .gap(3)
}

/// Footer hints shared by the single-input prompt overlays (rename pane/workspace/session, save
/// profile) so they read like the command palette instead of a bare dialog.
fn prompt_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    hint_row()
        .child(hint_pill(theme, "submit", "enter"))
        .child(hint_pill(theme, "cancel", "esc"))
        .into()
}

/// Shared chrome for the single-input prompt overlays so they all read like the command palette:
/// palette placement/border, no inner input border, a leading gap, and a submit/cancel hint footer.
/// Callers supply only what differs (title, placeholder, bound state, focus key, and messages).
///
/// `confirm` arms an in-modal destructive warning: when `Some(note)` the modal border and title turn
/// the error color and `note` renders as an inline caption above the hints, so a re-submit
/// confirmation reads off the modal itself instead of a separate toast.
#[allow(clippy::too_many_arguments)]
fn prompt_overlay(
    ctx: &Context<HyprmuxApp>,
    title: &str,
    placeholder: &str,
    input_state: &TextInput,
    input_key: &'static str,
    on_change: impl Fn(InputEvent) -> Msg + 'static,
    close: Msg,
    submit: Msg,
    confirm: Option<&str>,
) -> Element {
    let theme = &ctx.state.theme;
    let close_on_key = close.clone();
    let input = Input::bound(input_state)
        .placeholder(placeholder)
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .border(false)
        .padding((0, 1))
        .on_change(ctx.link().callback(on_change))
        .on_key(ctx.link().key_handler(move |key| {
            if key.is(KeyCode::Esc) {
                Some(close_on_key.clone())
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(submit.clone())
            } else {
                None
            }
        }));

    let mut body = VStack::new()
        .height(Length::Auto)
        .padding((1, 0, 0, 0))
        .child(input.key(input_key));
    if let Some(note) = confirm {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(note)
                        .overflow(Overflow::Wrap)
                        .style(Style::new().fg(theme.status.error).italic()),
                ),
        );
    }
    body = body.child(prompt_hints(ctx));

    let mut modal = action_palette_modal(ctx, title)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(body));
    if confirm.is_some() {
        // Recolor the shared modal chrome to the error accent so the whole dialog reads as "armed"
        // (border + title), matching the inline caption. The modal captures focus the moment it
        // opens, so `focus_style` must repeat the accent or the theme focus role repaints the border.
        let armed_frame = Style::new()
            .bg(theme.surface.element)
            .fg(theme.status.error);
        modal = modal
            .frame_style(armed_frame)
            .focus_style(armed_frame)
            .title_style(Style::new().fg(theme.status.error).bold());
    }
    modal.into()
}

pub(crate) fn rename_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename.as_ref() else {
        return Text::new("").into();
    };
    prompt_overlay(
        ctx,
        &format!("Rename pane {}", rename.target),
        "Pane name, empty clears custom title",
        &rename.input,
        rename_input_key(),
        Msg::RenamePaneChanged,
        Msg::CloseRenamePane,
        Msg::SubmitRenamePane,
        None,
    )
}

pub(crate) fn rename_session_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename_session.as_ref() else {
        return Text::new("").into();
    };
    let (title, placeholder) = match rename.mode {
        crate::state::NamingMode::CreateSession => {
            ("Create session".to_string(), "Session name".to_string())
        }
        // The same ephemeral-naming prompt serves in-place naming and detach-and-name; the latter
        // keeps the server running for reattach, so it names the action to make that clear.
        crate::state::NamingMode::NameEphemeralSession if rename.detach_after => (
            "Detach session".to_string(),
            "Name to keep it running".to_string(),
        ),
        crate::state::NamingMode::NameEphemeralSession => {
            ("Name session".to_string(), "Session name".to_string())
        }
        crate::state::NamingMode::RenameSession => {
            ("Rename session".to_string(), "Session name".to_string())
        }
        crate::state::NamingMode::RenameWorkspace { index } => (
            format!("Rename workspace {}", index + 1),
            "Workspace name, empty clears it".to_string(),
        ),
    };
    // A create-session prompt that would discard the current disposable ephemeral session shows an
    // in-modal warning once armed (see `SessionRenameState::pending_confirm`).
    let confirm = rename
        .pending_confirm
        .then_some("again to confirm (ends ephemeral session)");
    prompt_overlay(
        ctx,
        &title,
        &placeholder,
        &rename.input,
        rename_session_input_key(),
        Msg::RenameSessionChanged,
        Msg::CloseRenameSession,
        Msg::SubmitRenameSession,
        confirm,
    )
}

pub(crate) fn save_profile_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(prompt) = ctx.state.save_profile_prompt.as_ref() else {
        return Text::new("").into();
    };
    prompt_overlay(
        ctx,
        "Save profile",
        "Profile name",
        &prompt.input,
        save_profile_key(),
        Msg::SaveProfileNameChanged,
        Msg::CloseSaveProfile,
        Msg::SubmitSaveProfile,
        None,
    )
}

/// Assemble a palette-style overlay: shared modal chrome, a borderless frame, a close handler, and
/// the overlay's focus key. `content` is the palette itself, or a body wrapping a palette plus a
/// hint footer.
fn action_palette(
    ctx: &Context<HyprmuxApp>,
    title: &str,
    key: &'static str,
    close: Msg,
    content: impl Into<Element>,
    width: u16,
) -> Element {
    action_palette_modal_with_width(ctx, title, width)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(content))
        .key(key)
}

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
    let error_bg = theme.status.error;
    let selection_style =
        picker_selection_style(theme, pending_delete.is_some().then_some(error_bg));

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
/// strikes the row through, since the row itself is going away), the target survives — the cost is
/// to the *current* ephemeral session — so it reads as a warning-colored highlight whose hint spells
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

pub(crate) fn session_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Text::new("").into();
    };
    let body = VStack::new()
        .height(Length::Auto)
        .child(session_picker_palette(ctx, picker))
        .child(session_picker_hints(ctx));

    action_palette(
        ctx,
        "Sessions",
        session_picker_key(),
        Msg::CloseSessionPicker,
        body,
        64,
    )
}

pub(crate) fn client_list_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(list) = ctx.state.client_list.as_ref() else {
        return Text::new("").into();
    };
    let Some(shared) = ctx.state.shared.as_ref() else {
        return Text::new("").into();
    };
    let entries = shared
        .clients
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let mut markers = Vec::new();
            if client.id == shared.client_id {
                markers.push("you");
            }
            if Some(client.id) == shared.controller {
                markers.push("controller");
            }
            if client.read_only {
                markers.push("read-only");
            }
            SearchEntry::item(format!("{}  #{}", client.label, client.id), index)
                .description(ItemDescription::new().right(markers.join(" · ")))
        })
        .collect::<Vec<_>>();
    let key = client_list_key();
    let selected = list.selected;
    let client_count = shared.clients.len();
    let can_grant = !shared.read_only && shared.is_controller();
    let interceptor = ctx.link().key_handler(move |key_event| {
        if key_event.is(KeyCode::Esc) {
            Some(Msg::CloseClientList)
        } else if key_event.is(KeyCode::Char('j')) {
            Some(Msg::ClientListSelect(
                (selected + 1).min(client_count.saturating_sub(1)),
            ))
        } else if key_event.is(KeyCode::Char('k')) {
            Some(Msg::ClientListSelect(selected.saturating_sub(1)))
        } else if can_grant && matches!(key_event.code, KeyCode::Char('g') | KeyCode::Char('G')) {
            Some(Msg::ClientListGrant(selected))
        } else {
            None
        }
    });
    let mut palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("")
        .initial_selected_item_index(Some(list.selected))
        .sync_selection(true)
        .description_placement(DescriptionPlacement::Right)
        .input_key_interceptor(interceptor)
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::ClientListSelect(event.item.value)),
        );
    if can_grant {
        palette = palette.on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::ClientListGrant(event.item.value)),
        );
    }
    let mut body = VStack::new().height(Length::Auto).child(palette);
    if can_grant {
        body =
            body.child(hint_row().child(hint_pill(&ctx.state.theme, "grant control", "enter / g")));
    }
    action_palette(ctx, "Session clients", key, Msg::CloseClientList, body, 64)
}

/// The footer hint row only advertises keys that would actually act on the current state, so a
/// hint never lies: `detach` appears only for an attached named session, `kill`/`reset` for a
/// selectable session, and `open` only for a selectable non-current session.
fn session_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
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
    // Opening (attaching to) the session you are already on is a no-op, so only offer it for some
    // other session. Killing the current session is allowed — it shuts the server down and hops the
    // UI onto a fresh ephemeral — so its hint follows any selection.
    let selected_actionable = selected.is_some_and(|entry| current != Some(entry.name.as_str()));

    let mut row = hint_row();
    if selected_actionable {
        row = row.child(hint_pill(theme, "open", "enter"));
    }
    row = row.child(hint_pill(theme, "new", "ctrl+n"));
    if ctx.state.session_attached && !ctx.state.is_ephemeral_session() {
        row = row.child(hint_pill(theme, "detach", "ctrl+d"));
    }
    if ctx.state.session_attached && ctx.state.is_ephemeral_session() {
        row = row.child(hint_pill(theme, "name current", "ctrl+s"));
    }
    if let Some(entry) = selected {
        let label = if entry.ephemeral { "reset" } else { "kill" };
        row = row.child(hint_pill(theme, label, "ctrl+k"));
    }
    row.into()
}

fn session_picker_palette(
    ctx: &Context<HyprmuxApp>,
    picker: &SessionPickerState,
) -> SearchPalette<usize> {
    let theme = &ctx.state.theme;
    let query = picker.input.text().trim().to_ascii_lowercase();
    let current = ctx.state.session_name.as_deref();
    let ephemeral_entries = picker
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.ephemeral.then_some(index))
        .collect::<Vec<_>>();
    let entries = picker
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
        .map(|(index, entry)| {
            // Ephemeral sessions carry an ugly generated `eph-<pid>` name shown as "ephemeral" (they
            // stay reattachable — activation is by row index, not this label).
            let mut label = if entry.ephemeral {
                "ephemeral".to_string()
            } else {
                entry.name.clone()
            };
            if current == Some(entry.name.as_str()) {
                label.push_str("  • current");
            }
            SearchEntry::item(label, index).description(session_description(
                entry,
                current == Some(entry.name.as_str()),
            ))
        })
        .collect::<Vec<_>>();
    let empty_text = if picker.entries.is_empty() {
        "No sessions - press Ctrl+N to create".to_string()
    } else if query.is_empty() {
        "Type to filter sessions, or press Ctrl+N to create".to_string()
    } else {
        format!("No sessions match `{query}` - press Ctrl+N to create")
    };

    let pending_kill = picker.pending_kill;
    let pending_open = picker.pending_open;
    let error_bg = theme.status.error;
    let warn_bg = theme.status.warning;
    let ephemeral_style = fg_only(&theme.primary).italic();
    let description_style = fg_only(&theme.muted);
    // A kill and an open are never armed at once; the kill's error red takes precedence over the
    // open's warning tint if both were ever present.
    let pending_accent = pending_kill
        .map(|_| error_bg)
        .or_else(|| pending_open.map(|_| warn_bg));
    let selection_style = picker_selection_style(theme, pending_accent);

    let mut palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("Search sessions...")
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
    if pending_kill.is_some() || pending_open.is_some() || !ephemeral_entries.is_empty() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            if pending_kill == Some(item.value) {
                Some(render_pending_delete_item(item, error_bg))
            } else if pending_open == Some(item.value) {
                Some(render_pending_open_item(item, warn_bg))
            } else if ephemeral_entries.contains(&item.value) {
                Some(render_ephemeral_session_item(
                    item,
                    &ephemeral_style,
                    &description_style,
                ))
            } else {
                None
            }
        }));
    }
    palette
}

fn session_description(
    entry: &crate::session::discovery::DiscoveredSession,
    is_current: bool,
) -> ItemDescription {
    use crate::session::discovery::DiscoveredSessionStatus;
    match &entry.status {
        DiscoveredSessionStatus::Running { panes, clients, .. } => {
            let panes_label = if *panes == 1 {
                "1 pane".to_string()
            } else {
                format!("{panes} panes")
            };
            // A discovery probe is not attached, so every reported client on another session is
            // already there and a new attach will join as a follower. The current row is built
            // locally and includes this UI in its count, so only surface clients besides us.
            let other_clients = clients.saturating_sub(u32::from(is_current));
            let label = match other_clients {
                0 => panes_label,
                1 => format!("{panes_label} · 1 other client"),
                count => format!("{panes_label} · {count} other clients"),
            };
            ItemDescription::new().right(label)
        }
        DiscoveredSessionStatus::Busy => ItemDescription::new().right("busy"),
        DiscoveredSessionStatus::Unknown => ItemDescription::new().right("unavailable"),
    }
}

fn session_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    let is_ephemeral = ctx.state.is_ephemeral_session();
    let is_attached = ctx.state.session_attached;
    ctx.link().key_handler(move |key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSessionPicker)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            Some(Msg::SessionPickerCreateFromQuery)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D')) {
            if !is_ephemeral {
                Some(Msg::SessionPickerDetachCurrent)
            } else {
                None
            }
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')) {
            if is_attached && is_ephemeral {
                Some(Msg::SessionPickerNameCurrent)
            } else {
                None
            }
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K')) {
            Some(Msg::SessionPickerKillSelected)
        } else {
            None
        }
    })
}

pub(crate) fn palette_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    // Commands (labels, categories, live keybinding hints, and the handler to run) come
    // straight from the registry `commands.rs` builds. Only palette-eligible ids appear here
    // (see `commands::is_palette_eligible`); the help overlay remains the full reference,
    // including frequent single-key actions this intentionally omits. Group by category
    // (first-seen order) so each category header appears once even when entries of the same
    // category aren't registered contiguously.
    let mut groups: Vec<(String, Vec<SearchEntry<Callback<()>>>)> = Vec::new();
    for entry in ctx.command_registry().entries() {
        if !crate::commands::is_palette_eligible(entry.id.as_str())
            || Action::from_id(entry.id.as_str())
                .is_some_and(|action| !crate::commands::command_available(action, &ctx.state))
        {
            continue;
        }
        let category = entry.category.as_deref().unwrap_or("Other").to_string();
        let mut item = SearchEntry::Item(
            SearchItem::new(entry.label.to_string(), entry.handler.clone())
                .aliases(command_palette_aliases(entry.id.as_str())),
        );
        let hint = entry.keybinding_hint.as_deref().unwrap_or("");
        if !hint.is_empty() {
            item = item.description(ItemDescription::new().right(hint.to_string()));
        }
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, items)) => items.push(item),
            None => groups.push((category, vec![item])),
        }
    }

    let mut entries: Vec<SearchEntry<Callback<()>>> = Vec::new();
    for (category, items) in groups {
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }
    let palette = action_search_palette(ctx, entries, "Search commands…");

    action_palette(
        ctx,
        "Commands",
        palette_key(),
        Msg::ClosePalette,
        palette,
        60,
    )
}

fn command_palette_aliases(id: &str) -> Vec<Arc<str>> {
    match id {
        "change-appearance" => alias_list(&[
            "theme",
            "themes",
            "appearance",
            "style",
            "chrome",
            "padding",
            "terminal padding",
            "border",
            "borders",
            "border merge",
            "titlebar",
            "titlebars",
            "titlebar style",
            "workbar",
            "top bar",
            "workbar gap",
            "workbar position",
            "workbar style",
            "badge",
            "badges",
            "workbar badge",
            "powerline",
            "tab",
            "tabs",
            "workbar tab",
            "animations",
            "motion",
            "transitions",
            "focused border",
            "focused background",
        ]),
        _ => Vec::new(),
    }
}

fn appearance_palette_aliases(action: AppearanceAction) -> Vec<Arc<str>> {
    match action {
        AppearanceAction::Theme => alias_list(&[
            "theme",
            "themes",
            "color",
            "colors",
            "colour",
            "colours",
            "scheme",
        ]),
        AppearanceAction::EditPadding => alias_list(&[
            "padding",
            "margin",
            "margins",
            "inset",
            "insets",
            "terminal",
            "pane",
        ]),
        AppearanceAction::ToggleTitles => alias_list(&[
            "titlebar",
            "titlebars",
            "title",
            "titles",
            "title bar",
            "show titles",
        ]),
        AppearanceAction::CycleTitleStyle => alias_list(&[
            "titlebar style",
            "title style",
            "titlebar caps",
            "title caps",
            "pill",
            "round",
            "arrow",
            "half",
            "padded",
            "cap style",
        ]),
        AppearanceAction::ToggleWorkbar => alias_list(&[
            "workbar",
            "top bar",
            "status bar",
            "bar",
            "show workbar",
        ]),
        AppearanceAction::ToggleWorkbarGap => alias_list(&[
            "workbar gap",
            "gap",
            "spacing",
            "gutter",
            "separator",
        ]),
        AppearanceAction::ToggleWorkbarPosition => alias_list(&[
            "workbar position",
            "position",
            "placement",
            "top",
            "bottom",
            "relocate",
        ]),
        AppearanceAction::CycleWorkbarStyle => alias_list(&[
            "workbar style",
            "workbar caps",
            "bar style",
            "bar caps",
        ]),
        AppearanceAction::CycleWorkbarBadgeStyle => alias_list(&[
            "badge",
            "badges",
            "badge style",
            "workbar badge",
            "chip",
            "chips",
            "mode chip",
            "powerline badge",
        ]),
        AppearanceAction::ToggleWorkbarPowerline => alias_list(&[
            "powerline",
            "chain",
            "chained",
            "interlock",
            "badge chain",
        ]),
        AppearanceAction::CycleWorkbarTabStyle => alias_list(&[
            "tab",
            "tabs",
            "workspace tabs",
            "tab style",
            "workbar tabs",
        ]),
        AppearanceAction::ToggleAnimations => alias_list(&[
            "animation",
            "animations",
            "motion",
            "transitions",
            "animate",
        ]),
        AppearanceAction::ToggleHighlightFocusedBackground => alias_list(&[
            "focused background",
            "focus background",
            "highlight background",
            "active background",
            "focused pane",
        ]),
        AppearanceAction::ToggleHighlightFocusedBorder => alias_list(&[
            "focused border",
            "focus border",
            "highlight border",
            "active border",
        ]),
        AppearanceAction::ToggleBorderMerge => alias_list(&[
            "border merge",
            "merge borders",
            "merging",
            "seam",
            "border seam",
        ]),
        AppearanceAction::CycleBorderStyle => alias_list(&[
            "border style",
            "rounded",
            "square",
            "border caps",
        ]),
    }
}

fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}

pub(crate) fn appearance_overlay(ctx: &Context<HyprmuxApp>) -> Element {
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

    action_palette(
        ctx,
        "Change appearance",
        appearance_palette_key(),
        Msg::CloseAppearance,
        palette,
        60,
    )
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
    SearchEntry::Item(
        SearchItem::new(label, action).aliases(appearance_palette_aliases(action)),
    )
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
        .preserve_groups(true)
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

#[cfg(test)]
mod palette_alias_tests {
    use super::{appearance_palette_aliases, command_palette_aliases};
    use crate::state::AppearanceAction;

    #[test]
    fn every_appearance_action_has_search_aliases() {
        let actions = [
            AppearanceAction::Theme,
            AppearanceAction::EditPadding,
            AppearanceAction::ToggleTitles,
            AppearanceAction::CycleTitleStyle,
            AppearanceAction::ToggleWorkbar,
            AppearanceAction::ToggleWorkbarGap,
            AppearanceAction::ToggleWorkbarPosition,
            AppearanceAction::CycleWorkbarStyle,
            AppearanceAction::CycleWorkbarBadgeStyle,
            AppearanceAction::ToggleWorkbarPowerline,
            AppearanceAction::CycleWorkbarTabStyle,
            AppearanceAction::ToggleAnimations,
            AppearanceAction::ToggleHighlightFocusedBackground,
            AppearanceAction::ToggleHighlightFocusedBorder,
            AppearanceAction::ToggleBorderMerge,
            AppearanceAction::CycleBorderStyle,
        ];
        for action in actions {
            assert!(
                !appearance_palette_aliases(action).is_empty(),
                "missing aliases for {action:?}"
            );
        }
    }

    #[test]
    fn change_appearance_command_aliases_cover_recent_controls() {
        let aliases = command_palette_aliases("change-appearance");
        for term in [
            "padding",
            "powerline",
            "workbar badge",
            "workbar tab",
            "workbar style",
            "titlebar style",
        ] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == term),
                "missing change-appearance alias: {term}"
            );
        }
    }
}
