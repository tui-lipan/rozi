//! The two alert rows are the only user-facing switch for pane-border and workspace-tab alerts, so
//! pin that both reach the Appearance overlay and report their live mode. A row wired everywhere
//! except the picker compiles and tests green while being unreachable, which is exactly the failure
//! these guard against.

use hyprmux::HyprmuxApp;
use hyprmux::state::{AlertMode, PaneBorderMode};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

/// Isolated per `AGENTS.md`: building a `HyprmuxApp` otherwise resolves the developer's own config
/// and state directories.
fn appearance_backend(w: u16, h: u16) -> TestBackend<HyprmuxApp> {
    hyprmux::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend.state_mut().show_appearance = true;
    backend
}

/// Tall enough that the whole row list clears the fold, so a status string can be asserted against
/// the drawn grid rather than against whatever happens to be scrolled into view.
fn rendered_rows(backend: &mut TestBackend<HyprmuxApp>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

/// How many rows carry `label`, counted from the snapshot's widget inventory rather than the grid:
/// a row below the fold is still a row that exists.
fn row_count(backend: &mut TestBackend<HyprmuxApp>, label: &str) -> usize {
    backend.render();
    backend
        .capture_ui_snapshot()
        .to_markdown()
        .lines()
        .filter(|line| line.trim() == format!("- `{label}`"))
        .count()
}

/// Rendering the full app tree needs more stack than a default test thread has, same as
/// `sidebar_toggle_smoke`.
fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn appearance smoke thread")
        .join()
        .expect("appearance smoke completes");
}

#[test]
fn appearance_lists_both_alert_rows_with_their_current_modes() {
    on_large_stack(|| {
        let mut backend = appearance_backend(100, 60);
        {
            let state = backend.state_mut();
            state.config.pane.alert_border = AlertMode::Static;
            state.config.workbar.alert.mode = AlertMode::Pulse;
        }
        let frame = rendered_rows(&mut backend);

        // Distinct modes per surface: one shared status string would pass even if both rows read
        // the same config key.
        assert!(
            frame.contains("Static"),
            "the pane alert row does not show its own mode:\n{frame}"
        );
        assert!(
            frame.contains("Pulse"),
            "the workbar alert row does not show its own mode:\n{frame}"
        );
        assert_eq!(
            row_count(&mut backend, "Alert"),
            2,
            "expected a pane alert row and a workbar alert row"
        );
    });
}

#[test]
fn appearance_alert_rows_report_their_disabled_reasons() {
    on_large_stack(|| {
        // Each row depends on a different parent feature, so a shared "Needs ..." string would hide
        // a row wired to the wrong dependency.
        let mut backend = appearance_backend(100, 60);
        {
            let state = backend.state_mut();
            state.config.pane.border_mode = PaneBorderMode::None;
            state.config.pane.show_workbar = false;
        }
        let frame = rendered_rows(&mut backend);

        assert!(
            frame.contains("Needs pane borders"),
            "the pane alert row stays active without borders:\n{frame}"
        );
        assert!(
            frame.contains("Needs workbar"),
            "the workbar alert row stays active without a workbar:\n{frame}"
        );
    });
}

/// A narrow viewport scrolls rows out of the grid; the rows must still exist so search can reach
/// them.
#[test]
fn appearance_keeps_both_alert_rows_on_a_narrow_viewport() {
    on_large_stack(|| {
        let mut backend = appearance_backend(70, 24);
        backend.state_mut().config.workbar.alert.mode = AlertMode::Off;
        assert_eq!(row_count(&mut backend, "Alert"), 2, "alert rows dropped");
    });
}
