use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::{find_pane_in_namespace, find_pane_in_namespace_mut, remove_pane_after_exit};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

pub(crate) fn layout_committed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    rev: u64,
    author: ClientId,
    layout: SharedLayout,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch)
            && let Some(shared) = attachment.shared.as_mut()
        {
            shared.layout_rev = rev;
            if shared.client_id != author {
                shared.assumed_rev = rev;
                shared.last_committed_layout = Some(layout.clone());
                attachment.pending_background_layout = Some((rev, layout));
            }
        }
        return Update::none();
    }
    let my_id = ctx
        .state
        .current()
        .shared
        .as_ref()
        .map(|shared| shared.client_id);
    if my_id == Some(author) {
        // Echo of our own commit: confirm the revision, never re-apply our own layout.
        if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
            shared.layout_rev = rev;
        }
        Update::none()
    } else {
        crate::shared_layout::apply_shared_layout(ctx, &layout, rev)
    }
}

pub(crate) fn layout_rejected(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    current_rev: u64,
    layout: Option<SharedLayout>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch)
            && let Some(shared) = attachment.shared.as_mut()
        {
            shared.assumed_rev = current_rev;
            shared.last_committed_layout = None;
            if let Some(layout) = layout {
                attachment.pending_background_layout = Some((current_rev, layout));
            }
        }
        return Update::none();
    }
    let update = if let Some(layout) = layout {
        crate::shared_layout::apply_shared_layout(ctx, &layout, current_rev)
    } else {
        Update::full()
    };
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.assumed_rev = current_rev;
        // Clear the dirty detector so the debounced chokepoint recommits from current state.
        shared.last_committed_layout = None;
    }
    update
}

pub(crate) fn ping(ctx: &mut Context<AppRoot>, epoch: u64, seq: u64) -> Update {
    if let Some(client) = ctx
        .state
        .attachment_for_epoch(epoch)
        .and_then(|attachment| attachment.session_client.as_ref())
    {
        client.pong(seq);
    }
    Update::none()
}

pub(crate) fn flush_layout_commit(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.layout_commit_scheduled = false;
        }
        return Update::none();
    }
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.layout_commit_scheduled = false;
    }
    crate::update::flush_layout_commit(ctx);
    Update::none()
}

/// How long a queued replay input waits for its pane's shell to report a prompt (OSC 133 A/B)
/// before being written as plain type-ahead anyway - a shell without integration never reports
/// one, and correctness does not depend on the prompt: type-ahead input is read whenever the
/// shell gets there. Waiting only avoids the cosmetic double echo of injecting mid-startup
/// (kernel tty echo first, readline's redraw second).
pub(crate) const REPLAY_PROMPT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(800);

pub(crate) fn replay_input_deadline_command(epoch: u64, pane_id: PaneId, generation: u64) -> Command {
    Command::after(
        REPLAY_PROMPT_DEADLINE,
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::ReplayInputDeadline {
                epoch,
                pane_id,
                generation,
            });
        },
    )
}

pub(crate) fn replay_input_deadline(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            flush_attachment_replay_input(attachment, pane_id, generation);
        }
        return Update::none();
    }
    flush_replay_input(ctx, pane_id, generation);
    Update::none()
}

/// The held `new-pane` reply waited long enough (see
/// [`crate::state::State::pending_spawn_replies`]). Answer with the readiness the pane actually
/// reached, so a slow or wedged spawn degrades to today's `pty_ready:false` instead of leaving the
/// caller on the control connection's own timeout.
pub(crate) fn spawn_reply_deadline(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
) -> Update {
    let ready = if epoch == ctx.state.runtime_epoch {
        find_pane_in_namespace(&ctx.state, pane_id, local)
            .is_some_and(|pane| pane.pty_generation == generation && pane.terminal.is_ready())
    } else if local {
        false
    } else {
        ctx.state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.find_pane_mut(pane_id))
            .is_some_and(|pane| pane.pty_generation == generation && pane.terminal.is_ready())
    };
    crate::ops::control::resolve_spawn_reply(
        &mut ctx.state,
        epoch,
        pane_id,
        local,
        generation,
        ready,
        None,
    );
    Update::none()
}

/// Write the type-ahead a control `send-text` / `send-keys` accepted while this pane's PTY was
/// still starting (see [`crate::state::State::pending_control_input`]). Always runs behind
/// [`flush_replay_input`] so a restored pane's own command reaches the shell first.
pub(crate) fn flush_pending_control_input(
    ctx: &mut Context<AppRoot>,
    pane_id: PaneId,
    generation: u64,
    local: bool,
) {
    let Some(bytes) = ctx
        .state
        .pending_control_input
        .remove(&(local, pane_id, generation))
    else {
        return;
    };
    if find_pane_in_namespace(&ctx.state, pane_id, local)
        .is_none_or(|pane| pane.pty_generation != generation)
    {
        return;
    }
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.send_input(pane_id, generation, local, bytes);
    }
}

/// Deliver a queued replay command (see `State::pending_replay_inputs`) exactly once: sent as
/// ordinary pane input followed by a carriage return, the pane's interactive shell reads and runs
/// it as if the user had typed it - aliases, shell functions, and rc-file PATH resolve, and the
/// prompt's title/OSC integration has already run. The entry is consumed even when the pane is
/// gone or the client dropped; a later respawn queues its own fresh entry.
pub(crate) fn flush_replay_input(ctx: &mut Context<AppRoot>, pane_id: PaneId, generation: u64) {
    let Some(input) = ctx
        .state
        .current_mut()
        .pending_replay_inputs
        .remove(&(pane_id, generation))
    else {
        return;
    };
    if find_pane_in_namespace(&ctx.state, pane_id, false)
        .is_some_and(|pane| pane.pty_generation == generation)
        && let Some(client) = ctx.state.current().session_client.clone()
    {
        let mut bytes = input.into_bytes();
        bytes.push(b'\r');
        client.send_input(pane_id, generation, false, bytes);
    }
    flush_pending_control_input(ctx, pane_id, generation, false);
}

pub(crate) fn flush_attachment_replay_input(
    attachment: &mut crate::state::Attachment,
    pane_id: PaneId,
    generation: u64,
) {
    let Some(input) = attachment
        .pending_replay_inputs
        .remove(&(pane_id, generation))
    else {
        return;
    };
    if attachment
        .find_pane_mut(pane_id)
        .is_none_or(|pane| pane.pty_generation != generation)
    {
        return;
    }
    if let Some(client) = attachment.session_client.as_ref() {
        let mut bytes = input.into_bytes();
        bytes.push(b'\r');
        client.send_input(pane_id, generation, false, bytes);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    pid: Option<u32>,
    ok: bool,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if local {
            return Update::none();
        }
        let Some(attachment) = ctx.state.background.get_mut(&epoch) else {
            return Update::none();
        };
        let is_controller = attachment.is_controller();
        let mut spawned_live = false;
        if let Some(pane) = attachment.find_pane_mut(pane_id) {
            if pane.pty_generation != generation {
                return Update::none();
            }
            spawned_live = ok;
            if !pane.terminal.is_ready() {
                pane.terminal.bind_server_backend(pane_id, generation);
            }
            pane.terminal.child_pid = pid;
            if ok {
                pane.terminal.status = ManagedTerminalStatus::Ready;
            } else {
                let message = error
                    .clone()
                    .unwrap_or_else(|| "session spawn failed".to_string());
                pane.terminal.status = ManagedTerminalStatus::Error(message.into());
                if is_controller {
                    attachment.pending_background_layout = None;
                }
            }
        }
        let replay_armed = attachment
            .pending_replay_inputs
            .contains_key(&(pane_id, generation));
        if replay_armed && !spawned_live {
            attachment
                .pending_replay_inputs
                .remove(&(pane_id, generation));
        }
        crate::ops::control::resolve_spawn_reply(
            &mut ctx.state,
            epoch,
            pane_id,
            local,
            generation,
            ok,
            error.as_deref(),
        );
        if replay_armed && spawned_live {
            return Update::with_command(replay_input_deadline_command(epoch, pane_id, generation));
        }
        return Update::none();
    }
    let is_controller = ctx.state.is_controller();
    let mut should_close = false;
    let mut toast_error = None;
    let mut spawned_live = false;
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        spawned_live = ok;
        // A follower may already hold this pane (bound and Ready) from the reconciler; only (re)bind
        // a fresh backend for a pane still waiting on its own spawn to complete, so we never destroy
        // a live screen that is already replaying server output.
        if !pane.terminal.is_ready() {
            pane.terminal.bind_server_backend(pane_id, generation);
        }
        pane.terminal.child_pid = pid;
        if ok {
            pane.terminal.status = ManagedTerminalStatus::Ready;
        } else {
            let message = error
                .clone()
                .unwrap_or_else(|| "session spawn failed".to_string());
            pane.terminal.status = ManagedTerminalStatus::Error(message.clone().into());
            toast_error = Some(message);
            // Only the controller structurally removes the failed pane; followers wait for the
            // resulting layout commit.
            should_close = local || is_controller;
        }
    } else if let Some(error) = error {
        toast_error = Some(error);
    }
    // A queued replay command (see `State::pending_replay_inputs`) is not written yet: it waits
    // for the shell's first prompt report (`pane_runtime_changed` flushes it) so readline echoes
    // it exactly once at the prompt, instead of the kernel tty echoing it again mid-startup. The
    // deadline command is the fallback for shells without OSC 133 integration. A failed or
    // superseded spawn drops the entry instead.
    let mut replay_deadline = None;
    if ctx
        .state
        .current()
        .pending_replay_inputs
        .contains_key(&(pane_id, generation))
    {
        if spawned_live {
            replay_deadline = Some(replay_input_deadline_command(epoch, pane_id, generation));
        } else {
            ctx.state
                .current_mut()
                .pending_replay_inputs
                .remove(&(pane_id, generation));
        }
    }
    // The `new-pane` reply is held until here so it can state real readiness rather than mere
    // acceptance (see `State::pending_spawn_replies`).
    crate::ops::control::resolve_spawn_reply(
        &mut ctx.state,
        epoch,
        pane_id,
        local,
        generation,
        spawned_live,
        toast_error.as_deref(),
    );
    // Queued control input (see `State::pending_control_input`) rides behind the replay command
    // when there is one; with no replay armed the PTY is ready now, so write it here. A failed
    // spawn drops it: nothing will ever read those bytes.
    if replay_deadline.is_none() {
        if spawned_live {
            flush_pending_control_input(ctx, pane_id, generation, local);
        } else {
            ctx.state
                .pending_control_input
                .remove(&(local, pane_id, generation));
        }
    }
    ctx.state.commands_dirty = true;
    if let Some(error) = toast_error {
        crate::pty_events::notify_error(ctx, "Spawn failed", error);
    }
    if should_close {
        // The popup lives outside every workspace, so the generic teardown cannot reach it: with a
        // local namespace and no scratch membership it falls through and marks nothing, leaving a
        // dead pane on screen. `exited` intercepts popups the same way. `close` rather than
        // `handle_exit` - a spawn that never started has nothing for `keep_open` to keep.
        if local && pane_id == crate::state::POPUP_PANE_ID {
            return crate::popup::close(ctx);
        }
        remove_pane_after_exit(ctx, pane_id, local)
    } else if let Some(command) = replay_deadline {
        Update::with_command(command)
    } else {
        Update::full()
    }
}
