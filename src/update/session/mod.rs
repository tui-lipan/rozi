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

    let client_event = crate::events::Event::from_map(event.kind, fields);

    ctx.state
        .event_hub
        .publish_scoped(&client_event, Some(&scope));

    if is_controller {
        crate::events::run_hooks_for_scope(&ctx.state, &client_event, remote_host.as_deref());
    }

    Update::none()
}

#[cfg(test)]
mod tests;
