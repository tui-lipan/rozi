use rozi::config::{
    SidebarDockPanel,
    SidebarPosition::{Left, Right},
    SidebarTab, SidebarTabId,
};
use rozi::input::Action;
use rozi::state::SettingsAction;
use rozi::{AppRoot, Msg};
use tui_lipan::{TestBackend, prelude::*};

fn on_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

fn backend(
    width: u16,
    height: u16,
    counts: [usize; 2],
    visible: [bool; 2],
) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    });
    let state = backend.state_mut();
    state.config.animations.enabled = false;
    state.config.pane.show_workbar = false;
    state.config.sidebar.tabs.clear();
    for (index, side) in [Left, Right].into_iter().enumerate() {
        let dock = state.config.sidebar.layout.dock_mut(side);
        dock.panel_count = counts[index];
        dock.panels = (0..counts[index])
            .map(|panel| SidebarDockPanel {
                weight: 1.0,
                tabs: vec![format!("{}{}", side.id(), panel + 1)],
            })
            .collect();
        for panel in 0..counts[index] {
            state.config.sidebar.tabs.push(SidebarTab::Launcher {
                name: SidebarTabId::new(format!("{}{}", side.id(), panel + 1)),
                label: format!("{}{}", side.label(), panel + 1),
                entries: Vec::new(),
                env: Vec::new(),
            });
        }
    }
    state.sidebar.dock_visible = visible;
    state.sidebar_visible = visible.iter().any(|v| *v);
    state.sidebar.apply_configured_panels(&state.config.sidebar);
    backend.render();
    backend
}

#[test]
fn dock_panel_combinations_share_geometry_and_render_expected_bars() {
    on_stack(|| {
        for visible in [[false, false], [true, false], [false, true], [true, true]] {
            for left in 1..=3 {
                for right in 1..=3 {
                    for width in [24, 40, 80, 120, 180] {
                        let mut b = backend(width, 30, [left, right], visible);
                        let tree = b.capture_ui_snapshot().to_markdown();
                        assert_eq!(
                            tree.matches("DraggableTabBar").count(),
                            usize::from(visible[0]) * left + usize::from(visible[1]) * right,
                            "{visible:?} {left}/{right} {width}\n{tree}"
                        );
                        assert!(b.state().content_viewport(b.viewport()).w >= 20);
                        let grid = b.capture_frame().to_fixed_grid_lines();
                        assert_eq!(grid.len(), 30);
                        if width >= 120 {
                            assert_eq!(grid.iter().any(|row| row.contains("Left1")), visible[0]);
                            assert_eq!(grid.iter().any(|row| row.contains("Right1")), visible[1]);
                        }
                        let saved = b.state().config.sidebar.layout.clone();
                        b.set_viewport(Rect {
                            x: 0,
                            y: 0,
                            w: 20,
                            h: 10,
                        });
                        b.render();
                        b.set_viewport(Rect {
                            x: 0,
                            y: 0,
                            w: width,
                            h: 30,
                        });
                        b.render();
                        assert_eq!(b.state().config.sidebar.layout, saved);
                    }
                }
            }
        }
    });
}

#[test]
fn panel_choice_waits_for_confirmation_and_cancellation_is_lossless() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [3, 2], [true, true]);
        let saved = b.state().config.sidebar.layout.clone();
        b.state_mut().show_settings = true;
        b.dispatch(Msg::SettingsActivate(SettingsAction::LeftSidebarPanels))
            .unwrap();
        b.dispatch(Msg::SettingsChoiceSelect(0)).unwrap();
        assert_eq!(b.state().config.sidebar.layout, saved);
        b.dispatch(Msg::SettingsChoiceCancel).unwrap();
        assert_eq!(b.state().config.sidebar.layout, saved);
        b.dispatch(Msg::SettingsActivate(SettingsAction::LeftSidebarPanels))
            .unwrap();
        b.dispatch(Msg::SettingsChoicePick(0)).unwrap();
        assert_eq!(b.state().config.sidebar.layout.left.panel_count, 1);
        assert_eq!(
            b.state().config.sidebar.layout.left.panels,
            saved.left.panels
        );
        assert_eq!(b.state().config.sidebar.layout.right, saved.right);
    });
}

#[test]
fn manager_recovers_hidden_tabs_and_moves_focused_content_between_docks() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [2, 2], [true, false]);
        b.dispatch(Msg::RunAction(Action::ManageSidebarTabs))
            .unwrap();
        b.dispatch(Msg::SidebarManagerActivate("left1".into()))
            .unwrap();
        b.dispatch(Msg::SidebarManagerActivate("visibility".into()))
            .unwrap();
        assert!(
            b.state()
                .config
                .sidebar
                .layout
                .hidden
                .contains(&"left1".into())
        );
        assert!(b.state().sidebar.panels[0].tabs.is_empty());
        b.dispatch(Msg::SidebarManagerActivate("right:1".into()))
            .unwrap();
        assert_eq!(
            b.state().config.sidebar.layout.location("left1"),
            Some((Right, 1))
        );
        b.dispatch(Msg::SidebarManagerActivate("locate".into()))
            .unwrap();
        b.render();
        assert!(b.state().sidebar.dock_visible[1]);
        assert!(!b.state().sidebar_manager);
        assert!(b.state().config.sidebar.layout.hidden.is_empty());
        assert_eq!(
            b.state().sidebar.active_tab(),
            Some(&SidebarTabId::new("left1"))
        );
        assert!(b.state().sidebar.focused);
        assert!(b.focused_key().is_some());
        b.dispatch(Msg::RunAction(Action::ToggleRightSidebar))
            .unwrap();
        b.render();
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Left);
        assert!(b.focused_key().is_some());
    });
}

#[test]
fn global_toggle_restores_the_dock_combination_and_stale_events_are_ignored() {
    on_stack(|| {
        let mut b = backend(120, 30, [3, 3], [true, true]);
        let epoch = b.state().sidebar.layout_epoch;
        b.state_mut().config.sidebar.layout.left.panel_count = 1;
        let state = b.state_mut();
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        b.dispatch(Msg::SidebarUiEvent {
            epoch,
            event: Box::new(Msg::SidebarTabSelected { panel: 0, index: 1 }),
        })
        .unwrap();
        assert_eq!(
            b.state().sidebar.active_tab(),
            Some(&SidebarTabId::new("left1"))
        );
        b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
        b.render();
        assert_eq!(b.state().content_viewport(b.viewport()).w, 120);
        assert_eq!(b.state().sidebar.dock_visible, [true, true]);
        b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
        b.render();
        assert_eq!(b.state().content_viewport(b.viewport()).w, 56);
    });
}

#[test]
fn dual_three_panel_terminal_snapshot() {
    on_stack(|| {
        let b = backend(100, 24, [3, 3], [true, true]);
        let grid = b.capture_frame().to_fixed_grid_lines().join("\n") + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sidebar/dual-three-panels.txt");
        if std::env::var_os("ROZI_UPDATE_DOCKING_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &grid).unwrap();
        }
        assert_eq!(grid, std::fs::read_to_string(path).unwrap());
    });
}

fn mouse(
    x: u16,
    y: u16,
    kind: tui_lipan::core::event::MouseKind,
) -> tui_lipan::core::event::MouseEvent {
    tui_lipan::core::event::MouseEvent {
        x,
        y,
        kind,
        mods: tui_lipan::core::event::KeyMods::NONE,
    }
}

#[test]
fn dragging_one_dock_preserves_the_other_docks_unconstrained_preference() {
    on_stack(|| {
        use tui_lipan::core::event::{MouseButton, MouseKind};
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(100, 24, [1, 1], [true, true]);
        b.state_mut().config.sidebar.layout.left.width = 32;
        b.state_mut().config.sidebar.layout.right.width = 60;
        b.render();
        for (kind, x) in [
            (MouseKind::Down(MouseButton::Left), 31),
            (MouseKind::Drag(MouseButton::Left), 25),
            (MouseKind::Up(MouseButton::Left), 25),
        ] {
            b.send_mouse(mouse(x, 10, kind)).unwrap();
            b.render();
        }
        assert_eq!(b.state().config.sidebar.layout.left.width, 26);
        assert_eq!(b.state().config.sidebar.layout.right.width, 60);
    });
}

#[test]
fn native_drag_transfers_a_tab_into_an_empty_panel_in_the_other_dock() {
    on_stack(|| {
        use tui_lipan::core::event::{MouseButton, MouseKind};
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 24, [2, 2], [true, true]);
        let state = b.state_mut();
        state.config.sidebar.layout.right.panels[1].tabs.clear();
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        b.render();
        let grid = b.capture_frame().to_fixed_grid_lines();
        let target_y = grid
            .iter()
            .position(|row| row.contains("Drag tabs here"))
            .expect("empty target") as u16;
        for (kind, x, y) in [
            (MouseKind::Down(MouseButton::Left), 3, 0),
            (MouseKind::Drag(MouseButton::Left), 92, target_y),
            (MouseKind::Up(MouseButton::Left), 92, target_y),
        ] {
            b.send_mouse(mouse(x, y, kind)).unwrap();
            b.render();
        }
        assert_eq!(
            b.state().config.sidebar.layout.location("left1"),
            Some((Right, 1))
        );
        assert!(!b.state().sidebar.focused);
    });
}

#[test]
fn follower_docks_keep_the_controller_canvas_and_layout_revision() {
    on_stack(|| {
        let mut b = backend(120, 30, [3, 3], [false, false]);
        let mut shared = rozi::state::SharedSessionState::new(1);
        shared.controller = Some(2);
        shared.canonical_canvas = Some((120, 30));
        b.state_mut().current_mut().shared = Some(shared);
        for action in [
            Action::ToggleLeftSidebar,
            Action::ToggleRightSidebar,
            Action::ToggleSidebar,
            Action::ToggleSidebar,
        ] {
            b.dispatch(Msg::RunAction(action)).unwrap();
            b.render();
            let shared = b.state().current().shared.as_ref().unwrap();
            assert_eq!(shared.canonical_canvas, Some((120, 30)));
            assert_eq!(shared.layout_rev, 0);
            assert!(!shared.layout_commit_scheduled);
            assert!(!shared.is_controller());
        }
    });
}

#[test]
fn compacted_reordering_preserves_home_panels_hidden_slots_and_selection() {
    on_stack(|| {
        use tui_lipan::core::event::{KeyCode, KeyEvent, KeyMods};
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [3, 1], [true, false]);
        let state = b.state_mut();
        state.config.sidebar.layout.left.panels[1].tabs =
            vec!["left2".into(), "unavailable.tab".into(), "left3".into()];
        state.config.sidebar.layout.left.panels[2].tabs.clear();
        state.config.sidebar.layout.left.panel_count = 1;
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("left3"));
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        b.render();
        b.send_key(KeyEvent {
            code: KeyCode::Left,
            mods: KeyMods {
                ctrl: true,
                shift: true,
                ..KeyMods::NONE
            },
        })
        .unwrap();
        b.render();
        assert_eq!(
            b.state().config.sidebar.layout.left.panels[0].tabs,
            ["left1"]
        );
        assert_eq!(
            b.state().config.sidebar.layout.left.panels[1].tabs,
            ["left3", "unavailable.tab", "left2"]
        );
        b.state_mut().config.sidebar.layout.left.panel_count = 3;
        let state = b.state_mut();
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        assert_eq!(state.sidebar.active_panel, 1);
        assert_eq!(
            state.sidebar.active_tab(),
            Some(&SidebarTabId::new("left3"))
        );
        assert_eq!(
            state.sidebar.panels[1].tabs,
            [SidebarTabId::new("left3"), SidebarTabId::new("left2")]
        );
        assert!(state.sidebar.panels[2].tabs.is_empty());
    });
}

#[test]
fn settings_persists_startup_docks_while_runtime_toggles_remain_client_local() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [2, 2], [true, false]);
        let path = rozi::config::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[theme]\nname = \"nord\"\n").unwrap();
        b.state_mut().config.sidebar.layout.left.visible = true;
        b.state_mut().config.sidebar.layout.right.visible = false;
        b.dispatch(Msg::SettingsActivate(
            SettingsAction::ToggleRightSidebarStartup,
        ))
        .unwrap();
        assert!(b.state().config.sidebar.layout.right.visible);
        assert_eq!(b.state().sidebar.dock_visible, [true, true]);
        let loaded = rozi::config::load_config();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let restarted = rozi::state::State::new(loaded.config, Theme::default());
        assert!(restarted.sidebar_visible);
        assert_eq!(restarted.sidebar.dock_visible, [true, true]);
        let saved = std::fs::read_to_string(&path).unwrap();
        for action in [
            Action::ToggleLeftSidebar,
            Action::ToggleRightSidebar,
            Action::ToggleSidebar,
        ] {
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        }
        // Settings reflects the configured startup value even when local chrome is hidden.
        b.dispatch(Msg::SettingsActivate(
            SettingsAction::ToggleLeftSidebarStartup,
        ))
        .unwrap();
        assert!(!b.state().config.sidebar.layout.left.visible);
        let loaded = rozi::config::load_config();
        let restarted = rozi::state::State::new(loaded.config, Theme::default());
        assert_eq!(restarted.sidebar.dock_visible, [false, true]);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("name = \"nord\"")
        );
    });
}

#[test]
fn right_only_startup_focuses_a_mounted_panel_and_recovers_a_hidden_target() {
    on_stack(|| {
        let mut b = backend(120, 30, [2, 2], [false, true]);
        let mut config = b.state().config.clone();
        config.sidebar.layout.left.visible = false;
        config.sidebar.layout.right.visible = true;
        *b.state_mut() = rozi::state::State::new(config, Theme::default());
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Right);
        b.render();
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        b.render();
        assert!(b.state().sidebar.focused);
        assert!(
            b.focused_key().unwrap().as_ref().contains("right-0"),
            "{:?}",
            b.focused_key()
        );
        b.dispatch(Msg::SidebarBlur).unwrap();
        b.state_mut().sidebar.active_panel = 0;
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        b.render();
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Right);
        assert!(
            b.focused_key().unwrap().as_ref().contains("right-0"),
            "{:?}",
            b.focused_key()
        );
        assert_eq!(b.state().sidebar.dock_visible, [false, true]);
    });
}

#[test]
fn split_shortcut_restores_each_docks_preferred_count_even_after_restart() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let path = rozi::config::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        for side in [Left, Right] {
            let mut b = backend(120, 30, [3, 3], [true, true]);
            b.state_mut().config.sidebar.layout.left.visible = true;
            b.state_mut().config.sidebar.layout.right.visible = true;
            let selected = b
                .state()
                .sidebar
                .panels
                .iter()
                .position(|p| p.dock == side)
                .unwrap();
            b.state_mut().sidebar.active_panel = selected;
            let panels = b.state().config.sidebar.layout.dock(side).panels.clone();
            b.dispatch(Msg::RunAction(Action::ToggleSidebarSplit))
                .unwrap();
            assert_eq!(b.state().config.sidebar.layout.dock(side).panel_count, 1);
            assert_eq!(
                b.state()
                    .config
                    .sidebar
                    .layout
                    .dock(side)
                    .expanded_panel_count,
                3
            );
            assert_eq!(b.state().config.sidebar.layout.dock(side).panels, panels);
            assert_eq!(
                b.state()
                    .config
                    .sidebar
                    .layout
                    .dock(side.toggled())
                    .panel_count,
                3
            );
            let loaded = rozi::config::load_config();
            assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
            assert_eq!(
                loaded.config.sidebar.layout.dock(side).expanded_panel_count,
                3
            );
            *b.state_mut() = rozi::state::State::new(loaded.config, Theme::default());
            let selected = b
                .state()
                .sidebar
                .panels
                .iter()
                .position(|p| p.dock == side)
                .unwrap();
            b.state_mut().sidebar.active_panel = selected;
            b.dispatch(Msg::RunAction(Action::ToggleSidebarSplit))
                .unwrap();
            assert_eq!(b.state().config.sidebar.layout.dock(side).panel_count, 3);
            assert_eq!(
                b.state()
                    .sidebar
                    .panels
                    .iter()
                    .filter(|p| p.dock == side)
                    .count(),
                3
            );
            // A later explicit two-panel choice replaces the remembered expanded count.
            b.dispatch(Msg::SettingsActivate(if side == Left {
                SettingsAction::LeftSidebarPanels
            } else {
                SettingsAction::RightSidebarPanels
            }))
            .unwrap();
            b.dispatch(Msg::SettingsChoicePick(1)).unwrap();
            for count in [1, 2] {
                b.dispatch(Msg::RunAction(Action::ToggleSidebarSplit))
                    .unwrap();
                assert_eq!(
                    b.state().config.sidebar.layout.dock(side).panel_count,
                    count
                );
            }
        }
    });
}

#[test]
fn independent_dock_commands_open_only_the_requested_dock_from_hidden_startup() {
    on_stack(|| {
        for (action, expected) in [
            (Action::ToggleLeftSidebar, [true, false]),
            (Action::ToggleRightSidebar, [false, true]),
        ] {
            let mut b = backend(120, 30, [2, 2], [false, false]);
            let mut config = b.state().config.clone();
            config.sidebar.layout.left.visible = false;
            config.sidebar.layout.right.visible = false;
            *b.state_mut() = rozi::state::State::new(config, Theme::default());
            assert!(!b.state().sidebar_visible);
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(b.state().sidebar_visible);
            assert_eq!(b.state().sidebar.dock_visible, expected);
            let frame = b.capture_frame().to_fixed_grid_lines();
            assert_eq!(frame.iter().any(|row| row.contains("Left1")), expected[0]);
            assert_eq!(frame.iter().any(|row| row.contains("Right1")), expected[1]);
        }
    });
}

#[test]
fn independent_dock_commands_preserve_the_global_restore_combination() {
    on_stack(|| {
        for action in [Action::ToggleLeftSidebar, Action::ToggleRightSidebar] {
            let mut b = backend(120, 30, [2, 2], [true, true]);
            b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(b.state().sidebar_visible);
            assert_eq!(
                b.state()
                    .sidebar
                    .dock_visible
                    .iter()
                    .filter(|v| **v)
                    .count(),
                1
            );
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(!b.state().sidebar_visible);
            b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
            assert_eq!(b.state().sidebar.dock_visible, [true, true]);
        }
    });
}
