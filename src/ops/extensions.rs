use std::collections::HashSet;

use tui_lipan::prelude::*;

use crate::AppRoot;

pub(crate) fn subscription_opened(
    ctx: &mut Context<AppRoot>,
    id: u64,
    provenance: crate::config::ExtensionProvenance,
    cancel: std::sync::mpsc::SyncSender<()>,
    reply: std::sync::mpsc::Sender<bool>,
) -> Update {
    let active = crate::config::provenance_is_active(&ctx.state.extension_generations, &provenance);
    if active {
        ctx.state.extension_subscriptions.insert(
            id,
            crate::state::ExtensionSubscriptionState {
                extension: provenance,
                cancel,
            },
        );
    }
    let _ = reply.send(active);
    Update::none()
}

pub(crate) fn subscription_closed(ctx: &mut Context<AppRoot>, id: u64) -> Update {
    ctx.state.extension_subscriptions.remove(&id);
    Update::none()
}

pub(crate) fn unload(ctx: &mut Context<AppRoot>, retired: &HashSet<String>) {
    let ids: Vec<_> = ctx
        .state
        .extension_subscriptions
        .iter()
        .filter_map(|(stream_id, stream)| {
            retired.contains(&stream.extension.id).then_some(*stream_id)
        })
        .collect();
    for id in ids {
        if let Some(stream) = ctx.state.extension_subscriptions.remove(&id) {
            let _ = stream.cancel.try_send(());
        }
    }
}
