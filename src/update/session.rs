use tui_lipan::prelude::*;

use super::attach::{
    apply_attached_panes, bind_attached_pane_backends, flush_pending_spawns,
    reset_state_for_shared_seed, spawn_state_panes_on_session,
};
use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::pane_lifecycle::{begin_close_pane, find_pane, find_pane_mut};
use crate::pty_events::{error_toast, maybe_notify_pane_exit, maybe_notify_pane_status};
use crate::session::client::SessionClient;
use crate::session::protocol::{ClientInfo, ControllerChangeReason, PaneMeta, PaneRuntimeState};
use crate::shared_layout::{ClientId, SharedLayout};
use crate::state::PaneId;

pub(super) fn connected(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    name: String,
    client: SessionClient,
) -> Update {
    let Some(pending) = ctx.state.current_mut().pending_session_attach.as_mut() else {
        return Update::none();
    };
    if pending.epoch != epoch || pending.name != name {
        return Update::none();
    }
    pending.client = Some(client);
    Update::full()
}

pub(super) fn disconnected(ctx: &mut Context<HyprmuxApp>, epoch: u64, name: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        let disconnected = ctx
            .state
            .background
            .get_mut(&epoch)
            .filter(|attachment| attachment.session_name.as_deref() == Some(name.as_str()))
            .filter(|attachment| attachment.pending_session_attach.is_none());
        if let Some(attachment) = disconnected {
            attachment.mark_disconnected();
            crate::update::sidebar::invalidate_sessions(ctx);
        }
        return Update::none();
    }
    // Only the current session's unexpected disconnect matters; an intentional detach or
    // attach-elsewhere has already bumped the epoch, so its stale disconnect is filtered out above.
    if ctx.state.current().session_name.as_deref() != Some(name.as_str()) {
        return Update::none();
    }
    if ctx.state.current().pending_session_attach.is_some() {
        return Update::full();
    }
    crate::update::sidebar::invalidate_sessions(ctx);
    ctx.state.current_mut().mark_disconnected();
    crate::ops::session::reconnect_current_session(ctx)
}

pub(super) fn attach_failed(ctx: &mut Context<HyprmuxApp>, epoch: u64, message: String) -> Update {
    let Some(pending) = ctx.state.current().pending_session_attach.as_ref() else {
        return Update::none();
    };
    if pending.epoch != epoch {
        return Update::none();
    }
    let was_remote = pending.remote_host.is_some();
    let was_reconnect = pending.reconnect;
    let parked_epoch = pending.parked_epoch;
    ctx.state.current_mut().pending_session_attach = None;
    ctx.state.current_mut().connection = if was_remote {
        crate::state::ConnectionState::Unreachable
    } else {
        crate::state::ConnectionState::Disconnected
    };
    ctx.toast()
        .push(error_toast(&ctx.state.theme, "Attach failed", message));
    if !was_reconnect {
        // Creating/switching parked a live session to move away from it. If the new attach failed,
        // restore that session rather than strand the user on the broken empty attachment — for a
        // remote connect this also avoids the ephemeral fallback re-attaching to this process's own
        // `eph-<pid>` server (still controlled by the parked client) and joining as a follower.
        if let Some(parked_id) = parked_epoch
            && ctx.state.background.contains_key(&parked_id)
        {
            let failed_epoch = ctx.state.runtime_epoch;
            let update = crate::ops::session::switch_to_parked(ctx, parked_id);
            // Drop the dead empty attachment the failed attach left behind; it never attached, so it
            // holds no client to detach.
            ctx.state.background.remove(&failed_epoch);
            return update;
        }
        // Nothing to restore (a bare `--remote` launch, say): an unreachable host would otherwise
        // strand the UI with no session at all, so fall back to a fresh local ephemeral. Remote-only:
        // a *local* ephemeral attach is itself the fallback, and retrying it here would spin forever.
        if was_remote
            && !ctx.state.current().session_attached
            && ctx.state.current().session_client.is_none()
        {
            return crate::ops::session::attach_startup_ephemeral(ctx);
        }
    }
    Update::full()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn attached(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    session: String,
    client_id: ClientId,
    panes: Vec<PaneMeta>,
    layout_rev: u64,
    layout: Option<SharedLayout>,
    controller: Option<ClientId>,
    clients: Vec<ClientInfo>,
    input_locked: bool,
    read_only: bool,
    created_from_profile: Option<String>,
) -> Update {
    let Some(pending) = ctx.state.current().pending_session_attach.as_ref() else {
        return Update::none();
    };
    if pending.epoch != epoch || pending.name != session {
        return Update::none();
    }
    let pending = ctx
        .state
        .current_mut()
        .pending_session_attach
        .take()
        .expect("pending attach checked above");
    let reconnect = pending.reconnect;
    let Some(client) = pending.client else {
        return Update::none();
    };
    ctx.state.runtime_epoch = epoch;
    ctx.state.current_mut().session_client = Some(client);
    ctx.state.current_mut().session_name = Some(session.clone());
    if let Some(host) = pending.remote_host {
        ctx.state.current_mut().remote_host = Some(host);
    }
    ctx.state.current_mut().created_from_profile = created_from_profile;
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connected;
    ctx.state.current_mut().reconnect_read_only = read_only;
    ctx.state.current_mut().session_attached = true;
    crate::update::sidebar::invalidate_sessions(ctx);
    ctx.state.current_mut().deferred_profile_seed = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    if !crate::state::is_ephemeral_session_name(&session) {
        crate::session::record_last_named_session(&session);
    }

    let mut shared = crate::state::SharedSessionState::new(client_id);
    shared.layout_rev = layout_rev;
    shared.assumed_rev = layout_rev;
    shared.controller = controller;
    shared.clients = clients;
    shared.input_locked = input_locked;
    shared.read_only = read_only;
    ctx.state.current_mut().shared = Some(shared);

    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::SessionAttached,
            vec![
                ("session", session.clone()),
                ("client_id", client_id.to_string()),
                (
                    "controller",
                    controller.map(|id| id.to_string()).unwrap_or_default(),
                ),
                ("read_only", read_only.to_string()),
            ],
        ),
    );

    // The session identity just changed (ephemeral vs named), which the "Name/Rename session"
    // palette label reflects; the lease state affects command labels too.
    ctx.state.commands_dirty = true;

    let panes: Vec<_> = panes
        .into_iter()
        .filter(|pane| pane.exited.is_none())
        .collect();
    let had_panes = !panes.is_empty();

    let populated = layout.is_some() || had_panes;
    let update = if let Some(layout) = layout {
        // Shared attach: seed the whole window-manager structure from the authoritative layout via
        // the one reconciler code path, then bind server backends and sizes from the pane metadata
        // before the replay seed frames arrive.
        if !reconnect {
            reset_state_for_shared_seed(&mut ctx.state);
        }
        crate::shared_layout::apply_shared_layout(ctx, &layout, layout_rev);
        // Attaching reveals an already-running session, so snap to its authoritative geometry
        // instead of interpolating from the previous session's pane rectangles.
        ctx.state.animation = GeometryAnimation::None;
        bind_attached_pane_backends(ctx, panes);
        flush_pending_spawns(ctx);
        Update::full()
    } else if had_panes {
        // Defensive: a live server holding panes but no committed layout (should not occur under
        // protocol v6). Adopt the panes, then republish a layout if we control it.
        apply_attached_panes(ctx, panes);
        ctx.state.animation = GeometryAnimation::None;
        flush_pending_spawns(ctx);
        Update::full()
    } else {
        // An empty server (fresh ephemeral, autostarted named session, or one whose panes all
        // exited): seed it with the panes the client already holds in state; the first attacher
        // (controller) commits rev 1 on the tail chokepoint pass.
        let spawned = spawn_state_panes_on_session(ctx);
        flush_pending_spawns(ctx);
        if spawned.is_empty() {
            Update::full()
        } else {
            let open_delay = crate::anim::open_delay(ctx.state.config.animations);
            let activate_delay = crate::anim::activation_delay(ctx.state.config.animations);
            Update::with_command(crate::pane_lifecycle::open_timers_batch_command(
                epoch,
                spawned,
                open_delay,
                activate_delay,
            ))
        }
    };

    let named = !crate::state::is_ephemeral_session_name(&session);
    let suffix = pending
        .left
        .as_ref()
        .map(|left| {
            if left.was_ephemeral_shutdown {
                "\nEnded the temporary session".to_string()
            } else {
                format!("\nDetached from `{}`", left.name)
            }
        })
        .unwrap_or_default();
    if !populated && let crate::state::AttachIntent::ProfileSeed { profile, path } = &pending.intent
    {
        if let Some(client) = ctx.state.current().session_client.as_ref() {
            client.set_session_origin(profile.clone());
        }
        ctx.state.current_mut().pending_profile_loaded =
            Some((profile.clone(), path.clone(), session.clone()));
        if named {
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Launched `{session}` from profile{suffix}"),
            ));
        }
    } else if named {
        if populated {
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Attached to `{session}`{suffix}"),
            ));
        } else if matches!(pending.intent, crate::state::AttachIntent::Plain) {
            crate::events::emit(
                &ctx.state,
                crate::events::Event::new(
                    crate::events::EventKind::SessionCreated,
                    vec![("session", session.clone())],
                ),
            );
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Created session `{session}`{suffix}"),
            ));
        }
    }
    if let Some(origin) = ctx.state.current().created_from_profile.clone() {
        confirm_profile_origin(ctx, origin);
    }
    crate::update::sidebar::request_sessions_refresh(ctx);
    update
}

pub(super) fn origin_set(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    created_from_profile: String,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            attachment.created_from_profile = Some(created_from_profile);
        }
        return Update::none();
    }
    confirm_profile_origin(ctx, created_from_profile);
    Update::full()
}

fn confirm_profile_origin(ctx: &mut Context<HyprmuxApp>, created_from_profile: String) {
    ctx.state.current_mut().created_from_profile = Some(created_from_profile.clone());
    if let Some((profile, path, session)) = ctx.state.current_mut().pending_profile_loaded.take()
        && profile == created_from_profile
    {
        crate::events::emit(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::ProfileLoaded,
                vec![
                    ("profile", profile),
                    ("path", path.display().to_string()),
                    ("session", session),
                ],
            ),
        );
    }
}

pub(super) fn layout_committed(
    ctx: &mut Context<HyprmuxApp>,
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

pub(super) fn layout_rejected(
    ctx: &mut Context<HyprmuxApp>,
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

pub(super) fn controller_changed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    controller: Option<ClientId>,
    reason: ControllerChangeReason,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.controller = controller;
            if shared.is_controller() {
                shared.assumed_rev = shared.layout_rev;
                shared.last_committed_layout = None;
            }
        }
        return Update::none();
    }
    let was_controller = ctx.state.is_controller();
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.controller = controller;
        if shared.is_controller() {
            // Gaining control: rebase optimistic commits, and clear the dirty detector so the tail
            // chokepoint republishes the layout with our canonical canvas.
            shared.assumed_rev = shared.layout_rev;
            shared.last_committed_layout = None;
        }
    }
    let now_controller = ctx.state.is_controller();
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::ControllerChanged,
            vec![
                (
                    "controller",
                    controller.map(|id| id.to_string()).unwrap_or_default(),
                ),
                ("self_controller", now_controller.to_string()),
                ("reason", controller_change_reason_id(reason).to_string()),
            ],
        ),
    );
    if was_controller && !now_controller {
        ctx.state.moving_pane = None;
        ctx.state.resizing_pane = None;
        ctx.state.split_drag = None;
        let who = controller
            .map(|id| format!("client {id}"))
            .unwrap_or_else(|| "another client".to_string());
        crate::pty_events::replace_toast(
            ctx,
            crate::state::ToastChannel::LayoutControl,
            crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Layout control taken by {who}"),
            ),
        );
    } else if !was_controller && now_controller {
        crate::ops::session::apply_pending_background_closes(ctx);
        crate::pty_events::replace_toast(
            ctx,
            crate::state::ToastChannel::LayoutControl,
            crate::pty_events::info_toast(&ctx.state.theme, "You now control the layout"),
        );
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(super) fn clients_changed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    clients: Vec<ClientInfo>,
    input_locked: bool,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.clients = clients;
            shared.input_locked = input_locked;
        }
        return Update::none();
    }
    let roster_events = ctx
        .state
        .current()
        .shared
        .as_ref()
        .map(|shared| roster_diff_events(&shared.clients, &clients))
        .unwrap_or_default();
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        let changed = shared.input_locked != input_locked;
        shared.clients = clients;
        shared.input_locked = input_locked;
        if changed {
            crate::pty_events::replace_toast(
                ctx,
                crate::state::ToastChannel::InputState,
                crate::pty_events::info_toast(
                    &ctx.state.theme,
                    if input_locked {
                        "Input locked to the controller"
                    } else {
                        "Input unlocked"
                    },
                ),
            );
        }
    }
    for event in roster_events {
        crate::events::emit(&ctx.state, event);
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(super) fn control_requested(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    from: ClientId,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    // Only the controller can act on a request; ignore a stale one that arrived after we lost the
    // lease. The requester's badge lives in the roster (via ClientsChanged) regardless.
    if !ctx.state.is_controller() {
        return Update::none();
    }
    let who = ctx
        .state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.clients.iter().find(|client| client.id == from))
        .map(|client| format!("{} #{}", client.label, client.id))
        .unwrap_or_else(|| format!("client {from}"));
    // Advertise the live grant binding so the hint tracks any `[keys]` override instead of a
    // hardcoded key; fall back to the session-clients view when the action is unbound.
    let how = crate::commands::command_prefix_chord(ctx, "grant-control")
        .map(|chord| format!("{chord} to grant"))
        .unwrap_or_else(|| "grant from Session clients".to_string());
    ctx.toast().push(crate::pty_events::info_toast(
        &ctx.state.theme,
        format!("{who} requests layout control\n{how}"),
    ));
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(super) fn control_declined(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    ctx.toast().push(crate::pty_events::info_toast(
        &ctx.state.theme,
        "Your control request was declined",
    ));
    Update::full()
}

pub(super) fn ping(ctx: &mut Context<HyprmuxApp>, epoch: u64, seq: u64) -> Update {
    if let Some(client) = ctx
        .state
        .attachment_for_epoch(epoch)
        .and_then(|attachment| attachment.session_client.as_ref())
    {
        client.pong(seq);
    }
    Update::none()
}

pub(super) fn flush_pane_resizes(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(shared) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.shared.as_mut())
        {
            shared.resize_flush_scheduled = false;
            shared.pending_resizes.clear();
        }
        return Update::none();
    }
    crate::pty_events::flush_pending_resizes(ctx);
    Update::none()
}

pub(super) fn flush_layout_commit(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
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
    super::flush_layout_commit(ctx);
    Update::none()
}

pub(super) fn output(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    bytes: Vec<u8>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        // A retained background attachment: keep its screens live so switching back is instant, but
        // never draw them (nothing background is on screen).
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            attachment.apply_background_output(pane_id, generation, &bytes);
        }
        return Update::none();
    }
    let focused = ctx.state.current().focused_pane;
    let bell_notifications = ctx.state.config.notifications.bell;
    // Activity/bell indicators are workspace-agnostic (the workbar counts them across every
    // workspace), so an off-screen pane still needs a frame on the chunk that first raises one.
    // Both flags only ever go false -> true here, so that is a single frame per quiet period
    // rather than one per output chunk.
    let mut indicator_raised = false;
    let matched = match find_pane_mut(&mut ctx.state, pane_id) {
        Some(pane) if pane.pty_generation == generation => {
            pane.terminal.process_server_output(&bytes);
            let bell = pane.terminal.take_bell();
            pane.activity.last_activity = Some(std::time::Instant::now());
            if focused != Some(pane_id) {
                indicator_raised |= !pane.activity.has_unseen_output;
                pane.activity.has_unseen_output = true;
                if bell && bell_notifications {
                    indicator_raised |= !pane.activity.bell;
                    pane.activity.bell = true;
                }
            }
            true
        }
        _ => false,
    };
    if !matched {
        // Output arrived before the layout commit that introduces this pane (or its new generation).
        // Buffer it so the reconciler can replay it when the pane appears; dropping it would leave
        // a follower's fresh pane blank until the next redraw. Nothing draws it yet, so no frame.
        if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
            shared.buffer_orphan_output(pane_id, generation, &bytes);
        }
        return Update::none();
    }
    // The screen is already updated above; only ask for a frame when the result reaches the
    // display. A chatty pane on an inactive workspace would otherwise drive the renderer at full
    // rate painting a view its output never appears in (see `State::pane_is_rendered`).
    if indicator_raised || ctx.state.pane_is_rendered(pane_id) {
        Update::full()
    } else {
        Update::none()
    }
}

pub(super) fn resized(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    cols: u16,
    rows: u16,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        // Keep a retained background attachment's screen at the server's size for an instant, correct
        // switch-back.
        if let Some(pane) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.find_pane_mut(pane_id))
            && pane.pty_generation == generation
        {
            pane.terminal.apply_server_resize(cols, rows);
        }
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
        && pane.pty_generation == generation
        && pane.terminal.apply_server_resize(cols, rows)
        && ctx.state.pane_is_rendered(pane_id)
    {
        return Update::full();
    }
    Update::none()
}

pub(super) fn exited(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    code: i32,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        // Mark the pane exited on a retained background attachment so switching back shows the exit.
        // The structural close (a layout commit) is deferred until the attachment is current again.
        if let Some(pane) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.find_pane_mut(pane_id))
            && pane.pty_generation == generation
        {
            pane.terminal.status = ManagedTerminalStatus::Exited(code);
            if !should_hold_on_exit(ctx.state.config.pane.hold_on_exit, pane_id, pane.closing)
                && ctx
                    .state
                    .background
                    .get(&epoch)
                    .is_some_and(crate::state::Attachment::is_controller)
                && let Some(attachment) = ctx.state.background.get_mut(&epoch)
            {
                attachment
                    .pending_background_closes
                    .push((pane_id, generation));
            }
        }
        return Update::none();
    }
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::PaneExited,
            vec![("pane", pane_id.to_string()), ("code", code.to_string())],
        ),
    );
    let mut should_close = false;
    let mut already_closing = false;
    let hold_on_exit = ctx.state.config.pane.hold_on_exit;
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        pane.terminal.status = ManagedTerminalStatus::Exited(code);
        already_closing = pane.closing;
        should_close = !should_hold_on_exit(hold_on_exit, pane_id, pane.closing);
    }
    ctx.state.commands_dirty = true;
    // A user-initiated close already tore this pane down; the exit is expected, so skip the exit
    // notification/toast and the redundant close call.
    if already_closing {
        return Update::full();
    }
    // The scratchpad is a local overlay (never in the shared layout), so every client that owns it
    // closes it directly.
    if pane_id == crate::state::POPUP_PANE_ID {
        return crate::popup::handle_exit(ctx);
    }
    if crate::scratchpad::is_scratch(pane_id) {
        return crate::scratchpad::handle_scratch_exit(ctx);
    }
    // Closing a tiled/floating pane is a structural layout change: only the controller acts on the
    // exit and commits the new layout; followers close it when that commit arrives.
    if !ctx.state.is_controller() {
        return Update::full();
    }
    maybe_notify_pane_exit(&ctx.state.config, pane_id, code);
    if !should_close {
        return Update::full();
    }
    // A clean exit closes the pane on its own; only a failure code is worth surfacing.
    if code != 0 {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            format!("Pane {pane_id} exited ({code})"),
        ));
    }
    begin_close_pane(ctx, pane_id, ctx.state.config.animations)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pane_logging_changed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    enabled: bool,
    path: Option<String>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
        && pane.pty_generation == generation
    {
        pane.logging = enabled;
    }
    let message = error.unwrap_or_else(|| {
        if enabled {
            format!(
                "Logging pane {pane_id} to {}",
                path.as_deref().unwrap_or("log file")
            )
        } else {
            format!("Stopped logging pane {pane_id}")
        }
    });
    ctx.toast()
        .push(crate::pty_events::info_toast(&ctx.state.theme, message));
    Update::full()
}

fn status_is(value: Option<&str>, needle: &str) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case(needle))
}

fn status_is_quiescent(value: Option<&str>) -> bool {
    status_is(value, crate::session::protocol::pane_status::IDLE)
        || status_is(value, crate::session::protocol::pane_status::DONE)
}

fn status_is_active_run(value: Option<&str>) -> bool {
    value.is_some() && !status_is_quiescent(value)
}

/// React to an agent-status transition. `previous_age` is how long the outgoing status had held,
/// sampled before it was overwritten.
///
/// Stamps the local fallback, banks the length of an active run as it ends so a finished run can
/// report what it cost, and updates the "unseen finish" pulse: armed on an active -> quiescent edge
/// (the run finished while you were looking elsewhere), disarmed the moment the agent resumes
/// working, so a spinning agent never wears a completed-dot. A `blocked` outcome is deliberately
/// left un-armed: it already has its own loud glyph, and the server-owned run timestamp keeps the
/// same timer ready for a later resume. A separate focus chokepoint clears the flag once the pane is
/// actually looked at.
fn update_agent_status_edge(
    pane: &mut crate::pane::TerminalPane,
    previous: Option<&str>,
    previous_age: Option<std::time::Duration>,
) {
    let current = pane.agent_status();
    let current = current.as_deref();
    if current != previous {
        if status_is_active_run(previous) && status_is_quiescent(current) {
            pane.last_run = previous_age;
        }
        pane.status_since = Some(std::time::Instant::now());
    }
    if status_is(current, crate::session::protocol::pane_status::WORKING) {
        pane.finished_unseen = false;
    } else if status_is(previous, crate::session::protocol::pane_status::WORKING)
        && current.is_some()
        && !status_is(current, crate::session::protocol::pane_status::BLOCKED)
    {
        pane.finished_unseen = true;
    }
}

pub(super) fn pane_runtime_changed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    state: PaneRuntimeState,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        let at_prompt = matches!(
            state.command_phase,
            crate::session::protocol::PaneCommandPhase::Prompt
                | crate::session::protocol::PaneCommandPhase::Input
        );
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            if let Some(pane) = attachment.find_pane_mut(pane_id)
                && pane.pty_generation == generation
                && state.sequence > pane.terminal.runtime_sequence
            {
                let previous_agent_status = pane.terminal.agent_status();
                let previous_age = pane.terminal.status_age();
                pane.terminal.runtime_sequence = state.sequence;
                pane.terminal.cwd = state.cwd;
                pane.terminal.cwd_host = state.cwd_host;
                pane.terminal.display_path = state.display_path;
                pane.terminal.project_root = state.project_root;
                pane.terminal.git_branch = state.git_branch;
                pane.terminal.foreground_program = state.foreground_program;
                pane.terminal.command_phase = state.command_phase;
                pane.terminal.last_exit_status = state.last_exit_status;
                pane.terminal.reported_status = state.status;
                pane.terminal.detected_agent = state.detected_agent;
                pane.terminal.work_started_at = state.work_started_at;
                update_agent_status_edge(
                    &mut pane.terminal,
                    previous_agent_status.as_deref(),
                    previous_age,
                );
            }
            if at_prompt {
                flush_attachment_replay_input(attachment, pane_id, generation);
            }
        }
        return Update::none();
    }
    let at_prompt = matches!(
        state.command_phase,
        crate::session::protocol::PaneCommandPhase::Prompt
            | crate::session::protocol::PaneCommandPhase::Input
    );
    let mut transition = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
        && pane.pty_generation == generation
        && state.sequence > pane.terminal.runtime_sequence
    {
        let previous = pane.terminal.reported_status.clone();
        let previous_agent_status = pane.terminal.agent_status();
        // Sampled before the incoming runtime state overwrites the status it dates.
        let previous_age = pane.terminal.status_age();
        pane.terminal.runtime_sequence = state.sequence;
        pane.terminal.cwd = state.cwd;
        pane.terminal.cwd_host = state.cwd_host;
        pane.terminal.display_path = state.display_path;
        pane.terminal.project_root = state.project_root;
        pane.terminal.git_branch = state.git_branch;
        pane.terminal.foreground_program = state.foreground_program;
        pane.terminal.command_phase = state.command_phase;
        pane.terminal.last_exit_status = state.last_exit_status;
        pane.terminal.reported_status = state.status;
        pane.terminal.detected_agent = state.detected_agent;
        pane.terminal.work_started_at = state.work_started_at;
        update_agent_status_edge(
            &mut pane.terminal,
            previous_agent_status.as_deref(),
            previous_age,
        );
        if previous != pane.terminal.reported_status {
            transition = Some((
                previous,
                pane.terminal.reported_status.clone(),
                pane.display_title(None),
            ));
        }
    }
    if let Some((previous, current, title)) = transition {
        crate::events::emit_with_controller_hooks(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::PaneStatusChanged,
                vec![
                    ("pane", pane_id.to_string()),
                    (
                        "status",
                        current
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "reason",
                        current
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_status",
                        previous
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_reason",
                        previous
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                ],
            ),
        );
        maybe_notify_pane_status(
            &ctx.state.config,
            ctx.state.is_controller(),
            ctx.state.current().focused_pane == Some(pane_id),
            pane_id,
            &title,
            current.as_ref(),
        );
    }
    // The shell reached its first prompt: deliver any queued replay input now, so readline
    // echoes it exactly once at the prompt (see `flush_replay_input`).
    if at_prompt {
        flush_replay_input(ctx, pane_id, generation);
    }
    // An agent that just started working is the moment a row first gains an elapsed time, and the
    // Agents tab may already be open with nothing ticking.
    crate::update::sidebar::arm_agent_tick(ctx);
    Update::full()
}

/// How long a queued replay input waits for its pane's shell to report a prompt (OSC 133 A/B)
/// before being written as plain type-ahead anyway - a shell without integration never reports
/// one, and correctness does not depend on the prompt: type-ahead input is read whenever the
/// shell gets there. Waiting only avoids the cosmetic double echo of injecting mid-startup
/// (kernel tty echo first, readline's redraw second).
const REPLAY_PROMPT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(800);

fn replay_input_deadline_command(epoch: u64, pane_id: PaneId, generation: u64) -> Command {
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

pub(super) fn replay_input_deadline(
    ctx: &mut Context<HyprmuxApp>,
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

/// Deliver a queued replay command (see `State::pending_replay_inputs`) exactly once: sent as
/// ordinary pane input followed by a carriage return, the pane's interactive shell reads and runs
/// it as if the user had typed it - aliases, shell functions, and rc-file PATH resolve, and the
/// prompt's title/OSC integration has already run. The entry is consumed even when the pane is
/// gone or the client dropped; a later respawn queues its own fresh entry.
fn flush_replay_input(ctx: &mut Context<HyprmuxApp>, pane_id: PaneId, generation: u64) {
    let Some(input) = ctx
        .state
        .current_mut()
        .pending_replay_inputs
        .remove(&(pane_id, generation))
    else {
        return;
    };
    if !find_pane(&ctx.state, pane_id)
        .is_some_and(|pane| pane.pty_generation == generation && !pane.closing)
    {
        return;
    }
    if let Some(client) = ctx.state.current().session_client.clone() {
        let mut bytes = input.into_bytes();
        bytes.push(b'\r');
        client.send_input(pane_id, generation, bytes);
    }
}

fn flush_attachment_replay_input(
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
    if !attachment
        .find_pane_mut(pane_id)
        .is_some_and(|pane| pane.pty_generation == generation && !pane.closing)
    {
        return;
    }
    if let Some(client) = attachment.session_client.as_ref() {
        let mut bytes = input.into_bytes();
        bytes.push(b'\r');
        client.send_input(pane_id, generation, bytes);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_result(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    pid: Option<u32>,
    ok: bool,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        let Some(attachment) = ctx.state.background.get_mut(&epoch) else {
            return Update::none();
        };
        let is_controller = attachment.is_controller();
        let mut spawned_live = false;
        if let Some(pane) = attachment.find_pane_mut(pane_id) {
            if pane.pty_generation != generation {
                return Update::none();
            }
            spawned_live = ok && !pane.closing;
            if !pane.terminal.is_ready() {
                pane.terminal.bind_server_backend(pane_id, generation);
            }
            pane.terminal.child_pid = pid;
            if ok {
                pane.terminal.status = ManagedTerminalStatus::Ready;
            } else {
                let message = error.unwrap_or_else(|| "session spawn failed".to_string());
                pane.terminal.status = ManagedTerminalStatus::Error(message.into());
                if !pane.closing && is_controller {
                    attachment
                        .pending_background_closes
                        .push((pane_id, generation));
                }
            }
        }
        if attachment
            .pending_replay_inputs
            .contains_key(&(pane_id, generation))
        {
            if spawned_live {
                return Update::with_command(replay_input_deadline_command(
                    epoch, pane_id, generation,
                ));
            }
            attachment
                .pending_replay_inputs
                .remove(&(pane_id, generation));
        }
        return Update::none();
    }
    let is_controller = ctx.state.is_controller();
    let mut should_close = false;
    let mut toast_error = None;
    let mut spawned_live = false;
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        spawned_live = ok && !pane.closing;
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
            should_close = !pane.closing && is_controller;
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
    ctx.state.commands_dirty = true;
    if let Some(error) = toast_error {
        ctx.toast()
            .push(error_toast(&ctx.state.theme, "Spawn failed", error));
    }
    if should_close {
        begin_close_pane(ctx, pane_id, ctx.state.config.animations)
    } else if let Some(command) = replay_deadline {
        Update::with_command(command)
    } else {
        Update::full()
    }
}

pub(super) fn error(ctx: &mut Context<HyprmuxApp>, epoch: u64, message: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if message.trim().is_empty() {
        return Update::none();
    }
    ctx.toast()
        .push(error_toast(&ctx.state.theme, "Session error", message));
    Update::full()
}

pub(super) fn renamed(ctx: &mut Context<HyprmuxApp>, epoch: u64, session: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            attachment.session_name = Some(session);
            crate::update::sidebar::invalidate_sessions(ctx);
        }
        return Update::none();
    }
    let previous = ctx
        .state
        .current_mut()
        .session_name
        .replace(session.clone())
        .unwrap_or_default();
    crate::session::record_last_named_session(&session);
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::SessionRenamed,
            vec![("session", session.clone()), ("previous", previous)],
        ),
    );
    // An ephemeral session becoming named flips the "Name/Rename session" palette label.
    ctx.state.commands_dirty = true;
    ctx.toast().push(crate::pty_events::info_toast(
        &ctx.state.theme,
        format!("Renamed session to `{session}`"),
    ));
    Update::full()
}

fn should_hold_on_exit(hold_on_exit: bool, pane_id: PaneId, closing: bool) -> bool {
    hold_on_exit && !crate::scratchpad::is_scratch(pane_id) && !closing
}

fn controller_change_reason_id(reason: ControllerChangeReason) -> &'static str {
    match reason {
        ControllerChangeReason::Released => "released",
        ControllerChangeReason::Expired => "expired",
        ControllerChangeReason::Granted => "granted",
    }
}

fn roster_diff_events(
    previous: &[ClientInfo],
    current: &[ClientInfo],
) -> Vec<crate::events::Event> {
    let count = current.len().to_string();
    let mut events = Vec::new();
    for client in current {
        if !previous.iter().any(|existing| existing.id == client.id) {
            events.push(crate::events::Event::new(
                crate::events::EventKind::ClientJoined,
                vec![
                    ("client_id", client.id.to_string()),
                    ("client_name", client.label.clone()),
                    ("count", count.clone()),
                ],
            ));
        }
    }
    for client in previous {
        if !current.iter().any(|existing| existing.id == client.id) {
            events.push(crate::events::Event::new(
                crate::events::EventKind::ClientLeft,
                vec![
                    ("client_id", client.id.to_string()),
                    ("client_name", client.label.clone()),
                    ("count", count.clone()),
                ],
            ));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use tui_lipan::TestBackend;

    fn agent_pane(status: &str) -> crate::pane::TerminalPane {
        let mut pane = crate::pane::TerminalPane::new(100);
        pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::Claude,
            state: crate::session::protocol::DetectedAgentState::Idle,
        });
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: status.to_string(),
            reason: None,
            set_at: 1,
        });
        pane
    }

    #[test]
    fn parked_disconnect_preserves_identity_and_marks_attachment_offline() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let (client, _outbound) = SessionClient::test_channel();
                let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 4;
                    state.current_mut().epoch = 4;
                    state.current_mut().session_name = Some("dev".to_string());
                    state.current_mut().session_client = Some(client);
                    state.current_mut().session_attached = true;
                    state.current_mut().pending_session_attach = None;
                    state.current_mut().connection = crate::state::ConnectionState::Connected;
                    state.current_mut().remote_host = Some("workbox".to_string());
                    state.current_mut().remote_target = Some(target.clone());
                    state.park_current(4, crate::state::Attachment::new());
                    state.runtime_epoch = 5;
                }

                assert_eq!(backend.state().runtime_epoch, 5);
                let before = backend
                    .state()
                    .background
                    .get(&4)
                    .expect("parked before drop");
                assert_eq!(before.session_name.as_deref(), Some("dev"));
                assert!(before.pending_session_attach.is_none());

                backend
                    .dispatch(Msg::SessionDisconnected {
                        epoch: 4,
                        name: "dev".to_string(),
                    })
                    .expect("dispatch parked disconnect");

                let parked = backend
                    .state()
                    .background
                    .get(&4)
                    .expect("retained session");
                assert_eq!(
                    parked.connection,
                    crate::state::ConnectionState::Disconnected
                );
                assert!(!parked.session_attached);
                assert!(parked.session_client.is_none());
                assert_eq!(parked.remote_target.as_ref(), Some(&target));
                assert_eq!(parked.session_name.as_deref(), Some("dev"));
            })
            .expect("spawn parked-disconnect test")
            .join()
            .expect("parked-disconnect test completes");
    }

    #[test]
    fn parked_rename_updates_retained_identity() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 8;
                    state.current_mut().session_name = Some("before".to_string());
                    state.park_current(8, crate::state::Attachment::new());
                    state.runtime_epoch = 9;
                }

                backend
                    .dispatch(Msg::SessionRenamed {
                        epoch: 8,
                        session: "after".to_string(),
                    })
                    .expect("dispatch parked rename");

                assert_eq!(
                    backend
                        .state()
                        .background
                        .get(&8)
                        .and_then(|attachment| attachment.session_name.as_deref()),
                    Some("after")
                );
            })
            .expect("spawn parked-rename test")
            .join()
            .expect("parked-rename test completes");
    }

    #[test]
    fn finished_unseen_arms_on_working_to_quiescent_and_disarms_on_resume() {
        // working -> idle arms the pulse.
        let mut pane = agent_pane("idle");
        update_agent_status_edge(&mut pane, Some("working"), None);
        assert!(pane.finished_unseen);

        // A later idle -> idle poll leaves it armed until the pane is looked at.
        update_agent_status_edge(&mut pane, Some("idle"), None);
        assert!(pane.finished_unseen);

        // Resuming work disarms it: a spinning agent must not wear a completed dot.
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "working".into(),
            reason: None,
            set_at: 2,
        });
        update_agent_status_edge(&mut pane, Some("idle"), None);
        assert!(!pane.finished_unseen);
    }

    /// The duration column is only honest if the stamp moves on a real state change and holds
    /// still across the repeated polls that report the same state.
    #[test]
    fn status_since_stamps_transitions_and_survives_unchanged_polls() {
        let mut pane = agent_pane("working");
        assert!(pane.status_since.is_none());
        update_agent_status_edge(&mut pane, Some("idle"), None);
        let stamped = pane.status_since.expect("a transition stamps the pane");

        update_agent_status_edge(&mut pane, Some("working"), None);
        assert_eq!(pane.status_since, Some(stamped));

        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "idle".into(),
            reason: None,
            set_at: 2,
        });
        update_agent_status_edge(&mut pane, Some("working"), None);
        assert!(pane.status_since.expect("re-stamped") > stamped);
    }

    /// A finished run reports what it cost. The number is banked as the run ends and never moves
    /// again, so it cannot drift into meaning "how long ago it stopped".
    #[test]
    fn finishing_a_run_banks_its_length_and_freezes_it() {
        let run = std::time::Duration::from_secs(12 * 60);
        let mut pane = agent_pane("idle");
        update_agent_status_edge(&mut pane, Some("working"), Some(run));
        assert_eq!(pane.last_run, Some(run));

        // Repeated idle polls afterwards leave the banked run alone.
        update_agent_status_edge(
            &mut pane,
            Some("idle"),
            Some(std::time::Duration::from_secs(1)),
        );
        assert_eq!(pane.last_run, Some(run));

        // Only a `working` stretch is banked; leaving any other state does not overwrite it.
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "working".into(),
            reason: None,
            set_at: 2,
        });
        update_agent_status_edge(
            &mut pane,
            Some("blocked"),
            Some(std::time::Duration::from_secs(3)),
        );
        assert_eq!(pane.last_run, Some(run));
    }

    #[test]
    fn finished_unseen_ignores_working_to_blocked() {
        let mut pane = agent_pane("blocked");
        update_agent_status_edge(&mut pane, Some("working"), None);
        assert!(!pane.finished_unseen);
    }

    #[test]
    fn hold_on_exit_excludes_disabled_scratch_and_closing_panes() {
        assert!(should_hold_on_exit(true, 1, false));
        assert!(!should_hold_on_exit(false, 1, false));
        assert!(!should_hold_on_exit(
            true,
            crate::state::SCRATCH_PANE_ID,
            false
        ));
        assert!(!should_hold_on_exit(true, 1, true));
    }

    #[test]
    fn roster_diff_emits_joins_and_leaves_with_the_new_count() {
        let client = |id, label: &str| ClientInfo {
            id,
            label: label.to_string(),
            read_only: false,
            requesting_control: false,
        };
        let events = roster_diff_events(
            &[client(1, "one"), client(2, "two")],
            &[client(2, "renamed"), client(3, "three")],
        );

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, crate::events::EventKind::ClientJoined);
        assert_eq!(
            events[0].fields,
            vec![
                ("client_id", "3".into()),
                ("client_name", "three".into()),
                ("count", "2".into()),
            ]
        );
        assert_eq!(events[1].kind, crate::events::EventKind::ClientLeft);
        assert_eq!(
            events[1].fields,
            vec![
                ("client_id", "1".into()),
                ("client_name", "one".into()),
                ("count", "2".into()),
            ]
        );
    }

    #[test]
    fn runtime_status_transitions_emit_once_and_stale_updates_are_ignored() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let epoch = backend.state().runtime_epoch;
                let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                pane.pty_generation = 7;
                pane.terminal.bind_session(pane.id, 7);
                let events = backend.state().event_hub.subscribe(Some(HashSet::from([
                    crate::events::EventKind::PaneStatusChanged,
                ])));
                let status = crate::session::protocol::PaneStatus {
                    value: "blocked".into(),
                    reason: Some("needs approval".into()),
                    set_at: 1,
                };
                let runtime = PaneRuntimeState {
                    status: Some(status.clone()),
                    detected_agent: Some(crate::session::protocol::DetectedAgent {
                        kind: crate::session::protocol::AgentKind::OpenCode,
                        state: crate::session::protocol::DetectedAgentState::Blocked,
                    }),
                    sequence: 1,
                    ..PaneRuntimeState::default()
                };

                backend
                    .dispatch(Msg::SessionPaneRuntimeChanged {
                        epoch,
                        pane_id: 1,
                        generation: 7,
                        state: runtime.clone(),
                    })
                    .expect("dispatch status transition");
                let event: serde_json::Value =
                    serde_json::from_str(&events.try_recv().expect("transition event")).unwrap();
                assert_eq!(event["event"], "pane-status-changed");
                assert_eq!(event["data"]["pane"], "1");
                assert_eq!(event["data"]["status"], "blocked");
                assert_eq!(event["data"]["reason"], "needs approval");
                assert_eq!(event["data"]["previous_status"], "");
                assert_eq!(
                    backend.state().current().workspaces[0].panes[0]
                        .terminal
                        .detected_agent
                        .as_ref()
                        .map(|agent| agent.kind),
                    Some(crate::session::protocol::AgentKind::OpenCode)
                );

                backend
                    .dispatch(Msg::SessionPaneRuntimeChanged {
                        epoch,
                        pane_id: 1,
                        generation: 7,
                        state: runtime,
                    })
                    .expect("dispatch duplicate status");
                assert!(events.try_recv().is_err());

                backend
                    .dispatch(Msg::SessionPaneRuntimeChanged {
                        epoch,
                        pane_id: 1,
                        generation: 7,
                        state: PaneRuntimeState {
                            status: None,
                            sequence: 0,
                            ..PaneRuntimeState::default()
                        },
                    })
                    .expect("dispatch stale status");
                assert_eq!(
                    backend.state().current().workspaces[0].panes[0]
                        .terminal
                        .reported_status,
                    Some(status)
                );
                assert!(
                    backend.state().current().workspaces[0].panes[0]
                        .terminal
                        .detected_agent
                        .is_some()
                );
                assert!(events.try_recv().is_err());
            })
            .expect("spawn runtime status test thread")
            .join()
            .expect("runtime status test thread completes");
    }

    #[test]
    fn parked_runtime_updates_keep_background_metadata_current() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 4;
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.pty_generation = 7;
                    pane.terminal.bind_session(pane.id, 7);
                    state.park_current(4, crate::state::Attachment::new());
                    state.runtime_epoch = 5;
                }

                backend
                    .dispatch(Msg::SessionPaneRuntimeChanged {
                        epoch: 4,
                        pane_id: 1,
                        generation: 7,
                        state: PaneRuntimeState {
                            cwd: Some("/remote/project".to_string()),
                            foreground_program: Some("cargo".to_string()),
                            sequence: 1,
                            ..PaneRuntimeState::default()
                        },
                    })
                    .expect("dispatch parked runtime update");

                let pane = &backend.state().background[&4].workspaces[0].panes[0];
                assert_eq!(pane.terminal.cwd.as_deref(), Some("/remote/project"));
                assert_eq!(pane.terminal.foreground_program.as_deref(), Some("cargo"));
            })
            .expect("spawn parked runtime test")
            .join()
            .expect("parked runtime test completes");
    }

    /// A failed *local* attach must not install another pending attach: a local ephemeral is
    /// itself the fallback, so retrying it on failure would spin forever (fail → fall back → fail →
    /// …). Only a remote failure falls back, which is verified live. Side-effect-free: the local
    /// path returns no command, so nothing is spawned.
    #[test]
    fn local_attach_failure_does_not_retry_into_an_ephemeral_loop() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                backend.state_mut().current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 42,
                        name: "eph-local".into(),
                        client: None,
                        autostart: true,
                        read_only: false,
                        reconnect: false,
                        remote_host: None,
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: None,
                    });
                backend
                    .dispatch(Msg::SessionAttachFailed {
                        epoch: 42,
                        message: "no local server".into(),
                    })
                    .expect("dispatch local attach failure");
                assert!(
                    backend.state().current().pending_session_attach.is_none(),
                    "a local ephemeral failure must clear the pending attach and not re-arm one"
                );
            })
            .expect("spawn no-loop test thread")
            .join()
            .expect("no-loop test thread completes");
    }

    #[test]
    fn retained_remote_reconnect_failure_stays_offline_and_remote() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
                {
                    let state = backend.state_mut();
                    state.current_mut().session_name = Some("dev".to_string());
                    state.current_mut().remote_host = Some("workbox".to_string());
                    state.current_mut().remote_target = Some(target.clone());
                    state.current_mut().pending_session_attach =
                        Some(crate::state::PendingSessionAttach {
                            epoch: 42,
                            name: "dev".to_string(),
                            client: None,
                            autostart: false,
                            read_only: false,
                            reconnect: true,
                            remote_host: Some("workbox".to_string()),
                            intent: crate::state::AttachIntent::Plain,
                            left: None,
                            parked_epoch: None,
                        });
                }

                backend
                    .dispatch(Msg::SessionAttachFailed {
                        epoch: 42,
                        message: "offline".to_string(),
                    })
                    .expect("dispatch reconnect failure");

                assert!(backend.state().current().pending_session_attach.is_none());
                assert_eq!(
                    backend.state().current().connection,
                    crate::state::ConnectionState::Unreachable
                );
                assert_eq!(
                    backend.state().current().remote_target.as_ref(),
                    Some(&target)
                );
                assert_eq!(
                    backend.state().current().session_name.as_deref(),
                    Some("dev")
                );
            })
            .expect("spawn retained reconnect test")
            .join()
            .expect("retained reconnect test completes");
    }

    /// A failed *remote* connect that had parked a live session restores that session rather than
    /// falling back to a fresh local ephemeral. The ephemeral fallback would re-attach to this
    /// process's own `eph-<pid>` server — still controlled by the parked client — and come back as a
    /// follower of itself. Restoring the parked attachment keeps the user on their real session, and
    /// the dead empty connect attachment is discarded.
    #[test]
    fn failed_remote_connect_restores_the_parked_session_not_a_follower_ephemeral() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let target = crate::session::remote::RemoteTarget::Alias("windev".to_string());
                {
                    let state = backend.state_mut();
                    // The live local session, parked into the background under epoch 1 when the
                    // connect started.
                    let mut parked = crate::state::Attachment::new();
                    parked.epoch = 1;
                    parked.session_name = Some("eph-local".to_string());
                    parked.session_attached = true;
                    parked.connection = crate::state::ConnectionState::Connected;
                    state.background.insert(1, parked);

                    // The current attachment is the fresh empty one the connect installed; it never
                    // attached.
                    state.runtime_epoch = 2;
                    state.current_mut().epoch = 2;
                    state.current_mut().remote_host = Some("windev".to_string());
                    state.current_mut().remote_target = Some(target.clone());
                    state.current_mut().pending_session_attach =
                        Some(crate::state::PendingSessionAttach {
                            epoch: 2,
                            name: "eph-windev".to_string(),
                            client: None,
                            autostart: true,
                            read_only: false,
                            reconnect: false,
                            remote_host: Some("windev".to_string()),
                            intent: crate::state::AttachIntent::Plain,
                            left: None,
                            parked_epoch: Some(1),
                        });
                }

                backend
                    .dispatch(Msg::SessionAttachFailed {
                        epoch: 2,
                        message: "could not resolve hostname windev".to_string(),
                    })
                    .expect("dispatch remote connect failure");

                // Restored onto the parked local session, not a fresh ephemeral, and no longer
                // pointed at the remote host.
                assert_eq!(
                    backend.state().current().session_name.as_deref(),
                    Some("eph-local")
                );
                assert!(backend.state().current().session_attached);
                assert_eq!(backend.state().current().remote_target, None);
                assert!(backend.state().current().pending_session_attach.is_none());
                // The parked entry is gone (now current) and the dead connect attachment was not
                // retained in its place.
                assert!(!backend.state().background.contains_key(&1));
                assert!(!backend.state().background.contains_key(&2));
            })
            .expect("spawn failed-connect-restore test")
            .join()
            .expect("failed-connect-restore test completes");
    }

    /// A failed *local* create also restores the parked session — creating now parks the current
    /// session like a switch, so a create that can't start its server must not strand the user on
    /// the broken empty attachment either. (The remote-only ephemeral fallback stays remote-only.)
    #[test]
    fn failed_local_create_restores_the_parked_session() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                {
                    let state = backend.state_mut();
                    let mut parked = crate::state::Attachment::new();
                    parked.epoch = 1;
                    parked.session_name = Some("eph-local".to_string());
                    parked.session_attached = true;
                    parked.connection = crate::state::ConnectionState::Connected;
                    state.background.insert(1, parked);

                    state.runtime_epoch = 2;
                    state.current_mut().epoch = 2;
                    state.current_mut().pending_session_attach =
                        Some(crate::state::PendingSessionAttach {
                            epoch: 2,
                            name: "work".to_string(),
                            client: None,
                            autostart: true,
                            read_only: false,
                            reconnect: false,
                            // Local create: no remote host.
                            remote_host: None,
                            intent: crate::state::AttachIntent::Plain,
                            left: None,
                            parked_epoch: Some(1),
                        });
                }

                backend
                    .dispatch(Msg::SessionAttachFailed {
                        epoch: 2,
                        message: "could not start session server".to_string(),
                    })
                    .expect("dispatch local create failure");

                assert_eq!(
                    backend.state().current().session_name.as_deref(),
                    Some("eph-local")
                );
                assert!(backend.state().current().session_attached);
                assert!(backend.state().current().pending_session_attach.is_none());
                assert!(!backend.state().background.contains_key(&1));
                assert!(!backend.state().background.contains_key(&2));
            })
            .expect("spawn failed-local-create test")
            .join()
            .expect("failed-local-create test completes");
    }

    #[test]
    fn empty_ephemeral_profile_seed_emits_profile_loaded_after_attach() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let (client, rx) = SessionClient::test_channel();
                let path = PathBuf::from("legacy-profile.toml");
                backend.state_mut().current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 1,
                        name: "eph-test".into(),
                        client: Some(client),
                        autostart: true,
                        read_only: false,
                        reconnect: false,
                        remote_host: None,
                        intent: crate::state::AttachIntent::ProfileSeed {
                            profile: "legacy-profile".into(),
                            path: path.clone(),
                        },
                        left: None,
                        parked_epoch: None,
                    });
                backend.state_mut().show_profile_picker = true;
                backend.state_mut().profile_picker =
                    Some(crate::state::ProfilePickerState::new(Vec::new()));
                let events = backend.state().event_hub.subscribe(Some(HashSet::from([
                    crate::events::EventKind::ProfileLoaded,
                ])));

                backend
                    .dispatch(Msg::SessionAttached {
                        epoch: 1,
                        session: "eph-test".into(),
                        client_id: 1,
                        panes: Vec::new(),
                        layout_rev: 0,
                        layout: None,
                        controller: Some(1),
                        clients: Vec::new(),
                        input_locked: false,
                        read_only: false,
                        created_from_profile: None,
                    })
                    .expect("dispatch attach");

                assert_eq!(backend.state().current().created_from_profile, None);
                assert!(!backend.state().show_profile_picker);
                assert!(backend.state().profile_picker.is_none());
                assert!(rx.try_iter().any(|message| matches!(
                    message,
                    crate::session::client::ClientOutbound::Control(
                        crate::session::protocol::ClientMessage::SetSessionOrigin { profile }
                    ) if profile == "legacy-profile"
                )));
                assert!(events.try_recv().is_err());
                backend
                    .dispatch(Msg::SessionOriginSet {
                        epoch: 1,
                        created_from_profile: "legacy-profile".to_string(),
                    })
                    .expect("acknowledge session origin");
                assert_eq!(
                    backend.state().current().created_from_profile.as_deref(),
                    Some("legacy-profile")
                );

                let event: serde_json::Value =
                    serde_json::from_str(&events.try_recv().expect("profile-loaded event"))
                        .expect("event json");
                assert_eq!(
                    event,
                    serde_json::json!({
                        "event": "profile-loaded",
                        "data": {
                            "profile": "legacy-profile",
                            "path": path.display().to_string(),
                            "session": "eph-test"
                        }
                    })
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }
}
