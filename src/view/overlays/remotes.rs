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

fn remote_host_status(
    ctx: &Context<AppRoot>,
    target: &crate::session::remote::RemoteTarget,
) -> crate::state::HostStatus {
    let connections = remote_host_connections(ctx, target);
    ctx.state
        .hosts
        .status_for(target, connections.iter(), false)
}

fn remote_host_description(
    ctx: &Context<AppRoot>,
    entry: &crate::state::HostEntry,
) -> ItemDescription {
    let cached = crate::session::host_sessions_for(
        &ctx.state.host_session_cache,
        &entry.target,
    )
    .map(|sessions| sessions.len())
    .unwrap_or_default();
    let status = remote_host_status(ctx, &entry.target);
    // Session counts win where they exist and the status is settled — "2 sessions" says more about
    // a reachable host than "reached" does. The status words themselves come from the one shared
    // vocabulary, so this row and the sidebar's badge never call the same state two things.
    let label = match (cached, status, entry.origin) {
        (
            _,
            crate::state::HostStatus::Connecting | crate::state::HostStatus::Unreachable,
            _,
        ) => crate::view::session_status::host_status_label(status).to_string(),
        (1, _, _) => "1 session".to_string(),
        (count, _, _) if count > 1 => format!("{count} sessions"),
        (_, crate::state::HostStatus::Connected | crate::state::HostStatus::Reachable, _) => {
            crate::view::session_status::host_status_label(status).to_string()
        }
        (_, _, crate::state::HostOrigin::Configured) => "configured".to_string(),
        (_, _, crate::state::HostOrigin::Recent) => "recent".to_string(),
        (_, _, crate::state::HostOrigin::Attached) => "attached".to_string(),
    };
    ItemDescription::new().right(label)
}

fn remote_host_label(
    ctx: &Context<AppRoot>,
    alias: &str,
    target: &crate::session::remote::RemoteTarget,
) -> String {
    let duplicate_label = ctx
        .state
        .hosts
        .iter()
        .filter(|candidate| candidate.alias == alias)
        .count()
        > 1;
    if duplicate_label {
        format!("{alias} ({})", target.to_spec())
    } else {
        alias.to_string()
    }
}

fn remote_hosts_palette(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
) -> SearchPalette<crate::session::remote::RemoteTarget> {
    let connecting = matches!(picker.host_probe, crate::state::HostProbe::InFlight);
    let connecting_target = connecting
        .then(|| picker.selected_host.clone())
        .flatten();
    let selected_entry = remote_selected_host(ctx, picker);
    let can_forget = !connecting
        && selected_entry.is_some_and(|entry| {
            crate::ops::session::remotes::host_can_forget(&ctx.state, &entry.target)
        });
    let mut entries = ctx
        .state
        .hosts
        .iter()
        .map(|entry| {
            SearchEntry::item(
                remote_host_label(ctx, &entry.alias, &entry.target),
                entry.target.clone(),
            )
            .description(remote_host_description(ctx, entry))
        })
        .collect::<Vec<_>>();
    if let Some(target) = connecting_target.as_ref()
        && ctx.state.hosts.get(target).is_none()
    {
        let alias = target.display_label();
        entries.push(
            SearchEntry::item(remote_host_label(ctx, &alias, target), target.clone()).description(
                ItemDescription::new().right(crate::view::session_status::host_status_label(
                    crate::state::HostStatus::Connecting,
                )),
            ),
        );
    }
    let selected = picker.selected_host.as_ref().and_then(|selected| {
        entries.iter().enumerate().find_map(|(index, entry)| {
            matches!(entry, SearchEntry::Item(item) if &item.value == selected).then_some(index)
        })
    });
    let interceptor = ctx.link().key_handler({
        let connecting_target = connecting_target.clone();
        move |key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseRemotePicker)
            } else if connecting {
                connecting_target
                    .clone()
                    .map(Msg::RemotePickerHostSelect)
            } else if ctrl_letter(&key, 'n') {
                Some(Msg::RemotePickerNewHost)
            } else if can_forget && ctrl_letter(&key, 'k') {
                Some(Msg::RemotePickerForgetHost)
            } else {
                None
            }
        }
    });
    let empty_text = if picker.host_input.text().trim().is_empty() {
        "No known remote hosts".to_string()
    } else {
        format!("No hosts match `{}`", picker.host_input.text().trim())
    };
    let pending_forget = picker.pending_forget.clone();
    let error_bg = ctx.state.theme.status.error;
    let selection_style = picker_selection_style(
        &ctx.state.theme,
        pending_forget.is_some().then_some(error_bg),
    );
    let mut palette =
        shared_search_palette::<crate::session::remote::RemoteTarget>(ctx, Length::Auto, false)
            .width(Length::Flex(1))
            .entries(entries)
            .placeholder("Search hosts...")
            .initial_query(picker.host_input.text().to_string())
            .initial_selected_item_index(selected)
            .sync_selection(true)
            .empty_text(empty_text)
            .description_placement(DescriptionPlacement::Right)
            .list_selection_style(selection_style)
            .list_unfocused_selection_style(selection_style)
            .input_key_interceptor(interceptor)
            .item_gutter(Arc::new({
                let connecting_target = connecting_target.clone();
                let styles = crate::view::session_status::HostStatusStyles::from_theme(&ctx.state.theme);
                let statuses: Vec<(
                    crate::session::remote::RemoteTarget,
                    crate::state::HostStatus,
                )> = ctx
                    .state
                    .hosts
                    .iter()
                    .map(|entry| (entry.target.clone(), remote_host_status(ctx, &entry.target)))
                    .collect();
                move |item: &SearchItem<crate::session::remote::RemoteTarget>, _hl| {
                    let status = if connecting_target.as_ref() == Some(&item.value) {
                        crate::state::HostStatus::Connecting
                    } else {
                        statuses
                            .iter()
                            .find(|(target, _)| target == &item.value)
                            .map(|(_, status)| *status)?
                    };
                    Some(crate::view::session_status::host_status_gutter(status, styles))
                }
            }))
            .on_query_change(
                ctx.link().callback(|query: Arc<str>| {
                    Msg::RemotePickerHostQueryChanged(query.to_string())
                }),
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
            ));
    if let Some(pending) = pending_forget {
        palette = palette.render_item(Arc::new(
            move |item: &SearchItem<crate::session::remote::RemoteTarget>, _highlighted| {
                (item.value == pending).then(|| {
                    render_pending_confirm_item(
                        item.label.as_ref(),
                        error_bg,
                        "again to forget",
                        true,
                    )
                })
            },
        ));
    }
    palette
}

fn remote_selected_host<'a>(
    ctx: &'a Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
) -> Option<&'a crate::state::HostEntry> {
    let selected = picker.selected_host.as_ref()?;
    let query = picker.host_input.text().trim().to_ascii_lowercase();
    ctx.state.hosts.get(selected).filter(|entry| {
        query.is_empty()
            || entry.alias.to_ascii_lowercase().contains(&query)
            || entry.target.to_spec().to_ascii_lowercase().contains(&query)
    })
}

fn remote_host_sessions_palette(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
    target: &crate::session::remote::RemoteTarget,
) -> SearchPalette<RemoteSessionIdentity> {
    let entries = picker
        .sessions
        .iter()
        .filter_map(|session| {
            let identity = RemoteSessionIdentity::of(session)?;
            let label = if session.ephemeral {
                "ephemeral".to_string()
            } else {
                session.name.clone()
            };
            // Our own connection is one of the clients the remote server counts, and rozi keeps a
            // session it opened attached in the background. Without discounting it, every session
            // this client has ever opened reports itself as shared with a stranger.
            let we_hold = ctx
                .state
                .attachment_by_identity(&session.name, Some(target))
                .is_some();
            Some(
                SearchEntry::item(label, identity)
                    .description(session_description(session, we_hold)),
            )
        })
        .collect::<Vec<_>>();
    let selected = picker.selected_session.as_ref().and_then(|selected| {
        picker
            .sessions
            .iter()
            .filter_map(RemoteSessionIdentity::of)
            .position(|identity| &identity == selected)
    });
    let empty_text = if picker.session_input.text().trim().is_empty() {
        "No sessions".to_string()
    } else {
        format!(
            "No sessions match `{}`",
            picker.session_input.text().trim()
        )
    };
    let pending_kill = picker.pending_kill.clone();
    let pending_restart = picker.pending_restart.clone();
    let error_bg = ctx.state.theme.status.error;
    let warning_bg = ctx.state.theme.status.warning;
    let selection_style = picker_selection_style(
        &ctx.state.theme,
        pending_kill
            .is_some()
            .then_some(error_bg)
            .or_else(|| pending_restart.is_some().then_some(warning_bg)),
    );
    let selected_session = remote_selected_session(picker).cloned();
    let can_restart = selected_session
        .as_ref()
        .is_some_and(crate::ops::session::session_row_can_restart);
    let can_disconnect = selected_session.as_ref().is_some_and(|session| {
        crate::ops::session::session_row_can_disconnect(&ctx.state, session)
    });
    let can_disconnect_host = !remote_host_connections(ctx, target).is_empty();
    let interceptor = ctx.link().key_handler(move |key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseRemotePicker)
        } else if ctrl_letter(&key, 'n') {
            Some(Msg::RemotePickerCreateSession)
        } else if ctrl_letter(&key, 't') {
            Some(Msg::RemotePickerEphemeral)
        } else if selected_session.is_some() && ctrl_letter(&key, 'k') {
            Some(Msg::RemotePickerKillSession)
        } else if can_restart && ctrl_letter(&key, 'e') {
            Some(Msg::RemotePickerRestartSession)
        } else if can_disconnect && ctrl_letter(&key, 'w') {
            Some(Msg::RemotePickerDisconnectSession)
        } else if can_disconnect_host && ctrl_letter(&key, 'x') {
            Some(Msg::RemotePickerDisconnectHost)
        } else {
            None
        }
    });
    let mut palette = shared_search_palette::<RemoteSessionIdentity>(ctx, Length::Auto, false)
        .width(Length::Flex(1))
        .entries(entries)
        .placeholder("Search sessions...")
        .initial_query(picker.session_input.text().to_string())
        .initial_selected_item_index(selected)
        .sync_selection(true)
        .empty_text(empty_text)
        .description_placement(DescriptionPlacement::Right)
        .list_selection_style(selection_style)
        .list_unfocused_selection_style(selection_style)
        .input_key_interceptor(interceptor)
        .on_query_change(ctx.link().callback(|query: Arc<str>| {
            Msg::RemotePickerSessionQueryChanged(query.to_string())
        }))
        .on_select(ctx.link().callback(
            |event: SearchEvent<RemoteSessionIdentity>| {
                Msg::RemotePickerSessionSelect(event.item.value.clone())
            },
        ))
        .on_activate(ctx.link().callback(
            |event: SearchEvent<RemoteSessionIdentity>| {
                Msg::RemotePickerSessionActivate(event.item.value.clone())
            },
        ));
    if pending_kill.is_some() || pending_restart.is_some() {
        palette = palette.render_item(Arc::new(move |item: &SearchItem<RemoteSessionIdentity>, _hl| {
            if pending_kill.as_ref() == Some(&item.value) {
                Some(render_pending_confirm_item(
                    item.label.as_ref(),
                    error_bg,
                    "again to kill",
                    true,
                ))
            } else if pending_restart.as_ref() == Some(&item.value) {
                Some(render_pending_confirm_item(
                    item.label.as_ref(),
                    warning_bg,
                    "again to restart",
                    false,
                ))
            } else {
                None
            }
        }));
    }
    palette
}

fn remote_selected_session(
    picker: &crate::state::RemotePickerState,
) -> Option<&crate::session::discovery::DiscoveredSession> {
    let selected = picker.selected_session.as_ref()?;
    let query = picker.session_input.text().trim().to_ascii_lowercase();
    picker.sessions.iter().find(|session| {
        RemoteSessionIdentity::of(session).as_ref() == Some(selected)
            && (query.is_empty() || session.name.to_ascii_lowercase().contains(&query))
    })
}

fn remote_host_session_hints(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
    target: &crate::session::remote::RemoteTarget,
) -> Element {
    let selected = remote_selected_session(picker);
    let mut row = hint_row();
    if let Some(session) = selected
        && !crate::ops::session::session_row_is_current(&ctx.state, session)
    {
        row = row.child(hint_pill(&ctx.state.theme, "open", "enter"));
    }
    row = row
        .child(hint_pill(&ctx.state.theme, "new", "ctrl+n"))
        .child(hint_pill(&ctx.state.theme, "ephemeral", "ctrl+t"));
    if let Some(session) = selected {
        if crate::ops::session::session_row_can_disconnect(&ctx.state, session) {
            row = row.child(hint_pill(&ctx.state.theme, "disconnect", "ctrl+w"));
        }
        if crate::ops::session::session_row_can_restart(session) {
            row = row.child(hint_pill(&ctx.state.theme, "restart", "ctrl+e"));
        }
        row = row.child(hint_pill(&ctx.state.theme, "kill", "ctrl+k"));
    }
    if !remote_host_connections(ctx, target).is_empty() {
        row = row.child(hint_pill(
            &ctx.state.theme,
            "disconnect host",
            "ctrl+x",
        ));
    }
    row.into()
}

pub(crate) fn remote_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.remote_picker.as_ref() else {
        return Text::new("").into();
    };
    if let Some(prompt) = picker.target_prompt.as_ref() {
        return prompt_overlay(
            ctx,
            PromptChrome {
                caption: prompt.error.as_deref().map(PromptCaption::Armed),
                ..PromptChrome::new(
                    "Connect new host",
                    "host / alias / ssh://...",
                    &[("connect", "enter")],
                )
            },
            &prompt.input,
            remote_target_input_key(),
            Msg::RemoteTargetPromptChanged,
            Msg::CloseRemoteTargetPrompt,
            Msg::SubmitRemoteTarget,
        );
    }
    let (title, body): (String, Element) = match &picker.mode {
        crate::state::RemotePickerMode::Hosts => {
            let connecting = matches!(picker.host_probe, crate::state::HostProbe::InFlight);
            let selected = remote_selected_host(ctx, picker);
            let mut hints = hint_row();
            if connecting {
                hints = hints.child(hint_pill(&ctx.state.theme, "cancel", "esc"));
            } else {
                if selected.is_some() {
                    hints = hints.child(hint_pill(&ctx.state.theme, "open", "enter"));
                }
                hints = hints.child(hint_pill(&ctx.state.theme, "new host", "ctrl+n"));
                if selected.is_some_and(|entry| {
                    crate::ops::session::remotes::host_can_forget(&ctx.state, &entry.target)
                }) {
                    hints = hints.child(hint_pill(&ctx.state.theme, "forget", "ctrl+k"));
                }
            }
            let palette = remote_hosts_palette(ctx, picker);
            let palette: Element = if connecting {
                Element::from(palette).key(format!(
                    "remote-hosts-{}",
                    picker.interaction_epoch
                ))
            } else {
                palette.into()
            };
            let body = VStack::new()
                .height(Length::Auto)
                .child(palette)
                .child(hints);
            ("Remote hosts".to_string(), body.into())
        }
        crate::state::RemotePickerMode::HostSessions { target } => {
            let body = VStack::new()
                .height(Length::Auto)
                .child(remote_host_sessions_palette(ctx, picker, target))
                .child(remote_host_session_hints(ctx, picker, target));
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
