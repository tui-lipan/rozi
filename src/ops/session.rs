use std::time::Duration;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::Msg;
use crate::ops::focus::{
    request_current_pane_focus, request_rename_session_focus, request_session_picker_focus,
};
use crate::session::discovery::DiscoveredSession;
use crate::state::{NamingMode, SessionPickerState, SessionRenameState};

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

/// Cadence for the off-thread auto-refresh that keeps the open session picker current (sessions
/// appearing/disappearing from other UIs) without a manual refresh key.
const SESSION_PICKER_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);

pub(crate) fn open_session_picker(ctx: &mut Context<AppRoot>) -> Update {
    // Open instantly from local discovery and the last successful remote-host snapshots. Live
    // remote state arrives through the recurring watcher; opening the picker does not need a
    // duplicate eager ssh sweep.
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
        ctx.state.config.remote.clone(),
    ))
}

/// Open the session picker at startup (nothing attached yet). Sets up the picker state and returns
/// the watcher epoch so `init` can kick off the first discovery tick. Local rows show immediately;
/// live remote rows arrive async, so a dead configured host never stalls startup.
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
        ctx.state.config.remote.clone(),
    ))
}

/// Fast, local-only rows used by the picker and Sessions sidebar: local named sessions plus the
/// attached session, with no remote ssh.
pub(crate) fn local_picker_rows(ctx: &Context<AppRoot>) -> Vec<DiscoveredSession> {
    let current_name = ctx.state.local_current_session_name();
    let mut rows =
        crate::session::discovery::discover_selectable_sessions(current_name).unwrap_or_default();
    push_attached_session_rows(ctx, &mut rows);
    rows
}

fn immediate_picker_rows(ctx: &mut Context<AppRoot>) -> Vec<DiscoveredSession> {
    if ctx.state.host_session_cache.is_empty() {
        ctx.state.host_session_cache = crate::session::read_host_session_cache();
    }
    let mut rows = local_picker_rows(ctx);
    push_cached_configured_remote_rows(
        &mut rows,
        &ctx.state.config.remote,
        &ctx.state.host_session_cache,
        &[],
    );
    sort_session_rows(&mut rows);
    rows
}

/// Apply a batch of freshly discovered sessions from the auto-refresh watcher, then re-arm the next
/// tick. Ignored (stopping the loop) once the picker is closed or a newer opening supersedes this
/// `epoch`, which is how the watcher shuts itself down. Selection and the typed query are preserved
/// so a live refresh never disrupts navigation.
pub(crate) fn apply_discovered_sessions(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    mut rows: Vec<DiscoveredSession>,
    host_status: HostProbeStatus,
) -> Update {
    if !ctx.state.show_session_picker || epoch != ctx.state.session_picker_epoch {
        return Update::none();
    }
    let successful_targets: Vec<_> = host_status
        .iter()
        .filter_map(|(target, error)| error.is_none().then_some(target.clone()))
        .collect();
    for target in &successful_targets {
        let label = target.display_label();
        let sessions = cached_sessions_for_target(&rows, target);
        let known = ctx.state.host_session_cache.contains_key(&label);
        if (!sessions.is_empty() || known)
            && ctx.state.host_session_cache.get(&label) != Some(&sessions)
        {
            crate::session::record_host_sessions(&label, sessions.clone());
            ctx.state.host_session_cache.insert(label, sessions);
        }
    }
    // A failed (or not-yet-run) host probe keeps its last successful snapshot visible. Successful
    // hosts use only the fresh rows above, including an empty result which clears stale sessions.
    push_cached_configured_remote_rows(
        &mut rows,
        &ctx.state.config.remote,
        &ctx.state.host_session_cache,
        &successful_targets,
    );
    push_attached_session_rows(ctx, &mut rows);
    sort_session_rows(&mut rows);
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        let selected_identity = picker
            .entries
            .get(picker.selected)
            .map(|entry| (entry.name.clone(), entry.remote_target.clone()));
        let old_selected = picker.selected;
        let entries_changed = picker.entries != rows;
        picker.entries = rows;
        picker.selected = selected_identity
            .and_then(|(name, target)| {
                picker
                    .entries
                    .iter()
                    .position(|entry| entry.name == name && entry.remote_target == target)
            })
            .unwrap_or_else(|| old_selected.min(picker.entries.len().saturating_sub(1)));
        // A destructive confirmation belongs to the exact list the user armed it on. A refresh
        // may insert, remove, or reorder rows, so never let a numeric index carry across a change.
        if entries_changed {
            picker.pending_kill = None;
            picker.pending_restart = None;
        }
    }
    Update::with_command(session_watch_command(
        epoch,
        ctx.state.local_current_session_name().map(str::to_string),
        ctx.state.config.remote.clone(),
    ))
}

fn session_watch_command(
    epoch: u64,
    current_name: Option<String>,
    remote_config: crate::config::RemoteConfig,
) -> Command {
    // Recurring watch: see `profile_session_watch_command` -- the wait belongs on the timer
    // thread, not on a pooled worker held open for the life of the picker.
    Command::after(
        SESSION_PICKER_REFRESH_INTERVAL,
        move |link: CommandLink<Msg>| {
            // Discovery runs here (off the UI thread); a failed sweep simply skips this tick and lets
            // the loop stop rather than clobbering the last good list.
            if let Ok((rows, host_status)) =
                discover_picker_sessions(current_name.as_deref(), &remote_config)
            {
                link.send(Msg::SessionsDiscovered {
                    epoch,
                    rows,
                    host_status,
                });
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
pub(crate) fn discover_picker_sessions(
    current_name: Option<&str>,
    remote_config: &crate::config::RemoteConfig,
) -> std::io::Result<(Vec<DiscoveredSession>, HostProbeStatus)> {
    let mut rows = crate::session::discovery::discover_selectable_sessions(current_name)?;
    let (remote_rows, host_status) =
        probe_remote_targets_reporting(&configured_targets(remote_config), remote_config);
    rows.extend(remote_rows);
    sort_session_rows(&mut rows);
    Ok((rows, host_status))
}

/// Per-probed-host outcome carried back from a sidebar sweep: `None` cleared the host's error,
/// `Some(msg)` records why its probe failed so it can be surfaced inline.
pub(crate) type HostProbeStatus = Vec<(crate::session::remote::RemoteTarget, Option<String>)>;

/// The result of a sidebar discovery sweep: the session rows (or a local-discovery error) plus the
/// per-host probe outcomes.
type SidebarDiscovery = (std::io::Result<Vec<DiscoveredSession>>, HostProbeStatus);

/// Every configured remote target: `[remote.hosts.*]` aliases plus `default_host`.
fn configured_targets(
    remote_config: &crate::config::RemoteConfig,
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
    remote_config: &crate::config::RemoteConfig,
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

/// Convert freshly discovered rows into the small per-host snapshot persisted for immediate picker
/// rendering on the next open.
fn cached_sessions_for_target(
    rows: &[DiscoveredSession],
    target: &crate::session::remote::RemoteTarget,
) -> Vec<crate::session::CachedHostSession> {
    rows.iter()
        .filter(|entry| entry.remote_target.as_ref() == Some(target))
        .map(|entry| crate::session::CachedHostSession {
            name: entry.name.clone(),
            ephemeral: entry.ephemeral,
            panes: match &entry.status {
                crate::session::discovery::DiscoveredSessionStatus::Running { panes, .. } => *panes,
                crate::session::discovery::DiscoveredSessionStatus::Restorable
                | crate::session::discovery::DiscoveredSessionStatus::Busy
                | crate::session::discovery::DiscoveredSessionStatus::Unknown => 0,
            },
        })
        .collect()
}

/// Add last-successful rows for configured hosts not present in `fresh_targets`. Live/local rows
/// win identity collisions, especially for an attachment whose pane/client counts are newer.
fn push_cached_configured_remote_rows(
    rows: &mut Vec<DiscoveredSession>,
    remote_config: &crate::config::RemoteConfig,
    cache: &crate::session::HostSessionCache,
    fresh_targets: &[crate::session::remote::RemoteTarget],
) {
    for target in configured_targets(remote_config)
        .into_iter()
        .filter(|target| !fresh_targets.contains(target))
    {
        let label = target.display_label();
        let Some(sessions) = cache.get(&label) else {
            continue;
        };
        for session in sessions {
            merge_current_session_row(
                rows,
                DiscoveredSession {
                    name: session.name.clone(),
                    ephemeral: session.ephemeral,
                    host: Some(label.clone()),
                    remote_target: Some(target.clone()),
                    status: crate::session::discovery::DiscoveredSessionStatus::Running {
                        panes: session.panes,
                        has_layout: false,
                        clients: 0,
                        created_from_profile: None,
                    },
                },
            );
        }
    }
}

/// Sidebar discovery (off the UI thread): local sessions always, but only the *expanded* host
/// groups in `probe_targets` are contacted over ssh. This is the on-demand default — a collapsed
/// host is never probed, so opening the Sessions tab does not fan out to every configured host.
pub(crate) fn discover_sidebar_sessions(
    current_name: Option<&str>,
    remote_config: &crate::config::RemoteConfig,
    probe_targets: Vec<crate::session::remote::RemoteTarget>,
    attached: Vec<DiscoveredSession>,
) -> SidebarDiscovery {
    let (remote_rows, host_status) = probe_remote_targets_reporting(&probe_targets, remote_config);
    let combine = |local: Vec<DiscoveredSession>,
                   remote: Vec<DiscoveredSession>,
                   attached: Vec<DiscoveredSession>| {
        let mut rows = local;
        rows.extend(remote);
        for row in attached {
            merge_current_session_row(&mut rows, row);
        }
        sort_session_rows(&mut rows);
        rows
    };
    // The local scan and the remote probes are independent, and only a total failure is worth
    // reporting as one. Bundling them meant a single transient local error — a runtime directory
    // read that lost a race, say — threw away remote rows that had just been fetched successfully,
    // so a host the user had connected listed nothing while its header still read Online.
    let rows = match crate::session::discovery::discover_selectable_sessions(current_name) {
        Ok(local) => Ok(combine(local, remote_rows, attached)),
        Err(err) if remote_rows.is_empty() && attached.is_empty() => Err(err),
        Err(_) => Ok(combine(Vec::new(), remote_rows, attached)),
    };
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
fn push_attached_session_rows(ctx: &Context<AppRoot>, rows: &mut Vec<DiscoveredSession>) {
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
pub(crate) fn seed_host_registry(ctx: &mut Context<AppRoot>) {
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

fn require_attached(ctx: &mut Context<AppRoot>) -> Option<()> {
    if ctx.state.current().session_attached {
        Some(())
    } else {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        None
    }
}

fn require_writable(ctx: &mut Context<AppRoot>) -> Option<()> {
    require_attached(ctx)?;
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        crate::pty_events::notify_info(ctx, "Not attached to a session");
        return None;
    };
    if shared.read_only {
        crate::pty_events::notify_info(ctx, "Attached read-only");
        return None;
    }
    Some(())
}

/// If this client is a follower (attached but not the controller), push the take-control nudge and
/// return `true` so the caller aborts a layout-mutating gesture. Controllers and local/unattached
/// sessions return `false`.
pub(crate) fn nudge_if_follower(ctx: &mut Context<AppRoot>) -> bool {
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
    let allow_takeover = ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.allow_takeover);
    let verb = if allow_takeover { "take" } else { "request" };
    let how = crate::commands::command_prefix_chord(ctx, "request-control")
        .map(|chord| format!("{chord} to {verb} control"))
        .unwrap_or_else(|| format!("Try to {verb} control"));
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        None,
        format!("Layout controlled by {who}\n{how}"),
    );
    true
}

/// Request the layout-control lease. A takeover-enabled server grants immediately; cooperative
/// sessions flag the request and notify the controller for a grant or decline.
pub(crate) fn request_control(ctx: &mut Context<AppRoot>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    if ctx.state.is_controller() {
        crate::pty_events::notify_info(ctx, "You already control the layout");
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
    let allow_takeover = shared.allow_takeover;
    let controller_label = shared
        .controller
        .and_then(|id| shared.clients.iter().find(|client| client.id == id))
        .map(|client| format!("{} #{}", client.label, client.id));
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.request_control();
    }
    let message = if allow_takeover {
        "Taking layout control".to_string()
    } else {
        match (already_requested, controller_label) {
            (true, Some(who)) => format!("Still waiting on {who} for layout control"),
            (true, None) => "Control request already pending".to_string(),
            (false, Some(who)) => format!("Requested layout control from {who}"),
            (false, None) => "Requested layout control".to_string(),
        }
    };
    crate::pty_events::notify_on(
        ctx,
        crate::state::ToastChannel::LayoutControl,
        None,
        message,
    );
    Update::full()
}

/// Open the roster of everyone else on the session. The session-wide controls that go with it
/// (request control, input lock, takeover) are their own command-palette entries, not a menu here.
pub(crate) fn open_collaborators(ctx: &mut Context<AppRoot>) -> Update {
    let Some(()) = require_attached(ctx) else {
        return Update::full();
    };
    ctx.state.show_palette = false;
    ctx.state.show_session_picker = false;
    ctx.state.collaboration = Some(crate::state::CollaborationState::new());
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::collaboration_key());
    Update::full()
}

/// Controller-only: remove the client at `index` in the roster. Destructive to someone else's
/// attachment, so it goes through the shared arm-then-confirm window ([`crate::ops::confirm`]) the
/// session kill and pane close use: the first press arms, a second within the window sends it, and
/// an arming left alone lapses.
pub(crate) fn evict_client(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
        return Update::full();
    }
    if target.id == shared.client_id {
        return Update::full();
    }
    let target_id = target.id;
    let label = format!("{} #{}", target.label, target.id);
    let armed = ctx
        .state
        .collaboration
        .as_ref()
        .is_some_and(|collaboration| collaboration.pending_kick == Some(target_id));
    if !armed {
        // First press only arms, on the same clock every other destructive gesture uses: the row
        // renders its own struck-through "again to kill" cue, and the arming lapses on its own
        // if the second press never comes.
        if let Some(collaboration) = ctx.state.collaboration.as_mut() {
            collaboration.pending_kick = Some(target_id);
        }
        return crate::ops::confirm::arm(ctx);
    }
    if let Some(collaboration) = ctx.state.collaboration.as_mut() {
        collaboration.pending_kick = None;
    }
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.evict_client(target_id);
    }
    crate::pty_events::notify_info(ctx, format!("Removed {label} from the session"));
    Update::full()
}

/// Whether this client can remove others: the writable controller of a session whose server is new
/// enough to understand the message.
pub(crate) fn can_evict(state: &crate::state::State) -> bool {
    state.current().shared.as_ref().is_some_and(|shared| {
        !shared.read_only && shared.is_controller() && state.current().session_client.is_some()
    })
}

/// Raise the follow prompt if this attach landed on a session another client is actively driving.
///
/// Following used to be what happened to whoever attached second, which is a poor way to learn that
/// your keyboard no longer shapes the layout. It is now a decision: watch along, ask for the lease,
/// or back out. A session with no active controller — including one whose only other client is
/// parked — needs no prompt, because attaching there gets control outright.
pub(crate) fn prompt_follow_if_occupied(ctx: &mut Context<AppRoot>) {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return;
    };
    // A read-only attach already said it is not here to drive the layout.
    if shared.read_only || shared.is_controller() {
        return;
    }
    let Some(controller) = shared.controller else {
        return;
    };
    let controller_label = shared
        .clients
        .iter()
        .find(|client| client.id == controller)
        .map(|client| client.label.clone())
        .unwrap_or_else(|| format!("client {controller}"));
    let Some(session) = ctx.state.current().session_name.clone() else {
        return;
    };
    let allow_takeover = shared.allow_takeover;
    ctx.state.show_palette = false;
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.collaboration = None;
    ctx.state.follow_prompt = Some(crate::state::FollowPromptState {
        session,
        controller_label,
        allow_takeover,
        selected: 0,
    });
    ctx.state.commands_dirty = true;
    ctx.request_focus(crate::view::follow_prompt_key());
}

/// Apply the user's answer to the follow prompt. Cancelling leaves the session the way switching
/// away from it would, landing on the session picker when other choices remain, otherwise the
/// launcher.
pub(crate) fn resolve_follow_prompt(
    ctx: &mut Context<AppRoot>,
    choice: crate::state::FollowChoice,
) -> Update {
    ctx.state.follow_prompt = None;
    ctx.state.commands_dirty = true;
    match choice {
        crate::state::FollowChoice::Follow => {
            request_current_pane_focus(ctx);
            Update::full()
        }
        crate::state::FollowChoice::AskForControl => {
            request_current_pane_focus(ctx);
            request_control(ctx)
        }
        crate::state::FollowChoice::Cancel => {
            let name = ctx.state.current().session_name.clone();
            let detached_epoch = ctx.state.runtime_epoch;
            crate::update::flush_layout_commit(ctx);
            crate::ops::exit::mark_session_detached(ctx, None);
            if let Some(client) = ctx.state.current().session_client.clone() {
                client.detach();
            }
            let update = land_on_surviving_session(ctx);
            // Switching back temporarily parks the cancelled attachment. It was intentionally
            // detached, so retaining it would make discovery render the still-live server offline.
            ctx.state.background.remove(&detached_epoch);
            if let Some(name) = name {
                crate::pty_events::notify_info(ctx, format!("Left `{name}` alone"));
            }
            update
        }
    }
}

pub(crate) fn toggle_input_lock(ctx: &mut Context<AppRoot>) -> Update {
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

pub(crate) fn toggle_control_takeover(ctx: &mut Context<AppRoot>) -> Update {
    if nudge_if_follower(ctx) {
        return Update::full();
    }
    let Some(()) = require_writable(ctx) else {
        return Update::full();
    };
    if ctx.state.current().session_client.is_none() {
        return Update::full();
    }
    let allowed = !ctx
        .state
        .current()
        .shared
        .as_ref()
        .expect("writable session checked")
        .allow_takeover;
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        client.set_control_takeover(allowed);
    }
    Update::full()
}

pub(crate) fn grant_control(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(shared) = ctx.state.current().shared.as_ref() else {
        return Update::none();
    };
    let Some(target) = shared.clients.get(index) else {
        return Update::none();
    };
    if !ctx.state.is_controller() {
        nudge_if_follower(ctx);
    } else if target.read_only {
        crate::pty_events::notify_info(ctx, "Read-only clients cannot control the layout");
    } else if target.id != shared.client_id
        && let Some(client) = ctx.state.current().session_client.as_ref()
    {
        client.grant_control(target.id);
        ctx.state.collaboration = None;
    }
    Update::full()
}

/// Controller-only quick action: grant the lease to the client that requested it (the earliest
/// pending requester when several are waiting). Nudges a follower, and toasts when nothing is
/// pending, so the bound key always gives feedback.
pub(crate) fn grant_control_to_requester(ctx: &mut Context<AppRoot>) -> Update {
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
            ctx.state.collaboration = None;
        }
        None => {
            crate::pty_events::notify_info(ctx, "No pending control requests");
        }
    }
    Update::full()
}

/// Controller-only: decline the pending control request from the client at `index` in the roster.
/// A no-op (with a follower nudge) when this client is not the controller, or when the target has no
/// pending request.
pub(crate) fn decline_control(ctx: &mut Context<AppRoot>, index: usize) -> Update {
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
pub(crate) fn release_current_session(ctx: &mut Context<AppRoot>) {
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
pub(crate) fn park_current_session(ctx: &mut Context<AppRoot>) {
    crate::update::sidebar::invalidate_sessions(ctx);
    // The popup is a client-local overlay bound to the current server; it must not linger across a
    // switch. The scratchpad, likewise client-local, closes with the current view.
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::update::flush_layout_commit(ctx);
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
fn mark_current_parked(ctx: &mut Context<AppRoot>, parked: bool) {
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
fn discard_parked_if_disposable(ctx: &mut Context<AppRoot>, epoch: crate::state::AttachmentId) {
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
    crate::update::sidebar::invalidate_sessions(ctx);
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    crate::update::flush_layout_commit(ctx);
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
fn dismiss_session_pickers(ctx: &mut Context<AppRoot>) {
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
}

/// Shared cleanup when a new current session is installed: close the popup and scratchpad (bound to
/// the outgoing session) and the session/profile selection overlays that led here, and mark the
/// Sessions tab stale so the post-update chokepoint re-sweeps for the new current.
fn prepare_session_install(ctx: &mut Context<AppRoot>) {
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    dismiss_session_pickers(ctx);
    ctx.state.sidebar.invalidate_sessions();
}

/// Shared tail after a new current attachment is in place: snap geometry, resync commands and the
/// terminal palette.
fn finish_session_install(ctx: &mut Context<AppRoot>) {
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
/// exactly
/// as it was, so this no longer rebuilds — and silently loses — that state.
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
    crate::update::flush_layout_commit(ctx);
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
    crate::update::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.shutdown();
    }
    enter_sessionless(ctx);
    if let Some((session_name, target)) = killed_identity {
        remove_cached_remote_session(ctx, &session_name, &target);
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
    crate::update::flush_layout_commit(ctx);
    crate::ops::exit::mark_session_detached(ctx, None);
    if let Some(client) = ctx.state.current().session_client.clone() {
        client.shutdown();
    }
    if let Some(target) = remote_target.as_ref() {
        remove_cached_remote_session(ctx, &name, target);
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

fn enter_sessionless(ctx: &mut Context<AppRoot>) {
    crate::popup::kill_if_open(ctx);
    crate::scratchpad::close_for_session_switch(ctx);
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    crate::ops::session::clear_pending_session_action(ctx, None);
    // Leave the session picker alone: kill-from-picker refreshes it in place, and
    // `offer_session_picker_or_launcher` opens or closes it deliberately.
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.attachment = crate::state::Attachment::new();
    ctx.state.runtime_epoch = ctx.state.mint_attachment_id();
    ctx.state.current_mut().epoch = ctx.state.runtime_epoch;
    finish_session_install(ctx);
}

fn has_meaningful_session_choices(ctx: &mut Context<AppRoot>) -> bool {
    !immediate_picker_rows(ctx).is_empty() || crate::session::bootstrap::has_session_candidates()
}

fn offer_session_picker_or_launcher(ctx: &mut Context<AppRoot>) -> Update {
    if has_meaningful_session_choices(ctx) {
        open_session_picker(ctx)
    } else {
        ctx.state.show_session_picker = false;
        ctx.state.session_picker = None;
        ctx.state.commands_dirty = true;
        Update::full()
    }
}

/// After a kill from the open picker: rebuild the list, keep the nearest selection, and close into
/// the launcher when nothing remains to pick.
fn refresh_picker_after_kill(ctx: &mut Context<AppRoot>) -> Update {
    let update = refresh_session_picker(ctx);
    let empty = ctx
        .state
        .session_picker
        .as_ref()
        .is_none_or(|picker| picker.entries.is_empty());
    if empty && !crate::session::bootstrap::has_session_candidates() {
        return close_session_picker(ctx);
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
    // Discovery already probed this session; an `Unknown` status means the handshake was refused
    // (an incompatible older server is the usual cause). Attaching would only fail after the connect
    // retry deadline, so reject it up front, keep the picker open, and point at the fix - killing
    // the row (Ctrl+K) still works even against a server we can't speak to.
    if matches!(
        entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Unknown
    ) {
        crate::pty_events::notify_error(
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

/// Go to this client's scratch session: the session picker's `Ctrl+T`, and its `Enter` when there
/// is nothing on the list to activate.
///
/// One key covers both directions — start the ephemeral when there is none, switch to it when there
/// already is — because from the keyboard they are the same request. Already being on it is a
/// no-op beyond closing the picker: switching somewhere you already are is not worth a toast.
pub(crate) fn open_ephemeral_session(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    // Checked before the launcher case: the session on screen being the scratch one settles this
    // whether or not its client is live, and re-attaching what is already attached is never right.
    if ctx.state.is_ephemeral_session() {
        return close_session_picker(ctx);
    }
    // In the launcher there is nothing to park, and the panes the launch prepared are still waiting
    // to be handed to the session that starts.
    if ctx.state.needs_session_for_pty() {
        return start_launcher_shell(ctx);
    }
    let held = held_ephemeral_session(&ctx.state).map(|attachment| {
        (
            attachment.session_name.clone().unwrap_or_default(),
            attachment.remote_host.clone(),
            attachment.remote_target.clone(),
        )
    });
    let (name, remote_host, remote_target) = held.unwrap_or_else(|| {
        // Nothing held: create the one this client would launch. Under `--remote` the ephemeral
        // lives on the remote host, so it takes the host-qualified name and that host's target.
        let remote_target = ctx.state.current().remote_target.clone();
        let name = if remote_target.is_some() {
            crate::state::remote_ephemeral_session_name()
        } else {
            crate::state::ephemeral_session_name()
        };
        (name, ctx.state.current().remote_host.clone(), remote_target)
    });
    attach_session_by_name(ctx, name, remote_host, remote_target, true)
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
        Some(target) => ctx
            .state
            .host_session_cache
            .get(&target.display_label())
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

pub(crate) fn apply_rename_session(ctx: &mut Context<AppRoot>) -> Update {
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
            crate::ops::overlay_return::finish(ctx)
        }
        NamingMode::CreateSession | NamingMode::OpenProfileAs => {
            let open_ephemeral = rename_state.mode == NamingMode::OpenProfileAs && name.is_empty();
            if !open_ephemeral && !crate::session::discovery::valid_session_name(&name) {
                reject_session_name(ctx, "Use letters, numbers, _ or -");
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
            if !open_ephemeral && session_name_already_running(ctx, &name, host_target.as_ref()) {
                reject_session_name(ctx, format!("Session `{name}` is already running"));
                return Update::full();
            }
            if let Some(target) = host_target {
                ctx.state.rename_session = None;
                // Attaching retires the picker this was raised from: its rows are about to be
                // stale, so land on the new session rather than back in a list.
                crate::ops::overlay_return::leave(ctx);
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
            crate::ops::overlay_return::leave(ctx);
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
            let leave = rename_state.leave;
            // At the leave prompt an empty name is not a mistake, it is the other answer: close
            // these sessions and go. It takes a second press, and what that press closes is
            // spelled out in the prompt itself while the finger is still over the key.
            if name.is_empty()
                && let Some(leave) = leave
            {
                let confirm = ctx.state.config.confirm.quit_ephemeral;
                if confirm && !leave.armed {
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
                return crate::ops::exit::leave_client_now(ctx, true);
            }
            if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
                reject_session_name(ctx, "Use letters, numbers, _ or -");
                return Update::full();
            }

            // Naming must not collide with another live session.
            if session_name_already_running(ctx, &name, ctx.state.current().remote_target.as_ref())
                && ctx.state.current().session_name.as_deref() != Some(name.as_str())
            {
                reject_session_name(ctx, format!("Session `{name}` is already running"));
                return Update::full();
            }

            ctx.state.rename_session = None;

            if leave.is_some() {
                // The client is leaving the session; there is nothing to return to.
                crate::ops::overlay_return::leave(ctx);
                let Some(client) = ctx.state.current().session_client.clone() else {
                    crate::pty_events::notify_error(
                        ctx,
                        "Rename failed",
                        "Session connection lost",
                    );
                    return Update::full();
                };
                crate::update::flush_layout_commit(ctx);
                client.rename(name.clone());
                ctx.state.current_mut().session_name = Some(name);
                // Naming kept this one; anything still temporary gets its own prompt on the way
                // out, so no session is closed without having been offered a name.
                return crate::ops::exit::leave_client(ctx);
            }

            if ctx.state.current().session_name.as_deref() == Some(name.as_str()) {
                return crate::ops::overlay_return::finish(ctx);
            }

            if let Some(client) = ctx.state.current().session_client.clone() {
                client.rename(name);
            }
            // Naming in place attaches nothing, so the picker it was raised from ("name current")
            // is still valid - reopen it showing the new name.
            crate::ops::overlay_return::finish(ctx)
        }
        NamingMode::RenameSession => {
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

            if ctx.state.current().session_name.as_deref() == Some(name.as_str()) {
                return crate::ops::overlay_return::finish(ctx);
            }

            if let Some(client) = ctx.state.current().session_client.clone() {
                client.rename(name);
            }
            crate::ops::overlay_return::finish(ctx)
        }
        NamingMode::ConnectRemoteHost => {
            let host = name;
            if host.is_empty() {
                // An empty target is a cancel by another name.
                ctx.state.rename_session = None;
                return crate::ops::overlay_return::finish(ctx);
            }
            // Validate the SSH target before tearing anything down; a bad host must not strand the
            // current session.
            if let Err(err) = crate::session::remote::parse_remote_target(&host) {
                crate::pty_events::notify_error(
                    ctx,
                    "Invalid remote host",
                    format!("`{host}`: {err}"),
                );
                request_rename_session_focus(ctx);
                return Update::full();
            }
            ctx.state.rename_session = None;
            crate::ops::overlay_return::leave(ctx);
            crate::session::record_recent_remote(&host);
            // Attach a fresh ephemeral session on the remote host (as `--remote <host>` does with no
            // session named). The current session is retained in the background per the usual switch.
            let session = crate::state::remote_ephemeral_session_name();
            attach_session_by_name(ctx, session, Some(host), None, true)
        }
    }
}

pub(crate) fn open_connect_remote_host(ctx: &mut Context<AppRoot>) -> Update {
    clear_pending_session_arms(ctx);
    enter_session_rename(ctx, SessionRenameState::new_connect_host())
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
pub(crate) fn restart_discovered_session(
    ctx: &mut Context<AppRoot>,
    entry: DiscoveredSession,
) -> Update {
    if matches!(
        &entry.status,
        crate::session::discovery::DiscoveredSessionStatus::Restorable
    ) {
        return activate_discovered_session(ctx, entry);
    }
    let is_current = ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target;
    if is_current {
        return restart_current_session(ctx);
    }
    // Drop any parked attachment before recreating so we do not keep a dead background client.
    if let Some(id) = ctx
        .state
        .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
        && let Some(attachment) = ctx.state.background.remove(&id)
        && let Some(client) = attachment.session_client.as_ref()
    {
        client.detach();
    }
    let remote_config = ctx.state.config.remote.clone();
    if let Err(err) = shutdown_discovered_session(&entry, &remote_config) {
        crate::pty_events::notify_error(ctx, "Restart failed", err.to_string());
        return Update::full();
    }
    if let Some(target) = entry.remote_target.as_ref() {
        remove_cached_remote_session(ctx, &entry.name, target);
    }
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    let restart_name = if entry.ephemeral {
        if entry.remote_target.is_some() {
            crate::state::remote_ephemeral_session_name()
        } else {
            crate::state::fresh_ephemeral_session_name(ctx.state.mint_attachment_id())
        }
    } else {
        entry.name.clone()
    };
    // Recreate and make it active immediately — never leave a silent background reconnect.
    let update = attach_session_by_name(
        ctx,
        restart_name,
        entry.host.clone(),
        entry.remote_target.clone(),
        true,
    );
    crate::pty_events::notify_info(ctx, format!("Restarted session `{display}`"));
    update
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
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target
    {
        return kill_current_session(ctx, display);
    }
    let remote_config = ctx.state.config.remote.clone();
    match shutdown_discovered_session(&entry, &remote_config) {
        Ok(()) => {
            // Drop any parked client attachment for the killed server so it cannot linger offline.
            if let Some(id) = ctx
                .state
                .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
                && let Some(attachment) = ctx.state.background.remove(&id)
                && let Some(client) = attachment.session_client.as_ref()
            {
                client.detach();
            }
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
            crate::pty_events::notify_error(ctx, "Kill failed", err.to_string());
            Update::full()
        }
    }
}

fn remove_cached_remote_session(
    ctx: &mut Context<AppRoot>,
    session_name: &str,
    target: &crate::session::remote::RemoteTarget,
) {
    let label = target.display_label();
    let Some(sessions) = ctx.state.host_session_cache.get_mut(&label) else {
        return;
    };
    let old_len = sessions.len();
    sessions.retain(|session| session.name != session_name);
    if sessions.len() != old_len {
        crate::session::record_host_sessions(&label, sessions.clone());
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
    let display = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    let is_current = ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_target == entry.remote_target;
    if is_current {
        crate::pty_events::notify_info(
            ctx,
            "Kill (Ctrl+K) the current session, or leave the client",
        );
        return Update::full();
    }
    let Some(id) = ctx
        .state
        .parked_attachment_id(&entry.name, entry.remote_target.as_ref())
    else {
        crate::pty_events::notify_info(ctx, format!("Not connected to `{display}`"));
        return Update::full();
    };
    if let Some(attachment) = ctx.state.background.remove(&id)
        && let Some(client) = attachment.session_client.as_ref()
    {
        client.detach();
    }
    crate::pty_events::notify_info(
        ctx,
        format!("Disconnected from `{display}` — server still running"),
    );
    refresh_session_picker(ctx)
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
    let Some(target) = entry.remote_target.clone() else {
        crate::pty_events::notify_info(ctx, "Not a remote session");
        return Update::full();
    };
    disconnect_host(ctx, &target)
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

fn shutdown_discovered_session(
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

fn shutdown_session(name: &str) -> std::io::Result<()> {
    crate::session::server::shutdown_named_session(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
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

    #[test]
    fn picker_refresh_preserves_identity_and_clears_destructive_arms() {
        use crate::AppRoot;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let mut picker = SessionPickerState::new(vec![
                    session_row("alpha", None),
                    session_row("zulu", None),
                ]);
                picker.selected = 1;
                picker.pending_kill = Some(1);
                backend.state_mut().session_picker = Some(picker);
                backend.state_mut().show_session_picker = true;
                let epoch = backend.state().session_picker_epoch;

                backend
                    .update_level(crate::Msg::SessionsDiscovered {
                        epoch,
                        rows: vec![session_row("beta", None), session_row("zulu", None)],
                        host_status: Vec::new(),
                    })
                    .expect("apply picker refresh");

                let picker = backend.state().session_picker.as_ref().expect("picker");
                assert_eq!(picker.entries[picker.selected].name, "zulu");
                assert!(picker.pending_kill.is_none());
                assert!(picker.pending_restart.is_none());
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn cached_configured_hosts_are_available_without_a_probe() {
        let mut config = crate::config::RemoteConfig::default();
        config.hosts.insert(
            "winvm".to_string(),
            crate::config::RemoteHostConfig::default(),
        );
        let mut cache = crate::session::HostSessionCache::new();
        cache.insert(
            "winvm".to_string(),
            vec![crate::session::CachedHostSession {
                name: "dev".to_string(),
                ephemeral: false,
                panes: 4,
            }],
        );
        let mut rows = vec![session_row("local", None)];

        push_cached_configured_remote_rows(&mut rows, &config, &cache, &[]);

        let remote = rows
            .iter()
            .find(|row| row.name == "dev")
            .expect("cached remote row");
        assert_eq!(remote.host.as_deref(), Some("winvm"));
        assert_eq!(
            remote.remote_target,
            Some(crate::session::remote::RemoteTarget::Alias(
                "winvm".to_string()
            ))
        );
        assert!(matches!(
            remote.status,
            crate::session::discovery::DiscoveredSessionStatus::Running { panes: 4, .. }
        ));
    }

    #[test]
    fn fresh_host_results_replace_cached_rows() {
        let mut config = crate::config::RemoteConfig::default();
        config.hosts.insert(
            "winvm".to_string(),
            crate::config::RemoteHostConfig::default(),
        );
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        let mut cache = crate::session::HostSessionCache::new();
        cache.insert(
            "winvm".to_string(),
            vec![crate::session::CachedHostSession {
                name: "stale".to_string(),
                ephemeral: false,
                panes: 2,
            }],
        );
        let mut rows = vec![session_row("live", Some("winvm"))];

        push_cached_configured_remote_rows(
            &mut rows,
            &config,
            &cache,
            std::slice::from_ref(&target),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "live");
    }

    fn ephemeral_state(client_id: u64, controller: u64, clients: Vec<ClientInfo>) -> State {
        let mut state = State::new(Config::default(), ThemePreset::Lipan.theme());
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
        use crate::AppRoot;
        use crate::Msg;
        use crate::input::Action;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
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
                            parked: false,
                        },
                        ClientInfo {
                            id: 2,
                            label: "them".into(),
                            read_only: false,
                            requesting_control: false,
                            parked: false,
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

    /// Set up a controller on a session shared with one writable client that wants the lease.
    #[cfg(test)]
    fn shared_controller_backend() -> tui_lipan::TestBackend<crate::AppRoot> {
        use crate::AppRoot;
        use crate::session::client::SessionClient;
        use tui_lipan::TestBackend;
        use tui_lipan::prelude::Rect;

        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 90,
            h: 28,
        });
        let (client, _rx) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_name = Some("dev".into());
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.clients = vec![
            ClientInfo {
                id: 1,
                label: "me".into(),
                read_only: false,
                requesting_control: false,
                parked: false,
            },
            ClientInfo {
                id: 2,
                label: "laptop".into(),
                read_only: false,
                requesting_control: true,
                parked: false,
            },
        ];
        state.current_mut().shared = Some(shared);
        backend
    }

    /// The dialog is rows and chrome, never prose: this client's identity and role ride the top
    /// border as a right header, the other clients are rows with compact markers, and the keys that
    /// currently apply are footer pills. Nothing states a fact in a sentence.
    #[test]
    fn collaborators_dialog_is_rows_and_chrome_with_no_prose_line() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

                backend.render();
                let rendered = backend.capture_frame().to_fixed_grid();
                // Title and self-context share the top border, so neither costs a content row.
                assert!(
                    rendered.contains("Manage collaborators"),
                    "expected the title on the border: {rendered}"
                );
                assert!(
                    rendered.contains("me #1 · ctrl"),
                    "expected the self tag as a right header: {rendered}"
                );
                assert!(
                    !rendered.contains("You:"),
                    "the self context must not be a prose line: {rendered}"
                );
                assert_eq!(rendered.matches("me #1").count(), 1, "{rendered}");
                assert!(rendered.contains("Search other clients"), "{rendered}");
                assert!(rendered.contains("laptop #2"), "{rendered}");
                assert!(rendered.contains("wants ctrl"), "{rendered}");
                // Every key that applies is advertised, and each is a Ctrl chord or Enter, because
                // the query input owns focus and a bare letter has to reach the filter.
                assert!(rendered.contains("grant control enter"), "{rendered}");
                assert!(rendered.contains("decline ctrl+d"), "{rendered}");
                assert!(rendered.contains("kick ctrl+k"), "{rendered}");
            })
            .expect("spawn collaborators view test")
            .join()
            .expect("collaborators view test completes");
    }

    /// Typing filters the roster instead of triggering actions: the letters that used to navigate
    /// or act (`j`, `k`, `g`, `d`, `x`) must reach the query input now that it owns focus.
    #[test]
    fn plain_letters_reach_the_filter_instead_of_acting() {
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                let (client, rx) = SessionClient::test_channel();
                backend.state_mut().current_mut().session_client = Some(client);
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
                backend.render();

                for letter in ['j', 'k', 'g', 'd', 'x'] {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Char(letter),
                            mods: KeyMods::NONE,
                        })
                        .expect("send letter");
                }

                let sent: Vec<_> = rx.try_iter().collect();
                assert!(
                    !sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(
                            ClientMessage::GrantControl { .. }
                                | ClientMessage::DeclineControl { .. }
                                | ClientMessage::EvictClient { .. }
                        )
                    )),
                    "typing must not act on a client, got {sent:?}"
                );
                assert!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .is_some_and(|collaboration| collaboration.pending_kick.is_none()),
                    "typing must not arm a removal"
                );
                // The letters landed in the filter, which is the whole point of freeing them.
                let rendered = backend.capture_frame().to_fixed_grid();
                assert!(rendered.contains("jkgdx"), "{rendered}");
                assert!(!rendered.contains("laptop #2"), "{rendered}");
            })
            .expect("spawn filter-typing test")
            .join()
            .expect("filter-typing test completes");
    }

    /// An empty list means two different things, and the message must not claim the wrong one: a
    /// query that matched nobody is not the same as a session nobody else is on.
    #[test]
    fn an_empty_list_says_whether_it_is_the_filter_or_the_roster() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
                backend
                    .dispatch(crate::Msg::CollaborationQueryChanged("zzz".to_string()))
                    .expect("filter to nothing");
                backend.render();
                let filtered = backend.capture_frame().to_fixed_grid();
                assert!(
                    filtered.contains("No client matches `zzz`"),
                    "a filtered-out roster must name the query: {filtered}"
                );
                assert!(
                    !filtered.contains("No other clients"),
                    "clients are attached, so claiming otherwise is false: {filtered}"
                );

                // The same dialog with the roster genuinely empty says so, query or not.
                if let Some(shared) = backend.state_mut().current_mut().shared.as_mut() {
                    shared.clients.retain(|client| client.id == 1);
                }
                backend.render();
                let empty = backend.capture_frame().to_fixed_grid();
                assert!(empty.contains("No other clients"), "{empty}");
                assert!(!empty.contains("No client matches"), "{empty}");
            })
            .expect("spawn empty-text test")
            .join()
            .expect("empty-text test completes");
    }

    /// A query that hides every client must leave nothing to act on: the footer stops advertising
    /// keys, and `ctrl+k` cannot reach a row scrolled out of sight by the filter.
    #[test]
    fn a_filter_that_hides_everyone_disarms_the_dialog() {
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                let (client, rx) = SessionClient::test_channel();
                backend.state_mut().current_mut().session_client = Some(client);
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
                backend.render();

                // Positive control: unfiltered, the chord reaches the interceptor and arms the row.
                // Without this the negative below would pass even if no key arrived at all.
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('k'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("send ctrl+k");
                assert_eq!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .and_then(|collaboration| collaboration.pending_kick),
                    Some(2),
                    "ctrl+k must arm the highlighted client when it is visible"
                );

                backend
                    .dispatch(crate::Msg::CollaborationQueryChanged("zzz".to_string()))
                    .expect("filter to nothing");
                backend.render();
                let rendered = backend.capture_frame().to_fixed_grid();
                assert!(!rendered.contains("kick"), "{rendered}");
                assert!(!rendered.contains("grant control"), "{rendered}");

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('k'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("send ctrl+k");
                let sent: Vec<_> = rx.try_iter().collect();
                assert!(
                    !sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::EvictClient { .. })
                    )),
                    "a hidden client must not be removable, got {sent:?}"
                );
            })
            .expect("spawn hidden-filter test")
            .join()
            .expect("hidden-filter test completes");
    }

    /// Removing a client is destructive to somebody else's attachment, so the first press only arms
    /// the row and nothing goes on the wire until the second one.
    #[test]
    fn kicking_a_collaborator_takes_two_presses() {
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                // `test_channel` speaks this build's maximum protocol, which is what gates evicting.
                let (client, rx) = SessionClient::test_channel();
                backend.state_mut().current_mut().session_client = Some(client);
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

                backend
                    .dispatch(crate::Msg::CollaborationKick(1))
                    .expect("arm the removal");
                assert_eq!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .and_then(|collaboration| collaboration.pending_kick),
                    Some(2),
                    "arming is held by client id, not roster position"
                );
                let armed_traffic: Vec<_> = rx.try_iter().collect();
                assert!(
                    !armed_traffic.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::EvictClient { .. })
                    )),
                    "arming must not remove anyone yet, got {armed_traffic:?}"
                );

                backend
                    .dispatch(crate::Msg::CollaborationKick(1))
                    .expect("confirm the removal");
                let sent: Vec<_> = rx.try_iter().collect();
                assert!(
                    sent.iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::EvictClient { target: 2 })
                    )),
                    "expected an evict for client 2, got {sent:?}"
                );
                assert!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .is_some_and(|collaboration| collaboration.pending_kick.is_none())
                );
            })
            .expect("spawn kick test")
            .join()
            .expect("kick test completes");
    }

    /// The arming runs on the shared confirmation clock, so a kick left half-pressed lapses like
    /// every other destructive gesture rather than waiting indefinitely for a second key.
    #[test]
    fn an_unconfirmed_kick_lapses_on_the_shared_confirm_window() {
        use crate::session::client::SessionClient;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = shared_controller_backend();
                let (client, _rx) = SessionClient::test_channel();
                backend.state_mut().current_mut().session_client = Some(client);
                backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

                backend
                    .dispatch(crate::Msg::CollaborationKick(1))
                    .expect("arm the removal");
                let armed_epoch = backend.state().confirm_epoch;
                assert!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .is_some_and(|collaboration| collaboration.pending_kick.is_some()),
                    "arming must register with the shared clock"
                );

                backend
                    .dispatch(crate::Msg::ConfirmationExpired(armed_epoch))
                    .expect("the window lapses");
                assert!(
                    backend
                        .state()
                        .collaboration
                        .as_ref()
                        .is_some_and(|collaboration| collaboration.pending_kick.is_none()),
                    "an unconfirmed kick must disarm itself"
                );
            })
            .expect("spawn kick-expiry test")
            .join()
            .expect("kick-expiry test completes");
    }

    #[test]
    fn occupied_session_prompt_keeps_context_in_the_title() {
        use crate::AppRoot;
        use crate::state::FollowPromptState;
        use tui_lipan::TestBackend;
        use tui_lipan::prelude::Rect;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 72,
                    h: 20,
                });
                backend.state_mut().follow_prompt = Some(FollowPromptState {
                    session: "test".into(),
                    controller_label: "razuer".into(),
                    allow_takeover: true,
                    selected: 0,
                });

                backend.render();
                let rendered = backend.capture_frame().to_fixed_grid();
                assert!(rendered.contains("`test` in use by razuer"), "{rendered}");
                assert!(!rendered.contains("is being driven"), "{rendered}");
                assert!(rendered.contains("no layout control"), "{rendered}");
                assert!(rendered.contains("control moves to you"), "{rendered}");
                assert!(rendered.contains("go back"), "{rendered}");
            })
            .expect("spawn occupied-session prompt test")
            .join()
            .expect("occupied-session prompt test completes");
    }

    #[test]
    fn cancelling_occupied_attach_does_not_retain_it_as_offline() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::{Attachment, ConnectionState, FollowPromptState};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (target_client, target_rx) = SessionClient::test_channel();
                let (survivor_client, _survivor_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 10;
                    state.current_mut().epoch = 10;
                    state.current_mut().session_name = Some("occupied".into());
                    state.current_mut().session_attached = true;
                    state.current_mut().connection = ConnectionState::Connected;
                    state.current_mut().session_client = Some(target_client);
                    let mut shared = SharedSessionState::new(2);
                    shared.controller = Some(1);
                    shared.clients = vec![
                        ClientInfo {
                            id: 1,
                            label: "desktop".into(),
                            read_only: false,
                            requesting_control: false,
                            parked: false,
                        },
                        ClientInfo {
                            id: 2,
                            label: "laptop".into(),
                            read_only: false,
                            requesting_control: false,
                            parked: false,
                        },
                    ];
                    state.current_mut().shared = Some(shared);

                    let mut survivor = Attachment::new();
                    survivor.epoch = 5;
                    survivor.parked_seq = 1;
                    survivor.session_name = Some("previous".into());
                    survivor.session_attached = true;
                    survivor.connection = ConnectionState::Connected;
                    survivor.session_client = Some(survivor_client);
                    let mut survivor_shared = SharedSessionState::new(1);
                    survivor_shared.controller = Some(1);
                    survivor.shared = Some(survivor_shared);
                    state.background.insert(5, survivor);
                    state.follow_prompt = Some(FollowPromptState {
                        session: "occupied".into(),
                        controller_label: "desktop".into(),
                        allow_takeover: false,
                        selected: 2,
                    });
                }

                backend
                    .dispatch(Msg::FollowPromptChoose(2))
                    .expect("cancel occupied attach");

                let state = backend.state();
                assert!(
                    state.is_launcher(),
                    "cancelling leaves the foreground sessionless rather than auto-attaching"
                );
                assert!(
                    state.show_session_picker,
                    "the parked previous session remains as a choice"
                );
                assert!(!state.background.contains_key(&10));
                assert!(state.attachment_by_identity("occupied", None).is_none());
                assert!(
                    state.background.values().any(|attachment| {
                        attachment.session_name.as_deref() == Some("previous")
                    }),
                    "previous stays parked for an explicit picker choice"
                );
                assert!(target_rx.try_iter().any(|message| matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::Detach)
                )));
            })
            .expect("spawn cancel attach test")
            .join()
            .expect("cancel attach test completes");
    }

    #[test]
    fn killing_the_last_attached_session_stays_sessionless_without_auto_attach() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::input::Action;
        use crate::session::bootstrap::has_session_candidates;
        use crate::session::client::SessionClient;
        use crate::state::ConnectionState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.config.confirm.kill_session = false;
                    state.current_mut().session_name = Some("solo".into());
                    state.current_mut().session_attached = true;
                    state.current_mut().connection = ConnectionState::Connected;
                    state.current_mut().session_client = Some(client);
                    state.current_mut().pending_session_attach = None;
                    state.background.clear();
                    state.host_session_cache.clear();
                    state.show_session_picker = false;
                    state.session_picker = None;
                }

                backend
                    .dispatch(Msg::RunAction(Action::KillSession))
                    .expect("kill last session");

                let state = backend.state();
                assert!(state.is_launcher());
                assert!(state.current().pending_session_attach.is_none());
                // Other sessions on the host machine still count as choices; only a truly empty
                // discovery set keeps the picker closed.
                assert_eq!(state.show_session_picker, has_session_candidates());
            })
            .expect("spawn last-session kill test")
            .join()
            .expect("last-session kill test completes");
    }

    #[test]
    fn starting_a_shell_from_the_picker_attaches_the_ephemeral_with_the_launch_seed() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::state::SessionPickerState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                {
                    let state = backend.state_mut();
                    // The startup picker parks the panes the launch prepared and leaves the
                    // foreground empty, which is the launcher the scratch key starts from.
                    let mut seed = crate::state::fresh_default_attachment(&state.config);
                    seed.workspaces[0].panes[0].identity.cwd = Some("/seeded".into());
                    *state.current_mut() = crate::state::Attachment::new();
                    state.launcher_seed = Some(seed);
                    state.show_session_picker = true;
                    state.session_picker =
                        Some(SessionPickerState::new(vec![session_row("dev", None)]));
                }
                assert!(backend.state().is_launcher());

                backend
                    .dispatch(Msg::SessionPickerEphemeral)
                    .expect("start a shell from the picker");

                let state = backend.state();
                assert!(!state.show_session_picker && state.session_picker.is_none());
                let pending = state
                    .current()
                    .pending_session_attach
                    .as_ref()
                    .expect("attaching the ephemeral session");
                assert_eq!(pending.name, crate::state::ephemeral_session_name());
                assert!(
                    state.launcher_seed.is_none(),
                    "the parked launch panes are consumed, not left for a second start"
                );
                assert_eq!(
                    state.current().workspaces[0].panes[0]
                        .identity
                        .cwd
                        .as_deref(),
                    Some("/seeded"),
                    "the shell starts with the layout the launch intended"
                );
            })
            .expect("spawn picker start-shell test")
            .join()
            .expect("picker start-shell test completes");
    }

    #[test]
    fn creating_a_session_with_an_existing_name_keeps_the_prompt_and_shows_an_inline_error() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::SessionClient;
        use crate::state::{ConnectionState, SessionRenameState};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    // Current attachment owns `dev`, so create must treat that name as taken.
                    state.current_mut().session_name = Some("dev".into());
                    state.current_mut().session_attached = true;
                    state.current_mut().connection = ConnectionState::Connected;
                    state.current_mut().session_client = Some(client);
                    state.current_mut().pending_session_attach = None;
                    state.overlay_return = Some(crate::state::OverlayOrigin::SessionPicker {
                        query: String::new(),
                        selected: 0,
                    });
                    state.rename_session =
                        Some(SessionRenameState::new("dev", NamingMode::CreateSession));
                }

                backend
                    .dispatch(Msg::SubmitRenameSession)
                    .expect("submit colliding create");

                let state = backend.state();
                let rename = state
                    .rename_session
                    .as_ref()
                    .expect("create prompt must stay open");
                assert_eq!(
                    rename.error.as_deref(),
                    Some("Session `dev` is already running")
                );
                assert_eq!(rename.input.text(), "dev");
                assert!(
                    state.current().pending_session_attach.is_none(),
                    "a rejected create must not start an attach"
                );
                assert_eq!(
                    state.current().session_name.as_deref(),
                    Some("dev"),
                    "the active session must stay put"
                );
                assert!(
                    state.overlay_return.is_some(),
                    "parent picker origin must survive a rejected create"
                );
            })
            .expect("spawn colliding-create test")
            .join()
            .expect("colliding-create test completes");
    }

    #[test]
    fn create_session_starts_fresh_instead_of_carrying_current_panes() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::state::SessionRenameState;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                // A profile name unlikely to exist on disk, so resolution must fall through to a
                // fresh empty session rather than a profile seed.
                let name = format!("create-fresh-{}", std::process::id());
                {
                    let state = backend.state_mut();
                    state.current_mut().session_name = Some("eph-test".to_string());
                    state.current_mut().session_attached = true;
                    state.current_mut().engaged = true;
                    state.current_mut().pending_session_attach = None;
                    state.sidebar.command_epoch = 7;
                    state.sidebar.config_epoch = 11;
                    // Client-global chrome that must survive a create: an open sidebar on a chosen
                    // tab, and live workbar command scheduling state.
                    state.sidebar_visible = true;
                    state.sidebar.panels[0].active_tab =
                        Some(crate::config::SidebarTabId::new("sessions"));
                    state.workbar.command_epoch = 3;
                    state
                        .workbar
                        .command_in_flight
                        .insert("date".to_string(), 3);
                    // Simulate a profile-seeded session: the current pane carries a command.
                    state.current_mut().workspaces[0].panes[0].identity.command =
                        Some("nvim".to_string());
                    state.rename_session =
                        Some(SessionRenameState::new(&name, NamingMode::CreateSession));
                }
                backend.render();
                // `update_level`, not `dispatch`: every assertion below is about what the create
                // installs synchronously, and no server is ever going to answer for this name.
                // `dispatch` drains until idle, so the create thread's fast failure could land in
                // the same pump and tear the pending attach back down before the test reads it.
                backend
                    .update_level(Msg::SubmitRenameSession)
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
                // flicker), the sidebar stays open on its tab, and workbar scheduling stays live.
                // This is the whole point of installing an attachment instead of rebuilding State.
                assert_eq!(state.sidebar.command_epoch, 7);
                assert_eq!(state.sidebar.config_epoch, 11);
                assert!(state.sidebar_visible);
                assert_eq!(
                    state.sidebar.active_tab(),
                    Some(&crate::config::SidebarTabId::new("sessions"))
                );
                assert_eq!(state.workbar.command_epoch, 3);
                assert_eq!(state.workbar.command_in_flight.get("date"), Some(&3));
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn controller_grant_control_key_grants_to_the_earliest_requester() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::input::Action;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
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
                        parked: false,
                    };
                    shared.clients = vec![
                        ClientInfo {
                            id: 1,
                            label: "me".into(),
                            read_only: false,
                            requesting_control: false,
                            parked: false,
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
            parked: false,
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

    /// Leaving a session for another one: whether the one being left is kept alive in the
    /// background. Driven through the create-session flow, the same path a switch takes.
    fn background_after_leaving_ephemeral(engaged: bool) -> bool {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::SessionClient;
        use crate::state::{NamingMode, SessionRenameState};
        use tui_lipan::TestBackend;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _outbound) = SessionClient::test_channel();
                let name = format!("leave-target-{}", std::process::id());
                {
                    let state = backend.state_mut();
                    state.current_mut().session_name = Some("eph-startup".to_string());
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    // What a bare launch produces: an ephemeral the client picked the name for.
                    state.current_mut().auto_created = true;
                    state.current_mut().engaged = engaged;
                    state.current_mut().pending_session_attach = None;
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    shared.clients = vec![ClientInfo {
                        id: 1,
                        label: "me".into(),
                        read_only: false,
                        requesting_control: false,
                        parked: false,
                    }];
                    state.current_mut().shared = Some(shared);
                    state.rename_session =
                        Some(SessionRenameState::new(&name, NamingMode::CreateSession));
                }
                backend.render();
                // Synchronous outcome only - see `create_session_starts_fresh…`: draining until
                // idle lets the create thread's failure restore the session this asserts was
                // parked.
                backend
                    .update_level(Msg::SubmitRenameSession)
                    .expect("dispatch create session");
                tx.send(!backend.state().background.is_empty())
                    .expect("report result");
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
        rx.recv().expect("test result")
    }

    /// The startup ephemeral is the session nobody asked for. Switching away from an untouched one
    /// must remove it, not leave it running where it later shows up as a session to confirm away.
    #[test]
    fn switching_away_discards_an_untouched_startup_ephemeral() {
        assert!(!background_after_leaving_ephemeral(false));
    }

    /// The same ephemeral, once worked in, is real work: switching away parks it so it can be
    /// switched back to.
    #[test]
    fn switching_away_parks_a_used_ephemeral() {
        assert!(background_after_leaving_ephemeral(true));
    }

    /// Attaching to a session that is already retained must retire the Profiles overlay the same way
    /// a launch does — otherwise Enter on a running profile leaves the picker covering the session
    /// that just came to the foreground.
    #[test]
    fn attaching_to_parked_session_closes_profile_picker() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::SessionClient;
        use crate::state::{Attachment, ConnectionState, ProfilePickerState};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (current_client, _current_rx) = SessionClient::test_channel();
                let (parked_client, _parked_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 1;
                    state.current_mut().epoch = 1;
                    state.current_mut().session_name = Some("other".into());
                    state.current_mut().session_attached = true;
                    state.current_mut().connection = ConnectionState::Connected;
                    state.current_mut().session_client = Some(current_client);

                    let mut parked = Attachment::new();
                    parked.epoch = 2;
                    parked.parked_seq = 1;
                    parked.session_name = Some("dev".into());
                    parked.session_attached = true;
                    parked.connection = ConnectionState::Connected;
                    parked.session_client = Some(parked_client);
                    state.background.insert(2, parked);

                    state.show_profile_picker = true;
                    state.profile_picker = Some(ProfilePickerState::new(Vec::new()));
                    state.session_picker =
                        Some(SessionPickerState::new(vec![session_row("dev", None)]));
                    state.show_session_picker = true;
                }

                backend
                    .dispatch(Msg::SessionPickerActivate(0))
                    .expect("attach to parked session");

                assert_eq!(
                    backend.state().current().session_name.as_deref(),
                    Some("dev")
                );
                assert!(!backend.state().show_profile_picker);
                assert!(backend.state().profile_picker.is_none());
                assert!(!backend.state().show_session_picker);
                assert!(backend.state().session_picker.is_none());
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    /// A cold attach (session running, not retained here) must dismiss Profiles as soon as the
    /// switch starts — not wait for `SessionAttached`, which left the overlay up over Connecting.
    #[test]
    fn cold_attach_closes_profile_picker_before_connect() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::SessionClient;
        use crate::state::{ConnectionState, ProfilePickerState};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.runtime_epoch = 1;
                    state.current_mut().epoch = 1;
                    state.current_mut().session_name = Some("other".into());
                    state.current_mut().session_attached = true;
                    state.current_mut().connection = ConnectionState::Connected;
                    state.current_mut().session_client = Some(client);
                    state.show_profile_picker = true;
                    state.profile_picker = Some(ProfilePickerState::new(Vec::new()));
                    state.session_picker =
                        Some(SessionPickerState::new(vec![session_row("dev", None)]));
                    state.show_session_picker = true;
                }

                // `update_level`, not `dispatch`: the assertions below are about the state the
                // switch leaves behind *before* it connects, and "dev" is not a session that
                // exists here. `dispatch` drains until idle, so the attach thread's fast failure
                // could land in the same pump and clear `pending_session_attach` out from under
                // the test - which it did, on roughly one run in five.
                backend
                    .update_level(Msg::SessionPickerActivate(0))
                    .expect("start cold attach");

                assert!(!backend.state().show_profile_picker);
                assert!(backend.state().profile_picker.is_none());
                assert!(
                    backend
                        .state()
                        .current()
                        .pending_session_attach
                        .as_ref()
                        .is_some_and(|pending| pending.name == "dev")
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn activating_restorable_session_autostarts_its_server() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::AppRoot;
                use crate::Msg;
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                let mut row = session_row("saved", None);
                row.status = crate::session::discovery::DiscoveredSessionStatus::Restorable;
                backend.state_mut().session_picker = Some(SessionPickerState::new(vec![row]));
                backend.state_mut().show_session_picker = true;

                backend
                    .update_level(Msg::SessionPickerActivate(0))
                    .expect("start snapshot restore");

                let pending = backend
                    .state()
                    .current()
                    .pending_session_attach
                    .as_ref()
                    .expect("snapshot attach");
                assert_eq!(pending.name, "saved");
                assert!(pending.autostart);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }
}
