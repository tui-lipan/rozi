//! Streaming pane output is a repaint, not a rebuild.
//!
//! A pane's screen is handed to the widget as a `TerminalScreenHandle`, so nothing in the element
//! tree depends on what the child program just drew. That is what lets `Msg::SessionOutput` ask for
//! `Update::paint()`: one agent streaming into one pane must not re-run `view()` and layout for every
//! other pane, workbar segment and sidebar row in the window on every chunk.

use rozi::AppRoot;
use rozi::state::{Pane, PaneId};
use rozi::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect, UpdateLevel};

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 80,
    h: 24,
};

const PANE: PaneId = 10;
const OTHER_PANE: PaneId = 11;

fn backend() -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: f32::from(VIEWPORT.w),
            h: f32::from(VIEWPORT.h),
        };
        for id in [PANE, OTHER_PANE] {
            let mut pane = Pane::new(id, 1_000, rect);
            pane.opening = false;
            pane.terminal_active = true;
            state.current_mut().workspaces[0].panes.push(pane);
        }
        let start_axis = state.current().workspaces[0].start_axis;
        let ratios = state.current().workspaces[0].split_ratios.clone();
        state.current_mut().workspaces[0].tile_tree =
            build_dwindle_tree(&[PANE, OTHER_PANE], start_axis, &ratios);
        state.current_mut().focused_pane = Some(PANE);
        state.current_mut().workspaces[0].focused_pane = Some(PANE);
    }
    backend.render();
    backend
}

fn output(bytes: &str) -> rozi::Msg {
    rozi::Msg::SessionOutput {
        epoch: 0,
        pane_id: PANE,
        generation: 0,
        bytes: bytes.as_bytes().to_vec(),
    }
}

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test completes");
}

#[test]
fn output_to_a_visible_pane_asks_for_a_repaint() {
    on_large_stack(|| {
        let mut backend = backend();
        // The first chunk carries the pane out of `Starting`, which the titlebar renders - so that
        // one is a rebuild. Every chunk after it is pure screen movement.
        let _ = backend
            .update_level(output("ready\r\n"))
            .expect("first chunk");
        backend.render();

        assert_eq!(
            backend
                .update_level(output("streaming\r\n"))
                .expect("second chunk"),
            UpdateLevel::Paint,
            "screen-only output must not re-run view() and layout"
        );

        // And the new content still reaches the screen on a paint-only frame.
        assert!(backend.refresh_live_terminals(), "the screen moved");
        let lines = backend.capture_frame().to_fixed_grid_lines();
        assert!(
            lines.iter().any(|line| line.contains("streaming")),
            "paint-only output is still painted: {lines:#?}"
        );
    });
}

#[test]
fn an_osc_title_change_still_asks_for_a_full_frame() {
    on_large_stack(|| {
        let mut backend = backend();
        let _ = backend.update_level(output("ready\r\n")).expect("settle");
        backend.render();

        // The titlebar renders the pane title, which lives outside the screen the widget reads.
        assert_eq!(
            backend
                .update_level(output("\x1b]0;renamed\x07"))
                .expect("title chunk"),
            UpdateLevel::Full,
            "chrome outside the screen has to be rebuilt"
        );
    });
}

#[test]
fn output_to_an_unrendered_pane_asks_for_no_frame_at_all() {
    on_large_stack(|| {
        let mut backend = backend();
        let _ = backend.update_level(output("ready\r\n")).expect("settle");
        backend.render();
        backend.state_mut().current_mut().active_workspace = 1;

        assert_eq!(
            backend
                .update_level(output("offscreen\r\n"))
                .expect("offscreen chunk"),
            UpdateLevel::None,
            "a pane nobody is looking at costs nothing to update"
        );
    });
}
