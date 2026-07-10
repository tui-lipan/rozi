use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim;
use crate::pane_lifecycle::{begin_close_pane, close_pane_state, prune_closed_batch_command};
use crate::profiles;
use crate::pty_events::{confirm_toast, info_toast};
use crate::state::{PendingDestructive, PendingDestructiveConfirmation, State};

/// How long a destructive action stays armed after its first press. The confirm toast is shown
/// for the same duration, so the toast disappearing means the confirmation expired.
pub(crate) const CONFIRM_WINDOW_SECS: f64 = 3.0;

fn clear_pending(ctx: &mut Context<HyprmuxApp>) {
    if let Some(pending) = ctx.state.pending_destructive.take() {
        ctx.toast().dismiss(pending.toast_id);
    }
}

/// True when `pending` was armed by an earlier press and is still within the confirm window
/// (consuming it); otherwise (re-)arms it and returns false.
fn confirm_second_press(
    ctx: &mut Context<HyprmuxApp>,
    pending: PendingDestructive,
    toast: Toast,
) -> bool {
    if let Some(armed) = ctx.state.pending_destructive.take() {
        ctx.toast().dismiss(armed.toast_id);
        if armed.action == pending
            && armed.armed_at.elapsed() <= Duration::from_secs_f64(CONFIRM_WINDOW_SECS)
        {
            return true;
        }
    }

    let toast_id = ctx.toast().push(toast);
    ctx.state.pending_destructive = Some(PendingDestructiveConfirmation {
        action: pending,
        armed_at: Instant::now(),
        toast_id,
    });
    false
}

/// Leave the TUI while keeping the session server running for later reattach (tmux-style detach).
/// The server already holds the authoritative layout from live commits; detach mirrors it to disk
/// so a fresh launch can restore it even after the server is gone.
///
/// Detaching an *anonymous* ephemeral session is contradictory (there is no name to reattach by),
/// so an attached ephemeral session first prompts for a name; naming it turns the detach into a
/// durable named detach (see [`crate::session_ops::apply_rename_session`]). A named session (or one
/// with no live client to rename) detaches immediately.
pub(crate) fn detach(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending(ctx);
    if ctx.state.session_attached
        && ctx.state.is_ephemeral_session()
        && ctx.state.session_client.is_some()
    {
        return crate::session_ops::open_rename_for_detach(ctx);
    }
    if let Some(client) = ctx.state.session_client.clone() {
        // The server is layout-authoritative from live commits, so there is nothing to push on
        // detach; just release the connection. Disk autosave still mirrors the layout below.
        client.detach();
    }
    profiles::persist_session_on_detach(&ctx.state);
    ctx.quit();
    Update::none()
}

/// Whether any tiled/floating pane still has a running process. Used to decide whether quitting
/// an ephemeral session (which shuts the server down and kills its PTYs) warrants a confirmation.
fn any_pane_live(state: &State) -> bool {
    state
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .any(|pane| !pane.closing && pane.terminal.is_running())
}

/// Quit the client. An ephemeral session is shut down (its PTYs die with it); a named session is
/// left running so it can be reattached later.
///
/// When `confirmations_enabled` and `[confirm].quit_ephemeral` are both set, quitting an
/// ephemeral session that still has a live pane routes through the shared confirm flow (arm on
/// the first press, quit on a second press within the confirm window) so an accidental `q`
/// doesn't tear down running work. A named session, a session with no live pane, or the flag
/// being off quits immediately as before.
pub(crate) fn quit_client(ctx: &mut Context<HyprmuxApp>, confirmations_enabled: bool) -> Update {
    if confirmations_enabled
        && ctx.state.config.confirm.quit_ephemeral
        && ctx.state.is_ephemeral_session()
        && any_pane_live(&ctx.state)
        && !confirm_second_press(
            ctx,
            PendingDestructive::Quit,
            confirm_toast(&ctx.state.theme, "Again to quit and close panes"),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    if ctx.state.is_ephemeral_session()
        && let Some(client) = ctx.state.session_client.clone()
    {
        client.shutdown();
    }
    profiles::persist_session_if_enabled(&ctx.state);
    ctx.quit();
    Update::none()
}

pub(crate) fn close_focused_pane_with_confirmation(
    ctx: &mut Context<HyprmuxApp>,
    confirmations_enabled: bool,
) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let needs_confirm = confirmations_enabled
        && ctx.state.config.confirm.close_pane
        && crate::pane_lifecycle::find_pane(&ctx.state, id)
            .is_some_and(|pane| !pane.closing && pane.terminal.is_running());

    if needs_confirm
        && !confirm_second_press(
            ctx,
            PendingDestructive::ClosePane(id),
            confirm_toast(&ctx.state.theme, "Again to kill pane"),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    begin_close_pane(ctx, id, ctx.state.config.animations)
}

pub(crate) fn kill_workspace_with_confirmation(
    ctx: &mut Context<HyprmuxApp>,
    confirmations_enabled: bool,
) -> Update {
    let workspace_index = ctx.state.active_workspace;
    let pane_count = ctx.state.workspaces[workspace_index]
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .count();
    if pane_count == 0 {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Workspace is already empty"));
        return Update::full();
    }

    if confirmations_enabled && ctx.state.config.confirm.kill_workspace {
        let label = workspace_index + 1;
        if !confirm_second_press(
            ctx,
            PendingDestructive::KillWorkspace(workspace_index),
            confirm_toast(
                &ctx.state.theme,
                format!("Again to kill {pane_count} pane(s) on workspace {label}"),
            ),
        ) {
            return Update::full();
        }
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

pub(crate) fn kill_session_with_confirmation(
    ctx: &mut Context<HyprmuxApp>,
    confirmations_enabled: bool,
) -> Update {
    if !ctx.state.session_attached {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            "Not attached to a named session",
        ));
        return Update::full();
    }

    let session_name = ctx
        .state
        .session_name
        .clone()
        .unwrap_or_else(|| "session".to_string());

    if confirmations_enabled
        && ctx.state.config.confirm.kill_session
        && !confirm_second_press(
            ctx,
            PendingDestructive::KillSession,
            confirm_toast(
                &ctx.state.theme,
                format!("Again to kill session `{session_name}`"),
            ),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    if let Some(client) = ctx.state.session_client.clone() {
        client.shutdown();
    }
    ctx.quit();
    Update::none()
}
