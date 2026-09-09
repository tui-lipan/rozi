/// This surface lists remembered session *counts*, not live rows, so it never lets a cached count
/// stand in for having reached the host.
fn remote_host_status(
    ctx: &Context<AppRoot>,
    target: &crate::session::remote::RemoteTarget,
) -> crate::state::HostStatus {
    crate::view::session_status::host_connection_status(&ctx.state, target, false)
}

fn remote_host_description(
    ctx: &Context<AppRoot>,
    entry: &crate::state::HostEntry,
) -> ItemDescription {
    let cached = crate::session::host_sessions_for(&ctx.state.host_session_cache, &entry.target)
        .map(|sessions| sessions.len())
        .unwrap_or_default();
    let status = remote_host_status(ctx, &entry.target);
    // A failure is the one thing worth more than a session count: the row has to say why the last
    // attempt did not land, or the user is left retrying something they cannot diagnose. The short
    // phrase comes from the shared vocabulary; the raw ssh output stays in the toast.
    if status == crate::state::HostStatus::Unreachable
        && let Some(error) = entry.probe.error()
    {
        return picker_description(crate::session::discovery::probe_failure_reason(error));
    }
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
        (_, _, crate::state::HostOrigin::Saved) => "saved".to_string(),
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
    let connecting_target = matches!(picker.host_probe, crate::state::HostProbe::InFlight)
        .then(|| picker.probe_target.clone())
        .flatten();
    let selected_entry = remote_selected_host(ctx, picker);
    let selected_target = selected_entry.map(|entry| entry.target.clone());
    // Navigation stays free while a host connects — the user can read their other machines instead
    // of watching one spinner. What waits is starting a *second* connection: this picker tracks one
    // in-flight target, so a second ssh would strand the first host spinning with no answer coming.
    let connecting = connecting_target.is_some();
    let can_forget = selected_target
        .as_ref()
        .is_some_and(|target| crate::ops::session::remotes::host_can_forget(&ctx.state, target));
    let can_edit = selected_target
        .as_ref()
        .is_some_and(|target| crate::ops::session::remotes::host_can_edit(&ctx.state, target));
    let entries = ctx
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
    let selected = picker.selected_host.as_ref().and_then(|selected| {
        entries.iter().enumerate().find_map(|(index, entry)| {
            matches!(entry, SearchEntry::Item(item) if &item.value == selected).then_some(index)
        })
    });
    let fallback = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::CloseRemotePicker));
    let empty_text = if picker.host_input.text().trim().is_empty() {
        "No known remote hosts".to_string()
    } else {
        format!("No hosts match `{}`", picker.host_input.text().trim())
    };
    let pending_forget = picker.pending_forget.clone();
    let error_bg = ctx.state.theme.status.error;
    // `Enter` connects a host that is not yet reached and opens one that is, so the hint says which
    // of the two the highlighted row will get.
    let selected_reached = selected_target.as_ref().is_some_and(|target| {
        matches!(
            ctx.state.hosts.get(target).map(|entry| &entry.probe),
            Some(crate::state::HostProbe::Reached)
        )
    });
    let actions = vec![
        OverlayAction::new(
            "enter",
            if selected_reached { "open" } else { "connect" },
            selected_target
                .clone()
                .map(Msg::RemotePickerHostActivate)
                .unwrap_or(Msg::CloseRemotePicker),
            selected_target.is_some() && !connecting,
        )
        .hint_only(),
        OverlayAction::new("ctrl-n", "add host", Msg::RemotePickerNewHost, true),
        OverlayAction::new("ctrl-e", "edit", Msg::RemotePickerEditHost, can_edit),
        OverlayAction::new(
            "ctrl-r",
            "reconnect",
            Msg::RemotePickerReconnectHost,
            selected_target.is_some() && !connecting,
        ),
        OverlayAction::new(
            "ctrl-k",
            "forget",
            Msg::RemotePickerForgetHost,
            can_forget,
        )
        .confirm_if(pending_forget.is_some(), "again to forget", error_bg, true),
    ];
    let overlay = OverlayPalette::new(
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
            let agents = crate::view::session_status::host_agent_label(&ctx.state, session);
            Some(
                SearchEntry::item(label, identity)
                    .description(session_description(session, we_hold, agents)),
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

/// Width of the host editor's label column, so `Host`, `Username`, and `Port` line their fields up.
const HOST_FORM_LABEL_WIDTH: u16 = 10;

/// One line of the host editor.
///
/// Only the line the cursor is on mounts an editable `Input`; the others show what they hold as
/// plain text. One input in the dialog means the framework's focus ring has nothing to shuffle
/// between, so `Tab` — claimed at the root, see `input::routing` — is the only thing that moves the
/// cursor, and a keystroke can never land on the line being left.
///
/// A *Username* the host line has already answered never takes the cursor at all: there is nothing
/// to type into it, and stopping there would only invite the user to contradict the host line.
fn host_form_row(
    ctx: &Context<AppRoot>,
    form: &crate::state::HostFormState,
    field: crate::state::HostFormField,
) -> Element {
    let theme = &ctx.state.theme;
    let locked = field == crate::state::HostFormField::User && form.user_is_locked();
    let focused = form.focus == field && !locked;
    let label_style = if focused {
        fg_only(&theme.primary).bold()
    } else {
        fg_only(&theme.muted)
    };
    let input: Element = if focused {
        Input::bound(form.input(field))
            .placeholder(field.placeholder())
            .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
            .focus_style(
                Style::new()
                    .fg(theme.border_active)
                    .bg(theme.surface.element),
            )
            .selection_style(theme.text_selection)
            .width(Length::Flex(1))
            .border(false)
            .padding((0, 0))
            .on_change(
                ctx.link()
                    .callback(move |event: InputEvent| Msg::HostFormChanged(field, event)),
            )
            .on_key(ctx.link().key_handler(|key| {
                if key.is(KeyCode::Esc) {
                    Some(Msg::CloseHostForm)
                } else if key.code == KeyCode::Enter && !key.mods.ctrl && !key.mods.alt {
                    Some(Msg::SubmitHostForm)
                } else {
                    None
                }
            }))
            .key(crate::view::host_form_input_key())
    } else {
        // A locked *Username* shows the host line's login rather than the user's own text, and says
        // so by reading as derived: clearing the `user@` gives the line and its contents back.
        let (text, derived) = if locked {
            (form.shown_user(), true)
        } else {
            (form.input(field).text(), false)
        };
        let (text, style) = if text.is_empty() {
            (field.placeholder(), fg_only(&theme.muted).italic())
        } else if derived {
            (text, fg_only(&theme.muted))
        } else {
            (text, fg_only(&theme.primary))
        };
        Text::new(text)
            .overflow(Overflow::Clip)
            .width(Length::Flex(1))
            .style(style)
            .into()
    };
    HStack::new()
        .height(Length::Auto)
        .padding((0, 1, 0, 0))
        .child(
            // A borderless field in a borderless dialog needs *something* saying which line the
            // next keystroke lands on. The bold label alone is too quiet on a line whose only other
            // content is a placeholder.
            Text::new(if focused { "› " } else { "  " })
                .overflow(Overflow::Clip)
                .width(Length::Px(2))
                .style(fg_only(&theme.accent)),
        )
        .child(
            Text::new(field.label())
                .overflow(Overflow::Clip)
                .width(Length::Px(HOST_FORM_LABEL_WIDTH))
                .style(label_style),
        )
        .child(input)
        .into()
}

/// The *Add host* / *Edit host* form: host, login, and port, always all three.
///
/// Adding and editing show the same shape. A form that reveals its fields as it goes hides what it
/// is going to ask for, and the answer to "what does rozi store about a host" should be legible
/// from the dialog itself rather than discovered one `Enter` at a time.
fn host_form_overlay(ctx: &Context<AppRoot>, form: &crate::state::HostFormState) -> Element {
    let theme = &ctx.state.theme;
    let mut body = VStack::new().height(Length::Auto).padding((1, 0, 0, 0));
    for field in crate::state::HostFormField::ORDER {
        body = body.child(host_form_row(ctx, form, field));
    }
    if let Some(error) = form.error.as_deref() {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 1, 0, 1))
                .child(
                    Text::new(error)
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(Style::new().fg(theme.status.warning).italic()),
                ),
        );
    }
    // `tab` alone: Shift+Tab goes back, but a hint bar that spells out both halves of one
    // convention spends a pill saying what every dialog already does.
    body = body.child(
        hint_row()
            .child(hint_pill(theme, "save", "enter"))
            .child(hint_pill(theme, "next field", "tab"))
            .child(hint_pill(theme, "cancel", "esc")),
    );

    action_palette_modal(ctx, form.title())
        .on_close(ctx.link().callback(|_| Msg::CloseHostForm))
        .child(action_palette_frame(body))
        .into()
}

pub(crate) fn remote_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.remote_picker.as_ref() else {
        return Text::new("").into();
    };
    if let Some(form) = picker.host_form.as_ref() {
        return host_form_overlay(ctx, form);
    }
    match &picker.mode {
        crate::state::RemotePickerMode::Hosts => remote_hosts_overlay(ctx, picker),
        crate::state::RemotePickerMode::HostSessions { target } => {
            remote_host_sessions_overlay(ctx, picker, target)
        }
    }
}
