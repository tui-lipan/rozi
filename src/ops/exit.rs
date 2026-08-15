use std::time::Instant;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::close_pane;
use crate::profiles;
use crate::pty_events::confirm_toast;
use crate::state::{PendingDestructive, PendingDestructiveConfirmation, SessionDisposition, State};

pub(crate) fn clear_pending(ctx: &mut Context<AppRoot>) {
    if let Some(pending) = ctx.state.pending_destructive.take() {
        ctx.toast().dismiss(pending.toast_id);
    }
}

/// True when `pending` was armed by an earlier press and is still within the confirm window
/// (consuming it); otherwise (re-)arms it and returns false.
///
/// This one is checked lazily against its arm time rather than cleared by a scheduled expiry the
/// way the sidebar and picker confirmations are: nothing renders as armed for it, so a stale field
/// is invisible. The confirm toast runs for the same window, which is what tells the user the
/// confirmation is over.
fn confirm_second_press(
    ctx: &mut Context<AppRoot>,
    pending: PendingDestructive,
    toast: Toast,
) -> bool {
    if let Some(armed) = ctx.state.pending_destructive.take() {
        ctx.toast().dismiss(armed.toast_id);
        if armed.action == pending
            && armed.armed_at.elapsed() <= crate::ops::confirm::CONFIRM_WINDOW
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

/// Leave the client. There is one way out, because in a client that visits many sessions "how you
/// left" says nothing useful — what matters is what happens to each session, and that is decided
/// per session:
///
/// - A **named** session is detached and its server left running, always.
/// - An **untouched** temporary session the client created for the user is closed silently; it
///   holds nothing (see [`crate::state::Attachment::disposition`]).
/// - A **used but unnamed** temporary session is the only one worth asking about, because leaving
///   is the last chance to keep it. It raises the leave prompt: name it to keep it running, or
///   submit nothing to close it.
///
/// Both `quit` and `detach` land here. Cancelling the prompt (`Esc`) returns to the session with
/// nothing torn down.
pub(crate) fn leave_client(ctx: &mut Context<AppRoot>) -> Update {
    crate::popup::kill_if_open(ctx);
    crate::ops::session::flush_layout_commit(ctx);
    clear_pending(ctx);
    let temporary = keepable_temporary_count(&ctx.state);
    if temporary > 0 {
        // Ask about the session on screen. If the one to ask about is parked, come back to it
        // first: a prompt about a session the user cannot see is a prompt they cannot judge.
        if !keepable_temporary(ctx.state.current())
            && let Some((&id, _)) = ctx
                .state
                .background
                .iter()
                .find(|(_, attachment)| keepable_temporary(attachment))
        {
            let _ = crate::ops::session::switch_to_parked(ctx, id);
        }
        return crate::ops::session::open_leave_prompt(ctx, temporary);
    }
    leave_client_now(ctx, false)
}

/// Leave for real. `close_temporary` is the user's answer to the leave prompt: with it set, the
/// temporary sessions that prompt was about are closed; without it, everything that can keep
/// running does. Every path out of the client ends here.
pub(crate) fn leave_client_now(ctx: &mut Context<AppRoot>, close_temporary: bool) -> Update {
    clear_pending(ctx);
    crate::ops::pick::cancel_pick(ctx, Some("detached"));
    crate::popup::kill_if_open(ctx);
    let shutdown_current = shutdown_on_exit(ctx.state.current(), close_temporary);
    mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        if shutdown_current {
            client.shutdown();
        } else {
            client.detach();
        }
    }
    crate::ops::session::release_background_for_exit(ctx, close_temporary);
    crate::ops::services::terminate_all(&mut ctx.state);
    // A session whose server goes down with us leaves no other copy of its layout, so mirror it to
    // disk regardless of `[session] autosave`; anything still running is its own record.
    if shutdown_current {
        profiles::persist_session_on_detach(&ctx.state);
    } else {
        profiles::persist_session_if_enabled(&ctx.state);
    }
    ctx.quit();
    Update::none()
}

/// Whether leaving closes this session's server rather than detaching from it: what the session's
/// own [disposition](crate::state::Attachment::disposition) says, plus the user's answer for the
/// sessions the leave prompt asked about.
pub(crate) fn shutdown_on_exit(
    attachment: &crate::state::Attachment,
    close_temporary: bool,
) -> bool {
    match attachment.disposition() {
        SessionDisposition::Keep => false,
        SessionDisposition::Discard => true,
        SessionDisposition::AskBeforeClosing => close_temporary,
    }
}

/// Whether the leave prompt should ask about this session. Exactly the sessions
/// [`SessionDisposition::AskBeforeClosing`] names, so the count in the prompt and the set the
/// answer closes can never disagree.
fn keepable_temporary(attachment: &crate::state::Attachment) -> bool {
    attachment.disposition() == SessionDisposition::AskBeforeClosing
}

fn keepable_temporary_count(state: &State) -> usize {
    std::iter::once(state.current())
        .chain(state.background.values())
        .filter(|attachment| keepable_temporary(attachment))
        .count()
}

/// Mark the current session as intentionally left and emit its hook event exactly once.
/// Callers must flush pending layout changes before entering this transition.
pub(crate) fn mark_session_detached(ctx: &mut Context<AppRoot>, session: Option<&str>) {
    if !ctx.state.current().session_attached {
        return;
    }
    let session = session
        .map(str::to_string)
        .or_else(|| ctx.state.current().session_name.clone())
        .unwrap_or_default();
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::SessionDetached,
            vec![("session", session)],
        ),
    );
    ctx.state.current_mut().session_attached = false;
    ctx.state.sidebar.invalidate_sessions();
}

/// The controlling terminal/console went away (Unix `SIGHUP`/`SIGTERM`, Windows console close,
/// logoff, or shutdown): detach cleanly instead of letting the process be killed where it stands
/// (cross-platform plan Phase 5b).
///
/// Unlike [`leave_client`], this never prompts. There is nobody left to answer a prompt, and the
/// window before the OS force-kills us is short (Windows gives a few seconds; a Unix `SIGHUP` is
/// usually followed by the emulator exiting). So a temporary session skips the "name it first" flow
/// and simply detaches: its server shuts itself down after the no-client grace period, which is the
/// right outcome for a session nobody can reattach to by name anyway.
pub(crate) fn detach_on_hangup(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::flush_layout_commit(ctx);
    crate::ops::pick::cancel_pick(ctx, Some("detached"));
    mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.detach();
    }
    crate::ops::session::release_background_for_exit(ctx, false);
    crate::ops::services::terminate_all(&mut ctx.state);
    profiles::persist_session_on_detach(&ctx.state);
    ctx.quit();
    Update::none()
}

/// Whether any tiled/floating pane still has a running process. Used to decide whether quitting
/// an ephemeral session (which shuts the server down and kills its PTYs) warrants a confirmation.
pub(crate) fn any_pane_live(state: &State) -> bool {
    state.current().any_pane_live()
}

/// Leave the client without ever raising the leave prompt, preserving everything that can be
/// preserved. Used where there is nobody to answer a question: the control socket, which is
/// scripted, and any other non-interactive caller. A scripted exit must never destroy a session.
pub(crate) fn leave_client_unattended(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::flush_layout_commit(ctx);
    leave_client_now(ctx, false)
}

pub(crate) fn close_focused_pane_with_confirmation(
    ctx: &mut Context<AppRoot>,
    confirmations_enabled: bool,
) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
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
            confirm_toast(
                &ctx.state.theme,
                ctx.state.config.pane.toast_opacity,
                "Again to kill pane",
            ),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    close_pane(ctx, id)
}

pub(crate) fn kill_workspace_with_confirmation(
    ctx: &mut Context<AppRoot>,
    confirmations_enabled: bool,
) -> Update {
    let workspace_index = ctx.state.current().active_workspace;
    let workspace = &ctx.state.current().workspaces[workspace_index];
    let pane_count = workspace.panes.iter().filter(|pane| !pane.closing).count();
    if pane_count == 0 {
        crate::pty_events::notify_info(ctx, "Workspace is already empty");
        return Update::full();
    }

    if confirmations_enabled && ctx.state.config.confirm.kill_workspace {
        let label = workspace_index + 1;
        if !confirm_second_press(
            ctx,
            PendingDestructive::KillWorkspace(workspace_index),
            confirm_toast(
                &ctx.state.theme,
                ctx.state.config.pane.toast_opacity,
                format!(
                    "Again to kill {pane_count} {} on workspace {label}",
                    if pane_count == 1 { "pane" } else { "panes" }
                ),
            ),
        ) {
            return Update::full();
        }
    }

    clear_pending(ctx);
    let pane_ids: Vec<_> = ctx.state.current().workspaces[workspace_index]
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .map(|pane| pane.id)
        .collect();

    // One Update carries one Command, so the whole batch shares a single delayed prune.
    let targets: Vec<(crate::state::PaneId, u64)> = pane_ids
        .into_iter()
        .filter_map(|id| {
            crate::pane_lifecycle::close_pane_inner_without_focus(ctx, id, true)
                .map(|generation| (id, generation))
        })
        .collect();

    if targets.is_empty() {
        return Update::full();
    }

    // The whole active workspace is now closing. Resolve its focus once, after every target has
    // left the tiled set, so a Scrollable teardown cannot focus a neighbour that this same batch
    // will close on its next iteration. No inactive workspace is consulted or mutated here.
    crate::ops::focus::choose_fallback_focus(&mut ctx.state);
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
    if workspace.layout_kind == crate::state::LayoutKind::Scrollable
        && workspace.tiled_ids().is_empty()
    {
        workspace.set_scrollable_viewport(None, crate::state::ScrollableRevealEdge::Left);
    }
    crate::ops::focus::request_current_pane_focus(ctx);

    Update::with_command(crate::pane_lifecycle::prune_closed_batch_command(
        ctx.state.runtime_epoch,
        targets,
        crate::anim::retained_pane_timeout(ctx.state.config.animations),
    ))
}

pub(crate) fn kill_session_with_confirmation(
    ctx: &mut Context<AppRoot>,
    confirmations_enabled: bool,
) -> Update {
    if !ctx.state.current().session_attached {
        crate::pty_events::notify_info(ctx, "Not attached to a named session");
        return Update::full();
    }
    if !ctx.state.is_controller() {
        crate::ops::session::nudge_if_follower(ctx);
        return Update::full();
    }

    let session_name = ctx
        .state
        .current()
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
                ctx.state.config.pane.toast_opacity,
                format!("Again to kill session `{session_name}`"),
            ),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    crate::ops::session::kill_current_session(ctx, session_name)
}

pub(crate) fn restart_session_with_confirmation(
    ctx: &mut Context<AppRoot>,
    confirmations_enabled: bool,
) -> Update {
    if !ctx.state.current().session_attached {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        return Update::full();
    }
    if !ctx.state.is_controller() {
        crate::ops::session::nudge_if_follower(ctx);
        return Update::full();
    }

    let session_name = ctx
        .state
        .current()
        .session_name
        .clone()
        .unwrap_or_else(|| "session".to_string());

    if confirmations_enabled
        && ctx.state.config.confirm.kill_session
        && !confirm_second_press(
            ctx,
            PendingDestructive::RestartSession,
            confirm_toast(
                &ctx.state.theme,
                ctx.state.config.pane.toast_opacity,
                format!("Again to restart session `{session_name}`"),
            ),
        )
    {
        return Update::full();
    }

    clear_pending(ctx);
    crate::ops::session::restart_current_session(ctx)
}

pub(crate) fn confirm_new_temporary_session(ctx: &mut Context<AppRoot>) -> bool {
    let pending = PendingDestructive::NewTemporarySession;
    let toast = confirm_toast(
        &ctx.state.theme,
        ctx.state.config.pane.toast_opacity,
        "Again to start a fresh temporary session\nCurrent session is discarded",
    );
    confirm_second_press(ctx, pending, toast)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Msg;
    use crate::events::EventKind;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientInfo, ClientMessage};
    use crate::state::SharedSessionState;
    use std::collections::HashSet;
    use std::time::Duration;
    use tui_lipan::TestBackend;

    fn confirming_backend() -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_name = Some("eph-confirm".to_string());
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        // A session the user has worked in: the one shape that is worth confirming away, and the
        // one that survives a switch instead of being discarded as an untouched startup ephemeral.
        state.current_mut().engaged = true;
        state.config.confirm.new_temporary_session = true;
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.clients = vec![ClientInfo {
            id: 1,
            label: "me".to_string(),
            read_only: false,
            requesting_control: false,
            parked: false,
        }];
        state.current_mut().shared = Some(shared);
        backend
    }

    fn press_new_temporary(backend: &mut TestBackend<AppRoot>) {
        backend
            .dispatch(Msg::RunAction(Action::NewTemporarySession))
            .expect("dispatch new temporary session");
    }

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    fn named_attached_backend() -> (
        TestBackend<AppRoot>,
        std::sync::mpsc::Receiver<ClientOutbound>,
        std::sync::mpsc::Receiver<String>,
    ) {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        let events = {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("named".to_string());
            state.current_mut().session_attached = true;
            state.current_mut().session_client = Some(client);
            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(1);
            shared.clients = vec![ClientInfo {
                id: 1,
                label: "me".to_string(),
                read_only: false,
                requesting_control: false,
                parked: false,
            }];
            shared.last_committed_layout = None;
            state.current_mut().shared = Some(shared);
            state.event_hub.subscribe(None)
        };
        (backend, outbound, events)
    }

    #[test]
    fn detach_flushes_layout_before_marking_session_detached() {
        on_large_stack(|| {
            let (mut backend, outbound, events) = named_attached_backend();
            backend.render();

            backend
                .dispatch(Msg::RunAction(Action::Detach))
                .expect("dispatch detach");

            let sent: Vec<_> = outbound.try_iter().collect();
            let commit = sent.iter().position(|message| {
                matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::CommitLayout { .. })
                )
            });
            let detach = sent.iter().position(|message| {
                matches!(message, ClientOutbound::Control(ClientMessage::Detach))
            });
            assert!(
                commit.is_some_and(|commit| detach.is_some_and(|detach| commit < detach)),
                "expected layout commit before detach, got {sent:?}"
            );
            assert!(!backend.state().current().session_attached);
            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("session-detached event")).unwrap();
            assert_eq!(
                event,
                serde_json::json!({"event":"session-detached","data":{"session":"named"}})
            );
        });
    }

    #[test]
    fn named_session_quit_emits_session_detached() {
        on_large_stack(|| {
            let (mut backend, _outbound, events) = named_attached_backend();
            backend.render();

            backend
                .dispatch(Msg::RunAction(Action::Quit))
                .expect("dispatch quit");

            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("session-detached event")).unwrap();
            assert_eq!(event["event"], "session-detached");
            assert_eq!(event["data"]["session"], "named");
            assert!(!backend.state().current().session_attached);
        });
    }

    #[test]
    fn detach_prompts_to_name_a_retained_ephemeral_before_exiting() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            {
                let state = backend.state_mut();
                state.runtime_epoch = 3;
                state.park_current(3, crate::state::Attachment::new());
                state.runtime_epoch = 4;
                let (client, _outbound) = SessionClient::test_channel();
                state.current_mut().session_name = Some("named".to_string());
                state.current_mut().session_client = Some(client);
                state.current_mut().session_attached = true;
                let mut shared = SharedSessionState::new(2);
                shared.controller = Some(2);
                shared.clients = vec![ClientInfo {
                    id: 2,
                    label: "me".to_string(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                }];
                state.current_mut().shared = Some(shared);
            }

            backend
                .dispatch(Msg::RunAction(Action::Detach))
                .expect("dispatch detach with retained ephemeral");

            assert!(backend.state().is_ephemeral_session());
            assert!(
                backend
                    .state()
                    .rename_session
                    .as_ref()
                    .is_some_and(|rename| rename.leave.is_some())
            );
            assert!(
                backend
                    .state()
                    .background
                    .values()
                    .any(|attachment| attachment.session_name.as_deref() == Some("named"))
            );
        });
    }

    #[test]
    fn hangup_emits_session_detached() {
        on_large_stack(|| {
            let (mut backend, _outbound, events) = named_attached_backend();
            backend.render();

            backend.dispatch(Msg::Hangup).expect("dispatch hangup");

            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("session-detached event")).unwrap();
            assert_eq!(event["event"], "session-detached");
            assert_eq!(event["data"]["session"], "named");
            assert!(!backend.state().current().session_attached);
        });
    }

    /// The leave prompt asks about every temporary session leaving would close, including the ones
    /// parked in the background — those are just as unreachable once this client is gone.
    #[test]
    fn the_leave_prompt_counts_retained_live_temporary_sessions() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            {
                let state = backend.state_mut();
                state.runtime_epoch = 3;
                state.park_current(3, crate::state::Attachment::new());
                state.runtime_epoch = 4;
                state.current_mut().session_name = Some("named".to_string());
            }

            assert_eq!(keepable_temporary_count(backend.state()), 1);
        });
    }

    /// Leaving with a named session takes nothing away: it detaches and goes, no prompt, because
    /// there is nothing about it that leaving could destroy.
    #[test]
    fn leaving_a_named_session_asks_nothing_and_detaches() {
        on_large_stack(|| {
            let (mut backend, outbound, _events) = named_attached_backend();
            backend.render();

            backend
                .dispatch(Msg::RunAction(Action::Quit))
                .expect("dispatch quit");

            assert!(backend.state().rename_session.is_none());
            let sent: Vec<_> = outbound.try_iter().collect();
            assert!(
                sent.iter().any(|message| matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::Detach)
                )),
                "a named session must be left running, got {sent:?}"
            );
            assert!(
                !sent.iter().any(|message| matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::Shutdown)
                )),
                "leaving must never shut down a named session, got {sent:?}"
            );
        });
    }

    /// A temporary session the user worked in is the one case leaving could destroy something, so
    /// it asks — and an empty answer takes a second press, with the prompt saying what it closes.
    #[test]
    fn leaving_a_used_temporary_session_asks_then_closes_on_the_second_empty_submit() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            backend.render();

            backend
                .dispatch(Msg::RunAction(Action::Quit))
                .expect("dispatch quit");
            let armed = backend
                .state()
                .rename_session
                .as_ref()
                .and_then(|rename| rename.leave)
                .expect("leaving raises the leave prompt");
            assert!(!armed.armed, "the prompt opens unarmed");
            assert_eq!(armed.temporary, 1);

            // First empty submit only arms; nothing is torn down yet.
            backend
                .dispatch(Msg::SubmitRenameSession)
                .expect("first empty submit");
            assert!(
                backend
                    .state()
                    .rename_session
                    .as_ref()
                    .and_then(|rename| rename.leave)
                    .is_some_and(|leave| leave.armed),
                "an empty submit arms the close instead of doing it"
            );

            backend
                .dispatch(Msg::SubmitRenameSession)
                .expect("second empty submit");
            assert!(backend.state().rename_session.is_none());
        });
    }

    /// Typing after arming is a change of mind. The armed close must not survive it, or clearing
    /// the field again would close on a press that never warned.
    #[test]
    fn editing_the_name_disarms_a_pending_close() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            backend.render();
            backend
                .dispatch(Msg::RunAction(Action::Quit))
                .expect("dispatch quit");
            backend
                .dispatch(Msg::SubmitRenameSession)
                .expect("arm the close");

            backend
                .dispatch(Msg::RenameSessionChanged(InputEvent {
                    value: "d".into(),
                    cursor: 1,
                    anchor: None,
                }))
                .expect("type a name");

            assert!(
                backend
                    .state()
                    .rename_session
                    .as_ref()
                    .and_then(|rename| rename.leave)
                    .is_some_and(|leave| !leave.armed),
                "editing must disarm the pending close"
            );
        });
    }

    #[test]
    fn matching_second_press_within_window_consumes_confirmation() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            press_new_temporary(&mut backend);
            assert_eq!(
                backend
                    .state()
                    .pending_destructive
                    .as_ref()
                    .map(|pending| pending.action),
                Some(PendingDestructive::NewTemporarySession)
            );
            press_new_temporary(&mut backend);
            assert!(backend.state().pending_destructive.is_none());
            assert_ne!(
                backend.state().current().session_name.as_deref(),
                Some("eph-confirm")
            );
        });
    }

    #[test]
    fn expired_confirmation_is_rearmed() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            press_new_temporary(&mut backend);
            backend
                .state_mut()
                .pending_destructive
                .as_mut()
                .expect("armed confirmation")
                .armed_at = Instant::now() - Duration::from_secs(10);
            press_new_temporary(&mut backend);
            let pending = backend
                .state()
                .pending_destructive
                .as_ref()
                .expect("expired press rearmed");
            assert_eq!(pending.action, PendingDestructive::NewTemporarySession);
            assert!(pending.armed_at.elapsed() < Duration::from_secs(1));
            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("eph-confirm")
            );
        });
    }

    #[test]
    fn killing_a_scrollable_workspace_resolves_focus_once_after_batch_teardown() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            {
                let state = backend.state_mut();
                state.config.confirm.kill_workspace = false;
                let rect = FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 80.0,
                    h: 24.0,
                };
                let workspace = &mut state.current_mut().workspaces[0];
                workspace.layout_kind = crate::state::LayoutKind::Scrollable;
                workspace.panes.clear();
                // Storage order differs from the strip order to make an accidental per-pane
                // neighbor walk observable.
                for id in [20, 10, 30] {
                    let mut pane = crate::state::Pane::new(id, 100, rect);
                    pane.opening = false;
                    pane.terminal_active = true;
                    workspace.panes.push(pane);
                }
                workspace.tile_tree = crate::tiling::build_dwindle_tree(
                    &[10, 30, 20],
                    crate::state::SplitAxis::Horizontal,
                    &[0.5, 0.5],
                );
                workspace.focused_pane = Some(30);
                workspace.scrollable_anchor = Some(30);
                state.current_mut().focused_pane = Some(30);

                // Killing the active workspace must not touch an inactive workspace.
                let mut inactive = crate::state::Pane::new(90, 100, rect);
                inactive.opening = false;
                inactive.terminal_active = true;
                state.current_mut().workspaces[1].panes.push(inactive);
                crate::tiling::append_tiled_window(&mut state.current_mut().workspaces[1], 90);
            }
            backend.render();
            let focus_events = backend
                .state()
                .event_hub
                .subscribe(Some(HashSet::from([EventKind::FocusChanged])));

            backend
                .dispatch(Msg::RunAction(Action::KillWorkspace))
                .expect("kill Scrollable workspace");

            let workspace = &backend.state().current().workspaces[0];
            assert_eq!(backend.state().current().focused_pane, None);
            assert_eq!(workspace.focused_pane, None);
            assert!(workspace.panes.iter().all(|pane| pane.closing));
            assert!(workspace.tiled_ids().is_empty());
            assert_eq!(workspace.scrollable_anchor, None);
            assert_eq!(
                focus_events.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty),
                "batch teardown must not emit transient per-neighbor focus events"
            );

            let inactive = &backend.state().current().workspaces[1];
            assert!(
                inactive
                    .panes
                    .iter()
                    .any(|pane| pane.id == 90 && !pane.closing)
            );
            assert_eq!(inactive.tiled_ids(), [90]);
        });
    }

    #[test]
    fn mismatched_confirmation_is_replaced() {
        on_large_stack(|| {
            let mut backend = confirming_backend();
            press_new_temporary(&mut backend);
            backend
                .state_mut()
                .pending_destructive
                .as_mut()
                .expect("armed confirmation")
                .action = PendingDestructive::KillSession;
            press_new_temporary(&mut backend);
            assert_eq!(
                backend
                    .state()
                    .pending_destructive
                    .as_ref()
                    .map(|pending| pending.action),
                Some(PendingDestructive::NewTemporarySession)
            );
            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("eph-confirm")
            );
        });
    }
}
