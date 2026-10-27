//! Sidebar tabs: selection and scrolling, appearance, launcher entries, and per-tab settings.
mod interaction {
    use rozi::config::{SidebarTab, SidebarTabId};
    use rozi::state::{SidebarCommandOutput, SidebarCommandRow};
    use rozi::{AppRoot, Msg};
    use tui_lipan::core::event::{KeyMods, MouseButton, MouseEvent, MouseKind};
    use tui_lipan::prelude::Component;
    use tui_lipan::{Rect, TestBackend};

    fn mouse(x: u16, kind: MouseKind, mods: KeyMods) -> MouseEvent {
        MouseEvent {
            x,
            y: 0,
            kind,
            mods,
        }
    }

    fn scroll<C: Component>(backend: &mut TestBackend<C>, kind: MouseKind) {
        for _ in 0..12 {
            backend
                .send_mouse(mouse(1, kind, KeyMods::SHIFT))
                .expect("horizontal scroll");
        }
    }

    fn click_label<C: Component>(backend: &mut TestBackend<C>, label: &str) {
        let line = backend.capture_frame().to_fixed_grid_lines().remove(0);
        let x = line.find(label).expect("label should be visible") as u16;
        backend
            .send_mouse(mouse(x, MouseKind::Down(MouseButton::Left), KeyMods::NONE))
            .expect("mouse down");
        backend
            .send_mouse(mouse(x, MouseKind::Up(MouseButton::Left), KeyMods::NONE))
            .expect("mouse up");
    }

    #[test]
    fn revisiting_cached_command_tab_renders_without_an_unrelated_update() {
        rozi::test_support::on_large_stack(|| {
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            let files = SidebarTabId::new("files");
            {
                let state = backend.state_mut();
                state.sidebar.shown = [true, false];
                // Revealing the sidebar after the first frame is a real toggle, so it runs the
                // real slide; these assertions are about the settled column, not a frame
                // part-way through it.
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![
                    SidebarTab::Panes,
                    SidebarTab::Command {
                        name: files.clone(),
                        label: "Files".to_string(),
                        command: "printf file-row".to_string(),
                        interval_secs: 10,
                        on_click: None,
                        group_prefix: None,
                        env: Vec::new(),
                        format: rozi::config::SidebarCommandFormat::Lines,
                    },
                ];
                // One panel holding exactly these two tabs, so nothing the default split would
                // put in a second panel can render its own rows into the assertions below.
                state.config.sidebar.layout.left.panel_count = 1;
                state.sidebar.apply_configured_panels(&state.config.sidebar);
                state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
                state.sidebar.command_output.insert(
                    files.clone(),
                    SidebarCommandOutput {
                        epoch: 1,
                        tree: None,
                        cwd: None,
                        rows: vec![SidebarCommandRow {
                            raw: "file-row".to_string(),
                            display: "file-row".to_string(),
                            error: false,
                            header: false,
                        }],
                    },
                );
                // Selecting the tab increments the epoch; keep polling parked so the assertion
                // covers the selection update rather than a command result racing it.
                state.sidebar.command_in_flight.insert(files.clone(), 1);
            }
            backend.render();
            assert!(!backend.capture_frame().to_fixed_grid().contains("file-row"));

            // Cached output is only shown while it still describes the focused pane's directory,
            // which the first update resolves. Stamping it afterwards is what the real poll does.
            backend
                .dispatch(Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("re-select the active tab so the per-message sync resolves the cwd");
            {
                let state = backend.state_mut();
                let cwd = state.sidebar.command_cwd.clone();
                if let Some(output) = state.sidebar.command_output.get_mut(&files) {
                    output.cwd = cwd;
                }
            }

            backend
                .dispatch(Msg::SidebarTabSelected { panel: 0, index: 1 })
                .expect("select command tab");

            assert!(backend.capture_frame().to_fixed_grid().contains("file-row"));
        });
    }

    #[test]
    fn app_sidebar_tabs_keep_native_selection_hover_click_and_wheel_behavior() {
        rozi::test_support::on_large_stack(|| {
            rozi::test_support::isolate_user_dirs();
            let launcher = |name: &str, label: &str| SidebarTab::Launcher {
                name: SidebarTabId::new(name),
                label: label.into(),
                entries: Vec::new(),
                env: Vec::new(),
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            {
                let state = backend.state_mut();
                state.sidebar.shown = [true, false];
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![
                    SidebarTab::Panes,
                    launcher("build", "Build"),
                    launcher("sessions-long", "Sessions"),
                    launcher("deployment", "Deployment"),
                ];
                state.sidebar.panels[0].tabs = state
                    .config
                    .sidebar
                    .tabs
                    .iter()
                    .map(SidebarTab::id)
                    .collect();
                state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
            }
            backend.render();

            let theme = backend.state().theme.clone();
            let initial = backend.capture_frame();
            let strip = theme.surface.element.elevate_by(0.05);
            assert_eq!(initial.cell(2, 0).bg, theme.border_active);
            assert_eq!(initial.cell(9, 0).bg, strip);

            backend
                .send_mouse(mouse(9, MouseKind::Moved, KeyMods::NONE))
                .expect("hover Build tab");
            assert!(backend.hovered().is_some());
            assert_eq!(
                backend.capture_frame().cell(9, 0).bg,
                strip.elevate_by(0.08)
            );

            click_label(&mut backend, "Build");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("build"))
            );

            scroll(&mut backend, MouseKind::ScrollDown);
            click_label(&mut backend, "Deployment");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("deployment"))
            );
        });
    }

    #[test]
    fn destination_selection_resolves_after_transfer_into_an_empty_panel() {
        rozi::test_support::on_large_stack(|| {
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.config.sidebar.tabs = vec![SidebarTab::activity()];
                state.sidebar.panels = vec![
                    rozi::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("activity")],
                        active_tab: Some(SidebarTabId::new("activity")),
                        ..Default::default()
                    },
                    rozi::state::SidebarPanelState {
                        home: 1,
                        ..Default::default()
                    },
                ];
                assert!(state.sidebar.transfer_tab(0, 1, 0, 0));
            }

            backend
                .dispatch(Msg::SidebarTabSelected { panel: 1, index: 0 })
                .expect("destination change follows transfer");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("activity"))
            );
            assert_eq!(backend.state().sidebar.active_panel_index(), 1);
        });
    }
}

mod appearance {
    //! Sidebar fill and the blank row under each tab bar.

    use rozi::AppRoot;
    use rozi::config::SidebarTab;
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    fn sidebar_backend() -> TestBackend<AppRoot> {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            state.sidebar.shown = [true, false];
            state.config.animations.sidebar = false;
            state.config.sidebar.tabs = vec![SidebarTab::Panes];
            state.config.sidebar.layout.left.panel_count = 1;
            state.sidebar.apply_configured_panels(&state.config.sidebar);
            state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
        }
        backend
    }

    fn sidebar_columns(backend: &mut TestBackend<AppRoot>) -> Vec<String> {
        backend.render();
        let width = backend.state().effective_sidebar_width(backend.viewport()) as usize;
        backend
            .capture_frame()
            .to_fixed_grid_lines()
            .iter()
            .map(|line| line.chars().take(width.saturating_sub(1)).collect())
            .collect()
    }

    fn tab_then_body(lines: &[String]) -> (usize, usize) {
        let tab = lines
            .iter()
            .position(|line| line.contains("Panes"))
            .unwrap_or_else(|| panic!("tab bar shows Panes:\n{lines:#?}"));
        let body = lines
            .iter()
            .position(|line| line.contains("Workspace") || line.contains("+ New pane"))
            .unwrap_or_else(|| panic!("body shows a panes row:\n{lines:#?}"));
        (tab, body)
    }

    #[test]
    fn sidebar_gap_inserts_one_row_between_the_tab_bar_and_its_list() {
        rozi::test_support::on_large_stack(|| {
            let mut backend = sidebar_backend();
            let (tab, body) = tab_then_body(&sidebar_columns(&mut backend));
            assert_eq!(
                body,
                tab + 2,
                "default gap is one blank row under the tab bar; tab={tab} body={body}"
            );

            backend.state_mut().config.sidebar.gap = false;
            let (tab, body) = tab_then_body(&sidebar_columns(&mut backend));
            assert_eq!(
                body,
                tab + 1,
                "disabled gap seats the list on the next row; tab={tab} body={body}"
            );
        });
    }

    #[test]
    fn round_tab_style_caps_the_active_sidebar_tab() {
        rozi::test_support::on_large_stack(|| {
            let mut backend = sidebar_backend();
            backend.state_mut().config.sidebar.tab_style = tui_lipan::prelude::CapStyle::Round;
            let lines = sidebar_columns(&mut backend);
            let tab = lines
                .iter()
                .find(|line| line.contains("Panes"))
                .unwrap_or_else(|| panic!("tab bar shows Panes:\n{lines:#?}"));
            assert!(
                tab.contains('\u{e0b6}') && tab.contains('\u{e0b4}'),
                "round tabs wrap the active label in end caps:\n{tab}"
            );
        });
    }
}

mod launcher {
    use rozi::AppRoot;
    use rozi::config::{SidebarLauncherEntry, SidebarTab, SidebarTabId, UserCommandAction};
    use rozi::state::{SidebarCommandOutput, SidebarCommandRow};
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    fn sidebar_lines(tab: SidebarTab) -> Vec<String> {
        rozi::test_support::on_large_stack(move || {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar.shown = [true, false];
                // Revealing the sidebar after the first frame is a real toggle, so it runs the
                // real slide; these assertions are about the settled column, not a frame
                // part-way through it.
                state.config.animations.sidebar = false;
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
            }
            backend.render();
            backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect()
        })
    }

    /// A launcher entry renders in the same two-line row shape as the built-in tabs: an action glyph in
    /// the glyph column, the label as the title, and what the entry does on the detail line directly
    /// beneath it — not a bare label at column zero.
    #[test]
    fn launcher_entries_render_as_glyphed_two_line_rows() {
        let lines = sidebar_lines(SidebarTab::Launcher {
            name: SidebarTabId::new("deploy"),
            label: "Deploy".into(),
            env: Vec::new(),
            entries: vec![
                SidebarLauncherEntry {
                    label: "Build".into(),
                    group: None,
                    action: UserCommandAction::run("cargo build --release"),
                },
                SidebarLauncherEntry {
                    label: "Date".into(),
                    group: None,
                    action: UserCommandAction::Send("date\n".into()),
                },
                SidebarLauncherEntry {
                    label: "Logs".into(),
                    group: None,
                    action: UserCommandAction::popup("journalctl -f"),
                },
            ],
        });
        let index = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("sidebar shows {needle:?}"))
        };

        // Each label carries its action glyph, and its detail line follows immediately — no blank row
        // between entries.
        assert!(lines[index("Build")].contains('▸'));
        assert_eq!(index("cargo build"), index("Build") + 1);
        assert!(lines[index("Date")].contains('⏎'));
        assert_eq!(index("send"), index("Date") + 1);
        assert!(lines[index("Logs")].contains('▫'));
        assert_eq!(index("journalctl"), index("Logs") + 1);
        assert_eq!(index("Date"), index("cargo build") + 1);

        // Rows are inset past the marker gutter rather than starting at column zero.
        assert!(lines[index("Build")].starts_with(' '));
    }

    /// Grouped entries read as sections: a header above each group, a blank row between them, and the
    /// ungrouped entries leading without a header of their own.
    #[test]
    fn grouped_launcher_entries_render_under_section_headers() {
        let entry = |label: &str, group: Option<&str>, command: &str| SidebarLauncherEntry {
            label: label.into(),
            group: group.map(Into::into),
            action: UserCommandAction::run(command),
        };
        let lines = sidebar_lines(SidebarTab::Launcher {
            name: SidebarTabId::new("agents"),
            label: "Agents".into(),
            env: Vec::new(),
            entries: vec![
                entry("Shell", None, "bash"),
                entry("rozi", Some("claude"), "claude"),
                entry("docs", Some("claude"), "claude"),
                entry("rozi", Some("codex"), "codex"),
            ],
        });
        let index = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("sidebar shows {needle:?}"))
        };

        // Ungrouped rows lead with no header over them; sections follow in first-appearance order.
        assert!(index("Shell") < index("claude"));
        assert!(index("claude") < index("codex"));
        // A header sits directly above the first row of its section, separated from what came before
        // it by a blank row.
        assert_eq!(index("rozi"), index("claude") + 1);
        // Only the row's own text matters here; the sidebar's border and scrollbar occupy the tail of
        // every captured line.
        let blank = |line: &str| line.chars().take(24).all(char::is_whitespace);
        assert!(blank(&lines[index("claude") - 1]));
        assert!(blank(&lines[index("codex") - 1]));
        // Both entries of a section sit under the one header rather than repeating it.
        assert_eq!(index("docs"), index("rozi") + 2);
        // A header is a label, not a row: no action glyph.
        assert!(!lines[index("claude")].contains('▸'));
    }

    #[test]
    fn launcher_tab_without_entries_shows_a_muted_placeholder() {
        let lines = sidebar_lines(SidebarTab::Launcher {
            name: SidebarTabId::new("deploy"),
            label: "Deploy".into(),
            entries: Vec::new(),
            env: Vec::new(),
        });
        assert!(
            lines
                .iter()
                .any(|line| line.contains("No launcher entries"))
        );
    }

    #[test]
    fn read_only_command_output_has_one_cell_of_leading_padding() {
        rozi::test_support::on_large_stack(|| {
            let tab_id = SidebarTabId::new("branches");
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            {
                let state = backend.state_mut();
                state.sidebar.shown = [true, false];
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![SidebarTab::Command {
                    name: tab_id.clone(),
                    label: "Branches".into(),
                    command: "git branch --format='%(refname:short)'".into(),
                    interval_secs: 30,
                    on_click: None,
                    group_prefix: None,
                    env: Vec::new(),
                    format: rozi::config::SidebarCommandFormat::Lines,
                }];
                state.sidebar.panels[0].tabs = vec![tab_id.clone()];
                state.sidebar.panels[0].active_tab = Some(tab_id.clone());
                state.sidebar.command_output.insert(
                    tab_id,
                    SidebarCommandOutput {
                        epoch: 1,
                        tree: None,
                        cwd: None,
                        rows: vec![SidebarCommandRow {
                            raw: "master".into(),
                            display: "master".into(),
                            error: false,
                            header: false,
                        }],
                    },
                );
            }
            backend.render();

            let line = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .into_iter()
                .find(|line| line.contains("master"))
                .expect("command output row renders");
            assert!(
                line.starts_with(" master"),
                "row has one-cell inset: {line:?}"
            );
            assert!(
                !line.starts_with("  master"),
                "row is not over-indented: {line:?}"
            );
        });
    }
}

mod settings {
    use rozi::config::{SidebarTab, SidebarTreeConfig, SidebarTreeRoot, SidebarTreeView};
    use rozi::input::Action;
    use rozi::state::SettingsAction;
    use rozi::{AppRoot, Msg};
    use tui_lipan::core::event::{MouseButton, MouseKind};
    use tui_lipan::{TestBackend, prelude::*};

    fn on_stack(test: impl FnOnce() + Send + 'static) {
        rozi::test_support::on_deeper_stack(test);
    }

    fn backend() -> TestBackend<AppRoot> {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 120,
            h: 36,
        });
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.pane.show_workbar = false;
        backend.render();
        backend
    }

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    fn files(b: &TestBackend<AppRoot>) -> SidebarTreeConfig {
        b.state()
            .config
            .sidebar
            .tabs
            .iter()
            .find_map(|tab| match tab {
                SidebarTab::Tree {
                    view: SidebarTreeView::Files,
                    config,
                } => Some(config.clone()),
                _ => None,
            })
            .expect("the Files tab is configured")
    }

    fn saved() -> toml::Table {
        toml::from_str(&std::fs::read_to_string(rozi::config::config_path()).unwrap()).unwrap()
    }

    fn saved_files_setting(key: &str) -> Option<toml::Value> {
        saved()
            .get("sidebar")?
            .get("settings")?
            .get("files")?
            .get(key)
            .cloned()
    }

    /// The row a modal's top border, starting with `title`, is drawn on.
    fn title_row(grid: &str, title: &str) -> usize {
        grid.lines()
            .position(|line| line.contains(title))
            .unwrap_or_else(|| panic!("`{title}` is on screen:\n{grid}"))
    }

    /// The cell `text` starts at, on the last line that shows it: the topmost card's copy.
    fn cell_of(grid: &str, text: &str) -> (u16, u16) {
        let (y, line) = grid
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(text))
            .last()
            .unwrap_or_else(|| panic!("`{text}` is on screen:\n{grid}"));
        let x = line[..line.find(text).unwrap()].chars().count();
        (x as u16, y as u16)
    }

    fn screen(b: &mut TestBackend<AppRoot>) -> String {
        b.render();
        b.capture_frame().to_fixed_grid_lines().join("\n")
    }

    /// The whole round trip from Sidebar tabs: open a tab's settings, change each kind of setting,
    /// reset one, and come back to the same row. Every change lands in `[sidebar.settings.files]` and
    /// takes effect through the reload, not by the picker poking the running config.
    #[test]
    fn a_tabs_settings_are_edited_from_sidebar_tabs_and_saved() {
        on_stack(|| {
            let _config = rozi::test_support::lock_config_file();
            let mut b = backend();
            // Every change reloads this file, so it carries the same quiet chrome the backend starts with.
            std::fs::write(
                rozi::config::config_path(),
                "[animations]\nenabled = false\n\n[pane]\nshow_workbar = false\n",
            )
            .unwrap();

            b.dispatch(Msg::RunAction(Action::OpenSettings)).unwrap();
            b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarTabs))
                .unwrap();
            // A tab with settings counts them beside its status; one without shows only the status.
            let grid = screen(&mut b);
            let row = |name: &str| {
                grid.lines()
                    .find(|line| line.contains(&format!("● {name} ")))
                    .unwrap_or_else(|| panic!("{name} is listed:\n{grid}"))
                    .to_string()
            };
            assert!(row("Files").contains("Enabled · 5 settings"), "{grid}");
            assert!(row("Git").contains("Enabled · 6 settings"), "{grid}");
            assert!(row("Activity").contains("Enabled · 1 setting "), "{grid}");
            assert!(!row("Panes").contains("setting"), "{grid}");

            b.dispatch(Msg::SidebarManagerSelect("panes".into()))
                .unwrap();
            // Panes has no settings, so the action is greyed and does nothing.
            b.send_key(key(KeyCode::Char('e'), KeyMods::CTRL)).unwrap();
            assert!(b.state().tab_settings.is_none());

            b.dispatch(Msg::SidebarManagerSelect("files".into()))
                .unwrap();
            b.send_key(key(KeyCode::Char('e'), KeyMods::CTRL)).unwrap();
            assert_eq!(
                b.state().tab_settings.as_ref().map(|p| p.tab.as_str()),
                Some("files")
            );
            // The settings stack on Sidebar tabs, which stays on screen one row higher.
            let grid = screen(&mut b);
            assert!(grid.contains("Search bar"), "{grid}");
            assert_eq!(
                title_row(&grid, "╭Files settings"),
                title_row(&grid, "╭Sidebar tabs") + 1,
                "{grid}"
            );

            // A toggle flips on Enter. Files has its search bar on by default.
            assert!(files(&b).explorer);
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            assert!(!files(&b).explorer);
            assert_eq!(
                saved_files_setting("explorer").and_then(|v| v.as_bool()),
                Some(false)
            );
            assert!(b.state().tab_settings.is_some(), "the picker stays open");

            // A choice lists its values; choosing one closes the list.
            b.dispatch(Msg::TabSettingsSelect("root".into())).unwrap();
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            // The choice list is a third layer, one row below the settings.
            let grid = screen(&mut b);
            assert!(grid.contains("Pane directory"), "{grid}");
            assert_eq!(
                title_row(&grid, "╭Files · Root"),
                title_row(&grid, "╭Files settings") + 1,
                "{grid}"
            );
            // Moving the highlight and clicking the footer's `apply` applies the highlighted value.
            b.send_key(key(KeyCode::Down, KeyMods::NONE)).unwrap();
            let grid = screen(&mut b);
            let (x, y) = cell_of(&grid, "apply Enter");
            for kind in [
                MouseKind::Down(MouseButton::Left),
                MouseKind::Up(MouseButton::Left),
            ] {
                b.send_mouse(MouseEvent {
                    x,
                    y,
                    kind,
                    mods: KeyMods::NONE,
                })
                .unwrap();
            }
            assert_eq!(files(&b).root, SidebarTreeRoot::Repo);
            assert!(
                b.state()
                    .tab_settings
                    .as_ref()
                    .is_some_and(|p| p.choosing.is_none())
            );

            // A number is typed; one out of range is refused at the prompt.
            b.dispatch(Msg::TabSettingsSelect("max_entries".into()))
                .unwrap();
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            let set_text = |b: &mut TestBackend<AppRoot>, text: &str| {
                let prompt = b
                    .state_mut()
                    .tab_settings
                    .as_mut()
                    .and_then(|p| p.prompt.as_mut())
                    .expect("the prompt is open");
                prompt.input.set_text(text.to_string());
            };
            set_text(&mut b, "0");
            b.dispatch(Msg::TabSettingPromptSubmit).unwrap();
            let grid = screen(&mut b);
            assert!(grid.contains("Enter 1 to 10000"), "{grid}");
            assert_eq!(
                title_row(&grid, "╭Entry limit"),
                title_row(&grid, "╭Files settings") + 1,
                "{grid}"
            );
            assert_eq!(files(&b).max_entries, 2000);
            set_text(&mut b, "50");
            b.dispatch(Msg::TabSettingPromptSubmit).unwrap();
            assert_eq!(files(&b).max_entries, 50);
            assert_eq!(
                saved_files_setting("max_entries").and_then(|v| v.as_integer()),
                Some(50)
            );

            // Reset removes the line, so the default applies again.
            b.send_key(key(KeyCode::Char('r'), KeyMods::CTRL)).unwrap();
            assert_eq!(files(&b).max_entries, 2000);
            assert!(saved_files_setting("max_entries").is_none());
            assert!(!files(&b).explorer, "other settings are untouched");

            // Esc leads back to Sidebar tabs on the same row, then to Settings.
            b.send_key(key(KeyCode::Esc, KeyMods::NONE)).unwrap();
            assert!(b.state().tab_settings.is_none());
            assert_eq!(
                b.state()
                    .sidebar_manager
                    .as_ref()
                    .and_then(|m| m.selected.as_deref()),
                Some("files")
            );
            assert!(screen(&mut b).contains("Sidebar tabs"));
            b.send_key(key(KeyCode::Esc, KeyMods::NONE)).unwrap();
            assert!(b.state().sidebar_manager.is_none());
            assert!(b.state().show_settings);
        });
    }

    /// The Worktrees tab's setting is `[worktrees] delete_branch`, shared with the CLI, and its list of
    /// branches stays open so both copies can be set in one visit.
    #[test]
    fn the_worktrees_tab_edits_which_branches_removal_deletes() {
        on_stack(|| {
            let _config = rozi::test_support::lock_config_file();
            let mut b = backend();
            std::fs::write(
                rozi::config::config_path(),
                "[animations]\nenabled = false\n\n[pane]\nshow_workbar = false\n",
            )
            .unwrap();
            let delete_branch = || {
                saved()
                    .get("worktrees")
                    .and_then(|table| table.get("delete_branch"))
                    .cloned()
            };
            let names = |branches: &[&str]| {
                toml::Value::Array(
                    branches
                        .iter()
                        .map(|name| toml::Value::String((*name).to_string()))
                        .collect(),
                )
            };

            b.dispatch(Msg::RunAction(Action::OpenSettings)).unwrap();
            b.dispatch(Msg::SettingsActivate(SettingsAction::SidebarTabs))
                .unwrap();
            b.dispatch(Msg::SidebarManagerSelect("worktrees".into()))
                .unwrap();
            b.send_key(key(KeyCode::Char('e'), KeyMods::CTRL)).unwrap();
            let grid = screen(&mut b);
            assert!(grid.contains("Worktrees settings"), "{grid}");
            assert!(grid.contains("Delete branch on remove"), "{grid}");
            assert!(grid.contains("None"), "{grid}");

            // Enter opens the two copies; each Enter flips the highlighted one and keeps the list open.
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            assert_eq!(delete_branch(), Some(names(&["local"])));
            assert!(b.state().tab_settings.as_ref().unwrap().choosing.is_some());
            b.send_key(key(KeyCode::Down, KeyMods::NONE)).unwrap();
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            assert_eq!(delete_branch(), Some(names(&["local", "remote"])));
            assert_eq!(
                b.state().config.worktrees.delete_branch,
                rozi::git::worktrees::BranchDeletion {
                    local: true,
                    remote: true
                }
            );
            b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
            assert_eq!(delete_branch(), Some(names(&["local"])));

            // Esc closes the list; the row shows the value, and reset puts the default back.
            b.send_key(key(KeyCode::Esc, KeyMods::NONE)).unwrap();
            let grid = screen(&mut b);
            let row = grid
                .lines()
                .find(|line| line.contains("Delete branch on remove"))
                .unwrap_or_else(|| panic!("the setting is listed:\n{grid}"));
            assert!(
                row.contains("remove  ") && row.contains("Local │") && !row.contains("Remote"),
                "{row}"
            );
            b.send_key(key(KeyCode::Char('r'), KeyMods::CTRL)).unwrap();
            assert_eq!(delete_branch(), None);
            assert!(!b.state().config.worktrees.delete_branch.any());
        });
    }
}
