//! Dump pane scrollback to a private file and open it in `$EDITOR`.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::config::{config_editor, editor_launch, missing_editor_command};
use crate::pane::lifecycle::find_pane_mut;
use crate::platform::paths::{PlatformEnv, write_scrollback_dump};
use crate::state::PaneIdentity;

/// Dump the focused pane's full scrollback to a private file and open it in `$EDITOR` as a tiled
/// pane (same pattern as [`crate::ops::config::open_config_file`]).
pub(crate) fn edit_scrollback(ctx: &mut Context<AppRoot>) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::none();
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Update::none();
    };
    let text = pane.terminal.capture_scrollback_text(None);
    let path = match write_scrollback_dump(&PlatformEnv::from_process(), u64::from(id), &text) {
        Ok(path) => path,
        Err(err) => {
            crate::pane::pty_events::notify_error(ctx, "Scrollback dump failed", err.to_string());
            return Update::none();
        }
    };

    let editor = config_editor();
    if let Some(command) = missing_editor_command(&editor) {
        crate::pane::pty_events::notify_error(
            ctx,
            "Editor not found",
            format!("`{command}` is not available\nSet $EDITOR"),
        );
        return Update::none();
    }
    let launch = match editor_launch(&editor, &path) {
        Ok(launch) => launch,
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Editor not found", error);
            return Update::none();
        }
    };
    let identity = PaneIdentity {
        launch: Some(launch),
        ..PaneIdentity::default()
    };
    crate::pane::lifecycle::spawn_interactive_pane(
        ctx,
        ctx.state.current().active_workspace,
        None,
        identity,
    )
    .1
}
