use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::update::sidebar::polling::{SESSION_REFRESH_INTERVAL, sessions_active};

/// Nudge the open Sessions tab to re-sweep now (e.g. after a config reload or a session change),
/// without disturbing the epoch. The steady-state loop is kept alive by [`ensure_sessions_refresh_armed`];
/// this just kicks an extra immediate sweep for the current epoch.
pub(crate) fn request_sessions_refresh(ctx: &Context<AppRoot>) {
    if sessions_active(ctx)
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        link.send(crate::Msg::SidebarSessionsRefresh {
            epoch: ctx.state.sidebar.sessions_epoch,
        });
    }
}

/// Keep the Sessions tab's auto-refresh loop alive. Called from the post-update chokepoint after
/// every message: if the tab is active but the loop's epoch has fallen behind (a session switch,
/// create, or reopen bumped `sessions_epoch` and killed the old loop), re-arm it. The armed-epoch
/// guard makes this fire exactly once per epoch, so it never stacks parallel loops.
pub(crate) fn ensure_sessions_refresh_armed(ctx: &mut Context<AppRoot>) {
    if !sessions_active(ctx) {
        ctx.state.sidebar.sessions_refresh_armed_epoch = None;
        return;
    }
    let epoch = ctx.state.sidebar.sessions_epoch;
    if ctx.state.sidebar.sessions_refresh_armed_epoch == Some(epoch) {
        return;
    }
    // Only mark the epoch armed once we can actually kick the loop, so a missing link (very early
    // startup) retries on the next message instead of latching a loop that never started.
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    ctx.state.sidebar.sessions_refresh_armed_epoch = Some(epoch);
    // Fill the tab instantly with local rows + known hosts when it would otherwise be blank (the
    // epoch bump on a switch clears the list), so it never flashes empty while the async sweep runs.
    if ctx.state.sidebar.sessions.is_empty() {
        ctx.state.sidebar.sessions = crate::ops::session::local_picker_rows(ctx);
    }
    crate::ops::session::seed_host_registry(ctx);
    link.send(crate::Msg::SidebarSessionsRefresh { epoch });
}

pub(crate) fn open_sessions(ctx: &mut Context<AppRoot>) {
    // Populate the tab instantly with local rows, then run the full sweep (configured remote hosts
    // included) off the UI thread. Querying remote hosts over ssh here used to block the tab switch
    // on a round-trip — or the whole connect timeout when a host was down — every time it opened.
    ctx.state.sidebar.sessions = crate::ops::session::local_picker_rows(ctx);
    crate::ops::session::seed_host_registry(ctx);
}

pub(crate) fn refresh_sessions(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    // The loop is now live for this epoch, so the post-update chokepoint won't kick a duplicate.
    ctx.state.sidebar.sessions_refresh_armed_epoch = Some(epoch);
    // Only a *local* current session is excluded from the local scan; see
    // [`crate::state::State::local_current_session_name`].
    let current_name = ctx.state.local_current_session_name().map(str::to_string);
    let attached = crate::ops::session::attached_session_rows(&ctx.state);
    let remote_config = ctx.state.config.remote.clone();
    // On-demand: only *connected* hosts are contacted over ssh — those the user connected, or that
    // already hold an attachment. `Idle` is the disconnected state and is never probed, so the sweep
    // touches nothing the user has not asked for.
    //
    // A failed probe keeps being retried, because connecting is an intent the user expressed and a
    // failure is just this sweep's outcome. Dropping a failed host from the sweep meant one blip —
    // a laptop lid, a VPN reconnect — demoted a connected host to Offline permanently, with its
    // sessions gone until it was connected by hand again.
    let probe_targets: Vec<crate::session::remote::RemoteTarget> = ctx
        .state
        .hosts
        .iter()
        .filter(|host| {
            !matches!(host.probe, crate::state::HostProbe::Idle)
                || ctx
                    .state
                    .background
                    .values()
                    .chain(std::iter::once(ctx.state.current()))
                    .any(|attachment| attachment.remote_target.as_ref() == Some(&host.target))
        })
        .map(|host| host.target.clone())
        .collect();
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let (rows, host_status) = crate::ops::session::discover_sidebar_sessions(
            current_name.as_deref(),
            &remote_config,
            probe_targets,
            attached,
        );
        link.send(crate::Msg::SidebarSessionsDiscovered {
            epoch,
            rows: rows.map_err(|error| error.to_string()),
            host_status,
        });
    }))
}

pub(crate) fn sessions_discovered(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    rows: std::result::Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
    host_status: Vec<(crate::session::remote::RemoteTarget, Option<String>)>,
) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    if let Ok(rows) = rows {
        ctx.state.sidebar.sessions = rows;
    }
    crate::ops::session::seed_host_registry(ctx);
    // Apply fresh probe outcomes after the reseed so they win over the carried-over state: a host
    // that answered reads as reached (online), a host that failed shows why on its group header.
    // A host that answered also refreshes its persisted session cache (written only when it
    // changed, so a steady 1.5s sweep does not churn the disk).
    for (target, status) in host_status {
        if status.is_none() {
            let label = target.display_label();
            let sessions: Vec<crate::session::CachedHostSession> = ctx
                .state
                .sidebar
                .sessions
                .iter()
                .filter(|entry| entry.remote_target.as_ref() == Some(&target))
                .map(|entry| crate::session::CachedHostSession {
                    name: entry.name.clone(),
                    ephemeral: entry.ephemeral,
                    panes: match &entry.status {
                        crate::session::discovery::DiscoveredSessionStatus::Running {
                            panes,
                            ..
                        } => *panes,
                        _ => 0,
                    },
                })
                .collect();
            // Only persist a real change, and never write an empty list for a host that never had
            // one cached — there is nothing to remember, and it keeps the sweep from creating a
            // file on the first probe of a session-less host.
            let known = ctx.state.host_session_cache.contains_key(&label);
            if (!sessions.is_empty() || known)
                && ctx.state.host_session_cache.get(&label) != Some(&sessions)
            {
                crate::session::record_host_sessions(&label, sessions.clone());
                ctx.state.host_session_cache.insert(label, sessions);
            }
        }
        if let Some(entry) = ctx.state.hosts.get_mut(&target) {
            entry.probe = match status {
                Some(error) => crate::state::HostProbe::Failed(error),
                None => crate::state::HostProbe::Reached,
            };
        }
    }
    Update::with_command(Command::after(
        SESSION_REFRESH_INTERVAL,
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::SidebarSessionsRefresh { epoch });
        },
    ))
}

pub(crate) fn activate_session(
    ctx: &mut Context<AppRoot>,
    entry: crate::session::discovery::DiscoveredSession,
) -> Update {
    crate::ops::session::activate_discovered_session(ctx, entry)
}

/// "Click to connect": bring a host online. Mark its probe in flight (so the header reads
/// "Connecting…" at once) and bump the sessions epoch so the post-update chokepoint re-sweeps with
/// this host now included, probing it immediately rather than at the next periodic tick.
pub(crate) fn connect_host(
    ctx: &mut Context<AppRoot>,
    target: crate::session::remote::RemoteTarget,
) -> Update {
    let Some(entry) = ctx.state.hosts.get_mut(&target) else {
        return Update::none();
    };
    if matches!(entry.probe, crate::state::HostProbe::InFlight) {
        return Update::none();
    }
    entry.probe = crate::state::HostProbe::InFlight;
    ctx.state.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
    Update::full()
}

/// "Click to disconnect": the first activation arms a confirmation (`armed` is what the row was
/// showing); the second commits it. Disconnecting closes any live attachments to the host — their
/// servers keep running — and returns it to offline.
pub(crate) fn disconnect_host(
    ctx: &mut Context<AppRoot>,
    target: crate::session::remote::RemoteTarget,
    armed: Option<crate::session::remote::RemoteTarget>,
) -> Update {
    if armed.as_ref() != Some(&target) {
        // Arm: the render turns the row red and reads "Click again to confirm".
        ctx.state.sidebar.pending_host_disconnect = Some(target);
        return crate::ops::confirm::arm(ctx);
    }
    // The update is the disconnect's *result*, not a repaint hint: when the current session lived on
    // this host it carries the command that lands the user somewhere else — an attach round-trip for
    // a fresh ephemeral, or a reconnect for the session being switched to. Dropping it left the UI
    // holding an attachment marked `Connecting` with a pending attach that nothing would ever
    // complete: an empty workspace, a phantom pane, and every later session activation refused as
    // "attach already in progress".
    let landed = crate::ops::session::disconnect_host(ctx, &target);
    ctx.state.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
    landed
}
