//! Settings keeps persisted appearance and alert preferences in one searchable grouped list.

use rozi::AppRoot;
use rozi::state::{AlertMode, PaneBorderMode};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

/// Isolated per `AGENTS.md`: building a `AppRoot` otherwise resolves the developer's own config
/// and state directories.
fn settings_backend(w: u16, h: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend.state_mut().show_settings = true;
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

fn setting_row<'a>(frame: &'a str, label: &str) -> &'a str {
    frame
        .lines()
        .find(|line| line.contains(label))
        .unwrap_or_else(|| panic!("rendered Settings row `{label}`:\n{frame}"))
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
        let mut backend = settings_backend(100, 80);
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
        // Each row depends on a different parent feature, so a shared "Needs ..." string would hide
        // a row wired to the wrong dependency.
        let mut backend = settings_backend(100, 80);
        {
            let state = backend.state_mut();
            state.config.pane.border_mode = PaneBorderMode::None;
            state.config.pane.show_workbar = false;
        }
        let frame = rendered_rows(&mut backend);

        assert!(
            setting_row(&frame, "Pane border effect").contains("Needs pane borders"),
            "pane alert row is misbound:\n{frame}"
        );
        assert!(
            setting_row(&frame, "Workspace tab effect").contains("Needs workbar"),
            "workspace alert row is misbound:\n{frame}"
        );
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
fn settings_renders_the_accepted_groups_and_row_labels() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 110);
        let frame = rendered_rows(&mut backend);
        // Counted from the action list rather than hardcoded, so a row added to one and not the
        // other fails here instead of drifting until someone notices a setting nobody can search.
        let rows = rozi::state::SettingsAction::all().len();
        assert!(
            frame.contains(&format!("{rows}/{rows}")),
            "expected {rows} Settings rows:\n{frame}"
        );

        for (group, rows, next_group) in [
            (
                "General",
                &[
                    "Theme",
                    "Terminal padding",
                    "Animations",
                    "Which-key",
                    "Focus on hover",
                    "Background follows terminal",
                ][..],
                "Titlebar",
            ),
            (
                "Titlebar",
                &["Show titlebar", "Layout", "Style"][..],
                "Workbar",
            ),
            (
                "Workbar",
                &[
                    "Show workbar",
                    "Position",
                    "Gap",
                    "Style",
                    "Badge style",
                    "Tab style",
                    "Powerline",
                ][..],
                "Panes",
            ),
            (
                "Panes",
                &[
                    "Focused background",
                    "Focused border",
                    "Focused titlebar",
                    "Border mode",
                    "Border style",
                    "Open/close animation",
                ][..],
                "Alerts",
            ),
            (
                "Alerts",
                &[
                    "Bell urgency",
                    "Pane border effect",
                    "Workspace tab effect",
                    "Workspace tab highlight",
                    "Bell mark",
                    "Blocked mark",
                    "Finished mark",
                    "Working mark",
                    "Idle mark",
                ][..],
                "Desktop notifications",
            ),
            (
                "Desktop notifications",
                &[
                    "Show notifications",
                    "Blocked",
                    "Finished",
                    "Exit",
                    "Exit with error",
                ][..],
                "Sounds",
            ),
            (
                "Sounds",
                &[
                    "Play sounds",
                    "Bell",
                    "Blocked",
                    "Finished",
                    "Exit with error",
                ][..],
                "Sessions",
            ),
            (
                "Sessions",
                &[
                    "Startup mode",
                    "Layout autosave",
                    "Resurrect named sessions",
                ][..],
                "",
            ),
        ] {
            let rows_in_group = group_rows(&frame, group, next_group);
            for label in rows {
                assert!(
                    rows_in_group.contains(label),
                    "{group} is missing {label}:\n{frame}"
                );
            }
        }
    });
}

/// The behavioral group sits last and reads `[session]`, which no other row does.
#[test]
fn settings_reports_startup_and_session_values() {
    on_large_stack(|| {
        let mut backend = settings_backend(100, 110);
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
