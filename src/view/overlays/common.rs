use tui_lipan::prelude::*;

use super::fg_only;

/// Ctrl plus `letter`, case-insensitive. Overlay interceptors share this so a chord the footer
/// omitted is the same test the handler uses when it stays silent.
pub(super) fn ctrl_letter(key: &KeyEvent, letter: char) -> bool {
    key.mods.ctrl && matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&letter))
}

/// A single `label key` footer hint (e.g. `submit enter`), styled like the palette hint bar.
pub(super) fn hint_pill(theme: &Theme, label: &str, key: &str) -> Element {
    HStack::new()
        .gap(1)
        .width(Length::Auto)
        .height(Length::Auto)
        .child(
            Text::new(label)
                .overflow(Overflow::Clip)
                .style(fg_only(&theme.primary).bold()),
        )
        .child(
            Text::new(crate::view::keys_display::format_keys(key))
                .overflow(Overflow::Clip)
                .style(fg_only(&theme.muted)),
        )
        .into()
}

/// The base footer row shared by every overlay hint bar: content-height with a leading gap above
/// it. Callers add [`hint_pill`] children and may override justify/gap.
pub(super) fn hint_row() -> Flow {
    Flow::new().padding((1, 1, 0, 1)).gap(3).row_gap(0)
}
