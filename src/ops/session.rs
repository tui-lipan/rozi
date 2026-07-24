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

/// Clear the session-picker kill confirmation when navigation abandons its armed row.
pub(crate) fn clear_pending_session_arms(ctx: &mut Context<HyprmuxApp>) {
    clear_pending_kill(ctx);
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
    push_attached_session_rows(ctx, &mut rows);
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
    push_attached_session_rows(ctx, &mut rows);
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.entries = rows;
        picker.selected = picker.selected.min(picker.entries.len().saturating_sub(1));
    }
    let armed_out_of_range = ctx.state.session_picker.as_ref().is_some_and(|picker| {
        picker
            .pending_kill
            .is_some_and(|index| index >= picker.entries.len())
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
/// [`push_attached_session_rows`]) and drop *foreign* ephemeral sessions. Ephemeral sessions are
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
    sort_session_rows(&mut rows);
    Ok(rows)
}

/// Per-probed-host outcome carried back from a sidebar sweep: `None` cleared the host's error,
/// `Some(msg)` records why its probe failed so it can be surfaced inline.
pub(crate) type HostProbeStatus = Vec<(crate::session::remote::RemoteTarget, Option<String>)>;

/// The result of a sidebar discovery sweep: the session rows (or a local-discovery error) plus the
/// per-host probe outcomes.
type SidebarDiscovery = (std::io::Result<Vec<DiscoveredSession>>, HostProbeStatus);

/// Every configured remote target: `[remote.hosts.*]` aliases plus `default_host`.
fn configured_targets(
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> Vec<crate::session::remote::RemoteTarget> {
    let mut hosts: Vec<String> = remote_config.hosts.keys().cloned().collect();
    if let Some(default_host) = &remote_config.default_host
        && !hosts.iter().any(|h| h == default_host)
    {
        hosts.push(default_host.clone());
    }
    hosts.sort();
    hosts.dedup();
    hosts
        .iter()
        .filter_map(|alias| crate::session::remote::parse_remote_target(alias).ok())
        .collect()
}

/// Probe `targets` for their sessions, one ssh round-trip each, in parallel, reporting each host's
/// outcome. A host's rows are returned on success; on failure the host contributes no rows but a
/// `Some(error)` outcome so the caller can surface it inline instead of the host silently going
/// empty. One down host never blocks the others.
fn probe_remote_targets_reporting(
    targets: &[crate::session::remote::RemoteTarget],
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> (Vec<DiscoveredSession>, HostProbeStatus) {
    let mut handles = Vec::with_capacity(targets.len());
    for target in targets {
        let config = remote_config.clone();
        let target = target.clone();
        handles.push(std::thread::spawn(move || {
            let outcome = crate::session::discovery::discover_sessions_from(
                &crate::session::discovery::SessionSource::Remote(target.clone()),
                &config,
            );
            (target, outcome)
        }));
    }
    let mut rows = Vec::new();
    let mut host_status = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok((target, outcome)) = handle.join() {
            match outcome {
                Ok(mut remote_rows) => {
                    rows.append(&mut remote_rows);
                    host_status.push((target, None));
                }
                Err(error) => host_status.push((target, Some(error.to_string()))),
            }
        }
    }
    (rows, host_status)
}

/// Probe `targets` for their sessions, discarding per-host errors (used by the picker's eager
/// sweep, which shows only whatever hosts answer).
fn probe_remote_targets(
    targets: &[crate::session::remote::RemoteTarget],
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> Vec<DiscoveredSession> {
    probe_remote_targets_reporting(targets, remote_config).0
}

fn discover_configured_remote_sessions(
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> Vec<DiscoveredSession> {
    probe_remote_targets(&configured_targets(remote_config), remote_config)
}

/// Sidebar discovery (off the UI thread): local sessions always, but only the *expanded* host
/// groups in `probe_targets` are contacted over ssh. This is the on-demand default — a collapsed
/// host is never probed, so opening the Sessions tab does not fan out to every configured host.
pub(crate) fn discover_sidebar_sessions(
    current_name: Option<&str>,
    remote_config: &crate::config::HyprmuxRemoteConfig,
    probe_targets: Vec<crate::session::remote::RemoteTarget>,
    attached: Vec<DiscoveredSession>,
) -> SidebarDiscovery {
    let (remote_rows, host_status) = probe_remote_targets_reporting(&probe_targets, remote_config);
    let rows =
        crate::session::discovery::discover_selectable_sessions(current_name).map(|mut rows| {
            rows.extend(remote_rows);
            for row in attached {
                merge_current_session_row(&mut rows, row);
            }
            sort_session_rows(&mut rows);
            rows
        });
    (rows, host_status)
}

/// Add the attached session to `rows` unless it is already listed. Remote discovery returns the
/// attached session (the current-session exclusion only applies to the *local* scan), so both the
/// sidebar and the picker would otherwise show two `name@host • current` rows under `--remote`.
fn merge_current_session_row(rows: &mut Vec<DiscoveredSession>, current: DiscoveredSession) {
    let already = rows
        .iter()
        .any(|row| row.name == current.name && row.remote_target == current.remote_target);
    if !already {
        rows.push(current);
    }
}

fn sort_session_rows(rows: &mut [DiscoveredSession]) {
    rows.sort_by(|a, b| match (a.host.as_deref(), b.host.as_deref()) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (a_host, b_host) => a_host.cmp(&b_host).then_with(|| a.name.cmp(&b.name)),
    });
}

/// Append rows for every current or retained attachment. This keeps ad-hoc remotes selectable even
/// when they are not part of configured-host discovery.
fn push_attached_session_rows(ctx: &Context<HyprmuxApp>, rows: &mut Vec<DiscoveredSession>) {
    for attached in attached_session_rows(&ctx.state) {
        merge_current_session_row(rows, attached);
    }
    sort_session_rows(rows);
}

/// `(target, alias)` for every remote host a live attachment (current or retained) targets. Feeds
/// the host registry so a host you are attached to stays listed even if configured-host discovery
/// does not cover it (an ad-hoc `--remote` target).
pub(crate) fn held_host_targets(
    state: &crate::state::State,
) -> Vec<(crate::session::remote::RemoteTarget, String)> {
    std::iter::once(state.current())
        .chain(state.background.values())
        .filter_map(|attachment| {
            let target = attachment.remote_target.clone()?;
            let alias = attachment
                .remote_host
                .clone()
                .unwrap_or_else(|| target.display_label());
            Some((target, alias))
        })
        .collect()
}

/// Refresh the unified Sessions view's known-host registry from config, recent ad-hoc targets, and
/// live attachments. Preserves each host's expand/collapse and error state (see
/// [`crate::state::HostRegistry::seed`]).
pub(crate) fn seed_host_registry(ctx: &mut Context<HyprmuxApp>) {
    let recents = crate::session::read_recent_remotes();
    let held = held_host_targets(&ctx.state);
    let remote_config = ctx.state.config.remote.clone();
    ctx.state.hosts.seed(&remote_config, &recents, &held);
    // Load the persisted last-seen sessions once the known hosts exist, so an offline host can
    // still list its workplaces. Empty on first run or any read error.
    if ctx.state.host_session_cache.is_empty() {
        ctx.state.host_session_cache = crate::session::read_host_session_cache();
    }
}

pub(crate) fn attached_session_rows(state: &crate::state::State) -> Vec<DiscoveredSession> {
    std::iter::once(state.current())
        .chain(state.background.values())
        .filter_map(attachment_session_row)
        .collect()
}

fn attachment_session_row(attachment: &crate::state::Attachment) -> Option<DiscoveredSession> {
    let name = attachment.session_name.clone()?;
    Some(DiscoveredSession {
        name,
        ephemeral: attachment.is_ephemeral_session(),
        host: attachment.remote_host.clone(),
        remote_target: attachment.remote_target.clone(),
        status: crate::session::discovery::DiscoveredSessionStatus::Running {
            panes: attachment.workspaces.iter().map(|w| w.panes.len()).sum(),
            has_layout: true,
            clients: attachment.attached_client_count(),
            created_from_profile: attachment.created_from_profile.clone(),
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
    crate::scratchpad::close_for_session_switch(ctx);
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

/// Retain the current attached session in the background instead of tearing it down, so switching
/// back to it is instant and its screens stay live. The current attachment (client + screens) is
/// moved into `State::background` under its epoch, and a fresh empty attachment takes its place for
/// the session being switched to. Named and ephemeral sessions are both retained; parked sessions
/// are only torn down on quit (see [`crate::ops::exit`]).
pub(crate) fn park_current_session(ctx: &mut Context<HyprmuxApp>) {
    crate::update::sidebar::invalidate_sessions(ctx);
    // The popup is a client-local overlay bound to the current server; it must not linger across a
    // switch. The scratchpad, likewise client-local, closes with the current view.
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::update::flush_layout_commit(ctx);
    let old_epoch = ctx.state.runtime_epoch;
    ctx.state
        .park_current(old_epoch, crate::state::Attachment::new());
}

/// Switch to a session already retained in the background: park the current one and bring the parked
/// attachment (id `parked`) to the foreground. Its client and screens are already live, so no
/// reconnect is needed - only the view is re-seeded.
pub(crate) fn switch_to_parked(
    ctx: &mut Context<HyprmuxApp>,
    parked: crate::state::AttachmentId,
) -> Update {
    crate::update::sidebar::invalidate_sessions(ctx);
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::update::flush_layout_commit(ctx);
    let old_epoch = ctx.state.runtime_epoch;
    let Some(restored_epoch) = ctx.state.unpark(parked, old_epoch) else {
        return Update::none();
    };
    ctx.state.runtime_epoch = restored_epoch;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    // Snap to the restored session's geometry rather than interpolating from the previous view.
    ctx.state.animation = crate::anim::GeometryAnimation::None;
    if let Some((rev, layout)) = ctx.state.current_mut().pending_background_layout.take() {
        crate::shared_layout::apply_shared_layout(ctx, &layout, rev);
        ctx.state.animation = crate::anim::GeometryAnimation::None;
    }
    apply_pending_background_closes(ctx);
    if let Some(name) = ctx.state.current().session_name.clone() {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Switched to `{name}`"),
        ));
    }
    let focused = ctx.state.current().focused_pane;
    if let Some(id) = focused {
        crate::ops::focus::request_pane_focus(ctx, id);
    }
    if !ctx.state.current().session_attached {
        return reconnect_current_session(ctx);
    }
    Update::full()
}

pub(crate) fn apply_pending_background_closes(ctx: &mut Context<HyprmuxApp>) {
    if !ctx.state.is_controller() {
        return;
    }
    let pending_closes = std::mem::take(&mut ctx.state.current_mut().pending_background_closes);
    for (pane_id, generation) in pending_closes {
        if ctx
            .state
            .current_mut()
            .find_pane_mut(pane_id)
            .is_some_and(|pane| pane.pty_generation == generation)
        {
            crate::pane_lifecycle::remove_pane(&mut ctx.state, pane_id);
        }
    }
}

/// Reconnect the current attachment without replacing its retained screens or window-manager state.
/// The new id invalidates frames from the dead transport while preserving the attachment identity.
pub(crate) fn reconnect_current_session(ctx: &mut Context<HyprmuxApp>) -> Update {
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
    ctx.toast().push(crate::pty_events::info_toast(
        &ctx.state.theme,
        format!("Reconnecting to {name}…"),
    ));
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

/// Whether an attachment's server should be shut down (rather than detached) when it is released: a
/// solely-attached ephemeral controller owns a disposable server nobody else can reattach to.
pub(crate) fn may_shutdown_attachment(attachment: &crate::state::Attachment) -> bool {
    attachment.is_ephemeral_session()
        && attachment.is_controller()
        && attachment.attached_client_count() == 1
        && attachment
            .shared
            .as_ref()
            .is_none_or(|shared| !shared.read_only)
}

/// Tear down every retained background attachment when leaving the client. Ephemeral servers this
/// client solely owns are shut down; every other parked session is detached and left running for
/// reattach. Called on quit so parked sessions do not leak.
pub(crate) fn release_background(ctx: &mut Context<HyprmuxApp>) {
    for (_epoch, attachment) in std::mem::take(&mut ctx.state.background) {
        let Some(client) = attachment.session_client.as_ref() else {
            continue;
        };
        if may_shutdown_attachment(&attachment) {
            client.shutdown();
        } else {
            client.detach();
        }
    }
}

/// Shared cleanup when a new current session is installed: close the popup and scratchpad (bound to
/// the outgoing session) and the session/profile selection overlays that led here, and mark the
/// Sessions tab stale so the post-update chokepoint re-sweeps for the new current.
fn prepare_session_install(ctx: &mut Context<HyprmuxApp>) {
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.sidebar.invalidate_sessions();
}

/// Shared tail after a new current attachment is in place: snap geometry, resync commands and the
/// terminal palette.
fn finish_session_install(ctx: &mut Context<HyprmuxApp>) {
    // Snap to the new session's geometry rather than interpolating from the previous layout.
    ctx.state.animation = crate::anim::GeometryAnimation::None;
    ctx.state.commands_dirty = true;
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
}

/// Install `attachment` as the current session, dropping the outgoing one. Used only where the
/// outgoing session has *already* been torn down by the caller (kill / disconnect → fresh ephemeral),
/// so there is nothing to retain.
///
/// Only the *current attachment* changes: everything else on [`State`] is client-global (theme,
/// sidebar, background attachments, workbar pollers, control socket, event hub) and is left exactly
/// as it was, so this no longer rebuilds — and silently loses — that state.
pub(crate) fn install_fresh_attachment(
    ctx: &mut Context<HyprmuxApp>,
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
    ctx: &mut Context<HyprmuxApp>,
    attachment: crate::state::Attachment,
    new_epoch: crate::state::AttachmentId,
) -> (
    Option<crate::state::AttachmentId>,
    Option<crate::state::LeftSession>,
) {
    prepare_session_install(ctx);
    crate::update::flush_layout_commit(ctx);
    let outcome = if ctx.state.current().session_attached {
        let old_epoch = ctx.state.runtime_epoch;
        ctx.state.park_current(old_epoch, attachment);
        (Some(old_epoch), None)
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

/// Install a brand-new ephemeral session as current and spawn its attach, after the outgoing
/// session has already been shut down or detached by the caller.
pub(crate) fn swap_to_fresh_ephemeral(ctx: &mut Context<HyprmuxApp>) -> Update {
    let epoch = ctx.state.mint_attachment_id();
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    let attachment = crate::state::fresh_default_attachment(&ctx.state.config);
    install_fresh_attachment(ctx, attachment);
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host: None,
        intent: crate::state::AttachIntent::Plain,
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
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    remote_host: Option<String>,
    discovered_target: Option<crate::session::remote::RemoteTarget>,
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
    let remote_target = match (discovered_target, remote_host.as_deref()) {
        (Some(target), _) => Some(target),
        (None, Some(host)) => match crate::session::remote::parse_remote_target(host) {
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
        (None, None) => None,
    };
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(name.as_str())
        && ctx.state.current().remote_target == remote_target
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
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
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
    activate_discovered_session(ctx, entry)
}

/// Activate a discovered running session without resolving it through a mutable row index. Picker
/// and sidebar callers keep separate ephemeral-discard confirmations.
pub(crate) fn activate_discovered_session(
    ctx: &mut Context<HyprmuxApp>,
    entry: DiscoveredSession,
) -> Update {
    // Discovery already probed this session; an `Unknown` status means the handshake was refused
    // (an incompatible older server is the usual cause). Attaching would only fail after the connect
    // retry deadline, so reject it up front, keep the picker open, and point at the fix - killing
    // the row (Ctrl+K) still works even against a server we can't speak to.
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Unknown
    ) {
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
    // A session shown in the picker is already running, so don't autostart a replacement if it
    // died between discovery and attach.
    attach_session_by_name(ctx, entry.name, entry.host, entry.remote_target, false)
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

/// Raise the create-session prompt pre-targeted at a remote host ("New session on `<host>`"). The
/// named session is created on that host's server when the name is submitted.
pub(crate) fn open_create_session_on_host(
    ctx: &mut Context<HyprmuxApp>,
    target: crate::session::remote::RemoteTarget,
) -> Update {
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_create_on_host(target))
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
            if let Some(workspace) = ctx.state.current_mut().workspaces.get_mut(index) {
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

            // "New session on <host>": create/attach the named session on the remote host, parking
            // the current session in the background. No ephemeral-discard confirm — switching away
            // retains the current session rather than discarding it.
            let host_target = ctx
                .state
                .rename_session
                .as_ref()
                .and_then(|rename| rename.host_target.clone());
            if let Some(target) = host_target {
                ctx.state.rename_session = None;
                let alias = target.display_label();
                return attach_session_by_name(ctx, name, Some(alias), Some(target), true);
            }

            // Creating a session no longer discards the current ephemeral one — like switching, it
            // parks it live in the background — so there is nothing destructive to confirm; a single
            // Enter commits.
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
                client.rename(name.clone());
                ctx.state.current_mut().session_name = Some(name);
                return crate::ops::exit::detach(ctx);
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
        NamingMode::ConnectRemoteHost => {
            let host = name;
            if host.is_empty() {
                ctx.state.rename_session = None;
                request_current_pane_focus(ctx);
                return Update::full();
            }
            // Validate the SSH target before tearing anything down; a bad host must not strand the
            // current session.
            if let Err(err) = crate::session::remote::parse_remote_target(&host) {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Invalid remote host",
                    format!("`{host}`: {err}"),
                ));
                request_rename_session_focus(ctx);
                return Update::full();
            }
            ctx.state.rename_session = None;
            crate::session::record_recent_remote(&host);
            // Attach a fresh ephemeral session on the remote host (as `--remote <host>` does with no
            // session named). The current session is retained in the background per the usual switch.
            let session = crate::state::remote_ephemeral_session_name();
            attach_session_by_name(ctx, session, Some(host), None, true)
        }
    }
}

pub(crate) fn open_connect_remote_host(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_connect_host())
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
        && ctx.state.current().remote_target == entry.remote_target
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

/// Close the client-side attachment for the selected session, leaving its server running. Targets a
/// session retained in the background: its client connection is dropped and the attachment is
/// discarded, but the server (and any other clients) keep going. The current session is left alone -
/// closing it is Detach (`Ctrl+D`) or Kill (`Ctrl+K`) - and a merely-running session we do not hold
/// an attachment to has nothing to close.
pub(crate) fn close_selected_attachment(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    let is_current = ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target;
    if is_current {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            "Detach (Ctrl+D) or kill (Ctrl+K) the current session",
        ));
        return Update::full();
    }
    let Some(id) = ctx
        .state
        .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
    else {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Not attached to `{display}`"),
        ));
        return Update::full();
    };
    if let Some(attachment) = ctx.state.background.remove(&id)
        && let Some(client) = attachment.session_client.as_ref()
    {
        client.detach();
    }
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        format!("Closed attachment to `{display}` — server still running"),
    ));
    refresh_session_picker(ctx)
}

/// Disconnect the client from a remote host: close every attachment (current and retained) to the
/// selected row's host, leaving the remote servers running. A host-wide sibling of
/// [`close_selected_attachment`]; if the current session lives on that host the UI hops to a fresh
/// local ephemeral. Non-destructive - the remote sessions can be reattached later.
pub(crate) fn disconnect_selected_host(ctx: &mut Context<HyprmuxApp>) -> Update {
    clear_pending_session_arms(ctx);
    let Some(picker) = ctx.state.session_picker.as_ref() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    let Some(target) = entry.remote_target.clone() else {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not a remote session"));
        return Update::full();
    };
    disconnect_host(ctx, &target)
}

/// Disconnect from a remote host: close every attachment to it — current and retained — leaving the
/// remote servers running for reattach. If the current session lives on that host, the UI hops onto
/// a fresh local ephemeral. Non-destructive.
pub(crate) fn disconnect_host(
    ctx: &mut Context<HyprmuxApp>,
    target: &crate::session::remote::RemoteTarget,
) -> Update {
    let host_label = target.display_label();
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
        let update = swap_to_fresh_ephemeral(ctx);
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Disconnected from `{host_label}` — {closed} closed, servers still running"),
        ));
        return update;
    }
    if closed == 0 {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Not connected to `{host_label}`"),
        ));
        return Update::full();
    }
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        format!("Disconnected from `{host_label}` — {closed} closed, servers still running"),
    ));
    Update::full()
}

fn shutdown_discovered_session(
    entry: &DiscoveredSession,
    remote_config: &crate::config::HyprmuxRemoteConfig,
) -> std::io::Result<()> {
    if let Some(target) = &entry.remote_target {
        return crate::session::remote::kill_remote_session(target, &entry.name, remote_config)
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
            remote_target: host
                .map(|host| crate::session::remote::RemoteTarget::Alias(host.to_string())),
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
                    // Client-global chrome that must survive a create: an open sidebar on a chosen
                    // tab, and a running workbar command poller.
                    state.sidebar_visible = true;
                    state.sidebar.active_tab = Some(crate::config::SidebarTabId::new("sessions"));
                    state.workbar_commands_running.insert("date".to_string());
                    // Simulate a profile-seeded session: the current pane carries a command.
                    state.current_mut().workspaces[0].panes[0].identity.command =
                        Some("nvim".to_string());
                    state.rename_session =
                        Some(SessionRenameState::new(&name, NamingMode::CreateSession));
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
                // Creating from an attached (ephemeral) session parks it rather than detaching, so
                // there is no "left" session named in the toast, and the parked id is recorded so a
                // failed attach can restore it. The parked session is retained in the background.
                assert_eq!(pending.left, None);
                let parked_epoch = pending.parked_epoch.expect("current session was parked");
                assert!(state.background.contains_key(&parked_epoch));
                assert_eq!(
                    state.background[&parked_epoch].session_name.as_deref(),
                    Some("eph-test")
                );
                // The new session must not inherit the current layout: the installed attachment is a
                // fresh single-pane default with no launch command to respawn.
                assert_eq!(state.current().workspaces[0].panes.len(), 1);
                assert_eq!(
                    state.current().workspaces[0].panes[0].identity.command,
                    None
                );
                // Client-global state is not per-session, so installing a fresh attachment leaves it
                // untouched: command/config epochs don't churn (command tabs keep polling, no
                // flicker), the sidebar stays open on its tab, and workbar pollers keep running.
                // This is the whole point of installing an attachment instead of rebuilding State.
                assert_eq!(state.sidebar.command_epoch, 7);
                assert_eq!(state.sidebar.config_epoch, 11);
                assert!(state.sidebar_visible);
                assert_eq!(
                    state.sidebar.active_tab,
                    Some(crate::config::SidebarTabId::new("sessions"))
                );
                assert!(state.workbar_commands_running.contains("date"));
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
