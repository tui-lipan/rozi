use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;
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
fn attachment_connections(
    ctx: &Context<HyprmuxApp>,
    target: &RemoteTarget,
) -> Vec<ConnectionState> {
    std::iter::once(ctx.state.current())
        .chain(ctx.state.background.values())
        .filter(|attachment| attachment.remote_target.as_ref() == Some(target))
        .map(|attachment| attachment.connection)
        .collect()
}

/// The dot glyph, label, and text style for a host's status. Collapsed to the two states that
/// matter to the user — Online / Offline — plus the transient Connecting.
fn status_face(theme: &Theme, status: HostStatus) -> (&'static str, &'static str, Style) {
    match status {
        HostStatus::Connected => ("●", "Online", Style::new().fg(theme.status.success)),
        HostStatus::Reachable => ("●", "Online", Style::new().fg(theme.status.info)),
        HostStatus::Connecting => ("◌", "Connecting…", Style::new().fg(theme.status.warning)),
        HostStatus::Disconnected => ("○", "Offline", super::super::fg_only(&theme.muted)),
        HostStatus::Unreachable => ("○", "Offline", Style::new().fg(theme.status.error)),
    }
}

/// One live session row: name, current/background/reconnecting state, panes, and origin.
fn session_row(ctx: &Context<HyprmuxApp>, entry: &DiscoveredSession) -> SidebarRow {
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
    // the one on screen is fine; the UI hops onto a fresh ephemeral session rather than quitting.
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
    ctx: &Context<HyprmuxApp>,
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
/// A remote host is one two-line row: its connect/disconnect description is the detail line, so the
/// row list wraps the title and description in one `MouseRegion`. `LOCAL` remains a one-line,
/// inert header.
fn header_row(
    ctx: &Context<HyprmuxApp>,
    label: &str,
    remote: Option<(HostStatus, &RemoteTarget)>,
) -> SidebarRow {
    let theme = &ctx.state.theme;
    let mut row = Row::new(label.to_string())
        .group_level()
        .title_style(super::super::fg_only(&theme.accent).bold());
    let mut target = RowTarget::Inert;
    if let Some((status, remote_target)) = remote {
        let (dot, text, color) = status_face(theme, status);
        row = row.badge_text(format!("{dot} {text}"), color);
        let muted = super::super::fg_only(&theme.muted);
        let (description, description_style, row_target) = match status {
            HostStatus::Connected | HostStatus::Reachable => {
                let armed =
                    ctx.state.sidebar.pending_host_disconnect.as_ref() == Some(remote_target);
                if armed {
                    (
                        "Click again to confirm",
                        Style::new().fg(theme.status.error),
                        RowTarget::HostDisconnect(remote_target.clone()),
                    )
                } else {
                    (
                        "Click to disconnect",
                        muted,
                        RowTarget::HostDisconnect(remote_target.clone()),
                    )
                }
            }
            HostStatus::Disconnected | HostStatus::Unreachable => (
                "Click to connect",
                muted,
                RowTarget::HostConnect(remote_target.clone()),
            ),
            HostStatus::Connecting => ("Connecting…", muted, RowTarget::Inert),
        };
        row = row.detail(description, description_style);
        target = row_target;
    }
    SidebarRow::item(row, target)
}

/// The muted "nothing here" line for a group with no sessions to list.
fn empty_row(ctx: &Context<HyprmuxApp>, text: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(text).title_style(super::super::fg_only(&ctx.state.theme.muted)),
        RowTarget::Inert,
    )
}

/// A child-level "＋ …" action row within a session group.
fn session_action_row(ctx: &Context<HyprmuxApp>, label: &str, target: RowTarget) -> SidebarRow {
    let style = super::super::fg_only(&ctx.state.theme.accent);
    SidebarRow::item(Row::new(format!("+ {label}")).title_style(style), target)
}

/// A group-level "＋ …" action row (connect a host).
fn action_row(ctx: &Context<HyprmuxApp>, label: &str, target: RowTarget) -> SidebarRow {
    let style = super::super::fg_only(&ctx.state.theme.accent);
    SidebarRow::item(
        Row::new(label)
            .glyph(Text::new("+").style(style))
            .title_style(style),
        target,
    )
}

pub(super) fn sessions_rows(ctx: &Context<HyprmuxApp>) -> Vec<SidebarRow> {
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
            Some((status, &host.target)),
        ));
        if let Some(error) = host.probe.error() {
            rows.push(empty_row_error(ctx, error));
        }

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
                if let Some(cached) = ctx
                    .state
                    .host_session_cache
                    .get(&host.target.display_label())
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

/// The inline reason under a host that failed to connect: a short phrase naming what to go fix, not
/// the ssh/plumbing message behind it — see [`crate::session::discovery::probe_failure_reason`].
fn empty_row_error(ctx: &Context<HyprmuxApp>, error: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(crate::session::discovery::probe_failure_reason(error))
            // Aligned with the host row above rather than indented under it: this says something
            // about the host itself, not about one of the sessions it would have listed.
            .group_level()
            .title_style(Style::new().fg(ctx.state.theme.status.error)),
        RowTarget::Inert,
    )
}
