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
    let cached = crate::session::host_sessions_for(&ctx.state.host_session_cache, &entry.target)
        .map(|sessions| sessions.len())
        .unwrap_or_default();
    let status = remote_host_status(ctx, &entry.target);
    // Session counts win where they exist and the status is settled — "2 sessions" says more about
    // a reachable host than "reached" does. The status words themselves come from the one shared
    // vocabulary, so this row and the sidebar's badge never call the same state two things.
    let label = match (cached, status, entry.origin) {
        (_, crate::state::HostStatus::Connecting | crate::state::HostStatus::Unreachable, _) => {
            crate::view::session_status::host_status_label(status).to_string()
        }
        (1, _, _) => "1 session".to_string(),
        (count, _, _) if count > 1 => format!("{count} sessions"),
        (_, crate::state::HostStatus::Connected | crate::state::HostStatus::Reachable, _) => {
            crate::view::session_status::host_status_label(status).to_string()
        }
        (_, _, crate::state::HostOrigin::Configured) => "configured".to_string(),
        (_, _, crate::state::HostOrigin::Recent) => "recent".to_string(),
        (_, _, crate::state::HostOrigin::Attached) => "attached".to_string(),
    };
    picker_description(label)
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

fn remote_hosts_overlay(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
) -> Element {
    let connecting = matches!(picker.host_probe, crate::state::HostProbe::InFlight);
    let connecting_target = connecting.then(|| picker.selected_host.clone()).flatten();
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
                picker_description(crate::view::session_status::host_status_label(
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
    let fallback = ctx.link().key_handler({
        let connecting_target = connecting_target.clone();
        move |key| {
            if connecting {
                connecting_target.clone().map(Msg::RemotePickerHostSelect)
            } else if key.is(KeyCode::Esc) {
                Some(Msg::CloseRemotePicker)
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
    let actions = if connecting {
        vec![OverlayAction::new(
            "esc",
            "cancel",
            Msg::CloseRemotePicker,
            true,
        )]
    } else {
        vec![
            OverlayAction::new(
                "enter",
                "open",
                selected_entry
                    .map(|entry| Msg::RemotePickerHostActivate(entry.target.clone()))
                    .unwrap_or(Msg::CloseRemotePicker),
                selected_entry.is_some(),
            )
            .hint_only(),
            OverlayAction::new("ctrl-n", "new host", Msg::RemotePickerNewHost, true),
            OverlayAction::new("ctrl-k", "forget", Msg::RemotePickerForgetHost, can_forget)
                .confirm_if(pending_forget.is_some(), "again to forget", error_bg, true),
        ]
    };
    let mut overlay = OverlayPalette::new(
        "Remote hosts",
        remote_picker_key(),
        Msg::CloseRemotePicker,
        64,
    )
    .entries(entries)
    .actions(actions)
    .armed_row(pending_forget)
    .placeholder("Search hosts...")
    .initial_query(picker.host_input.text().to_string())
    .selected(selected)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
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
            Some(crate::view::session_status::host_status_gutter(
                status, styles,
            ))
        }
    }))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::RemotePickerHostQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<crate::session::remote::RemoteTarget>| {
                Msg::RemotePickerHostSelect(event.item.value.clone())
            }),
    )
    .on_activate(ctx.link().callback(
        |event: SearchEvent<crate::session::remote::RemoteTarget>| {
            Msg::RemotePickerHostActivate(event.item.value.clone())
        },
    ));
    if connecting {
        overlay = overlay.element_key(format!("remote-hosts-{}", picker.interaction_epoch));
    }
    overlay.render(ctx)
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

fn remote_host_sessions_overlay(
    ctx: &Context<AppRoot>,
    picker: &crate::state::RemotePickerState,
    target: &crate::session::remote::RemoteTarget,
) -> Element {
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
        format!("No sessions match `{}`", picker.session_input.text().trim())
    };
    let pending_kill = picker.pending_kill.clone();
    let pending_restart = picker.pending_restart.clone();
    let error_bg = ctx.state.theme.status.error;
    let warning_bg = ctx.state.theme.status.warning;
    let selected_session = remote_selected_session(picker).cloned();
    let can_restart = selected_session
        .as_ref()
        .is_some_and(crate::ops::session::session_row_can_restart);
    let can_disconnect = selected_session.as_ref().is_some_and(|session| {
        crate::ops::session::session_row_can_disconnect(&ctx.state, session)
    });
    let can_disconnect_host = crate::ops::session::host_can_disconnect(&ctx.state, target);
    let selected_identity = picker.selected_session.clone();
    let selected_is_current = selected_session
        .as_ref()
        .is_some_and(|session| crate::ops::session::session_row_is_current(&ctx.state, session));
    let actions = vec![
        OverlayAction::new(
            "enter",
            "open",
            selected_identity
                .clone()
                .map(Msg::RemotePickerSessionActivate)
                .unwrap_or(Msg::CloseRemotePicker),
            selected_session.is_some() && !selected_is_current,
        )
        .hint_only(),
        OverlayAction::new("ctrl-n", "new", Msg::RemotePickerCreateSession, true),
        OverlayAction::new("ctrl-t", "ephemeral", Msg::RemotePickerEphemeral, true),
        OverlayAction::new(
            "ctrl-w",
            "disconnect",
            Msg::RemotePickerDisconnectSession,
            can_disconnect,
        ),
        OverlayAction::new(
            "ctrl-e",
            "restart",
            Msg::RemotePickerRestartSession,
            can_restart,
        )
        .confirm_if(
            pending_restart.is_some(),
            "again to restart",
            warning_bg,
            false,
        ),
        OverlayAction::new(
            "ctrl-k",
            "kill",
            Msg::RemotePickerKillSession,
            selected_session.is_some(),
        )
        .confirm_if(pending_kill.is_some(), "again to kill", error_bg, true),
        OverlayAction::new(
            "ctrl-x",
            "disconnect host",
            Msg::RemotePickerDisconnectHost,
            can_disconnect_host,
        ),
    ];
    let fallback = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::CloseRemotePicker));
    OverlayPalette::new(
        format!("Sessions · {}", target.display_label()),
        remote_picker_key(),
        Msg::CloseRemotePicker,
        64,
    )
    .entries(entries)
    .actions(actions)
    .armed_row(pending_kill.or(pending_restart))
    .placeholder("Search sessions...")
    .initial_query(picker.session_input.text().to_string())
    .selected(selected)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::RemotePickerSessionQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<RemoteSessionIdentity>| {
                Msg::RemotePickerSessionSelect(event.item.value.clone())
            }),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<RemoteSessionIdentity>| {
                Msg::RemotePickerSessionActivate(event.item.value.clone())
            }),
    )
    .render(ctx)
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
    match &picker.mode {
        crate::state::RemotePickerMode::Hosts => remote_hosts_overlay(ctx, picker),
        crate::state::RemotePickerMode::HostSessions { target } => {
            remote_host_sessions_overlay(ctx, picker, target)
        }
    }
}
