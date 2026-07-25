//! The arm-then-confirm window shared by every destructive confirmation in the app.
//!
//! A destructive gesture never commits on its first press or click: it *arms*, the surface it came
//! from says so (a struck-through row, a "Click again to confirm" line, a confirm toast), and only a
//! repeat within the window commits. This module owns the clock that makes the window the same
//! length everywhere and the token that lets one arming's expiry not clear a later one's.

use std::time::Duration;

use tui_lipan::prelude::*;

use crate::{HyprmuxApp, Msg};

/// How long a destructive action stays armed. Long enough to move a pointer back onto a one-cell ✕
/// or find a key deliberately, short enough that a confirmation left on screen cannot be committed
/// by an unrelated click minutes later — by which time the user has forgotten what was armed.
pub(crate) const CONFIRM_WINDOW: Duration = Duration::from_secs(3);

/// Arm a confirmation: schedule the expiry that clears it, and return the update that carries it.
///
/// Callers set their own pending field and return this. Clearing one does *not* need a matching
/// call — arming always advances the token, so an expiry still in flight from an abandoned arming
/// can only ever find its own arming already gone and clear nothing.
pub(crate) fn arm(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.confirm_epoch = ctx.state.confirm_epoch.wrapping_add(1);
    let epoch = ctx.state.confirm_epoch;
    Update::with_command(Command::after(
        CONFIRM_WINDOW,
        move |link: CommandLink<Msg>| {
            link.send(Msg::ConfirmationExpired(epoch));
        },
    ))
}

/// The window lapsed. Drop whatever was armed, unless something has been armed since — in which
/// case this expiry belongs to an arming that is already over and must not disarm the new one.
pub(crate) fn expired(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if ctx.state.confirm_epoch != epoch {
        return Update::none();
    }
    clear_all(ctx)
}

/// Drop every armed confirmation. Only one can be armed at a time in practice — the pickers are
/// modal over the sidebar — so this is written as "clear them all" rather than as a dispatch on
/// which surface armed it, which would have to be kept in step with every new one.
fn clear_all(ctx: &mut Context<HyprmuxApp>) -> Update {
    let mut cleared = ctx.state.sidebar.pending_row_close.take().is_some();
    cleared |= ctx.state.sidebar.pending_host_disconnect.take().is_some();
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        cleared |= picker.pending_kill.take().is_some();
    }
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        cleared |= picker.pending_delete.take().is_some();
        cleared |= picker.pending_open.take().is_some();
    }
    if cleared {
        Update::full()
    } else {
        Update::none()
    }
}
