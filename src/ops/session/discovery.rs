use std::time::Duration;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::Msg;
use crate::session::discovery::DiscoveredSession;

pub(crate) const SESSION_PICKER_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);

/// Fast, local-only rows used by the picker and Sessions sidebar: local named sessions plus the
/// attached session, with no remote ssh.
pub(crate) fn local_picker_rows(ctx: &Context<AppRoot>) -> Vec<DiscoveredSession> {
    let current_name = ctx.state.local_current_session_name();
    let mut rows =
        crate::session::discovery::discover_selectable_sessions(current_name).unwrap_or_default();
    push_attached_session_rows(ctx, &mut rows);
    rows
}

pub(crate) fn immediate_picker_rows(ctx: &mut Context<AppRoot>) -> Vec<DiscoveredSession> {
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

pub(crate) fn session_watch_command(
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
pub(crate) type SidebarDiscovery = (std::io::Result<Vec<DiscoveredSession>>, HostProbeStatus);

/// Every configured remote target: `[remote.hosts.*]` aliases plus `default_host`.
pub(crate) fn configured_targets(
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
pub(crate) fn probe_remote_targets_reporting(
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
pub(crate) fn cached_sessions_for_target(
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
pub(crate) fn push_cached_configured_remote_rows(
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
pub(crate) fn merge_current_session_row(rows: &mut Vec<DiscoveredSession>, current: DiscoveredSession) {
    let already = rows
        .iter()
        .any(|row| row.name == current.name && row.remote_target == current.remote_target);
    if !already {
        rows.push(current);
    }
}

pub(crate) fn sort_session_rows(rows: &mut [DiscoveredSession]) {
    rows.sort_by(|a, b| match (a.host.as_deref(), b.host.as_deref()) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (a_host, b_host) => a_host.cmp(&b_host).then_with(|| a.name.cmp(&b.name)),
    });
}

/// Append rows for every current or retained attachment. This keeps ad-hoc remotes selectable even
/// when they are not part of configured-host discovery.
pub(crate) fn push_attached_session_rows(ctx: &Context<AppRoot>, rows: &mut Vec<DiscoveredSession>) {
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

pub(crate) fn attachment_session_row(attachment: &crate::state::Attachment) -> Option<DiscoveredSession> {
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
