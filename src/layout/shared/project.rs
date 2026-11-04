//! Projecting a client's live `State` into a shared layout document, and reading placements back
//! out of one.

use super::*;

use crate::state::State;

impl SharedPaneGeometry {
    pub fn from_config(pane: crate::config::PaneConfig) -> Self {
        let top_gap = if pane.show_workbar && pane.workbar_gap {
            if pane.workbar_at_bottom { -1 } else { 1 }
        } else {
            0
        };
        Self {
            show_titles: pane.show_titles,
            titlebar: pane.titlebar,
            border_mode: pane.border_mode,
            keep_special_borders: pane.keep_special_borders,
            padding: pane.padding,
            top_gap,
        }
    }

    pub(crate) fn apply(self, pane: &mut crate::config::PaneConfig) {
        pane.show_titles = self.show_titles;
        pane.titlebar = self.titlebar;
        pane.border_mode = self.border_mode;
        pane.keep_special_borders = self.keep_special_borders;
        pane.padding = self.padding;
    }
}

/// Build the shared document from the client's live `State` for the given canonical canvas size
/// (in cells, excluding the workbar). The scratchpad is excluded because it is
/// local-only lifecycle, never shared.
pub fn shared_layout_from_state(state: &State, canvas: (u16, u16)) -> SharedLayout {
    let canvas_cols = canvas.0.max(1);
    let canvas_rows = canvas.1.max(1);
    let mut layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols,
        canvas_rows,
        pane_geometry: SharedPaneGeometry::from_config(state.workspace_pane_config()),
        workspaces: state
            .current()
            .workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                shared_workspace_from_state(index, workspace, canvas_cols, canvas_rows)
            })
            .collect(),
    };
    preserve_projected_rects(state, &mut layout);
    layout
}

/// A projected float must retain its canonical fractions until its local cell rect changes.
/// Otherwise a controller edit could publish a second revision just from a float round trip.
fn preserve_projected_rects(state: &State, layout: &mut SharedLayout) {
    let Some(previous) = state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.last_committed_layout.as_ref())
    else {
        return;
    };
    if (previous.canvas_cols, previous.canvas_rows) != (layout.canvas_cols, layout.canvas_rows) {
        return;
    }
    let rects: std::collections::HashMap<_, _> = previous
        .workspaces
        .iter()
        .flat_map(|ws| &ws.panes)
        .filter_map(|pane| {
            pane.rect
                .map(|rect| ((pane.pane_id, pane.generation), rect))
        })
        .collect();
    for pane in layout.workspaces.iter_mut().flat_map(|ws| &mut ws.panes) {
        if let Some(rect) = rects.get(&(pane.pane_id, pane.generation))
            && let Some(local) = state
                .current()
                .workspaces
                .iter()
                .flat_map(|ws| &ws.panes)
                .find(|local| local.id == pane.pane_id)
            && local.floating
            && local.floating_rect
                == frac_rect_to_float(*rect, layout.canvas_cols, layout.canvas_rows)
        {
            pane.rect = Some(*rect);
        }
    }
}

pub(super) fn shared_workspace_from_state(
    index: usize,
    workspace: &crate::state::Workspace,
    canvas_cols: u16,
    canvas_rows: u16,
) -> SharedWorkspace {
    // The effective tree prunes to live tiled panes and appends any stragglers, so it matches the
    // layout engine's live pane set.
    let tree = crate::layout::effective_tile_tree(workspace, None)
        .as_ref()
        .and_then(|tree| from_dwindle(tree, &|id| Some(id)));
    SharedWorkspace {
        index,
        name: workspace.name.clone(),
        synchronized: workspace.synchronized,
        layout: workspace.layout_kind.into(),
        start_axis: workspace.start_axis.into(),
        split_ratios: workspace.split_ratios.clone(),
        tree,
        panes: workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .map(|pane| SharedPane {
                pane_id: pane.id,
                generation: pane.pty_generation,
                title: pane.identity.custom_title.clone(),
                profile_name: pane.identity.profile_name.clone(),
                cwd: pane.identity.cwd.clone(),
                launch: pane.identity.launch.clone(),
                replay: pane.identity.replay,
                keep_open: pane.identity.keep_open,
                floating: pane.floating,
                pinned: pane.pinned,
                fullscreen: pane.fullscreen,
                rect: pane
                    .floating
                    .then(|| float_rect_to_frac(pane.floating_rect, canvas_cols, canvas_rows)),
                scrollable_width: crate::layout::tiling::sanitize_scrollable_width(
                    pane.scrollable_width,
                ),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_default_geometry_matches_the_default_config() {
        assert_eq!(
            SharedPaneGeometry::default(),
            SharedPaneGeometry::from_config(crate::config::PaneConfig::default())
        );
    }
    use crate::layout::shared::test_support::state_with_split;

    use crate::config::Config;
    use crate::layout::model::LayoutKind;
    use crate::state::Pane;

    #[test]
    fn client_scratchpad_is_excluded_from_shared_layout() {
        let mut state = State::new(Config::default(), Theme::default());
        let scratch_id = 1 << 31;
        state
            .scratch
            .panes
            .push(Pane::new(scratch_id, 100, FloatRect::default()));
        crate::layout::tiling::append_tiled_window(&mut state.scratch, scratch_id);

        let layout = shared_layout_from_state(&state, (100, 30));
        assert!(layout.workspaces.iter().all(|workspace| {
            workspace
                .panes
                .iter()
                .all(|pane| pane.pane_id != scratch_id)
        }));
    }

    /// A session server measures panes from the document, a client from its live workspace. The
    /// two must agree for every layout, or `layout get` answers differently depending on whether
    /// anyone is attached.
    #[test]
    fn the_shared_document_places_panes_where_the_live_workspace_does() {
        let mut state = State::new(Config::default(), Theme::default());
        let canvas = (100_u16, 30_u16);
        {
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes.clear();
            workspace.tile_tree = None;
            for id in 1..=4 {
                workspace
                    .panes
                    .push(Pane::new(id, 100, FloatRect::default()));
                crate::layout::tiling::append_tiled_window(workspace, id);
            }
            // Uneven Scrollable widths overflow the canvas, which is the case most likely to drift.
            for (pane, width) in workspace.panes.iter_mut().zip([0.5, 0.7, 0.3, 0.6]) {
                pane.scrollable_width = width;
            }
            let mut float = Pane::new(5, 100, FloatRect::default());
            float.floating = true;
            float.floating_rect = FloatRect {
                x: 10.0,
                y: 5.0,
                w: 30.0,
                h: 10.0,
            };
            workspace.panes.push(float);
            workspace.split_ratios[0] = 0.7;
            // The Scrollable anchor and focus are client-local; a document reader starts at the
            // first column, so compare against a client that has not scrolled.
            workspace.focused_pane = None;
            workspace.scrollable_anchor = None;
        }

        for kind in [
            LayoutKind::Dwindle,
            LayoutKind::Master,
            LayoutKind::Grid,
            LayoutKind::Columns,
            LayoutKind::Rows,
            LayoutKind::Scrollable,
            LayoutKind::Monocle,
        ] {
            state.current_mut().workspaces[0].layout_kind = kind;
            let bounds = FloatRect {
                x: 0.0,
                y: 0.0,
                w: f32::from(canvas.0),
                h: f32::from(canvas.1),
            };
            let cells = |placements: Vec<crate::layout::tiling::PanePlacement>| {
                let mut cells: Vec<_> = placements
                    .into_iter()
                    .map(|placement| {
                        (
                            placement.id,
                            crate::control::CellRect::from_float(placement.rect),
                        )
                    })
                    .collect();
                cells.sort_by_key(|(id, _)| *id);
                cells
            };
            let live = cells(crate::layout::workspace_target_rects(
                &state.current().workspaces[0],
                bounds,
                0.0,
                crate::layout::model::TileGap {
                    horizontal: 0.0,
                    vertical: 0.0,
                },
            ));
            let document = shared_layout_from_state(&state, canvas);
            let shared = cells(shared_workspace_placements(&document.workspaces[0], canvas));
            assert_eq!(live.len(), 5, "{kind:?} places every pane");
            assert_eq!(shared, live, "{kind:?} places panes differently");
        }
    }

    #[test]
    fn shared_layout_captures_panes_and_tree() {
        let state = state_with_split();
        let layout = shared_layout_from_state(&state, (80, 24));
        assert_eq!(layout.version, SHARED_LAYOUT_VERSION);
        assert_eq!(layout.canvas_cols, 80);
        let ws = &layout.workspaces[0];
        assert_eq!(ws.panes.len(), 2);
        assert!(ws.tree.is_some());
    }

    #[test]
    fn controller_projection_reemits_exact_canonical_float_fractions() {
        let mut state = State::new(Config::default(), Theme::default());
        let canvas = (137, 53);
        let rect = FracRect {
            x: 0.12345679,
            y: 0.2345679,
            w: 0.43219876,
            h: 0.32198766,
        };
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.pty_generation = 1;
        pane.floating = true;
        pane.floating_rect = frac_rect_to_float(rect, canvas.0, canvas.1);
        let mut canonical = shared_layout_from_state(&state, canvas);
        canonical.workspaces[0].panes[0].rect = Some(rect);
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.last_committed_layout = Some(canonical.clone());
        state.current_mut().shared = Some(shared);
        assert_eq!(shared_layout_from_state(&state, canvas), canonical);
        state.current_mut().workspaces[0].panes[0].floating_rect.x += 1.0;
        assert_ne!(shared_layout_from_state(&state, canvas), canonical);
    }

    #[test]
    fn floating_rect_round_trips_through_fractions() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes[0].floating = true;
        state.current_mut().workspaces[0].panes[0].floating_rect = FloatRect {
            x: 20.0,
            y: 6.0,
            w: 40.0,
            h: 12.0,
        };
        let layout = shared_layout_from_state(&state, (80, 24));
        let rect = layout.workspaces[0].panes[0].rect.expect("floating rect");
        let restored = frac_rect_to_float(rect, 80, 24);
        assert!((restored.x - 20.0).abs() < 0.001);
        assert!((restored.w - 40.0).abs() < 0.001);
    }

    #[test]
    fn dwindle_from_shared_drops_unknown_leaves() {
        let tree = SharedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SharedTree::Leaf { pane: 1 }),
            second: Box::new(SharedTree::Leaf { pane: 2 }),
        };
        let known: std::collections::HashSet<PaneId> = [1].into_iter().collect();
        let rebuilt = dwindle_from_shared(&tree, &known);
        assert_eq!(rebuilt, Some(DwindleTree::Leaf(1)));
    }

    #[test]
    fn shared_layout_round_trips_columns_and_scrollable_kinds() {
        for kind in [
            SharedLayoutKind::Columns,
            SharedLayoutKind::Rows,
            SharedLayoutKind::Scrollable,
        ] {
            let layout = SharedLayout {
                version: SHARED_LAYOUT_VERSION,
                pane_geometry: Default::default(),
                canvas_cols: 80,
                canvas_rows: 24,
                workspaces: vec![SharedWorkspace {
                    index: 0,
                    name: None,
                    synchronized: false,
                    layout: kind,
                    start_axis: SharedSplitAxis::Horizontal,
                    split_ratios: Vec::new(),
                    tree: None,
                    panes: vec![SharedPane {
                        pane_id: 1,
                        generation: 1,
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
                    }],
                }],
            };
            let encoded = serde_json::to_value(&layout).expect("encode");
            let label = match kind {
                SharedLayoutKind::Columns => "columns",
                SharedLayoutKind::Rows => "rows",
                SharedLayoutKind::Scrollable => "scrollable",
                _ => unreachable!(),
            };
            assert_eq!(encoded["workspaces"][0]["layout"], label);
            assert!(!encoded.to_string().contains("scrollable_anchor"));
            let decoded: SharedLayout = serde_json::from_value(encoded).expect("decode");
            assert_eq!(decoded.workspaces[0].layout, kind);
        }
    }
}
