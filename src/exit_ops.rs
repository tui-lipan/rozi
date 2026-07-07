use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim;
use crate::pane_lifecycle::{begin_close_pane, close_pane_state, prune_closed_batch_command};
use crate::profiles;
use crate::pty_events::info_toast;
use crate::state::PendingDestructive;

fn clear_pending(ctx: &mut Context<HyprmuxApp>) {
    ctx.state.pending_destructive = None;
}

pub(crate) fn detach(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending(ctx);
    if ctx.state.session_attached {
        return crate::session_ops::detach_current_session(ctx);
    }
    profiles::persist_session_on_detach(&ctx.state);
    ctx.quit();
    Update::none()
}

pub(crate) fn quit_client(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending(ctx);
    profiles::persist_session_if_enabled(&ctx.state);
    ctx.quit();
    Update::none()
}

pub(crate) fn close_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let needs_confirm = crate::pane_lifecycle::find_pane(&ctx.state, id)
        .is_some_and(|pane| !pane.closing && pane.terminal.is_running());

    if needs_confirm && ctx.state.pending_destructive != Some(PendingDestructive::ClosePane(id)) {
        ctx.state.pending_destructive = Some(PendingDestructive::ClosePane(id));
        ctx.toast()
            .push(info_toast("Press close again to kill the running pane"));
        return Update::full();
    }

    clear_pending(ctx);
    begin_close_pane(ctx, id, ctx.state.config.animations)
}

pub(crate) fn kill_workspace(ctx: &mut Context<HyprmuxApp>) -> Update {
    let workspace_index = ctx.state.active_workspace;
    let pane_count = ctx.state.workspaces[workspace_index]
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .count();
    if pane_count == 0 {
        ctx.toast().push(info_toast("Workspace is already empty"));
        return Update::full();
    }

    if ctx.state.pending_destructive != Some(PendingDestructive::KillWorkspace(workspace_index)) {
        ctx.state.pending_destructive = Some(PendingDestructive::KillWorkspace(workspace_index));
        let label = workspace_index + 1;
        ctx.toast().push(info_toast(format!(
            "Press kill-workspace again to close all {pane_count} pane(s) on workspace {label}"
        )));
        return Update::full();
    }

    clear_pending(ctx);
    let animations = ctx.state.config.animations;
    let pane_ids: Vec<_> = ctx.state.workspaces[workspace_index]
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .map(|pane| pane.id)
        .collect();

    let targets: Vec<_> = pane_ids
        .into_iter()
        .filter_map(|id| close_pane_state(ctx, id).map(|generation| (id, generation)))
        .collect();

    if targets.is_empty() {
        return Update::full();
    }

    Update::with_command(prune_closed_batch_command(
        ctx.state.runtime_epoch,
        targets,
        anim::close_delay(animations),
    ))
}

pub(crate) fn kill_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.session_attached {
        ctx.toast()
            .push(info_toast("Not attached to a named session"));
        return Update::full();
    }

    let session_name = ctx
        .state
        .session_name
        .clone()
        .unwrap_or_else(|| "session".to_string());

    if ctx.state.pending_destructive != Some(PendingDestructive::KillSession) {
        ctx.state.pending_destructive = Some(PendingDestructive::KillSession);
        ctx.toast().push(info_toast(format!(
            "Press kill-session again to shut down `{session_name}`"
        )));
        return Update::full();
    }

    clear_pending(ctx);
    if let Some(client) = ctx.state.session_client.clone() {
        client.shutdown();
    }
    ctx.quit();
    Update::none()
}
