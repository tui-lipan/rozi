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

/// A two-pane split with the right pane blocked, breathing its alert with motion on, its pulse
/// chain armed at the backend's start.
fn breathing_backend(paint: rozi::state::PaneAlertPaint) -> TestBackend<AppRoot> {
    let mut backend = backend(PaneBorderMode::Separate);
    {
        let state = backend.state_mut();
        state.config.animations.enabled = true;
        state.config.animations.pane_style = rozi::layout::anim::PaneAnimationStyle::Off;
        state.theme.status.error = Color::rgb(255, 0, 1);
        state.config.pane.highlight_focused_border = false;
        state.config.pane.alert_paint = paint;
        // What arming the chain does. Arming also sends the first tick through the app's command
        // link, which a test backend never gets, so `first_turn` delivers that one; the chain then
        // schedules every later tick itself.
        state.alert_pulse_armed = true;
        state.alert_pulse_armed_at = std::time::Duration::ZERO;
        state.alert_pulse_half =
            rozi::layout::anim::alert_pulse_half_period(state.config.animations);
    }
    block_second(&mut backend);
    backend
}

fn half_period(backend: &TestBackend<AppRoot>) -> std::time::Duration {
    backend.state().alert_pulse_half
}

/// Run to the chain's first deadline and deliver the tick arming sent for it; see
/// `breathing_backend`.
fn first_turn(backend: &mut TestBackend<AppRoot>) {
    backend.advance(half_period(backend));
    backend.dispatch(rozi::Msg::AlertPulseTick).unwrap();
    assert_eq!(backend.state().alert_pulse_turns, 1);
}

/// The blocked pane's top border colour and interior background, this frame, after delivering any
/// tick of the chain that has come due.
fn blocked_surfaces(backend: &mut TestBackend<AppRoot>) -> (Color, Color) {
    backend.pump().expect("deliver due ticks");
    backend.render();
    let frame = backend.capture_frame();
    (frame.cells[22].fg, frame.cells[5 * 30 + 22].bg)
}

/// How far `color` has travelled from `from` toward `to`, from 0 to 1.
fn travelled(color: Color, from: Color, to: Color) -> f32 {
    let channels = |color: Color| {
        let Color::Rgb(r, g, b) = color else {
            panic!("expected truecolor, got {color:?}");
        };
        [f32::from(r), f32::from(g), f32::from(b)]
    };
    let (color, from, to) = (channels(color), channels(from), channels(to));
    let distance =
        |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt();
    distance(color, from) / distance(to, from)
}

/// Both surfaces' ends and the calm pane's background: the border's peak and trough, the tint's
/// peak, read from a twin that covers the content and has breathed to the bottom of its first fade.
struct BreatheEnds {
    border: (Color, Color),
    tint: (Color, Color),
}

fn breathe_ends() -> BreatheEnds {
    let mut twin = breathing_backend(rozi::state::PaneAlertPaint::Both);
    let half = half_period(&twin);
    let calm_bg = twin.capture_frame().cells[5 * 30 + 7].bg;
    let (border_peak, tint_peak) = blocked_surfaces(&mut twin);
    first_turn(&mut twin);
    blocked_surfaces(&mut twin);
    twin.advance(half);
    let (border_trough, tint_trough) = blocked_surfaces(&mut twin);
    assert_ne!(border_trough, border_peak);
    assert_ne!(tint_peak, calm_bg, "tinted at the peak");
    assert_eq!(tint_trough, calm_bg, "no tint at the trough");
    BreatheEnds {
        border: (border_peak, border_trough),
        tint: (tint_peak, calm_bg),
    }
}

/// Step through a breathe and check the content tint keeps pace with the border: at every sample,
/// both are the same fraction of the way from their peak to their trough.
fn assert_surfaces_breathe_together(
    backend: &mut TestBackend<AppRoot>,
    ends: &BreatheEnds,
    samples: &[(std::time::Duration, &str)],
) {
    for &(step, when) in samples {
        backend.advance(step);
        let (frame_fg, content_bg) = blocked_surfaces(backend);
        let border_at = travelled(frame_fg, ends.border.0, ends.border.1);
        let tint_at = travelled(content_bg, ends.tint.0, ends.tint.1);
        assert!(
            (border_at - tint_at).abs() < 0.05,
            "{when}: the border is {border_at:.2} of the way down and the tint {tint_at:.2}"
        );
    }
}

/// One fade down, sampled at its quarters, starting from a turn that was just delivered.
fn a_fade_down(quarter: std::time::Duration) -> [(std::time::Duration, &'static str); 4] {
    [
        (quarter, "a quarter of the way down"),
        (quarter, "halfway down"),
        (quarter, "three quarters down"),
        (quarter, "at the trough"),
    ]
}

/// The content tint is a registry pulse anchored where the chain's beat is. Driven by the chain's
/// own ticks, it holds at its peak until the first turn, then fades to nothing and back in step with
/// the border fading to its trough and back - and it is still in step forty beats later.
#[test]
fn a_pulsing_content_tint_breathes_in_step_with_the_border() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let ends = breathe_ends();
        let mut backend = breathing_backend(PaneAlertPaint::Both);
        let quarter = half_period(&backend) / 4;

        assert_eq!(blocked_surfaces(&mut backend), (ends.border.0, ends.tint.0));
        first_turn(&mut backend);
        assert_eq!(
            blocked_surfaces(&mut backend),
            (ends.border.0, ends.tint.0),
            "both held at the peak until the first turn"
        );
        assert_surfaces_breathe_together(&mut backend, &ends, &a_fade_down(quarter));
        // The chain's own second tick came due as the trough arrived, and turns both back up.
        assert_eq!(backend.state().alert_pulse_turns, 2);
        assert_surfaces_breathe_together(
            &mut backend,
            &ends,
            &[
                (quarter, "a quarter of the way back up"),
                (quarter * 2, "three quarters back up"),
                (quarter, "back at the peak"),
            ],
        );

        for _ in 0..40 {
            backend.advance(quarter * 4);
            blocked_surfaces(&mut backend);
        }
        assert_eq!(backend.state().alert_pulse_turns, 43);
        assert_surfaces_breathe_together(&mut backend, &ends, &a_fade_down(quarter));
    });
}

/// Ticks handed over late - whenever a busy event loop next gets round to them - leave the tint
/// and the border in step. The loop here looks every 17 ms, which never divides the beat, so each
/// tick is picked up a different 0-16 ms after it fell due. A chain that timed each tick from when
/// the last one was handled would add that up, 16 ms a beat here; one that lays its deadlines from
/// its anchor cannot.
///
/// The tint holds each 10 fps sample until the next, so the surfaces are compared on the tint's
/// sample instants, where it is exact: there a border in step trails it only by how late its tick
/// was picked up. Up to 16 ms of that at the steepest point of the fade, plus one of the ~20 color
/// steps the faint tint spans, bounds the gap under 0.1; drift of a few beats' lateness is far past
/// it, and is caught first by the tick itself arriving more than one look late.
#[test]
fn late_ticks_leave_the_tint_and_border_in_step() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let ends = breathe_ends();
        let mut backend = breathing_backend(PaneAlertPaint::Both);
        let half = half_period(&backend);
        let look = std::time::Duration::from_millis(17);
        let sample = std::time::Duration::from_millis(
            1_000 / u64::from(rozi::layout::anim::ALERT_PULSE_FRAME_RATE),
        );
        // The clock, kept here: the chain is anchored at zero, so turn `n` falls due at `n * half`.
        let mut now = half;
        first_turn(&mut backend);
        blocked_surfaces(&mut backend);
        while backend.state().alert_pulse_turns < 21 {
            let turns = backend.state().alert_pulse_turns;
            backend.advance(look);
            now += look;
            backend.pump().expect("deliver due ticks");
            if backend.state().alert_pulse_turns != turns {
                backend.render();
            }
        }
        // Turn 21 was just picked up, a little late: its fade down is on its way.
        let turn = half * 21;
        assert!(
            now > turn && now - turn < look,
            "picked up {:?} late",
            now - turn
        );
        for samples in [2u32, 4, 6] {
            let at = turn + sample * samples;
            backend.advance(at - now);
            now = at;
            let (frame_fg, content_bg) = blocked_surfaces(&mut backend);
            let border_at = travelled(frame_fg, ends.border.0, ends.border.1);
            let tint_at = travelled(content_bg, ends.tint.0, ends.tint.1);
            assert!(
                (border_at - tint_at).abs() < 0.1,
                "{samples} samples into the fade of turn 21, twenty beats of late ticks on: the \
                 border is {border_at:.2} of the way down and the tint {tint_at:.2}"
            );
        }
    });
}

/// A stall of more than two half periods lands the border on the phase the clock has reached, in
/// step with the tint, instead of working through the turns it missed.
#[test]
fn a_long_stall_catches_the_border_up_with_the_tint() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let ends = breathe_ends();
        let mut backend = breathing_backend(PaneAlertPaint::Both);
        let half = half_period(&backend);
        first_turn(&mut backend);
        blocked_surfaces(&mut backend);
        backend.advance(half);
        blocked_surfaces(&mut backend);
        assert_eq!(backend.state().alert_pulse_turns, 2);

        // Nothing is handled for 2.5 half periods; the one tick waiting is picked up after.
        backend.advance(half * 2 + half / 2);
        blocked_surfaces(&mut backend);
        assert_eq!(
            backend.state().alert_pulse_turns,
            4,
            "the chain lands on the turn the clock is on"
        );
        // Four turns in is a fade down, as the tint is; the border caught up at the next turn.
        backend.advance(half / 2);
        blocked_surfaces(&mut backend);
        assert_eq!(backend.state().alert_pulse_turns, 5);
        assert_surfaces_breathe_together(&mut backend, &ends, &a_fade_down(half / 4));
    });
}

/// Covering the content in the middle of a breathe joins the border's beat, rather than starting a
/// tint pulse of its own from the moment the setting changed.
#[test]
fn switching_the_paint_to_cover_the_content_mid_breathe_joins_the_borders_beat() {
    use rozi::state::PaneAlertPaint;
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let ends = breathe_ends();
        let mut backend = breathing_backend(PaneAlertPaint::Border);
        let quarter = half_period(&backend) / 4;

        assert_eq!(
            blocked_surfaces(&mut backend),
            (ends.border.0, ends.tint.1),
            "border only, for now"
        );
        first_turn(&mut backend);
        blocked_surfaces(&mut backend);
        backend.advance(quarter * 2);
        assert_eq!(
            blocked_surfaces(&mut backend).1,
            ends.tint.1,
            "the content is still untouched halfway down"
        );

        backend.state_mut().config.pane.alert_paint = PaneAlertPaint::Both;
        assert_surfaces_breathe_together(
            &mut backend,
            &ends,
            &[
                (std::time::Duration::ZERO, "the frame the paint changed on"),
                (quarter, "three quarters down"),
                (quarter, "at the trough"),
                (quarter * 2, "halfway back up"),
            ],
        );
    });
}

/// A solid blue image `cols` by `rows` cells, sent the way `kitty icat` sends one. Blue, so that
/// the red alert tint would visibly move it.
fn blue_image(cols: u32, rows: u32) -> Vec<u8> {
    use base64::Engine as _;
    let cell = tui_lipan::host_cell_size();
    let (width, height) = (cols * u32::from(cell.width), rows * u32::from(cell.height));
    let pixels = [0u8, 0, 255].repeat((width * height) as usize);
    format!(
        "\x1b_Ga=T,f=24,s={width},v={height},t=d,i=1;{}\x1b\\",
        base64::engine::general_purpose::STANDARD.encode(pixels)
    )
    .into_bytes()
}

/// The alert tint marks the text a pane shows and never the pictures in it: an inline image keeps
/// its own pixels under a static tint, through the hold before a pulse's first turn, and while it
/// breathes.
#[test]
fn the_content_tint_leaves_inline_images_their_own_pixels() {
    use rozi::state::{AlertMode, PaneAlertPaint};
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let untouched = |backend: &mut TestBackend<AppRoot>, when: &str| {
            let (_, content_bg) = blocked_surfaces(backend);
            let frame = backend.capture_frame();
            let calm_bg = frame.cells[5 * 30 + 7].bg;
            assert_ne!(content_bg, calm_bg, "{when}: the cells are tinted");
            assert_eq!(frame.images.len(), 1, "{when}: the image is on screen");
            assert!(
                frame.images[0]
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|&pixel| pixel == [0, 0, 255, 255]),
                "{when}: every image pixel keeps its own color"
            );
        };
        let with_image = |mode: AlertMode| {
            let mut backend = breathing_backend(PaneAlertPaint::Content);
            backend.state_mut().config.pane.alert_border = mode;
            backend.state_mut().current_mut().workspaces[0].panes[1]
                .terminal
                .process_server_output(&blue_image(2, 1));
            backend
        };

        let mut backend = with_image(AlertMode::Static);
        untouched(&mut backend, "static");

        let mut backend = with_image(AlertMode::Pulse);
        untouched(&mut backend, "pulse, before the first turn");
        first_turn(&mut backend);
        blocked_surfaces(&mut backend);
        backend.advance(half_period(&backend) / 4);
        untouched(&mut backend, "pulse, a quarter of the way down");
    });
}
