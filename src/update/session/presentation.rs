use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::protocol::PresentationAck;

pub(crate) fn presented(ctx: &mut Context<AppRoot>, reports: Vec<PresentedTerminal>) -> Update {
    let attachment = ctx.state.current_mut();
    let mut acks = Vec::with_capacity(reports.len());
    for report in reports {
        if let Some(binding) = attachment
            .presentation_bindings
            .get_mut(&report.stable_terminal_id)
            && report.token > binding.acknowledged
            && report.token <= binding.output.sequence
        {
            binding.acknowledged = report.token;
            acks.push(PresentationAck {
                sequence: report.token,
                ..binding.output
            });
        }
    }
    let (scratch, session): (Vec<_>, Vec<_>) = acks.into_iter().partition(|ack| {
        ack.local
            && ctx
                .state
                .scratch
                .panes
                .iter()
                .any(|pane| pane.id == ack.pane_id)
    });
    if let Some(client) = &ctx.state.current().session_client {
        client.acknowledge_presentations(session);
    }
    if let Some(client) = ctx.state.scratch_client() {
        client.acknowledge_presentations(scratch);
    }
    Update::none()
}

pub(crate) fn sync_interest(ctx: &mut Context<AppRoot>) {
    use crate::session::protocol::PanePresentationInterest;
    use std::collections::{HashMap, HashSet};

    let attachment = ctx.state.current_mut();
    let Some(client) = attachment.session_client.as_ref() else {
        return;
    };
    if !attachment
        .presentation_connection
        .as_ref()
        .is_some_and(|token| token.is(client))
    {
        attachment.presentation_connection = Some(client.connection_token());
        attachment.presentation_interest.clear();
        for progress in attachment.presentation_bindings.values_mut() {
            progress.acknowledged = 0;
        }
    }
    let focused = ctx.state.focused_pane();
    let rate = ctx.state.config.frame_rate;
    let mut current = HashMap::new();
    let mut terminals = HashSet::new();
    for (pane, local) in ctx
        .state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.panes)
        .map(|pane| (pane, false))
        .chain(ctx.state.popup.iter().map(|pane| (pane, true)))
        .chain(ctx.state.scratch.panes.iter().map(|pane| (pane, true)))
    {
        terminals.insert(pane.terminal.stable_terminal_id());
        current.insert(
            (pane.id, local),
            PanePresentationInterest {
                local,
                pane_id: pane.id,
                generation: pane.pty_generation,
                visible: if local && crate::scratchpad::contains(&ctx.state, pane.id) {
                    ctx.state.scratch_visible
                } else {
                    ctx.state.pane_is_rendered(pane.id)
                },
                focused: focused == Some(pane.id),
                frames_per_second: rate,
            },
        );
    }
    let attachment = ctx.state.current_mut();
    let mut changed = Vec::new();
    for (id, interest) in &current {
        if attachment.presentation_interest.get(id) != Some(interest) {
            changed.push(*interest);
        }
    }
    for (id, old) in &attachment.presentation_interest {
        if !current.contains_key(id) && old.visible {
            changed.push(PanePresentationInterest {
                visible: false,
                focused: false,
                ..*old
            });
        }
    }
    attachment
        .presentation_bindings
        .retain(|id, _| terminals.contains(id));
    attachment.presentation_interest = current;
    changed.sort_by_key(|pane| pane.pane_id);
    let (scratch, session): (Vec<_>, Vec<_>) = changed.into_iter().partition(|interest| {
        interest.local
            && ctx
                .state
                .scratch
                .panes
                .iter()
                .any(|pane| pane.id == interest.pane_id)
    });
    if let Some(client) = &ctx.state.current().session_client {
        client.set_presentation_interest(session);
    }
    if let Some(client) = ctx.state.scratch_client() {
        client.set_presentation_interest(scratch);
    }
}
