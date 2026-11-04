//! Fixtures shared by the shared-layout module tests.

use super::*;
use crate::config::Config;
use crate::state::{Pane, State};

pub(super) fn state_with_split() -> State {
    let mut state = State::new(Config::default(), Theme::default());
    let rect = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 80.0,
        h: 24.0,
    };
    let previous = state.current().workspaces[0].focused_pane;
    let mut pane = Pane::new(2, state.config.scrollback, rect);
    pane.pty_generation = 7;
    state.current_mut().workspaces[0].panes.push(pane);
    let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 25,
    });
    let split_width_multiplier = state.config.layout.split_width_multiplier;
    crate::layout::place_spawned_pane(
        &mut state.current_mut().workspaces[0],
        2,
        previous,
        bounds,
        0.0,
        crate::layout::model::TileGap::DEFAULT,
        split_width_multiplier,
    );
    state.current_mut().next_pane_id = 3;
    state.current_mut().next_pty_generation = 8;
    state
}

pub(super) fn layout_with_panes(panes: &[(PaneId, u64)]) -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 100,
        canvas_rows: 28,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: None,
            synchronized: false,
            layout: SharedLayoutKind::Dwindle,
            start_axis: SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: None,
            panes: panes
                .iter()
                .map(|(id, generation)| SharedPane {
                    pane_id: *id,
                    generation: *generation,
                    title: None,
                    profile_name: None,
                    cwd: None,
                    launch: None,
                    replay: false,
                    keep_open: false,
                    floating: false,
                    pinned: false,
                    fullscreen: false,
                    rect: None,
                    scrollable_width: crate::layout::model::DEFAULT_SCROLLABLE_WIDTH,
                })
                .collect(),
        }],
    }
}
