use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::focus::{
    request_current_pane_focus, request_rename_session_focus, request_session_picker_focus,
};
use crate::ops::session::attach::{
    attach_session_by_name, disconnect_host, held_ephemeral_session_in, kill_current_session,
    refresh_picker_after_kill, restart_current_session, start_local_launcher_shell,
};
use crate::ops::session::control_lease::require_attached;
use crate::ops::session::discovery::{immediate_picker_rows, session_watch_command};
use crate::session::discovery::DiscoveredSession;
use crate::state::{NamingMode, SessionPickerState, SessionRenameState, State};

/// Whether this picker row is the session currently in the foreground. Enter/switch/connect
/// hide themselves here; activating it must stay silent rather than toasting "already attached".
pub(crate) fn session_row_is_current(state: &State, entry: &DiscoveredSession) -> bool {
    state.current().session_name.as_deref() == Some(entry.name.as_str())
        && state.current().remote_target == entry.remote_target
}

pub(crate) fn session_row_is_restorable(entry: &DiscoveredSession) -> bool {
    matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Restorable
    )
}

/// Restart recreates a live server. A restorable snapshot has none, so the chord is omitted.
pub(crate) fn session_row_can_restart(entry: &DiscoveredSession) -> bool {
    !session_row_is_restorable(entry)
}

/// Disconnect closes a *background* attachment. The current session is Kill or leave; a row we
/// do not hold has nothing to drop.
pub(crate) fn session_row_can_disconnect(state: &State, entry: &DiscoveredSession) -> bool {
    !state.is_attached_to(&entry.name, entry.remote_target.as_ref())
        && state
            .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
            .is_some()
}

/// Host-wide disconnect is only offered when this client actually holds a connection to that
/// remote — the current session, or one parked in the background.
pub(crate) fn session_row_can_disconnect_host(state: &State, entry: &DiscoveredSession) -> bool {
    let Some(target) = entry.remote_target.as_ref() else {
        return false;
    };
    state.current().remote_target.as_ref() == Some(target)
        || state
            .background
            .values()
            .any(|attachment| attachment.remote_target.as_ref() == Some(target))
}

/// Clear any armed session-picker kill and dismiss its confirmation toast. Called from every path
/// that abandons or resolves the arming (a confirmed kill, moving off the row, editing the query,
/// refreshing, closing, or switching sessions) so the "press again" toast never outlives the
/// confirmation. A no-op when nothing is armed.
pub(crate) fn clear_pending_kill(ctx: &mut Context<AppRoot>) {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.pending_kill = None;
        picker.pending_restart = None;
    }
}

/// Clear the session-picker kill confirmation when navigation abandons its armed row.
pub(crate) fn clear_pending_session_arms(ctx: &mut Context<AppRoot>) {
    clear_pending_kill(ctx);
}

pub(crate) fn open_session_picker(ctx: &mut Context<AppRoot>) -> Update {
    // Open instantly from local discovery and the last successful remote-host snapshots. The
    // recurring watcher refreshes local state only; remote discovery is explicit in Remote hosts.
    let rows = immediate_picker_rows(ctx);
    let mut picker = SessionPickerState::new(rows);
    if let Some(current_name) = ctx.state.current().session_name.as_deref()
        && let Some(pos) = picker
            .entries
            .iter()
            .position(|entry| entry.name == current_name)
    {
        picker.selected = pos;
    }
    ctx.state.session_picker = Some(picker);
    ctx.state.show_session_picker = true;
    // A new opening invalidates any in-flight watcher tick from a prior opening.
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    request_session_picker_focus(ctx);
    Update::with_command(session_watch_command(
        ctx.state.session_picker_epoch,
        ctx.state.local_current_session_name().map(str::to_string),
    ))
}

/// Open the session picker at startup (nothing attached yet). Sets up the picker state and returns
/// the watcher epoch so `init` can kick off the first discovery tick. Local rows show immediately;
/// cached remote rows are immediate, while local runtime changes arrive through the watcher.
///
/// `highlight` lands the selection on a specific session — what `[session] startup = "last"` uses
/// to point at the session it remembered but could not reopen.
pub(crate) fn open_startup_session_picker(
    ctx: &mut Context<AppRoot>,
    highlight: Option<String>,
) -> u64 {
    let rows = immediate_picker_rows(ctx);
    let mut picker = SessionPickerState::new(rows);
    if let Some(highlight) = highlight
        && let Some(index) = picker
            .entries
            .iter()
            .position(|entry| entry.name == highlight)
    {
        picker.selected = index;
    }
    ctx.state.session_picker = Some(picker);
    ctx.state.show_session_picker = true;
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    ctx.state.commands_dirty = true;
    request_session_picker_focus(ctx);
    ctx.state.session_picker_epoch
}

pub(crate) fn refresh_session_picker(ctx: &mut Context<AppRoot>) -> Update {
    // Carry the typed query and the highlighted row across the rebuild. After a kill the killed row
    // is gone, so clamping keeps the highlight on the row that slid into its place instead of
    // snapping back to the top; it also keeps our `selected` in step with the persistent
    // `SearchPalette` component, which does not re-resolve its keyboard selection when the entry
    // list changes underneath it. Rebuild from fast local rows and let the async sweep refill.
    let (query, selected) = ctx
        .state
        .session_picker
        .as_ref()
        .map(|p| (p.input.text().to_string(), p.selected))
        .unwrap_or_default();
    let rows = immediate_picker_rows(ctx);
    let mut picker = SessionPickerState::new(rows);
    picker.input.set_text(query);
    picker.selected = selected.min(picker.entries.len().saturating_sub(1));
    ctx.state.session_picker = Some(picker);
    Update::with_command(session_watch_command(
        ctx.state.session_picker_epoch,
        ctx.state.local_current_session_name().map(str::to_string),
    ))
}

pub(crate) fn activate_selected_session(ctx: &mut Context<AppRoot>, index: usize) -> Update {
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
    activate_discovered_session(ctx, entry)
}

/// Activate a discovered running session without resolving it through a mutable row index. Picker
/// and sidebar callers keep separate ephemeral-discard confirmations.
pub(crate) fn activate_discovered_session(
    ctx: &mut Context<AppRoot>,
    entry: DiscoveredSession,
) -> Update {
    // The footer omits Enter on the session already in the foreground. Repeating that destination
    // is not an error, and it is not worth a toast.
    if session_row_is_current(&ctx.state, &entry) {
        return Update::none();
    }
    // Discovery already probed this session; an `Unknown` status means the handshake was refused
    // (an incompatible older server is the usual cause). Attaching would only fail after the connect
    // retry deadline, so reject it up front, keep the picker open, and point at the fix - killing
    // the row (Ctrl+K) still works even against a server we can't speak to.
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Unknown
    ) {
        crate::pane::pty_events::notify_error(
            ctx,
            "Attach failed",
            format!(
                "`{}` runs an incompatible version\nCtrl+K removes it",
                entry.name
            ),
        );
        return Update::full();
    }
    // Live rows must not silently recreate a server that died after discovery. A snapshot-only row
    // is deliberately different: selecting it starts the named server so resurrection can restore
    // the session.
    let autostart = matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Restorable
    );
    attach_session_by_name(ctx, entry.name, entry.host, entry.remote_target, autostart)
}

/// Go to this client's **local** scratch session: the global Sessions picker's `Ctrl+T`, and its
/// `Enter` when there is nothing on the list to activate.
///
/// One key covers both directions — start the ephemeral when there is none, switch to it when there
/// already is — because from the keyboard they are the same request. Already being on it is a
/// no-op beyond closing the picker: switching somewhere you already are is not worth a toast.
///
/// Local, deliberately, even while a remote session fills the screen behind the overlay. The global
/// Sessions surface lists every host at once and so commits to none; an action that quietly
/// inherited whichever host happened to be attached would make the same key mean different things
/// on the same screen. A shell on a host is asked for on that host's own surface — `Ctrl+R`, the
/// host, then `Ctrl+T` — which says where it will land before it lands.
pub(crate) fn open_ephemeral_session(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    // Checked before the launcher case: the session on screen being the *local* scratch one settles
    // this whether or not its client is live, and re-attaching what is already attached is never
    // right. A remote scratch session on screen is a different session, and does not answer for it.
    if ctx.state.is_ephemeral_session() && ctx.state.current().remote_target.is_none() {
        return close_session_picker(ctx);
    }
    // In the launcher there is nothing to park, and the panes the launch prepared are still waiting
    // to be handed to the session that starts.
    if ctx.state.needs_session_for_pty() {
        return start_local_launcher_shell(ctx);
    }
    let name = held_ephemeral_session_in(&ctx.state, None)
        .and_then(|attachment| attachment.session_name.clone())
        .unwrap_or_else(crate::state::ephemeral_session_name);
    attach_session_by_name(ctx, name, None, None, true)
}

/// Close the session picker. With a session in the foreground this just returns focus to the
/// current pane; dismissed with nothing attached it leaves the client in the launcher, which is a
/// state the app is allowed to sit in. Dismissing a picker is not a request for a session, so it
/// no longer starts an ephemeral one — the launcher says how to start one.
pub(crate) fn close_session_picker(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    if ctx.state.is_launcher() {
        return Update::full();
    }
    request_current_pane_focus(ctx);
    Update::full()
}

/// Reject the name currently in the session prompt, keeping the prompt open with the reason on it.
///
/// The rule stays inside the prompt rather than in a toast for two reasons: the prompt is modal and
/// a toast would overlap the very field being corrected, and the message is about the text still
/// sitting in that field, so it should disappear when the text does.
fn reject_session_name(ctx: &mut Context<AppRoot>, reason: impl Into<String>) {
    if let Some(rename) = ctx.state.rename_session.as_mut() {
        rename.error = Some(reason.into());
    }
    request_rename_session_focus(ctx);
}

/// Whether `name` is already taken for a create-session submit: live discovery, a held attachment,
/// or a cached remote row. Checked before the create prompt is torn down so a collision stays in
/// the modal instead of toasting over a blank, unfocused client.
pub(crate) fn session_name_already_running(
    ctx: &Context<AppRoot>,
    name: &str,
    remote_target: Option<&crate::session::remote::RemoteTarget>,
) -> bool {
    if ctx
        .state
        .attachment_by_identity(name, remote_target)
        .is_some()
    {
        return true;
    }
    match remote_target {
        None => crate::session::discovery::discover_session(name)
            .ok()
            .flatten()
            .is_some(),
        Some(target) => crate::session::host_sessions_for(&ctx.state.host_session_cache, target)
            .is_some_and(|sessions| sessions.iter().any(|session| session.name == name)),
    }
}

/// Swap whatever overlays are open for a session naming/rename prompt and focus it. Shared by the
/// create-new, rename-in-place, and detach-and-name entry points so they raise the prompt the same
/// way.
fn enter_session_rename(ctx: &mut Context<AppRoot>, rename: SessionRenameState) -> Update {
    ctx.state.rename_session = Some(rename);
    // Raised from the session picker, cancelling returns to it rather than to the pane; the
    // branches of `apply_rename_session` that attach or detach drop the origin instead.
    ctx.state.overlay_return = crate::ops::overlay_return::picker_origin(&ctx.state);
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    crate::ops::session::remotes::dismiss_remote_picker(&mut ctx.state);
    ctx.state.mode = crate::state::Mode::Normal;
    request_rename_session_focus(ctx);
    Update::full()
}

/// Raise the create-session prompt, carrying whatever was typed into the session picker. Reaching
/// `Ctrl+N` from a query that matched nothing means "then make that one", so the name comes along
/// rather than making the user type it a second time.
pub(crate) fn open_create_session(ctx: &mut Context<AppRoot>) -> Update {
    let seed = ctx
        .state
        .session_picker
        .as_ref()
        .filter(|_| ctx.state.show_session_picker)
        .map(|picker| picker.input.text().trim().to_string())
        .unwrap_or_default();
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_create_named(seed))
}

/// Raise the create-session prompt pre-targeted at a remote host ("New session on `<host>`"). The
/// named session is created on that host's server when the name is submitted.
pub(crate) fn open_create_session_on_host(
    ctx: &mut Context<AppRoot>,
    target: crate::session::remote::RemoteTarget,
) -> Update {
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_create_on_host(target))
}

/// Raise the leave prompt on the way out of the client, for the `temporary` temporary sessions
/// leaving would close. A temporary session has no reattachable name, so naming it (Enter) is the
/// only way to keep it: the server is renamed, kept running, and the client leaves. Submitting
/// nothing closes those sessions instead, after a second press confirms it. Cancelling (`Esc`)
/// returns to the session with nothing torn down.
pub(crate) fn open_leave_prompt(ctx: &mut Context<AppRoot>, temporary: usize) -> Update {
    enter_session_rename(ctx, SessionRenameState::for_leave(temporary))
}

/// Open the prompt to rename the *current* session in place. Unlike the picker (which switches to a
/// separate session), this keeps every live pane where it is and just changes the name the server is
/// discoverable under. Works for both ephemeral (naming it for the first time) and already-named
/// sessions.
pub(crate) fn open_rename_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    let ephemeral = ctx.state.is_ephemeral_session();
    let initial = if ephemeral {
        String::new()
    } else {
        ctx.state.current().session_name.clone().unwrap_or_default()
    };
    let mode = if ephemeral {
        NamingMode::NameEphemeralSession
    } else {
        NamingMode::RenameSession
    };
    enter_session_rename(ctx, SessionRenameState::new(initial, mode))
}

fn apply_workspace_name(ctx: &mut Context<AppRoot>, index: usize, name: String) -> Update {
    if let Some(workspace) = ctx.state.current_mut().workspaces.get_mut(index) {
        workspace.name = (!name.is_empty()).then_some(name);
    }
    ctx.state.rename_session = None;
    ctx.state.commands_dirty = true;
    crate::ops::overlay_return::finish(ctx)
}

fn apply_create_session(
    ctx: &mut Context<AppRoot>,
    name: String,
    open_ephemeral: bool,
    host_target: Option<crate::session::remote::RemoteTarget>,
    profile_seed: Option<(String, std::path::PathBuf)>,
) -> Update {
    if !open_ephemeral && !crate::session::discovery::valid_session_name(&name) {
        reject_session_name(ctx, "Use letters, numbers, _ or -");
        return Update::full();
    }
    if !open_ephemeral && session_name_already_running(ctx, &name, host_target.as_ref()) {
        reject_session_name(ctx, format!("Session `{name}` is already running"));
        return Update::full();
    }
    if let Some(target) = host_target {
        ctx.state.rename_session = None;
        crate::ops::overlay_return::leave(ctx);
        let alias = target.display_label();
        return attach_session_by_name(ctx, name, Some(alias), Some(target), true);
    }

    ctx.state.rename_session = None;
    crate::ops::overlay_return::leave(ctx);
    let intent = match profile_seed {
        Some((profile, path)) => {
            crate::ops::profile::OpenNamedIntent::CreateFromProfile { profile, path }
        }
        None => crate::ops::profile::OpenNamedIntent::CreateFresh,
    };
    if open_ephemeral {
        let crate::ops::profile::OpenNamedIntent::CreateFromProfile { profile, path } = intent
        else {
            return Update::none();
        };
        return crate::ops::profile::load_profile_into_fresh_ephemeral(
            ctx,
            crate::config::ProfileEntry {
                name: profile,
                path,
            },
        );
    }
    crate::ops::profile::open_named_target(ctx, name, intent)
}

fn confirm_empty_ephemeral_leave(
    ctx: &mut Context<AppRoot>,
    leave: crate::state::LeaveIntent,
) -> Update {
    if ctx.state.config.confirm.quit_ephemeral && !leave.armed {
        if let Some(rename) = ctx.state.rename_session.as_mut() {
            rename.leave = Some(crate::state::LeaveIntent {
                armed: true,
                ..leave
            });
        }
        request_rename_session_focus(ctx);
        return Update::full();
    }
    ctx.state.rename_session = None;
    crate::ops::overlay_return::leave(ctx);
    crate::ops::exit::leave_client_now(ctx, true)
}

fn apply_session_name(
    ctx: &mut Context<AppRoot>,
    name: String,
    leave: Option<crate::state::LeaveIntent>,
) -> Update {
    if name.is_empty()
        && let Some(leave) = leave
    {
        return confirm_empty_ephemeral_leave(ctx, leave);
    }
    if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
        reject_session_name(ctx, "Use letters, numbers, _ or -");
        return Update::full();
    }
    if session_name_already_running(ctx, &name, ctx.state.current().remote_target.as_ref())
        && ctx.state.current().session_name.as_deref() != Some(name.as_str())
    {
        reject_session_name(ctx, format!("Session `{name}` is already running"));
        return Update::full();
    }

    ctx.state.rename_session = None;
    if leave.is_some() {
        crate::ops::overlay_return::leave(ctx);
        let Some(client) = ctx.state.current().session_client.clone() else {
            crate::pane::pty_events::notify_error(ctx, "Rename failed", "Session connection lost");
            return Update::full();
        };
        crate::ops::session::flush_layout_commit(ctx);
        client.rename(name.clone());
        ctx.state.current_mut().session_name = Some(name);
        return crate::ops::exit::leave_client(ctx);
    }
    if ctx.state.current().session_name.as_deref() == Some(name.as_str()) {
        return crate::ops::overlay_return::finish(ctx);
    }
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.rename(name);
    }
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn apply_rename_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(rename_state) = ctx.state.rename_session.as_ref() else {
        return Update::none();
    };
    let name = rename_state.input.text().trim().to_string();
    let mode = rename_state.mode;
    let leave = rename_state.leave;
    let host_target = rename_state.host_target.clone();
    let profile_seed = rename_state.profile_seed.clone();

    match mode {
        NamingMode::RenameWorkspace { index } => apply_workspace_name(ctx, index, name),
        NamingMode::CreateSession | NamingMode::OpenProfileAs => {
            let open_ephemeral = mode == NamingMode::OpenProfileAs && name.is_empty();
            apply_create_session(ctx, name, open_ephemeral, host_target, profile_seed)
        }
        NamingMode::NameEphemeralSession => apply_session_name(ctx, name, leave),
        NamingMode::RenameSession => apply_session_name(ctx, name, None),
    }
}

/// Open this client's temporary remote session on an explicitly selected host. The picker target
/// is authoritative even when a different remote attachment is visible behind the overlay.
pub(crate) fn open_ephemeral_session_on_host(
    ctx: &mut Context<AppRoot>,
    target: crate::session::remote::RemoteTarget,
) -> Update {
    crate::ops::overlay_return::leave(ctx);
    let name = crate::state::remote_ephemeral_session_name();
    attach_session_by_name(ctx, name, Some(target.display_label()), Some(target), true)
}

pub(crate) fn close_rename_session(ctx: &mut Context<AppRoot>) -> Update {
    // Cancelling any session naming prompt - including the detach-and-name one - just returns to the
    // session. A detach never tears panes down: quitting (with its own confirmation) is the only
    // path that shuts an ephemeral server down.
    ctx.state.rename_session = None;
    ctx.state.commands_dirty = true;
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn kill_selected_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    let armed = picker.pending_kill == Some(index);
    if !armed {
        // First press arms the kill: drop any stale arming (kill or restart), then mark this row.
        clear_pending_session_arms(ctx);
        if let Some(picker) = ctx.state.session_picker.as_mut() {
            picker.pending_kill = Some(index);
        }
        return crate::ops::confirm::arm(ctx);
    }
    clear_pending_session_arms(ctx);
    let killed = kill_discovered_session(ctx, entry);
    // Keep the picker open with the killed row gone and selection clamped; only close when the
    // list (and every other meaningful candidate) is empty.
    if ctx.state.show_session_picker {
        return refresh_picker_after_kill(ctx);
    }
    killed
}

pub(crate) fn restart_selected_session(ctx: &mut Context<AppRoot>) -> Update {
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    if !session_row_can_restart(&entry) {
        return Update::none();
    }
    let armed = picker.pending_restart == Some(index);
    if !armed {
        clear_pending_session_arms(ctx);
        if let Some(picker) = ctx.state.session_picker.as_mut() {
            picker.pending_restart = Some(index);
        }
        return crate::ops::confirm::arm(ctx);
    }
    clear_pending_session_arms(ctx);
    restart_discovered_session(ctx, entry)
}

/// Restart a discovered session: shut its server down and immediately recreate it as the active
/// session. Distinct from kill (sessionless landing) and from disconnect (server keeps running).
fn detach_parked_instance(
    ctx: &mut Context<AppRoot>,
    name: &str,
    remote_target: Option<&crate::session::remote::RemoteTarget>,
) {
    if let Some(id) = ctx.state.parked_attachment_id(name, remote_target)
        && let Some(attachment) = ctx.state.background.remove(&id)
        && let Some(client) = attachment.session_client.as_ref()
    {
        client.detach();
    }
}

fn restart_session_name(state: &mut crate::state::State, entry: &DiscoveredSession) -> String {
    if !entry.ephemeral {
        return entry.name.clone();
    }
    if entry.remote_target.is_some() {
        crate::state::remote_ephemeral_session_name()
    } else {
        crate::state::fresh_ephemeral_session_name(state.mint_attachment_id())
    }
}

pub(crate) fn restart_discovered_session(
    ctx: &mut Context<AppRoot>,
    entry: DiscoveredSession,
) -> Update {
    if matches!(
        &entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Restorable
    ) {
        // A snapshot has no live server to recreate. Restore is Enter; restart is omitted.
        return Update::none();
    }
    let is_current = ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target;
    if is_current {
        return restart_current_session(ctx);
    }
    detach_parked_instance(ctx, &entry.name, entry.remote_target.as_ref());
    let remote_config = ctx.state.config.remote.clone();
    if let Err(err) = shutdown_discovered_session(&entry, &remote_config) {
        crate::pane::pty_events::notify_error(ctx, "Restart failed", err.to_string());
        return Update::full();
    }
    if let Some(target) = entry.remote_target.as_ref() {
        remove_cached_remote_session(ctx, &entry.name, target);
    }
    let restart_name = restart_session_name(&mut ctx.state, &entry);
    // Recreate and make it active immediately. The picker giving way to the restarted session is
    // the confirmation, so a success toast would only repeat the visible state change.
    attach_session_by_name(
        ctx,
        restart_name,
        entry.host.clone(),
        entry.remote_target.clone(),
        true,
    )
}

/// Kill a discovered session outright: shut its server down, so its PTYs die with it.
///
/// Killing the session you're attached to is fine — the UI stays up and lands on the picker or
/// launcher rather than quitting. Shared by the session picker's `Ctrl+K` and the Sessions
/// sidebar's ✕, which mean the same thing and must not drift apart.
pub(crate) fn kill_discovered_session(
    ctx: &mut Context<AppRoot>,
    entry: DiscoveredSession,
) -> Update {
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target
    {
        return kill_current_session(ctx);
    }
    let remote_config = ctx.state.config.remote.clone();
    match shutdown_discovered_session(&entry, &remote_config) {
        Ok(()) => {
            // Drop any parked client attachment for the killed server so it cannot linger offline.
            detach_parked_instance(ctx, &entry.name, entry.remote_target.as_ref());
            // Drop the row now rather than waiting for the next sweep to notice, and bump the epoch
            // so the sweep re-runs against the server that is gone.
            ctx.state.sidebar.sessions.retain(|listed| {
                listed.name != entry.name || listed.remote_target != entry.remote_target
            });
            ctx.state.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
            if let Some(target) = entry.remote_target.as_ref() {
                remove_cached_remote_session(ctx, &entry.name, target);
            }
            // The row the user acted on vanished from the list above: that *is* the confirmation.
            Update::full()
        }
        Err(err) => {
            crate::pane::pty_events::notify_error(ctx, "Kill failed", err.to_string());
            Update::full()
        }
    }
}

pub(crate) fn remove_cached_remote_session(
    ctx: &mut Context<AppRoot>,
    session_name: &str,
    target: &crate::session::remote::RemoteTarget,
) {
    let Some(mut sessions) =
        crate::session::host_sessions_for(&ctx.state.host_session_cache, target)
            .map(|sessions| sessions.to_vec())
    else {
        return;
    };
    let old_len = sessions.len();
    sessions.retain(|session| session.name != session_name);
    if sessions.len() != old_len {
        crate::session::record_host_sessions(target, sessions.clone());
        crate::session::set_cached_host_sessions(
            &mut ctx.state.host_session_cache,
            target,
            sessions,
        );
    }
}

/// Disconnect this client's attachment for the selected session, leaving its server running.
/// Targets a session retained in the background: its client connection is dropped and the
/// attachment is discarded, but the server (and any other clients) keep going. The current session
/// is left alone — disconnecting it is Kill (`Ctrl+K`) or leaving the client — and a merely-running
/// session we do not hold an attachment to has nothing to disconnect.
pub(crate) fn disconnect_selected_attachment(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    disconnect_discovered_attachment(ctx, entry)
}

pub(crate) fn disconnect_discovered_attachment(
    ctx: &mut Context<AppRoot>,
    entry: DiscoveredSession,
) -> Update {
    if !session_row_can_disconnect(&ctx.state, &entry) {
        return Update::none();
    }
    let Some(id) = ctx
        .state
        .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
    else {
        return Update::none();
    };
    if let Some(attachment) = ctx.state.background.remove(&id)
        && let Some(client) = attachment.session_client.as_ref()
    {
        client.detach();
    }
    if ctx.state.show_session_picker {
        refresh_session_picker(ctx)
    } else {
        Update::full()
    }
}

/// Disconnect the client from a remote host: close every attachment (current and retained) to the
/// selected row's host, leaving the remote servers running. A host-wide sibling of
/// [`disconnect_selected_attachment`]; if the current session lives on that host the UI lands on the
/// session picker or launcher. Non-destructive - the remote sessions can be reattached later.
pub(crate) fn disconnect_selected_host(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    if !session_row_can_disconnect_host(&ctx.state, &entry) {
        return Update::none();
    }
    let Some(target) = entry.remote_target.clone() else {
        return Update::none();
    };
    disconnect_host(ctx, &target)
}

pub(crate) fn shutdown_discovered_session(
    entry: &DiscoveredSession,
    remote_config: &crate::config::RemoteConfig,
) -> std::io::Result<()> {
    if let Some(target) = &entry.remote_target {
        return crate::session::remote::kill_remote_session(target, &entry.name, remote_config)
            .map_err(std::io::Error::other);
    }
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Restorable
    ) {
        return crate::session::server::delete_snapshot(&entry.name);
    }
    shutdown_session(&entry.name)
}

pub(crate) fn shutdown_session(name: &str) -> std::io::Result<()> {
    crate::session::server::shutdown_named_session(name)
}
