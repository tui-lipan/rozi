//! Render-level regression coverage for pane alert colors across border modes.

use hyprmux::HyprmuxApp;
use hyprmux::state::{Pane, PaneBorderMode, SplitAxis};
use hyprmux::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{Color, FloatRect, Rect};

fn backend(mode: PaneBorderMode) -> TestBackend<HyprmuxApp> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 30,
        h: 10,
    });
    let state = backend.state_mut();
    state.config.animations.enabled = false;
    state.config.pane.show_workbar = false;
    state.config.pane.show_titles = false;
    state.config.pane.border_mode = mode;
    let workspace = &mut state.current_mut().workspaces[0];
    workspace.start_axis = SplitAxis::Horizontal;
    workspace.panes.clear();
    let ids = [10, 11];
    for id in ids {
        let mut pane = Pane::new(
            id,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 30.0,
                h: 10.0,
            },
        );
        pane.opening = false;
        pane.terminal_active = true;
        workspace.panes.push(pane);
    }
    workspace.tile_tree = build_dwindle_tree(&ids, workspace.start_axis, &[]);
    workspace.focused_pane = Some(10);
    state.current_mut().focused_pane = Some(10);
    backend
}

fn block_second(backend: &mut TestBackend<HyprmuxApp>) {
    backend.state_mut().current_mut().workspaces[0].panes[1]
        .terminal
        .reported_status = Some(hyprmux::session::protocol::PaneStatus {
        value: "blocked".into(),
        reason: None,
        set_at: 0,
    });
}

fn detect_second_as_blocked(backend: &mut TestBackend<HyprmuxApp>) {
    backend.state_mut().current_mut().workspaces[0].panes[1]
        .terminal
        .detected_agent = Some(hyprmux::session::protocol::DetectedAgent {
        kind: hyprmux::session::protocol::AgentKind::Codex,
        state: hyprmux::session::protocol::DetectedAgentState::Blocked,
    });
}

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn alert-border smoke test")
        .join()
        .expect("alert-border smoke test completes");
}

#[test]
fn alerts_color_frames_and_dividers_but_never_none_mode() {
    on_large_stack(|| {
        hyprmux::test_support::isolate_user_dirs();
        for mode in [
            PaneBorderMode::Separate,
            PaneBorderMode::Merged,
            PaneBorderMode::Dividers,
            PaneBorderMode::None,
        ] {
            let mut backend = backend(mode);
            backend.state_mut().theme.status.error = Color::rgb(255, 0, 1);
            // The focused seam intentionally wins; disable that independent accent to inspect the
            // alert layer itself on this two-pane split.
            backend.state_mut().config.pane.highlight_focused_border = false;
            if mode == PaneBorderMode::Merged {
                detect_second_as_blocked(&mut backend);
            } else {
                block_second(&mut backend);
            }
            backend.render();
            let frame = backend.capture_frame();
            let has_alert_color = frame
                .cells
                .iter()
                .any(|cell| cell.fg == Color::rgb(255, 0, 1));
            assert_eq!(
                has_alert_color,
                mode != PaneBorderMode::None,
                "{mode:?} should {}draw alert chrome",
                if mode == PaneBorderMode::None {
                    "not "
                } else {
                    ""
                }
            );
        }

        let mut backend = backend(PaneBorderMode::Separate);
        backend.state_mut().theme.status.error = Color::rgb(255, 0, 1);
        backend.state_mut().config.pane.alert_border = hyprmux::state::AlertMode::Off;
        block_second(&mut backend);
        backend.render();
        assert!(
            !backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == Color::rgb(255, 0, 1)),
            "alert_border = off must suppress frame colors"
        );
    });
}

#[test]
fn focusing_finished_alert_clears_it_and_focus_keeps_the_active_border() {
    on_large_stack(|| {
        hyprmux::test_support::isolate_user_dirs();
        let mut backend = backend(PaneBorderMode::Separate);
        {
            let state = backend.state_mut();
            state.current_mut().workspaces[0].panes[1]
                .terminal
                .finished_unseen = true;
        }
        backend.render();
        let success = backend.state().theme.status.success;
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == success)
        );

        // The update chokepoint acknowledges a finished pane as soon as it is focused.
        backend
            .dispatch(hyprmux::Msg::FocusPane(11))
            .expect("focus update succeeds");
        backend.render();
        assert!(
            !backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == success),
            "finished alert must remain cleared after focus"
        );

        block_second(&mut backend);
        backend.render();
        let active = backend.state().theme.border_active;
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == active)
        );
    });
}
