//! Streaming pane output is a terminal repaint, not a rebuild.
//!
//! A pane's screen is handed to the widget as a `TerminalScreenHandle`, so nothing in the element
//! tree depends on what the child program just drew. That is what lets `Msg::SessionOutput` ask for
//! `Update::terminal_paint()`: one agent streaming into one pane must not re-run `view()` and layout
//! for every other pane, workbar segment and sidebar row in the window on every chunk.
//!
//! `terminal_paint` claims more than `paint` - that live terminal and label content is all that
//! looks different - and the framework spends that claim on repainting only the rows the emulator
//! reports as damaged. So the level asserted here is a contract, not a detail: widening it back to
//! `paint` silently restores the full-window frame per changed character, and narrowing it in a
//! frame where chrome did move would leave that chrome stale.

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{Pane, PaneBorderMode, PaneId, PaneTitlebarMode};
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
    rozi::test_support::isolate_user_dirs();
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
    output_to(bytes, false)
}

fn output_to(bytes: &str, local: bool) -> rozi::Msg {
    rozi::Msg::SessionOutput {
        epoch: 0,
        pane_id: PANE,
        local,
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
            UpdateLevel::TerminalPaint,
            "screen-only output must not re-run view() and layout, and must say so narrowly \
             enough for the framework to repaint only the damaged rows"
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
fn osc_title_changes_repaint_all_live_title_modes() {
    on_large_stack(|| {
        for mode in [
            PaneTitlebarMode::Bar,
            PaneTitlebarMode::Inset,
            PaneTitlebarMode::Integrated,
        ] {
            for scratch in [false, true] {
                let mut backend = backend();
                {
                    let state = backend.state_mut();
                    state.config.animations.enabled = false;
                    state.config.pane.titlebar = mode;
                    // Integrated must own an interior title row, rather than a merged seam.
                    state.config.pane.border_mode = PaneBorderMode::Separate;
                    if scratch {
                        let pane = state.current_mut().workspaces[0].panes.remove(0);
                        assert_eq!(pane.id, PANE);
                        state.current_mut().workspaces[0].tile_tree = None;
                        state.current_mut().workspaces[0].focused_pane = Some(OTHER_PANE);
                        state.current_mut().focused_pane = Some(OTHER_PANE);
                        state.scratch.panes.clear();
                        state.scratch.panes.push(pane);
                        state.scratch.tile_tree = None;
                        state.scratch.focused_pane = Some(PANE);
                        state.scratch_visible = true;
                    }
                }
                backend
                    .update_level(output_to("ready\r\n\x1b]0;initial-title\x07", scratch))
                    .expect("settle");
                backend.render();
                let prefix = if scratch { "S · " } else { "" };
                let mut previous = "initial-title";
                assert!(
                    backend
                        .capture_frame()
                        .plain_text()
                        .contains(&format!("{prefix}{previous}")),
                    "{mode:?}/scratch={scratch}: initial title and marker must be rendered"
                );
                for title in ["renamed-session", "short"] {
                    assert_eq!(
                        backend
                            .update_level(output_to(&format!("\x1b]0;{title}\x07"), scratch))
                            .expect("title chunk"),
                        UpdateLevel::TerminalPaint,
                        "{mode:?}/scratch={scratch}: live titles need no composition"
                    );
                    assert!(
                        backend.refresh_live_terminals(),
                        "{mode:?}/scratch={scratch}: live title moved"
                    );
                    // No render()/view rebuild between the OSC update and the captured frame.
                    let frame = backend.capture_frame().plain_text();
                    assert!(
                        frame.contains(&format!("{prefix}{title}")),
                        "{mode:?}/scratch={scratch}: updated title and marker missing: {frame}"
                    );
                    assert!(
                        !frame.contains(previous),
                        "{mode:?}/scratch={scratch}: stale title remains: {frame}"
                    );
                    previous = title;
                }
            }
        }
    });
}

#[test]
fn osc_title_changes_still_rebuild_border_titles() {
    on_large_stack(|| {
        let mut backend = backend();
        backend.state_mut().config.pane.titlebar = PaneTitlebarMode::Border;
        backend.update_level(output("ready\r\n")).expect("settle");
        backend.render();
        assert_eq!(
            backend
                .update_level(output("\x1b]0;border renamed\x07"))
                .expect("border title"),
            UpdateLevel::Full,
            "titles embedded in a frame border still need composition"
        );
        backend.render();
        assert!(
            backend
                .capture_frame()
                .plain_text()
                .contains("border renamed")
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
