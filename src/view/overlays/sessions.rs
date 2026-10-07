use super::*;

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
        "Collaborators",
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
) -> (Vec<SearchItem<CollaboratorItem>>, Vec<CollaboratorItem>) {
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
        OverlayAction::destructive(
            ctx,
            "kick",
            Msg::CollaborationKick(item.roster_index),
            item.kickable,
            armed,
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

/// Progress chrome while an automatic reconnect preserves the panes underneath, or the offline
/// prompt after that window ends on a remote host. Neither is dismissible with a bare click: the
/// first is in-flight, the second is the only honest reading of a retained session whose host is
/// gone. `Esc` abandons an in-flight reconnect (and opens Sessions) so the overlay cannot trap
/// the user for the whole retry window. From offline, `Enter` retries; `Esc` opens Sessions.
pub(crate) fn reconnecting_overlay(ctx: &Context<AppRoot>) -> Element {
    let name = ctx
        .state
        .current()
        .session_name
        .as_deref()
        .unwrap_or("session");
    let offline = ctx.state.current().connection == crate::state::ConnectionState::Unreachable;
    let lost = ctx.state.current().remote_session_lost;
    let modal = styled_modal(ctx, &format!("Session · {name}"), 42).dismiss_on_escape(false);
    let actions = if offline {
        vec![
            OverlayAction::new(
                "enter",
                if lost { "recreate" } else { "reconnect" },
                if lost {
                    Msg::RecreateLostRemoteSession
                } else {
                    Msg::RetrySessionReconnect
                },
                true,
            ),
            OverlayAction::new(
                "esc",
                "sessions",
                Msg::RunAction(Action::OpenSessionPicker),
                true,
            ),
        ]
    } else {
        vec![OverlayAction::new(
            "esc",
            "sessions",
            Msg::AbandonSessionReconnect,
            true,
        )]
    };
    let status: Element = if offline {
        Text::new(if lost { "session lost" } else { "offline" })
            .style(Style::new().fg(ctx.state.theme.status.warning))
            .into()
    } else {
        Spinner::new()
            .spinner_style(SpinnerStyle::Dots)
            .label(if lost { "recreating" } else { "reconnecting" })
            .style(Style::new().fg(ctx.state.theme.status.warning))
            .label_style(fg_only(&ctx.state.theme.primary))
            .into()
    };
    modal
        .child(
            VStack::new()
                .height(Length::Auto)
                .padding((1, 0, 0, 0))
                // A modal with no focus target traps keys before root routing. Bind the same
                // actions the clickable hints use, including Esc, inside the modal's focus ring.
                .child(
                    KeyCapture::new()
                        .on_key(overlay_interceptor(ctx, &actions))
                        .key("session-connection-keys"),
                )
                .child(
                    HStack::new()
                        .height(Length::Auto)
                        .padding((0, 1))
                        .child(status),
                )
                .child(overlay_hints(ctx, &actions)),
        )
        .into()
}

/// The footer hint row only advertises keys that would actually act on the current state, so a
/// hint never lies. Enter is **switch** for a background-connected session, **restore** for a
/// resurrection snapshot, and **connect** when establishing a connection; **disconnect** closes
/// this client's attachment; **kill** destroys a live session and **forget** drops a snapshot or a
/// last-seen cache entry; **restart** recreates a live session.
///
/// Row actions for a restorable snapshot lead the bar (`restore`, `forget`) because those are the
/// verbs that apply to the highlighted recipe. A last-seen remote row leads with `connect` and
/// `forget` the same way: it is local cached knowledge, not a live server. Global picker actions
/// follow. Restart is omitted when there is no live server to recreate.
///
/// `ephemeral shell` is the exception that is deliberately *under*-advertised: `Ctrl+T` always
/// reaches this client's scratch session, but saying so is only worth a pill when the list cannot
/// say it already. With nothing to pick, Enter is free and carries it; with the scratch session
/// itself on the list, its own row is the obvious way to it. The label borrows the word the rows
/// use (`ephemeral`) so the hint and the session it lands on read as the same thing.
///
/// Every creating key acts on the active tab's host, which the tab strip names, so no label needs
/// to repeat it.
fn session_picker_actions(ctx: &Context<AppRoot>) -> Vec<OverlayAction> {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Vec::new();
    };
    let selected = selected_session(&ctx.state, picker);
    let restorable = selected.is_some_and(crate::ops::session::session_row_is_restorable);
    let last_seen = selected.is_some_and(crate::ops::session::session_row_is_last_seen);
    let mut actions = Vec::new();
    push_session_activation(ctx, picker, selected, restorable, last_seen, &mut actions);
    push_session_creation_actions(ctx, picker, &mut actions);
    push_session_management_actions(ctx, picker, selected, restorable, last_seen, &mut actions);
    actions
}

fn session_forget_action(ctx: &Context<AppRoot>, picker: &SessionPickerState) -> OverlayAction {
    OverlayAction::destructive(
        ctx,
        "forget",
        Msg::SessionPickerKillSelected,
        true,
        picker.pending_kill == Some(picker.selected),
    )
}

fn push_session_activation(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    selected: Option<&crate::session::discovery::DiscoveredSession>,
    restorable: bool,
    last_seen: bool,
    actions: &mut Vec<OverlayAction>,
) {
    if picker_list_is_empty(&ctx.state, picker) {
        actions.push(OverlayAction::new(
            "enter",
            session_creation_label(ctx, picker, "ephemeral shell"),
            Msg::SessionPickerEphemeral,
            true,
        ));
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
        actions.push(session_forget_action(ctx, picker));
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
        if last_seen {
            actions.push(session_forget_action(ctx, picker));
        }
    }
}

fn session_creation_label(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    action: &str,
) -> String {
    if picker.effective_tab() != crate::state::SessionPickerTab::All {
        return action.to_string();
    }
    let host = crate::ops::session::session_picker_creation_target(&ctx.state).map_or_else(
        || "Local".to_string(),
        |target| ctx.state.remote_target_label(&target),
    );
    format!("{action} on {host}")
}

fn push_session_creation_actions(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    actions: &mut Vec<OverlayAction>,
) {
    // Creation retains the browsing host during global search.
    actions.push(OverlayAction::new(
        "ctrl-n",
        session_creation_label(ctx, picker, "new"),
        Msg::SessionPickerCreateFromQuery,
        true,
    ));
    let show_ephemeral = !picker_list_is_empty(&ctx.state, picker)
        && crate::ops::session::held_ephemeral_session_in(
            &ctx.state,
            crate::ops::session::session_picker_creation_target(&ctx.state).as_ref(),
        )
        .is_none();
    actions.push(OverlayAction::new(
        "ctrl-t",
        session_creation_label(ctx, picker, "ephemeral shell"),
        Msg::SessionPickerEphemeral,
        show_ephemeral,
    ));
    if ctx.state.current().session_attached && ctx.state.is_ephemeral_session() {
        actions.push(OverlayAction::new(
            "ctrl-s",
            "name current",
            Msg::SessionPickerNameCurrent,
            true,
        ));
    }
    actions.push(OverlayAction::new(
        "ctrl-r",
        "remote hosts",
        Msg::SessionPickerRemoteHosts,
        true,
    ));
}

fn push_session_management_actions(
    ctx: &Context<AppRoot>,
    picker: &SessionPickerState,
    selected: Option<&crate::session::discovery::DiscoveredSession>,
    restorable: bool,
    last_seen: bool,
    actions: &mut Vec<OverlayAction>,
) {
    if let Some(entry) = selected
        && !restorable
        && !last_seen
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
        actions.push(OverlayAction::destructive(
            ctx,
            "kill",
            Msg::SessionPickerKillSelected,
            crate::ops::session::session_row_can_kill(entry),
            picker.pending_kill == Some(picker.selected),
        ));
    }
    if picker
        .effective_tab()
        .remote_target()
        .is_some_and(|target| crate::ops::session::host_can_disconnect(&ctx.state, target))
    {
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
fn selected_session<'a>(
    state: &crate::state::State,
    picker: &'a SessionPickerState,
) -> Option<&'a crate::session::discovery::DiscoveredSession> {
    let query_lower = picker.input.text().trim().to_ascii_lowercase();
    picker
        .entries
        .get(picker.selected)
        .filter(|entry| picker.in_tab(entry) && state.matches_session_query(entry, &query_lower))
}

/// Whether the active tab is showing no session at all — nothing discovered, or nothing left by the
/// query. There is then no row for Enter to activate, which is what frees it to start a shell.
fn picker_list_is_empty(state: &crate::state::State, picker: &SessionPickerState) -> bool {
    let query = picker.input.text().trim().to_ascii_lowercase();
    !picker
        .entries
        .iter()
        .any(|entry| picker.in_tab(entry) && state.matches_session_query(entry, &query))
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
    let mut reserve_discovered_gutter = false;
    // The palette's highlight is a position among the rows it draws, which on a tab is not the
    // row's index into every host's entries.
    let selected_position = picker
        .entries
        .iter()
        .take(picker.selected)
        .filter(|entry| picker.in_tab(entry) && ctx.state.matches_session_query(entry, &query))
        .count();
    for (index, entry) in
        picker.entries.iter().enumerate().filter(|(_, entry)| {
            picker.in_tab(entry) && ctx.state.matches_session_query(entry, &query)
        })
    {
        reserve_discovered_gutter |= statuses[index] != SessionConnectionStatus::Discovered;
        // Ephemeral sessions carry an ugly generated `eph-<pid>` name shown as "ephemeral" (they
        // stay reattachable - activation is by row index, not this label).
        let name = if entry.ephemeral {
            "ephemeral"
        } else {
            entry.name.as_str()
        };
        let label = if picker.effective_tab() == crate::state::SessionPickerTab::All {
            entry.remote_target.as_ref().map_or_else(
                || name.to_string(),
                |target| format!("{name}@{}", ctx.state.remote_target_label(target)),
            )
        } else {
            name.to_string()
        };
        // Host pages use bare names; All identifies remote destinations in the visible label.
        // The raw name and `name@host` remain searchable on every page.
        let mut aliases = Vec::new();
        if let Some(target) = &entry.remote_target {
            aliases.push(target.to_spec());
            aliases.push(format!("{}@{}", entry.name, target.to_spec()));
        }
        if entry.ephemeral {
            aliases.push(entry.name.clone());
        }
        if let Some(host) = entry.host.as_deref() {
            aliases.push(format!("{}@{host}", entry.name));
        }
        let we_hold = !matches!(statuses[index], SessionConnectionStatus::Discovered);
        let agents = crate::view::session_status::host_agent_label(&ctx.state, entry);
        entries.push(SearchEntry::Item(
            SearchItem::new(label, index)
                .aliases(aliases)
                .description(session_description(entry, we_hold, agents)),
        ));
    }
    // Say what is (not) there, nothing more: the footer already advertises `new ctrl+n`, and
    // repeating it in the body says the same thing twice in a longer sentence.
    let empty_text = if picker.first_in_tab().is_none() {
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

    let tabs = crate::ops::session::session_picker_tabs(&ctx.state);
    let mut overlay = OverlayPalette::new(
        "Sessions",
        session_picker_key(),
        Msg::CloseSessionPicker,
        64,
    )
    .entries(entries)
    .actions(session_picker_actions(ctx))
    .armed_row(pending_kill.or(pending_restart))
    .placeholder("Search sessions…")
    .initial_query(picker.input.text().to_string())
    .selected(Some(selected_position))
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
    .item_gutter(Arc::new(move |item: &SearchItem<usize>, _hl| {
        let status = *statuses.get(item.value)?;
        session_status_gutter(status, status_styles, reserve_discovered_gutter)
    }));
    if let Some(render_item) = render_item {
        overlay = overlay.render_item(render_item);
    }
    // A nonempty query temporarily highlights All when host navigation is visible.
    if tabs.len() > 1 {
        let active = tabs
            .iter()
            .position(|tab| *tab == picker.effective_tab())
            .unwrap_or(0);
        let labels = tabs.iter().map(|tab| tab.label(&ctx.state)).collect();
        overlay = overlay.tabs(OverlayTabs::new(labels, active, Msg::SessionPickerTab).with_all());
    }
    if let Some(target) = picker.effective_tab().remote_target() {
        overlay = overlay.header_right(remote_tab_status(ctx, target));
    }
    overlay.render(ctx)
}

/// A remote tab's status, in the frame's right header: what this client's link to the host is
/// doing.
///
/// It is the *host's* news rather than any row's, and it has to be said at all because this picker
/// never probes: it replays remembered rows for every host it holds no attachment on (see
/// [`crate::ops::session::discovery::push_cached_known_remote_rows`]). Without it a tab of those
/// looks exactly like a tab of live ones. Same vocabulary the sidebar badges its hosts with.
///
/// The rows are passed as *no* evidence of reachability, deliberately: a memory of a session is not
/// proof that anything answers there now, and must not talk the status into "reached".
fn remote_tab_status(
    ctx: &Context<AppRoot>,
    target: &crate::session::remote::RemoteTarget,
) -> String {
    let status = crate::view::session_status::host_connection_status(&ctx.state, target, false);
    crate::view::session_status::host_status_label(status).to_string()
}

/// A picker row's right-aligned line. `agents` is what a host monitor knows about the session's
/// agents, already rendered by [`crate::view::session_status::host_agent_label`]; it goes last,
/// after the facts about the session itself.
fn session_description(
    entry: &crate::session::discovery::DiscoveredSession,
    we_hold: bool,
    agents: Option<String>,
) -> ItemDescription {
    let description = session_status_description(entry, we_hold);
    picker_description(match agents {
        Some(agents) => format!("{description} · {agents}"),
        None => description,
    })
}

fn session_status_description(
    entry: &crate::session::discovery::DiscoveredSession,
    we_hold: bool,
) -> String {
    use crate::session::discovery::DiscoveredSessionStatus;
    match &entry.status {
        DiscoveredSessionStatus::Running { panes, clients, .. } => {
            let panes_label = panes_label(*panes);
            // `clients` counts every client attached to the server, ours included. Drop our own
            // connection (current or retained in the background) so this reports only *other* people
            // sharing the session — the ones a new attach would join.
            let others = clients.saturating_sub(u32::from(we_hold));
            let mut label = match others {
                0 => panes_label,
                1 => format!("{panes_label} · shared with 1 other"),
                count => format!("{panes_label} · shared with {count} others"),
            };
            if let Some(profile) = &entry.origin.profile {
                label.push_str(&format!(" · from {profile}"));
            }
            label
        }
        // A remembered row from a host nothing has reached this sweep. The pane count is still the
        // most useful thing to say about it, but on its own it is exactly what a live session says,
        // so it goes out qualified. Same words the sidebar's cached rows have always used.
        DiscoveredSessionStatus::LastSeen { panes } => {
            format!("{} · last seen", panes_label(*panes))
        }
        DiscoveredSessionStatus::Restorable => "restorable".to_string(),
        DiscoveredSessionStatus::Busy => "busy".to_string(),
        DiscoveredSessionStatus::Unknown => "unavailable".to_string(),
    }
}

fn panes_label(panes: usize) -> String {
    if panes == 1 {
        "1 pane".to_string()
    } else {
        format!("{panes} panes")
    }
}
