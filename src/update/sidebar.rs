use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::SidebarTabId;

const SESSION_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

fn sessions_active(ctx: &Context<HyprmuxApp>) -> bool {
    ctx.state.sidebar_visible
        && ctx
            .state
            .sidebar
            .active_tab
            .as_ref()
            .is_some_and(|id| id.as_str() == "sessions")
}

pub(crate) fn invalidate_sessions(ctx: &mut Context<HyprmuxApp>) {
    ctx.state.sidebar.invalidate_sessions();
}

pub(crate) fn request_sessions_refresh(ctx: &Context<HyprmuxApp>) {
    if sessions_active(ctx)
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        link.send(crate::Msg::SidebarSessionsRefresh {
            epoch: ctx.state.sidebar.sessions_epoch,
        });
    }
}

pub(super) fn tab_selected(ctx: &mut Context<HyprmuxApp>, id: SidebarTabId) -> Update {
    if ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id() == id)
    {
        if ctx.state.sidebar.active_tab.as_ref() == Some(&id) {
            return Update::none();
        }
        ctx.state.sidebar.invalidate_sessions();
        ctx.state.sidebar.active_tab = Some(id);
        if sessions_active(ctx) {
            refresh_sessions(ctx, ctx.state.sidebar.sessions_epoch)
        } else {
            Update::full()
        }
    } else {
        Update::none()
    }
}

pub(crate) fn visibility_changed(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.sidebar.invalidate_sessions();
    if sessions_active(ctx) {
        refresh_sessions(ctx, ctx.state.sidebar.sessions_epoch)
    } else {
        Update::full()
    }
}

pub(super) fn refresh_sessions(ctx: &Context<HyprmuxApp>, epoch: u64) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    let current_name = ctx.state.session_name.clone();
    let current = crate::ops::session::current_session_row(&ctx.state);
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let rows = crate::session::discovery::discover_selectable_sessions(current_name.as_deref())
            .map(|mut rows| {
                if let Some(current) = current {
                    rows.push(current);
                    rows.sort_by(|a, b| a.name.cmp(&b.name));
                }
                rows
            })
            .map_err(|error| error.to_string());
        link.send(crate::Msg::SidebarSessionsDiscovered { epoch, rows });
    }))
}

pub(super) fn sessions_discovered(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rows: std::result::Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    if let Ok(rows) = rows {
        if ctx
            .state
            .sidebar
            .pending_session_open
            .as_ref()
            .is_some_and(|pending| !rows.iter().any(|entry| &entry.name == pending))
        {
            ctx.state.sidebar.pending_session_open = None;
        }
        ctx.state.sidebar.sessions = rows;
    }
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::sleep(SESSION_REFRESH_INTERVAL);
        link.send(crate::Msg::SidebarSessionsRefresh { epoch });
    }))
}

pub(super) fn activate_session(
    ctx: &mut Context<HyprmuxApp>,
    entry: crate::session::discovery::DiscoveredSession,
) -> Update {
    crate::ops::session::activate_discovered_session(
        ctx,
        entry,
        crate::ops::session::SessionActivationSource::Sidebar,
    )
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

    fn discovered(name: &str) -> crate::session::discovery::DiscoveredSession {
        crate::session::discovery::DiscoveredSession {
            name: name.to_string(),
            ephemeral: false,
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: 1,
                clients: 0,
                has_layout: true,
                created_from_profile: None,
            },
        }
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn sidebar test")
            .join()
            .expect("sidebar test completes");
    }

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

    #[test]
    fn stale_session_results_are_ignored_after_close_switch_and_reload_epochs() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 10;
            }
            let stale = vec![discovered("old")];

            backend.state_mut().sidebar_visible = false;
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 10,
                    rows: Ok(stale.clone()),
                })
                .expect("stale close result");
            assert!(backend.state().sidebar.sessions.is_empty());

            backend.state_mut().sidebar_visible = true;
            backend.state_mut().sidebar.active_tab = Some(SidebarTabId::new("panes"));
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 11,
                    rows: Ok(stale.clone()),
                })
                .expect("stale tab result");
            assert!(backend.state().sidebar.sessions.is_empty());

            backend
                .state_mut()
                .sidebar
                .reconcile(&crate::config::SidebarConfig::default());
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 12,
                    rows: Ok(stale),
                })
                .expect("stale reload result");
            assert!(backend.state().sidebar.sessions.is_empty());
        });
    }

    #[test]
    fn current_session_results_apply_and_missing_confirmation_is_cleared() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 7;
                state.sidebar.pending_session_open = Some("gone".to_string());
            }
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 7,
                    rows: Ok(vec![discovered("dev")]),
                })
                .expect("current result");
            assert_eq!(backend.state().sidebar.sessions, vec![discovered("dev")]);
            assert_eq!(backend.state().sidebar.pending_session_open, None);
        });
    }

    #[test]
    fn sidebar_ephemeral_confirmation_is_independent_from_picker_confirmation() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.session_attached = true;
                state.session_name = Some("eph-test".to_string());
                let mut picker = crate::state::SessionPickerState::new(vec![discovered("picker")]);
                picker.pending_open = Some(0);
                state.session_picker = Some(picker);
            }
            backend
                .dispatch(crate::Msg::SidebarSessionActivate(discovered("dev")))
                .expect("arm sidebar activation");
            assert_eq!(
                backend.state().sidebar.pending_session_open.as_deref(),
                Some("dev")
            );
            assert_eq!(
                backend
                    .state()
                    .session_picker
                    .as_ref()
                    .and_then(|picker| picker.pending_open),
                Some(0)
            );
        });
    }
}
