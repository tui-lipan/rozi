//! Pins the sidebar's reveal and retract animation.
//!
//! The slide reads as a *push* because the pane column genuinely gives up the space, the same way
//! the tile beside a spawning pane does: the columns the sidebar reserves grow with the animation
//! and the pane column is resized to match. So its near edge travels with the panel while its far
//! edge stays pinned to the far edge of the screen, and the frame is whole in every frame - no
//! border is ever clipped off, and no gutter ever opens.
//!
//! The panel itself is not resized at all: it is laid out at its settled width and clipped to the
//! columns handed over so far, so it slides in whole rather than re-wrapping its tabs and rows on
//! every frame.

use std::time::Duration;

use rozi::AppRoot;
use rozi::config::{SidebarPosition, SidebarTab};
use rozi::state::{Pane, PaneBorderMode};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

const WIDTH: u16 = 100;
const HEIGHT: u16 = 20;
const SIDEBAR: usize = 32;
/// Sample points spread across the slide, none of them close enough to an end to be settled.
const IN_FLIGHT: [u64; 3] = [200, 500, 900];

fn backend(position: SidebarPosition) -> TestBackend<AppRoot> {
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
        // Deliberately slow: the assertions below sample the slide part-way through, and a short
        // animation would be raced by the wall-clock cost of the fixture itself.
        state.config.animations.geometry_duration = Duration::from_millis(3_000);
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = PaneBorderMode::Separate;
        state.config.sidebar.position = position;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(
            1,
            5_000,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: f32::from(WIDTH),
                h: f32::from(HEIGHT),
            },
        );
        pane.opening = false;
        pane.terminal_active = true;
        workspace.panes.push(pane);
        workspace.focused_pane = Some(1);
        state.current_mut().focused_pane = Some(1);
    }
    // Seeds the slide at retracted, so the reveal below is a real toggle rather than the settled
    // startup state.
    backend.render();
    backend
}

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn sidebar-slide smoke test")
        .join()
        .expect("sidebar-slide smoke test completes");
}

fn set_visible(backend: &mut TestBackend<AppRoot>, visible: bool) {
    backend.state_mut().sidebar_visible = visible;
    backend.render();
}

/// The pane frame's top row, which is the one row that says where the column's edges are: its two
/// corners, and an unbroken run of border between them.
fn pane_top_row(backend: &mut TestBackend<AppRoot>) -> String {
    backend.capture_frame().to_fixed_grid_lines()[0].clone()
}

/// Columns of the pane frame's top corners. The sidebar contributes only straight glyphs - its one
/// vertical per row is the splitter handle - so the corners name the pane column's two edges
/// unambiguously wherever the slide has carried them. Both are expected in every frame: a missing
/// one means a border has been clipped off the screen, which is the artifact these tests exist to
/// catch.
fn pane_corners(backend: &mut TestBackend<AppRoot>) -> Vec<usize> {
    pane_top_row(backend)
        .chars()
        .enumerate()
        .filter(|(_, ch)| matches!(ch, '╭' | '╮'))
        .map(|(index, _)| index)
        .collect()
}

#[test]
fn revealing_pushes_the_near_edge_and_leaves_the_far_edge_alone() {
    on_large_stack(|| {
        let mut backend = backend(SidebarPosition::Left);
        assert_eq!(pane_corners(&mut backend), vec![0, usize::from(WIDTH) - 1]);

        set_visible(&mut backend, true);
        let mut travel = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            let corners = pane_corners(&mut backend);
            // Both corners, every frame: the column is resized rather than shoved, so neither of
            // its borders is ever clipped off the screen and no gutter opens beside either.
            assert_eq!(
                corners.len(),
                2,
                "the pane frame should be whole mid-slide, got {corners:?}\n{}",
                backend.capture_frame().to_fixed_grid_lines().join("\n")
            );
            assert_eq!(
                corners[1],
                usize::from(WIDTH) - 1,
                "the far edge must not move:\n{}",
                backend.capture_frame().to_fixed_grid_lines().join("\n")
            );
            travel.push(corners[0]);
        }
        assert!(
            travel.windows(2).all(|pair| pair[0] < pair[1]),
            "the near edge should advance steadily, got {travel:?}"
        );
        assert!(travel.iter().all(|column| *column < SIDEBAR));

        backend.advance(Duration::from_millis(4_000));
        assert_eq!(
            pane_corners(&mut backend),
            vec![SIDEBAR, usize::from(WIDTH) - 1],
            "the column lands beside the sidebar, still flush with the far edge"
        );
    });
}

#[test]
fn the_pane_column_gives_up_its_columns_steadily_rather_than_in_one_step() {
    on_large_stack(|| {
        let mut backend = backend(SidebarPosition::Left);
        let viewport = backend.viewport();
        let width = |backend: &TestBackend<AppRoot>| backend.state().content_viewport(viewport).w;
        assert_eq!(width(&backend), WIDTH);

        set_visible(&mut backend, true);
        let mut widths = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            widths.push(width(&backend));
        }
        assert!(
            widths.windows(2).all(|pair| pair[0] > pair[1]),
            "the pane column should narrow as the sidebar arrives, got {widths:?}"
        );
        assert!(widths.iter().all(|w| *w > WIDTH - SIDEBAR as u16));

        backend.advance(Duration::from_millis(4_000));
        assert_eq!(width(&backend), WIDTH - SIDEBAR as u16);

        // And back, the same way.
        set_visible(&mut backend, false);
        let mut widths = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            widths.push(width(&backend));
        }
        assert!(
            widths.windows(2).all(|pair| pair[0] < pair[1]),
            "the pane column should widen as the sidebar leaves, got {widths:?}"
        );
        backend.advance(Duration::from_millis(4_000));
        assert_eq!(width(&backend), WIDTH);
    });
}

/// The panel's own row of tabs, cropped to the columns it currently occupies.
fn panel_row(backend: &mut TestBackend<AppRoot>, columns: usize) -> Vec<char> {
    backend.capture_frame().to_fixed_grid_lines()[0]
        .chars()
        .take(columns)
        .collect()
}

#[test]
fn the_panel_slides_in_whole_rather_than_being_re_laid_out() {
    on_large_stack(|| {
        let mut backend = backend(SidebarPosition::Left);
        let viewport = backend.viewport();
        // The splitter spends a column on its handle, so the panel is one short of the reservation.
        let panel_width = SIDEBAR - 1;

        set_visible(&mut backend, true);
        backend.advance(Duration::from_millis(4_000));
        let settled = panel_row(&mut backend, panel_width);
        assert!(
            settled.iter().collect::<String>().contains("Panes"),
            "the fixture's panel should carry a tab label, got {:?}",
            settled.iter().collect::<String>()
        );

        // Back out and part-way in again, so the panel is caught mid-slide.
        set_visible(&mut backend, false);
        backend.advance(Duration::from_millis(4_000));
        set_visible(&mut backend, true);
        backend.advance(Duration::from_millis(500));

        let window = usize::from(backend.state().effective_sidebar_width(viewport)) - 1;
        assert!(window > 0 && window < panel_width, "sampled mid-slide");
        let slice = panel_row(&mut backend, window);
        // What shows is the tail of the settled panel, column for column: the panel keeps its full
        // width and the screen edge cuts through it. A panel re-laid-out into the columns it has so
        // far would show its *leading* columns instead, re-wrapped to fit.
        assert_eq!(
            slice,
            settled[panel_width - window..].to_vec(),
            "the panel should be clipped to its tail, not re-laid-out into the window:\n{}",
            backend.capture_frame().to_fixed_grid_lines().join("\n")
        );
    });
}

#[test]
fn a_right_dock_pushes_the_pane_column_the_other_way() {
    on_large_stack(|| {
        let mut backend = backend(SidebarPosition::Right);
        set_visible(&mut backend, true);

        let mut travel = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            let corners = pane_corners(&mut backend);
            assert_eq!(corners.len(), 2, "the pane frame should be whole mid-slide");
            // Mirrored: the near edge is the pinned one now, and the panel pushes the far edge.
            assert_eq!(corners[0], 0, "the near edge must not move");
            travel.push(corners[1]);
        }
        assert!(
            travel.windows(2).all(|pair| pair[0] > pair[1]),
            "the far edge should retreat steadily, got {travel:?}"
        );

        backend.advance(Duration::from_millis(4_000));
        assert_eq!(
            pane_corners(&mut backend),
            vec![0, usize::from(WIDTH) - SIDEBAR - 1]
        );
    });
}

#[test]
fn retracting_hands_the_columns_back_the_same_way() {
    on_large_stack(|| {
        let mut backend = backend(SidebarPosition::Left);
        set_visible(&mut backend, true);
        backend.advance(Duration::from_millis(4_000));
        assert_eq!(pane_corners(&mut backend)[0], SIDEBAR);

        set_visible(&mut backend, false);
        let mut travel = Vec::new();
        for step in IN_FLIGHT {
            backend.advance(Duration::from_millis(step));
            let corners = pane_corners(&mut backend);
            assert_eq!(corners.len(), 2, "the pane frame should be whole mid-slide");
            assert_eq!(
                corners[1],
                usize::from(WIDTH) - 1,
                "the far edge must not move"
            );
            travel.push(corners[0]);
        }
        assert!(
            travel.windows(2).all(|pair| pair[0] > pair[1]),
            "the near edge should follow the retreating panel, got {travel:?}"
        );

        backend.advance(Duration::from_millis(4_000));
        assert_eq!(pane_corners(&mut backend), vec![0, usize::from(WIDTH) - 1]);
    });
}
