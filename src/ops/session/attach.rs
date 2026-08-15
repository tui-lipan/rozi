use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::session::discovery::immediate_picker_rows;

/// Release the currently attached session before switching away from it. The single rule used
/// everywhere a transition leaves the current session: a solely attached controller tears down
/// its ephemeral server, while followers, viewers, shared ephemeral clients, and named-session
/// clients detach so they cannot destroy another client's session.
pub(crate) fn release_current_session(ctx: &mut Context<AppRoot>) {
    ctx.state.sidebar.invalidate_sessions();
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::ops::session::flush_layout_commit(ctx);
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    let shutdown_ephemeral = may_shutdown_ephemeral(&ctx.state);
    crate::ops::exit::mark_session_detached(ctx, None);
    if shutdown_ephemeral {
        client.shutdown();
    } else {
        client.detach();
    }
}

/// Retain the current attached session in the background instead of tearing it down, so switching
/// back to it is instant and its screens stay live. The current attachment (client + screens) is
/// moved into `State::background` under its epoch, and a fresh empty attachment takes its place for
/// the session being switched to. Named and ephemeral sessions are both retained; parked sessions
/// are only torn down on quit (see [`crate::ops::exit`]).
pub(crate) fn park_current_session(ctx: &mut Context<AppRoot>) {
    ctx.state.sidebar.invalidate_sessions();
    // The popup is a client-local overlay bound to the current server; it must not linger across a
    // switch. The scratchpad, likewise client-local, closes with the current view.
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::ops::session::flush_layout_commit(ctx);
    mark_current_parked(ctx, true);
    let old_epoch = ctx.state.runtime_epoch;
    ctx.state
        .park_current(old_epoch, crate::state::Attachment::new());
    discard_parked_if_disposable(ctx, old_epoch);
}

/// Tell the server the current session is going into (or coming out of) the background, so the
/// layout-control lease follows what the client is actually doing. A parked connection is not an
/// occupant: it must not hold the lease, or the next client to attach joins as a follower of a
/// session nobody is looking at.
pub(crate) fn mark_current_parked(ctx: &mut Context<AppRoot>, parked: bool) {
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.set_parked(parked);
    }
}

/// Tear down a just-parked attachment that is not worth keeping: an ephemeral this client created
/// on the user's behalf and that they never worked in. Without this, every launch that ends in a
/// switch leaves its startup ephemeral running in the background, where it clutters the picker and
/// later asks to be confirmed away on quit — a session the user never asked for and never used.
///
/// A session the user asked for, worked in, or shares with another client is always kept.
pub(crate) fn discard_parked_if_disposable(
    ctx: &mut Context<AppRoot>,
    epoch: crate::state::AttachmentId,
) {
    let disposable = ctx.state.background.get(&epoch).is_some_and(|attachment| {
        attachment.disposition() == crate::state::SessionDisposition::Discard
    });
    if !disposable {
        return;
    }
    let Some(attachment) = ctx.state.background.remove(&epoch) else {
        return;
    };
    if let Some(name) = attachment.session_name.clone() {
        crate::events::emit(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::SessionDetached,
                vec![("session", name)],
            ),
        );
    }
    if let Some(client) = attachment.session_client.as_ref() {
        client.shutdown();
    }
}

/// Switch to a session already retained in the background: park the current one and bring the parked
/// attachment (id `parked`) to the foreground. Its client and screens are already live, so no
/// reconnect is needed - only the view is re-seeded.
pub(crate) fn switch_to_parked(
    ctx: &mut Context<AppRoot>,
    parked: crate::state::AttachmentId,
) -> Update {
    ctx.state.sidebar.invalidate_sessions();
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::ops::session::flush_layout_commit(ctx);
    mark_current_parked(ctx, true);
    let old_epoch = ctx.state.runtime_epoch;
    let Some(restored_epoch) = ctx.state.unpark(parked, old_epoch) else {
        // The switch did not take: this session is still the one on screen, so undo the parking.
        mark_current_parked(ctx, false);
        return Update::none();
    };
    discard_parked_if_disposable(ctx, old_epoch);
    ctx.state.runtime_epoch = restored_epoch;
    // Back in the foreground: reclaim the lease, which the server grants outright when the session
    // has no active controller — the usual case for a session this client left parked.
    mark_current_parked(ctx, false);
    dismiss_session_pickers(ctx);
    ctx.state.commands_dirty = true;
    // Snap to the restored session's geometry rather than interpolating from the previous view.
    ctx.state.animation = crate::anim::GeometryAnimation::None;
    if let Some((rev, layout)) = ctx.state.current_mut().pending_background_layout.take() {
        crate::shared_layout::apply_shared_layout(ctx, &layout, rev);
        ctx.state.animation = crate::anim::GeometryAnimation::None;
    }
    apply_pending_background_closes(ctx);
    // The whole screen just became the other session and the workbar badge carries its name; a
    // toast saying so would be the third copy.
    let focused = ctx.state.current().focused_pane;
    if let Some(id) = focused {
        crate::ops::focus::request_pane_focus(ctx, id);
    }
    if !ctx.state.current().session_attached {
        return reconnect_current_session(ctx);
    }
    Update::full()
}

pub(crate) fn apply_pending_background_closes(ctx: &mut Context<AppRoot>) {
    if !ctx.state.is_controller() {
        return;
    }
    let pending = std::mem::take(&mut ctx.state.current_mut().pending_background_closes);
    for (pane_id, generation) in pending {
        if ctx
            .state
            .current_mut()
            .find_pane_mut(pane_id)
            .is_some_and(|pane| pane.pty_generation == generation)
        {
            crate::pane_lifecycle::remove_pane_after_exit(ctx, pane_id, false);
        }
    }
}

/// Reconnect the current attachment without replacing its retained screens or window-manager state.
/// The new id invalidates frames from the dead transport while preserving the attachment identity.
pub(crate) fn reconnect_current_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(name) = ctx.state.current().session_name.clone() else {
        return Update::none();
    };
    let read_only = ctx.state.current().reconnect_read_only;
    let autostart = crate::state::is_ephemeral_session_name(&name);
    let epoch = ctx.state.mint_attachment_id();
    ctx.state.runtime_epoch = epoch;
    ctx.state.current_mut().epoch = epoch;
    ctx.state.current_mut().connection = crate::state::ConnectionState::Reconnecting;
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart,
        read_only,
        reconnect: true,
        remote_host: ctx.state.current().remote_host.clone(),
        intent: crate::state::AttachIntent::Plain,
        left: None,
        parked_epoch: None,
    });
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::SessionLifecycle,
        None,
        format!("Reconnecting to {name}…"),
    );
    if let Some(target) = ctx.state.current().remote_target.clone() {
        let remote_config = ctx.state.config.remote.clone();
        return Update::with_command(Command::spawn(move |link| {
            std::thread::spawn(move || {
                crate::session::bootstrap::attach_remote_session_client(
                    epoch,
                    name,
                    read_only,
                    false,
                    target,
                    remote_config,
                    true,
                    link,
                )
            });
        }));
    }
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(
                epoch, name, autostart, read_only, link,
            )
        });
    }))
}

pub(crate) fn may_shutdown_ephemeral(state: &crate::state::State) -> bool {
    may_shutdown_attachment(state.current())
}

/// Whether this client alone owns the attachment's disposable server, which is what makes closing
/// it this client's call at all. Whether it *should* be closed is
/// [`crate::state::Attachment::disposition`] — prefer that wherever the question is "what happens
/// to this session now", so the switch and exit paths keep answering it the same way.
pub(crate) fn may_shutdown_attachment(attachment: &crate::state::Attachment) -> bool {
    attachment.solely_owns_temporary_server()
}

/// Let go of every retained background attachment when leaving the client, applying the same
/// per-session rule the current session gets (see [`crate::ops::exit::shutdown_on_exit`]): close
/// what nobody could come back to, detach everything else and leave its server running.
pub(crate) fn release_background_for_exit(ctx: &mut Context<AppRoot>, close_temporary: bool) {
    for (_epoch, attachment) in std::mem::take(&mut ctx.state.background) {
        let Some(client) = attachment.session_client.as_ref() else {
            continue;
        };
        if crate::ops::exit::shutdown_on_exit(&attachment, close_temporary) {
            client.shutdown();
        } else {
            client.detach();
        }
    }
}

/// Drop the session and profile pickers that led into a session switch or attach.
pub(crate) fn dismiss_session_pickers(ctx: &mut Context<AppRoot>) {
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
}

/// Shared cleanup when a new current session is installed: close the popup and scratchpad (bound to
/// the outgoing session) and the session/profile selection overlays that led here, and mark the
/// Sessions tab stale so the post-update chokepoint re-sweeps for the new current.
pub(crate) fn prepare_session_install(ctx: &mut Context<AppRoot>) {
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    dismiss_session_pickers(ctx);
    ctx.state.sidebar.invalidate_sessions();
}

/// Shared tail after a new current attachment is in place: snap geometry, resync commands and the
/// terminal palette.
pub(crate) fn finish_session_install(ctx: &mut Context<AppRoot>) {
    // Snap to the new session's geometry rather than interpolating from the previous layout.
    ctx.state.animation = crate::anim::GeometryAnimation::None;
    ctx.state.commands_dirty = true;
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
}

/// Install `attachment` as the current session, dropping the outgoing one. Used only where the
/// outgoing session has *already* been torn down by the caller (kill / disconnect → sessionless, or
/// restart → replacement attach), so there is nothing to retain.
///
/// Only the *current attachment* changes: everything else on [`State`] is client-global (theme,
/// sidebar, background attachments, workbar scheduling, control socket, event hub) and is left
/// exactly as it was, so this no longer rebuilds — and silently loses — that state.
pub(crate) fn install_fresh_attachment(
    ctx: &mut Context<AppRoot>,
    attachment: crate::state::Attachment,
) {
    prepare_session_install(ctx);
    ctx.state.attachment = attachment;
    finish_session_install(ctx);
}

/// Park the current session and install `attachment` in its place. The outgoing session is kept
/// exactly the way a switch keeps it — **parked**, live in the background so returning to it is
/// instant — when it is attached; it is released only when there is nothing live to keep (a session
/// that was never attached: mid-connect, failed). This is what makes creating a session consistent
/// with switching to one and with creating on a remote host.
///
/// Returns `(parked_epoch, left)` for the pending attach: `parked_epoch` is the parked session's id,
/// so a failed attach restores it instead of stranding the user on a broken empty session; `left`
/// names a *released* session for the confirmation toast (`None` when parked, since parking is not a
/// detach). `new_epoch` becomes the runtime epoch.
pub(crate) fn park_current_and_install(
    ctx: &mut Context<AppRoot>,
    attachment: crate::state::Attachment,
    new_epoch: crate::state::AttachmentId,
) -> (
    Option<crate::state::AttachmentId>,
    Option<crate::state::LeftSession>,
) {
    prepare_session_install(ctx);
    crate::ops::session::flush_layout_commit(ctx);
    let outcome = if ctx.state.current().session_attached {
        mark_current_parked(ctx, true);
        let old_epoch = ctx.state.runtime_epoch;
        ctx.state.park_current(old_epoch, attachment);
        discard_parked_if_disposable(ctx, old_epoch);
        // A discarded session is gone, so there is nothing for a failed attach to fall back to.
        (
            ctx.state
                .background
                .contains_key(&old_epoch)
                .then_some(old_epoch),
            None,
        )
    } else {
        let left = ctx
            .state
            .current()
            .session_name
            .clone()
            .map(|name| crate::state::LeftSession {
                name,
                was_ephemeral_shutdown: may_shutdown_ephemeral(&ctx.state),
            });
        release_current_session(ctx);
        ctx.state.attachment = attachment;
        (None, left)
    };
    ctx.state.runtime_epoch = new_epoch;
    finish_session_install(ctx);
    outcome
}

/// Kill the current session's server (its PTYs die with it) but keep the UI alive. Does not quit
/// and does not auto-attach elsewhere: the client lands on the session picker when another choice
/// remains, otherwise the sessionless launcher.
pub(crate) fn kill_current_session(ctx: &mut Context<AppRoot>, name: String) -> Update {
    let killed_identity = ctx
        .state
        .current()
        .session_name
        .clone()
        .zip(ctx.state.current().remote_target.clone());
    let picker_was_open = ctx.state.show_session_picker;
    crate::ops::session::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.shutdown();
    }
    enter_sessionless(ctx);
    if let Some((session_name, target)) = killed_identity {
        crate::ops::session::lifecycle::remove_cached_remote_session(ctx, &session_name, &target);
    }
    crate::pty_events::notify_info(ctx, format!("Killed session `{name}`"));
    if picker_was_open {
        return refresh_picker_after_kill(ctx);
    }
    offer_session_picker_or_launcher(ctx)
}

/// Shut the current session's server down and immediately recreate it, keeping the client attached
/// to the replacement. Distinct from kill (which leaves the client sessionless).
pub(crate) fn restart_current_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(name) = ctx.state.current().session_name.clone() else {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        return Update::full();
    };
    let remote_host = ctx.state.current().remote_host.clone();
    let remote_target = ctx.state.current().remote_target.clone();
    let ephemeral = ctx.state.is_ephemeral_session();
    crate::ops::session::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.shutdown();
    }
    if let Some(target) = remote_target.as_ref() {
        crate::ops::session::lifecycle::remove_cached_remote_session(ctx, &name, target);
    }
    if ephemeral && remote_target.is_none() {
        let update = swap_to_fresh_ephemeral(ctx);
        crate::pty_events::notify_info(ctx, "Restarted temporary session");
        return update;
    }
    let restart_name = if ephemeral {
        crate::state::remote_ephemeral_session_name()
    } else {
        name.clone()
    };
    let update = attach_session_by_name(ctx, restart_name, remote_host, remote_target, true);
    crate::pty_events::notify_info(ctx, format!("Restarted session `{name}`"));
    update
}

/// Land after the active session is taken away rather than left — killed, disconnected, or
/// evicted. Never auto-attaches: open the session picker when another meaningful choice exists,
/// otherwise the sessionless launcher. The caller has already torn the outgoing session down.
pub(crate) fn land_on_surviving_session(ctx: &mut Context<AppRoot>) -> Update {
    let picker_was_open = ctx.state.show_session_picker;
    enter_sessionless(ctx);
    if picker_was_open {
        return refresh_picker_after_kill(ctx);
    }
    offer_session_picker_or_launcher(ctx)
}

/// Drop into the sessionless launcher. Raises the session picker only when another local, remote,
/// running, parked, or restorable session remains to choose from.
pub(crate) fn enter_launcher(ctx: &mut Context<AppRoot>) -> Update {
    enter_sessionless(ctx);
    offer_session_picker_or_launcher(ctx)
}

pub(crate) fn enter_sessionless(ctx: &mut Context<AppRoot>) {
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    clear_pending_session_action(ctx, None);
    // Leave the session picker alone: kill-from-picker refreshes it in place, and
    // `offer_session_picker_or_launcher` opens or closes it deliberately.
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.attachment = crate::state::Attachment::new();
    ctx.state.runtime_epoch = ctx.state.mint_attachment_id();
    ctx.state.current_mut().epoch = ctx.state.runtime_epoch;
    finish_session_install(ctx);
}

pub(crate) fn has_meaningful_session_choices(ctx: &mut Context<AppRoot>) -> bool {
    !immediate_picker_rows(ctx).is_empty() || crate::session::bootstrap::has_session_candidates()
}

pub(crate) fn offer_session_picker_or_launcher(ctx: &mut Context<AppRoot>) -> Update {
    if has_meaningful_session_choices(ctx) {
        crate::ops::session::lifecycle::open_session_picker(ctx)
    } else {
        ctx.state.show_session_picker = false;
        ctx.state.session_picker = None;
        ctx.state.commands_dirty = true;
        Update::full()
    }
}

/// After a kill from the open picker: rebuild the list, keep the nearest selection, and close into
/// the launcher when nothing remains to pick.
pub(crate) fn refresh_picker_after_kill(ctx: &mut Context<AppRoot>) -> Update {
    let update = crate::ops::session::lifecycle::refresh_session_picker(ctx);
    let empty = ctx
        .state
        .session_picker
        .as_ref()
        .is_none_or(|picker| picker.entries.is_empty());
    if empty && !crate::session::bootstrap::has_session_candidates() {
        return crate::ops::session::lifecycle::close_session_picker(ctx);
    }
    update
}

/// Install a brand-new ephemeral session as current and spawn its attach, after the outgoing
/// session has already been shut down or detached by the caller.
pub(crate) fn swap_to_fresh_ephemeral(ctx: &mut Context<AppRoot>) -> Update {
    let epoch = ctx.state.mint_attachment_id();
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    // A fresh ephemeral is a session with no recipe named for it, so it seeds from
    // `[profile] default` exactly as the launch that started rozi did.
    let (attachment, intent) = crate::profiles::default_session_seed(&ctx.state.config);
    install_fresh_attachment(ctx, attachment);
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host: None,
        intent,
        left: None,
        parked_epoch: None,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

pub(crate) fn attach_session_by_name(
    ctx: &mut Context<AppRoot>,
    name: String,
    remote_host: Option<String>,
    discovered_target: Option<crate::session::remote::RemoteTarget>,
    autostart: bool,
) -> Update {
    if !crate::session::discovery::valid_attach_target(&name) {
        crate::pty_events::notify_error(
            ctx,
            "Invalid session name",
            "Use letters, numbers, _ or -",
        );
        return Update::full();
    }
    let remote_target = match (discovered_target, remote_host.as_deref()) {
        (Some(target), _) => Some(target),
        (None, Some(host)) => match crate::session::remote::parse_remote_target(host) {
            Ok(target) => Some(target),
            Err(err) => {
                crate::pty_events::notify_error(
                    ctx,
                    "Invalid remote host",
                    format!("`{host}`: {err}"),
                );
                return Update::full();
            }
        },
        (None, None) => None,
    };
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(name.as_str())
        && ctx.state.current().remote_target == remote_target
    {
        crate::pty_events::notify_info(ctx, format!("Already attached to `{name}`"));
        return Update::full();
    }
    // An attach already running for *this same* target is the double-click case: say so and let it
    // finish. Aiming somewhere else is the user changing their mind, and must go through — refusing
    // it would make a pending attach a trap, with no way off a session that never finishes
    // connecting. The mid-connect attachment is released rather than parked by the install below
    // (it has no live client to keep), and the abandoned attach thread's reply is discarded by the
    // epoch check in `attach_failed`/`connected`.
    if let Some(pending) = ctx.state.current().pending_session_attach.as_ref()
        && pending.name == name
        && ctx.state.current().remote_target == remote_target
    {
        crate::pty_events::notify_info(ctx, "Attach already in progress");
        return Update::full();
    }
    // Fast path: the target session is already retained in the background - switch to it instantly
    // (its client and screens are live) instead of reconnecting.
    if let Some(parked) = ctx
        .state
        .parked_attachment_id(&name, remote_target.as_ref())
    {
        return switch_to_parked(ctx, parked);
    }
    // Attach-elsewhere. Retain the current attached session in the background so switching back is
    // instant and its screens stay live; only tear it down when it is not actually attached (e.g.
    // still mid-connect). The epoch advances below, so the retained session's remaining frames route
    // to it as a background attachment rather than the new current one.
    let epoch = ctx.state.mint_attachment_id();
    let mut parked_epoch = None;
    let left =
        if ctx.state.current().session_attached {
            // Retain the previous session under its current epoch so a failed attach can restore it.
            parked_epoch = Some(ctx.state.runtime_epoch);
            park_current_session(ctx);
            None
        } else {
            let left = ctx.state.current().session_name.clone().map(|left_name| {
                crate::state::LeftSession {
                    name: left_name,
                    was_ephemeral_shutdown: may_shutdown_ephemeral(&ctx.state),
                }
            });
            release_current_session(ctx);
            left
        };
    ctx.state.runtime_epoch = epoch;
    dismiss_session_pickers(ctx);
    ctx.state.commands_dirty = true;
    ctx.state.current_mut().remote_host = remote_host.clone();
    ctx.state.current_mut().remote_target = remote_target.clone();
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart,
        read_only: false,
        reconnect: false,
        remote_host: remote_host.clone(),
        intent: crate::state::AttachIntent::Plain,
        left,
        parked_epoch,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    let remote_config = ctx.state.config.remote.clone();
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            if let Some(target) = remote_target {
                crate::session::bootstrap::attach_remote_session_client(
                    epoch,
                    name,
                    false,
                    false,
                    target,
                    remote_config,
                    // Explicit attach: fail fast rather than blocking the UI on a dead host.
                    false,
                    link,
                );
            } else {
                crate::session::bootstrap::attach_session_client(
                    epoch, name, autostart, false, link,
                );
            }
        });
    }))
}

/// The launcher's one offer: start this client's ephemeral session now. Reached by `Enter` on the
/// launcher panel and by the session picker's scratch-session key, which is why it also drops any
/// deferred PTY action — asking for a plain shell replaces whatever spawn was queued against a
/// session that never arrived.
pub(crate) fn start_launcher_shell(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_action(ctx, None);
    attach_startup_ephemeral(ctx)
}

/// This client's own scratch session, when it already has one: the ephemeral it holds in the
/// foreground or parked in the background.
///
/// The name is read back off the attachment rather than recomputed from the pid, because a
/// restarted ephemeral is salted (`eph-<pid>-<salt>`) and would not be found by name. Other
/// clients' ephemerals are deliberately not counted — they are somebody else's scratch session, and
/// the picker already lists them as rows.
pub(crate) fn held_ephemeral_session(
    state: &crate::state::State,
) -> Option<&crate::state::Attachment> {
    std::iter::once(state.current())
        .chain(state.background.values())
        .find(|attachment| {
            attachment
                .session_name
                .as_deref()
                .is_some_and(crate::state::is_ephemeral_session_name)
        })
}

/// Attach this process's ephemeral session, seeded with the panes the launch had prepared (its
/// initial shell, or a restored profile/autosave layout). Used when the user explicitly starts a
/// shell from the launcher, so the layout the launch intended is still what they get.
///
/// A launcher reached by killing a session has no seed; it falls back to a single default pane.
/// When a [`PendingSessionAction`] is waiting, the seed is empty so the deferred action creates the
/// only pane after attach — avoiding a blank local pane and a leftover shell.
pub(crate) fn attach_startup_ephemeral(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    // Set when this session had to be seeded here rather than from the parked launcher seed, which
    // already carries the launch's own `deferred_profile_seed`.
    let mut seeded_intent = None;
    if ctx.state.needs_session_for_pty() {
        let mut seed_from_default = |ctx: &mut Context<AppRoot>| {
            let (attachment, intent) = crate::profiles::default_session_seed(&ctx.state.config);
            seeded_intent = Some(intent);
            attachment
        };
        let seed = if ctx.state.pending_session_action.is_some() {
            let mut empty = crate::state::Attachment::new();
            empty.auto_created = true;
            empty
        } else if ctx.state.is_launcher() {
            // A launcher reached by killing a session has no parked seed, so it opens the same way
            // a fresh ephemeral does: from `[profile] default` when one is configured.
            match ctx.state.launcher_seed.take() {
                Some(seed) => seed,
                None => seed_from_default(ctx),
            }
        } else {
            // Stuck no-client panes (e.g. a pre-fix blank spawn): replace with a working shell.
            seed_from_default(ctx)
        };
        let epoch = ctx.state.runtime_epoch;
        ctx.state.attachment = seed;
        ctx.state.current_mut().epoch = epoch;
        ctx.state.current_mut().auto_created = true;
        finish_session_install(ctx);
    }
    // This is a *local* fallback; clear any remote target left over from a failed `--remote` attach
    // so panes resolve their shell/cwd locally and the sidebar does not keep probing a dead host.
    ctx.state.current_mut().remote_host = None;
    ctx.state.current_mut().remote_target = None;
    let epoch = ctx.state.runtime_epoch;
    let name = crate::state::ephemeral_session_name();
    let intent = match ctx.state.current_mut().deferred_profile_seed.take() {
        Some((profile, path)) => crate::state::AttachIntent::ProfileSeed { profile, path },
        None => seeded_intent.unwrap_or(crate::state::AttachIntent::Plain),
    };
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host: None,
        intent,
        left: None,
        parked_epoch: None,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

/// True when a PTY spawn would hang with no client and no attach in flight.
pub(crate) fn needs_session_for_pty(state: &crate::state::State) -> bool {
    state.needs_session_for_pty()
}

/// If no session can run a PTY yet, stash `action` and start an ephemeral attach. Returns
/// `Some(update)` when the caller must stop — the action runs from [`run_pending_session_action`]
/// after `SessionAttached`. Returns `None` when the caller should proceed immediately.
pub(crate) fn ensure_session_for_pty(
    ctx: &mut Context<AppRoot>,
    action: crate::state::PendingSessionAction,
) -> Option<Update> {
    if !needs_session_for_pty(&ctx.state) {
        return None;
    }
    ctx.state.pending_session_action = Some(action);
    Some(attach_startup_ephemeral(ctx))
}

/// Drop a deferred PTY action (and any held control reply) without running it — attach failed, or
/// the user started a plain shell instead.
pub(crate) fn clear_pending_session_action(ctx: &mut Context<AppRoot>, error: Option<&str>) {
    ctx.state.pending_session_action = None;
    if let Some(reply) = ctx.state.pending_control_reply.take() {
        let _ = reply.send(match error {
            Some(message) => crate::control::ControlResponse::error(message),
            None => crate::control::ControlResponse::error("session attach cancelled"),
        });
    }
}

/// Replay a deferred PTY action now that a session client is installed.
pub(crate) fn run_pending_session_action(ctx: &mut Context<AppRoot>) -> Update {
    let Some(action) = ctx.state.pending_session_action.take() else {
        return Update::none();
    };
    match action {
        crate::state::PendingSessionAction::OpenConfigFile => {
            crate::ops::config::open_config_file(ctx)
        }
        crate::state::PendingSessionAction::ToggleScratchpad => crate::scratchpad::toggle(ctx),
        crate::state::PendingSessionAction::UserCommand { action, env } => {
            crate::actions::execute_user_command_action_with_env(ctx, &action, env)
        }
        crate::state::PendingSessionAction::NewPane {
            source,
            command,
            cwd,
            title,
            keep_open,
            focus,
        } => {
            let (id, update) = crate::ops::control::new_pane_after_session(
                ctx, source, command, cwd, title, keep_open, focus,
            );
            if let Some(reply) = ctx.state.pending_control_reply.take() {
                crate::ops::control::hold_spawn_reply(ctx, id, reply);
            }
            update
        }
        crate::state::PendingSessionAction::Popup {
            command,
            cwd,
            width,
            height,
            title,
            keep_open,
        } => {
            let result = crate::popup::open(
                ctx,
                command,
                cwd,
                width,
                height,
                title,
                keep_open,
                Vec::new(),
            );
            match result {
                Ok(update) => {
                    if let Some(reply) = ctx.state.pending_control_reply.take() {
                        let _ = reply.send(crate::control::ControlResponse::empty());
                    }
                    update
                }
                Err(error) => {
                    if let Some(reply) = ctx.state.pending_control_reply.take() {
                        let _ = reply.send(crate::control::ControlResponse::error(error.clone()));
                    }
                    crate::pty_events::notify_error(ctx, "Popup failed", error);
                    Update::full()
                }
            }
        }
    }
}

/// Disconnect from a remote host: close every attachment to it — current and retained — leaving the
/// remote servers running for reattach. If the current session lives on that host, the UI lands on
/// the session picker when other choices remain, otherwise the sessionless launcher.
/// Non-destructive.
///
/// The returned [`Update`] carries any picker-watch command that follows. Callers must return it;
/// dropping it strands the client without a way to rediscover sessions.
pub(crate) fn disconnect_host(
    ctx: &mut Context<AppRoot>,
    target: &crate::session::remote::RemoteTarget,
) -> Update {
    let host_label = target.display_label();
    // Back to `Idle`, which is what stops the sweep probing it — done here rather than at each call
    // site so disconnecting from the picker stops the ssh traffic the same way the sidebar does.
    if let Some(entry) = ctx.state.hosts.get_mut(target) {
        entry.probe = crate::state::HostProbe::Idle;
    }
    // Close every retained background attachment on this host; their servers keep running.
    let ids: Vec<crate::state::AttachmentId> = ctx
        .state
        .background
        .iter()
        .filter(|(_, attachment)| attachment.remote_target.as_ref() == Some(target))
        .map(|(id, _)| *id)
        .collect();
    let mut closed = 0usize;
    for id in ids {
        if let Some(attachment) = ctx.state.background.remove(&id) {
            if let Some(client) = attachment.session_client.as_ref() {
                client.detach();
            }
            closed += 1;
        }
    }
    let current_on_host = ctx.state.current().session_attached
        && ctx.state.current().remote_target.as_ref() == Some(target);
    if current_on_host {
        if let Some(client) = ctx.state.current().session_client.clone() {
            crate::ops::exit::mark_session_detached(ctx, None);
            client.detach();
        }
        closed += 1;
        // The session on screen is being taken away rather than left, so land somewhere the user
        // recognizes. Attachments on this host are already gone from the background above, so the
        // candidates are exactly the sessions that survive the disconnect.
        let update = land_on_surviving_session(ctx);
        crate::pty_events::notify_info(
            ctx,
            format!("Disconnected from `{host_label}` — {closed} closed, servers still running"),
        );
        return update;
    }
    if closed == 0 {
        crate::pty_events::notify_info(ctx, format!("Not connected to `{host_label}`"));
        return Update::full();
    }
    crate::pty_events::notify_info(
        ctx,
        format!("Disconnected from `{host_label}` — {closed} closed, servers still running"),
    );
    Update::full()
}
