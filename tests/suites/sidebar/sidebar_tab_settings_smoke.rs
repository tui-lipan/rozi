use rozi::config::{SidebarTab, SidebarTreeConfig, SidebarTreeRoot, SidebarTreeView};
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
        let grid = screen(&mut b);
        assert!(grid.contains("Files settings"), "{grid}");
        assert!(grid.contains("Search bar"), "{grid}");
        assert!(!grid.contains("Sidebar tabs"), "{grid}");

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
        let grid = screen(&mut b);
        assert!(grid.contains("Files · Root"), "{grid}");
        assert!(grid.contains("Pane directory"), "{grid}");
        b.dispatch(Msg::TabSettingsChoose("repo".into())).unwrap();
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

/// The palette command opens the settings of the focused panel's tab, and is unavailable when
/// that tab has none or the sidebar is hidden.
#[test]
fn the_palette_command_opens_the_active_tabs_settings() {
    on_stack(|| {
        let mut b = backend();
        b.dispatch(Msg::RunAction(Action::SidebarTabSettings))
            .unwrap();
        assert!(
            b.state().tab_settings.is_none(),
            "a hidden sidebar has no active tab"
        );

        b.state_mut().sidebar.shown = [true, false];
        let config = b.state().config.sidebar.clone();
        b.state_mut().sidebar.apply_configured_panels(&config);
        let active = b.state().sidebar.active_tab().cloned();
        assert_eq!(active.as_ref().map(|id| id.as_str()), Some("activity"));
        b.dispatch(Msg::RunAction(Action::SidebarTabSettings))
            .unwrap();
        assert_eq!(
            b.state().tab_settings.as_ref().map(|p| p.tab.as_str()),
            Some("activity")
        );
        assert!(screen(&mut b).contains("Activity settings"));

        // Opened from the palette, Esc goes back to the panes.
        b.send_key(key(KeyCode::Esc, KeyMods::NONE)).unwrap();
        assert!(b.state().tab_settings.is_none());
        assert!(b.state().sidebar_manager.is_none());
    });
}
