//! In-app *Sharing* picker operations and `AttachedControl` reply handling.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::control::{ControlCommand, ControlRequest, ControlResponse, ShareGrantInfo, ShareState};
use crate::session::protocol::SessionRole;
use crate::state::{AttachedReply, SharingRequestKind, SharingState};

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.sharing = Some(SharingState::new());
    crate::ops::focus::request_sharing_focus(ctx);
    send_control(ctx, ControlCommand::ShareList, SharingRequestKind::List);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.sharing.take().is_none() {
        return Update::none();
    }
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(sharing) = ctx.state.sharing.as_mut() {
        sharing.query = query;
        sharing.pending_revoke = None;
    }
    Update::full()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, uid: u32) -> Update {
    if let Some(sharing) = ctx.state.sharing.as_mut() {
        if sharing.selected_uid != Some(uid) {
            sharing.pending_revoke = None;
        }
        sharing.selected_uid = Some(uid);
    }
    Update::full()
}

pub(crate) fn grant_from_query(ctx: &mut Context<AppRoot>) -> Update {
    let Some(sharing) = ctx.state.sharing.as_mut() else {
        return Update::none();
    };
    sharing.pending_revoke = None;
    let user = sharing.query.trim().to_string();
    if user.is_empty() {
        crate::pane::pty_events::notify_info(
            ctx,
            "Type an account name or numeric id to share with",
        );
        return Update::full();
    }
    send_control(
        ctx,
        ControlCommand::ShareGrant {
            user,
            role: SessionRole::Viewer,
        },
        SharingRequestKind::GrantFromQuery,
    );
    Update::full()
}

pub(crate) fn cycle_role(ctx: &mut Context<AppRoot>, uid: u32) -> Update {
    let Some(sharing) = ctx.state.sharing.as_mut() else {
        return Update::none();
    };
    sharing.pending_revoke = None;
    let Some(grant) = sharing.grants.iter().find(|grant| grant.uid == uid) else {
        return Update::none();
    };
    let user = grant.user.clone();
    let role = next_role(grant.role);
    sharing.selected_uid = Some(uid);
    send_control(
        ctx,
        ControlCommand::ShareGrant { user, role },
        SharingRequestKind::CycleRole,
    );
    Update::full()
}

pub(crate) fn revoke_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some(sharing) = ctx.state.sharing.as_ref() else {
        return Update::none();
    };
    let Some(grant) = selected_grant(sharing).cloned() else {
        return Update::none();
    };
    if sharing.pending_revoke == Some(grant.uid) {
        if let Some(sharing) = ctx.state.sharing.as_mut() {
            sharing.pending_revoke = None;
        }
        send_control(
            ctx,
            ControlCommand::ShareRevoke {
                user: Some(grant.user),
            },
            SharingRequestKind::Revoke,
        );
        return Update::full();
    }
    if let Some(sharing) = ctx.state.sharing.as_mut() {
        sharing.pending_revoke = Some(grant.uid);
    }
    crate::ops::confirm::arm(ctx)
}

pub(crate) fn refresh_if_open(ctx: &mut Context<AppRoot>) {
    if ctx.state.sharing.is_some() {
        send_control(ctx, ControlCommand::ShareList, SharingRequestKind::List);
    }
}

pub(crate) fn answered(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    kind: SharingRequestKind,
    response: ControlResponse,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if !response.ok {
        if let Some(sharing) = ctx.state.sharing.as_mut() {
            sharing.loading = false;
        }
        let title = match kind {
            SharingRequestKind::List => "Could not load sharing",
            SharingRequestKind::GrantFromQuery | SharingRequestKind::CycleRole => "Share failed",
            SharingRequestKind::Revoke => "Unshare failed",
        };
        crate::pane::pty_events::notify_error(
            ctx,
            title,
            response
                .error
                .unwrap_or_else(|| "request failed".to_string()),
        );
        return Update::full();
    }
    let Some(share_state) = response
        .data
        .and_then(|data| serde_json::from_value::<ShareState>(data).ok())
    else {
        if let Some(sharing) = ctx.state.sharing.as_mut() {
            sharing.loading = false;
        }
        crate::pane::pty_events::notify_error(
            ctx,
            "Sharing reply invalid",
            "the session server returned an unexpected response",
        );
        return Update::full();
    };

    if let Some(sharing) = ctx.state.sharing.as_mut() {
        sharing.loading = false;
        if kind == SharingRequestKind::GrantFromQuery {
            let query = sharing.query.trim().to_string();
            let parsed_uid = query.parse::<u32>().ok();
            if let Some(matched) = share_state
                .grants
                .iter()
                .find(|grant| grant.user == query || parsed_uid == Some(grant.uid))
            {
                sharing.selected_uid = Some(matched.uid);
            }
            sharing.query.clear();
            sharing.query_epoch = sharing.query_epoch.wrapping_add(1);
        }
        sharing.grants = share_state.grants;
        if sharing
            .selected_uid
            .is_none_or(|uid| !sharing.grants.iter().any(|grant| grant.uid == uid))
        {
            sharing.selected_uid = sharing.grants.first().map(|grant| grant.uid);
        }
        if sharing
            .pending_revoke
            .is_some_and(|uid| !sharing.grants.iter().any(|grant| grant.uid == uid))
        {
            sharing.pending_revoke = None;
        }
    }
    Update::full()
}

pub(crate) fn next_role(role: SessionRole) -> SessionRole {
    match role {
        SessionRole::Viewer => SessionRole::Editor,
        SessionRole::Editor => SessionRole::Controller,
        SessionRole::Controller | SessionRole::Owner => SessionRole::Viewer,
    }
}

pub(crate) fn search_items(grants: &[ShareGrantInfo]) -> Vec<SearchItem<u32>> {
    grants
        .iter()
        .map(|grant| {
            let description = if grant.connected > 0 {
                format!("{} · {} attached", grant.role.as_str(), grant.connected)
            } else {
                grant.role.as_str().to_string()
            };
            SearchItem::new(grant.user.clone(), grant.uid)
                .aliases([grant.uid.to_string()])
                .description(ItemDescription::new().right(description))
        })
        .collect()
}

pub(crate) fn selected_grant(sharing: &SharingState) -> Option<&ShareGrantInfo> {
    let rows = search_items(&sharing.grants);
    let visible = tui_lipan::rank_search_palette_indices_with_mode(
        &rows,
        &sharing.query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    );
    if visible.is_empty() {
        return None;
    }
    let selected_idx = sharing
        .selected_uid
        .and_then(|uid| sharing.grants.iter().position(|grant| grant.uid == uid))
        .filter(|idx| visible.contains(idx))
        .unwrap_or(visible[0]);
    sharing.grants.get(selected_idx)
}

fn send_control(ctx: &mut Context<AppRoot>, command: ControlCommand, kind: SharingRequestKind) {
    crate::ops::attached_control::send(
        ctx,
        ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            extension: None,
            caller_context: None,
            credential: None,
        },
        AttachedReply::Sharing(kind),
    );
}

#[cfg(test)]
mod tests {
    use tui_lipan::TestBackend;

    use super::*;
    use crate::control::ControlResponse;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    #[test]
    fn share_state_reply_populates_rows_enter_cycles_role_and_guest_cannot_open() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, outbound) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().session_client = Some(client);
            }
            let epoch = backend.state().runtime_epoch;

            // Owner session has OpenSharing enabled; guest session disables it with a reason.
            assert_eq!(
                crate::commands::action_disabled_reason(Action::OpenSharing, backend.state()),
                None
            );
            backend.state_mut().current_mut().session_name = Some("alice/dev".into());
            assert_eq!(
                crate::commands::action_disabled_reason(Action::OpenSharing, backend.state())
                    .as_deref(),
                Some("only the session owner can share it")
            );
            backend.state_mut().current_mut().session_name = Some("dev".into());

            backend
                .dispatch(crate::Msg::RunAction(Action::OpenSharing))
                .unwrap();
            assert!(backend.state().sharing.is_some());

            let sent: Vec<_> = outbound
                .try_iter()
                .filter_map(|msg| match msg {
                    ClientOutbound::Control(ClientMessage::AttachedControl {
                        request_id,
                        request,
                    }) => Some((request_id, request.command)),
                    _ => None,
                })
                .collect();
            assert_eq!(sent.len(), 1);
            assert!(matches!(sent[0].1, ControlCommand::ShareList));

            let share_state = ShareState {
                endpoint: Some("/tmp/rozi-shared-1000/session-dev.sock".into()),
                grants: vec![ShareGrantInfo {
                    user: "bob".into(),
                    uid: 1001,
                    role: SessionRole::Editor,
                    connected: 1,
                }],
            };
            backend
                .dispatch(crate::Msg::SessionAttachedControlResult {
                    epoch,
                    request_id: sent[0].0,
                    response: ControlResponse::ok(share_state),
                })
                .unwrap();

            let sharing = backend.state().sharing.as_ref().unwrap();
            assert!(!sharing.loading);
            assert_eq!(sharing.grants.len(), 1);
            assert_eq!(sharing.grants[0].user, "bob");
            assert_eq!(sharing.grants[0].role, SessionRole::Editor);

            backend
                .dispatch(crate::Msg::Sharing(crate::update::sharing::Msg::CycleRole(
                    1001,
                )))
                .unwrap();
            let grant_cmds: Vec<_> = outbound
                .try_iter()
                .filter_map(|msg| match msg {
                    ClientOutbound::Control(ClientMessage::AttachedControl { request, .. }) => {
                        Some(request.command)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                grant_cmds,
                vec![ControlCommand::ShareGrant {
                    user: "bob".into(),
                    role: SessionRole::Controller,
                }]
            );
        });
    }
}
