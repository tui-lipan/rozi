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
    // Open instantly with local rows only; configured remote hosts are queried over ssh, which
    // costs a round-trip when up and the full connect timeout when down. Blocking the open on that
    // froze the UI every time. The remote rows stream in via `SessionsDiscovered` below.
    let rows = local_picker_rows(ctx);
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
    Update::with_command(session_discover_now_command(
        ctx.state.session_picker_epoch,
        ctx.state.current().session_name.clone(),
        ctx.state.config.remote.clone(),
    ))
}

/// Open the session picker at startup (nothing attached yet). Sets up the picker state and returns
/// the watcher epoch so `init` can kick off the first discovery tick. Local rows show immediately;
/// remote rows arrive async, so a dead configured host never stalls startup.
pub(crate) fn open_startup_session_picker(ctx: &mut Context<HyprmuxApp>) -> u64 {
    let rows = local_picker_rows(ctx);
    ctx.state.session_picker = Some(SessionPickerState::new(rows));
    ctx.state.show_session_picker = true;
    ctx.state.session_picker_epoch = ctx.state.session_picker_epoch.wrapping_add(1);
    ctx.state.commands_dirty = true;
    request_session_picker_focus(ctx);
    ctx.state.session_picker_epoch
}

pub(crate) fn refresh_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
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
    let rows = local_picker_rows(ctx);
    let mut picker = SessionPickerState::new(rows);
    picker.input.set_text(query);
    picker.selected = selected.min(picker.entries.len().saturating_sub(1));
    ctx.state.session_picker = Some(picker);
    Update::with_command(session_discover_now_command(
        ctx.state.session_picker_epoch,
        ctx.state.current().session_name.clone(),
        ctx.state.config.remote.clone(),
    ))
}

/// Fast, local-only picker rows for an immediate open: local named sessions plus the attached
/// session, with no remote ssh. The full list (configured remote hosts included) arrives async via
/// [`Msg::SessionsDiscovered`].
pub(crate) fn local_picker_rows(ctx: &Context<HyprmuxApp>) -> Vec<DiscoveredSession> {
    let current_name = ctx.state.current().session_name.as_deref();
    let mut rows =
        crate::session::discovery::discover_selectable_sessions(current_name).unwrap_or_default();
    push_current_session_row(ctx, &mut rows);
    rows
}

/// Run the full picker discovery (including configured remote hosts) once, off the UI thread, and
/// deliver it as [`Msg::SessionsDiscovered`]. Used to populate remote rows right after an
/// instant local-only open, without waiting a full refresh interval.
fn session_discover_now_command(
    epoch: u64,
    current_name: Option<String>,
    remote_config: crate::config::HyprmuxRemoteConfig,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        if let Ok(rows) = discover_picker_sessions(current_name.as_deref(), &remote_config) {
            link.send(Msg::SessionsDiscovered { epoch, rows });
        }
    })
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
    Update::with_command(session_watch_command(
        epoch,
        ctx.state.current().session_name.clone(),
        ctx.state.config.remote.clone(),
    ))
}

fn session_watch_command(
    epoch: u64,
    current_name: Option<String>,
    remote_config: crate::config::HyprmuxRemoteConfig,
) -> Command {
    // Recurring watch: see `profile_session_watch_command` -- the wait belongs on the timer
    // thread, not on a pooled worker held open for the life of the picker.
    Command::after(
        SESSION_PICKER_REFRESH_INTERVAL,
        move |link: CommandLink<Msg>| {
            // Discovery runs here (off the UI thread); a failed sweep simply skips this tick and lets
            // the loop stop rather than clobbering the last good list.
            if let Ok(rows) = discover_picker_sessions(current_name.as_deref(), &remote_config) {
                link.send(Msg::SessionsDiscovered { epoch, rows });
            }
        },
    )
}

/// Discover sessions for the picker: exclude the currently attached one (re-added separately by
/// [`push_current_session_row`]) and drop *foreign* ephemeral sessions. Ephemeral sessions are
/// per-process, disposable, and self-reaping, and their `eph-…` names are reserved; another
/// process's ephemeral has no business being a selectable row (attaching would fight its owner over
/// teardown), so it is filtered out here. Our own ephemeral still appears via the current row.
///
/// Configured `[remote.hosts.*]` aliases are probed in parallel (failures are skipped so one
/// unreachable host never blocks the picker).
fn discover_picker_sessions(
    current_name: Option<&str>,
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = crate::session::discovery::discover_selectable_sessions(current_name)?;
    rows.extend(discover_configured_remote_sessions(remote_config));
    rows.sort_by(|a, b| match (a.host.as_deref(), b.host.as_deref()) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (a_host, b_host) => a_host.cmp(&b_host).then_with(|| a.name.cmp(&b.name)),
    });
    Ok(rows)
}

fn discover_configured_remote_sessions(
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> Vec<DiscoveredSession> {
    let mut hosts: Vec<String> = remote_config.hosts.keys().cloned().collect();
    if let Some(default_host) = &remote_config.default_host
        && !hosts.iter().any(|h| h == default_host)
    {
        hosts.push(default_host.clone());
    }
    hosts.sort();
    hosts.dedup();

    let mut handles = Vec::with_capacity(hosts.len());
    for alias in hosts {
        let config = remote_config.clone();
        handles.push(std::thread::spawn(move || {
            let Ok(target) = crate::session::remote::parse_remote_target(&alias) else {
                return Vec::new();
            };
            crate::session::discovery::discover_sessions_from(
                &crate::session::discovery::SessionSource::Remote(target),
                &config,
            )
            .unwrap_or_default()
        }));
    }
    let mut rows = Vec::new();
    for handle in handles {
        if let Ok(mut remote_rows) = handle.join() {
            rows.append(&mut remote_rows);
        }
    }
    rows
}

/// Local + configured-remote discovery used by the picker and sidebar (off the UI thread).
pub(crate) fn discover_sessions_for_ui(
    current_name: Option<&str>,
    remote_config: &crate::config::HyprmuxRemoteConfig,
    current: Option<DiscoveredSession>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = discover_picker_sessions(current_name, remote_config)?;
    if let Some(current) = current {
        merge_current_session_row(&mut rows, current);
        rows.sort_by(|a, b| match (a.host.as_deref(), b.host.as_deref()) {
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (a_host, b_host) => a_host.cmp(&b_host).then_with(|| a.name.cmp(&b.name)),
        });
    }
    Ok(rows)
}

/// Add the attached session to `rows` unless it is already listed. Remote discovery returns the
/// attached session (the current-session exclusion only applies to the *local* scan), so both the
/// sidebar and the picker would otherwise show two `name@host • current` rows under `--remote`.
fn merge_current_session_row(rows: &mut Vec<DiscoveredSession>, current: DiscoveredSession) {
    let already = rows
        .iter()
        .any(|row| row.name == current.name && row.host == current.host);
    if !already {
        rows.push(current);
    }
}

/// Append a row for the attached session (discovery excludes it) and keep the list sorted.
fn push_current_session_row(ctx: &Context<HyprmuxApp>, rows: &mut Vec<DiscoveredSession>) {
    if let Some(current) = current_session_row(&ctx.state) {
        merge_current_session_row(rows, current);
        rows.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

pub(crate) fn current_session_row(state: &crate::state::State) -> Option<DiscoveredSession> {
    let name = state.current().session_name.clone()?;
    Some(DiscoveredSession {
        name,
        ephemeral: state.is_ephemeral_session(),
        host: state.current().remote_host.clone(),
        status: crate::session::discovery::DiscoveredSessionStatus::Running {
            panes: state.workspaces.iter().map(|w| w.panes.len()).sum(),
            has_layout: true,
            clients: state.attached_client_count(),
            created_from_profile: state.current().created_from_profile.clone(),
        },
    })
}

fn require_attached(ctx: &mut Context<HyprmuxApp>) -> Option<()> {
    if ctx.state.current().session_attached {
        Some(())
    } else {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
        None
    }
}

fn require_writable(ctx: &mut Context<HyprmuxApp>) -> Option<()> {
    require_attached(ctx)?;
    let Some(shared) = ctx.state.current().shared.as_ref() else {
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
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.controller)
        .map(|id| format!("client {id}"))
        .unwrap_or_else(|| "another client".to_string());
    // Advertise the live request binding so the hint tracks any `[keys]` override.
    let how = crate::commands::command_prefix_chord(ctx, "request-control")
        .map(|chord| format!("{chord} to request control"))
        .unwrap_or_else(|| "Try requesting control".to_string());
    crate::pty_events::replace_toast(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        info_toast(
            &ctx.state.theme,
            format!("Layout controlled by {who}\n{how}"),
        ),
    );
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
    let shared = ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked");
    let already_requested = shared
        .clients
        .iter()
        .any(|client| client.id == shared.client_id && client.requesting_control);
    let controller_label = shared
        .controller
        .and_then(|id| shared.clients.iter().find(|client| client.id == id))
        .map(|client| format!("{} #{}", client.label, client.id));
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.request_control();
    }
    let message = match (already_requested, controller_label) {
        (true, Some(who)) => format!("Still waiting on {who} for layout control"),
        (true, None) => "Control request already pending".to_string(),
        (false, Some(who)) => format!("Requested layout control from {who}"),
        (false, None) => "Requested layout control".to_string(),
    };
    crate::pty_events::replace_toast(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        info_toast(&ctx.state.theme, message),
    );
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
    let shared = ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked");
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.set_input_lock(!shared.input_locked);
    }
    Update::full()
}

pub(crate) fn grant_control(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
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
        && let Some(client) = ctx.state.current().session_client.as_ref()
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
    let Some(shared) = ctx.state.current().shared.as_ref() else {
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
            if let Some(client) = ctx.state.current().session_client.as_ref() {
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
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.requesting_control
        && target.id != shared.client_id
        && let Some(client) = ctx.state.current().session_client.as_ref()
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
    crate::update::sidebar::invalidate_sessions(ctx);
    crate::popup::kill_if_open(ctx);
    crate::update::flush_layout_commit(ctx);
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

pub(crate) fn may_shutdown_ephemeral(state: &crate::state::State) -> bool {
    state.is_ephemeral_session()
        && state.is_controller()
        && state.attached_client_count() == 1
        && state
            .current()
            .shared
            .as_ref()
            .is_none_or(|shared| !shared.read_only)
}

pub(crate) fn swap_state_for_attach(
    ctx: &mut Context<HyprmuxApp>,
    mut replacement: crate::state::State,
) {
    replacement.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
    replacement.sidebar.command_epoch = ctx.state.sidebar.command_epoch.wrapping_add(1);
    replacement.sidebar.config_epoch = ctx.state.sidebar.config_epoch.wrapping_add(1);
    replacement.theme_watcher = ctx.state.theme_watcher.take();
    replacement.system_theme = ctx.state.system_theme.clone();
    replacement.control_socket_path = ctx.state.control_socket_path.clone();
    replacement.command_link = ctx.state.command_link.clone();
    replacement.event_hub = ctx.state.event_hub.clone();
    replacement.runtime_epoch = ctx.state.runtime_epoch;
    ctx.state = replacement;
    ctx.state.commands_dirty = true;
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
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
    crate::update::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
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
    let old_epoch = ctx.state.runtime_epoch;
    let epoch = old_epoch.saturating_add(1);
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    let fresh = crate::state::State::new(config, theme);
    swap_state_for_attach(ctx, fresh);
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        remote_host: None,
        intent: crate::state::AttachIntent::Plain,
        left: None,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

pub(crate) fn attach_session_by_name(
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    remote_host: Option<String>,
    autostart: bool,
) -> Update {
    if !crate::session::discovery::valid_attach_target(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Invalid session name",
            "Use letters, numbers, _ or -",
        ));
        return Update::full();
    }
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(name.as_str())
        && ctx.state.current().remote_host == remote_host
    {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Already attached to `{name}`"),
        ));
        return Update::full();
    }
    if ctx.state.current().pending_session_attach.is_some() {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Attach already in progress"));
        return Update::full();
    }
    // Resolve the remote target before tearing down the current session, so a malformed host does
    // not strand a working attachment — and so the resolved target can be carried on `State` for a
    // reconnect to route on (rather than re-parsing).
    let remote_target = match remote_host.as_deref() {
        Some(host) => match crate::session::remote::parse_remote_target(host) {
            Ok(target) => Some(target),
            Err(err) => {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Invalid remote host",
                    format!("`{host}`: {err}"),
                ));
                return Update::full();
            }
        },
        None => None,
    };
    // Attach-elsewhere: release the current session (a named one is parked for reattach; an
    // ephemeral one is torn down so it does not leak an orphan server), then attach to the target.
    let left =
        ctx.state
            .current()
            .session_name
            .clone()
            .map(|left_name| crate::state::LeftSession {
                name: left_name,
                was_ephemeral_shutdown: may_shutdown_ephemeral(&ctx.state),
            });
    release_current_session(ctx);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    let epoch = ctx.state.runtime_epoch.saturating_add(1);
    ctx.state.current_mut().remote_host = remote_host.clone();
    ctx.state.current_mut().remote_target = remote_target.clone();
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart,
        read_only: false,
        remote_host: remote_host.clone(),
        intent: crate::state::AttachIntent::Plain,
        left,
    });
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
    activate_discovered_session(ctx, entry, SessionActivationSource::Picker(index))
}

#[derive(Clone, Copy)]
pub(crate) enum SessionActivationSource {
    Picker(usize),
    Sidebar,
}

/// Activate a discovered running session without resolving it through a mutable row index. Picker
/// and sidebar callers keep separate ephemeral-discard confirmations.
pub(crate) fn activate_discovered_session(
    ctx: &mut Context<HyprmuxApp>,
    entry: DiscoveredSession,
    source: SessionActivationSource,
) -> Update {
    // Discovery already probed this session; an `Unknown` status means the handshake was refused
    // (an incompatible older server is the usual cause). Attaching would only fail after the connect
    // retry deadline, so reject it up front, keep the picker open, and point at the fix - killing
    // the row (Ctrl+K) still works even against a server we can't speak to.
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Unknown
    ) {
        match source {
            SessionActivationSource::Picker(_) => clear_pending_open(ctx),
            SessionActivationSource::Sidebar => {
                ctx.state.sidebar.pending_session_open = None;
            }
        }
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Attach failed",
            format!(
                "`{}` runs an incompatible version\nCtrl+K removes it",
                entry.name
            ),
        ));
        return Update::full();
    }
    // Attaching elsewhere while on a disposable ephemeral session shuts that server down and kills
    // its panes (see `release_current_session`). That is easy to trigger by reflex from the picker,
    // so guard it with the same two-press confirmation as a kill: the first Enter arms and warns,
    // the second commits. Switching between two named sessions parks the old one and needs no guard.
    let discards_ephemeral = ctx.state.current().session_attached
        && ctx.state.is_ephemeral_session()
        && ctx.state.current().session_name.as_deref() != Some(entry.name.as_str());
    if discards_ephemeral {
        let armed = match source {
            SessionActivationSource::Picker(index) => ctx
                .state
                .session_picker
                .as_ref()
                .is_some_and(|picker| picker.pending_open == Some(index)),
            SessionActivationSource::Sidebar => {
                ctx.state.sidebar.pending_session_open.as_deref() == Some(entry.name.as_str())
            }
        };
        if !armed {
            // Arm the confirmation: the target row renders the warning-colored "⏎ again - ends temp
            // session" cue, so a second Enter is required and no toast is needed.
            match source {
                SessionActivationSource::Picker(index) => {
                    if let Some(picker) = ctx.state.session_picker.as_mut() {
                        picker.pending_open = Some(index);
                    }
                }
                SessionActivationSource::Sidebar => {
                    ctx.state.sidebar.pending_session_open = Some(entry.name);
                }
            }
            return Update::full();
        }
    }
    match source {
        SessionActivationSource::Picker(_) => clear_pending_open(ctx),
        SessionActivationSource::Sidebar => ctx.state.sidebar.pending_session_open = None,
    }
    // A session shown in the picker is already running, so don't autostart a replacement if it
    // died between discovery and attach.
    attach_session_by_name(ctx, entry.name, entry.host, false)
}

/// Attach the current (initial or restored-profile) state to this process's ephemeral session.
/// Used when the startup picker's "new ephemeral" row is chosen or the picker is dismissed with no
/// session attached, so a launch always ends with a working terminal.
pub(crate) fn attach_startup_ephemeral(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    // This is a *local* fallback; clear any remote target left over from a failed `--remote` attach
    // so panes resolve their shell/cwd locally and the sidebar does not keep probing a dead host.
    ctx.state.current_mut().remote_host = None;
    ctx.state.current_mut().remote_target = None;
    let epoch = ctx.state.runtime_epoch;
    let name = crate::state::ephemeral_session_name();
    let intent = ctx
        .state
        .current_mut()
        .deferred_profile_seed
        .take()
        .map_or(crate::state::AttachIntent::Plain, |(profile, path)| {
            crate::state::AttachIntent::ProfileSeed { profile, path }
        });
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        remote_host: None,
        intent,
        left: None,
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
    if !ctx.state.current().session_attached
        && ctx.state.current().session_client.is_none()
        && ctx.state.current().pending_session_attach.is_none()
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
        ctx.state.current().session_name.clone().unwrap_or_default()
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
        NamingMode::CreateSession | NamingMode::OpenProfileAs => {
            let open_ephemeral = rename_state.mode == NamingMode::OpenProfileAs && name.is_empty();
            if !open_ephemeral && !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Invalid session name",
                    "Use letters, numbers, _ or -",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            // Creating a session from an ephemeral one discards that disposable session. Guard it
            // with a two-press confirm shown *in the modal* (red border + inline note) rather than a
            // toast: the first Enter arms, a second commits. Editing the name clears the arm (see
            // `Msg::RenameSessionChanged`).
            let needs_confirm =
                ctx.state.current().session_attached && ctx.state.is_ephemeral_session();
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
            let profile_seed = ctx
                .state
                .rename_session
                .as_ref()
                .and_then(|rename| rename.profile_seed.clone());
            ctx.state.rename_session = None;
            let intent = match profile_seed {
                Some((profile, path)) => {
                    crate::ops::profile::OpenNamedIntent::CreateFromProfile { profile, path }
                }
                None => crate::ops::profile::OpenNamedIntent::CreateFresh,
            };
            if open_ephemeral {
                let crate::ops::profile::OpenNamedIntent::CreateFromProfile { profile, path } =
                    intent
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
        NamingMode::NameEphemeralSession => {
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Invalid session name",
                    "Use letters, numbers, _ or -",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            let detach_after = rename_state.detach_after;
            ctx.state.rename_session = None;

            if detach_after {
                let Some(client) = ctx.state.current().session_client.clone() else {
                    ctx.toast().push(crate::pty_events::error_toast(
                        &ctx.state.theme,
                        "Rename failed",
                        "Session connection lost",
                    ));
                    return Update::full();
                };
                crate::update::flush_layout_commit(ctx);
                crate::ops::exit::mark_session_detached(ctx, Some(&name));
                client.rename(name);
                client.detach();
                crate::profiles::persist_session_on_detach(&ctx.state);
                ctx.quit();
                return Update::none();
            }

            if ctx.state.current().session_name.as_deref() == Some(name.as_str()) {
                request_current_pane_focus(ctx);
                return Update::full();
            }

            if let Some(client) = ctx.state.current().session_client.clone() {
                client.rename(name);
            }
            request_current_pane_focus(ctx);
            Update::full()
        }
        NamingMode::RenameSession => {
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Invalid session name",
                    "Use letters, numbers, _ or -",
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }

            ctx.state.rename_session = None;

            if ctx.state.current().session_name.as_deref() == Some(name.as_str()) {
                request_current_pane_focus(ctx);
                return Update::full();
            }

            if let Some(client) = ctx.state.current().session_client.clone() {
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
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_host == entry.host
    {
        return kill_current_session(ctx, display);
    }
    let remote_config = ctx.state.config.remote.clone();
    match shutdown_discovered_session(&entry, &remote_config) {
        // The refreshed picker shows the row gone; that is the confirmation.
        Ok(()) => refresh_session_picker(ctx),
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Kill failed",
                err.to_string(),
            ));
            Update::full()
        }
    }
}

fn shutdown_discovered_session(
    entry: &DiscoveredSession,
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> std::io::Result<()> {
    if let Some(host) = &entry.host {
        let target =
            crate::session::remote::parse_remote_target(host).map_err(std::io::Error::other)?;
        return crate::session::remote::kill_remote_session(&target, &entry.name, remote_config)
            .map_err(std::io::Error::other);
    }
    shutdown_session(&entry.name)
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
        //
        // Piped/remote connections always report `peer_pid() == None`, so this fallback is unreachable
        // for `--remote` attaches — we must never `terminate_server` a local ssh pid.
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
    use crate::session::protocol::{
        ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
    };
    crate::session::protocol::write_frame(
        stream,
        &ClientMessage::Attach {
            session: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
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

    fn session_row(name: &str, host: Option<&str>) -> DiscoveredSession {
        DiscoveredSession {
            name: name.to_string(),
            ephemeral: false,
            host: host.map(str::to_string),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: 1,
                has_layout: true,
                clients: 1,
                created_from_profile: None,
            },
        }
    }

    /// Under `--remote` the discovery scan already returns the attached session, so merging the
    /// current-session row must not add a second copy — otherwise the picker shows two
    /// `name@host • current` entries. A same-name row on a *different* host is a real distinct
    /// session and must stay.
    #[test]
    fn merge_current_session_row_dedupes_by_name_and_host() {
        let mut rows = vec![session_row("dev", Some("winvm"))];
        merge_current_session_row(&mut rows, session_row("dev", Some("winvm")));
        assert_eq!(
            rows.len(),
            1,
            "the attached session must not be listed twice"
        );

        // Same name, different host: a genuinely different session, kept.
        merge_current_session_row(&mut rows, session_row("dev", Some("other")));
        assert_eq!(rows.len(), 2);

        // Not present yet: added.
        let mut empty = Vec::new();
        merge_current_session_row(&mut empty, session_row("dev", Some("winvm")));
        assert_eq!(empty.len(), 1);
    }

    fn ephemeral_state(client_id: u64, controller: u64, clients: Vec<ClientInfo>) -> State {
        let mut state = State::new(HyprmuxConfig::default(), ThemePreset::Lipan.theme());
        state.current_mut().session_name = Some("eph-test".to_string());
        state.current_mut().session_attached = true;
        let mut shared = SharedSessionState::new(client_id);
        shared.controller = Some(controller);
        shared.clients = clients;
        state.current_mut().shared = Some(shared);
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
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
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
                    state.current_mut().shared = Some(shared);
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
    fn create_session_starts_fresh_instead_of_carrying_current_panes() {
        use crate::HyprmuxApp;
        use crate::Msg;
        use crate::state::SessionRenameState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                // A profile name unlikely to exist on disk, so resolution must fall through to a
                // fresh empty session rather than a profile seed.
                let name = format!("create-fresh-{}", std::process::id());
                {
                    let state = backend.state_mut();
                    state.current_mut().session_name = Some("eph-test".to_string());
                    state.current_mut().session_attached = true;
                    state.current_mut().pending_session_attach = None;
                    state.sidebar.command_epoch = 7;
                    state.sidebar.config_epoch = 11;
                    // Simulate a profile-seeded session: the current pane carries a command.
                    state.workspaces[0].panes[0].identity.command = Some("nvim".to_string());
                    let mut rename = SessionRenameState::new(&name, NamingMode::CreateSession);
                    rename.pending_confirm = true;
                    state.rename_session = Some(rename);
                }
                backend.render();
                backend
                    .dispatch(Msg::SubmitRenameSession)
                    .expect("dispatch create session");

                let state = backend.state();
                let pending = state
                    .current()
                    .pending_session_attach
                    .as_ref()
                    .expect("create queues an attach");
                assert_eq!(pending.name, name);
                assert_eq!(pending.intent, crate::state::AttachIntent::Plain);
                assert_eq!(
                    pending.left.as_ref().map(|left| left.name.as_str()),
                    Some("eph-test")
                );
                // The new session must not inherit the current layout: the swapped state is a
                // fresh single-pane default with no launch command to respawn.
                assert_eq!(state.workspaces[0].panes.len(), 1);
                assert_eq!(state.workspaces[0].panes[0].identity.command, None);
                assert_eq!(state.sidebar.command_epoch, 8);
                assert_eq!(state.sidebar.config_epoch, 12);
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
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
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
                    state.current_mut().shared = Some(shared);
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
