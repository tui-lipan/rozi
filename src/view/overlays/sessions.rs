pub(crate) fn session_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Text::new("").into();
    };
    session_picker_palette(ctx, picker)
}

/// A row in the collaborators roster: one other client, plus what may be done to it from here.
#[derive(Clone, Copy, PartialEq)]
struct CollaboratorItem {
    /// Index into the shared roster, which is what the grant/decline/evict ops address.
    roster_index: usize,
    client_id: crate::layout::shared::ClientId,
    position: usize,
    grantable: bool,
    requesting: bool,
    kickable: bool,
}

/// Who else is on the session, and what this client may do about them.
pub(crate) fn collaboration_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(state) = ctx.state.collaboration.as_ref() else {
        return Text::new("").into();
    };
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Text::new("").into();
    };
    let can_evict = crate::ops::session::can_evict(&ctx.state);
    let (rows, items) = collaborator_rows(shared, can_evict);
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
    let actions = collaborator_actions(ctx, selected_item, armed.is_some());
    let fallback = ctx.link().key_handler(|key_event| {
        key_event
            .is(KeyCode::Esc)
            .then_some(Msg::CloseCollaboration)
    });

    OverlayPalette::new(
        "Manage collaborators",
        collaboration_key(),
        Msg::CloseCollaboration,
        64,
    )
    .header_right(self_tag(shared))
    .entries(entries)
    .actions(actions)
    .armed_row(armed.and(selected_item))
    .placeholder("Search other clients…")
    .empty_text(empty_text)
    .initial_query(state.query.clone())
    .selected(Some(selected))
    .fallback_interceptor(fallback)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::CollaborationQueryChanged(query.to_string())),
    )
    .on_select(ctx.link().callback(|event: SearchEvent<CollaboratorItem>| {
        Msg::CollaborationSelect(event.item.value.position)
    }))
    .on_activate(ctx.link().callback(
        |event: SearchEvent<CollaboratorItem>| match event.item.value {
            CollaboratorItem {
                roster_index,
                grantable: true,
                ..
            } => Msg::CollaborationGrant(roster_index),
            CollaboratorItem { position, .. } => Msg::CollaborationSelect(position),
        },
    ))
    .render(ctx)
}

fn collaborator_rows(
    shared: &crate::state::SharedSessionState,
    can_evict: bool,
) -> (
    Vec<SearchItem<CollaboratorItem>>,
    Vec<CollaboratorItem>,
) {
    let mut rows = Vec::new();
    let mut items = Vec::new();
    for (roster_index, client) in shared.clients.iter().enumerate() {
        if client.id == shared.client_id {
            continue;
        }
        let item = CollaboratorItem {
            roster_index,
            client_id: client.id,
            position: items.len(),
            grantable: !shared.read_only
                && shared.is_controller()
                && !client.read_only
                && !client.parked,
            requesting: client.requesting_control,
            kickable: can_evict,
        };
        rows.push(
            SearchItem::new(client_tag(&client.label, client.id), item).description(
                picker_description(collaborator_markers(shared, client).join(" · ")),
            ),
        );
        items.push(item);
    }
    (rows, items)
}

fn collaborator_markers(
    shared: &crate::state::SharedSessionState,
    client: &crate::session::protocol::ClientInfo,
) -> Vec<&'static str> {
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
    markers
}

fn collaborator_actions(
    ctx: &Context<AppRoot>,
    selected: Option<CollaboratorItem>,
    armed: bool,
) -> Vec<OverlayAction> {
    let Some(item) = selected else {
        return Vec::new();
    };
    vec![
        OverlayAction::new(
            "enter",
            "grant control",
            Msg::CollaborationGrant(item.roster_index),
            item.grantable,
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-d",
            "decline",
            Msg::CollaborationDecline(item.roster_index),
            item.grantable && item.requesting,
        ),
        OverlayAction::new(
            "ctrl-k",
            if armed { "confirm kick" } else { "kick" },
            Msg::CollaborationKick(item.roster_index),
            item.kickable,
        )
        .confirm_if(
            armed,
            "again to confirm",
            ctx.state.theme.status.error,
            true,
        ),
    ]
}

/// One client as a compact token: `razuer #2077`.
fn client_tag(label: &str, id: crate::layout::shared::ClientId) -> String {
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

/// The choice offered when an attach lands on a session another client is driving.
pub(crate) fn follow_prompt_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(prompt) = ctx.state.follow_prompt.as_ref() else {
        return Text::new("").into();
    };
    let entries = crate::state::FollowChoice::ALL
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            SearchEntry::item(choice.label(prompt.allow_takeover).to_string(), index).description(
                picker_description(choice.description(prompt.allow_takeover)),
            )
        })
        .collect::<Vec<_>>();
    let selected = prompt.selected;
    let last = crate::state::FollowChoice::ALL.len() - 1;
    let fallback = ctx.link().key_handler(move |key_event| {
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
    let title = format!("`{}` in use by {}", prompt.session, prompt.controller_label);
    OverlayPalette::new(
        title,
        crate::view::follow_prompt_key(),
        Msg::FollowPromptChoose(last),
        64,
    )
    .entries(entries)
    .placeholder("")
    .selected(Some(selected))
    .fallback_interceptor(fallback)
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::FollowPromptSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::FollowPromptChoose(event.item.value)),
    )
    .render(ctx)
}

/// Non-dismissible progress chrome while an automatic reconnect preserves the panes underneath.
pub(crate) fn reconnecting_overlay(ctx: &Context<AppRoot>) -> Element {
    let name = ctx
        .state
        .current()
        .session_name
        .as_deref()
        .unwrap_or("session");
    styled_modal(ctx, &format!("Session · {name}"), 42)
        .auto_focus(false)
        .dismiss_on_escape(false)
        .child(
            Spinner::new()
                .spinner_style(SpinnerStyle::Dots)
                .label("reconnecting")
                .style(Style::new().fg(ctx.state.theme.status.warning))
                .label_style(fg_only(&ctx.state.theme.primary)),
        )
        .into()
}

/// The footer hint row only advertises keys that would actually act on the current state, so a
/// hint never lies. Enter is **switch** for a background-connected session, **restore** for a
/// resurrection snapshot, and **connect** when establishing a connection; **disconnect** closes
/// this client's attachment; **kill** destroys a live session and **forget** drops a snapshot;
/// **restart** recreates a live session.
///
/// Row actions for a restorable snapshot lead the bar (`restore`, `forget`) because those are the
/// verbs that apply to the highlighted recipe. Global picker actions follow. Restart is omitted:
/// there is no live server to recreate.
///
/// `ephemeral shell` is the exception that is deliberately *under*-advertised: `Ctrl+T` always
/// reaches this client's scratch session, but saying so is only worth a pill when the list cannot
/// say it already. With nothing to pick, Enter is free and carries it; with the scratch session
/// itself on the list, its own row is the obvious way to it. The label borrows the word the rows
/// use (`ephemeral`) so the hint and the session it lands on read as the same thing.
/// Whether a remote host is anywhere in this client's picture: the session on screen, one parked in
/// the background, or the host a sessionless launcher is scoped to.
///
/// What it gates is the word "local" on the global picker's own keys. Those keys are local
/// unconditionally — the global Sessions surface shows every host and commits to none — but a user
/// with nothing remote in play has no second reading to be protected from, and "new local" would
/// only raise a question ("local as opposed to what?") the screen cannot answer.
fn remote_is_in_play(state: &crate::state::State) -> bool {
    state.current().remote_target.is_some()
        || state.launcher_scope.is_some()
        || state
            .background
            .values()
            .any(|attachment| attachment.remote_target.is_some())
}

fn session_picker_actions(ctx: &Context<AppRoot>) -> Vec<OverlayAction> {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Vec::new();
    };
    let selected = selected_session(picker);
    let restorable = selected.is_some_and(crate::ops::session::session_row_is_restorable);
    let mut actions = Vec::new();
    push_session_activation(ctx, picker, selected, restorable, &mut actions);
    push_session_creation_actions(ctx, picker, &mut actions);
    push_session_management_actions(ctx, picker, selected, restorable, &mut actions);
    actions
}

fn push_session_activation(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    selected: Option<&crate::session::discovery::DiscoveredSession>,
    restorable: bool,
    actions: &mut Vec<OverlayAction>,
) {
    if picker_list_is_empty(picker) {
        actions.push(
            OverlayAction::new(
                "enter",
                if remote_is_in_play(&ctx.state) {
                    "local ephemeral"
                } else {
                    "ephemeral shell"
                },
                Msg::SessionPickerEphemeral,
                true,
            ),
        );
    }
    let Some(entry) = selected else {
        return;
    };
    if restorable {
        actions.push(
            OverlayAction::new(
                "enter",
                "restore",
                Msg::SessionPickerActivate(picker.selected),
                true,
            )
            .hint_only(),
        );
        actions.push(
            OverlayAction::new("ctrl-k", "forget", Msg::SessionPickerKillSelected, true).confirm_if(
                picker.pending_kill == Some(picker.selected),
                "again to forget",
                ctx.state.theme.status.error,
                true,
            ),
        );
    } else if !crate::ops::session::session_row_is_current(&ctx.state, entry) {
        let held = ctx
            .state
            .attachment_by_identity(&entry.name, entry.remote_target.as_ref())
            .map(|attachment| attachment.connection);
        let label = match held {
            Some(crate::state::ConnectionState::Connected) => "switch",
            _ => "connect",
        };
        actions.push(
            OverlayAction::new(
                "enter",
                label,
                Msg::SessionPickerActivate(picker.selected),
                true,
            )
            .hint_only(),
        );
    }
}

fn push_session_creation_actions(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    actions: &mut Vec<OverlayAction>,
) {
    // Both keys act on this surface's scope, which is global — so they act locally. Saying "local"
    // is only worth the width once a remote host is in play; with nothing remote anywhere, "new"
    // has nothing to be mistaken for.
    let remote_in_play = remote_is_in_play(&ctx.state);
    actions.push(
        OverlayAction::new(
            "ctrl-n",
            if remote_in_play { "new local" } else { "new" },
            Msg::SessionPickerCreateFromQuery,
            true,
        ),
    );
    let show_ephemeral = !picker_list_is_empty(picker)
        && crate::ops::session::held_ephemeral_session_in(&ctx.state, None).is_none();
    actions.push(
        OverlayAction::new(
            "ctrl-t",
            if remote_in_play {
                "local ephemeral"
            } else {
                "ephemeral shell"
            },
            Msg::SessionPickerEphemeral,
            show_ephemeral,
        ),
    );
    if ctx.state.current().session_attached && ctx.state.is_ephemeral_session() {
        actions.push(
            OverlayAction::new(
                "ctrl-s",
                "name current",
                Msg::SessionPickerNameCurrent,
                true,
            ),
        );
    }
    actions.push(
        OverlayAction::new(
            "ctrl-r",
            "remote hosts",
            Msg::SessionPickerRemoteHosts,
            true,
        ),
    );
}

fn push_session_management_actions(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    selected: Option<&crate::session::discovery::DiscoveredSession>,
    restorable: bool,
    actions: &mut Vec<OverlayAction>,
) {
    if let Some(entry) = selected
        && !restorable
    {
        if crate::ops::session::session_row_can_disconnect(&ctx.state, entry) {
            actions.push(OverlayAction::new(
                "ctrl-w",
                "disconnect",
                Msg::SessionPickerDisconnectAttachment,
                true,
            ));
        }
        actions.push(
            OverlayAction::new(
                "ctrl-e",
                "restart",
                Msg::SessionPickerRestartSelected,
                crate::ops::session::session_row_can_restart(entry),
            )
            .confirm_if(
                picker.pending_restart == Some(picker.selected),
                "again to restart",
                ctx.state.theme.status.warning,
                false,
            ),
        );
        actions.push(
            OverlayAction::new("ctrl-k", "kill", Msg::SessionPickerKillSelected, true).confirm_if(
                picker.pending_kill == Some(picker.selected),
                "again to kill",
                ctx.state.theme.status.error,
                true,
            ),
        );
    }
    if selected.is_some_and(|entry| {
        crate::ops::session::session_row_can_disconnect_host(&ctx.state, entry)
    }) {
        actions.push(OverlayAction::new(
            "ctrl-x",
            "disconnect host",
            Msg::SessionPickerDisconnectHost,
            true,
        ));
    }
}

use crate::view::session_status::{
    SessionConnectionStatus, session_connection_status, session_status_gutter,
};

/// The currently highlighted session, if it is still on screen after filtering.
fn selected_session(
    picker: &SessionPickerState,
) -> Option<&crate::session::discovery::DiscoveredSession> {
    let query_lower = picker.input.text().trim().to_ascii_lowercase();
    picker
        .entries
        .get(picker.selected)
        .filter(|entry| matches_session_query(entry, &query_lower))
}

/// Whether a session row survives the picker's filter. The list, the footer hints, and the keys
/// that only apply to a listed row all have to agree on what is on screen, so they share one
/// predicate rather than each spelling the match out.
fn matches_session_query(
    entry: &crate::session::discovery::DiscoveredSession,
    query_lower: &str,
) -> bool {
    query_lower.is_empty()
        || entry.name.to_ascii_lowercase().contains(query_lower)
        || entry
            .host
            .as_deref()
            .is_some_and(|host| host.to_ascii_lowercase().contains(query_lower))
}

/// Whether the picker is showing no session at all — nothing discovered, or nothing left by the
/// query. There is then no row for Enter to activate, which is what frees it to start a shell.
fn picker_list_is_empty(picker: &SessionPickerState) -> bool {
    let query = picker.input.text().trim().to_ascii_lowercase();
    !picker
        .entries
        .iter()
        .any(|entry| matches_session_query(entry, &query))
}

fn session_picker_palette(ctx: &Context<AppRoot>, picker: &SessionPickerState) -> Element {
    let theme = &ctx.state.theme;
    let query = picker.input.text().trim().to_ascii_lowercase();
    let current_name = ctx.state.current().session_name.as_deref();
    let current_host = ctx.state.current().remote_host.as_deref();
    let current_remote_target = &ctx.state.current().remote_target;
    let statuses: Vec<SessionConnectionStatus> = picker
        .entries
        .iter()
        .map(|entry| {
            let is_current = current_name == Some(entry.name.as_str())
                && current_host == entry.host.as_deref()
                && current_remote_target == &entry.remote_target;
            let connection = ctx
                .state
                .attachment_by_identity(&entry.name, entry.remote_target.as_ref())
                .map(|attachment| attachment.connection);
            session_connection_status(is_current, connection)
        })
        .collect();
    let ephemeral_entries = picker
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.ephemeral.then_some(index))
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut last_group: Option<Option<&str>> = None;
    let mut reserve_discovered_gutter = false;
    for (index, entry) in picker
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches_session_query(entry, &query))
    {
        reserve_discovered_gutter |= statuses[index] != SessionConnectionStatus::Discovered;
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
        let we_hold = !matches!(statuses[index], SessionConnectionStatus::Discovered);
        entries
            .push(SearchEntry::item(label, index).description(session_description(entry, we_hold)));
    }
    // Say what is (not) there, nothing more: the footer already advertises `new ctrl+n`, and
    // repeating it in the body says the same thing twice in a longer sentence.
    let empty_text = if picker.entries.is_empty() {
        "No sessions".to_string()
    } else if query.is_empty() {
        "Type to filter sessions".to_string()
    } else {
        format!("No sessions match `{query}`")
    };

    let pending_kill = picker.pending_kill;
    let pending_restart = picker.pending_restart;
    let ephemeral_style = fg_only(&theme.primary).italic();
    let description_style = fg_only(&theme.muted);
    let status_styles = crate::view::session_status::SessionStatusStyles::from_theme(theme);
    let fallback = ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSessionPicker)
        } else if ctrl_letter(&key, 't') {
            // This remains intentionally usable when its redundant footer pill is hidden.
            Some(Msg::SessionPickerEphemeral)
        } else {
            None
        }
    });
    let render_item = (!ephemeral_entries.is_empty()).then(|| {
        Arc::new(move |item: &SearchItem<usize>, _hl: &SearchHighlight| {
            ephemeral_entries
                .contains(&item.value)
                .then(|| render_ephemeral_session_item(item, &ephemeral_style, &description_style))
        }) as OverlayItemRenderer<usize>
    });

    let mut overlay = OverlayPalette::new(
        "Sessions",
        session_picker_key(),
        Msg::CloseSessionPicker,
        64,
    )
    .entries(entries)
    .actions(session_picker_actions(ctx))
    .armed_row(pending_kill.or(pending_restart))
    .placeholder("Search sessions...")
    .initial_query(picker.input.text().to_string())
    .selected(Some(picker.selected))
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
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
    )
    // Leading space indents the marker; list item left padding is the gap before the label.
    .item_gutter(Arc::new(move |item: &SearchItem<usize>, _hl| {
        let status = *statuses.get(item.value)?;
        session_status_gutter(status, status_styles, reserve_discovered_gutter)
    }));
    if let Some(render_item) = render_item {
        overlay = overlay.render_item(render_item);
    }
    overlay.render(ctx)
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
            picker_description(label)
        }
        DiscoveredSessionStatus::Restorable => picker_description("restorable"),
        DiscoveredSessionStatus::Busy => picker_description("busy"),
        DiscoveredSessionStatus::Unknown => picker_description("unavailable"),
    }
}
