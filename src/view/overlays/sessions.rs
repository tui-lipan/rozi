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
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Text::new("").into();
    };
    let entries = shared
        .clients
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let mut markers = Vec::new();
            if client.id == shared.client_id {
                markers.push("you".to_string());
            }
            if Some(client.id) == shared.controller {
                markers.push("controller".to_string());
            }
            if client.read_only {
                markers.push("read-only".to_string());
            }
            if client.requesting_control && Some(client.id) != shared.controller {
                markers.push("wants control".to_string());
            }
            SearchEntry::item(format!("{}  #{}", client.label, client.id), index)
                .description(ItemDescription::new().right(markers.join(" · ")))
        })
        .collect::<Vec<_>>();
    let key = client_list_key();
    let selected = list.selected;
    let client_count = shared.clients.len();
    let can_grant = !shared.read_only && shared.is_controller();
    // Declining only applies to a controller acting on a client with a pending request.
    let selected_requesting = can_grant
        && shared.clients.get(selected).is_some_and(|client| {
            client.requesting_control && Some(client.id) != shared.controller
        });
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
        } else if selected_requesting
            && matches!(key_event.code, KeyCode::Char('d') | KeyCode::Char('D'))
        {
            Some(Msg::ClientListDecline(selected))
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
        let mut hints = hint_row().child(hint_pill(&ctx.state.theme, "grant control", "enter / g"));
        if selected_requesting {
            hints = hints.child(hint_pill(&ctx.state.theme, "decline", "d"));
        }
        body = body.child(hints);
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
    let current = ctx.state.current().session_name.as_deref();
    let visible = |entry: &crate::session::discovery::DiscoveredSession| {
        query_lower.is_empty() || entry.name.to_ascii_lowercase().contains(&query_lower)
    };
    let selected = picker
        .entries
        .get(picker.selected)
        .filter(|entry| visible(entry));
    // Opening (attaching to) the session you are already on is a no-op, so only offer it for some
    // other session. Killing the current session is allowed - it shuts the server down and hops the
    // UI onto a fresh ephemeral - so its hint follows any selection.
    let selected_actionable = selected.is_some_and(|entry| current != Some(entry.name.as_str()));

    let mut row = hint_row();
    if selected_actionable {
        row = row.child(hint_pill(theme, "open", "enter"));
    }
    row = row.child(hint_pill(theme, "new", "ctrl+n"));
    if ctx.state.current().session_attached && !ctx.state.is_ephemeral_session() {
        row = row.child(hint_pill(theme, "detach", "ctrl+d"));
    }
    if ctx.state.current().session_attached && ctx.state.is_ephemeral_session() {
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
    let current_name = ctx.state.current().session_name.as_deref();
    let current_host = ctx.state.current().remote_host.as_deref();
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
            // stay reattachable - activation is by row index, not this label).
            let mut label = if entry.ephemeral {
                "ephemeral".to_string()
            } else {
                entry.name.clone()
            };
            if let Some(host) = entry.host.as_deref() {
                label.push('@');
                label.push_str(host);
            }
            let is_current = current_name == Some(entry.name.as_str())
                && current_host == entry.host.as_deref();
            if is_current {
                label.push_str("  • current");
            }
            SearchEntry::item(label, index).description(session_description(entry, is_current))
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
        DiscoveredSessionStatus::Running {
            panes,
            clients,
            created_from_profile,
            ..
        } => {
            let panes_label = if *panes == 1 {
                "1 pane".to_string()
            } else {
                format!("{panes} panes")
            };
            // A discovery probe is not attached, so every reported client on another session is
            // already there and a new attach will join as a follower. The current row is built
            // locally and includes this UI in its count, so only surface clients besides us.
            let other_clients = clients.saturating_sub(u32::from(is_current));
            let mut label = match other_clients {
                0 => panes_label,
                1 => format!("{panes_label} · 1 other client"),
                count => format!("{panes_label} · {count} other clients"),
            };
            if let Some(profile) = created_from_profile {
                label.push_str(&format!(" · from {profile}"));
            }
            ItemDescription::new().right(label)
        }
        DiscoveredSessionStatus::Busy => ItemDescription::new().right("busy"),
        DiscoveredSessionStatus::Unknown => ItemDescription::new().right("unavailable"),
    }
}

fn session_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    let is_ephemeral = ctx.state.is_ephemeral_session();
    let is_attached = ctx.state.current().session_attached;
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
