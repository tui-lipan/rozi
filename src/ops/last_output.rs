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
        crate::pty_events::notify_info(ctx, "No last command output (enable shell integration)");
        return Update::full();
    };
    if text.is_empty() {
        crate::pty_events::notify_info(ctx, "Last command produced no output");
        return Update::full();
    }
    match ctx.clipboard().copy(&text) {
        Ok(()) => {
            crate::pty_events::notify_info(ctx, "Copied last command output");
            Update::full()
        }
        Err(err) => {
            crate::pty_events::notify_error(ctx, "Copy failed", err.to_string());
            Update::full()
        }
    }
}
