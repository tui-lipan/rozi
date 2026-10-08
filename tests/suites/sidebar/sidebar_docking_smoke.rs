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
    state.sidebar.shown = visible;
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
fn tabs_picker_toggles_visibility_without_moving_tabs_or_closing() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [2, 2], [true, false]);
        let location = b.state().config.sidebar.layout.location("left1");
        b.state_mut()
            .config
            .sidebar
            .layout
            .place("missing.extension", Right, 1);
        b.state_mut()
            .config
            .sidebar
            .layout
            .hidden
            .push("missing.extension".into());
        b.dispatch(Msg::RunAction(Action::SidebarTabs)).unwrap();
        b.render();
        for hidden in [true, false] {
            b.send_key(KeyEvent {
                code: KeyCode::Enter,
                mods: KeyMods::NONE,
            })
            .unwrap();
            assert_eq!(
                b.state()
                    .config
                    .sidebar
                    .layout
                    .hidden
                    .contains(&"left1".into()),
                hidden
            );
            assert_eq!(b.state().config.sidebar.layout.location("left1"), location);
            assert!(b.state().sidebar_manager);
            assert!(!b.state().sidebar.focused);
            let snapshot = b.capture_ui_snapshot().to_markdown();
            assert!(!snapshot.contains("Locate tab"));
            assert!(!snapshot.contains("layout presets"));
            assert!(snapshot.contains(if hidden { "Disabled" } else { "Enabled" }));
            let grid = b.capture_frame().to_fixed_grid_lines().join("\n");
            let marker = if hidden { "○" } else { "●" };
            assert!(grid.contains(&format!("│ {marker} Left1")), "{grid}");
            assert!(grid.contains("│ – missing.extension"), "{grid}");
        }
        let saved = b.state().config.sidebar.layout.clone();
        b.dispatch(Msg::SidebarManagerActivate("missing.extension".into()))
            .unwrap();
        assert_eq!(b.state().config.sidebar.layout, saved);
        assert!(
            b.capture_ui_snapshot()
                .to_markdown()
                .contains("Unavailable")
        );
        b.dispatch(Msg::SidebarManagerActivate("right:1".into()))
            .unwrap();
        assert_eq!(b.state().config.sidebar.layout.location("left1"), location);
        b.dispatch(Msg::SidebarManagerBack).unwrap();
        assert!(!b.state().sidebar_manager);
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
        assert_eq!(b.state().sidebar.shown, [false, false]);
        assert_eq!(b.state().sidebar.restore, [true, true]);
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
                alt: true,
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
        assert_eq!(state.sidebar.active_panel_index(), 1);
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
fn startup_preferences_apply_on_restart_without_changing_client_visibility() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [2, 2], [true, false]);
        let path = rozi::config::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[theme]\nname = \"nord\"\n").unwrap();
        for (index, expected) in [[false, false], [true, false], [false, true], [true, true]]
            .into_iter()
            .enumerate()
        {
            b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarStartup))
                .unwrap();
            b.dispatch(Msg::SettingsChoiceSelect(index)).unwrap();
            assert_eq!(b.state().sidebar.shown, [true, false]);
            b.dispatch(Msg::SettingsChoiceCancel).unwrap();
            b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarStartup))
                .unwrap();
            b.dispatch(Msg::SettingsChoicePick(index)).unwrap();
            assert_eq!(b.state().sidebar.shown, [true, false]);
            let loaded = rozi::config::load_config();
            assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
            let restarted = rozi::state::State::new(loaded.config, Theme::default());
            assert_eq!(restarted.sidebar.shown, expected);
            assert_eq!(restarted.sidebar.restore, [false, false]);
        }
        let saved = std::fs::read_to_string(&path).unwrap();
        for action in [
            Action::ToggleLeftSidebar,
            Action::ToggleRightSidebar,
            Action::ToggleSidebar,
        ] {
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        }
        let before = (b.state().sidebar.shown, b.state().sidebar.restore);
        b.dispatch(Msg::ConfigFileChanged).unwrap();
        assert_eq!((b.state().sidebar.shown, b.state().sidebar.restore), before);
        assert!(saved.contains("name = \"nord\""));
    });
}

#[test]
fn a_layout_save_migrates_startup_flags_without_losing_tab_definitions() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        rozi::test_support::isolate_user_dirs();
        let path = rozi::config::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = r#"[theme]
name = "nord"
[sidebar]
tabs = [{ name = "jobs", label = "Jobs", entries = [] }]
layout = { left = { visible = false }, right = { visible = true, width = 41, panels = [{ weight = 0.7, tabs = ["jobs", "missing.extension"] }] }, hidden = ["missing.extension"] }
"#;
        std::fs::write(&path, text).unwrap();
        let original: toml::Value = toml::from_str(text).unwrap();
        let loaded = rozi::config::load_config();
        assert_eq!(
            loaded.config.sidebar.startup,
            rozi::config::SidebarStartup::Right
        );
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("migrated to sidebar.startup"))
        );
        let mut b = backend(120, 30, [2, 2], [false, false]);
        *b.state_mut() = rozi::state::State::new(loaded.config, Theme::default());
        b.dispatch(Msg::SettingsActivate(SettingsAction::LeftSidebarPanels))
            .unwrap();
        b.dispatch(Msg::SettingsChoicePick(0)).unwrap();
        let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["sidebar"]["startup"].as_str(), Some("right"));
        assert_eq!(saved["sidebar"]["tabs"], original["sidebar"]["tabs"]);
        assert_eq!(saved["theme"], original["theme"]);
        assert!(saved["sidebar"]["layout"]["right"].get("visible").is_none());
        let reloaded = rozi::config::load_config();
        assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
        assert_eq!(reloaded.config.sidebar.layout.right.width, 41);
        assert_eq!(
            reloaded.config.sidebar.layout.location("missing.extension"),
            Some((Right, 0))
        );
        assert_eq!(b.state().sidebar.shown, [false, true]);
    });
}

#[test]
fn right_only_startup_focuses_a_mounted_panel_without_revealing_a_hidden_dock() {
    on_stack(|| {
        let mut b = backend(120, 30, [2, 2], [false, true]);
        let mut config = b.state().config.clone();
        config.sidebar.startup = rozi::config::SidebarStartup::Right;
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
        b.state_mut().sidebar.select_panel(0);
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        b.render();
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Right);
        assert!(
            b.focused_key().unwrap().as_ref().contains("right-0"),
            "{:?}",
            b.focused_key()
        );
        assert_eq!(b.state().sidebar.shown, [false, true]);
    });
}

#[test]
fn panel_count_changes_preserve_hidden_docks_and_dormant_panels() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        for side in [Left, Right] {
            let mut b = backend(120, 30, [3, 3], [false, false]);
            b.state_mut().sidebar.restore = [false, true];
            let action = if side == Left {
                SettingsAction::LeftSidebarPanels
            } else {
                SettingsAction::RightSidebarPanels
            };
            let saved = b.state().config.sidebar.layout.dock(side).clone();
            b.dispatch(Msg::SettingsActivate(action)).unwrap();
            b.dispatch(Msg::SettingsChoiceSelect(0)).unwrap();
            b.dispatch(Msg::SettingsChoiceCancel).unwrap();
            assert_eq!(b.state().config.sidebar.layout.dock(side), &saved);
            for count in [1, 2, 3] {
                b.dispatch(Msg::SettingsActivate(action)).unwrap();
                b.dispatch(Msg::SettingsChoicePick(count - 1)).unwrap();
                assert_eq!(
                    b.state().config.sidebar.layout.dock(side).panel_count,
                    count
                );
                assert_eq!(
                    b.state().config.sidebar.layout.dock(side).panels,
                    saved.panels
                );
                assert_eq!(b.state().sidebar.shown, [false, false]);
                assert_eq!(b.state().sidebar.restore, [false, true]);
                let loaded = rozi::config::load_config();
                assert_eq!(loaded.config.sidebar.layout.dock(side).panel_count, count);
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
            config.sidebar.startup = rozi::config::SidebarStartup::None;
            *b.state_mut() = rozi::state::State::new(config, Theme::default());
            assert!(!b.state().sidebar_shown());
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(b.state().sidebar_shown());
            assert_eq!(b.state().sidebar.shown, expected);
            let frame = b.capture_frame().to_fixed_grid_lines();
            assert_eq!(frame.iter().any(|row| row.contains("Left1")), expected[0]);
            assert_eq!(frame.iter().any(|row| row.contains("Right1")), expected[1]);
        }
    });
}

#[test]
fn hiding_the_last_dock_updates_the_global_restore_combination() {
    on_stack(|| {
        for action in [Action::ToggleLeftSidebar, Action::ToggleRightSidebar] {
            let mut b = backend(120, 30, [2, 2], [true, true]);
            b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(b.state().sidebar_shown());
            assert_eq!(b.state().sidebar.shown.iter().filter(|v| **v).count(), 1);
            b.dispatch(Msg::RunAction(action)).unwrap();
            assert!(!b.state().sidebar_shown());
            b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
            assert_eq!(
                b.state().sidebar.shown,
                [
                    action == Action::ToggleLeftSidebar,
                    action == Action::ToggleRightSidebar
                ]
            );
        }
    });
}

#[test]
fn visibility_actions_round_trip_every_shown_and_restore_combination() {
    on_stack(|| {
        for shown in [[false, false], [true, false], [false, true], [true, true]] {
            for restore in [[false, false], [true, false], [false, true], [true, true]] {
                for action in [
                    Action::ToggleSidebar,
                    Action::ToggleLeftSidebar,
                    Action::ToggleRightSidebar,
                ] {
                    let mut b = backend(120, 30, [2, 2], shown);
                    b.state_mut().sidebar.restore = restore;
                    b.dispatch(Msg::RunAction(action)).unwrap();
                    b.dispatch(Msg::RunAction(action)).unwrap();
                    assert_eq!(
                        b.state().sidebar.shown,
                        shown,
                        "{shown:?} {restore:?} {action:?}"
                    );
                }
            }
        }
        let mut b = backend(120, 30, [2, 2], [true, true]);
        b.dispatch(Msg::RunAction(Action::ToggleRightSidebar))
            .unwrap();
        b.dispatch(Msg::RunAction(Action::ToggleLeftSidebar))
            .unwrap();
        assert_eq!(b.state().sidebar.shown, [false, false]);
        assert_eq!(b.state().sidebar.restore, [true, false]);
        b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
        assert_eq!(b.state().sidebar.shown, [true, false]);
    });
}

#[test]
fn focus_uses_the_last_panel_on_a_shown_dock_without_reopening_a_hidden_dock() {
    on_stack(|| {
        for (hidden, toggle, shown, target, hidden_panel, key) in [
            (
                Right,
                Action::ToggleRightSidebar,
                [true, false],
                1,
                3,
                "left-1",
            ),
            (
                Left,
                Action::ToggleLeftSidebar,
                [false, true],
                3,
                1,
                "right-1",
            ),
        ] {
            let mut b = backend(120, 30, [2, 2], [true, true]);
            // Real tab-selection callbacks remember the second panel in each dock.
            for panel in [target, hidden_panel] {
                b.dispatch(Msg::SidebarTabSelected { panel, index: 0 })
                    .unwrap();
            }
            b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
            b.dispatch(Msg::SidebarBlur).unwrap();
            b.dispatch(Msg::RunAction(toggle)).unwrap();
            assert_eq!(b.state().sidebar.shown, shown);
            assert_eq!(b.state().sidebar.active_panel().unwrap().dock, hidden);
            b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
            b.render();
            assert_eq!(b.state().sidebar.shown, shown);
            assert_eq!(b.state().sidebar.active_panel_index(), target);
            assert!(b.focused_key().unwrap().as_ref().contains(key));
            assert!(b.state().sidebar.focused);
            let last_toggle = if hidden == Left {
                Action::ToggleRightSidebar
            } else {
                Action::ToggleLeftSidebar
            };
            b.dispatch(Msg::RunAction(last_toggle)).unwrap();
            assert!(!b.state().sidebar.focused);
            b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
            b.render();
            assert_eq!(b.state().sidebar.shown, shown);
            assert_eq!(b.state().sidebar.active_panel_index(), target);
            assert!(b.focused_key().unwrap().as_ref().contains(key));
        }
    });
}

#[test]
fn focus_restores_exactly_the_docks_the_global_toggle_would_show() {
    on_stack(|| {
        for shown in [[false, false], [true, false], [false, true], [true, true]] {
            for restore in [[false, false], [true, false], [false, true], [true, true]] {
                for panel in 0..4 {
                    let mut b = backend(120, 30, [2, 2], shown);
                    b.state_mut().sidebar.restore = restore;
                    b.state_mut().sidebar.select_panel(panel);
                    let expected = if shown == [false, false] {
                        if restore == [false, false] {
                            [true, true]
                        } else {
                            restore
                        }
                    } else {
                        shown
                    };
                    b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
                    b.render();
                    assert_eq!(
                        b.state().sidebar.shown,
                        expected,
                        "{shown:?} {restore:?} {panel}"
                    );
                    let active = b.state().sidebar.active_panel().unwrap();
                    assert!(expected[usize::from(active.dock == Right)]);
                    assert!(b.state().sidebar.focused);
                    assert!(b.focused_key().is_some());
                }
            }
        }
    });
}

#[test]
fn first_show_uses_available_tabs_and_explicit_actions_can_reveal_empty_docks() {
    on_stack(|| {
        for populated in [[true, false], [false, true], [true, true], [false, false]] {
            let mut b = backend(120, 30, [2, 2], [false, false]);
            for panel in &mut b.state_mut().sidebar.panels {
                if !populated[usize::from(panel.dock == Right)] {
                    panel.tabs.clear();
                    panel.active_tab = None;
                }
            }
            b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
            assert_eq!(
                b.state().sidebar.shown,
                if populated == [false, false] {
                    [true, false]
                } else {
                    populated
                }
            );
        }
        let mut b = backend(120, 30, [1, 1], [true, false]);
        b.state_mut().config.sidebar.tabs.clear();
        let state = b.state_mut();
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        b.dispatch(Msg::RunAction(Action::ToggleRightSidebar))
            .unwrap();
        assert_eq!(b.state().sidebar.shown, [true, true]);
        assert!(
            b.capture_frame()
                .to_fixed_grid_lines()
                .join("\n")
                .contains("Drag tabs here")
        );
        b.state_mut().sidebar.select_panel(1);
        b.dispatch(Msg::RunAction(Action::ToggleSidebar)).unwrap();
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        assert!(b.state().sidebar.focused);
        assert!(b.focused_key().unwrap().as_ref().contains("right-0"));
    });
}

#[test]
fn sidebar_mode_uses_directional_focus_and_alt_movement_across_visible_docks() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(140, 36, [3, 3], [true, true]);
        b.state_mut().sidebar.select_panel(1);
        b.dispatch(Msg::RunAction(Action::FocusSidebar)).unwrap();
        let key = |code, mods| KeyEvent { code, mods };
        b.send_key(key(KeyCode::Left, KeyMods::CTRL)).unwrap();
        assert_eq!(
            b.state().sidebar.active_panel_index(),
            1,
            "left from left does not wrap"
        );
        b.send_key(key(KeyCode::Right, KeyMods::CTRL)).unwrap();
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Right);
        assert_eq!(b.state().sidebar.active_panel().unwrap().home, 1);
        let right = b.state().sidebar.active_panel_index();
        b.send_key(key(KeyCode::Right, KeyMods::CTRL)).unwrap();
        assert_eq!(b.state().sidebar.active_panel_index(), right);
        b.send_key(key(KeyCode::Left, KeyMods::ALT)).unwrap();
        assert_eq!(
            b.state().config.sidebar.layout.location("right2"),
            Some((Left, 1))
        );
        assert_eq!(
            b.state().sidebar.active_tab(),
            Some(&SidebarTabId::new("right2"))
        );
        b.send_key(key(KeyCode::Down, KeyMods::ALT)).unwrap();
        assert_eq!(
            b.state().config.sidebar.layout.location("right2"),
            Some((Left, 2))
        );
        b.send_key(key(KeyCode::Right, KeyMods::ALT)).unwrap();
        assert_eq!(
            b.state().config.sidebar.layout.location("right2"),
            Some((Right, 2))
        );
        assert!(b.state().sidebar.focused);
        b.send_key(key(KeyCode::Left, KeyMods::CTRL)).unwrap();
        b.dispatch(Msg::RunAction(Action::ToggleRightSidebar))
            .unwrap();
        b.send_key(key(KeyCode::Right, KeyMods::CTRL)).unwrap();
        assert_eq!(b.state().sidebar.active_panel().unwrap().dock, Left);
        b.send_key(key(KeyCode::Right, KeyMods::ALT)).unwrap();
        assert!(
            !b.state().sidebar.shown[1],
            "arrangement does not enable a dock"
        );
        b.send_key(key(KeyCode::Char('?'), KeyMods::SHIFT)).unwrap();
        let help = b.state().keybindings.as_ref().expect("sidebar help");
        assert_eq!(help.tab, rozi::state::HelpTab::Modes);
        assert_eq!(help.query.text(), "sidebar");
    });
}

#[test]
fn sidebar_presets_apply_only_on_confirmation_and_settings_regains_focus() {
    on_stack(|| {
        let _config = rozi::test_support::lock_config_file();
        let mut b = backend(120, 30, [2, 2], [true, false]);
        let saved = b.state().config.sidebar.layout.clone();
        let mut preset = saved.clone();
        preset.place("left1", Right, 1);
        b.state_mut()
            .config
            .sidebar
            .presets
            .push(rozi::config::SidebarLayoutPreset {
                name: "extension.review".into(),
                label: "Review".into(),
                layout: preset.clone(),
            });
        b.state_mut().show_settings = true;
        b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarLayoutPreset))
            .unwrap();
        b.dispatch(Msg::SettingsChoiceSelect(1)).unwrap();
        assert_eq!(b.state().config.sidebar.layout, saved);
        b.dispatch(Msg::SettingsChoiceCancel).unwrap();
        assert_eq!(b.state().config.sidebar.layout, saved);
        b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarLayoutPreset))
            .unwrap();
        b.dispatch(Msg::SettingsChoicePick(1)).unwrap();
        assert_eq!(b.state().config.sidebar.layout, preset);
        assert_eq!(b.state().sidebar.shown, [true, false]);
        assert!(b.state().show_settings);
        assert!(b.state().settings_choice.is_none());
        b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarTabs))
            .unwrap();
        b.dispatch(Msg::SidebarManagerBack).unwrap();
        assert!(b.state().show_settings);
        b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarLayoutPreset))
            .unwrap();
        let path = rozi::config::config_path();
        std::fs::write(path, "[sidebar]\ntabs = [\"panes\"]\n").unwrap();
        b.dispatch(Msg::ConfigFileChanged).unwrap();
        assert!(
            b.state().settings_choice.is_none(),
            "reload cancels stale preset indices"
        );
        let reloaded = b.state().config.sidebar.layout.clone();
        b.dispatch(Msg::SettingsChoicePick(1)).unwrap();
        assert_eq!(b.state().config.sidebar.layout, reloaded);
    });
}
