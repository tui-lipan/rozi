use super::*;

use crate::ops::sharing::{search_items, selected_grant};
use crate::update::sharing::Msg as SharingMsg;

/// Manage which other OS accounts may attach to this session and their roles.
pub(crate) fn sharing_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(state) = ctx.state.sharing.as_ref() else {
        return Text::new("").into();
    };
    let rows = search_items(&state.grants);
    let entries: Vec<_> = rows.iter().cloned().map(SearchEntry::Item).collect();
    let selected_item = selected_grant(state);
    let selected_uid = selected_item.map(|grant| grant.uid);
    let selected_index =
        selected_uid.and_then(|uid| state.grants.iter().position(|grant| grant.uid == uid));
    let armed = state
        .pending_revoke
        .filter(|uid| selected_uid == Some(*uid));

    let query = state.query.trim();
    let empty_text = if state.loading && state.grants.is_empty() {
        "Loading…".to_string()
    } else if state.grants.is_empty() {
        "Not shared".to_string()
    } else {
        format!("No shares match `{query}`")
    };

    let actions = vec![
        OverlayAction::new(
            "enter",
            "role",
            selected_uid
                .map(|uid| Msg::Sharing(SharingMsg::CycleRole(uid)))
                .unwrap_or(Msg::Sharing(SharingMsg::Close)),
            selected_uid.is_some(),
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-n",
            "new share",
            Msg::Sharing(SharingMsg::GrantFromQuery),
            true,
        ),
        OverlayAction::destructive(
            ctx,
            "unshare",
            Msg::Sharing(SharingMsg::RevokeSelected),
            selected_uid.is_some(),
            armed.is_some(),
        ),
    ];

    let fallback = ctx.link().key_handler(|key_event| {
        key_event
            .is(KeyCode::Esc)
            .then_some(Msg::Sharing(SharingMsg::Close))
    });

    let mut overlay = OverlayPalette::new(
        "Sharing",
        sharing_key(),
        Msg::Sharing(SharingMsg::Close),
        64,
    )
    .entries(entries)
    .description_wins(Length::Percent(40))
    .actions(actions)
    .armed_row(armed)
    .placeholder("Account name or uid…")
    .empty_text(empty_text)
    .initial_query(state.query.clone())
    .reset_key(state.query_epoch.to_string())
    .selected(selected_index)
    .fallback_interceptor(fallback)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::Sharing(SharingMsg::QueryChanged(query.to_string()))),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<u32>| Msg::Sharing(SharingMsg::Select(event.item.value))),
    )
    .on_activate(
        ctx.link().callback(|event: SearchEvent<u32>| {
            Msg::Sharing(SharingMsg::CycleRole(event.item.value))
        }),
    );

    if let Some(session_name) = ctx.state.current().session_name.as_deref() {
        overlay = overlay.header_right(crate::view::vocabulary::session_name(session_name));
    }

    overlay.render(ctx)
}
