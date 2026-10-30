//! Rozi's own words for state, and the one place each is written.
//!
//! The sidebar tabs and the pickers describe the same few things — a session's standing with this
//! client, a host's reachability, a checkout and its pull request — so each is spelled, marked, and
//! ordered here once. A user who reads `Restorable` in one view and `restorable` in another, or a
//! hollow ring that means one thing in a picker and another in the sidebar, has to work out that
//! two spellings name one state.
//!
//! Every surface follows the same copy rules:
//!
//! - **A status is capitalized.** A word from a closed Rozi set that says what something *is* —
//!   `Running`, `Stale lock`, `Passed`, `Connected` — starts with a capital wherever it stands,
//!   sidebar or picker. Only its first letter: `Last seen`.
//! - **An action is a lowercase verb.** What a click or a key would *do* — a hover replacement, a
//!   footer hint, an `again to …` cue — reads `attach`, `switch`, `connect`. Case alone separates a
//!   state from an offer, so `Dirty · again to force` reads as one of each.
//! - **A fact is a lowercase phrase.** Counts and provenance continue a row rather than label it:
//!   `2 panes`, `from web`, `shared with 1 other`, `Codex blocked`. Counts come from [`count`].
//! - **Identity is verbatim.** Session, branch, host, agent, and program names, paths, custom agent
//!   statuses, and text an extension supplies keep their own spelling.
//! - **Fields join with ` · `, status first.** [`fields`] drops the empty ones. Notation inside one
//!   field keeps its own punctuation: `session@host`, `repo (branch)`, `+3 -1`.
//! - **Work in progress ends with `…`.** `Creating…` and `Connecting…` will finish; `Running` is a
//!   settled state and does not.
//!
//! The markers are closed sets as well, and within the session and host family a marker means one
//! thing everywhere: a filled dot is something of this client's connected and on screen, a half dot
//! is connected but in the background, a hollow ring is nothing of this client's connected, and a
//! dotted ring is nothing running yet to connect to.
//!
//! A surface with little room may hide part of a state — its word, say, keeping only the marker —
//! but never changes which state it is. A view takes the exact [`SessionPresence`] and decides how
//! much of it to draw; it does not map one presence onto another.

use tui_lipan::prelude::*;

use crate::git::pull_requests::{WorkStatus, WorktreeStatuses};
use crate::git::worktrees::WorktreeInfo;
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use crate::session::remote::RemoteTarget;
use crate::state::{ConnectionState, HostStatus, State};
use crate::view::fg_only;

/// Separates the fields of a description.
pub(crate) const KILLING: &str = "Killing…";
pub(crate) const SEPARATOR: &str = " · ";

/// Marks an agent run a client keeps behind the one it shows: published for its pane, not on
/// screen there. Layered squares - the run sits under what the pane shows - and no ring, which the
/// session family and the idle agent glyph already use.
pub(crate) const BACKGROUND_RUN: &str = "⧉";

/// The name shown for a temporary session in place of its generated one. Lowercase because it sits
/// in the name column, where it stands for a name rather than a state.
pub(crate) const EPHEMERAL: &str = "ephemeral";

/// Joins description fields with [`SEPARATOR`], skipping empty ones.
pub(crate) fn fields<I, S>(parts: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut joined = String::new();
    for part in parts {
        let part = part.as_ref();
        if part.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push_str(SEPARATOR);
        }
        joined.push_str(part);
    }
    joined
}

/// `1 pane`, `2 panes`: a count with its noun, plural by `s`.
pub(crate) fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A session's display name: [`EPHEMERAL`] for a temporary session, its own name otherwise.
pub(crate) fn session_name(name: &str) -> &str {
    if crate::state::is_ephemeral_session_name(name) {
        EPHEMERAL
    } else {
        name
    }
}

/// `session@host` for a session on another machine, the bare session here. The same notation the
/// session picker, its search aliases, and `rozi sessions attach` use.
pub(crate) fn session_at(session: &str, host: Option<&str>) -> String {
    let session = session_name(session);
    match host {
        Some(host) => format!("{session}@{host}"),
        None => session.to_string(),
    }
}

/// On screen, or reached.
const MARKER_LIVE: &str = "●";
/// Connected, but not the attachment on screen. Distinct from the hollow ring of a session this
/// client holds no connection to, which would read as "this session is offline too".
const MARKER_PARKED: &str = "◐";
/// Nothing of this client's connected: a live session it does not hold, or a host it has not
/// reached.
const MARKER_ABSENT: &str = "○";
/// Nothing running yet: a saved session to restore.
const MARKER_DORMANT: &str = "◌";
/// Held, and the connection was lost.
const MARKER_LOST: &str = "×";
/// Held, and reconnecting, where a spinner cannot be drawn: a text badge.
const MARKER_RECONNECTING: &str = "↻";
/// Known, with nothing more to say: a remembered, busy, or unreadable session.
const MARKER_QUIET: &str = "·";
/// A host the last attempt could not reach. Distinct from the plain ring a merely *disconnected*
/// host wears: those are two different pieces of news, and the error colour that used to be the
/// only thing separating them is lost the moment the row is highlighted.
const MARKER_FAILED: &str = "!";

/// How a session stands with this client, strongest first.
///
/// Every surface that lists sessions — the Sessions tab and picker, the sessions using a checkout,
/// a profile's session, a Projects row — takes its marker, its word, and the verb for opening it
/// from here, so one session reads the same in all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SessionPresence {
    /// The session on screen.
    Current,
    /// Held by this client in the background.
    Background,
    /// Held, and reconnecting.
    Reconnecting,
    /// Held, and the connection was lost.
    Offline,
    /// A live server this client holds no connection to.
    Running,
    /// Saved, with no server running.
    Restorable,
    /// Remembered on a host nothing has reached this time.
    LastSeen,
    /// An endpoint that did not answer this poll.
    Busy,
    /// An endpoint that answered with something this client cannot read.
    Unavailable,
}

impl SessionPresence {
    /// From whether the session is on screen and the connection this client holds to it, if any.
    /// A session this client does not hold reads as [`Self::Running`]; [`Self::discovered`] refines
    /// that when discovery knows more.
    pub(crate) fn held(is_current: bool, connection: Option<ConnectionState>) -> Self {
        match connection {
            _ if is_current => Self::Current,
            Some(ConnectionState::Connected) => Self::Background,
            Some(ConnectionState::Connecting | ConnectionState::Reconnecting) => Self::Reconnecting,
            Some(_) => Self::Offline,
            None => Self::Running,
        }
    }

    /// What discovery says of a session this client does not hold. A held session keeps its
    /// standing: this client's own connection is fresher news than a poll.
    pub(crate) fn discovered(self, status: &DiscoveredSessionStatus) -> Self {
        if self.is_held() {
            return self;
        }
        match status {
            DiscoveredSessionStatus::Running { .. } => Self::Running,
            DiscoveredSessionStatus::Restorable => Self::Restorable,
            DiscoveredSessionStatus::LastSeen { .. } => Self::LastSeen,
            DiscoveredSessionStatus::Busy => Self::Busy,
            DiscoveredSessionStatus::Unknown => Self::Unavailable,
        }
    }

    /// Where the Sessions picker lists this standing. The groups are coarse on purpose: a session
    /// reconnecting and then lost stays where it is instead of moving under the user's eye.
    pub(crate) fn picker_group(self) -> u8 {
        match self {
            Self::Current => 0,
            Self::Background | Self::Reconnecting | Self::Offline => 1,
            Self::Running => 2,
            Self::Restorable => 3,
            Self::LastSeen | Self::Busy | Self::Unavailable => 4,
        }
    }

    /// Whether this client holds a connection to the session, on screen or not.
    pub(crate) fn is_held(self) -> bool {
        matches!(
            self,
            Self::Current | Self::Background | Self::Reconnecting | Self::Offline
        )
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Background => "Background",
            Self::Reconnecting => "Reconnecting",
            Self::Offline => "Offline",
            Self::Running => "Running",
            Self::Restorable => "Restorable",
            Self::LastSeen => "Last seen",
            Self::Busy => "Busy",
            Self::Unavailable => "Unavailable",
        }
    }

    /// The marker as text. A gutter draws [`Self::Reconnecting`] as a spinner instead.
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::Current => MARKER_LIVE,
            Self::Background => MARKER_PARKED,
            Self::Reconnecting => MARKER_RECONNECTING,
            Self::Offline => MARKER_LOST,
            Self::Running => MARKER_ABSENT,
            Self::Restorable => MARKER_DORMANT,
            Self::LastSeen | Self::Busy | Self::Unavailable => MARKER_QUIET,
        }
    }

    pub(crate) fn style(self, styles: SessionStatusStyles) -> Style {
        match self {
            Self::Current => styles.current,
            Self::Background => styles.background,
            Self::Reconnecting => styles.reconnecting,
            Self::Offline => styles.offline,
            Self::Running | Self::Restorable | Self::LastSeen | Self::Busy | Self::Unavailable => {
                styles.label
            }
        }
    }

    /// What opening the session does, as an action verb. `None` for the session already on screen.
    pub(crate) fn verb(self) -> Option<&'static str> {
        match self {
            Self::Current => None,
            Self::Background => Some("switch"),
            Self::Reconnecting | Self::Offline | Self::LastSeen => Some("connect"),
            Self::Running | Self::Busy | Self::Unavailable => Some("attach"),
            Self::Restorable => Some("restore"),
        }
    }

    /// Picker left gutter: the marker, spinning while reconnecting.
    pub(crate) fn gutter(self, styles: SessionStatusStyles) -> ListItemGutter {
        let style = self.style(styles);
        match self {
            Self::Reconnecting => picker_circle_spinner_gutter(style),
            _ => picker_marker_gutter(self.marker(), style),
        }
    }
}

/// Theme-derived colors for session status chrome (Send-friendly for palette closures).
#[derive(Clone, Copy)]
pub(crate) struct SessionStatusStyles {
    pub current: Style,
    pub background: Style,
    pub reconnecting: Style,
    pub offline: Style,
    pub label: Style,
}

impl SessionStatusStyles {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            current: Style::new().fg(theme.status.success),
            background: fg_only(&theme.muted),
            reconnecting: Style::new().fg(theme.status.warning),
            offline: Style::new().fg(theme.status.error),
            label: fg_only(&theme.muted),
        }
    }
}

pub(crate) fn picker_circle_spinner(style: Style) -> Spinner {
    Spinner::new()
        .spinner_style(SpinnerStyle::Circle)
        .style(style)
}

pub(crate) fn picker_circle_spinner_gutter(style: Style) -> ListItemGutter {
    ListItemGutter::spinner(picker_circle_spinner(style).gap(0).label(" "))
}

pub(crate) fn picker_marker_gutter(glyph: &str, style: Style) -> ListItemGutter {
    ListItemGutter::from_spans([Span::new(format!("{glyph} ")).style(style)])
}

/// Theme-derived colors for host status chrome, alongside [`SessionStatusStyles`].
#[derive(Clone, Copy)]
pub(crate) struct HostStatusStyles {
    pub connected: Style,
    pub reachable: Style,
    pub connecting: Style,
    pub unreachable: Style,
    pub disconnected: Style,
    pub label: Style,
}

impl HostStatusStyles {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            connected: Style::new().fg(theme.status.success),
            reachable: Style::new().fg(theme.status.info),
            connecting: Style::new().fg(theme.status.info),
            unreachable: Style::new().fg(theme.status.error),
            disconnected: fg_only(&theme.muted),
            label: fg_only(&theme.muted),
        }
    }

    fn for_status(self, status: HostStatus) -> Style {
        match status {
            HostStatus::Connected => self.connected,
            HostStatus::Reachable => self.reachable,
            HostStatus::Connecting | HostStatus::Installing => self.connecting,
            HostStatus::Unreachable => self.unreachable,
            HostStatus::Disconnected => self.disconnected,
        }
    }
}

/// The display status of one known host, from this client's attachments on it plus its probe state.
///
/// Every surface that names a host's state goes through here, so the sidebar badge, the remote-host
/// picker row, and the session picker's group header can never call the same host three things.
///
/// `has_sessions` only decides the case where nothing has ever probed the host: a surface listing
/// *live* rows for it may pass `true` and read as reachable, but one listing remembered rows must
/// pass `false` — a memory is not evidence that anything answers there now.
pub(crate) fn host_connection_status(
    state: &State,
    target: &RemoteTarget,
    has_sessions: bool,
) -> HostStatus {
    let connections: Vec<ConnectionState> = std::iter::once(state.current())
        .chain(state.background.values())
        .filter(|attachment| attachment.remote_target.as_ref() == Some(target))
        .map(|attachment| attachment.connection)
        .collect();
    state
        .remote
        .hosts
        .status_for(target, connections.iter(), has_sessions)
}

/// The word for a host's state.
pub(crate) fn host_status_label(status: HostStatus) -> &'static str {
    match status {
        HostStatus::Connected => "Connected",
        HostStatus::Reachable => "Reached",
        HostStatus::Connecting => "Connecting…",
        HostStatus::Installing => "Installing…",
        HostStatus::Unreachable => "Unreachable",
        HostStatus::Disconnected => "Disconnected",
    }
}

/// Picker left gutter for a host row.
pub(crate) fn host_status_gutter(status: HostStatus, styles: HostStatusStyles) -> ListItemGutter {
    let style = styles.for_status(status);
    match status {
        HostStatus::Connecting | HostStatus::Installing => picker_circle_spinner_gutter(style),
        HostStatus::Connected | HostStatus::Reachable => picker_marker_gutter(MARKER_LIVE, style),
        HostStatus::Unreachable => picker_marker_gutter(MARKER_FAILED, style),
        HostStatus::Disconnected => picker_marker_gutter(MARKER_ABSENT, style),
    }
}

/// Sidebar badge for a host row: the same marker the picker draws — a real spinner while
/// connecting, not a static glyph standing in for one — beside the word.
pub(crate) fn host_status_badge(status: HostStatus, styles: HostStatusStyles) -> Element {
    let style = styles.for_status(status);
    let marker: Element = match status {
        HostStatus::Connecting | HostStatus::Installing => {
            picker_circle_spinner(style).height(Length::Px(1)).into()
        }
        HostStatus::Connected | HostStatus::Reachable => Text::new(MARKER_LIVE)
            .style(style)
            .height(Length::Px(1))
            .into(),
        HostStatus::Unreachable => Text::new(MARKER_FAILED)
            .style(style)
            .height(Length::Px(1))
            .into(),
        HostStatus::Disconnected => Text::new(MARKER_ABSENT)
            .style(style)
            .height(Length::Px(1))
            .into(),
    };
    HStack::new()
        .gap(1)
        .width(Length::Auto)
        .height(Length::Px(1))
        .child(marker)
        .child(
            Text::new(host_status_label(status))
                .style(styles.label)
                .height(Length::Px(1)),
        )
        .into()
}

/// Resolve a session on its exact host, including retained attachments. Discovery is not
/// consulted, so a session this client does not hold reads as [`SessionPresence::Running`].
pub(crate) fn session_presence_for(
    state: &State,
    name: &str,
    target: Option<&RemoteTarget>,
) -> SessionPresence {
    let current = state.current().session_name.as_deref() == Some(name)
        && state.current().remote_target.as_ref() == target;
    let connection = state
        .attachment_by_identity(name, target)
        .map(|attachment| attachment.connection);
    SessionPresence::held(current, connection)
}

/// How a discovered session row stands with this client.
pub(crate) fn session_row_presence(state: &State, entry: &DiscoveredSession) -> SessionPresence {
    session_presence_for(state, &entry.name, entry.remote_target.as_ref()).discovered(&entry.status)
}

/// What a discovered session row says about the session after its status: its panes, the agents a
/// host monitor reports, who else shares it, and the profile it came from. The agents come before
/// the rest because a description cut short loses its end first, and `Codex blocked` is the part
/// worth going to the session for.
///
/// `others` counts the *other* clients attached — the ones a new attach would join.
pub(crate) fn session_facts(
    entry: &DiscoveredSession,
    agents: Option<&str>,
    others: u32,
) -> String {
    let panes = match &entry.status {
        DiscoveredSessionStatus::Running { panes, .. }
        | DiscoveredSessionStatus::LastSeen { panes } => Some(count(*panes, "pane")),
        DiscoveredSessionStatus::Restorable
        | DiscoveredSessionStatus::Busy
        | DiscoveredSessionStatus::Unknown => None,
    };
    let shared = (others > 0).then(|| format!("shared with {}", count(others as usize, "other")));
    let profile = entry
        .origin
        .profile
        .as_ref()
        .map(|profile| format!("from {profile}"));
    fields(
        [panes, agents.map(str::to_string), shared, profile]
            .into_iter()
            .flatten(),
    )
}

/// How many clients other than this one are attached to a running session.
pub(crate) fn other_clients(entry: &DiscoveredSession, presence: SessionPresence) -> u32 {
    match &entry.status {
        DiscoveredSessionStatus::Running { clients, .. } => {
            clients.saturating_sub(u32::from(presence.is_held()))
        }
        _ => 0,
    }
}

/// What a host monitor knows about the agents in one session, as a compact fact.
///
/// This is the payoff of the monitor layer: the answer to "does anything on that machine want me"
/// without opening a session there. It stays deliberately thin — a state and a count — because the
/// monitor carries semantic summaries rather than the pane runtime a real attachment streams.
///
/// `None` for a session this client is attached to. That attachment already publishes live pane
/// state to the Agents tab and the pane chrome, and a snapshot taken between polls would be the
/// staler of the two answers sitting next to each other.
pub(crate) fn host_agent_label(state: &State, entry: &DiscoveredSession) -> Option<String> {
    use crate::session::protocol::AgentState;
    let target = entry.remote_target.as_ref()?;
    if state
        .attachment_by_identity(&entry.name, Some(target))
        .is_some()
    {
        return None;
    }
    let agents = state.remote.agents.get(target)?;
    let mine = agents.iter().filter(|agent| agent.session == entry.name);
    // Severity order, the same rule pane chrome uses over a set of published rows: one blocked
    // agent outranks any number of working ones, and any working one outranks the finished. Idle
    // is deliberately absent — a state that prompts no action is not news, and spending the line
    // on it would push a blocked agent in the *next* session off the visible part of the row.
    let mut counts = [0_usize; 4];
    let mut named: [Option<&str>; 4] = [None; 4];
    for agent in mine {
        let bucket = match agent.state {
            AgentState::Blocked => 0,
            AgentState::Error => 1,
            AgentState::Working => 2,
            AgentState::Done => 3,
            AgentState::Idle => continue,
        };
        counts[bucket] += 1;
        named[bucket] = named[bucket].or(Some(agent.label.as_str()));
    }
    let (bucket, word) = [(0, "blocked"), (1, "error"), (2, "working"), (3, "done")]
        .into_iter()
        .find(|(bucket, _)| counts[*bucket] > 0)?;
    match (counts[bucket], named[bucket]) {
        // One agent can be named, and its own label says more than the number 1 ever could. The
        // state continues the phrase, so it stays lowercase like any other fact.
        (1, Some(label)) => Some(format!("{label} {word}")),
        (count, _) => Some(format!("{count} {word}")),
    }
}

/// What a checkout is to Git, when that is worth saying: a plain linked checkout says nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckoutKind {
    Primary,
    StaleLock,
    Locked,
    Prunable,
}

impl CheckoutKind {
    pub(crate) fn of(tree: &WorktreeInfo) -> Option<Self> {
        if !tree.linked {
            Some(Self::Primary)
        } else if tree.lock.as_ref().is_some_and(|lock| lock.stale) {
            Some(Self::StaleLock)
        } else if tree.lock.is_some() {
            Some(Self::Locked)
        } else if tree.prunable {
            Some(Self::Prunable)
        } else {
            None
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Primary => "Primary",
            Self::StaleLock => "Stale lock",
            Self::Locked => "Locked",
            Self::Prunable => "Prunable",
        }
    }

    /// A kind that asks for cleanup wears the warning colour.
    pub(crate) fn style(self, theme: &Theme) -> Style {
        match self {
            Self::StaleLock | Self::Prunable => Style::new().fg(theme.status.warning),
            Self::Primary | Self::Locked => fg_only(&theme.muted),
        }
    }
}

/// A checkout's branch, or what stands in for one.
pub(crate) fn checkout_branch(tree: &WorktreeInfo) -> &str {
    match (&tree.branch, tree.bare) {
        (_, true) => "(bare)",
        (Some(branch), false) => branch,
        (None, false) => "(detached)",
    }
}

pub(crate) fn work_status_label(status: WorkStatus) -> &'static str {
    match status {
        WorkStatus::Open => "Open",
        WorkStatus::Draft => "Draft",
        WorkStatus::Merged => "Merged",
        WorkStatus::Closed => "Closed",
        WorkStatus::Passed => "Passed",
        WorkStatus::Failed => "Failed",
        WorkStatus::Running => "Running",
    }
}

pub(crate) fn work_status_style(theme: &Theme, status: WorkStatus) -> Style {
    match status {
        WorkStatus::Passed => Style::new().fg(theme.status.success),
        WorkStatus::Failed => Style::new().fg(theme.status.error),
        WorkStatus::Running => Style::new().fg(theme.status.warning),
        WorkStatus::Merged => fg_only(&theme.accent),
        WorkStatus::Open | WorkStatus::Draft | WorkStatus::Closed => fg_only(&theme.muted),
    }
}

/// A checkout's pull request — `#114 · Passed`, then `Stale` when the last lookup failed and this
/// is the one before it — with the status that colours it. `PR unavailable` for a branch whose
/// lookup failed with nothing to fall back on; `None` when there is nothing to say.
pub(crate) fn pull_request_text(
    tree: &WorktreeInfo,
    statuses: &WorktreeStatuses,
) -> Option<(String, Option<WorkStatus>)> {
    match statuses.checkouts.get(&tree.path) {
        Some(pr) => Some((
            fields([
                format!("#{}", pr.number).as_str(),
                work_status_label(pr.status),
                if statuses.unavailable { "Stale" } else { "" },
            ]),
            Some(pr.status),
        )),
        None if tree.linked && tree.branch.is_some() && statuses.unavailable => {
            Some(("PR unavailable".to_string(), None))
        }
        None => None,
    }
}

/// A checkout described on one line, as a picker row reads it: its kind, then its pull request.
pub(crate) fn checkout_description(tree: &WorktreeInfo, statuses: &WorktreeStatuses) -> String {
    fields([
        CheckoutKind::of(tree).map_or("", CheckoutKind::label),
        pull_request_text(tree, statuses)
            .map(|(text, _)| text)
            .unwrap_or_default()
            .as_str(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::AgentSummary;

    const PRESENCES: [SessionPresence; 9] = [
        SessionPresence::Current,
        SessionPresence::Background,
        SessionPresence::Reconnecting,
        SessionPresence::Offline,
        SessionPresence::Running,
        SessionPresence::Restorable,
        SessionPresence::LastSeen,
        SessionPresence::Busy,
        SessionPresence::Unavailable,
    ];

    /// A parked session and an offline host both used to wear `○`, which said a session in the
    /// background was as gone as a host that cannot be reached. Every state that has news of its
    /// own needs a marker of its own; only the quiet ones, which defer to their words, share `·`.
    #[test]
    fn every_session_marker_stands_for_one_state() {
        let markers: Vec<&str> = PRESENCES
            .iter()
            .map(|presence| presence.marker())
            .filter(|marker| *marker != MARKER_QUIET)
            .collect();
        for (index, marker) in markers.iter().enumerate() {
            assert!(
                !markers[index + 1..].contains(marker),
                "{marker} is used for more than one state"
            );
        }
    }

    /// A status reads the same in every view, so every word Rozi owns starts with a capital and no
    /// view lowercases it.
    #[test]
    fn every_owned_status_is_capitalized() {
        let hosts = [
            HostStatus::Connected,
            HostStatus::Reachable,
            HostStatus::Connecting,
            HostStatus::Installing,
            HostStatus::Unreachable,
            HostStatus::Disconnected,
        ]
        .map(host_status_label);
        let checkouts = [
            CheckoutKind::Primary,
            CheckoutKind::StaleLock,
            CheckoutKind::Locked,
            CheckoutKind::Prunable,
        ]
        .map(CheckoutKind::label);
        let work = [
            WorkStatus::Open,
            WorkStatus::Draft,
            WorkStatus::Merged,
            WorkStatus::Closed,
            WorkStatus::Passed,
            WorkStatus::Failed,
            WorkStatus::Running,
        ]
        .map(work_status_label);
        for label in PRESENCES
            .map(SessionPresence::label)
            .into_iter()
            .chain(hosts)
            .chain(checkouts)
            .chain(work)
        {
            assert!(
                label.starts_with(char::is_uppercase),
                "a status is capitalized: {label}"
            );
        }
    }

    /// And every action is a lowercase verb, so a hover never reads as a state.
    #[test]
    fn every_session_verb_is_lowercase() {
        for verb in PRESENCES.iter().filter_map(|presence| presence.verb()) {
            assert!(
                verb.starts_with(char::is_lowercase),
                "an action is lowercase: {verb}"
            );
        }
    }

    #[test]
    fn fields_skip_empty_parts_and_counts_pluralize() {
        assert_eq!(fields(["#114", "", "Passed"]), "#114 · Passed");
        assert_eq!(fields(["", ""]), "");
        assert_eq!(count(1, "pane"), "1 pane");
        assert_eq!(count(0, "pane"), "0 panes");
        assert_eq!(count(3, "other"), "3 others");
        assert_eq!(session_at("dev", Some("workbox")), "dev@workbox");
        assert_eq!(session_at("dev", None), "dev");
    }

    #[test]
    fn presence_uses_attachment_identity_and_keeps_other_hosts_running() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().session_name = Some("dev".into());
        let remote = RemoteTarget::Alias("workbox".into());
        let presence = |state: &State, target| session_presence_for(state, "dev", target).label();
        assert_eq!(presence(&state, None), "Current");
        assert_eq!(presence(&state, Some(&remote)), "Running");
        let mut attachment = crate::state::Attachment::new();
        attachment.session_name = Some("dev".into());
        attachment.remote_target = Some(remote.clone());
        attachment.connection = ConnectionState::Connected;
        state.background.insert(1, attachment);
        for (connection, expected) in [
            (ConnectionState::Connected, "Background"),
            (ConnectionState::Reconnecting, "Reconnecting"),
            (ConnectionState::Disconnected, "Offline"),
        ] {
            state.background.get_mut(&1).unwrap().connection = connection;
            assert_eq!(presence(&state, Some(&remote)), expected);
            assert_eq!(presence(&state, None), "Current");
        }
    }

    /// Discovery speaks only for a session this client does not hold.
    #[test]
    fn discovery_refines_only_an_unheld_session() {
        assert_eq!(
            SessionPresence::Running.discovered(&DiscoveredSessionStatus::Restorable),
            SessionPresence::Restorable
        );
        assert_eq!(
            SessionPresence::Running.discovered(&DiscoveredSessionStatus::Unknown),
            SessionPresence::Unavailable
        );
        assert_eq!(
            SessionPresence::Background.discovered(&DiscoveredSessionStatus::Busy),
            SessionPresence::Background
        );
    }

    fn summary(
        session: &str,
        pane: crate::state::PaneId,
        label: &str,
        state: &str,
    ) -> AgentSummary {
        AgentSummary {
            reference: crate::session::protocol::AgentRef::for_test(pane, 0, None),
            session: session.into(),
            agent: label.to_lowercase(),
            label: label.into(),
            state: crate::session::protocol::AgentState::from_status(state),
            reason: None,
            task: None,
            completions: 0,
            changed_at: 0,
        }
    }

    fn discovered(name: &str, target: &RemoteTarget) -> DiscoveredSession {
        DiscoveredSession {
            name: name.into(),
            origin: Default::default(),
            host: Some(target.display_label()),
            remote_target: Some(target.clone()),
            ephemeral: false,
            status: DiscoveredSessionStatus::Running {
                panes: 1,
                clients: 0,
                has_layout: false,
            },
        }
    }

    /// Panes first, then the agents, then who shares it and where it came from; a status that has
    /// no panes to count starts with what follows.
    #[test]
    fn session_facts_follow_one_order() {
        let mut entry = discovered("dev", &RemoteTarget::Alias("workbox".into()));
        entry.origin.profile = Some("web".into());
        assert_eq!(
            session_facts(&entry, Some("Codex blocked"), 2),
            "1 pane · Codex blocked · shared with 2 others · from web"
        );
        entry.status = DiscoveredSessionStatus::Restorable;
        assert_eq!(session_facts(&entry, None, 0), "from web");
    }

    /// The row has one line to spend, so it spends it on the most urgent thing the monitor knows.
    /// Idle is the state it must never name: a machine full of quiet agents would otherwise fill
    /// every row with news of nothing.
    #[test]
    fn host_agent_label_reports_the_most_urgent_state_in_the_session() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let entry = discovered("dev", &target);

        // Nothing known about the host at all: no token, not an empty one.
        assert_eq!(host_agent_label(&state, &entry), None);

        state.remote.agents.insert(
            target.clone(),
            vec![
                summary("dev", 1, "Codex", "working"),
                summary("dev", 2, "Claude Code", "blocked"),
                summary("dev", 3, "OpenCode", "idle"),
                // A different session on the same host must not leak into this row.
                summary("backend", 4, "Codex", "blocked"),
            ],
        );
        assert_eq!(
            host_agent_label(&state, &entry).as_deref(),
            Some("Claude Code blocked")
        );

        // Several in the winning bucket: a count, since no single label describes them.
        state.remote.agents.insert(
            target.clone(),
            vec![
                summary("dev", 1, "Codex", "blocked"),
                summary("dev", 2, "Claude Code", "blocked"),
            ],
        );
        assert_eq!(
            host_agent_label(&state, &entry).as_deref(),
            Some("2 blocked")
        );

        // A custom status is an active run, by the same rule the pane chrome uses.
        state.remote.agents.insert(
            target.clone(),
            vec![summary("dev", 1, "Codex", "running tests")],
        );
        assert_eq!(
            host_agent_label(&state, &entry).as_deref(),
            Some("Codex working")
        );

        // Quiet agents say nothing at all.
        state
            .remote
            .agents
            .insert(target.clone(), vec![summary("dev", 1, "Codex", "idle")]);
        assert_eq!(host_agent_label(&state, &entry), None);
    }

    /// An attachment streams live pane runtime to the Agents tab. A monitor snapshot taken between
    /// polls is the staler of the two answers, so the row stops repeating it.
    #[test]
    fn an_attached_session_reports_no_monitor_token() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state
            .remote
            .agents
            .insert(target.clone(), vec![summary("dev", 1, "Codex", "blocked")]);
        let entry = discovered("dev", &target);
        assert_eq!(
            host_agent_label(&state, &entry).as_deref(),
            Some("Codex blocked")
        );

        state.current_mut().session_name = Some("dev".into());
        state.current_mut().remote_target = Some(target.clone());
        assert_eq!(host_agent_label(&state, &entry), None);
    }
}
