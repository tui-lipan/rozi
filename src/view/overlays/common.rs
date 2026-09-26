use tui_lipan::prelude::*;

use super::fg_only;
use crate::{AppRoot, Msg};

const HINT_PAD_TOP: u16 = 1;
const HINT_PAD_X: u16 = 1;
const HINT_GAP: u16 = 3;

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

/// A [`hint_pill`] that also does what its key does when clicked: `msg` must be the message that
/// key sends, so a click and a keypress can never disagree. Looks the same at rest; the hover lift
/// is the only sign it answers to the pointer.
pub(super) fn hint_button(ctx: &Context<AppRoot>, label: &str, key: &str, msg: Msg) -> Element {
    MouseRegion::new()
        .on_click(ctx.link().callback(move |_| msg.clone()))
        .hover_effect(VisualEffect::transform_bg(crate::view::hover_lift()))
        .child(hint_pill(&ctx.state.theme, label, key))
        .into()
}

/// The base footer row shared by every overlay hint bar: content-height with a leading gap above
/// it. Callers add [`hint_button`] or [`hint_pill`] children and may override justify/gap.
pub(super) fn hint_row() -> Flow {
    Flow::new()
        .padding((HINT_PAD_TOP, HINT_PAD_X, 0, HINT_PAD_X))
        .gap(HINT_GAP)
        .row_gap(0)
}
