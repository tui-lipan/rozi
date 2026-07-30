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

/// A row in the collaborators roster: one other client, plus what may be done to it from here.
#[derive(Clone, Copy, PartialEq)]
struct CollaboratorItem {
    /// Index into the shared roster, which is what the grant/decline/evict ops address.
    roster_index: usize,
    client_id: crate::shared_layout::ClientId,
    position: usize,
    grantable: bool,
    requesting: bool,
    kickable: bool,
}

/// Who else is on the session, and what this client may do about them.
pub(crate) fn collaboration_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(state) = ctx.state.collaboration.as_ref() else {
        return Text::new("").into();
    };
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Text::new("").into();
    };
    let can_evict = crate::ops::session::can_evict(&ctx.state);
    let mut rows: Vec<SearchItem<CollaboratorItem>> = Vec::new();
    let mut items = Vec::new();
    for (roster_index, client) in shared.clients.iter().enumerate() {
        if client.id == shared.client_id {
            continue;
        }
        let position = items.len();
        let item = CollaboratorItem {
            roster_index,
            client_id: client.id,
            position,
            grantable: !shared.read_only
                && shared.is_controller()
                && !client.read_only
                && !client.parked,
            requesting: client.requesting_control,
            kickable: can_evict,
        };
        let mut markers = Vec::new();
        if Some(client.id) == shared.controller {
            markers.push("ctrl");
        }
        if client.read_only {
            markers.push("ro");
        }
        // A parked client is connected but not here: it holds no control and is not competing
        // for the session, which is worth flagging rather than listing it like an active viewer.
        if client.parked {
            markers.push("parked");
        }
        if client.requesting_control && Some(client.id) != shared.controller {
            markers.push("wants ctrl");
        }
        rows.push(
            SearchItem::new(client_tag(&client.label, client.id), item)
                .description(ItemDescription::new().right(markers.join(" · "))),
        );
        items.push(item);
    }
    let entries: Vec<_> = rows.iter().cloned().map(SearchEntry::Item).collect();

    // Rank the roster exactly as the widget does, so "the highlighted client" means the same thing
    // to the footer hints and the Ctrl chords as it does on screen. Without this the selection is an
    // index into the *unfiltered* roster, and a query that hides that row would leave `ctrl+k`
    // pointed at somebody the user can no longer see.
    let visible = rank_search_palette_indices_with_mode(
        &rows,
        &state.query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    );
    let selected = state.selected.min(items.len().saturating_sub(1));
    let selected_item = if visible.contains(&selected) {
        items.get(selected).copied()
    } else {
        // The filter moved the highlight off the recorded row; follow it to the top match.
        visible.first().and_then(|index| items.get(*index).copied())
    };
    let armed = state
        .pending_kick
        .filter(|id| selected_item.is_some_and(|item| item.client_id == *id));
    // An empty list means two different things and must not claim the wrong one: the roster really
    // is empty, or the query hid everyone in it.
    let query = state.query.trim();
    let empty_text = if items.is_empty() {
        "No other clients".to_string()
    } else {
        format!("No client matches `{query}`")
    };
    // The query input owns focus here, so every action key is a Ctrl chord: a bare letter has to
    // reach the filter. Enter is the exception the list already owns.
    let interceptor = ctx.link().key_handler(move |key_event| {
        if key_event.is(KeyCode::Esc) {
            Some(Msg::CloseCollaboration)
        } else if let Some(item) = selected_item.filter(|item| item.grantable && item.requesting)
            && key_event.mods.ctrl
            && matches!(key_event.code, KeyCode::Char('d') | KeyCode::Char('D'))
        {
            Some(Msg::CollaborationDecline(item.roster_index))
        } else if let Some(item) = selected_item.filter(|item| item.kickable)
            && key_event.mods.ctrl
            && matches!(key_event.code, KeyCode::Char('k') | KeyCode::Char('K'))
        {
            Some(Msg::CollaborationKick(item.roster_index))
        } else {
            None
        }
    });
    let error_bg = ctx.state.theme.status.error;
    let mut palette = shared_search_palette::<CollaboratorItem>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("Search other clients…")
        .empty_text(empty_text)
        .initial_query(state.query.clone())
        .initial_selected_item_index(Some(selected))
        .sync_selection(true)
        .description_placement(DescriptionPlacement::Right)
        .input_key_interceptor(interceptor)
        .on_query_change(
            ctx.link()
                .callback(|query: Arc<str>| Msg::CollaborationQueryChanged(query.to_string())),
        )
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<CollaboratorItem>| {
                    Msg::CollaborationSelect(event.item.value.position)
                }),
        )
        .on_activate(ctx.link().callback(
            |event: SearchEvent<CollaboratorItem>| match event.item.value {
                CollaboratorItem {
                    roster_index,
                    grantable: true,
                    ..
                } => Msg::CollaborationGrant(roster_index),
                CollaboratorItem { position, .. } => Msg::CollaborationSelect(position),
            },
        ));
    if let Some(armed) = armed {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<CollaboratorItem>, _hl| {
            (item.value.client_id == armed).then(|| render_pending_kick_item(item, error_bg))
        }));
    }

    let mut body = VStack::new().height(Length::Auto).child(palette);
    if let Some(item) = selected_item {
        let mut hints = hint_row();
        if item.grantable {
            hints = hints.child(hint_pill(&ctx.state.theme, "grant control", "enter"));
        }
        if item.grantable && item.requesting {
            hints = hints.child(hint_pill(&ctx.state.theme, "decline", "ctrl+d"));
        }
        if item.kickable {
            let label = if armed.is_some() { "confirm kick" } else { "kick" };
            hints = hints.child(hint_pill(&ctx.state.theme, label, "ctrl+k"));
        }
        body = body.child(hints);
    }
    // This client's own identity and role ride the top border as a right header rather than a line
    // of prose above the input — see the overlay convention in AGENTS.md.
    let panel: Element = Frame::new()
        .header_left("Manage collaborators")
        .header_right(self_tag(shared))
        .header_style(ctx.state.theme.accent.bold())
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(action_palette_frame(body))
        .into();
    Modal::new()
        .width(Length::Px(64))
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(|_| Msg::CloseCollaboration))
        .child(panel)
        .key(collaboration_key())
}

/// One client as a compact token: `razuer #2077`.
fn client_tag(label: &str, id: crate::shared_layout::ClientId) -> String {
    format!("{label} #{id}")
}

/// This client's own row of the roster, for the dialog's right header: `razuer #2077 · ctrl`.
fn self_tag(shared: &crate::state::SharedSessionState) -> String {
    let label = shared
        .clients
        .iter()
        .find(|client| client.id == shared.client_id)
        .map(|client| client_tag(&client.label, client.id))
        .unwrap_or_else(|| format!("#{}", shared.client_id));
    let role = if shared.read_only {
        "ro"
    } else if shared.is_controller() {
        "ctrl"
    } else {
        "follow"
    };
    format!("{label} · {role}")
}

/// The row of a client awaiting its confirming second press: struck through in the error color,
/// worded exactly like an armed session kill in the session picker.
fn render_pending_kick_item(item: &SearchItem<CollaboratorItem>, error_bg: Color) -> ListItem {
    let fg = readable_text_color(None, error_bg);
    ListItem::from_spans(vec![
        Span::new(item.label.as_ref()).style(Style::new().fg(fg).strikethrough()),
    ])
    .description("again to confirm")
    .description_style(Style::new().fg(fg).italic())
    .style(Style::new().bg(error_bg).fg(fg))
}

/// The choice offered when an attach lands on a session another client is driving.
pub(crate) fn follow_prompt_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(prompt) = ctx.state.follow_prompt.as_ref() else {
        return Text::new("").into();
    };
    let entries = crate::state::FollowChoice::ALL
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            SearchEntry::item(choice.label(prompt.allow_takeover).to_string(), index).description(
                ItemDescription::new().right(choice.description(prompt.allow_takeover).to_string()),
            )
        })
        .collect::<Vec<_>>();
    let selected = prompt.selected;
    let last = crate::state::FollowChoice::ALL.len() - 1;
    let interceptor = ctx.link().key_handler(move |key_event| {
        if key_event.is(KeyCode::Esc) {
            // Backing out of the prompt is itself a choice: cancel, not a silent follow.
            Some(Msg::FollowPromptChoose(last))
        } else if key_event.is(KeyCode::Char('j')) {
            Some(Msg::FollowPromptSelect((selected + 1).min(last)))
        } else if key_event.is(KeyCode::Char('k')) {
            Some(Msg::FollowPromptSelect(selected.saturating_sub(1)))
        } else {
            None
        }
    });
    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("")
        .initial_selected_item_index(Some(selected))
        .sync_selection(true)
        .description_placement(DescriptionPlacement::Right)
        .input_key_interceptor(interceptor)
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::FollowPromptSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::FollowPromptChoose(event.item.value)),
        );
    let title = format!("`{}` in use by {}", prompt.session, prompt.controller_label);
    action_palette(
        ctx,
        &title,
        crate::view::follow_prompt_key(),
        Msg::FollowPromptChoose(last),
        palette,
        64,
    )
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
    row = row.child(hint_pill(theme, "connect host", "ctrl+r"));
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
    // A session retained in the background can have its client attachment closed (the server keeps
    // running); offer it only for such a parked row, never the current session.
    if let Some(entry) = selected
        && current != Some(entry.name.as_str())
        && ctx
            .state
            .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
            .is_some()
    {
        row = row.child(hint_pill(theme, "close", "ctrl+w"));
    }
    // Disconnecting a host closes every attachment to it; offer it when the selected row is a remote
    // session we currently hold at least one attachment to.
    if let Some(target) = selected.and_then(|entry| entry.remote_target.as_ref())
        && (ctx.state.current().remote_target.as_ref() == Some(target)
            || ctx
                .state
                .background
                .values()
                .any(|attachment| attachment.remote_target.as_ref() == Some(target)))
    {
        row = row.child(hint_pill(theme, "disconnect host", "ctrl+x"));
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
    let mut entries = Vec::new();
    let mut last_group: Option<Option<&str>> = None;
    for (index, entry) in picker.entries.iter().enumerate().filter(|(_, entry)| {
        query.is_empty()
            || entry.name.to_ascii_lowercase().contains(&query)
            || entry
                .host
                .as_deref()
                .is_some_and(|host| host.to_ascii_lowercase().contains(&query))
    }) {
        let group = entry.host.as_deref();
        if last_group != Some(group) {
            if last_group.is_some() {
                entries.push(SearchEntry::spacer());
            }
            entries.push(SearchEntry::header(match group {
                Some(host) => format!("REMOTE · {host}"),
                None => "LOCAL".to_string(),
            }));
            last_group = Some(group);
        }
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
            && current_host == entry.host.as_deref()
            && ctx.state.current().remote_target == entry.remote_target;
        let attachment = ctx
            .state
            .attachment_by_identity(&entry.name, entry.remote_target.as_ref());
        let connection = attachment.map(|attachment| attachment.connection);
        // We hold a live client connection to this session (current, or retained in the background).
        let we_hold = attachment.is_some();
        let is_connected = connection == Some(crate::state::ConnectionState::Connected);
        if is_current {
            label.push_str("  • current");
        } else if is_connected {
            // Attached but not on screen: our connection is retained in the background.
            label.push_str("  • background");
        } else if matches!(
            connection,
            Some(
                crate::state::ConnectionState::Connecting
                    | crate::state::ConnectionState::Reconnecting
            )
        ) {
            label.push_str("  • reconnecting");
        } else if connection.is_some() {
            label.push_str("  • offline");
        }
        entries.push(
            SearchEntry::item(label, index)
                .description(session_description(entry, we_hold)),
        );
    }
    let empty_text = if picker.entries.is_empty() {
        "No sessions - press Ctrl+N to create".to_string()
    } else if query.is_empty() {
        "Type to filter sessions, or press Ctrl+N to create".to_string()
    } else {
        format!("No sessions match `{query}` - press Ctrl+N to create")
    };

    let pending_kill = picker.pending_kill;
    let error_bg = theme.status.error;
    let ephemeral_style = fg_only(&theme.primary).italic();
    let description_style = fg_only(&theme.muted);
    let pending_accent = pending_kill.map(|_| error_bg);
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
    if pending_kill.is_some() || !ephemeral_entries.is_empty() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<usize>, _hl| {
            if pending_kill == Some(item.value) {
                Some(render_pending_delete_item(item, error_bg))
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
    we_hold: bool,
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
            // `clients` counts every client attached to the server, ours included. Drop our own
            // connection (current or retained in the background) so this reports only *other* people
            // sharing the session — the ones a new attach would join.
            let others = clients.saturating_sub(u32::from(we_hold));
            let mut label = match others {
                0 => panes_label,
                1 => format!("{panes_label} · shared with 1 other"),
                count => format!("{panes_label} · shared with {count} others"),
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
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W')) {
            Some(Msg::SessionPickerCloseAttachment)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')) {
            Some(Msg::SessionPickerDisconnectHost)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            Some(Msg::SessionPickerConnectHost)
        } else {
            None
        }
    })
}
