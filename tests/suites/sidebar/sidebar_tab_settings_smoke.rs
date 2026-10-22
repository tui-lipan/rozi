use rozi::config::{SidebarTab, SidebarTreeConfig, SidebarTreeRoot, SidebarTreeView};
use rozi::input::Action;
use rozi::state::SettingsAction;
use rozi::{AppRoot, Msg};
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::{TestBackend, prelude::*};

fn on_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
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

        // A toggle flips on Enter.
        b.send_key(key(KeyCode::Enter, KeyMods::NONE)).unwrap();
        assert!(files(&b).explorer);
        assert_eq!(
            saved_files_setting("explorer").and_then(|v| v.as_bool()),
            Some(true)
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
        assert!(files(&b).explorer, "other settings are untouched");

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
