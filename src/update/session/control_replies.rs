use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::shared::{ClientId, SharedLayout};
use crate::pane::lifecycle::{
    find_pane_in_namespace, find_pane_in_namespace_mut, remove_pane_after_exit,
};
use crate::state::PaneId;

pub(crate) fn agent_report_result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    response: crate::control::ControlResponse,
) -> Update {
    let Some(pending) = ctx.state.pending_agent_report_replies.get(&request_id) else {
        return Update::none();
    };
    if pending
        .origin_epoch
        .is_some_and(|origin_epoch| origin_epoch != epoch)
    {
        return Update::none();
    }
    if let Some(pending) = ctx.state.pending_agent_report_replies.remove(&request_id) {
        let _ = pending.reply.send(response);
    }
    Update::none()
}

/// Metadata precedes placement and replay so a parser never interprets server history using
/// client configuration. A reconnect advertises the capacity again before its replay seed.
pub(crate) fn pane_metadata(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    local: bool,
    meta: crate::session::protocol::PaneMeta,
    cell_width: u16,
    cell_height: u16,
) -> Update {
    let geometry = crate::state::PaneGeometry {
        cols: meta.cols,
        rows: meta.rows,
        cell: crate::session::protocol::wire_cell(cell_width, cell_height),
    };
    if local {
        if epoch == ctx.state.runtime_epoch
            && let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, meta.pane_id, true)
            && pane.pty_generation == meta.generation
        {
            pane.terminal.use_server_scrollback(meta.scrollback);
            pane.terminal
                .apply_server_geometry(geometry.cols, geometry.rows, geometry.cell);
        }
        return Update::none();
    }
    if let Some(attachment) = ctx.state.attachment_for_epoch_mut(epoch) {
        store_pane_metadata(attachment, &meta);
        super::pane_events::apply_or_hold_geometry(
            attachment,
            meta.pane_id,
            meta.generation,
            geometry,
        );
    }
    Update::none()
}

fn store_pane_metadata(
    attachment: &mut crate::state::Attachment,
    meta: &crate::session::protocol::PaneMeta,
) {
    if let Some(shared) = attachment.shared.as_mut() {
        shared
            .pane_scrollback
            .insert((meta.pane_id, meta.generation), meta.scrollback);
    }
    for pane in attachment
        .workspaces
        .iter_mut()
        .flat_map(|workspace| &mut workspace.panes)
    {
        if pane.id == meta.pane_id && pane.pty_generation == meta.generation {
            pane.terminal.use_server_scrollback(meta.scrollback);
        }
    }
}

pub(crate) fn layout_committed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    rev: u64,
    author: ClientId,
    layout: SharedLayout,
    live: bool,
) -> Update {
    if let Some(attachment) = ctx.state.attachment_for_epoch_mut(epoch) {
        confirm_shared_spawn(attachment, rev, author, &layout);
    }
    if epoch != ctx.state.runtime_epoch {
        background_layout_committed(&mut ctx.state, epoch, rev, author, layout);
        return Update::none();
    }
    let my_id = ctx
        .state
        .current()
        .shared
        .as_ref()
        .map(|shared| shared.client_id);
    let own = my_id == Some(author);
    let update = if own {
        // Echo of our own commit: confirm the revision, never re-apply our own layout.
        if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
            shared.layout_rev = rev;
        }
        Update::none()
    } else {
        let update = reconcile_with_pending_spawns(ctx, &layout, rev);
        // A step of the controller's gesture is drawn where it lands, as the controller draws it.
        // Only the default transition yields: a revision that closes panes keeps its exit motion.
        if live && ctx.state.animation == crate::layout::anim::GeometryAnimation::TileFloat {
            ctx.state.animation = crate::layout::anim::GeometryAnimation::None;
        }
        update
    };
    emit_layout_changed(
        ctx,
        rev,
        if own {
            "self"
        } else if author == 0 {
            // Client ids start at 1; a revision the server wrote itself carries 0.
            "server"
        } else {
            "client"
        },
    );
    update
}

fn background_layout_committed(
    state: &mut crate::state::State,
    epoch: u64,
    rev: u64,
    author: ClientId,
    layout: SharedLayout,
) {
    if let Some(attachment) = state.background.get_mut(&epoch)
        && let Some(shared) = attachment.shared.as_mut()
    {
        shared.layout_rev = rev;
        if shared.client_id != author {
            shared.assumed_rev = rev;
            shared.last_committed_layout = Some(layout.clone());
            attachment.pending_background_layout = Some((rev, layout));
        }
    }
}

fn confirm_shared_spawn(
    attachment: &mut crate::state::Attachment,
    rev: u64,
    author: ClientId,
    layout: &SharedLayout,
) {
    let Some(shared) = attachment.shared.as_mut() else {
        return;
    };
    let Some(flight) = shared.spawn_in_flight else {
        return;
    };
    if author != shared.client_id || rev != flight.base_rev.saturating_add(1) {
        return;
    }
    if !layout
        .workspaces
        .iter()
        .flat_map(|ws| &ws.panes)
        .any(|pane| pane.pane_id == flight.pane_id && pane.generation == flight.generation)
    {
        return;
    }
    shared.spawn_in_flight = None;
    shared.assumed_rev = rev;
    shared.last_committed_layout = Some(layout.clone());
}

/// A failure consumes no revision. Wait for the server's rejection frame, which follows the
/// result even for PTY startup failures, before allowing another shared transaction to start.
fn reject_shared_spawn(
    state: &mut crate::state::State,
    epoch: u64,
    rev: u64,
    layout: Option<&SharedLayout>,
) -> bool {
    let Some(attachment) = state.attachment_for_epoch_mut(epoch) else {
        return false;
    };
    let Some(shared) = attachment.shared.as_mut() else {
        return false;
    };
    let Some(flight) = shared.spawn_in_flight.filter(|flight| flight.failed) else {
        return false;
    };
    shared.spawn_in_flight = None;
    shared.layout_rev = rev;
    shared.assumed_rev = rev;
    shared.last_committed_layout = layout.cloned();
    rev == flight.base_rev
}

/// Foreign corrections must not close placeholders whose spawn has not been submitted yet.
fn reconcile_with_pending_spawns(
    ctx: &mut Context<AppRoot>,
    layout: &SharedLayout,
    rev: u64,
) -> Update {
    let mut pending: std::collections::HashSet<_> = ctx
        .state
        .current()
        .pending_spawns
        .iter()
        .map(|request| (request.pane_id, request.generation))
        .collect();
    if let Some(flight) = ctx
        .state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.spawn_in_flight)
    {
        pending.insert((flight.pane_id, flight.generation));
    }
    for pane in layout.workspaces.iter().flat_map(|ws| &ws.panes) {
        pending.remove(&(pane.pane_id, pane.generation));
    }
    let mut retained = Vec::new();
    for (index, ws) in ctx.state.current_mut().workspaces.iter_mut().enumerate() {
        let (waiting, placed): (Vec<_>, Vec<_>) = std::mem::take(&mut ws.panes)
            .into_iter()
            .partition(|pane| !pane.closing && pending.contains(&(pane.id, pane.pty_generation)));
        ws.panes = placed;
        retained.push((index, waiting));
    }
    let update = crate::layout::shared::apply_shared_layout(ctx, layout, rev);
    for (index, waiting) in retained {
        let ws = &mut ctx.state.current_mut().workspaces[index];
        ws.panes.extend(waiting);
        ws.tile_tree = crate::layout::effective_tile_tree(ws, None);
    }
    update
}

/// Announce that the session's layout now stands at accepted revision `rev`: a commit of this
/// client's confirmed, or another client's or the server's applied.
///
/// Only an accepted revision raises it. A rejected commit is the layout specifically *not*
/// changing - the request that made it already hears the conflict - so it is not announced as a
/// change, and neither are animation frames or a commit still waiting for its confirmation.
fn emit_layout_changed(ctx: &Context<AppRoot>, rev: u64, author: &'static str) {
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::LayoutChanged,
            vec![
                ("revision", rev.to_string()),
                ("author", author.to_string()),
            ],
        ),
    );
}

/// Mirror (or clear) the controller's lifted pane.
///
/// The author ignores its own echo - its `moving_pane` is the real gesture, and adopting the
/// round-tripped copy would make the pane it is carrying track a position one relay old. Background
/// attachments retain the latest presence so a later switch cannot resurrect an old lift.
pub(crate) fn drag_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    author: ClientId,
    drag: Option<crate::state::RemoteDrag>,
) -> Update {
    let is_current = epoch == ctx.state.runtime_epoch;
    let Some(attachment) = ctx.state.attachment_for_epoch_mut(epoch) else {
        return Update::none();
    };
    let my_id = attachment.shared.as_ref().map(|shared| shared.client_id);
    if my_id == Some(author) {
        return Update::none();
    }
    if attachment.remote_drag == drag {
        return Update::none();
    }
    let first_lift = attachment.remote_drag.is_none() && drag.is_some();
    attachment.remote_drag = drag;
    if first_lift && is_current {
        attachment.remote_drag_snap.set(None);
        ctx.state.animation = crate::layout::anim::GeometryAnimation::TileFloat;
    }
    if is_current {
        Update::full()
    } else {
        Update::none()
    }
}

pub(crate) fn layout_rejected(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    current_rev: u64,
    layout: Option<SharedLayout>,
) -> Update {
    if reject_shared_spawn(&mut ctx.state, epoch, current_rev, layout.as_ref()) {
        return Update::full();
    }
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
        reconcile_with_pending_spawns(ctx, &layout, current_rev)
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
    crate::ops::session::flush_layout_commit(ctx);
    Update::none()
}

/// How long a queued replay input waits for its pane's shell to report a prompt (OSC 133 A/B)
/// before being written as plain type-ahead anyway - a shell without integration never reports
/// one, and correctness does not depend on the prompt: type-ahead input is read whenever the
/// shell gets there. Waiting only avoids the cosmetic double echo of injecting mid-startup
/// (kernel tty echo first, readline's redraw second).
pub(crate) const REPLAY_PROMPT_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(800);

pub(crate) fn replay_input_deadline_command(
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
) -> Command {
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
    if let Some(client) = ctx.state.pty_client_for_pane(pane_id) {
        match crate::ops::capture_wait::mark_queued_input(
            &mut ctx.state,
            pane_id,
            generation,
            local,
        ) {
            Some(token) => client.send_marked_input(pane_id, generation, local, bytes, token),
            None => client.send_input(pane_id, generation, local, bytes),
        }
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

struct SpawnReply {
    pane_id: PaneId,
    local: bool,
    generation: u64,
    pid: Option<u32>,
    ok: bool,
    error: Option<String>,
}

impl SpawnReply {
    fn error_message(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| "session spawn failed".to_string())
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
    let reply = SpawnReply {
        pane_id,
        local,
        generation,
        pid,
        ok,
        error,
    };
    let failed_transaction = mark_failed_shared_spawn(&mut ctx.state, epoch, &reply);
    if epoch != ctx.state.runtime_epoch {
        return background_spawn_result(ctx, epoch, reply, failed_transaction);
    }
    current_spawn_result(ctx, epoch, reply, failed_transaction)
}

fn mark_failed_shared_spawn(
    state: &mut crate::state::State,
    epoch: u64,
    reply: &SpawnReply,
) -> bool {
    if reply.local || reply.ok {
        return false;
    }
    let Some(flight) = state
        .attachment_for_epoch_mut(epoch)
        .and_then(|attachment| attachment.shared.as_mut())
        .and_then(|shared| shared.spawn_in_flight.as_mut())
    else {
        return false;
    };
    if (flight.pane_id, flight.generation) != (reply.pane_id, reply.generation) {
        return false;
    }
    flight.failed = true;
    true
}

/// A reconciled follower may already be replaying output. Rebind only a starting parser.
fn apply_spawn_terminal(pane: &mut crate::state::Pane, reply: &SpawnReply) -> bool {
    if pane.pty_generation != reply.generation {
        return false;
    }
    if !pane.terminal.is_ready() {
        pane.terminal
            .bind_server_backend(reply.pane_id, reply.generation);
    }
    pane.terminal.child_pid = reply.pid;
    pane.terminal.status = if reply.ok {
        ManagedTerminalStatus::Ready
    } else {
        ManagedTerminalStatus::Error(reply.error_message().into())
    };
    true
}

fn arm_spawn_replay(
    attachment: &mut crate::state::Attachment,
    reply: &SpawnReply,
    live: bool,
) -> bool {
    let key = (reply.pane_id, reply.generation);
    if !attachment.pending_replay_inputs.contains_key(&key) {
        return false;
    }
    if live {
        return true;
    }
    attachment.pending_replay_inputs.remove(&key);
    false
}

fn resolve_spawn_control_reply(
    state: &mut crate::state::State,
    epoch: u64,
    reply: &SpawnReply,
    live: bool,
    error: Option<&str>,
) {
    crate::ops::control::resolve_spawn_reply(
        state,
        epoch,
        reply.pane_id,
        reply.local,
        reply.generation,
        live,
        error,
    );
}

fn background_spawn_result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    reply: SpawnReply,
    failed_transaction: bool,
) -> Update {
    if reply.local {
        return Update::none();
    }
    let Some(attachment) = ctx.state.background.get_mut(&epoch) else {
        return Update::none();
    };
    let mut live = false;
    if let Some(pane) = attachment.find_pane_mut(reply.pane_id) {
        if !apply_spawn_terminal(pane, &reply) {
            return Update::none();
        }
        live = reply.ok;
        // Keep the placeholder open until the deferred close can repair focus, Scrollable
        // state, and pane retention when this attachment returns to the foreground in control.
        if failed_transaction
            && !attachment
                .pending_background_closes
                .contains(&(reply.pane_id, reply.generation))
        {
            attachment
                .pending_background_closes
                .push((reply.pane_id, reply.generation));
        }
    }
    if !reply.ok && attachment.is_controller() {
        attachment.pending_background_layout = None;
    }
    let replay = arm_spawn_replay(attachment, &reply, live);
    resolve_spawn_control_reply(
        &mut ctx.state,
        epoch,
        &reply,
        reply.ok,
        reply.error.as_deref(),
    );
    if replay {
        Update::with_command(replay_input_deadline_command(
            epoch,
            reply.pane_id,
            reply.generation,
        ))
    } else {
        Update::none()
    }
}

struct SpawnPresentation {
    live: bool,
    close: bool,
    error: Option<String>,
}

fn apply_current_spawn_reply(
    state: &mut crate::state::State,
    reply: &SpawnReply,
    failed_transaction: bool,
) -> Option<SpawnPresentation> {
    let may_close = reply.local || state.is_controller() || failed_transaction;
    let Some(pane) = find_pane_in_namespace_mut(state, reply.pane_id, reply.local) else {
        return Some(SpawnPresentation {
            live: false,
            close: false,
            error: reply.error.clone(),
        });
    };
    if !apply_spawn_terminal(pane, reply) {
        return None;
    }
    Some(SpawnPresentation {
        live: reply.ok,
        close: !reply.ok && may_close,
        error: (!reply.ok).then(|| reply.error_message()),
    })
}

fn finish_spawn_control_input(
    ctx: &mut Context<AppRoot>,
    reply: &SpawnReply,
    live: bool,
    replay: bool,
) {
    if replay {
        return;
    }
    if live {
        flush_pending_control_input(ctx, reply.pane_id, reply.generation, reply.local);
    } else {
        ctx.state
            .pending_control_input
            .remove(&(reply.local, reply.pane_id, reply.generation));
    }
}

fn current_spawn_result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    reply: SpawnReply,
    failed_transaction: bool,
) -> Update {
    let Some(presentation) = apply_current_spawn_reply(&mut ctx.state, &reply, failed_transaction)
    else {
        return Update::none();
    };
    // Replay waits for prompt readiness, with a deadline for shells without integration.
    let replay = arm_spawn_replay(ctx.state.current_mut(), &reply, presentation.live);
    resolve_spawn_control_reply(
        &mut ctx.state,
        epoch,
        &reply,
        presentation.live,
        presentation.error.as_deref(),
    );
    finish_spawn_control_input(ctx, &reply, presentation.live, replay);
    ctx.state.commands_dirty = true;
    if let Some(error) = presentation.error {
        crate::pane::pty_events::notify_error(ctx, "Spawn failed", error);
    }
    if presentation.close {
        // Popups live outside workspaces and need their own close lifecycle.
        if reply.local && reply.pane_id == crate::state::POPUP_PANE_ID {
            return crate::ops::popup::close(ctx);
        }
        return remove_pane_after_exit(ctx, reply.pane_id, reply.local);
    }
    if replay {
        Update::with_command(replay_input_deadline_command(
            epoch,
            reply.pane_id,
            reply.generation,
        ))
    } else {
        Update::full()
    }
}
