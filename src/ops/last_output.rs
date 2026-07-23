//! Copy the last shell-integration command's output to the clipboard.

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane_lifecycle::find_pane_mut;

/// Resolve `last_command_output_range`, copy the text, and confirm with a toast.
///
/// A viewport selection flash would be misleading when the output has scrolled into
/// history (or spans more than the live grid), so this path uses a toast instead.
pub(crate) fn copy_last_output(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.current().focused_pane else {
        return Update::none();
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Update::none();
    };
    let Some(text) = pane.terminal.capture_last_command_output() else {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            "No last command output (enable shell integration)",
        ));
        return Update::full();
    };
    if text.is_empty() {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            "Last command produced no output",
        ));
        return Update::full();
    }
    match ctx.clipboard().copy(&text) {
        Ok(()) => {
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                "Copied last command output",
            ));
            Update::full()
        }
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Copy failed",
                err.to_string(),
            ));
            Update::full()
        }
    }
}
