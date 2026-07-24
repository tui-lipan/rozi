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

/// The connection state of every live attachment (current or retained) on `target`. Feeds the
/// host-group status dot, which describes the *host*, not any one session.
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

/// The dot glyph, label, and text style for a host's derived status.
fn status_face(theme: &Theme, status: HostStatus) -> (&'static str, &'static str, Style) {
    match status {
        HostStatus::Connected => ("●", "connected", Style::new().fg(theme.status.success)),
        HostStatus::Connecting => ("◌", "connecting…", Style::new().fg(theme.status.warning)),
        HostStatus::Reachable => ("●", "online", Style::new().fg(theme.status.info)),
        HostStatus::Disconnected => ("○", "disconnected", super::super::fg_only(&theme.muted)),
        HostStatus::Unreachable => ("⊘", "unreachable", Style::new().fg(theme.status.error)),
    }
}

/// One session row: name, current/attached/reconnecting/offline state, panes, and origin.
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

/// A remote-host group header: a caret, the host alias, and a status dot. Selecting it toggles the
/// group. Kept as a selectable row (unlike the inert `LOCAL` header) so a host with no live sessions
/// is still a place the keyboard can land and expand.
fn host_header_row(ctx: &Context<HyprmuxApp>, host: &HostEntry, status: HostStatus) -> SidebarRow {
    let theme = &ctx.state.theme;
    let (dot, label, color) = status_face(theme, status);
    let caret = if host.expanded { "▾" } else { "▸" };
    let mut row = Row::new(host.alias.clone())
        .glyph(Text::new(caret).style(super::super::fg_only(&theme.muted)))
        .title_style(super::super::fg_only(&theme.accent).bold())
        .badge(format!("{dot} {label}"), color);
    // Only a real error adds a second line — an empty detail would still cost the row its height.
    if let Some(error) = host.probe.error() {
        row = row.detail(error.to_string(), Style::new().fg(theme.status.error));
    }
    SidebarRow::item(row, RowTarget::HostToggle(host.target.clone()))
}

/// A cached (last-seen) session row for an offline host: muted, tagged "offline (cached)", and
/// activatable — selecting it connects to the host and attaches. Built from the persisted cache so
/// a host's known workplaces stay visible while it is unreachable.
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
            .detail(format!("offline · {panes} · cached"), muted),
        RowTarget::Session(Box::new(entry)),
    )
}

/// The muted placeholder shown inside an expanded host group that has no sessions to list.
fn host_placeholder(ctx: &Context<HyprmuxApp>, status: HostStatus) -> SidebarRow {
    // The header caret + status dot already say whether we are connecting/online/unreachable, and
    // the "New session on <host>" action row below is the way to start one — so this line only
    // names the empty state, never claims that selecting the host header connects (it toggles).
    let text = match status {
        HostStatus::Connecting => "Connecting…",
        HostStatus::Connected | HostStatus::Reachable => "No sessions here yet",
        HostStatus::Unreachable => "Host unreachable",
        HostStatus::Disconnected => "Not connected",
    };
    SidebarRow::item(
        Row::new(text).title_style(super::super::fg_only(&ctx.state.theme.muted)),
        RowTarget::Inert,
    )
}

/// A muted "＋ …" action row (new session, connect a host). Rendered like a session row so it reads
/// as part of the tree, but styled subdued so it never competes with a real session.
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
    let theme = &ctx.state.theme;
    let mut rows = Vec::new();

    // Local group is always present and always expanded — local sessions are instant and never
    // gated behind a connection.
    rows.push(SidebarRow::header(
        Text::new("LOCAL").style(super::super::fg_only(&theme.muted).bold()),
    ));
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
        rows.push(SidebarRow::item(
            Row::new("No local sessions").title_style(super::super::fg_only(&theme.muted)),
            RowTarget::Inert,
        ));
    }
    rows.push(action_row(ctx, "New session", RowTarget::NewSession(None)));

    // One collapsible group per known remote host — configured, recently used, or currently
    // attached — so a host stays listed even while disconnected or empty.
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
        rows.push(host_header_row(ctx, host, status));
        if !host.expanded {
            continue;
        }
        if !sessions.is_empty() {
            for entry in sessions {
                rows.push(session_row(ctx, entry));
            }
        } else {
            // No live sessions: fall back to the persisted cache so an offline host still shows the
            // workplaces it had. Ephemeral (disposable) sessions are never offered for reconnect.
            let cached: Vec<&crate::session::CachedHostSession> = ctx
                .state
                .host_session_cache
                .get(&host.target.display_label())
                .map(|list| list.iter().filter(|s| !s.ephemeral).collect())
                .unwrap_or_default();
            if cached.is_empty() {
                rows.push(host_placeholder(ctx, status));
            } else {
                for entry in cached {
                    rows.push(cached_session_row(ctx, host, entry));
                }
            }
        }
        rows.push(action_row(
            ctx,
            &format!("New session on {}", host.alias),
            RowTarget::NewSession(Some(host.target.clone())),
        ));
    }

    // A discoverable path to the connect-remote-host prompt, so a host that is not yet configured or
    // recent can still be reached without knowing the Ctrl+R binding.
    rows.push(SidebarRow::spacer());
    rows.push(action_row(ctx, "Connect a host…", RowTarget::ConnectHost));

    rows
}
