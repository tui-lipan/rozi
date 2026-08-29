fn remote_host_connections(
    ctx: &Context<AppRoot>,
    target: &crate::session::remote::RemoteTarget,
) -> Vec<crate::state::ConnectionState> {
    std::iter::once(ctx.state.current())
        .chain(ctx.state.background.values())
        .filter(|attachment| attachment.remote_target.as_ref() == Some(target))
        .map(|attachment| attachment.connection)
        .collect()
}

fn remote_host_description(
    ctx: &Context<AppRoot>,
    entry: &crate::state::HostEntry,
) -> ItemDescription {
    let cached = ctx
        .state
        .host_session_cache
        .get(&entry.target.display_label())
        .map(Vec::len)
        .unwrap_or_default();
    let connections = remote_host_connections(ctx, &entry.target);
    let status = ctx
        .state
        .hosts
        .status_for(&entry.target, connections.iter(), cached > 0);
    let label = match (cached, status, entry.origin) {
        (1, _, _) => "1 session".to_string(),
        (count, _, _) if count > 1 => format!("{count} sessions"),
        (_, crate::state::HostStatus::Reachable, _) => "reached".to_string(),
        (_, crate::state::HostStatus::Connecting, _) => "connecting".to_string(),
        (_, crate::state::HostStatus::Unreachable, _) => "unreachable".to_string(),
        (_, _, crate::state::HostOrigin::Configured) => "configured".to_string(),
        (_, _, crate::state::HostOrigin::Recent) => "recent".to_string(),
        (_, _, crate::state::HostOrigin::Attached) => "attached".to_string(),
    };
    ItemDescription::new().right(label)
}

fn remote_host_marker(
    ctx: &Context<AppRoot>,
    entry: &crate::state::HostEntry,
) -> &'static str {
    let cached = ctx
        .state
        .host_session_cache
        .get(&entry.target.display_label())
        .is_some_and(|sessions| !sessions.is_empty());
    let connections = remote_host_connections(ctx, &entry.target);
    match ctx
        .state
        .hosts
        .status_for(&entry.target, connections.iter(), cached)
    {
        crate::state::HostStatus::Connected | crate::state::HostStatus::Reachable => "●",
        crate::state::HostStatus::Connecting => "◌",
        crate::state::HostStatus::Disconnected | crate::state::HostStatus::Unreachable => "○",
    }
}

fn remote_hosts_palette(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
) -> SearchPalette<crate::session::remote::RemoteTarget> {
    let entries = ctx
        .state
        .hosts
        .iter()
        .map(|entry| {
            SearchEntry::item(
                format!("{} {}", remote_host_marker(ctx, entry), entry.alias),
                entry.target.clone(),
            )
            .description(remote_host_description(ctx, entry))
        })
        .collect::<Vec<_>>();
    let selected = picker.selected_host.as_ref().and_then(|selected| {
        ctx.state
            .hosts
            .iter()
            .position(|entry| &entry.target == selected)
    });
    let interceptor = ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseRemotePicker)
        } else {
            None
        }
    });
    let empty_text = if picker.host_input.text().trim().is_empty() {
        "No known remote hosts".to_string()
    } else {
        format!("No hosts match `{}`", picker.host_input.text().trim())
    };
    shared_search_palette::<crate::session::remote::RemoteTarget>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("Search hosts...")
        .initial_query(picker.host_input.text().to_string())
        .initial_selected_item_index(selected)
        .sync_selection(true)
        .empty_text(empty_text)
        .description_placement(DescriptionPlacement::Right)
        .input_key_interceptor(interceptor)
        .on_query_change(
            ctx.link()
                .callback(|query: Arc<str>| Msg::RemotePickerHostQueryChanged(query.to_string())),
        )
        .on_select(ctx.link().callback(
            |event: SearchEvent<crate::session::remote::RemoteTarget>| {
                Msg::RemotePickerHostSelect(event.item.value.clone())
            },
        ))
        .on_activate(ctx.link().callback(
            |event: SearchEvent<crate::session::remote::RemoteTarget>| {
                Msg::RemotePickerHostActivate(event.item.value.clone())
            },
        ))
}

fn remote_host_sessions_placeholder(ctx: &Context<AppRoot>) -> Element {
    shared_search_palette::<usize>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(Vec::new())
        .placeholder("Search sessions...")
        .empty_text("Discovering sessions...")
        .input_key_interceptor(ctx.link().key_handler(|key| {
            key.is(KeyCode::Esc).then_some(Msg::CloseRemotePicker)
        }))
        .into()
}

pub(crate) fn remote_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.remote_picker.as_ref() else {
        return Text::new("").into();
    };
    let (title, body) = match &picker.mode {
        crate::state::RemotePickerMode::Hosts => {
            let body = VStack::new()
                .height(Length::Auto)
                .child(remote_hosts_palette(ctx, picker))
                .child(
                    hint_row()
                        .child(hint_pill(&ctx.state.theme, "open", "enter")),
                );
            ("Remote hosts".to_string(), body.into())
        }
        crate::state::RemotePickerMode::HostSessions { target } => {
            let body = VStack::new()
                .height(Length::Auto)
                .child(remote_host_sessions_placeholder(ctx));
            (format!("Sessions · {}", target.display_label()), body.into())
        }
    };
    action_palette(
        ctx,
        &title,
        remote_picker_key(),
        Msg::CloseRemotePicker,
        body,
        64,
    )
}
