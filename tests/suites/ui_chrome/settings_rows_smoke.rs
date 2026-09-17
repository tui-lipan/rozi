//! Settings keeps persisted appearance and alert preferences in one searchable grouped list.

use rozi::AppRoot;
use rozi::state::{AlertMode, PaneBorderMode, SettingsAction, SettingsTab};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{CapStyle, KeyCode, KeyEvent, KeyMods, Rect};

/// Isolated per `AGENTS.md`: building a `AppRoot` otherwise resolves the developer's own config
/// and state directories.
fn settings_backend(w: u16, h: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend.state_mut().show_settings = true;
    backend.state_mut().settings_navigation.tab = SettingsTab::All;
    backend
}

/// Tall enough that the whole row list clears the fold, so a status string can be asserted against
/// the drawn grid rather than against whatever happens to be scrolled into view.
fn rendered_rows(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn type_query(backend: &mut TestBackend<AppRoot>, query: &str) {
    backend.render();
    for character in query.chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(character),
                mods: KeyMods::NONE,
            })
            .expect("type settings query");
    }
}

fn group_rows<'a>(frame: &'a str, group: &str, next_group: &str) -> &'a str {
    let start = frame.find(group).expect("rendered group");
    let end = if next_group.is_empty() {
        frame.len()
    } else {
        frame[start..]
            .find(next_group)
            .map(|offset| start + offset)
            .unwrap_or(frame.len())
    };
    &frame[start..end]
}

fn setting_label_matches(line: &str, label: &str) -> bool {
    let Some(pos) = line.find(label) else {
        return false;
    };
    let after = &line[pos + label.len()..];
    after.is_empty() || after.starts_with("  ")
}

fn setting_row<'a>(frame: &'a str, label: &str) -> &'a str {
    frame
        .lines()
        .find(|line| setting_label_matches(line, label))
        .unwrap_or_else(|| panic!("rendered Settings row `{label}`:\n{frame}"))
}

fn list_body(frame: &str) -> String {
    let lines: Vec<_> = frame.lines().collect();
    let tabs = lines
        .iter()
        .position(|line| {
            line.contains("General")
                && line.contains("Panes")
                && line.contains("Bars")
                && line.contains("Alerts")
                && line.contains("Sessions")
        })
        .unwrap_or_else(|| panic!("settings tab strip:\n{frame}"));
    lines[tabs + 1..].join("\n")
}

fn body_has_group_header(body: &str, name: &str) -> bool {
    body.lines().any(|line| setting_label_matches(line, name))
}

fn list_gap_after_tabs(frame: &str) -> usize {
    list_body(frame)
        .lines()
        .take_while(|line| {
            line.chars()
                .all(|ch| ch.is_whitespace() || matches!(ch, '│' | '▐' | '▌'))
        })
        .count()
}

/// Rendering the full app tree needs more stack than a default test thread has, same as
/// `sidebar_toggle_smoke`.
fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn settings smoke thread")
        .join()
        .expect("settings smoke completes");
}

#[test]
fn settings_lists_both_effect_rows_with_their_current_modes() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        {
            let state = backend.state_mut();
            state.config.pane.alert_border = AlertMode::Static;
            state.config.workbar.alert.mode = AlertMode::Pulse;
        }
        let frame = rendered_rows(&mut backend);

        // Distinct modes per surface: one shared status string would pass even if both rows read
        // the same config key.
        assert!(
            setting_row(&frame, "Pane border effect").contains("Static"),
            "pane alert row is misbound:\n{frame}"
        );
        assert!(
            setting_row(&frame, "Workspace tab effect").contains("Pulse"),
            "workspace alert row is misbound:\n{frame}"
        );
    });
}

#[test]
fn settings_rows_report_their_disabled_reasons() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        {
            let state = backend.state_mut();
            state.config.pane.border_mode = PaneBorderMode::None;
            state.config.pane.alert_border = AlertMode::Pulse;
        }
        backend.render();
        let capture = backend.capture_frame();
        let lines = capture.to_fixed_grid_lines();
        let frame = lines.join("\n");
        assert!(
            setting_row(&frame, "Pane border effect").contains("Needs pane borders"),
            "{frame}"
        );
        let search = lines
            .iter()
            .position(|line| line.contains("Search settings"))
            .expect("settings search");
        let divider = &lines[search + 1];
        let rule = divider.find('─').expect("search divider");
        let divider_fg = capture.cell(rule as u16, (search + 1) as u16).fg;
        let row = lines
            .iter()
            .position(|line| setting_label_matches(line, "Pane border effect"))
            .expect("disabled settings row");
        let label = lines[row]
            .find("Pane border effect")
            .expect("disabled label");
        let reason = lines[row]
            .find("Needs pane borders")
            .expect("disabled reason");
        assert_eq!(capture.cell(label as u16, row as u16).fg, divider_fg);
        assert_eq!(capture.cell(reason as u16, row as u16).fg, divider_fg);
        type_query(&mut backend, "pane border effect");
        key(&mut backend, KeyCode::Enter);
        assert!(backend.state().settings_choice.is_none());
        assert_eq!(backend.state().config.pane.alert_border, AlertMode::Pulse);
    });
}

/// A narrow viewport scrolls both rows out of the unfiltered grid; search must still reach both
/// effect controls in their shared Alerts group.
#[test]
fn settings_keeps_both_effect_rows_on_a_narrow_viewport() {
    on_large_stack(|| {
        let mut backend = settings_backend(70, 24);
        backend.state_mut().config.workbar.alert.mode = AlertMode::Off;
        type_query(&mut backend, "effect");
        let frame = rendered_rows(&mut backend);
        assert!(frame.contains("Alerts"), "Alerts group missing:\n{frame}");
        assert!(
            frame.contains("Pane border effect"),
            "pane effect missing:\n{frame}"
        );
        assert!(
            frame.contains("Workspace tab effect"),
            "tab effect missing:\n{frame}"
        );
    });
}

#[test]
fn settings_all_keeps_every_control_available() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        let frame = rendered_rows(&mut backend);
        let rows = SettingsAction::all().len();
        assert!(frame.contains(&format!("{rows}/{rows}")), "{frame}");
        for label in [
            "Theme",
            "Animations",
            "Workspace switching animation",
            "Nerd icons",
            "Which-key",
            "Focus on hover",
            "Border",
            "Selection",
            "Terminal padding",
            "Background follows terminal",
            "Show titlebar",
            "Layout",
            "Show workbar",
            "Position",
            "Background",
            "Badge style",
            "Powerline",
            "Focused background",
            "Focused border",
            "Focused titlebar",
            "Border mode",
            "Border style",
            "Floating border",
            "Scratchpad border",
            "Fullscreen border",
            "Open/close animation",
            "Tab strip",
            "Bell urgency",
            "Pane border effect",
            "Workspace tab effect",
            "Workspace tab highlight",
            "Bell mark",
            "Blocked mark",
            "Finished mark",
            "Working mark",
            "Idle mark",
            "Show notifications",
            "Blocked",
            "Finished",
            "Exit",
            "Exit with error",
            "Play sounds",
            "Bell",
            "Startup mode",
            "Layout autosave",
            "Resurrect named sessions",
            "Restored running commands",
        ] {
            setting_row(&frame, label);
        }
        let pickers = group_rows(&frame, "Pickers", "Panes");
        setting_row(pickers, "Border");
        setting_row(pickers, "Tab strip");
        setting_row(pickers, "Tab style");
        setting_row(pickers, "Selection");
        let titlebar = group_rows(&frame, "Titlebar", "Workbar");
        setting_row(titlebar, "Style");
        let workbar = group_rows(&frame, "Workbar", "Sidebar");
        setting_row(workbar, "Gap");
        setting_row(workbar, "Style");
        setting_row(workbar, "Tab style");
        let sidebar = group_rows(&frame, "Sidebar", "Alerts");
        setting_row(sidebar, "Background follows terminal");
        setting_row(sidebar, "Gap");
        setting_row(sidebar, "Tab style");
        assert!(!frame.contains("Extensions"), "{frame}");
        let body = list_body(&frame);
        for group in [
            "General",
            "Pickers",
            "Panes",
            "Titlebar",
            "Workbar",
            "Sidebar",
            "Alerts",
            "Desktop notifications",
            "Sounds",
            "Sessions",
        ] {
            assert!(
                body_has_group_header(&body, group),
                "All is missing the {group} header:\n{frame}"
            );
        }
    });
}

#[test]
fn settings_omits_the_inner_header_that_repeats_the_active_tab() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        backend.state_mut().settings_navigation.tab = SettingsTab::General;
        let general = rendered_rows(&mut backend);
        let general_body = list_body(&general);
        assert!(
            !body_has_group_header(&general_body, "General"),
            "General repeats its tab name:\n{general}"
        );
        setting_row(&general, "Theme");
        assert!(
            body_has_group_header(&general_body, "Pickers"),
            "General is missing Pickers:\n{general}"
        );

        backend.state_mut().settings_navigation.tab = SettingsTab::Panes;
        let panes = rendered_rows(&mut backend);
        let panes_body = list_body(&panes);
        assert!(
            !body_has_group_header(&panes_body, "Panes"),
            "Panes repeats its tab name:\n{panes}"
        );
        assert!(
            body_has_group_header(&panes_body, "Titlebar"),
            "Panes is missing Titlebar:\n{panes}"
        );

        backend.state_mut().settings_navigation.tab = SettingsTab::Bars;
        let bars = rendered_rows(&mut backend);
        let bars_body = list_body(&bars);
        assert!(
            body_has_group_header(&bars_body, "Workbar"),
            "Bars is missing Workbar:\n{bars}"
        );
        assert!(
            body_has_group_header(&bars_body, "Sidebar"),
            "Bars is missing Sidebar:\n{bars}"
        );

        backend.state_mut().settings_navigation.tab = SettingsTab::Alerts;
        let alerts = rendered_rows(&mut backend);
        let alerts_body = list_body(&alerts);
        assert!(
            !body_has_group_header(&alerts_body, "Alerts"),
            "Alerts repeats its tab name:\n{alerts}"
        );
        assert!(
            body_has_group_header(&alerts_body, "Desktop notifications"),
            "Alerts is missing Desktop notifications:\n{alerts}"
        );
        assert!(
            body_has_group_header(&alerts_body, "Sounds"),
            "Alerts is missing Sounds:\n{alerts}"
        );

        backend.state_mut().settings_navigation.tab = SettingsTab::Sessions;
        let sessions = rendered_rows(&mut backend);
        let sessions_body = list_body(&sessions);
        assert!(
            !body_has_group_header(&sessions_body, "Sessions"),
            "Sessions repeats its tab name:\n{sessions}"
        );
        setting_row(&sessions, "Startup mode");

        type_query(&mut backend, "titlebar");
        let search = rendered_rows(&mut backend);
        assert!(
            search.contains("Panes › Titlebar"),
            "search dropped the Titlebar breadcrumb:\n{search}"
        );
    });
}

#[test]
fn settings_matches_keybindings_chrome_without_hints() {
    on_large_stack(|| {
        let mut backend = settings_backend(80, 30);
        backend.state_mut().settings_selected = Some(SettingsAction::ToggleAnimations);
        let frame = rendered_rows(&mut backend);
        let lines: Vec<_> = frame.lines().collect();
        let search = lines
            .iter()
            .position(|line| line.contains("Search settings"))
            .expect("settings search field");
        let tabs = lines
            .iter()
            .position(|line| {
                line.contains("General")
                    && line.contains("Panes")
                    && line.contains("Bars")
                    && line.contains("Alerts")
                    && line.contains("Sessions")
            })
            .expect("settings tab strip");
        assert!(search < tabs, "{frame}");
        assert!(!frame.contains("change Enter"), "{frame}");
        assert!(!frame.contains("←→ change"), "{frame}");
        assert!(
            !frame.contains("previous Left") && !frame.contains("next Right"),
            "{frame}"
        );
    });
}

/// The behavioral group sits last and reads `[session]`, which no other row does.
#[test]
fn settings_reports_startup_and_session_values() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        {
            let state = backend.state_mut();
            state.config.session.startup = rozi::config::SessionStartup::Last;
            state.config.session.autosave = true;
        }
        let frame = rendered_rows(&mut backend);

        assert!(
            setting_row(&frame, "Startup mode").contains("Last"),
            "startup row is misbound:\n{frame}"
        );
        assert!(
            setting_row(&frame, "Layout autosave").contains("Enabled"),
            "autosave row is misbound:\n{frame}"
        );
        assert!(
            setting_row(&frame, "Resurrect named sessions").contains("Enabled"),
            "resurrect row is misbound:\n{frame}"
        );
    });
}

/// Sidebar chrome lives in its own group, so a shared label with Workbar cannot hide a miswire.
#[test]
fn settings_reports_sidebar_values() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 160);
        {
            let state = backend.state_mut();
            state.config.sidebar.background_follows_terminal = true;
            state.config.sidebar.gap = false;
            state.config.sidebar.background = false;
            state.config.sidebar.tab_style = tui_lipan::prelude::CapStyle::Round;
        }
        let frame = rendered_rows(&mut backend);
        let sidebar = group_rows(&frame, "Sidebar", "Alerts");
        assert!(
            setting_row(sidebar, "Background follows terminal").contains("Enabled"),
            "sidebar backdrop row is misbound:\n{frame}"
        );
        assert!(
            setting_row(sidebar, "Gap").contains("Disabled"),
            "sidebar gap row is misbound:\n{frame}"
        );
        assert!(
            setting_row(sidebar, "Tab strip").contains("Disabled"),
            "sidebar tab strip row is misbound:\n{frame}"
        );
        assert!(
            setting_row(sidebar, "Tab style").contains("Round"),
            "sidebar tab style row is misbound:\n{frame}"
        );
    });
}

#[test]
fn settings_filtered_duplicate_labels_keep_their_group_headers() {
    on_large_stack(|| {
        let mut backend = settings_backend(80, 30);
        type_query(&mut backend, "blocked");
        let frame = rendered_rows(&mut backend);
        for group in ["Alerts", "Desktop notifications", "Sounds"] {
            assert!(
                frame.contains(group),
                "filtered Blocked row lost {group} header:\n{frame}"
            );
        }
        assert_eq!(
            frame
                .lines()
                .filter(|line| {
                    line.contains("Blocked mark") || line.trim_start().starts_with("│ Blocked ")
                })
                .count(),
            3,
            "expected one Blocked row in each alert channel:\n{frame}"
        );
    });
}

#[test]
fn settings_search_does_not_leave_a_double_gap_under_the_tabs() {
    on_large_stack(|| {
        let mut backend = settings_backend(80, 24);
        type_query(&mut backend, "bg");
        let later = rendered_rows(&mut backend);
        assert_eq!(
            list_gap_after_tabs(&later),
            1,
            "later-group search opened a double gap:\n{later}"
        );
        assert!(later.contains("Panes"), "{later}");
        key(&mut backend, KeyCode::Esc);
        type_query(&mut backend, "theme");
        let first = rendered_rows(&mut backend);
        assert_eq!(
            list_gap_after_tabs(&first),
            1,
            "first-group search gap drifted:\n{first}"
        );
    });
}

#[test]
fn picker_tab_strip_and_selection_caps_follow_pane_config() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 40);
        backend.state_mut().settings_navigation.tab = SettingsTab::General;
        let theme = backend.state().theme.clone();
        let host = theme.surface.element;
        let strip = host.elevate_by(0.05);
        backend.render();
        let capture = backend.capture_frame();
        let lines = capture.to_fixed_grid_lines();
        let tabs = lines
            .iter()
            .position(|line| line.contains("General") && line.contains("Panes"))
            .expect("settings tab strip");
        let panes = lines[tabs].find("Panes").expect("inactive Panes tab");
        assert_eq!(
            capture.cell(panes as u16, tabs as u16).bg,
            strip,
            "picker tab strip should lift like the sidebar:\n{}",
            lines.join("\n")
        );

        backend.state_mut().config.pane.picker_tab_background = false;
        backend.render();
        let capture = backend.capture_frame();
        let lines = capture.to_fixed_grid_lines();
        let tabs = lines
            .iter()
            .position(|line| line.contains("General") && line.contains("Panes"))
            .expect("settings tab strip");
        let panes = lines[tabs].find("Panes").expect("inactive Panes tab");
        assert_eq!(
            capture.cell(panes as u16, tabs as u16).bg,
            host,
            "picker tab strip off should match the picker body:\n{}",
            lines.join("\n")
        );

        backend.state_mut().config.pane.picker_selection_style = CapStyle::Round;
        backend.state_mut().settings_selected = Some(SettingsAction::Theme);
        let rendered = rendered_rows(&mut backend);
        assert!(
            rendered.contains('\u{e0b6}'),
            "round picker selection cap missing:\n{rendered}"
        );
        assert!(
            !rendered.contains("\u{e0b6} "),
            "capped selection kept leading item padding:\n{rendered}"
        );
        assert!(
            !rendered.contains(" \u{e0b4}"),
            "capped selection kept trailing item padding:\n{rendered}"
        );
        assert!(
            rendered.contains("│ Animations"),
            "unselected rows should keep their inset:\n{rendered}"
        );
    });
}

#[test]
fn picker_border_style_changes_settings_frame_glyphs() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 40);
        let rounded = rendered_rows(&mut backend);
        assert!(
            rounded.lines().any(|line| line.contains("╭Settings")),
            "default picker frame is rounded:\n{rounded}"
        );

        backend.state_mut().config.pane.picker_border_style = rozi::state::PaneBorderStyle::Plain;
        let plain = rendered_rows(&mut backend);
        assert!(
            plain.lines().any(|line| line.contains("┌Settings")),
            "plain picker frame should use square corners:\n{plain}"
        );
        assert!(
            !plain.contains("╭Settings"),
            "rounded corners should be gone:\n{plain}"
        );
    });
}

#[test]
fn workspace_animation_setting_is_searchable_persisted_and_gated_by_master() {
    on_large_stack(|| {
        use rozi::state::SettingsAction::ToggleWorkspaceAnimation;
        let mut backend = settings_backend(90, 30);
        backend.state_mut().config.animations.enabled = true;
        backend.state_mut().config.animations.workspace = true;
        type_query(&mut backend, "workspace switching");
        assert!(
            setting_row(
                &rendered_rows(&mut backend),
                "Workspace switching animation"
            )
            .contains("Enabled")
        );
        backend
            .dispatch(rozi::Msg::SettingsActivate(ToggleWorkspaceAnimation))
            .unwrap();
        assert!(!backend.state().config.animations.workspace);
        assert!(!rozi::config::load_config().config.animations.workspace);
        assert!(backend.state().show_settings);
        assert_eq!(
            backend.state().settings_selected,
            Some(ToggleWorkspaceAnimation)
        );
        backend.state_mut().config.animations.enabled = false;
        backend
            .dispatch(rozi::Msg::SettingsActivate(ToggleWorkspaceAnimation))
            .unwrap();
        assert!(!backend.state().config.animations.workspace);
        assert_eq!(
            ToggleWorkspaceAnimation.disabled_reason(&backend.state().config),
            Some("Needs animations")
        );
    });
}

fn key(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
    key_mods(backend, code, KeyMods::NONE);
}

fn key_mods(backend: &mut TestBackend<AppRoot>, code: KeyCode, mods: KeyMods) {
    backend.send_key(KeyEvent { code, mods }).unwrap();
    backend.render();
}

#[test]
fn settings_searches_globally_and_escape_restores_browse_selection() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 35);
        backend.state_mut().settings_navigation.tab = SettingsTab::Panes;
        backend.state_mut().settings_selected = Some(SettingsAction::CycleBorderMode);
        backend.render();
        type_query(&mut backend, "startup");
        let frame = rendered_rows(&mut backend);
        assert!(
            frame.contains("Sessions") && frame.contains("Startup mode"),
            "{frame}"
        );
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleStartupMode)
        );
        key(&mut backend, KeyCode::Esc);
        assert!(backend.state().show_settings);
        assert_eq!(backend.state().settings_navigation.tab, SettingsTab::Panes);
        assert!(backend.state().settings_navigation.query.text().is_empty());
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleBorderMode)
        );
        let frame = rendered_rows(&mut backend);
        assert!(!frame.contains("Startup mode"), "{frame}");
        key(&mut backend, KeyCode::Esc);
        assert!(!backend.state().show_settings);
    });
}

#[test]
fn settings_tabs_remember_their_highlighted_row() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 50);
        backend.state_mut().settings_navigation.tab = SettingsTab::General;
        backend
            .dispatch(rozi::Msg::SettingsSelect(SettingsAction::CycleWhichKey))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::Panes))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsSelect(SettingsAction::CycleBorderMode))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::All))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsSelect(SettingsAction::ToggleNerdIcons))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::General))
            .unwrap();
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleWhichKey)
        );
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::Panes))
            .unwrap();
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleBorderMode)
        );
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::All))
            .unwrap();
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::ToggleNerdIcons)
        );

        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::Panes))
            .unwrap();
        type_query(&mut backend, "startup");
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleStartupMode)
        );
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::Alerts))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsTabSelected(SettingsTab::Panes))
            .unwrap();
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleBorderMode)
        );
    });
}

#[test]
fn settings_arrows_switch_tabs_and_shift_enter_opens_a_choice_picker() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 35);
        backend.state_mut().settings_navigation.tab = SettingsTab::General;
        backend.render();
        key(&mut backend, KeyCode::BackTab);
        assert_eq!(backend.state().settings_navigation.tab, SettingsTab::All);
        key(&mut backend, KeyCode::Tab);
        assert_eq!(
            backend.state().settings_navigation.tab,
            SettingsTab::General
        );
        key(&mut backend, KeyCode::Right);
        assert_eq!(backend.state().settings_navigation.tab, SettingsTab::Panes);
        key(&mut backend, KeyCode::Left);
        assert_eq!(
            backend.state().settings_navigation.tab,
            SettingsTab::General
        );
        type_query(&mut backend, "pane open/close");
        let original = backend.state().config.animations.pane_style;
        backend.state_mut().config.animations.enabled = true;
        key(&mut backend, KeyCode::Enter);
        assert!(backend.state().settings_choice.is_none());
        assert_ne!(backend.state().config.animations.pane_style, original);
        key_mods(&mut backend, KeyCode::Enter, KeyMods::SHIFT);
        assert!(backend.state().settings_choice.is_some());
        let cycled = backend.state().config.animations.pane_style;
        backend
            .dispatch(rozi::Msg::SettingsChoiceSelect(0))
            .unwrap();
        backend
            .dispatch(rozi::Msg::SettingsChoiceSelect(1))
            .unwrap();
        key(&mut backend, KeyCode::Esc);
        assert!(backend.state().settings_choice.is_none());
        assert_eq!(backend.state().config.animations.pane_style, cycled);
        assert_eq!(
            backend.state().settings_navigation.tab,
            SettingsTab::General
        );
    });
}

#[test]
fn settings_choice_lists_every_option() {
    on_large_stack(|| {
        let mut backend = settings_backend(80, 30);
        backend
            .dispatch(rozi::Msg::SettingsOpenChoice(SettingsAction::CycleWhichKey))
            .unwrap();
        let frame = rendered_rows(&mut backend);
        for label in ["Off", "Instant", "Short", "Long"] {
            assert!(frame.contains(label), "{label} missing:\n{frame}");
        }
        assert!(frame.contains("Search…"), "{frame}");
        assert!(frame.contains("current"), "{frame}");
        assert!(!frame.contains('‹') && !frame.contains('›'), "{frame}");
        assert!(!frame.contains("change Enter"), "{frame}");
        assert!(!frame.contains("choose"), "{frame}");
    });
}

#[test]
fn settings_empty_search_does_not_edit_a_stale_selection() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 35);
        type_query(&mut backend, "nothing_matches_this_setting_123");
        let frame = rendered_rows(&mut backend);
        assert!(frame.contains("No matches"), "{frame}");
        assert_eq!(backend.state().settings_selected, None);
        let animations = backend.state().config.animations.enabled;
        key(&mut backend, KeyCode::Enter);
        assert_eq!(backend.state().config.animations.enabled, animations);
        assert!(backend.state().show_settings);
        key(&mut backend, KeyCode::Esc);
        assert!(backend.state().show_settings);
        assert!(backend.state().settings_navigation.query.text().is_empty());
    });
}

#[test]
fn settings_categories_cover_all_controls_and_keep_pane_motion_local() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 50);
        for (tab, count, expected) in [
            (SettingsTab::General, 10, "Workspace switching animation"),
            (SettingsTab::Panes, 14, "Open/close animation"),
            (SettingsTab::Bars, 12, "Show workbar"),
            (SettingsTab::Alerts, 19, "Bell urgency"),
            (SettingsTab::Sessions, 4, "Startup mode"),
        ] {
            backend
                .dispatch(rozi::Msg::SettingsTabSelected(tab))
                .unwrap();
            let frame = rendered_rows(&mut backend);
            assert!(
                frame.contains(&format!("{count}/{count}")),
                "{tab:?}: {frame}"
            );
            assert!(frame.contains(expected), "{tab:?}: {frame}");
        }
    });
}

#[test]
fn deleting_the_query_restores_category_and_selection() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 35);
        backend.state_mut().settings_navigation.tab = SettingsTab::Panes;
        backend.state_mut().settings_selected = Some(SettingsAction::CycleBorderMode);
        type_query(&mut backend, "startup");
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleStartupMode)
        );
        for _ in 0..7 {
            key(&mut backend, KeyCode::Backspace);
        }
        assert_eq!(backend.state().settings_navigation.tab, SettingsTab::Panes);
        assert_eq!(
            backend.state().settings_selected,
            Some(SettingsAction::CycleBorderMode)
        );
    });
}
