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
    let mut fields: std::collections::BTreeMap<String, String> = event.data;
    if matches!(
        event.kind,
        crate::events::EventKind::PaneExited
            | crate::events::EventKind::PaneStatusChanged
            | crate::events::EventKind::AgentStateChanged
            | crate::events::EventKind::ForegroundChanged
    ) {
        let is_focused = fields
            .get("pane")
            .and_then(|p| p.parse().ok())
            .is_some_and(|id| ctx.state.current().focused_pane == Some(id));
        fields.insert("focused".to_string(), is_focused.to_string());
    }

    let client_event = crate::events::Event::from_map(event.kind, fields);
    let (scope, remote_host) = if let Some(attachment) = ctx.state.attachment_for_epoch(epoch) {
        (
            crate::events::EventScope {
                host: crate::state::HostKey::of(attachment.remote_target.as_ref()),
                session: attachment.session_instance.clone(),
            },
            attachment.remote_host.clone(),
        )
    } else {
        (
            crate::events::EventScope::of(&ctx.state, &client_event),
            ctx.state.current().remote_host.clone(),
        )
    };

    ctx.state
        .event_hub
        .publish_scoped(&client_event, Some(&scope));

    if ctx.state.is_controller() {
        crate::events::run_hooks_for_scope(&ctx.state, &client_event, remote_host.as_deref());
    }

    Update::none()
}

#[cfg(test)]
mod tests;
