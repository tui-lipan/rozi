//! Shared session-attachment chrome for the picker gutter and the Sessions sidebar badge.

use tui_lipan::prelude::*;

use crate::state::ConnectionState;
use crate::view::fg_only;

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

/// Picker left gutter: chrome only. Leading space keeps glyphs aligned with `" ●"`.
pub(crate) fn session_status_gutter(
    status: SessionConnectionStatus,
    styles: SessionStatusStyles,
    reserve_discovered: bool,
) -> Option<ListItemGutter> {
    match status {
        SessionConnectionStatus::Current => Some(ListItemGutter::from_spans([
            Span::new(" ●").style(styles.current)
        ])),
        SessionConnectionStatus::Background => Some(ListItemGutter::from_spans([
            Span::new(" ○").style(styles.background)
        ])),
        SessionConnectionStatus::Reconnecting => Some(
            ListItemGutter::spinner(
                Spinner::new()
                    .spinner_style(SpinnerStyle::Arc)
                    .style(styles.reconnecting),
            )
            .leading(1),
        ),
        SessionConnectionStatus::Offline => Some(ListItemGutter::from_spans([
            Span::new(" ×").style(styles.offline)
        ])),
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
            Text::new("○")
                .style(styles.background)
                .height(Length::Px(1))
                .into(),
            "background",
        ),
        SessionConnectionStatus::Reconnecting => (
            Spinner::new()
                .spinner_style(SpinnerStyle::Arc)
                .style(styles.reconnecting)
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
    Some(
        HStack::new()
            .gap(1)
            .width(Length::Auto)
            .height(Length::Px(1))
            .child(chrome)
            .child(Text::new(label).style(styles.label).height(Length::Px(1)))
            .into(),
    )
}
