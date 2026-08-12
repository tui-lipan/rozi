//! Alert controls live only in Alerts, where the four alert channels are configured together.

use hyprmux::AppRoot;
use hyprmux::state::{AlertMode, PaneBorderMode};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

/// Isolated per `AGENTS.md`: building a `AppRoot` otherwise resolves the developer's own config
/// and state directories.
fn alerts_backend(w: u16, h: u16) -> TestBackend<AppRoot> {
    hyprmux::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend.state_mut().show_alerts = true;
    backend
}

/// Tall enough that the whole row list clears the fold, so a status string can be asserted against
/// the drawn grid rather than against whatever happens to be scrolled into view.
fn rendered_rows(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

/// How many rows carry `label`, counted from the snapshot's widget inventory rather than the grid:
/// a row below the fold is still a row that exists.
fn row_count(backend: &mut TestBackend<AppRoot>, label: &str) -> usize {
    backend.render();
    backend
        .capture_ui_snapshot()
        .to_markdown()
        .lines()
        .filter(|line| line.trim() == format!("- `{label}`"))
        .count()
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

/// Rendering the full app tree needs more stack than a default test thread has, same as
/// `sidebar_toggle_smoke`.
fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn alerts smoke thread")
        .join()
        .expect("appearance smoke completes");
}

#[test]
fn alerts_lists_both_effect_rows_with_their_current_modes() {
    on_large_stack(|| {
        let mut backend = alerts_backend(100, 60);
        {
            let state = backend.state_mut();
            state.config.pane.alert_border = AlertMode::Static;
            state.config.workbar.alert.mode = AlertMode::Pulse;
        }
        let frame = rendered_rows(&mut backend);

        // Distinct modes per surface: one shared status string would pass even if both rows read
        // the same config key.
        let pane_border = group_rows(&frame, "Pane border", "Workspace tab");
        let workspace_tab = group_rows(&frame, "Workspace tab", "Status marks");
        assert!(
            pane_border.contains("Effect") && pane_border.contains("Static"),
            "pane alert row is misbound:\n{frame}"
        );
        assert!(
            workspace_tab.contains("Effect") && workspace_tab.contains("Pulse"),
            "workspace alert row is misbound:\n{frame}"
        );
        assert_eq!(
            frame
                .lines()
                .filter(|line| line.trim_start().starts_with("│ Effect"))
                .count(),
            2,
            "expected exactly two Effect rows:\n{frame}"
        );
    });
}

#[test]
fn alerts_rows_report_their_disabled_reasons() {
    on_large_stack(|| {
        // Each row depends on a different parent feature, so a shared "Needs ..." string would hide
        // a row wired to the wrong dependency.
        let mut backend = alerts_backend(100, 60);
        {
            let state = backend.state_mut();
            state.config.pane.border_mode = PaneBorderMode::None;
            state.config.pane.show_workbar = false;
        }
        let frame = rendered_rows(&mut backend);

        let pane_border = group_rows(&frame, "Pane border", "Workspace tab");
        let workspace_tab = group_rows(&frame, "Workspace tab", "Status marks");
        assert!(
            pane_border.contains("Needs pane borders"),
            "pane alert row is misbound:\n{frame}"
        );
        assert!(
            workspace_tab.contains("Needs workbar"),
            "workspace alert row is misbound:\n{frame}"
        );
    });
}

/// A narrow viewport scrolls rows out of the grid; the rows must still exist so search can reach
/// them.
#[test]
fn alerts_keeps_both_effect_rows_on_a_narrow_viewport() {
    on_large_stack(|| {
        let mut backend = alerts_backend(70, 24);
        backend.state_mut().config.workbar.alert.mode = AlertMode::Off;
        assert_eq!(row_count(&mut backend, "Effect"), 4, "effect rows dropped");
    });
}

#[test]
fn alerts_renders_the_accepted_groups_and_row_labels() {
    on_large_stack(|| {
        let mut backend = alerts_backend(100, 60);
        let frame = rendered_rows(&mut backend);

        for (group, rows, next_group) in [
            (
                "General",
                &["Do not disturb", "Bell urgency"][..],
                "Pane border",
            ),
            ("Pane border", &["Effect"][..], "Workspace tab"),
            (
                "Workspace tab",
                &["Effect", "Highlight"][..],
                "Status marks",
            ),
            (
                "Status marks",
                &["Bell", "Blocked", "Finished", "Working", "Idle"][..],
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

#[test]
fn appearance_no_longer_offers_alert_rows() {
    on_large_stack(|| {
        let mut backend = alerts_backend(100, 60);
        backend.state_mut().show_alerts = false;
        backend.state_mut().show_appearance = true;
        assert_eq!(row_count(&mut backend, "Effect"), 0);
    });
}
