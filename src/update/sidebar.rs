use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::SidebarTabId;

pub(super) fn tab_selected(ctx: &mut Context<HyprmuxApp>, id: SidebarTabId) -> Update {
    if ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id() == id)
    {
        ctx.state.sidebar.active_tab = Some(id);
        Update::full()
    } else {
        Update::none()
    }
}

pub(super) fn focus_pane(ctx: &mut Context<HyprmuxApp>, id: crate::state::PaneId) -> Update {
    if crate::ops::focus::focus_pane_anywhere(ctx, id) {
        Update::full()
    } else {
        Update::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Pane;
    use tui_lipan::TestBackend;

    #[test]
    fn sidebar_focus_switches_workspace_and_clears_activity() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let mut pane = Pane::new(
                    2,
                    100,
                    FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 40.0,
                        h: 20.0,
                    },
                );
                pane.activity.has_unseen_output = true;
                backend.state_mut().workspaces[1].panes.push(pane);
                backend
                    .dispatch(crate::Msg::SidebarFocusPane(2))
                    .expect("focus sidebar pane");
                assert_eq!(backend.state().active_workspace, 1);
                assert_eq!(backend.state().focused_pane, Some(2));
                assert!(
                    !backend.state().workspaces[1].panes[0]
                        .activity
                        .has_unseen_output
                );
            })
            .expect("spawn sidebar focus test")
            .join()
            .expect("sidebar focus test completes");
    }
}
