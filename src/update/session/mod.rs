use tui_lipan::prelude::*;

pub(crate) mod control_replies;
pub(crate) mod lease;
pub(crate) mod lifecycle;
pub(crate) mod pane_events;
pub(crate) mod presentation;
pub(crate) mod runtime_events;
pub(crate) mod status;

pub(crate) use control_replies::{
    agent_report_result, drag_changed, flush_layout_commit, layout_committed, layout_rejected,
    ping, replay_input_deadline, spawn_reply_deadline, spawn_result,
};
pub(crate) use lease::{
    clients_changed, control_declined, control_requested, controller_changed, evicted,
};
pub(crate) use lifecycle::{
    attach_failed, attached, connected, disconnected, error, lost, origin_set, renamed,
    transport_failed,
};
pub(crate) use pane_events::{
    exited, flush_pane_resizes, output, pane_logging_changed, pane_reset, resized,
};
pub(crate) use status::pane_runtime_changed;

pub(crate) fn session_event(
    ctx: &mut Context<crate::app::AppRoot>,
    epoch: u64,
    event: crate::session::server::events::SessionEvent,
) -> Update {
    let Some(attachment) = ctx.state.attachment_for_epoch(epoch) else {
        return Update::none();
    };
    if attachment
        .session_instance
        .as_ref()
        .is_some_and(|instance| instance != &event.session_instance)
    {
        return Update::none();
    }
    let is_controller = attachment.is_controller();
    let client_id = attachment
        .shared
        .as_ref()
        .map(|shared| shared.client_id.to_string());
    let scope = crate::events::EventScope {
        host: crate::state::HostKey::of(attachment.remote_target.as_ref()),
        session: attachment.session_instance.clone(),
    };
    let remote_host = attachment.remote_host.clone();
    let mut fields: std::collections::BTreeMap<String, String> = event.data;
    if matches!(
        event.kind,
        crate::events::EventKind::PaneExited
            | crate::events::EventKind::PaneStatusChanged
            | crate::events::EventKind::PaneTitleChanged
            | crate::events::EventKind::PaneCwdChanged
            | crate::events::EventKind::AgentStateChanged
            | crate::events::EventKind::ForegroundChanged
            | crate::events::EventKind::Bell
    ) {
        let is_focused = fields
            .get("pane")
            .and_then(|p| p.parse().ok())
            .is_some_and(|id| {
                epoch == ctx.state.runtime_epoch
                    && ctx.state.focused_pane_target() == Some((id, false))
            });
        fields.insert("focused".to_string(), is_focused.to_string());
    }
    if event.kind == crate::events::EventKind::ControllerChanged {
        let is_self = client_id.as_deref().is_some_and(|id| {
            fields
                .get("controller")
                .is_some_and(|c| !c.is_empty() && c == id)
        });
        fields.insert("self_controller".to_string(), is_self.to_string());
    }
    if event.kind == crate::events::EventKind::LayoutChanged
        && let Some(author_client) = fields.remove("author_client")
        && author_client != "0"
        && client_id.as_deref() == Some(&author_client)
    {
        fields.insert("author".to_string(), "self".to_string());
    }

    let presentation = if event.kind == crate::events::EventKind::SessionNotification
        && let Some(notification) = notification_from_fields(&fields, event.seq)
    {
        session_notifications(
            ctx,
            epoch,
            event.session_instance.clone(),
            fields.get("session").cloned().unwrap_or_default(),
            vec![notification],
        )
    } else {
        Update::none()
    };
    let client_event = crate::events::Event::from_map(event.kind, fields);

    ctx.state
        .event_hub
        .publish_scoped(&client_event, Some(&scope));

    if is_controller {
        crate::events::run_hooks_for_scope(&ctx.state, &client_event, remote_host.as_deref());
    }

    presentation
}

#[cfg(test)]
mod tests;

fn notification_from_fields(
    fields: &std::collections::BTreeMap<String, String>,
    seq: u64,
) -> Option<crate::control::SessionNotification> {
    Some(crate::control::SessionNotification {
        id: fields.get("id")?.clone(),
        seq,
        level: crate::control::NotifyLevel::parse_cli(fields.get("level")?).ok()?,
        title: fields
            .get("title")
            .filter(|title| !title.is_empty())
            .cloned(),
        message: fields.get("message")?.clone(),
    })
}

pub(crate) fn session_actions(
    ctx: &mut Context<crate::app::AppRoot>,
    epoch: u64,
    actions: Vec<crate::control::PaletteAction>,
) -> Update {
    let Some(attachment) = ctx.state.attachment_for_epoch_mut(epoch) else {
        return Update::none();
    };
    attachment.session_actions = actions;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn session_notifications(
    ctx: &mut Context<crate::app::AppRoot>,
    epoch: u64,
    instance: crate::session::protocol::SessionInstanceId,
    session: String,
    notifications: Vec<crate::control::SessionNotification>,
) -> Update {
    if ctx
        .state
        .attachment_for_epoch(epoch)
        .is_none_or(|attachment| attachment.session_instance.as_ref() != Some(&instance))
    {
        return Update::none();
    }
    let mut shown = false;
    for notification in notifications {
        let cursor = ctx
            .state
            .session_notification_cursors
            .entry(instance.clone())
            .or_default();
        if notification.seq <= *cursor {
            continue;
        }
        *cursor = notification.seq;
        shown = true;
        let message = format!("{session}: {}", notification.message);
        match notification.level {
            crate::control::NotifyLevel::Info => crate::pane::pty_events::notify_info(ctx, message),
            crate::control::NotifyLevel::Error => crate::pane::pty_events::notify_error(
                ctx,
                notification
                    .title
                    .unwrap_or_else(|| "Session notification".into()),
                message,
            ),
        };
    }
    if shown {
        Update::full()
    } else {
        Update::none()
    }
}
