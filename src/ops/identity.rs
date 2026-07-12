use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::ops::focus::{request_rename_focus, request_rename_session_focus};
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{Mode, PaneId, PaneRenameState, SessionRenameState, Workspace};

pub(crate) fn rename_pane_in_workspaces(workspaces: &mut [Workspace], id: PaneId, title: &str) {
    if let Some(pane) = workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
    {
        pane.set_custom_title(title);
    }
}

pub(crate) fn open_rename_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.focused_pane else {
        return Update::full();
    };
    let initial = find_pane_mut(&mut ctx.state, target)
        .and_then(|pane| pane.identity.custom_title.clone())
        .unwrap_or_default();

    ctx.state.rename = Some(PaneRenameState::new(target, initial));
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_rename_focus(ctx);
    Update::full()
}

pub(crate) fn apply_rename_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some((target, title)) = ctx
        .state
        .rename
        .as_ref()
        .map(|rename| (rename.target, rename.input.text().to_string()))
    else {
        return Update::none();
    };

    rename_pane_in_workspaces(&mut ctx.state.workspaces, target, &title);
    ctx.state.rename = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn close_rename_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.rename = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn open_rename_workspace(ctx: &mut Context<HyprmuxApp>) -> Update {
    let target = ctx.state.active_workspace;
    let initial = ctx.state.workspaces[target]
        .name
        .clone()
        .unwrap_or_default();

    ctx.state.rename_session = Some(SessionRenameState::new_rename_workspace(target, initial));
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_rename_session_focus(ctx);
    Update::full()
}

#[cfg(test)]
mod tests {
    use crate::state::{Pane, Workspace};
    use tui_lipan::prelude::*;

    #[test]
    fn rename_pane_by_id_sets_custom_title() {
        let mut workspace = Workspace::new(0);
        workspace.panes.push(Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        let mut workspaces = vec![workspace];

        super::rename_pane_in_workspaces(&mut workspaces, 1, "logs");

        assert_eq!(
            workspaces[0].panes[0].identity.custom_title.as_deref(),
            Some("logs")
        );
    }

    #[test]
    fn empty_rename_clears_restored_profile_name() {
        let mut workspace = Workspace::new(0);
        let mut pane = Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        pane.identity.custom_title = Some("server".to_string());
        pane.identity.profile_name = Some("server".to_string());
        workspace.panes.push(pane);
        let mut workspaces = vec![workspace];

        super::rename_pane_in_workspaces(&mut workspaces, 1, "   ");

        assert_eq!(workspaces[0].panes[0].identity.custom_title, None);
        assert_eq!(workspaces[0].panes[0].identity.profile_name, None);
    }
}
