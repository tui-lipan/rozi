//! Shared session-attachment and host chrome for the picker gutter and the Sessions sidebar badge.
//!
//! One vocabulary for both surfaces, because the sidebar and the remote-host picker describe the
//! same two things — a host's reachability and a session's attachment — and a user reading
//! "unreachable" in one and "Offline" in the other has to work out that they mean the same state.
//! Only the casing differs: the picker's rows are lowercase, the sidebar's badges are capitalized
//! like the rest of its tabs.
//!
//! The markers are a closed set, and no two states in *either* vocabulary may share one. A hollow
//! ring says "nothing is connected here"; a filled dot says "something is"; a half dot says
//! "connected, but not the thing in front of you".

use tui_lipan::prelude::*;

use crate::state::{ConnectionState, HostStatus};
use crate::view::fg_only;

/// Connected, but not the attachment on screen. Distinct from the hollow ring a *disconnected*
/// host wears — the two used to share `○`, which read as "this session is offline too".
const MARKER_PARKED: &str = "◐";
const MARKER_LIVE: &str = "●";
const MARKER_ABSENT: &str = "○";

/// Client attachment tier for a discovered session row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionConnectionStatus {
    Current,
    Background,
    Reconnecting,
    Offline,
    Discovered,
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

pub(crate) fn session_connection_status(
    is_current: bool,
    connection: Option<ConnectionState>,
) -> SessionConnectionStatus {
    if is_current {
        SessionConnectionStatus::Current
    } else if connection == Some(ConnectionState::Connected) {
        SessionConnectionStatus::Background
    } else if matches!(
        connection,
        Some(ConnectionState::Connecting | ConnectionState::Reconnecting)
    ) {
        SessionConnectionStatus::Reconnecting
    } else if connection.is_some() {
        SessionConnectionStatus::Offline
    } else {
        SessionConnectionStatus::Discovered
    }
}

pub(crate) fn picker_circle_spinner(style: Style) -> Spinner {
    Spinner::new()
        .spinner_style(SpinnerStyle::Circle)
        .style(style)
}

/// Picker left gutter spinner. Leading space keeps the glyph aligned with `" ●"`.
pub(crate) fn picker_circle_spinner_gutter(style: Style) -> ListItemGutter {
    ListItemGutter::spinner(picker_circle_spinner(style)).leading(1)
}

fn picker_marker_gutter(glyph: &str, style: Style) -> ListItemGutter {
    ListItemGutter::from_spans([Span::new(format!(" {glyph}")).style(style)])
}

pub(crate) fn picker_filled_gutter(style: Style) -> ListItemGutter {
    picker_marker_gutter(MARKER_LIVE, style)
}

pub(crate) fn picker_ring_gutter(style: Style) -> ListItemGutter {
    picker_marker_gutter(MARKER_ABSENT, style)
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
            HostStatus::Connecting => self.connecting,
            HostStatus::Unreachable => self.unreachable,
            HostStatus::Disconnected => self.disconnected,
        }
    }
}

/// The word for a host's state, lowercase as the picker's rows read it. The sidebar capitalizes it.
pub(crate) fn host_status_label(status: HostStatus) -> &'static str {
    match status {
        HostStatus::Connected => "connected",
        HostStatus::Reachable => "reached",
        HostStatus::Connecting => "connecting…",
        HostStatus::Unreachable => "unreachable",
        HostStatus::Disconnected => "disconnected",
    }
}

/// Picker left gutter for a host row.
pub(crate) fn host_status_gutter(status: HostStatus, styles: HostStatusStyles) -> ListItemGutter {
    let style = styles.for_status(status);
    match status {
        HostStatus::Connecting => picker_circle_spinner_gutter(style),
        HostStatus::Connected | HostStatus::Reachable => picker_filled_gutter(style),
        HostStatus::Unreachable | HostStatus::Disconnected => picker_ring_gutter(style),
    }
}

/// Sidebar badge for a host row: the same marker the picker draws — a real spinner while
/// connecting, not a static glyph standing in for one — beside the capitalized word.
pub(crate) fn host_status_badge(status: HostStatus, styles: HostStatusStyles) -> Element {
    let style = styles.for_status(status);
    let marker: Element = match status {
        HostStatus::Connecting => picker_circle_spinner(style).height(Length::Px(1)).into(),
        HostStatus::Connected | HostStatus::Reachable => Text::new(MARKER_LIVE)
            .style(style)
            .height(Length::Px(1))
            .into(),
        HostStatus::Unreachable | HostStatus::Disconnected => Text::new(MARKER_ABSENT)
            .style(style)
            .height(Length::Px(1))
            .into(),
    };
    badge_row(marker, capitalized(host_status_label(status)), styles.label)
}

/// `chrome word`, the shape every sidebar status badge takes.
fn badge_row(chrome: Element, label: String, label_style: Style) -> Element {
    HStack::new()
        .gap(1)
        .width(Length::Auto)
        .height(Length::Px(1))
        .child(chrome)
        .child(Text::new(label).style(label_style).height(Length::Px(1)))
        .into()
}

/// The shared vocabulary is written lowercase for the pickers; the sidebar's badges are
/// capitalized, like its tab labels and its section headers.
fn capitalized(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Picker left gutter: chrome only. Leading space keeps glyphs aligned with `" ●"`.
pub(crate) fn session_status_gutter(
    status: SessionConnectionStatus,
    styles: SessionStatusStyles,
    reserve_discovered: bool,
) -> Option<ListItemGutter> {
    match status {
        SessionConnectionStatus::Current => Some(picker_filled_gutter(styles.current)),
        SessionConnectionStatus::Background => {
            Some(picker_marker_gutter(MARKER_PARKED, styles.background))
        }
        SessionConnectionStatus::Reconnecting => {
            Some(picker_circle_spinner_gutter(styles.reconnecting))
        }
        SessionConnectionStatus::Offline => Some(picker_marker_gutter(" ×", styles.offline)),
        SessionConnectionStatus::Discovered if reserve_discovered => {
            Some(ListItemGutter::text("  "))
        }
        SessionConnectionStatus::Discovered => None,
    }
}

/// Sidebar title-line badge: chrome + word. Omitted for current (▍ already marks it) and discovered.
pub(crate) fn session_status_badge(
    status: SessionConnectionStatus,
    styles: SessionStatusStyles,
) -> Option<Element> {
    let (chrome, label): (Element, &str) = match status {
        SessionConnectionStatus::Background => (
            Text::new(MARKER_PARKED)
                .style(styles.background)
                .height(Length::Px(1))
                .into(),
            "background",
        ),
        SessionConnectionStatus::Reconnecting => (
            picker_circle_spinner(styles.reconnecting)
                .height(Length::Px(1))
                .into(),
            "reconnecting",
        ),
        SessionConnectionStatus::Offline => (
            Text::new("×")
                .style(styles.offline)
                .height(Length::Px(1))
                .into(),
            "offline",
        ),
        SessionConnectionStatus::Current | SessionConnectionStatus::Discovered => return None,
    };
    Some(badge_row(chrome, capitalized(label), styles.label))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parked session and an offline host both used to wear `○`, which said a session in the
    /// background was as gone as a host that cannot be reached. No two states in the two
    /// vocabularies may share a marker.
    #[test]
    fn every_status_marker_stands_for_one_state() {
        let markers = [MARKER_LIVE, MARKER_PARKED, MARKER_ABSENT];
        for (index, marker) in markers.iter().enumerate() {
            assert!(
                !markers[index + 1..].contains(marker),
                "{marker} is used for more than one state"
            );
        }
    }

    /// One vocabulary, two casings: the pickers' rows read lowercase, the sidebar's badges read
    /// like the rest of its tabs.
    #[test]
    fn host_labels_are_lowercase_and_capitalize_for_the_sidebar() {
        for status in [
            HostStatus::Connected,
            HostStatus::Reachable,
            HostStatus::Connecting,
            HostStatus::Unreachable,
            HostStatus::Disconnected,
        ] {
            let label = host_status_label(status);
            assert!(
                label.starts_with(|c: char| c.is_lowercase()),
                "picker rows read lowercase: {label}"
            );
            assert_eq!(capitalized(label)[..1], label.to_uppercase()[..1]);
        }
    }

    #[test]
    fn capitalizing_leaves_the_rest_of_the_word_alone() {
        assert_eq!(capitalized("connecting…"), "Connecting…");
        assert_eq!(capitalized(""), "");
    }
}
