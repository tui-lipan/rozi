//! A pane running a full-screen TUI enables mouse tracking, which normally forwards mouse events
//! straight to that terminal before the pane's own `MouseRegion`. The left press that first focuses
//! it is the exception: the framework consumes that gesture and bubbles the press so the pane can
//! reconcile logical focus without also activating whatever the child drew under the pointer.

use rozi::AppRoot;
use rozi::state::{Pane, PaneId};
use rozi::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::prelude::{FloatRect, ManagedTerminalStatus, MouseEvent, Rect};

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 100,
    h: 30,
};

/// Well inside the right-hand pane's body.
const CLICK_X: u16 = 75;
const CLICK_Y: u16 = 15;

fn mouse(x: u16, y: u16, kind: MouseKind) -> MouseEvent {
    MouseEvent {
        x,
        y,
        kind,
        mods: Default::default(),
    }
}

/// Two side-by-side panes, focus on the left, hover-focus off, and the right pane running a
/// full-screen TUI (any-event tracking + SGR encoding, as vim or lazygit enable).
fn backend_with_tracking_pane() -> TestBackend<AppRoot> {
    backend_with_tracking_pane_hover(false)
}

fn backend_with_tracking_pane_hover(focus_on_hover: bool) -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        state.config.pane.focus_on_hover = focus_on_hover;
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: f32::from(VIEWPORT.w),
            h: f32::from(VIEWPORT.h),
        };
        // Ids start past the pane `State::new` seeds, whose stale transition entries would
        // otherwise keep a reused id off its target rect.
        for id in [10 as PaneId, 11] {
            let mut pane = Pane::new(id, 1_000, rect);
            pane.opening = false;
            pane.terminal_active = true;
            if id == 11 {
                // DECSET 1003 (any-event tracking) + 1006 (SGR encoding).
                pane.terminal
                    .process_server_output(b"\x1b[?1003h\x1b[?1006h");
            }
            state.current_mut().workspaces[0].panes.push(pane);
        }
        let start_axis = state.current().workspaces[0].start_axis;
        let ratios = state.current().workspaces[0].split_ratios.clone();
        state.current_mut().workspaces[0].tile_tree =
            build_dwindle_tree(&[10, 11], start_axis, &ratios);
        state.current_mut().next_pane_id = 20;
        state.current_mut().focused_pane = Some(10);
        state.current_mut().workspaces[0].focused_pane = Some(10);
    }
    backend.render();
    backend
}

fn on_deep_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn clicking_a_mouse_tracking_pane_focuses_it_with_hover_focus_disabled() {
    on_deep_stack(|| {
        let mut backend = backend_with_tracking_pane();
        assert_eq!(
            backend.state().current().focused_pane,
            Some(10),
            "left pane starts focused"
        );
        // Guard against the setup silently not tracking, which would make this test vacuous: the
        // framework only bypasses the pane's MouseRegion for a terminal that requested tracking.
        assert_ne!(
            backend.state().current().workspaces[0].panes[1]
                .terminal
                .snapshot()
                .mouse_mode
                .mode,
            tui_lipan::prelude::MouseMode::None,
            "right pane must actually be tracking the mouse, or this tests nothing"
        );

        backend
            .send_mouse(mouse(CLICK_X, CLICK_Y, MouseKind::Down(MouseButton::Left)))
            .expect("mouse down");
        backend
            .send_mouse(mouse(CLICK_X, CLICK_Y, MouseKind::Up(MouseButton::Left)))
            .expect("mouse up");

        assert_eq!(
            backend.state().current().focused_pane,
            Some(11),
            "clicking a full-screen TUI pane must focus it even with focus_on_hover disabled"
        );
        assert!(
            !matches!(
                backend.state().current().workspaces[0].panes[1]
                    .terminal
                    .status,
                ManagedTerminalStatus::Error(_)
            ),
            "the gesture that only focused the pane must not be forwarded to its disconnected child"
        );
    });
}

/// The counterpart: motion alone must not steal focus, or the fix above would quietly reintroduce
/// hover-to-focus for full-screen TUIs against the user's configuration.
#[test]
fn hovering_a_mouse_tracking_pane_does_not_focus_it_with_hover_focus_disabled() {
    on_deep_stack(|| {
        let mut backend = backend_with_tracking_pane();
        backend
            .send_mouse(mouse(CLICK_X, CLICK_Y, MouseKind::Moved))
            .expect("mouse move");
        assert_eq!(
            backend.state().current().focused_pane,
            Some(10),
            "motion alone must not move focus while focus_on_hover is disabled"
        );
    });
}

/// With hover-focus on, motion over a tracking pane must still focus it — the click fix must not
/// come at the cost of the behavior that path was originally written for.
#[test]
fn hovering_a_mouse_tracking_pane_focuses_it_with_hover_focus_enabled() {
    on_deep_stack(|| {
        let mut backend = backend_with_tracking_pane_hover(true);
        backend
            .send_mouse(mouse(CLICK_X, CLICK_Y, MouseKind::Moved))
            .expect("mouse move");
        assert_eq!(
            backend.state().current().focused_pane,
            Some(11),
            "hover-focus must still reach a full-screen TUI pane"
        );
    });
}
