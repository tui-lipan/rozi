//! Render-level regression coverage for pane alert colors across border modes.

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{Pane, PaneBorderMode, SplitAxis};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{Color, FloatRect, Rect};

fn backend(mode: PaneBorderMode) -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
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

fn block_second(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().workspaces[0].panes[1]
        .terminal
        .reported_status = Some(rozi::session::protocol::PaneStatus {
        value: "blocked".into(),
        reason: None,
        set_at: 0,
    });
}

fn detect_second_as_blocked(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().workspaces[0].panes[1]
        .terminal
        .detected_agent = Some(rozi::session::protocol::DetectedAgent {
        agent: rozi::session::protocol::AgentIdentity::new("codex", "Codex").into(),
        state: rozi::session::protocol::DetectedAgentState::Blocked,
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
        rozi::test_support::isolate_user_dirs();
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
        backend.state_mut().config.pane.alert_border = rozi::state::AlertMode::Off;
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
        rozi::test_support::isolate_user_dirs();
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
            .dispatch(rozi::Msg::FocusPane(11))
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

/// The pane the user is already sitting in still marks a finished run: the run ended while they
/// were elsewhere, and nothing on screen says so. Its live states stay suppressed there, because a
/// blocked or working agent is already legible in the pane's own content.
#[test]
fn the_focused_pane_marks_a_finished_run_but_not_its_live_states() {
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let finished_color = Color::rgb(0, 255, 1);
        let blocked_color = Color::rgb(255, 0, 1);

        let mut finished = backend(PaneBorderMode::Separate);
        {
            let state = finished.state_mut();
            state.theme.status.success = finished_color;
            state.theme.status.error = blocked_color;
            state.current_mut().workspaces[0].panes[0]
                .terminal
                .finished_unseen = true;
        }
        finished.render();
        assert!(
            finished
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == finished_color),
            "a finished run must mark the focused pane's own border"
        );

        let mut blocked = backend(PaneBorderMode::Separate);
        {
            let state = blocked.state_mut();
            state.theme.status.success = finished_color;
            state.theme.status.error = blocked_color;
            state.current_mut().workspaces[0].panes[0]
                .terminal
                .reported_status = Some(rozi::session::protocol::PaneStatus {
                value: "blocked".into(),
                reason: None,
                set_at: 0,
            });
        }
        blocked.render();
        assert!(
            !blocked
                .capture_frame()
                .cells
                .iter()
                .any(|cell| cell.fg == blocked_color),
            "a blocked focused pane says so in its own content; its border must not"
        );
    });
}

/// `alert_paint` splits the alert between two surfaces: the frame, and a faint wash over the
/// terminal. Animations are off here, so a tint holds at its peak and the frame is deterministic.
#[test]
fn alert_paint_chooses_between_the_frame_and_a_faint_content_tint() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let alert = Color::rgb(255, 0, 1);
        let render = |mode: PaneBorderMode, paint: PaneAlertPaint| {
            let mut backend = backend(mode);
            backend.state_mut().theme.status.error = alert;
            backend.state_mut().config.pane.highlight_focused_border = false;
            backend.state_mut().config.pane.alert_paint = paint;
            block_second(&mut backend);
            backend.render();
            backend.capture_frame()
        };
        // Interior cells of the calm left pane and the blocked right one.
        let interiors = |frame: &tui_lipan::CapturedFrame| {
            let at = |x: u16, y: u16| frame.cells[usize::from(y) * 30 + usize::from(x)].clone();
            (at(7, 5), at(22, 5))
        };

        let frame = render(PaneBorderMode::Separate, PaneAlertPaint::Border);
        let (calm, blocked) = interiors(&frame);
        assert!(frame.cells.iter().any(|cell| cell.fg == alert));
        assert_eq!(
            calm.bg, blocked.bg,
            "a border-only alert leaves content alone"
        );

        for mode in [PaneBorderMode::Separate, PaneBorderMode::None] {
            let frame = render(mode, PaneAlertPaint::Content);
            let (calm, blocked) = interiors(&frame);
            assert!(
                !frame.cells.iter().any(|cell| cell.fg == alert),
                "{mode:?}: a content-only alert leaves the frame alone"
            );
            let (Color::Rgb(cr, cg, cb), Color::Rgb(br, bg, bb)) = (calm.bg, blocked.bg) else {
                panic!("{mode:?}: expected truecolor pane backgrounds: {calm:?} {blocked:?}");
            };
            assert!(br > cr, "{mode:?}: the blocked pane warms toward red");
            assert!(bg <= cg && bb <= cb);
            // Faint, not a lighthouse: well short of the alert colour itself.
            assert!(
                u16::from(br - cr) < 40,
                "{mode:?}: tint too strong: {calm:?} -> {blocked:?}"
            );
        }

        let frame = render(PaneBorderMode::Separate, PaneAlertPaint::Both);
        let (calm, blocked) = interiors(&frame);
        // The wash reaches the frame on every side, not just the grid the shell has drawn on.
        let at = |x: u16, y: u16| frame.cells[usize::from(y) * 30 + usize::from(x)].clone();
        for (x, y) in [(19, 1), (28, 1), (19, 8), (28, 8)] {
            assert_eq!(
                at(x, y).bg,
                blocked.bg,
                "untinted interior cell at ({x}, {y})"
            );
        }
        assert!(frame.cells.iter().any(|cell| cell.fg == alert));
        assert_ne!(calm.bg, blocked.bg);
    });
}

/// A breathing content tint is a registry pulse kept on the border's beat: it holds at its peak
/// until the chain's first turn, then fades to nothing over one half period - as the border fades to
/// its trough - and back over the next.
#[test]
fn a_pulsing_content_tint_breathes_on_the_borders_beat() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let mut backend = backend(PaneBorderMode::Separate);
        {
            let state = backend.state_mut();
            state.config.animations.enabled = true;
            state.config.animations.pane_style = rozi::layout::anim::PaneAnimationStyle::Off;
            state.theme.status.error = Color::rgb(255, 0, 1);
            state.config.pane.highlight_focused_border = false;
            state.config.pane.alert_paint = PaneAlertPaint::Content;
            state.alert_pulse_armed = true;
        }
        block_second(&mut backend);
        let half = rozi::layout::anim::alert_pulse_half_period(backend.state().config.animations);
        let blocked_bg = |backend: &mut TestBackend<AppRoot>| {
            backend.render();
            let frame = backend.capture_frame();
            let calm = frame.cells[5 * 30 + 7].bg;
            let blocked = frame.cells[5 * 30 + 22].bg;
            (calm, blocked)
        };

        // Armed but not yet turned: held at the peak, however long that takes.
        let (calm, peak) = blocked_bg(&mut backend);
        assert_ne!(calm, peak, "tinted at its peak before the first turn");
        backend.advance(half);
        assert_eq!(blocked_bg(&mut backend).1, peak);

        // The first turn starts the pulse from the peak.
        backend.state_mut().alert_pulse_turns = 1;
        backend.state_mut().alert_pulse_phase = true;
        assert_eq!(blocked_bg(&mut backend).1, peak);
        backend.advance(half / 2);
        let (_, midway) = blocked_bg(&mut backend);
        assert!(
            midway != peak && midway != calm,
            "halfway down: {calm:?} < {midway:?} < {peak:?}"
        );
        backend.advance(half / 2);
        let (calm, trough) = blocked_bg(&mut backend);
        assert_eq!(
            trough, calm,
            "no tint at the trough, as the border bottoms out"
        );
        backend.advance(half);
        assert_eq!(
            blocked_bg(&mut backend).1,
            peak,
            "back at the peak a period later"
        );
    });
}
