use hyprmux::HyprmuxApp;
use hyprmux::config::{SidebarPosition, SidebarTab, SidebarTabId};
use hyprmux::state::SidebarPanelState;
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{KeyMods, Rect};

fn mouse(x: u16, y: u16, kind: MouseKind) -> MouseEvent {
    MouseEvent {
        x,
        y,
        kind,
        mods: KeyMods::NONE,
    }
}

fn rendered_sidebar(position: SidebarPosition) -> Vec<String> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.sidebar.position = position;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
    }
    backend.render();
    assert_eq!(backend.state().content_viewport(backend.viewport()).w, 68);
    assert_eq!(backend.state().last_viewport.get().unwrap().w, 100);
    assert_eq!(backend.state().last_content_viewport.get().unwrap().w, 68);
    backend.capture_frame().to_fixed_grid_lines()
}

#[test]
fn sidebar_shell_reserves_the_same_content_width_on_both_docks() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let left = rendered_sidebar(SidebarPosition::Left);
            let right = rendered_sidebar(SidebarPosition::Right);
            assert!(left.iter().any(|line| line.contains("Panes")));
            assert!(right.iter().any(|line| line.contains("Panes")));
            assert!(
                left.iter()
                    .any(|line| line.chars().take(32).collect::<String>().contains("Panes"))
            );
            assert!(
                right.iter().any(|line| line
                    .chars()
                    .skip(68)
                    .collect::<String>()
                    .contains("Panes"))
            );
        })
        .expect("spawn sidebar smoke thread")
        .join()
        .expect("sidebar smoke completes");
}

#[test]
fn live_dock_flip_keeps_configured_sidebar_width() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.width = 30;
                state.config.sidebar.position = SidebarPosition::Left;
                state.config.sidebar.tabs = vec![SidebarTab::Panes];
                state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
            }
            backend.render();
            assert_eq!(
                backend.state().effective_sidebar_width(backend.viewport()),
                30
            );
            assert!(
                backend
                    .capture_frame()
                    .to_fixed_grid_lines()
                    .iter()
                    .any(|line| line.chars().take(30).collect::<String>().contains("Panes"))
            );

            backend.state_mut().config.sidebar.position = SidebarPosition::Right;
            backend.render();
            assert_eq!(
                backend.state().effective_sidebar_width(backend.viewport()),
                30
            );
            assert_eq!(backend.state().content_viewport(backend.viewport()).w, 70);
            let lines = backend.capture_frame().to_fixed_grid_lines();
            assert!(
                lines.iter().any(|line| line
                    .chars()
                    .skip(70)
                    .collect::<String>()
                    .contains("Panes")),
                "sidebar must stay 30 cols on the right after a live dock flip; got:\n{}",
                lines.join("\n")
            );
            assert!(
                lines.iter().all(|line| !line
                    .chars()
                    .take(70)
                    .collect::<String>()
                    .contains("Panes")),
                "pane area must not inherit the old sidebar column after a live dock flip"
            );
        })
        .expect("spawn live dock flip thread")
        .join()
        .expect("live dock flip completes");
}

#[test]
fn narrow_sidebar_yields_to_a_one_column_canvas() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 5,
            });
            backend.state_mut().sidebar_visible = true;
            assert_eq!(
                backend.state().effective_sidebar_width(backend.viewport()),
                9
            );
            assert_eq!(backend.state().content_viewport(backend.viewport()).w, 1);
        })
        .expect("spawn narrow sidebar test")
        .join()
        .expect("narrow sidebar test completes");
}

#[test]
fn split_sidebar_renders_two_draggable_tab_bars() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![SidebarTab::Agents, SidebarTab::Panes];
                state.sidebar.panels = vec![
                    SidebarPanelState {
                        tabs: vec![SidebarTabId::new("agents")],
                        active_tab: Some(SidebarTabId::new("agents")),
                        ..Default::default()
                    },
                    SidebarPanelState {
                        tabs: vec![SidebarTabId::new("panes")],
                        active_tab: Some(SidebarTabId::new("panes")),
                        ..Default::default()
                    },
                ];
            }
            backend.render();

            let snapshot = backend.capture_ui_snapshot().to_markdown();
            assert_eq!(snapshot.matches("DraggableTabBar").count(), 2, "{snapshot}");
            let grid = backend.capture_frame().to_fixed_grid();
            assert!(grid.contains("Agents"), "{grid}");
            assert!(grid.contains("Panes"), "{grid}");
        })
        .expect("spawn split sidebar thread")
        .join()
        .expect("split sidebar render completes");
}

#[test]
fn split_flag_changes_presentation_without_changing_panel_recipe() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let configured_panels = vec![
                vec![SidebarTabId::new("agents")],
                vec![SidebarTabId::new("panes")],
            ];
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![SidebarTab::Agents, SidebarTab::Panes];
                state.config.sidebar.panels = configured_panels.clone();
                state.config.sidebar.split = false;
                state.sidebar.reconcile(&state.config.sidebar.clone());
            }
            backend.render();
            let snapshot = backend.capture_ui_snapshot().to_markdown();
            assert_eq!(snapshot.matches("DraggableTabBar").count(), 1, "{snapshot}");
            assert_eq!(backend.state().config.sidebar.panels, configured_panels);

            {
                let state = backend.state_mut();
                state.config.sidebar.split = true;
                state.sidebar.reconcile(&state.config.sidebar.clone());
            }
            backend.render();
            let snapshot = backend.capture_ui_snapshot().to_markdown();
            assert_eq!(snapshot.matches("DraggableTabBar").count(), 2, "{snapshot}");
            assert_eq!(backend.state().config.sidebar.panels, configured_panels);
        })
        .expect("spawn split presentation thread")
        .join()
        .expect("split presentation completes");
}

#[test]
fn split_sidebar_junction_resizes_both_splitters_without_entering_pane_content() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![SidebarTab::Agents, SidebarTab::Panes];
                state.sidebar.panels = vec![
                    SidebarPanelState {
                        tabs: vec![SidebarTabId::new("agents")],
                        active_tab: Some(SidebarTabId::new("agents")),
                        ..Default::default()
                    },
                    SidebarPanelState {
                        tabs: vec![SidebarTabId::new("panes")],
                        active_tab: Some(SidebarTabId::new("panes")),
                        ..Default::default()
                    },
                ];
            }
            backend.render();

            let frame = backend.capture_frame();
            let divider_x = (0..40)
                .find(|x| frame.cell(*x, 2).symbol == "│")
                .expect("outer sidebar divider");
            let divider_y = (2..28)
                .find(|y| {
                    (0..divider_x)
                        .filter(|x| frame.cell(*x, *y).symbol == "─")
                        .count()
                        > usize::from(divider_x / 2)
                })
                .expect("panel divider");
            assert_eq!(frame.cell(divider_x, divider_y).symbol, "┤");
            assert_ne!(
                frame.cell(divider_x + 1, divider_y).symbol,
                "─",
                "the panel splitter must stop before pane content"
            );

            backend
                .send_mouse(mouse(
                    divider_x,
                    divider_y,
                    MouseKind::Down(MouseButton::Left),
                ))
                .expect("grab sidebar splitter junction");
            backend
                .send_mouse(mouse(
                    divider_x + 4,
                    divider_y + 3,
                    MouseKind::Drag(MouseButton::Left),
                ))
                .expect("drag sidebar splitter junction");
            backend.render();

            assert_eq!(backend.state().sidebar.width_preview, Some(36));
            let dragged = backend.capture_frame();
            assert_eq!(dragged.cell(5, divider_y + 3).symbol, "─");

            backend
                .send_mouse(mouse(
                    divider_x + 4,
                    divider_y + 3,
                    MouseKind::Up(MouseButton::Left),
                ))
                .expect("release sidebar splitter junction");
            backend.render();
            assert_eq!(backend.state().config.sidebar.width, 36);
            assert!(backend.state().config.sidebar.split_ratio > 0.5);
        })
        .expect("spawn sidebar junction thread")
        .join()
        .expect("sidebar junction completes");
}

#[test]
fn sidebar_splitter_moves_live_before_the_resize_is_committed() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            backend.state_mut().sidebar_visible = true;
            backend.render();
            let initial = backend.capture_frame();
            let divider_bg = backend.state().theme.surface.element;
            let divider = (0..40)
                .find(|x| initial.cell(*x, 8).symbol == "│")
                .expect("sidebar divider");
            assert_eq!(initial.cell(divider, 8).bg, divider_bg);
            let divider_fg = initial.cell(divider, 8).fg;

            backend
                .send_mouse(mouse(divider, 8, MouseKind::Moved))
                .expect("hover sidebar splitter");
            let hovered = backend.capture_frame();
            assert_eq!(hovered.cell(divider, 8).fg, divider_fg);
            assert_eq!(hovered.cell(divider, 8).bg, divider_bg);

            assert!(
                backend
                    .send_mouse(mouse(divider, 8, MouseKind::Down(MouseButton::Left)))
                    .expect("grab sidebar splitter")
            );
            assert!(
                backend
                    .send_mouse(mouse(divider + 8, 8, MouseKind::Drag(MouseButton::Left)))
                    .expect("drag sidebar splitter")
            );
            backend.render();

            assert_eq!(backend.state().config.sidebar.width, 32);
            assert_eq!(backend.state().sidebar.width_preview, Some(40));
            assert_eq!(backend.state().content_viewport(backend.viewport()).w, 60);
            let frame = backend.capture_frame();
            let moved_divider =
                (divider + 1..divider + 12).find(|x| frame.cell(*x, 8).symbol == "│");
            assert_eq!(
                moved_divider,
                Some(divider + 8),
                "{}",
                frame.to_fixed_grid()
            );

            backend
                .send_mouse(mouse(divider + 8, 8, MouseKind::Up(MouseButton::Left)))
                .expect("release sidebar splitter");
            backend.render();
            assert_eq!(backend.state().sidebar.width_preview, None);
            assert_eq!(backend.state().config.sidebar.width, 40);
        })
        .expect("spawn splitter drag thread")
        .join()
        .expect("splitter drag completes");
}
