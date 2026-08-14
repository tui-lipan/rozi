//! Pins the scratchpad's deploy animation.
//!
//! The dropdown *grows* out of the bottom edge rather than sliding up as a rigid block, so its
//! bottom border holds its row for the whole animation and the frame is whole in every frame.
//! Translating it instead carried that border off the bottom of the screen until the very last
//! frame, so the dropdown arrived as a top edge with nothing under it and then snapped its floor
//! into place.
//!
//! Growing means the panes inside are resized as it opens. That is affordable here in a way it is
//! not horizontally: only the row count changes, and terminal reflow is a function of columns.

use std::time::Duration;

use rozi::AppRoot;
use rozi::anim::GeometryAnimation;
use rozi::state::{Pane, PaneBorderMode};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

const WIDTH: u16 = 60;
const HEIGHT: u16 = 20;
/// Sample points spread across the deploy, none close enough to an end to be settled.
const IN_FLIGHT: [u64; 3] = [400, 500, 500];

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: WIDTH,
        h: HEIGHT,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = true;
        // Deliberately slow: the assertions below sample the deploy part-way through, and a short
        // animation would be raced by the wall-clock cost of the fixture itself.
        state.config.animations.geometry_duration = Duration::from_millis(3_000);
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = PaneBorderMode::Separate;
        // What toggling the scratchpad actually arms. Without it every pane transition is instant
        // whatever the layer asks for, and the snapping these tests are about is unobservable.
        state.animation = GeometryAnimation::TileFloat;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: f32::from(WIDTH),
            h: f32::from(HEIGHT),
        };
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 5_000, rect);
        pane.opening = false;
        pane.terminal_active = true;
        workspace.panes.push(pane);
        workspace.focused_pane = Some(1);
        state.current_mut().focused_pane = Some(1);
        let mut scratch = Pane::new(2, 5_000, rect);
        scratch.opening = false;
        scratch.terminal_active = true;
        state.scratch.panes.push(scratch);
        state.scratch.focused_pane = Some(2);
    }
    // Seeds the slide at retracted, so the toggle below is a real deploy rather than the settled
    // startup state.
    backend.render();
    backend
}

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn scratchpad-slide smoke test")
        .join()
        .expect("scratchpad-slide smoke test completes");
}

fn set_visible(backend: &mut TestBackend<AppRoot>, visible: bool) {
    backend.state_mut().scratch_visible = visible;
    backend.state_mut().animation = GeometryAnimation::TileFloat;
    backend.render();
}

/// First and last screen row carrying the dropdown's own frame. The dropdown is the focused pane
/// while it is open, so its double-ruled border tells it apart from the workspace beneath.
fn dropdown_rows(backend: &mut TestBackend<AppRoot>) -> Option<(usize, usize)> {
    let lines = backend.capture_frame().to_fixed_grid_lines();
    let rows: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.chars().any(|ch| matches!(ch, '╔' | '║' | '╚')))
        .map(|(index, _)| index)
        .collect();
    Some((*rows.first()?, *rows.last()?))
}

fn deployed(backend: &mut TestBackend<AppRoot>) -> (usize, usize) {
    dropdown_rows(backend).expect("the dropdown should be on screen")
}

#[test]
fn the_dropdown_grows_out_of_the_bottom_edge_it_never_leaves() {
    on_large_stack(|| {
        let mut backend = backend();
        assert_eq!(dropdown_rows(&mut backend), None, "hidden to start with");

        set_visible(&mut backend, true);
        let floor = usize::from(HEIGHT) - 1;
        let mut tops = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            let (top, bottom) = deployed(&mut backend);
            // The whole point: the bottom border holds the last row for the entire animation
            // instead of arriving with it.
            assert_eq!(
                bottom,
                floor,
                "the dropdown left its bottom edge behind:\n{}",
                backend.capture_frame().to_fixed_grid_lines().join("\n")
            );
            tops.push(top);
        }
        assert!(
            tops.windows(2).all(|pair| pair[0] > pair[1]),
            "the top edge should climb steadily, got {tops:?}"
        );

        backend.advance(Duration::from_millis(4_000));
        let (settled_top, settled_bottom) = deployed(&mut backend);
        assert_eq!(settled_bottom, floor);
        assert!(settled_top < *tops.last().expect("sampled"));
    });
}

#[test]
fn retracting_hands_the_rows_back_the_same_way() {
    on_large_stack(|| {
        let mut backend = backend();
        set_visible(&mut backend, true);
        backend.advance(Duration::from_millis(4_000));
        let (deployed_top, _) = deployed(&mut backend);

        set_visible(&mut backend, false);
        let floor = usize::from(HEIGHT) - 1;
        let mut tops = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            let Some((top, bottom)) = dropdown_rows(&mut backend) else {
                break;
            };
            assert_eq!(
                bottom, floor,
                "the bottom edge stays put on the way out too"
            );
            tops.push(top);
        }
        assert!(
            tops.last().is_some_and(|top| *top > deployed_top),
            "the dropdown should shrink back into the bottom edge, got {tops:?}"
        );
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "the top edge should retreat steadily, got {tops:?}"
        );

        backend.advance(Duration::from_millis(4_000));
        assert_eq!(dropdown_rows(&mut backend), None, "fully retracted");
    });
}

#[test]
fn a_terminal_resize_snaps_the_dropdown_instead_of_animating_it() {
    on_large_stack(|| {
        let mut backend = backend();
        set_visible(&mut backend, true);
        backend.advance(Duration::from_millis(4_000));
        let (top, bottom) = deployed(&mut backend);
        assert_eq!(bottom, usize::from(HEIGHT) - 1);
        let height = bottom - top;

        // A viewport change is not an animation: the dropdown belongs at its new size and place on
        // the very next frame, the same as every pane. The scratch layer used to opt out of that,
        // so the whole dropdown glided across the screen after each resize.
        let taller = HEIGHT + 10;
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: WIDTH,
            h: taller,
        });
        backend.render();
        let (top, bottom) = deployed(&mut backend);
        assert_eq!(
            bottom,
            usize::from(taller) - 1,
            "the dropdown should already be on the new bottom row:\n{}",
            backend.capture_frame().to_fixed_grid_lines().join("\n")
        );
        assert!(
            bottom - top > height,
            "and already grown to the taller viewport's share"
        );
    });
}

/// The deploy has to *finish* on its own clock. Its last row lands once progress is already within
/// an epsilon of settled, so a snap condition asking "is it still animating" hands that one row to a
/// full geometry transition - and the dropdown hangs a line short of home and crawls the rest.
#[test]
fn the_last_row_lands_with_the_rest_of_the_deploy() {
    on_large_stack(|| {
        let mut backend = backend();
        // Real durations: the bug is a disagreement between two clocks, so the deploy has to be
        // sampled against the geometry transition it would otherwise leak into. The height matters
        // too - `round` lands the dropdown's last row at `1 - 0.5 / rows_travelled`, so it is the
        // taller dropdowns whose last row falls past the end of the deploy.
        {
            let state = backend.state_mut();
            state.config.animations.geometry_duration = Duration::from_millis(220);
            state.config.scratchpad.height = 0.6;
        }
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: WIDTH,
            h: 24,
        });

        set_visible(&mut backend, true);
        // A frame cadence over the whole deploy - two thirds of `geometry_ms` - plus a frame's
        // grace. Nothing may still be in flight after this.
        for _ in 0..10 {
            backend.advance(Duration::from_millis(16));
        }
        let (arrived, _) = deployed(&mut backend);

        backend.advance(Duration::from_millis(4_000));
        let (settled, _) = deployed(&mut backend);
        assert_eq!(
            arrived,
            settled,
            "the dropdown stopped a row short and crawled the rest:\n{}",
            backend.capture_frame().to_fixed_grid_lines().join("\n")
        );
    });
}
