use tui_lipan::prelude::*;

use super::attach::{
    apply_attached_panes, bind_attached_pane_backends, flush_pending_spawns,
    reset_state_for_shared_seed, spawn_state_panes_on_session,
};
use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::pane_lifecycle::{begin_close_pane, find_pane_mut};
use crate::pty_events::{error_toast, maybe_notify_pane_exit};
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
    let Some(pending) = ctx.state.pending_session_attach.as_mut() else {
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
        return Update::none();
    }
    // Only the current session's unexpected disconnect matters; an intentional detach or
    // attach-elsewhere has already bumped the epoch, so its stale disconnect is filtered out above.
    if ctx.state.session_name.as_deref() != Some(name.as_str()) {
        return Update::none();
    }
    if ctx.state.pending_session_attach.is_some() {
        return Update::full();
    }
    ctx.state.session_attached = false;
    ctx.state.session_client = None;
    let read_only = ctx
        .state
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only);
    // Drop shared-lease bookkeeping: while disconnected we behave as a solo controller, and a
    // successful reconnect rebuilds this from the fresh `Attached` frame.
    ctx.state.shared = None;
    for pane in ctx
        .state
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
    {
        pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
    }
    // Try to reconnect: an ephemeral server may still be alive (transient hiccup), so reattach and
    // re-seed. Only ephemeral sessions autostart a replacement server.
    let autostart = crate::state::is_ephemeral_session_name(&name);
    let new_epoch = ctx.state.runtime_epoch.saturating_add(1);
    ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch: new_epoch,
        name: name.clone(),
        client: None,
        autostart,
        read_only,
        intent: crate::state::AttachIntent::Plain,
        left: None,
    });
    ctx.toast().push(crate::pty_events::info_toast(
        &ctx.state.theme,
        format!("Reconnecting to {name}…"),
    ));
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(
                new_epoch, name, autostart, read_only, link,
            )
        });
    }))
}

pub(super) fn attach_failed(ctx: &mut Context<HyprmuxApp>, epoch: u64, message: String) -> Update {
    let expected_pending = ctx
        .state
        .pending_session_attach
        .as_ref()
        .is_some_and(|pending| pending.epoch == epoch);
    if !expected_pending {
        return Update::none();
    }
    ctx.state.pending_session_attach = None;
    ctx.toast()
        .push(error_toast(&ctx.state.theme, "Sessions", message));
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
) -> Update {
    let Some(pending) = ctx.state.pending_session_attach.as_ref() else {
        return Update::none();
    };
    if pending.epoch != epoch || pending.name != session {
        return Update::none();
    }
    let pending = ctx
        .state
        .pending_session_attach
        .take()
        .expect("pending attach checked above");
    let Some(client) = pending.client else {
        return Update::none();
    };
    ctx.state.runtime_epoch = epoch;
    ctx.state.session_client = Some(client);
    ctx.state.session_name = Some(session.clone());
    ctx.state.session_attached = true;
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
    ctx.state.shared = Some(shared);

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
        reset_state_for_shared_seed(&mut ctx.state);
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
                " - ended temporary session".to_string()
            } else {
                format!(" - detached from `{}` (still running)", left.name)
            }
        })
        .unwrap_or_default();
    if !populated && let crate::state::AttachIntent::ProfileSeed { profile, path } = &pending.intent
    {
        crate::events::emit(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::ProfileLoaded,
                vec![
                    ("profile", profile.clone()),
                    ("path", path.display().to_string()),
                    ("session", session.clone()),
                ],
            ),
        );
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
    update
}

pub(super) fn layout_committed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rev: u64,
    author: ClientId,
    layout: SharedLayout,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let my_id = ctx.state.shared.as_ref().map(|shared| shared.client_id);
    if my_id == Some(author) {
        // Echo of our own commit: confirm the revision, never re-apply our own layout.
        if let Some(shared) = ctx.state.shared.as_mut() {
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
        return Update::none();
    }
    let update = if let Some(layout) = layout {
        crate::shared_layout::apply_shared_layout(ctx, &layout, current_rev)
    } else {
        Update::full()
    };
    if let Some(shared) = ctx.state.shared.as_mut() {
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
        return Update::none();
    }
    let was_controller = ctx.state.is_controller();
    if let Some(shared) = ctx.state.shared.as_mut() {
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
        return Update::none();
    }
    let roster_events = ctx
        .state
        .shared
        .as_ref()
        .map(|shared| roster_diff_events(&shared.clients, &clients))
        .unwrap_or_default();
    if let Some(shared) = ctx.state.shared.as_mut() {
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
                        "Input locked to controller"
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
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(client) = ctx.state.session_client.as_ref() {
        client.pong(seq);
    }
    Update::none()
}

pub(super) fn flush_pane_resizes(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    crate::pty_events::flush_pending_resizes(ctx);
    Update::none()
}

pub(super) fn flush_layout_commit(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(shared) = ctx.state.shared.as_mut() {
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
        return Update::none();
    }
    let focused = ctx.state.focused_pane;
    let bell_notifications = ctx.state.config.notifications.bell;
    let matched = match find_pane_mut(&mut ctx.state, pane_id) {
        Some(pane) if pane.pty_generation == generation => {
            pane.terminal.process_server_output(&bytes);
            let bell = pane.terminal.take_bell();
            pane.activity.last_activity = Some(std::time::Instant::now());
            if focused != Some(pane_id) {
                pane.activity.has_unseen_output = true;
                if bell && bell_notifications {
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
        // a follower's fresh pane blank until the next redraw.
        if let Some(shared) = ctx.state.shared.as_mut() {
            shared.buffer_orphan_output(pane_id, generation, &bytes);
        }
    }
    Update::full()
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
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
        && pane.pty_generation == generation
        && pane.terminal.apply_server_resize(cols, rows)
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

pub(super) fn pane_runtime_changed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    state: PaneRuntimeState,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
        && pane.pty_generation == generation
        && state.sequence > pane.terminal.runtime_sequence
    {
        pane.terminal.runtime_sequence = state.sequence;
        pane.terminal.cwd = state.cwd;
        pane.terminal.cwd_host = state.cwd_host;
        pane.terminal.foreground_program = state.foreground_program;
        pane.terminal.command_phase = state.command_phase;
        pane.terminal.last_exit_status = state.last_exit_status;
    }
    Update::full()
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
        return Update::none();
    }
    let is_controller = ctx.state.is_controller();
    let mut should_close = false;
    let mut toast_error = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
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
    ctx.state.commands_dirty = true;
    if let Some(error) = toast_error {
        ctx.toast()
            .push(error_toast(&ctx.state.theme, "Session Spawn", error));
    }
    if should_close {
        begin_close_pane(ctx, pane_id, ctx.state.config.animations)
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
        .push(error_toast(&ctx.state.theme, "Session", message));
    Update::full()
}

pub(super) fn renamed(ctx: &mut Context<HyprmuxApp>, epoch: u64, session: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let previous = ctx
        .state
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
    fn empty_ephemeral_profile_seed_emits_profile_loaded_after_attach() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::HyprmuxApp::default());
                let (client, _rx) = SessionClient::test_channel();
                let path = PathBuf::from("legacy profile.toml");
                backend.state_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 1,
                        name: "eph-test".into(),
                        client: Some(client),
                        autostart: true,
                        read_only: false,
                        intent: crate::state::AttachIntent::ProfileSeed {
                            profile: "legacy profile".into(),
                            path: path.clone(),
                        },
                        left: None,
                    });
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
                    })
                    .expect("dispatch attach");

                let event: serde_json::Value =
                    serde_json::from_str(&events.try_recv().expect("profile-loaded event"))
                        .expect("event json");
                assert_eq!(
                    event,
                    serde_json::json!({
                        "event": "profile-loaded",
                        "data": {
                            "profile": "legacy profile",
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
