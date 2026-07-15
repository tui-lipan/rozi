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

pub(crate) fn clear_pending(ctx: &mut Context<HyprmuxApp>) {
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
/// Detaching an *anonymous* ephemeral session is contradictory: it has no name to reattach by, so a
/// literal detach could only shut it down - indistinguishable from a quit, minus the confirmation.
/// Instead an ephemeral session first prompts for a name; naming it turns the detach into a durable
/// named detach that keeps the server running (see [`crate::ops::session::open_detach_rename`] and
/// [`crate::ops::session::apply_rename_session`]), while cancelling returns to the session. Tearing an
/// ephemeral session down is left to [`quit_client`], which guards it with `[confirm].quit_ephemeral`.
/// A named session (or one with no live client to rename) detaches immediately.
pub(crate) fn detach(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::popup::kill_if_open(ctx);
    crate::update::flush_layout_commit(ctx);
    clear_pending(ctx);
    if ctx.state.is_ephemeral_session() && ctx.state.session_client.is_some() {
        return crate::ops::session::open_detach_rename(ctx);
    }
    mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.session_client.clone() {
        client.detach();
        profiles::persist_session_on_detach(&ctx.state);
    }
    ctx.quit();
    Update::none()
}

/// Mark the current session as intentionally left and emit its hook event exactly once.
/// Callers must flush pending layout changes before entering this transition.
pub(crate) fn mark_session_detached(ctx: &mut Context<HyprmuxApp>, session: Option<&str>) {
    if !ctx.state.session_attached {
        return;
    }
    let session = session
        .map(str::to_string)
        .or_else(|| ctx.state.session_name.clone())
        .unwrap_or_default();
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::SessionDetached,
            vec![("session", session)],
        ),
    );
    ctx.state.session_attached = false;
}

/// The controlling terminal/console went away (Unix `SIGHUP`/`SIGTERM`, Windows console close,
/// logoff, or shutdown): detach cleanly instead of letting the process be killed where it stands
/// (cross-platform plan Phase 5b).
///
/// Unlike [`detach`], this never prompts. There is nobody left to answer a prompt, and the window
/// before the OS force-kills us is short (Windows gives a few seconds; a Unix `SIGHUP` is usually
/// followed by the emulator exiting). So an ephemeral session skips the "name it first" flow and
/// simply detaches: its server shuts itself down after the no-client grace period, which is the
/// right outcome for a session nobody can reattach to by name anyway.
pub(crate) fn detach_on_hangup(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::update::flush_layout_commit(ctx);
    mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.session_client.clone() {
        client.detach();
    }
    profiles::persist_session_on_detach(&ctx.state);
    ctx.quit();
    Update::none()
}

/// Whether any tiled/floating pane still has a running process. Used to decide whether quitting
/// an ephemeral session (which shuts the server down and kills its PTYs) warrants a confirmation.
pub(crate) fn any_pane_live(state: &State) -> bool {
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
    crate::update::flush_layout_commit(ctx);
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
    crate::popup::kill_if_open(ctx);
    let shutdown_ephemeral = crate::ops::session::may_shutdown_ephemeral(&ctx.state);
    mark_session_detached(ctx, None);
    if shutdown_ephemeral && let Some(client) = ctx.state.session_client.clone() {
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
    if !ctx.state.is_controller() {
        crate::ops::session::nudge_if_follower(ctx);
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
    crate::ops::session::kill_current_session(ctx, session_name)
}

pub(crate) fn confirm_new_temporary_session(ctx: &mut Context<HyprmuxApp>) -> bool {
    let pending = PendingDestructive::NewTemporarySession;
    let toast = confirm_toast(
        &ctx.state.theme,
        "Again to start new temporary session\n(current will be discarded)",
    );
    confirm_second_press(ctx, pending, toast)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientInfo, ClientMessage};
    use crate::state::SharedSessionState;
    use tui_lipan::TestBackend;

    fn confirming_backend() -> TestBackend<HyprmuxApp> {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        let (client, _rx) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.session_name = Some("eph-confirm".to_string());
        state.session_attached = true;
        state.session_client = Some(client);
        state.config.confirm.new_temporary_session = true;
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.clients = vec![ClientInfo {
            id: 1,
            label: "me".to_string(),
            read_only: false,
            requesting_control: false,
        }];
        state.shared = Some(shared);
        backend
    }

    fn press_new_temporary(backend: &mut TestBackend<HyprmuxApp>) {
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
        TestBackend<HyprmuxApp>,
        std::sync::mpsc::Receiver<ClientOutbound>,
        std::sync::mpsc::Receiver<String>,
    ) {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        let (client, outbound) = SessionClient::test_channel();
        let events = {
            let state = backend.state_mut();
            state.session_name = Some("named".to_string());
            state.session_attached = true;
            state.session_client = Some(client);
            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(1);
            shared.clients = vec![ClientInfo {
                id: 1,
                label: "me".to_string(),
                read_only: false,
                requesting_control: false,
            }];
            shared.last_committed_layout = None;
            state.shared = Some(shared);
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
            assert!(!backend.state().session_attached);
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
            assert!(!backend.state().session_attached);
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
            assert!(!backend.state().session_attached);
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
            assert_ne!(backend.state().session_name.as_deref(), Some("eph-confirm"));
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
            assert_eq!(backend.state().session_name.as_deref(), Some("eph-confirm"));
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
            assert_eq!(backend.state().session_name.as_deref(), Some("eph-confirm"));
        });
    }
}
