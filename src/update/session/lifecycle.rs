use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::anim::GeometryAnimation;
use crate::layout::shared::{ClientId, SharedLayout};
use crate::session::client::SessionClient;
use crate::session::protocol::{ClientInfo, PaneMeta};
use crate::update::attach::{
    apply_attached_panes, bind_attached_pane_backends, flush_pending_spawns,
    reset_state_for_shared_seed, spawn_state_panes_on_session,
};

pub(crate) fn connected(
    ctx: &mut Context<AppRoot>,
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

pub(crate) fn disconnected(ctx: &mut Context<AppRoot>, epoch: u64, name: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        let disconnected = ctx
            .state
            .background
            .get_mut(&epoch)
            .filter(|attachment| attachment.session_name.as_deref() == Some(name.as_str()))
            .filter(|attachment| attachment.pending_session_attach.is_none());
        if let Some(attachment) = disconnected {
            attachment.mark_disconnected();
            ctx.state.sidebar.invalidate_sessions();
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
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.current_mut().mark_disconnected();
    crate::ops::session::reconnect_current_session(ctx)
}

pub(crate) fn transport_failed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    name: String,
    message: String,
) -> Update {
    crate::pane::pty_events::notify_error(ctx, "Session transport", message);
    disconnected(ctx, epoch, name)
}

pub(crate) fn attach_failed(ctx: &mut Context<AppRoot>, epoch: u64, message: String) -> Update {
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
    crate::ops::session::clear_pending_session_action(ctx, Some(&message));
    ctx.state.current_mut().connection = if was_remote {
        crate::state::ConnectionState::Unreachable
    } else {
        crate::state::ConnectionState::Disconnected
    };
    ctx.state.commands_dirty = true;
    crate::pane::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::SessionLifecycle,
        Some("Attach failed".to_string()),
        message,
    );
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
        // Nothing to restore (a bare `--remote` launch, say). The user asked for a session on a
        // host that is not answering; inventing a *local* ephemeral in its place is not what they
        // asked for, so drop into the launcher with the picker up and let them choose. Remote-only:
        // a local attach failing here has already reported itself, and the launcher is where a
        // dismissed picker leaves the client anyway.
        if was_remote
            && !ctx.state.current().session_attached
            && ctx.state.current().session_client.is_none()
        {
            return crate::ops::session::enter_launcher(ctx);
        }
    }
    Update::full()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn attached(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    session: String,
    client_id: ClientId,
    panes: Vec<PaneMeta>,
    layout_rev: u64,
    layout: Option<SharedLayout>,
    controller: Option<ClientId>,
    clients: Vec<ClientInfo>,
    input_locked: bool,
    allow_takeover: bool,
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
    if !reconnect && let Some(target) = ctx.state.current().remote_target.as_ref() {
        crate::session::record_recent_remote(target);
    }
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
    // Working somewhere sets the scope you come back to: killing this session leaves the launcher
    // pointed at the same machine rather than silently back on this one.
    ctx.state.launcher_scope = ctx.state.current().remote_target.clone();
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.current_mut().deferred_profile_seed = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    if !crate::state::is_ephemeral_session_name(&session) {
        // Remembered per scope: the last session on `workbox` is what `--remote workbox` reopens,
        // and it must not become what a bare launch reaches for.
        crate::session::record_last_session(ctx.state.current().remote_target.as_ref(), &session);
    }

    let mut shared = crate::state::SharedSessionState::new(client_id);
    shared.layout_rev = layout_rev;
    shared.assumed_rev = layout_rev;
    shared.controller = controller;
    shared.clients = clients;
    shared.input_locked = input_locked;
    shared.allow_takeover = allow_takeover;
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
    let mut update = if let Some(layout) = layout {
        // Shared attach: seed the whole window-manager structure from the authoritative layout via
        // the one reconciler code path, then bind server backends and sizes from the pane metadata
        // before the replay seed frames arrive.
        if !reconnect {
            reset_state_for_shared_seed(&mut ctx.state);
        }
        crate::layout::shared::apply_shared_layout(ctx, &layout, layout_rev);
        // Attaching reveals an already-running session, so snap to its authoritative geometry
        // instead of interpolating from the previous session's pane rectangles.
        ctx.state.animation = GeometryAnimation::None;
        bind_attached_pane_backends(ctx, panes);
        flush_pending_spawns(ctx);
        crate::pane::pty_events::flush_pending_resizes(ctx);
        Update::full()
    } else if had_panes {
        // Defensive: a live server holding panes but no committed layout (should not occur under
        // protocol v6). Adopt the panes, then republish a layout if we control it.
        apply_attached_panes(ctx, panes);
        ctx.state.animation = GeometryAnimation::None;
        flush_pending_spawns(ctx);
        crate::pane::pty_events::flush_pending_resizes(ctx);
        Update::full()
    } else {
        // An empty server (fresh ephemeral, autostarted named session, or one whose panes all
        // exited): seed it with the panes the client already holds in state; the first attacher
        // (controller) commits rev 1 on the tail chokepoint pass.
        let spawned = spawn_state_panes_on_session(ctx);
        flush_pending_spawns(ctx);
        crate::pane::pty_events::flush_pending_resizes(ctx);
        if spawned.is_empty() {
            Update::full()
        } else {
            let open_delay = crate::layout::anim::open_delay(ctx.state.config.animations);
            let activate_delay = crate::layout::anim::activation_delay(ctx.state.config.animations);
            Update::with_command(crate::pane::lifecycle::open_timers_batch_command(
                epoch,
                spawned,
                open_delay,
                activate_delay,
            ))
        }
    };

    // Adopted panes are born activated, so no open/activate timer ever runs to hand them the
    // keyboard - and the framework's `OnDemand` policy has no first-widget fallback. Without this
    // the attached session draws its focused pane as focused while nothing holds framework focus,
    // and typing goes nowhere until the pane is clicked. Requested before the follow prompt, which
    // takes the keyboard for itself and restores it to the pane on dismissal.
    crate::ops::focus::request_current_pane_focus(ctx);

    let named = !crate::state::is_ephemeral_session_name(&session);
    if !populated && let crate::state::AttachIntent::ProfileSeed { profile, path } = &pending.intent
    {
        if let Some(client) = ctx.state.current().session_client.as_ref() {
            client.set_session_origin(profile.clone());
        }
        ctx.state.current_mut().pending_profile_loaded =
            Some((profile.clone(), path.clone(), session.clone()));
    } else if named && !populated && matches!(pending.intent, crate::state::AttachIntent::Plain) {
        crate::events::emit(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::SessionCreated,
                vec![("session", session.clone())],
            ),
        );
    }
    // Where this client landed is already on screen in the workbar session badge, so announcing it
    // would just repeat the badge. What it *left* has no surface at all once it is off screen -
    // that is the half worth a toast.
    if let Some(left) = pending.left.as_ref() {
        let message = if left.was_ephemeral_shutdown {
            "Ended the temporary session".to_string()
        } else {
            format!("Detached from `{}`", left.name)
        };
        crate::pane::pty_events::notify_info(ctx, message);
    }
    if let Some(origin) = ctx.state.current().created_from_profile.clone() {
        confirm_profile_origin(ctx, origin);
    }
    crate::update::sidebar::request_sessions_refresh(ctx);
    // Landing on a session someone else is driving is a fork in the road, not a fait accompli: ask
    // before this client settles in as a follower. Raised last so the attach is fully installed —
    // cancelling from the prompt leaves the session the same way switching away from it would.
    if !reconnect {
        crate::ops::session::prompt_follow_if_occupied(ctx);
    }
    if ctx.state.pending_session_action.is_some() {
        let deferred = crate::ops::session::run_pending_session_action(ctx);
        // Empty-seed deferred starts leave `update` as a command-less full refresh; prefer the
        // deferred spawn's timers when present.
        if deferred.command.is_some() {
            return deferred;
        }
        if deferred.dirty {
            update = deferred;
        }
    }
    update
}

pub(crate) fn origin_set(
    ctx: &mut Context<AppRoot>,
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

pub(crate) fn confirm_profile_origin(ctx: &mut Context<AppRoot>, created_from_profile: String) {
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

pub(crate) fn error(ctx: &mut Context<AppRoot>, epoch: u64, message: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if message.trim().is_empty() {
        return Update::none();
    }
    crate::pane::pty_events::notify_error(ctx, "Session error", message);
    Update::full()
}

pub(crate) fn renamed(ctx: &mut Context<AppRoot>, epoch: u64, session: String) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            attachment.session_name = Some(session);
            ctx.state.sidebar.invalidate_sessions();
        }
        return Update::none();
    }
    let previous = ctx
        .state
        .current_mut()
        .session_name
        .replace(session.clone())
        .unwrap_or_default();
    crate::session::record_last_session(ctx.state.current().remote_target.as_ref(), &session);
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::SessionRenamed,
            vec![("session", session.clone()), ("previous", previous)],
        ),
    );
    // An ephemeral session becoming named flips the "Name/Rename session" palette label.
    ctx.state.commands_dirty = true;
    // The new name is already rendered in the workbar session badge by the time this returns.
    Update::full()
}
