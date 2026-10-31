use tui_lipan::prelude::*;

use super::row::{Priority, Row, RowTarget, Shape, SidebarRow};
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use crate::state::{HostEntry, HostStatus, State};
use crate::view::vocabulary::{self, SessionPresence};

/// One live session row: name, its standing with this client, then panes, agents, and origin.
fn session_row(state: &State, entry: &DiscoveredSession) -> SidebarRow {
    let presence =
        vocabulary::session_presence_for(state, &entry.name, entry.remote_target.as_ref())
            .discovered(&entry.status);
    let muted = super::super::fg_only(&state.theme.muted);
    let label = if entry.ephemeral {
        vocabulary::EPHEMERAL.to_string()
    } else {
        entry.name.clone()
    };
    // The word, then how many other clients share the session. Who they are and what they do is
    // the picker's business; the sidebar only says that someone else is here. The word stands
    // without its picker marker: at the narrowest width the marker's two cells would clip it.
    let others = vocabulary::other_clients(entry, presence);
    let shared = (others > 0).then(|| format!("{}{others}", state.config.shared_clients_icon()));
    let badge = vocabulary::fields([presence.label(), shared.as_deref().unwrap_or_default()]);
    let mut row = Row::new(label)
        .priority(Priority::Balanced)
        .active(presence == SessionPresence::Current)
        .title_style(super::super::fg_only(&state.theme.primary))
        .badge_text(badge, muted);
    // The badge already counts the other clients; the detail carries the rest.
    let agents = vocabulary::host_agent_label(state, entry);
    let detail = vocabulary::session_facts(entry, agents.as_deref(), 0);
    if !detail.is_empty() {
        row = row.detail(detail, muted);
    }
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
/// Deliberately has no ✕: there is no live server to kill. The session picker forgets this cache
/// entry with Ctrl+K; a later probe that still sees the session lists it again.
fn cached_session_row(
    state: &State,
    host: &HostEntry,
    cached: &crate::session::CachedHostSession,
) -> SidebarRow {
    let entry = DiscoveredSession {
        name: cached.name.clone(),
        origin: Default::default(),
        ephemeral: false,
        host: Some(host.alias.clone()),
        remote_target: Some(host.target.clone()),
        status: DiscoveredSessionStatus::LastSeen {
            panes: cached.panes,
        },
    };
    let muted = super::super::fg_only(&state.theme.muted);
    // A cached row is a memory of a session on a host nothing can reach, so there is no monitor
    // snapshot to draw an agent token from — the pane count alone is the whole line.
    let detail = vocabulary::session_facts(&entry, None, 0);
    SidebarRow::item(
        Row::new(cached.name.clone())
            .title_style(muted)
            .badge_text(SessionPresence::LastSeen.label(), muted)
            .detail(detail, muted),
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
/// - an offline host with nothing to explain has that line free, and a standing "click to connect"
///   is plainer than one the pointer has to go looking for.
fn header_row(state: &State, label: &str, host: Option<(&HostEntry, HostStatus)>) -> SidebarRow {
    let theme = &state.theme;
    let mut row = Row::new(label.to_string()).title_style(super::super::rozi_fg(theme).bold());
    let Some((host, status)) = host else {
        return SidebarRow::item(row, RowTarget::Inert);
    };
    row = row.badge(vocabulary::host_status_badge(
        status,
        vocabulary::HostStatusStyles::from_theme(theme),
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
                    .hover_badge_text("connect", super::super::fg_only(&theme.muted)),
                // Nothing to explain, so the second line is free. Spend it: a standing invitation
                // is plainer than one the pointer has to go looking for.
                None => row.detail("click to connect", super::super::fg_only(&theme.muted)),
            };
            let row = SidebarRow::item(row, RowTarget::HostConnect(host.target.clone()));
            if crate::ops::session::host_can_disconnect(state, &host.target) {
                row.closable(crate::state::SidebarClose::Host {
                    target: host.target.clone(),
                })
            } else {
                row
            }
        }
        HostStatus::Connecting | HostStatus::Installing => {
            let row = SidebarRow::item(row, RowTarget::Inert);
            if crate::ops::session::host_can_disconnect(state, &host.target) {
                row.closable(crate::state::SidebarClose::Host {
                    target: host.target.clone(),
                })
            } else {
                row
            }
        }
    }
}

/// The muted "nothing here" line for a group with no sessions to list.
fn empty_row(state: &State, text: &str) -> SidebarRow {
    SidebarRow::item(
        Row::new(text).title_style(super::super::fg_only(&state.theme.muted)),
        RowTarget::Inert,
    )
}

/// The Sessions tab as a tree: each host is a heading, with its sessions and the way to start
/// another beneath it.
pub(super) fn sessions_rows(state: &State) -> Vec<SidebarRow> {
    let mut rows = Vec::new();

    // Local group: always present, always available.
    rows.push(
        header_row(state, "LOCAL", None)
            .shape(Shape::Heading)
            .keyed("host:local"),
    );
    let mut any_local = false;
    for entry in state.sidebar.sessions.iter().filter(|e| e.host.is_none()) {
        rows.push(
            session_row(state, entry)
                .at(1)
                .keyed(format!("session:local:{}", entry.name)),
        );
        any_local = true;
    }
    if !any_local {
        rows.push(
            empty_row(state, "No local sessions")
                .at(1)
                .keyed("empty:local"),
        );
    }
    rows.push(
        SidebarRow::action(state, "New session", RowTarget::NewSession(None)).keyed("new:local"),
    );

    // One group per known remote host — configured, recently used, or currently attached — so a
    // host stays listed even while offline. Connecting a host lists its sessions; disconnecting
    // returns it to offline.
    for host in state.remote.hosts.iter() {
        let id = &host.alias;
        let sessions: Vec<&DiscoveredSession> = state
            .sidebar
            .sessions
            .iter()
            .filter(|e| e.remote_target.as_ref() == Some(&host.target))
            .collect();
        // Only a live row is evidence the host answered. The list also carries the host's cached
        // last-seen rows, and a memory must not talk an idle host into "reached".
        let status = vocabulary::host_connection_status(
            state,
            &host.target,
            sessions
                .iter()
                .any(|entry| !crate::ops::session::session_row_is_last_seen(entry)),
        );
        rows.push(
            header_row(state, &host.alias.to_uppercase(), Some((host, status)))
                .shape(Shape::Heading)
                .apart()
                .keyed(format!("host:{id}")),
        );

        match status {
            HostStatus::Connecting | HostStatus::Installing => {}
            HostStatus::Connected | HostStatus::Reachable => {
                // Online: live sessions follow the host row, then the way to start another.
                if sessions.is_empty() {
                    rows.push(
                        empty_row(state, "No sessions here yet")
                            .at(1)
                            .keyed(format!("empty:{id}")),
                    );
                } else {
                    for entry in sessions {
                        rows.push(
                            session_row(state, entry)
                                .at(1)
                                .keyed(format!("session:{id}:{}", entry.name)),
                        );
                    }
                }
                rows.push(
                    SidebarRow::action(
                        state,
                        "New session",
                        RowTarget::NewSession(Some(host.target.clone())),
                    )
                    .keyed(format!("new:{id}")),
                );
            }
            HostStatus::Disconnected | HostStatus::Unreachable => {
                // Offline: the host row connects it; last-seen sessions remain visible from cache.
                if let Some(cached) =
                    crate::session::host_sessions_for(&state.remote.session_cache, &host.target)
                {
                    for entry in cached.iter().filter(|s| !s.ephemeral) {
                        rows.push(
                            cached_session_row(state, host, entry)
                                .at(1)
                                .keyed(format!("cached:{id}:{}", entry.name)),
                        );
                    }
                }
            }
        }
    }

    // A discoverable path to the connect-remote-host prompt, so a host that is not yet configured or
    // recent can still be reached without knowing the Ctrl+R binding.
    rows.push(
        SidebarRow::action(state, "Connect a host…", RowTarget::ConnectHost)
            .apart()
            .keyed("connect"),
    );

    rows
}
