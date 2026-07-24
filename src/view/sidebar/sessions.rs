use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use crate::session::remote::RemoteTarget;
use crate::state::{ConnectionState, HostEntry, HostStatus};

fn session_detail(entry: &DiscoveredSession, we_hold: bool) -> String {
    match &entry.status {
        DiscoveredSessionStatus::Running {
            panes,
            clients,
            created_from_profile,
            ..
        } => {
            let mut detail = format!("{panes} pane{}", if *panes == 1 { "" } else { "s" });
            // `clients` includes our own connection when we hold one; drop it so this counts only
            // other people sharing the session.
            let others = clients.saturating_sub(u32::from(we_hold));
            if others > 0 {
                detail.push_str(&format!(
                    " · shared with {others} other{}",
                    if others == 1 { "" } else { "s" }
                ));
            }
            if let Some(profile) = created_from_profile {
                detail.push_str(&format!(" · from {profile}"));
            }
            detail
        }
        DiscoveredSessionStatus::Busy => "busy".to_string(),
        DiscoveredSessionStatus::Unknown => "incompatible or unavailable".to_string(),
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
    // We hold a connection to this session (current or retained) — used to drop our own client from
    // the "shared with N others" count and to prefix the retained state.
    let we_hold = current || connection.is_some();
    let label = if entry.ephemeral {
        "ephemeral".to_string()
    } else {
        entry.name.clone()
    };
    let detail = session_detail(entry, we_hold);
    let row = Row::new(label)
        .active(current)
        .title_style(super::super::fg_only(&ctx.state.theme.primary))
        .detail(
            match (current, connection) {
                // Attached but not the session on screen: our connection is kept in the background.
                (false, Some(ConnectionState::Connected)) => format!("background · {detail}"),
                (false, Some(ConnectionState::Connecting | ConnectionState::Reconnecting)) => {
                    format!("reconnecting · {detail}")
                }
                (false, Some(_)) => format!("offline · {detail}"),
                _ => detail,
            },
            super::super::fg_only(&ctx.state.theme.muted),
        );
    SidebarRow::item(row, RowTarget::Session(Box::new(entry.clone())))
}

/// A cached (last-seen) session row for an offline host: muted, activatable — selecting it connects
/// to the host and attaches — so a host's known workplaces stay visible while it is offline.
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

/// A section header (`LOCAL`, or a host alias) with an optional right-aligned status badge and an
/// inline error line. Inert — the connect/disconnect action lives on its own row beneath it.
fn header_row(ctx: &Context<HyprmuxApp>, label: &str, status: Option<HostStatus>) -> SidebarRow {
    let theme = &ctx.state.theme;
    let mut row =
        Row::new(label.to_string()).title_style(super::super::fg_only(&theme.muted).bold());
    if let Some(status) = status {
        let (dot, text, color) = status_face(theme, status);
        row = row.badge(format!("{dot} {text}"), color);
    }
    SidebarRow::item(row, RowTarget::Inert)
}

/// The muted "nothing here" line for a group with no sessions to list.
fn empty_row(ctx: &Context<HyprmuxApp>, text: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(text).title_style(super::super::fg_only(&ctx.state.theme.muted)),
        RowTarget::Inert,
    )
}

/// A muted "＋ …" action row (new session, connect a host).
fn action_row(ctx: &Context<HyprmuxApp>, label: &str, target: RowTarget) -> SidebarRow {
    let style = super::super::fg_only(&ctx.state.theme.accent);
    SidebarRow::item(
        Row::new(label)
            .glyph(Text::new("+").style(style))
            .title_style(style),
        target,
    )
}

/// "Click to connect" — the connect action for an offline host.
fn connect_row(ctx: &Context<HyprmuxApp>, host: &HostEntry) -> SidebarRow {
    SidebarRow::item(
        Row::new("Click to connect").title_style(super::super::fg_only(&ctx.state.theme.accent)),
        RowTarget::HostConnect(host.target.clone()),
    )
}

/// "Click to disconnect" — the disconnect action for an online host, with the app's click-again
/// confirmation: the first click arms it (the row turns red and reads "Click again to confirm"),
/// the second commits.
fn disconnect_row(ctx: &Context<HyprmuxApp>, host: &HostEntry) -> SidebarRow {
    let armed = ctx.state.sidebar.pending_host_disconnect.as_ref() == Some(&host.target);
    let (label, style) = if armed {
        (
            "Click again to confirm",
            Style::new().fg(ctx.state.theme.status.error),
        )
    } else {
        (
            "Click to disconnect",
            super::super::fg_only(&ctx.state.theme.muted),
        )
    };
    SidebarRow::item(
        Row::new(label).title_style(style),
        RowTarget::HostDisconnect(host.target.clone()),
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
    rows.push(action_row(ctx, "New session", RowTarget::NewSession(None)));

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
        rows.push(header_row(ctx, &host.alias.to_uppercase(), Some(status)));
        if let Some(error) = host.probe.error() {
            rows.push(empty_row_error(ctx, error));
        }

        match status {
            HostStatus::Connecting => rows.push(empty_row(ctx, "Connecting…")),
            HostStatus::Connected | HostStatus::Reachable => {
                // Online: the disconnect action sits right under the header, then the live sessions,
                // then the way to start another.
                rows.push(disconnect_row(ctx, host));
                if sessions.is_empty() {
                    rows.push(empty_row(ctx, "No sessions here yet"));
                } else {
                    for entry in sessions {
                        rows.push(session_row(ctx, entry));
                    }
                }
                rows.push(action_row(
                    ctx,
                    "New session",
                    RowTarget::NewSession(Some(host.target.clone())),
                ));
            }
            HostStatus::Disconnected | HostStatus::Unreachable => {
                // Offline: connect, then the host's last-seen sessions (from the cache) so its
                // known workplaces stay visible without contacting it.
                rows.push(connect_row(ctx, host));
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

/// The inline error line under a host that failed to connect.
fn empty_row_error(ctx: &Context<HyprmuxApp>, error: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(error.to_string()).title_style(Style::new().fg(ctx.state.theme.status.error)),
        RowTarget::Inert,
    )
}
