use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::focus_ops::{
    request_current_pane_focus, request_rename_session_focus, request_session_picker_focus,
};
use crate::session::discovery::DiscoveredSession;
use crate::state::{PendingKill, SessionPickerState, SessionRenameState};
use crate::{
    HyprmuxApp,
    pty_events::{confirm_toast, info_toast},
};

/// Clear any armed session-picker kill and dismiss its confirmation toast. Called from every path
/// that abandons or resolves the arming (a confirmed kill, moving off the row, editing the query,
/// refreshing, closing, or switching sessions) so the "press again" toast never outlives the
/// confirmation. A no-op when nothing is armed.
pub(crate) fn clear_pending_kill(ctx: &mut Context<HyprmuxApp>) {
    let toast_id = ctx
        .state
        .session_picker
        .as_mut()
        .and_then(|picker| picker.pending_kill.take())
        .map(|pending| pending.toast_id);
    if let Some(toast_id) = toast_id {
        ctx.toast().dismiss(toast_id);
    }
}

/// Cadence for the off-thread auto-refresh that keeps the open session picker current (sessions
/// appearing/disappearing from other UIs) without a manual refresh key.
const SESSION_PICKER_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);

pub(crate) fn open_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match picker_rows(ctx) {
        Ok(rows) => ctx.state.session_picker = Some(SessionPickerState::new(rows)),
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
            ctx.state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }
    }
    ctx.state.show_session_picker = true;
    // A new opening invalidates any in-flight watcher tick from a prior opening.
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    request_session_picker_focus(ctx);
    Update::with_command(session_watch_command(
        ctx.state.session_picker_epoch,
        ctx.state.session_name.clone(),
    ))
}

/// Open the session picker at startup (nothing attached yet). Sets up the picker state and returns
/// the watcher epoch so `init` can kick off the first discovery tick. Discovery failures degrade to
/// an empty picker (Esc still falls back to a fresh ephemeral session).
pub(crate) fn open_startup_session_picker(ctx: &mut Context<HyprmuxApp>) -> u64 {
    let rows = picker_rows(ctx).unwrap_or_default();
    ctx.state.session_picker = Some(SessionPickerState::new(rows));
    ctx.state.show_session_picker = true;
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    ctx.state.commands_dirty = true;
    request_session_picker_focus(ctx);
    ctx.state.session_picker_epoch
}

pub(crate) fn refresh_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match picker_rows(ctx) {
        Ok(rows) => {
            // Carry the typed query and the highlighted row across the rebuild. After a kill the
            // killed row is gone, so clamping keeps the highlight on the row that slid into its
            // place instead of snapping back to the top; it also keeps our `selected` in step with
            // the persistent `SearchPalette` component, which does not re-resolve its internal
            // keyboard selection when the entry list changes underneath it.
            let (query, selected) = ctx
                .state
                .session_picker
                .as_ref()
                .map(|p| (p.input.text().to_string(), p.selected))
                .unwrap_or_default();
            let mut picker = SessionPickerState::new(rows);
            picker.input.set_text(query);
            picker.selected = selected.min(picker.entries.len().saturating_sub(1));
            ctx.state.session_picker = Some(picker);
        }
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
        }
    };
    Update::full()
}

/// Apply a batch of freshly discovered sessions from the auto-refresh watcher, then re-arm the next
/// tick. Ignored (stopping the loop) once the picker is closed or a newer opening supersedes this
/// `epoch`, which is how the watcher shuts itself down. Selection and the typed query are preserved
/// so a live refresh never disrupts navigation.
pub(crate) fn apply_discovered_sessions(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    mut rows: Vec<DiscoveredSession>,
) -> Update {
    if !ctx.state.show_session_picker || epoch != ctx.state.session_picker_epoch {
        return Update::none();
    }
    push_current_session_row(ctx, &mut rows);
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.entries = rows;
        picker.selected = picker.selected.min(picker.entries.len().saturating_sub(1));
    }
    let armed_out_of_range = ctx.state.session_picker.as_ref().is_some_and(|picker| {
        picker
            .pending_kill
            .is_some_and(|pending| pending.index >= picker.entries.len())
    });
    if armed_out_of_range {
        clear_pending_kill(ctx);
    }
    Update::with_command(session_watch_command(epoch, ctx.state.session_name.clone()))
}

fn session_watch_command(epoch: u64, current_name: Option<String>) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(SESSION_PICKER_REFRESH_INTERVAL);
        // Discovery runs here (off the UI thread); a failed sweep simply skips this tick and lets
        // the loop stop rather than clobbering the last good list.
        if let Ok(rows) =
            crate::session::discovery::discover_sessions_excluding(current_name.as_deref())
        {
            link.send(Msg::SessionsDiscovered { epoch, rows });
        }
    })
}

/// Build the full picker row list: every discovered session plus a row for the currently attached
/// one, sorted by name.
fn picker_rows(ctx: &Context<HyprmuxApp>) -> std::io::Result<Vec<DiscoveredSession>> {
    let current_name = ctx.state.session_name.as_deref();
    let mut rows = crate::session::discovery::discover_sessions_excluding(current_name)?;
    push_current_session_row(ctx, &mut rows);
    Ok(rows)
}

/// Append a row for the attached session (discovery excludes it) and keep the list sorted.
fn push_current_session_row(ctx: &Context<HyprmuxApp>, rows: &mut Vec<DiscoveredSession>) {
    if let Some(name) = &ctx.state.session_name {
        rows.push(DiscoveredSession {
            name: name.clone(),
            ephemeral: ctx.state.is_ephemeral_session(),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: ctx.state.workspaces.iter().map(|w| w.panes.len()).sum(),
                has_layout: true,
            },
        });
        rows.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// Release the currently attached session before switching away from it. The single rule used
/// everywhere a transition leaves the current session: an ephemeral session is torn down
/// (`shutdown`, its PTYs die with it — it is disposable and would otherwise leak an orphan
/// server), while a named session is parked (its layout is pushed and the client detaches, so the
/// server stays running for later reattach). A no-op when nothing is attached.
pub(crate) fn release_current_session(ctx: &Context<HyprmuxApp>) {
    let Some(client) = ctx.state.session_client.clone() else {
        return;
    };
    if ctx.state.is_ephemeral_session() {
        client.shutdown();
    } else {
        client.push_layout(
            crate::profiles::profile_from_state(&ctx.state)
                .to_toml_string()
                .unwrap_or_default(),
        );
        client.detach();
    }
}

/// Detach the current session and switch the UI to a fresh ephemeral session, so the user keeps a
/// working terminal. A named session stays parked for reattach; an ephemeral one is shut down (it
/// is disposable), which is equivalent to starting a brand-new ephemeral session.
pub(crate) fn detach_current_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_kill(ctx);
    if !ctx.state.session_attached {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
        return Update::full();
    }
    let was_named = !ctx.state.is_ephemeral_session();
    release_current_session(ctx);
    let update = swap_to_fresh_ephemeral(ctx);
    let message = if was_named {
        "Detached (session still running)"
    } else {
        "Started a fresh ephemeral session"
    };
    ctx.toast().push(info_toast(&ctx.state.theme, message));
    update
}

/// Kill the current session's server (its PTYs die with it) but keep the UI alive by switching to a
/// fresh ephemeral session — the picker equivalent of "kill session" without quitting the client.
fn kill_current_session(ctx: &mut Context<HyprmuxApp>, name: String) -> Update {
    if let Some(client) = ctx.state.session_client.clone() {
        client.shutdown();
    }
    let update = swap_to_fresh_ephemeral(ctx);
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        format!("Killed session `{name}`"),
    ));
    update
}

/// Replace `ctx.state` with a brand-new ephemeral session and spawn its attach. Shared by detach
/// (old server left running) and killing the current session (old server already told to shut down);
/// the caller is responsible for detaching/shutting down the outgoing connection first.
fn swap_to_fresh_ephemeral(ctx: &mut Context<HyprmuxApp>) -> Update {
    let config = ctx.state.config.clone();
    let theme = ctx.state.theme.clone();
    let theme_watcher = ctx.state.theme_watcher.take();
    let system_theme = ctx.state.system_theme.clone();
    let control_socket_path = ctx.state.control_socket_path.clone();
    let old_epoch = ctx.state.runtime_epoch;
    let epoch = old_epoch.saturating_add(1);
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    let mut fresh = crate::state::State::new(config, theme);
    fresh.theme_watcher = theme_watcher;
    fresh.system_theme = system_theme;
    fresh.control_socket_path = control_socket_path;
    // Keep the pre-attach epoch so stale messages from the just-closed connection are filtered
    // out; `Msg::SessionAttached` advances it to `epoch` once the fresh ephemeral is live.
    fresh.runtime_epoch = old_epoch;
    fresh.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
    });
    ctx.state = fresh;
    ctx.state.commands_dirty = true;
    crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, true, link));
    }))
}

pub(crate) fn attach_session_by_name(
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    autostart: bool,
) -> Update {
    if !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            "Invalid session name",
        ));
        return Update::full();
    }
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(name.as_str()) {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Already attached to `{name}`"),
        ));
        return Update::full();
    }
    if ctx.state.pending_session_attach.is_some() {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Attach already in progress"));
        return Update::full();
    }
    // Attach-elsewhere: release the current session (a named one is parked for reattach; an
    // ephemeral one is torn down so it does not leak an orphan server), then attach to the target.
    release_current_session(ctx);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    let epoch = ctx.state.runtime_epoch.saturating_add(1);
    ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, autostart, link));
    }))
}

pub(crate) fn activate_selected_session(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    clear_pending_kill(ctx);
    let Some(entry) = ctx
        .state
        .session_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index).cloned())
    else {
        return Update::full();
    };
    // A session shown in the picker is already running, so don't autostart a replacement if it
    // died between discovery and attach.
    attach_session_by_name(ctx, entry.name, false)
}

/// Attach the current (initial or restored-profile) state to this process's ephemeral session.
/// Used when the startup picker's "new ephemeral" row is chosen or the picker is dismissed with no
/// session attached, so a launch always ends with a working terminal.
pub(crate) fn attach_startup_ephemeral(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    let epoch = ctx.state.runtime_epoch;
    let name = crate::state::ephemeral_session_name();
    ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, true, link));
    }))
}

/// Close the session picker. Normally this just returns focus to the current pane, but if it is the
/// startup picker being dismissed with nothing attached, fall back to attaching an ephemeral session
/// so the launch is never stranded without a terminal.
pub(crate) fn close_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_kill(ctx);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    if !ctx.state.session_attached
        && ctx.state.session_client.is_none()
        && ctx.state.pending_session_attach.is_none()
    {
        return attach_startup_ephemeral(ctx);
    }
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn create_from_query(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_kill(ctx);
    let name = ctx
        .state
        .session_picker
        .as_ref()
        .map(|picker| picker.input.text().trim().to_string())
        .unwrap_or_default();
    if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            "Use letters, numbers, _ or - for session names",
        ));
        return Update::full();
    }
    // Creating/selecting from the picker always goes to a *separate* session, parking the current
    // one (see `attach_session_by_name`). Renaming the session you're in is a distinct action
    // (`open_rename_session`) so the two intents never share one gesture.
    attach_session_by_name(ctx, name, true)
}

/// Open the prompt to rename the *current* session in place. Unlike the picker (which switches to a
/// separate session), this keeps every live pane where it is and just changes the name the server is
/// discoverable under. Works for both ephemeral (naming it for the first time) and already-named
/// sessions.
pub(crate) fn open_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.session_attached {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
        return Update::full();
    }
    // Seed with the current name only when it's a real one; an ephemeral's generated `eph-…` name is
    // never a useful starting point.
    let initial = if ctx.state.is_ephemeral_session() {
        String::new()
    } else {
        ctx.state.session_name.clone().unwrap_or_default()
    };
    ctx.state.rename_session = Some(SessionRenameState::new(initial));
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.mode = crate::state::Mode::Normal;
    request_rename_session_focus(ctx);
    Update::full()
}

/// Open the name prompt raised by `prefix d` on an ephemeral session: naming it detaches durably,
/// cancelling quits (see [`crate::exit_ops::detach`]). Distinct from [`open_rename_session`] only
/// in the `detach_after` intent it carries.
pub(crate) fn open_rename_for_detach(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.rename_session = Some(SessionRenameState::for_detach());
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.mode = crate::state::Mode::Normal;
    request_rename_session_focus(ctx);
    Update::full()
}

pub(crate) fn apply_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some((name, detach_after)) = ctx
        .state
        .rename_session
        .as_ref()
        .map(|rename| (rename.input.text().trim().to_string(), rename.detach_after))
    else {
        return Update::none();
    };
    if !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            "Use letters, numbers, _ or - for session names",
        ));
        return Update::full();
    }
    // Name-on-detach: rename in place (so the server persists under a real name), push the layout,
    // then detach and quit. The rename and detach travel the same ordered connection, so the server
    // renames first (and thus never self-reaps as an ephemeral) and keeps running after the detach.
    if detach_after {
        let Some(client) = ctx.state.session_client.clone() else {
            // The session connection dropped (e.g. `SessionDisconnected` cleared the client) while
            // this name-on-detach prompt was open. Quitting now would send no rename and leave the
            // session ephemeral, so it would self-reap after the grace instead of persisting as the
            // user asked. Surface the failure and keep the prompt open so they can retry once
            // reconnected (or press Esc to quit and discard).
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                "Lost connection to the session - can't name it right now. Try again, or press Esc to quit.",
            ));
            request_rename_session_focus(ctx);
            return Update::full();
        };
        client.rename(name);
        client.push_layout(
            crate::profiles::profile_from_state(&ctx.state)
                .to_toml_string()
                .unwrap_or_default(),
        );
        client.detach();
        crate::profiles::persist_session_on_detach(&ctx.state);
        ctx.state.rename_session = None;
        ctx.quit();
        return Update::none();
    }
    if ctx.state.session_name.as_deref() == Some(name.as_str()) {
        return close_rename_session(ctx);
    }
    // The rename is server-side and asynchronous: the `Renamed` reply updates `session_name` and
    // toasts (a collision or reserved name surfaces there as an error). Panes never move.
    if let Some(client) = ctx.state.session_client.clone() {
        client.rename(name);
    }
    close_rename_session(ctx)
}

pub(crate) fn close_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    // Cancelling a name-on-detach prompt means the user declined to name the session, which the
    // detach flow treats as a plain quit (shutting the still-ephemeral server down).
    let detach_after = ctx
        .state
        .rename_session
        .as_ref()
        .is_some_and(|rename| rename.detach_after);
    ctx.state.rename_session = None;
    if detach_after {
        // Declining the name-on-detach prompt is already an explicit "quit and shut down"
        // decision, so it bypasses the ephemeral-quit confirmation.
        return crate::exit_ops::quit_client(ctx, false);
    }
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn kill_selected_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    let armed = picker
        .pending_kill
        .is_some_and(|pending| pending.index == index);
    // The current session may be ephemeral; keep the toast label in sync.
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    if !armed {
        // First press arms the kill: drop any stale arming, then track the confirm toast's id so
        // it can be dismissed the moment the kill runs or the arming is abandoned.
        clear_pending_kill(ctx);
        let toast_id = ctx.toast().push(confirm_toast(
            &ctx.state.theme,
            format!("Press Ctrl+K again to kill `{display}`"),
        ));
        if let Some(picker) = ctx.state.session_picker.as_mut() {
            picker.pending_kill = Some(PendingKill { index, toast_id });
        }
        return Update::full();
    }
    clear_pending_kill(ctx);
    // Killing the session you're attached to is fine: shut its server down and hop the UI onto a
    // fresh ephemeral session rather than quitting the client.
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(entry.name.as_str())
    {
        return kill_current_session(ctx, display);
    }
    match shutdown_session(&entry.name) {
        Ok(()) => {
            ctx.toast().push(info_toast(
                &ctx.state.theme,
                format!("Killed session `{display}`"),
            ));
            refresh_session_picker(ctx)
        }
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
            Update::full()
        }
    }
}

fn shutdown_session(name: &str) -> std::io::Result<()> {
    use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION};
    let path = crate::session::server::session_socket_path(name)?;
    let mut stream = std::os::unix::net::UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
    crate::session::protocol::write_frame(
        &mut stream,
        &ClientMessage::Attach {
            session: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
    )?;
    let _ = crate::session::protocol::read_frame::<_, crate::session::protocol::ServerMessage>(
        &mut stream,
    )?;
    crate::session::protocol::write_frame(&mut stream, &ClientMessage::Shutdown)?;
    // The server unlinks its socket only once it finishes tearing down, which races the refresh
    // that follows a kill. Drop the path now so the dead session leaves the list immediately.
    let _ = std::fs::remove_file(&path);
    Ok(())
}
