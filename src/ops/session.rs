use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::ops::focus::{
    request_current_pane_focus, request_rename_session_focus, request_session_picker_focus,
};
use crate::session::discovery::DiscoveredSession;
use crate::state::{NamingMode, SessionPickerState, SessionRenameState};
use crate::{HyprmuxApp, pty_events::info_toast};

/// Clear any armed session-picker kill and dismiss its confirmation toast. Called from every path
/// that abandons or resolves the arming (a confirmed kill, moving off the row, editing the query,
/// refreshing, closing, or switching sessions) so the "press again" toast never outlives the
/// confirmation. A no-op when nothing is armed.
pub(crate) fn clear_pending_kill(ctx: &mut Context<HyprmuxApp>) {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.pending_kill = None;
    }
}

/// Clear any armed "open discards the current ephemeral" confirmation. The open sibling of
/// [`clear_pending_kill`]; the two arms are mutually exclusive, so abandon paths clear both via
/// [`clear_pending_session_arms`].
pub(crate) fn clear_pending_open(ctx: &mut Context<HyprmuxApp>) {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.pending_open = None;
    }
}

/// Clear both session-picker confirmations (kill and open). Called from every path that abandons or
/// resolves an arming without itself re-arming one - closing, navigating away, editing the query, or
/// moving the highlight off the armed row.
pub(crate) fn clear_pending_session_arms(ctx: &mut Context<HyprmuxApp>) {
    clear_pending_kill(ctx);
    clear_pending_open(ctx);
}

/// Cadence for the off-thread auto-refresh that keeps the open session picker current (sessions
/// appearing/disappearing from other UIs) without a manual refresh key.
const SESSION_PICKER_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);

pub(crate) fn open_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match picker_rows(ctx) {
        Ok(rows) => {
            let mut picker = SessionPickerState::new(rows);
            if let Some(current_name) = ctx.state.session_name.as_deref()
                && let Some(pos) = picker
                    .entries
                    .iter()
                    .position(|entry| entry.name == current_name)
            {
                picker.selected = pos;
            }
            ctx.state.session_picker = Some(picker);
        }
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
        let len = picker.entries.len();
        picker.pending_kill.is_some_and(|index| index >= len)
            || picker.pending_open.is_some_and(|index| index >= len)
    });
    if armed_out_of_range {
        clear_pending_session_arms(ctx);
    }
    Update::with_command(session_watch_command(epoch, ctx.state.session_name.clone()))
}

fn session_watch_command(epoch: u64, current_name: Option<String>) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(SESSION_PICKER_REFRESH_INTERVAL);
        // Discovery runs here (off the UI thread); a failed sweep simply skips this tick and lets
        // the loop stop rather than clobbering the last good list.
        if let Ok(rows) = discover_picker_sessions(current_name.as_deref()) {
            link.send(Msg::SessionsDiscovered { epoch, rows });
        }
    })
}

/// Build the full picker row list: every discovered session plus a row for the currently attached
/// one, sorted by name.
fn picker_rows(ctx: &Context<HyprmuxApp>) -> std::io::Result<Vec<DiscoveredSession>> {
    let current_name = ctx.state.session_name.as_deref();
    let mut rows = discover_picker_sessions(current_name)?;
    push_current_session_row(ctx, &mut rows);
    Ok(rows)
}

/// Discover sessions for the picker: exclude the currently attached one (re-added separately by
/// [`push_current_session_row`]) and drop *foreign* ephemeral sessions. Ephemeral sessions are
/// per-process, disposable, and self-reaping, and their `eph-…` names are reserved; another
/// process's ephemeral has no business being a selectable row (attaching would fight its owner over
/// teardown), so it is filtered out here. Our own ephemeral still appears via the current row.
fn discover_picker_sessions(current_name: Option<&str>) -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = crate::session::discovery::discover_sessions_excluding(current_name)?;
    rows.retain(|entry| !entry.ephemeral);
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
                clients: ctx.state.attached_client_count(),
            },
        });
        rows.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

fn require_attached(ctx: &mut Context<HyprmuxApp>) -> Option<()> {
    if ctx.state.session_attached {
        Some(())
    } else {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
        None
    }
}

fn require_writable(ctx: &mut Context<HyprmuxApp>) -> Option<()> {
    require_attached(ctx)?;
    let Some(shared) = ctx.state.shared.as_ref() else {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
        return None;
    };
    if shared.read_only {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Attached read-only"));
        return None;
    }
    Some(())
}

/// If this client is a follower (attached but not the controller), push the take-control nudge and
/// return `true` so the caller aborts a layout-mutating gesture. Controllers and local/unattached
/// sessions return `false`.
pub(crate) fn nudge_if_follower(ctx: &mut Context<HyprmuxApp>) -> bool {
    if ctx.state.is_controller() {
        return false;
    }
    let who = ctx
        .state
        .shared
        .as_ref()
        .and_then(|shared| shared.controller)
        .map(|id| format!("client {id}"))
        .unwrap_or_else(|| "another client".to_string());
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        format!("Layout controlled by {who}\nTry requesting control"),
    ));
    true
}

/// Ask the current controller for the layout-control lease (cooperative - never steals). A no-op
/// with a toast when unattached, read-only, or already in control. The server auto-grants only when
/// no controller holds the lease; otherwise it flags the request and notifies the controller, who
/// grants or declines from the session-clients view. Repeated presses re-send harmlessly (the server
/// debounces the controller's toast) but keep the local status message informative.
pub(crate) fn request_control(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    if ctx.state.is_controller() {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            "You already control the layout",
        ));
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    let shared = ctx.state.shared.as_ref().expect("writable session checked");
    let already_requested = shared
        .clients
        .iter()
        .any(|client| client.id == shared.client_id && client.requesting_control);
    let controller_label = shared
        .controller
        .and_then(|id| shared.clients.iter().find(|client| client.id == id))
        .map(|client| format!("{} #{}", client.label, client.id));
    if let Some(client) = ctx.state.session_client.clone() {
        client.request_control();
    }
    let message = match (already_requested, controller_label) {
        (true, Some(who)) => format!("Still waiting on {who} for layout control"),
        (true, None) => "Control request already pending".to_string(),
        (false, Some(who)) => format!("Requested layout control from {who}"),
        (false, None) => "Requested layout control".to_string(),
    };
    ctx.toast().push(info_toast(&ctx.state.theme, message));
    Update::full()
}

pub(crate) fn open_client_list(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    ctx.state.show_palette = false;
    ctx.state.show_session_picker = false;
    ctx.state.client_list = Some(crate::state::ClientListState { selected: 0 });
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::client_list_key());
    Update::full()
}

pub(crate) fn toggle_input_lock(ctx: &mut Context<HyprmuxApp>) -> Update {
    if nudge_if_follower(ctx) {
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    let shared = ctx.state.shared.as_ref().expect("writable session checked");
    if let Some(client) = ctx.state.session_client.as_ref() {
        client.set_input_lock(!shared.input_locked);
    }
    Update::full()
}

pub(crate) fn grant_control(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(shared) = ctx.state.shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.read_only {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            "Read-only clients cannot control the layout",
        ));
    } else if target.id != shared.client_id
        && let Some(client) = ctx.state.session_client.as_ref()
    {
        client.grant_control(target.id);
        ctx.state.client_list = None;
    }
    Update::full()
}

/// Controller-only quick action: grant the lease to the client that requested it (the earliest
/// pending requester when several are waiting). Nudges a follower, and toasts when nothing is
/// pending, so the bound key always gives feedback.
pub(crate) fn grant_control_to_requester(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(shared) = ctx.state.shared.as_ref() else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
        return Update::full();
    }
    let target = shared
        .clients
        .iter()
        .filter(|client| {
            client.requesting_control && !client.read_only && client.id != shared.client_id
        })
        .min_by_key(|client| client.id)
        .map(|client| client.id);
    match target {
        Some(id) => {
            if let Some(client) = ctx.state.session_client.as_ref() {
                client.grant_control(id);
            }
            ctx.state.client_list = None;
        }
        None => {
            ctx.toast()
                .push(info_toast(&ctx.state.theme, "No pending control requests"));
        }
    }
    Update::full()
}

/// Controller-only: decline the pending control request from the client at `index` in the roster.
/// A no-op (with a follower nudge) when this client is not the controller, or when the target has no
/// pending request.
pub(crate) fn decline_control(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(shared) = ctx.state.shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.requesting_control
        && target.id != shared.client_id
        && let Some(client) = ctx.state.session_client.as_ref()
    {
        client.decline_control(target.id);
    }
    Update::full()
}

/// Release the currently attached session before switching away from it. The single rule used
/// everywhere a transition leaves the current session: a solely attached controller tears down
/// its ephemeral server, while followers, viewers, shared ephemeral clients, and named-session
/// clients detach so they cannot destroy another client's session.
pub(crate) fn release_current_session(ctx: &mut Context<HyprmuxApp>) {
    crate::popup::kill_if_open(ctx);
    crate::update::flush_layout_commit(ctx);
    let Some(client) = ctx.state.session_client.clone() else {
        return;
    };
    if may_shutdown_ephemeral(&ctx.state) {
        client.shutdown();
    } else {
        client.detach();
    }
}

pub(crate) fn may_shutdown_ephemeral(state: &crate::state::State) -> bool {
    state.is_ephemeral_session()
        && state.is_controller()
        && state.attached_client_count() == 1
        && state.shared.as_ref().is_none_or(|shared| !shared.read_only)
}

/// Detach the current named session and exit the client, leaving the server running for reattach.
pub(crate) fn detach_current_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    crate::ops::exit::detach(ctx)
}

/// Kill the current session's server (its PTYs die with it) but keep the UI alive by switching to a
/// fresh ephemeral session.
pub(crate) fn kill_current_session(ctx: &mut Context<HyprmuxApp>, name: String) -> Update {
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

/// Replace `ctx.state` with a brand-new ephemeral session and spawn its attach after the current
/// session has been shut down.
pub(crate) fn swap_to_fresh_ephemeral(ctx: &mut Context<HyprmuxApp>) -> Update {
    let config = ctx.state.config.clone();
    let theme = ctx.state.theme.clone();
    let theme_watcher = ctx.state.theme_watcher.take();
    let system_theme = ctx.state.system_theme.clone();
    let control_socket_path = ctx.state.control_socket_path.clone();
    let command_link = ctx.state.command_link.clone();
    let old_epoch = ctx.state.runtime_epoch;
    let epoch = old_epoch.saturating_add(1);
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    let mut fresh = crate::state::State::new(config, theme);
    fresh.theme_watcher = theme_watcher;
    fresh.system_theme = system_theme;
    fresh.control_socket_path = control_socket_path;
    fresh.command_link = command_link;
    // Keep the pre-attach epoch so stale messages from the just-closed connection are filtered
    // out; `Msg::SessionAttached` advances it to `epoch` once the fresh ephemeral is live.
    fresh.runtime_epoch = old_epoch;
    fresh.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
    });
    ctx.state = fresh;
    ctx.state.commands_dirty = true;
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

pub(crate) fn attach_session_by_name(
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    autostart: bool,
) -> Update {
    if !crate::session::discovery::valid_attach_target(&name) {
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
        read_only: false,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, autostart, false, link)
        });
    }))
}

pub(crate) fn activate_selected_session(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    // A kill can never be armed on the same press that resolves an open, so drop any kill arm; the
    // open arm is handled explicitly below.
    clear_pending_kill(ctx);
    let Some(entry) = ctx
        .state
        .session_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index).cloned())
    else {
        return Update::full();
    };
    // Discovery already probed this session; an `Unknown` status means the handshake was refused
    // (an incompatible older server is the usual cause). Attaching would only fail after the connect
    // retry deadline, so reject it up front, keep the picker open, and point at the fix - killing
    // the row (Ctrl+K) still works even against a server we can't speak to.
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Unknown
    ) {
        clear_pending_open(ctx);
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            format!(
                "Session `{}` is unavailable (incompatible version)\nPress Ctrl+K to remove it",
                entry.name
            ),
        ));
        return Update::full();
    }
    // Attaching elsewhere while on a disposable ephemeral session shuts that server down and kills
    // its panes (see `release_current_session`). That is easy to trigger by reflex from the picker,
    // so guard it with the same two-press confirmation as a kill: the first Enter arms and warns,
    // the second commits. Switching between two named sessions parks the old one and needs no guard.
    let discards_ephemeral = ctx.state.session_attached
        && ctx.state.is_ephemeral_session()
        && ctx.state.session_name.as_deref() != Some(entry.name.as_str());
    if discards_ephemeral {
        let armed = ctx
            .state
            .session_picker
            .as_ref()
            .is_some_and(|picker| picker.pending_open == Some(index));
        if !armed {
            // Arm the confirmation: the target row renders the warning-colored "⏎ again - ends temp
            // session" cue, so a second Enter is required and no toast is needed.
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                picker.pending_open = Some(index);
            }
            return Update::full();
        }
    }
    clear_pending_open(ctx);
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
        read_only: false,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

/// Close the session picker. Normally this just returns focus to the current pane, but if it is the
/// startup picker being dismissed with nothing attached, fall back to attaching an ephemeral session
/// so the launch is never stranded without a terminal.
pub(crate) fn close_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
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

/// Swap whatever overlays are open for a session naming/rename prompt and focus it. Shared by the
/// create-new, rename-in-place, and detach-and-name entry points so they raise the prompt the same
/// way.
fn enter_session_rename(ctx: &mut Context<HyprmuxApp>, rename: SessionRenameState) -> Update {
    ctx.state.rename_session = Some(rename);
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.mode = crate::state::Mode::Normal;
    request_rename_session_focus(ctx);
    Update::full()
}

pub(crate) fn open_create_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_create())
}

/// Raise the "name this session to detach" prompt for the current ephemeral session. An ephemeral
/// session has no reattachable name, so naming it (Enter) is what makes a detach meaningful: the
/// server is renamed, kept running, and the client leaves (see `apply_rename_session`,
/// `NameEphemeralSession` + `detach_after`). Cancelling (`Esc`) returns to the session without
/// tearing anything down - quitting is the destructive path.
pub(crate) fn open_detach_rename(ctx: &mut Context<HyprmuxApp>) -> Update {
    enter_session_rename(ctx, SessionRenameState::for_detach())
}

/// Open the prompt to rename the *current* session in place. Unlike the picker (which switches to a
/// separate session), this keeps every live pane where it is and just changes the name the server is
/// discoverable under. Works for both ephemeral (naming it for the first time) and already-named
/// sessions.
pub(crate) fn open_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    let ephemeral = ctx.state.is_ephemeral_session();
    let initial = if ephemeral {
        String::new()
    } else {
        ctx.state.session_name.clone().unwrap_or_default()
    };
    let mode = if ephemeral {
        NamingMode::NameEphemeralSession
    } else {
        NamingMode::RenameSession
    };
    enter_session_rename(ctx, SessionRenameState::new(initial, mode))
}

pub(crate) fn apply_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(rename_state) = ctx.state.rename_session.as_ref() else {
        return Update::none();
    };
    let name = rename_state.input.text().trim().to_string();

    match rename_state.mode {
        NamingMode::RenameWorkspace { index } => {
            if let Some(workspace) = ctx.state.workspaces.get_mut(index) {
                workspace.name = (!name.is_empty()).then_some(name);
            }
            ctx.state.rename_session = None;
            ctx.state.commands_dirty = true;
            request_current_pane_focus(ctx);
            Update::full()
        }
        NamingMode::CreateSession => {
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Sessions",
                    "Use letters, numbers, _ or - for session names",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            // Creating a session from an ephemeral one discards that disposable session. Guard it
            // with a two-press confirm shown *in the modal* (red border + inline note) rather than a
            // toast: the first Enter arms, a second commits. Editing the name clears the arm (see
            // `Msg::RenameSessionChanged`).
            let needs_confirm = ctx.state.session_attached && ctx.state.is_ephemeral_session();
            let armed = ctx
                .state
                .rename_session
                .as_ref()
                .is_some_and(|rename| rename.pending_confirm);
            if needs_confirm && !armed {
                if let Some(rename) = ctx.state.rename_session.as_mut() {
                    rename.pending_confirm = true;
                }
                request_rename_session_focus(ctx);
                return Update::full();
            }
            ctx.state.rename_session = None;

            attach_session_by_name(ctx, name, true)
        }
        NamingMode::NameEphemeralSession => {
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Sessions",
                    "Use letters, numbers, _ or - for session names",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            let detach_after = rename_state.detach_after;
            ctx.state.rename_session = None;

            if detach_after {
                let Some(client) = ctx.state.session_client.clone() else {
                    ctx.toast().push(crate::pty_events::error_toast(
                        &ctx.state.theme,
                        "Sessions",
                        "Lost connection to the session - can't name it right now.",
                    ));
                    return Update::full();
                };
                client.rename(name);
                client.detach();
                crate::profiles::persist_session_on_detach(&ctx.state);
                ctx.quit();
                return Update::none();
            }

            if ctx.state.session_name.as_deref() == Some(name.as_str()) {
                request_current_pane_focus(ctx);
                return Update::full();
            }

            if let Some(client) = ctx.state.session_client.clone() {
                client.rename(name);
            }
            request_current_pane_focus(ctx);
            Update::full()
        }
        NamingMode::RenameSession => {
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Sessions",
                    "Use letters, numbers, _ or - for session names",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            ctx.state.rename_session = None;

            if ctx.state.session_name.as_deref() == Some(name.as_str()) {
                request_current_pane_focus(ctx);
                return Update::full();
            }

            if let Some(client) = ctx.state.session_client.clone() {
                client.rename(name);
            }
            request_current_pane_focus(ctx);
            Update::full()
        }
    }
}

pub(crate) fn close_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    // Cancelling any session naming prompt - including the detach-and-name one - just returns to the
    // session. A detach never tears panes down: quitting (with its own confirmation) is the only
    // path that shuts an ephemeral server down.
    ctx.state.rename_session = None;
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
    let armed = picker.pending_kill == Some(index);
    // The current session may be ephemeral; keep the label in sync.
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    if !armed {
        // First press arms the kill: drop any stale arming (kill or open), then mark this row. The
        // row renders its own struck-through "again to confirm" cue, so no confirm toast is needed.
        clear_pending_session_arms(ctx);
        if let Some(picker) = ctx.state.session_picker.as_mut() {
            picker.pending_kill = Some(index);
        }
        return Update::full();
    }
    clear_pending_session_arms(ctx);
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
    let endpoint = crate::session::server::session_endpoint(name)?;
    if let Ok(mut stream) = endpoint.connect() {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(1)));
        // Grab the server's pid up front (protocol-independent) so an older, incompatible server -
        // one that rejects our attach handshake and can't be told to `Shutdown` - can still be
        // reaped instead of leaking as an unkillable orphan (cross-platform plan Phase 5b
        // stale-server recovery). `terminate_server` is the forced last resort *after* the graceful
        // protocol path has already failed: SIGTERM escalating to SIGKILL on Unix, `TerminateProcess`
        // on Windows (which takes the server's kill-on-close Job Object, and so its ConPTY children,
        // with it).
        let server_pid = stream.peer_pid();
        if graceful_shutdown(&mut stream, name).is_err()
            && let Some(pid) = server_pid
        {
            crate::platform::server_lifecycle::terminate_server(pid);
        }
    }
    // The server retires its endpoint only once it finishes tearing down, which races the refresh
    // that follows a kill (and a killed or already-dead server may never retire it at all). Drop it
    // now so the killed session leaves the list immediately.
    endpoint.remove_stale();
    crate::session::server::delete_snapshot(name)?;
    Ok(())
}

/// Attempt the in-protocol graceful shutdown: attach, then send `Shutdown`. Returns an error if the
/// server refuses the handshake (e.g. a version mismatch replies with `Error`) or the connection
/// breaks, signalling [`shutdown_session`] to fall back to a signal-based kill rather than pushing a
/// `Shutdown` into a half-closed pipe (which surfaced as a bogus "Broken pipe" delete failure).
fn graceful_shutdown(
    stream: &mut crate::platform::ipc::IpcConnection,
    name: &str,
) -> std::io::Result<()> {
    use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};
    crate::session::protocol::write_frame(
        stream,
        &ClientMessage::Attach {
            session: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            label: crate::platform::user::current_user_label(),
            read_only: false,
        },
    )?;
    match crate::session::protocol::read_frame::<_, ServerMessage>(stream)? {
        ServerMessage::Attached { .. } => {
            crate::session::protocol::write_frame(stream, &ClientMessage::Shutdown)
        }
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("server refused shutdown handshake: {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use crate::session::protocol::ClientInfo;
    use crate::state::{SharedSessionState, State, ThemePreset};

    fn ephemeral_state(client_id: u64, controller: u64, clients: Vec<ClientInfo>) -> State {
        let mut state = State::new(HyprmuxConfig::default(), ThemePreset::Lipan.theme());
        state.session_name = Some("eph-test".to_string());
        state.session_attached = true;
        let mut shared = SharedSessionState::new(client_id);
        shared.controller = Some(controller);
        shared.clients = clients;
        state.shared = Some(shared);
        state
    }

    #[test]
    fn follower_request_control_asks_the_controller_without_stealing() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::input::Action;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    // A follower: client 2 holds the lease.
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2);
                    shared.clients = vec![
                        ClientInfo {
                            id: 1,
                            label: "me".into(),
                            read_only: false,
                            requesting_control: false,
                        },
                        ClientInfo {
                            id: 2,
                            label: "them".into(),
                            read_only: false,
                            requesting_control: false,
                        },
                    ];
                    state.shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::RunAction(Action::RequestControl))
                    .expect("dispatch request-control");

                let sent: Vec<ClientOutbound> = rx.try_iter().collect();
                assert!(
                    sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::RequestControl)
                    )),
                    "a follower must ask for control, got {sent:?}"
                );
                assert!(
                    !sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::GrantControl { .. })
                    )),
                    "requesting must never steal the lease"
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn controller_grant_control_key_grants_to_the_earliest_requester() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::input::Action;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.session_attached = true;
                    state.session_client = Some(client);
                    // We (client 1) are the controller; clients 2 and 3 both want control.
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    let requester = |id| ClientInfo {
                        id,
                        label: format!("c{id}"),
                        read_only: false,
                        requesting_control: true,
                    };
                    shared.clients = vec![
                        ClientInfo {
                            id: 1,
                            label: "me".into(),
                            read_only: false,
                            requesting_control: false,
                        },
                        requester(3),
                        requester(2),
                    ];
                    state.shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::RunAction(Action::GrantControl))
                    .expect("dispatch grant-control");

                let sent: Vec<ClientOutbound> = rx.try_iter().collect();
                // The earliest requester (smallest id = 2) is granted, not the roster's first entry.
                assert!(
                    sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::GrantControl { to: 2 })
                    )),
                    "expected a grant to client 2, got {sent:?}"
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn only_solo_ephemeral_controller_may_shutdown_on_release() {
        let client = |id| ClientInfo {
            id,
            label: format!("client-{id}"),
            read_only: false,
            requesting_control: false,
        };
        assert!(may_shutdown_ephemeral(&ephemeral_state(
            1,
            1,
            vec![client(1)]
        )));
        assert!(!may_shutdown_ephemeral(&ephemeral_state(
            2,
            1,
            vec![client(1), client(2)]
        )));
        assert!(!may_shutdown_ephemeral(&ephemeral_state(
            1,
            1,
            vec![client(1), client(2)]
        )));
    }
}
