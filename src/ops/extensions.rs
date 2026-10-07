use std::collections::HashSet;

use tui_lipan::prelude::*;

use crate::AppRoot;

pub(crate) fn subscription_opened(
    ctx: &mut Context<AppRoot>,
    id: u64,
    provenance: crate::config::ExtensionProvenance,
    worker: Option<crate::state::WorkerId>,
    cancel: std::sync::mpsc::SyncSender<()>,
    reply: std::sync::mpsc::Sender<
        std::result::Result<Option<crate::state::WorkerBinding>, crate::control::ControlResponse>,
    >,
) -> Update {
    let binding =
        match worker.map(|worker| crate::ops::extension_workers::live_worker(&ctx.state, worker)) {
            None => None,
            Some(Ok(worker)) => Some(worker.binding.clone()),
            Some(Err(response)) => {
                let _ = reply.send(Err(response));
                return Update::none();
            }
        };
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
    let _ = reply.send(if active {
        Ok(binding)
    } else {
        Err(crate::control::ControlResponse::error_with(
            crate::control::ControlErrorCode::ExtensionInactive,
            "extension generation is not active",
        ))
    });
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
