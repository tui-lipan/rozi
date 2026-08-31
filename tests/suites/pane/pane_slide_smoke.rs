//! Pins the `[animations] pane_style = "slide"` open animation.
//!
//! The point of the style is that the arriving pane is *clipped to its destination tile*: it emerges
//! from behind the seam at its final size rather than scaling up inside the tile or flying across its
//! neighbour. These tests assert that shape directly - where the pane's own frame sits at each stage,
//! and that nothing of it ever lands in the tile next door.

use std::time::Duration;

use rozi::AppRoot;
use rozi::layout::anim::{GeometryAnimation, PaneAnimationStyle, SlideEdge};
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{Pane, PaneBorderMode, SplitAxis};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::prelude::{FloatRect, MouseEvent, Rect};

const WIDTH: u16 = 40;
const HEIGHT: u16 = 10;

/// Vertical border columns of the two settled tiles in this fixture, left pane then right pane.
/// Asserted by [`the_fixture_geometry_is_what_the_other_tests_assume`] so a layout change fails
/// there with an explanation instead of making the slide assertions quietly meaningless.
const SETTLED_BORDERS: [usize; 4] = [0, 22, 24, 39];
/// First column of the arriving pane's tile.
const RIGHT_TILE_START: usize = SETTLED_BORDERS[2];

fn backend(style: PaneAnimationStyle) -> TestBackend<AppRoot> {
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
        // What a real spawn arms. Without it every geometry transition is instant, so the scale
        // contrast below would snap straight to the settled rect.
        state.animation = GeometryAnimation::Spawn;
        state.config.animations.enabled = true;
        state.config.animations.pane_style = style;
        state.config.animations.geometry_duration = Duration::from_millis(200);
        state.config.animations.open_delay = Duration::ZERO;
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = PaneBorderMode::Separate;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.start_axis = SplitAxis::Horizontal;
        workspace.panes.clear();
        workspace.tile_tree = None;
        for id in [10, 11] {
            let mut pane = Pane::new(
                id,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: f32::from(WIDTH),
                    h: f32::from(HEIGHT),
                },
            );
            // Pane 11 is the one arriving; 10 is the tile it took the space from.
            pane.opening = id == 11;
            pane.slide_edge = SlideEdge::Right;
            pane.terminal_active = true;
            workspace.panes.push(pane);
        }
        workspace.tile_tree = build_dwindle_tree(&[10, 11], workspace.start_axis, &[]);
        workspace.focused_pane = Some(10);
        // The attachment keeps its own focus alongside the workspace's; the click test reads that one.
        state.current_mut().focused_pane = Some(10);
    }
    backend
}

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn pane-slide smoke test")
        .join()
        .expect("pane-slide smoke test completes");
}

/// Let the arriving pane start arriving: the target flips to deployed and the transition carries it.
fn begin_arrival(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().workspaces[0]
        .panes
        .iter_mut()
        .find(|pane| pane.id == 11)
        .expect("arriving pane")
        .opening = false;
    backend.render();
}

/// Columns on the panes' middle row carrying a vertical frame glyph. The middle row avoids the
/// horizontal top/bottom runs, so every hit is a left or right border.
fn border_columns(backend: &mut TestBackend<AppRoot>) -> Vec<usize> {
    let frame = backend.capture_frame();
    let lines = frame.to_fixed_grid_lines();
    lines[(HEIGHT / 2) as usize]
        .chars()
        .enumerate()
        .filter(|(_, ch)| *ch == '│')
        .map(|(index, _)| index)
        .collect()
}

fn grid(backend: &mut TestBackend<AppRoot>) -> String {
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

#[test]
fn the_fixture_geometry_is_what_the_other_tests_assume() {
    on_large_stack(|| {
        let mut backend = backend(PaneAnimationStyle::Slide);
        begin_arrival(&mut backend);
        backend.advance(Duration::from_millis(400));
        assert_eq!(
            border_columns(&mut backend),
            SETTLED_BORDERS.to_vec(),
            "the two tiles moved; the slide assertions below are written against these columns:\n{}",
            grid(&mut backend)
        );
    });
}

#[test]
fn an_opening_pane_starts_entirely_outside_its_tile() {
    on_large_stack(|| {
        let mut backend = backend(PaneAnimationStyle::Slide);
        backend.render();
        // Still `opening`, so slide progress rests at 0.0: the pane sits a full tile to the right of
        // its destination and the clip leaves none of it on screen.
        let columns = border_columns(&mut backend);
        assert!(
            columns.iter().all(|column| *column < RIGHT_TILE_START),
            "a pane that has not started arriving must not be drawn: {columns:?}\n{}",
            grid(&mut backend)
        );
    });
}

#[test]
fn a_sliding_pane_is_clipped_to_its_tile_and_never_leaks_into_its_neighbour() {
    on_large_stack(|| {
        let mut backend = backend(PaneAnimationStyle::Slide);
        backend.render();
        begin_arrival(&mut backend);
        backend.advance(Duration::from_millis(100));

        let columns = border_columns(&mut backend);
        let (left_of_seam, in_tile): (Vec<usize>, Vec<usize>) = columns
            .iter()
            .copied()
            .partition(|column| *column < RIGHT_TILE_START);

        // Half way in, the pane shows exactly its leading (left) border: the trailing one is still
        // outside the tile, and the clip drops it rather than painting it over the viewport edge.
        assert_eq!(
            in_tile.len(),
            1,
            "mid-slide only the leading border belongs inside the tile: {columns:?}\n{}",
            grid(&mut backend)
        );
        // Strictly inside - past the tile's own left edge, short of its right edge - which is what
        // distinguishes a pane part-way in from one that has arrived or never left.
        assert!(
            in_tile[0] > RIGHT_TILE_START && in_tile[0] < SETTLED_BORDERS[3],
            "the leading border should be travelling inside the tile: {columns:?}\n{}",
            grid(&mut backend)
        );
        // The clip window is the destination tile, so nothing of the arriving pane can paint over
        // the neighbour it is emerging from behind: only that neighbour's own two borders are left
        // of the seam.
        assert_eq!(
            left_of_seam,
            vec![SETTLED_BORDERS[0], SETTLED_BORDERS[1]],
            "the arriving pane leaked past its own tile: {columns:?}\n{}",
            grid(&mut backend)
        );

        backend.advance(Duration::from_millis(300));
        assert_eq!(
            border_columns(&mut backend),
            SETTLED_BORDERS.to_vec(),
            "a deployed pane fills its tile:\n{}",
            grid(&mut backend)
        );
    });
}

/// The springy half of the style: the tile that gives up the space overshoots its new size once and
/// settles back, rather than gliding into it. Sampled as the column of that pane's own right border,
/// which has to travel *past* its resting column before coming back to it.
#[test]
fn the_tile_making_room_overshoots_its_new_size_and_settles() {
    on_large_stack(|| {
        let mut backend = backend(PaneAnimationStyle::Slide);
        // Start with pane 10 alone, so its rect transition is resting at the full-width tile.
        {
            let workspace = &mut backend.state_mut().current_mut().workspaces[0];
            workspace.panes.retain(|pane| pane.id == 10);
            workspace.tile_tree = build_dwindle_tree(&[10], SplitAxis::Horizontal, &[]);
        }
        backend.render();
        let full_width = border_columns(&mut backend);
        assert_eq!(
            full_width,
            vec![0, (WIDTH - 1) as usize],
            "the lone pane should fill the viewport:\n{}",
            grid(&mut backend)
        );

        // Pane 11 arrives and takes the right half, so pane 10's target shrinks to the left tile.
        {
            let workspace = &mut backend.state_mut().current_mut().workspaces[0];
            let mut arriving = Pane::new(
                11,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: f32::from(WIDTH),
                    h: f32::from(HEIGHT),
                },
            );
            arriving.opening = false;
            arriving.slide_edge = SlideEdge::Right;
            arriving.terminal_active = true;
            workspace.panes.push(arriving);
            workspace.tile_tree = build_dwindle_tree(&[10, 11], SplitAxis::Horizontal, &[]);
        }
        backend.render();

        // Sample the shrinking pane's right border across the whole animation.
        let resting = SETTLED_BORDERS[1];
        let mut minimum = usize::MAX;
        for _ in 0..14 {
            backend.advance(Duration::from_millis(20));
            if let Some(column) = border_columns(&mut backend)
                .into_iter()
                .filter(|column| *column > 0)
                .min()
            {
                minimum = minimum.min(column);
            }
        }
        assert!(
            minimum < resting,
            "the tile making room should overshoot past its resting column {resting}, \
             but never went below {minimum}"
        );

        backend.advance(Duration::from_millis(300));
        let settled = border_columns(&mut backend);
        assert_eq!(
            settled,
            SETTLED_BORDERS.to_vec(),
            "the spring has to settle exactly on the layout, not near it:\n{}",
            grid(&mut backend)
        );
    });
}

/// The clip wrapper sits between the workspace canvas and the pane, so it is on the path every
/// pointer event takes. A settled pane has to stay clickable through it.
#[test]
fn a_slid_pane_is_still_clickable_once_it_has_arrived() {
    on_large_stack(|| {
        let mut backend = backend(PaneAnimationStyle::Slide);
        begin_arrival(&mut backend);
        backend.advance(Duration::from_millis(400));
        assert_eq!(
            backend.state().current().focused_pane,
            Some(10),
            "the left pane starts focused"
        );

        // Well inside the arrived pane's body.
        let (x, y) = ((RIGHT_TILE_START + 6) as u16, HEIGHT / 2);
        backend
            .send_mouse(MouseEvent {
                x,
                y,
                kind: MouseKind::Down(MouseButton::Left),
                mods: Default::default(),
            })
            .expect("mouse down");
        assert_eq!(
            backend.state().current().focused_pane,
            Some(11),
            "clicking a pane that slid in must focus it through the clip wrapper"
        );
    });
}

#[test]
fn the_scale_style_grows_an_opening_pane_inside_its_tile_instead() {
    on_large_stack(|| {
        // The contrast that makes the slide assertions meaningful. Scale places an opening pane at a
        // shrunken rect *within* its tile, so it shows *both* of its borders, inset from the tile
        // edges - never one border part-way across. Sampled on the frame arrival starts, since a 0.9
        // scale of a 16-column tile is barely over a cell per side and rounding closes it quickly.
        let mut backend = backend(PaneAnimationStyle::Scale);
        backend.render();
        begin_arrival(&mut backend);

        let in_tile: Vec<usize> = border_columns(&mut backend)
            .into_iter()
            .filter(|column| *column >= RIGHT_TILE_START)
            .collect();
        assert_eq!(
            in_tile.len(),
            2,
            "a scaling pane keeps both borders on screen:\n{}",
            grid(&mut backend)
        );
        assert!(
            in_tile[0] > RIGHT_TILE_START && in_tile[1] < SETTLED_BORDERS[3],
            "a scaling pane starts inset from its tile edges: {in_tile:?}\n{}",
            grid(&mut backend)
        );

        backend.advance(Duration::from_millis(400));
        assert_eq!(
            border_columns(&mut backend),
            SETTLED_BORDERS.to_vec(),
            "both styles settle in the same place:\n{}",
            grid(&mut backend)
        );
    });
}
