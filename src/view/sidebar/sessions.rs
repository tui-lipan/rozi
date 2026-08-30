use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, SidebarRow};
use crate::AppRoot;
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use crate::session::remote::RemoteTarget;
use crate::state::{ConnectionState, HostEntry, HostStatus};

fn session_detail(entry: &DiscoveredSession) -> String {
    match &entry.status {
        DiscoveredSessionStatus::Running {
            panes,
            created_from_profile,
            ..
        } => {
            let mut detail = format!("{panes} pane{}", if *panes == 1 { "" } else { "s" });
            if let Some(profile) = created_from_profile {
                detail.push_str(&format!(" · from {profile}"));
            }
            detail
        }
        DiscoveredSessionStatus::Restorable => "restorable".to_string(),
        DiscoveredSessionStatus::Busy => "busy".to_string(),
        DiscoveredSessionStatus::Unknown => "incompatible or unavailable".to_string(),
    }
}

fn shared_client_count(entry: &DiscoveredSession, we_hold: bool) -> Option<u32> {
    match &entry.status {
        DiscoveredSessionStatus::Running { clients, .. } => {
            (clients.saturating_sub(u32::from(we_hold)) > 0).then_some(*clients)
        }
        _ => None,
    }
}

/// The connection state of every live attachment (current or retained) on `target`. Feeds the host
/// header's status dot, which describes the *host*, not any one session.
fn attachment_connections(ctx: &Context<AppRoot>, target: &RemoteTarget) -> Vec<ConnectionState> {
    std::iter::once(ctx.state.current())
        .chain(ctx.state.background.values())
        .filter(|attachment| attachment.remote_target.as_ref() == Some(target))
        .map(|attachment| attachment.connection)
        .collect()
}

/// One live session row: name, current/background/reconnecting state, panes, and origin.
fn session_row(ctx: &Context<AppRoot>, entry: &DiscoveredSession) -> SidebarRow {
    let current = ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
        && ctx.state.current().remote_host == entry.host
        && ctx.state.current().remote_target == entry.remote_target;
    let connection = ctx
        .state
        .attachment_by_identity(&entry.name, entry.remote_target.as_ref())
        .map(|attachment| attachment.connection);
    // We hold a connection to this session (current or retained), so discovery's client count
    // includes us. Show the total only when at least one other client is present.
    let we_hold = current || connection.is_some();
    let label = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    let muted = super::super::fg_only(&ctx.state.theme.muted);
    let status = crate::view::session_status::session_connection_status(current, connection);
    let styles = crate::view::session_status::SessionStatusStyles::from_theme(&ctx.state.theme);
    let mut row = Row::new(label)
        .active(current)
        .title_style(super::super::fg_only(&ctx.state.theme.primary));
    // Connection chrome+label owns the title-line badge when parked; otherwise a shared-client
    // count can use that slot. Hover ✕ still replaces whichever badge is showing.
    if let Some(badge) = crate::view::session_status::session_status_badge(status, styles) {
        row = row.badge(badge);
    } else if let Some(clients) = shared_client_count(entry, we_hold) {
        row = row.badge_text(format!("󰍺 {clients}"), muted);
    }
    let row = row.detail(session_detail(entry), muted);
    // The ✕ kills the session — shuts its server down, the same as the picker's `Ctrl+K`. Killing
    // the one on screen is fine; the UI lands on the picker or launcher rather than quitting.
    SidebarRow::item(row, RowTarget::Session(Box::new(entry.clone()))).closable(
        crate::state::SidebarClose::Session {
            name: entry.name.clone(),
            remote_target: entry.remote_target.clone(),
        },
    )
}

/// A cached (last-seen) session row for an offline host: muted, activatable — selecting it connects
/// to the host and attaches — so a host's known workplaces stay visible while it is offline.
///
/// Deliberately has no ✕: the host is offline, so there is nothing there to kill, and the row is a
/// memory of a session rather than a live one.
fn cached_session_row(
    ctx: &Context<AppRoot>,
    host: &HostEntry,
    cached: &crate::session::CachedHostSession,
) -> SidebarRow {
    let panes = format!(
        "{} pane{}",
        cached.panes,
        if cached.panes == 1 { "" } else { "s" }
    );
    let entry = DiscoveredSession {
        name: cached.name.clone(),
        ephemeral: false,
        host: Some(host.alias.clone()),
        remote_target: Some(host.target.clone()),
        status: DiscoveredSessionStatus::Running {
            panes: cached.panes,
            clients: 0,
            has_layout: false,
            created_from_profile: None,
        },
    };
    let muted = super::super::fg_only(&ctx.state.theme.muted);
    SidebarRow::item(
        Row::new(cached.name.clone())
            .title_style(muted)
            .detail(format!("{panes} · last seen"), muted),
        RowTarget::Session(Box::new(entry)),
    )
}

/// A section header (`LOCAL`, or a host alias) with an optional right-aligned status badge.
///
/// The badge says what state the host is in; the second line, when it has one, says why. What to
/// *do* about the state depends on whether that line is already spoken for:
///
/// - a connected host is one line, and its ✕ appears under the pointer;
/// - a host that failed spends its second line on the reason, so its connect affordance hides in
///   the badge's slot rather than growing the row to three lines;
/// - an offline host with nothing to explain has that line free, and a standing "Click to connect"
///   is plainer than one the pointer has to go looking for.
fn header_row(
    ctx: &Context<AppRoot>,
    label: &str,
    host: Option<(&HostEntry, HostStatus)>,
) -> SidebarRow {
    let theme = &ctx.state.theme;
    let mut row = Row::new(label.to_string())
        .group_level()
        .title_style(super::super::fg_only(&theme.accent).bold());
    let Some((host, status)) = host else {
        return SidebarRow::item(row, RowTarget::Inert);
    };
    row = row.badge(crate::view::session_status::host_status_badge(
        status,
        crate::view::session_status::HostStatusStyles::from_theme(theme),
    ));
    match status {
        HostStatus::Connected | HostStatus::Reachable => SidebarRow::item(row, RowTarget::Inert)
            .closable(crate::state::SidebarClose::Host {
                target: host.target.clone(),
            }),
        HostStatus::Disconnected | HostStatus::Unreachable => {
            // Only while unreachable: a live attachment outranks the probe in `status_for`, so a
            // host that failed once and is connected now would otherwise keep explaining a failure
            // that no longer describes anything on screen.
            let reason = (status == HostStatus::Unreachable)
                .then(|| host.probe.error())
                .flatten();
            row = match reason {
                // The reason has taken the second line, and a row that is already two lines high
                // must not grow a third — so the affordance hides in the badge's slot, muted,
                // reading as the same quiet chrome as the word it replaces.
                Some(error) => row
                    .detail(
                        crate::session::discovery::probe_failure_reason(error),
                        Style::new().fg(theme.status.error),
                    )
                    .hover_badge_text("Connect", super::super::fg_only(&theme.muted)),
                // Nothing to explain, so the second line is free. Spend it: a standing invitation
                // is plainer than one the pointer has to go looking for.
                None => row.detail("Click to connect", super::super::fg_only(&theme.muted)),
            };
            SidebarRow::item(row, RowTarget::HostConnect(host.target.clone()))
        }
        HostStatus::Connecting => SidebarRow::item(row, RowTarget::Inert),
    }
}

/// The muted "nothing here" line for a group with no sessions to list.
fn empty_row(ctx: &Context<AppRoot>, text: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(text).title_style(super::super::fg_only(&ctx.state.theme.muted)),
        RowTarget::Inert,
    )
}

/// A child-level "＋ …" action row within a session group.
fn session_action_row(ctx: &Context<AppRoot>, label: &str, target: RowTarget) -> SidebarRow {
    let style = super::super::fg_only(&ctx.state.theme.accent);
    SidebarRow::item(Row::new(format!("+ {label}")).title_style(style), target)
}

/// A group-level "＋ …" action row (connect a host).
fn action_row(ctx: &Context<AppRoot>, label: &str, target: RowTarget) -> SidebarRow {
    let style = super::super::fg_only(&ctx.state.theme.accent);
    SidebarRow::item(
        Row::new(label)
            .glyph(Text::new("+").style(style))
            .title_style(style),
        target,
    )
}

pub(super) fn sessions_rows(ctx: &Context<AppRoot>) -> Vec<SidebarRow> {
    let mut rows = Vec::new();

    // Local group: always present, always available.
    rows.push(header_row(ctx, "LOCAL", None));
    let mut any_local = false;
    for entry in ctx
        .state
        .sidebar
        .sessions
        .iter()
        .filter(|e| e.host.is_none())
    {
        rows.push(session_row(ctx, entry));
        any_local = true;
    }
    if !any_local {
        rows.push(empty_row(ctx, "No local sessions"));
    }
    rows.push(session_action_row(
        ctx,
        "New session",
        RowTarget::NewSession(None),
    ));

    // One group per known remote host — configured, recently used, or currently attached — so a
    // host stays listed even while offline. Connecting a host lists its sessions; disconnecting
    // returns it to offline.
    for host in ctx.state.hosts.iter() {
        rows.push(SidebarRow::spacer());
        let sessions: Vec<&DiscoveredSession> = ctx
            .state
            .sidebar
            .sessions
            .iter()
            .filter(|e| e.remote_target.as_ref() == Some(&host.target))
            .collect();
        let status = ctx.state.hosts.status_for(
            &host.target,
            attachment_connections(ctx, &host.target).iter(),
            !sessions.is_empty(),
        );
        rows.push(header_row(
            ctx,
            &host.alias.to_uppercase(),
            Some((host, status)),
        ));

        match status {
            HostStatus::Connecting => {}
            HostStatus::Connected | HostStatus::Reachable => {
                // Online: live sessions follow the host row, then the way to start another.
                if sessions.is_empty() {
                    rows.push(empty_row(ctx, "No sessions here yet"));
                } else {
                    for entry in sessions {
                        rows.push(session_row(ctx, entry));
                    }
                }
                rows.push(session_action_row(
                    ctx,
                    "New session",
                    RowTarget::NewSession(Some(host.target.clone())),
                ));
            }
            HostStatus::Disconnected | HostStatus::Unreachable => {
                // Offline: the host row connects it; last-seen sessions remain visible from cache.
                if let Some(cached) =
                    crate::session::host_sessions_for(&ctx.state.host_session_cache, &host.target)
                {
                    for entry in cached.iter().filter(|s| !s.ephemeral) {
                        rows.push(cached_session_row(ctx, host, entry));
                    }
                }
            }
        }
    }

    // A discoverable path to the connect-remote-host prompt, so a host that is not yet configured or
    // recent can still be reached without knowing the Ctrl+R binding.
    rows.push(SidebarRow::spacer());
    rows.push(action_row(ctx, "Connect a host…", RowTarget::ConnectHost));

    rows
}
