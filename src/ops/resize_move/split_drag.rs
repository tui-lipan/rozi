use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::active_pane_is_fullscreen;
use crate::state::{self, LayoutKind, PaneId, SplitDragKind, SplitDragSession, Workspace};
use crate::tiling::{SplitEdge, axis_split_path_for_edge, resize_tiled_split_for_edge};

use super::float::{ensure_tile_tree, layout_has_resizable_splits};
use super::tiling::{master_available_width, resize_master_split_by_pixels};

pub(crate) fn begin_resize_split_drag(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    x: u16,
    y: u16,
) -> Update {
    begin_split_drag(
        ctx,
        SplitDragKind::Single {
            pane_id,
            horizontal_split,
        },
        x,
        y,
    )
}

pub(crate) fn begin_resize_split_junction_drag(
    ctx: &mut Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
    x: u16,
    y: u16,
) -> Update {
    begin_split_drag(
        ctx,
        SplitDragKind::Junction {
            horizontal_panes,
            vertical_panes,
        },
        x,
        y,
    )
}

fn begin_split_drag(ctx: &mut Context<HyprmuxApp>, kind: SplitDragKind, x: u16, y: u16) -> Update {
    if crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    let workspace_index = ctx.state.current().active_workspace;
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
    ensure_tile_tree(workspace);
    ctx.state.split_drag = Some(SplitDragSession {
        kind,
        workspace: workspace_index,
        start_x: x,
        start_y: y,
        start_tile_tree: workspace.tile_tree.clone(),
        start_split_ratios: workspace.split_ratios.clone(),
    });
    Update::none()
}

/// Resolve which boundary a drag event belongs to, and rewind the layout to the drag's starting
/// ratios so the pointer delta can be applied absolutely.
///
/// The live session - not the event payload - decides the target. Drag events are routed by the
/// position a mouse region occupied when the pointer went down, and a resize strip moves as soon as
/// the split it describes moves, so a mid-gesture event routinely arrives from a *neighbouring*
/// strip's region carrying that strip's pane id. The drag origin is fixed for the whole gesture, so
/// matching on it keeps every event applied to the boundary the pointer actually grabbed. Only a
/// gesture with no session yet (the drag-start event) adopts `requested`.
fn resolve_split_drag(
    ctx: &mut Context<HyprmuxApp>,
    requested: SplitDragKind,
    from_x: u16,
    from_y: u16,
) -> Option<SplitDragKind> {
    // Followers are nudged once at drag start; do not re-nudge (via `begin_split_drag`) on every
    // subsequent drag event, which would stack a toast per pointer move. A follower never has a
    // session, so the resize bails regardless.
    if !ctx.state.is_controller() {
        return None;
    }
    let workspace_index = ctx.state.current().active_workspace;
    let matches = ctx.state.split_drag.as_ref().is_some_and(|session| {
        session.workspace == workspace_index
            && session.start_x == from_x
            && session.start_y == from_y
    });
    if !matches {
        begin_split_drag(ctx, requested, from_x, from_y);
    }

    let session = ctx.state.split_drag.as_ref()?;
    let kind = session.kind.clone();
    let start_tile_tree = session.start_tile_tree.clone();
    let start_split_ratios = session.start_split_ratios.clone();
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
    workspace.tile_tree = start_tile_tree;
    workspace.split_ratios = start_split_ratios;
    Some(kind)
}

/// Adjust the split on a tiled boundary by a mouse drag. `pane_id` is the pane on the
/// left/top side of the dragged gap, so its trailing edge identifies the exact divider even when
/// the pane has a deeper split on the same axis. `horizontal_split` is true for a vertical gap (a
/// left|right split). Used by the draggable gap strips in the view. Dwindle and master only.
pub(crate) fn resize_split_by_drag(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let requested = SplitDragKind::Single {
        pane_id,
        horizontal_split,
    };
    let Some(kind) = resolve_split_drag(ctx, requested, from_x, from_y) else {
        return Update::none();
    };
    apply_split_drag(ctx, kind, from_x, from_y, x, y)
}

fn apply_split_drag(
    ctx: &mut Context<HyprmuxApp>,
    kind: SplitDragKind,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let dx = (x as i32 - from_x as i32) as f32;
    let dy = (y as i32 - from_y as i32) as f32;
    match kind {
        SplitDragKind::Single {
            pane_id,
            horizontal_split,
        } => {
            let pixels = if horizontal_split { dx } else { dy };
            apply_resize_split_pixels(ctx, pane_id, horizontal_split, pixels);
        }
        SplitDragKind::Junction {
            horizontal_panes,
            vertical_panes,
        } => {
            let workspace = &ctx.state.current().workspaces[ctx.state.current().active_workspace];
            let horizontal_panes = distinct_split_representatives(
                workspace,
                &horizontal_panes,
                state::SplitAxis::Horizontal,
            );
            let vertical_panes = distinct_split_representatives(
                workspace,
                &vertical_panes,
                state::SplitAxis::Vertical,
            );
            for pane_id in horizontal_panes {
                apply_resize_split_pixels(ctx, pane_id, true, dx);
            }
            for pane_id in vertical_panes {
                apply_resize_split_pixels(ctx, pane_id, false, dy);
            }
        }
    }
    Update::full()
}

fn apply_resize_split_pixels(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
    pixels: f32,
) -> bool {
    if active_pane_is_fullscreen(&ctx.state, pane_id) {
        return false;
    }
    let axis = if horizontal_split {
        state::SplitAxis::Horizontal
    } else {
        state::SplitAxis::Vertical
    };

    let workspace_index = ctx.state.current().active_workspace;
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let tile_bounds = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap());
    let tile_gap = ctx.state.tile_gap();
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&pane_id)
    {
        return false;
    }

    if workspace.layout_kind == LayoutKind::Master {
        if axis != state::SplitAxis::Horizontal {
            return false;
        }
        let available = master_available_width(tile_bounds, tile_gap);
        if resize_master_split_by_pixels(workspace, pane_id, pixels, available) {
            ctx.state.animation = GeometryAnimation::None;
            return true;
        }
        return false;
    }
    if !layout_has_resizable_splits(workspace.layout_kind) {
        return false;
    }

    ensure_tile_tree(workspace);
    if resize_tiled_split_for_edge(
        workspace,
        tile_bounds,
        tile_gap,
        pane_id,
        axis,
        SplitEdge::Trailing,
        pixels,
    ) {
        ctx.state.animation = GeometryAnimation::None;
        return true;
    }
    false
}

pub(crate) fn resize_split_junction_by_drag(
    ctx: &mut Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    let requested = SplitDragKind::Junction {
        horizontal_panes,
        vertical_panes,
    };
    let Some(kind) = resolve_split_drag(ctx, requested, from_x, from_y) else {
        return Update::none();
    };
    apply_split_drag(ctx, kind, from_x, from_y, x, y)
}

fn distinct_split_representatives(
    workspace: &Workspace,
    panes: &[PaneId],
    axis: state::SplitAxis,
) -> Vec<PaneId> {
    let Some(tree) = workspace.tile_tree.as_ref() else {
        return Vec::new();
    };
    let mut paths = std::collections::HashSet::new();
    panes
        .iter()
        .copied()
        .filter(|pane_id| {
            axis_split_path_for_edge(tree, *pane_id, axis, SplitEdge::Trailing)
                .is_some_and(|path| paths.insert(path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::resize_move::test_util::{
        TEST_VIEWPORT, assert_ratio_close, balanced_grid_ratios, balanced_grid_tree, divider_cell,
        first_pane_extent, in_test_stack, root_ratio, steps, two_pane_backend,
    };
    use crate::state::{Pane, SplitAxis, TileGap, Workspace};
    use crate::tiling::{DwindleTree, nearest_split_available, resize_tiled_split};
    use crate::{HyprmuxApp, Msg};
    use tui_lipan::TestBackend;

    #[test]
    fn absolute_split_drag_preserves_clamp_overshoot_until_cursor_returns_to_handle() {
        let start_tree = DwindleTree::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(DwindleTree::Leaf(1)),
            second: Box::new(DwindleTree::Leaf(2)),
        };
        let workspace_for_delta = |pixels: f32| {
            let mut workspace = Workspace::new(0);
            workspace
                .panes
                .push(Pane::new(1, 100, FloatRect::default()));
            workspace
                .panes
                .push(Pane::new(2, 100, FloatRect::default()));
            workspace.tile_tree = Some(start_tree.clone());
            resize_tiled_split(
                &mut workspace,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 40.0,
                },
                TileGap {
                    horizontal: 0.0,
                    vertical: 0.0,
                },
                1,
                SplitAxis::Horizontal,
                pixels,
            );
            workspace
        };

        assert_eq!(root_ratio(&workspace_for_delta(60.0)), 0.8);
        assert_eq!(root_ratio(&workspace_for_delta(50.0)), 0.8);
        assert_eq!(root_ratio(&workspace_for_delta(20.0)), 0.7);
    }

    /// The junction targets are fixed when the drag starts, and each successive event re-applies
    /// the pointer's absolute offset from the origin. Representatives that resolve to the same
    /// split (panes 1 and 2 both sit left of the root divider) must move it once, not twice.
    #[test]
    fn four_pane_junction_tracks_absolute_pointer_once_per_split() {
        in_test_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(TEST_VIEWPORT);
            let (root_available, left_available) = {
                let state = backend.state_mut();
                let workspace = state.active_workspace_mut();
                workspace.panes.clear();
                for id in 1..=4 {
                    workspace
                        .panes
                        .push(Pane::new(id, 100, FloatRect::default()));
                }
                workspace.tile_tree = Some(balanced_grid_tree());

                let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
                let tile_bounds = workspace_tile_bounds(bounds, state.workspace_top_gap());
                let tree = state.current().workspaces[state.current().active_workspace]
                    .tile_tree
                    .as_ref()
                    .unwrap();
                (
                    nearest_split_available(
                        tree,
                        tile_bounds,
                        TileGap::DEFAULT,
                        1,
                        SplitAxis::Horizontal,
                    )
                    .unwrap(),
                    nearest_split_available(
                        tree,
                        tile_bounds,
                        TileGap::DEFAULT,
                        1,
                        SplitAxis::Vertical,
                    )
                    .unwrap(),
                )
            };
            backend.render();
            backend
                .dispatch(Msg::BeginResizeSplitJunction(vec![1, 2], vec![1], 50, 15))
                .expect("begin junction drag");
            backend
                .dispatch(Msg::ResizeSplitJunction(
                    vec![1, 2],
                    vec![1, 3],
                    50,
                    15,
                    60,
                    18,
                ))
                .expect("resize junction");

            let ratios = balanced_grid_ratios(
                backend.state_mut().current_mut().workspaces[0]
                    .tile_tree
                    .as_ref()
                    .unwrap(),
            );
            // Each divider commits on a whole cell, so the check is where it renders, not what
            // fraction it stores: the pointer moved 10 columns and 3 rows, so each divider moved
            // that many cells from where it started.
            let root_start = divider_cell(0.5, root_available);
            let left_start = divider_cell(0.5, left_available);
            assert_eq!(divider_cell(ratios.0, root_available), root_start + 10.0);
            assert_eq!(divider_cell(ratios.1, left_available), left_start + 3.0);
            assert_ratio_close(ratios.2, 0.5);

            backend
                .dispatch(Msg::ResizeSplitJunction(
                    vec![1, 2],
                    vec![1, 3],
                    50,
                    15,
                    61,
                    19,
                ))
                .expect("continue junction drag");
            let ratios = balanced_grid_ratios(
                backend.state_mut().current_mut().workspaces[0]
                    .tile_tree
                    .as_ref()
                    .unwrap(),
            );
            assert_eq!(divider_cell(ratios.0, root_available), root_start + 11.0);
            assert_eq!(divider_cell(ratios.1, left_available), left_start + 4.0);
            assert_ratio_close(ratios.2, 0.5);
        });
    }

    /// Drags driven by real pointer events, so they exercise how the framework routes a gesture
    /// after the first move: by the position the grabbed mouse region held when the pointer went
    /// down. A resize strip moves with the split it describes, so mid-drag events arrive from a
    /// neighbouring strip's region - the drag session, not the event payload, has to decide which
    /// boundary moves.
    mod pointer_drags {
        use super::*;
        use tui_lipan::core::event::{KeyMods, MouseButton, MouseEvent, MouseKind};

        fn axis(vertical_divider: bool) -> SplitAxis {
            if vertical_divider {
                SplitAxis::Horizontal
            } else {
                SplitAxis::Vertical
            }
        }

        fn leaf(id: PaneId) -> Box<DwindleTree> {
            Box::new(DwindleTree::Leaf(id))
        }

        fn split(
            axis: SplitAxis,
            first: Box<DwindleTree>,
            second: Box<DwindleTree>,
        ) -> Box<DwindleTree> {
            Box::new(DwindleTree::Split {
                axis,
                ratio: 0.5,
                first,
                second,
            })
        }

        fn leaf_ids(tree: &DwindleTree, out: &mut Vec<PaneId>) {
            match tree {
                DwindleTree::Leaf(id) => out.push(*id),
                DwindleTree::Split { first, second, .. } => {
                    leaf_ids(first, out);
                    leaf_ids(second, out);
                }
            }
        }

        /// Every split ratio in leaf order, keyed by its tree path (`false` = first child).
        fn ratios(backend: &mut TestBackend<HyprmuxApp>) -> Vec<(Vec<bool>, f32)> {
            fn visit(tree: &DwindleTree, path: &mut Vec<bool>, out: &mut Vec<(Vec<bool>, f32)>) {
                let DwindleTree::Split {
                    ratio,
                    first,
                    second,
                    ..
                } = tree
                else {
                    return;
                };
                out.push((path.clone(), *ratio));
                path.push(false);
                visit(first, path, out);
                path.pop();
                path.push(true);
                visit(second, path, out);
                path.pop();
            }
            let mut out = Vec::new();
            visit(
                backend
                    .state_mut()
                    .active_workspace_mut()
                    .tile_tree
                    .as_ref()
                    .expect("tile tree"),
                &mut Vec::new(),
                &mut out,
            );
            out
        }

        fn backend_with(tree: &DwindleTree) -> TestBackend<HyprmuxApp> {
            let mut ids = Vec::new();
            leaf_ids(tree, &mut ids);
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(TEST_VIEWPORT);
            {
                let workspace = backend.state_mut().active_workspace_mut();
                workspace.panes.clear();
                for id in ids {
                    workspace
                        .panes
                        .push(Pane::new(id, 100, FloatRect::default()));
                }
                workspace.tile_tree = Some(tree.clone());
            }
            backend.render();
            backend
        }

        fn mouse(x: u16, y: u16, kind: MouseKind) -> MouseEvent {
            MouseEvent {
                x,
                y,
                kind,
                mods: KeyMods::NONE,
            }
        }

        /// Press at `(x, y)`, move to the target in several steps (each one re-renders the shifted
        /// layout), then release.
        fn drag(backend: &mut TestBackend<HyprmuxApp>, x: u16, y: u16, dx: i32, dy: i32) {
            let at = |step: i32, total: i32| {
                (
                    (x as i32 + dx * step / total).max(0) as u16,
                    (y as i32 + dy * step / total).max(0) as u16,
                )
            };
            backend
                .send_mouse(mouse(x, y, MouseKind::Down(MouseButton::Left)))
                .expect("press");
            for step in 1..=4 {
                let (nx, ny) = at(step, 4);
                backend
                    .send_mouse(mouse(nx, ny, MouseKind::Drag(MouseButton::Left)))
                    .expect("drag");
            }
            let (nx, ny) = at(1, 1);
            backend
                .send_mouse(mouse(nx, ny, MouseKind::Up(MouseButton::Left)))
                .expect("release");
        }

        /// A pane's extent along one axis, as `(pane, start, end)` in canvas coordinates.
        type Span = (PaneId, f32, f32);

        /// Pane spans along `axis`, as `(pane, start, end)` in canvas coordinates. Ratios are
        /// proportional, so only absolute spans show whether a drag moved a divider it should not
        /// have touched.
        fn spans(backend: &mut TestBackend<HyprmuxApp>, axis: SplitAxis) -> Vec<Span> {
            let state = backend.state_mut();
            let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
            let top_gap = state.workspace_top_gap();
            let gap = state.tile_gap();
            let index = state.current().active_workspace;
            let mut spans: Vec<Span> = crate::layout::workspace_target_rects(
                &state.current().workspaces[index],
                bounds,
                top_gap,
                gap,
            )
            .iter()
            .map(|placement| match axis {
                SplitAxis::Horizontal => (
                    placement.id,
                    placement.rect.x,
                    placement.rect.x + placement.rect.w,
                ),
                SplitAxis::Vertical => (
                    placement.id,
                    placement.rect.y,
                    placement.rect.y + placement.rect.h,
                ),
            })
            .collect();
            spans.sort_by_key(|(id, ..)| *id);
            spans
        }

        /// Drag one divider and report the pane spans along `axis` before and after.
        fn drag_divider(
            tree: &DwindleTree,
            (x, y): (u16, u16),
            (dx, dy): (i32, i32),
            axis: SplitAxis,
        ) -> (Vec<Span>, Vec<Span>) {
            let mut backend = backend_with(tree);
            let before = spans(&mut backend, axis);
            drag(&mut backend, x, y, dx, dy);
            (before, spans(&mut backend, axis))
        }

        /// Root-space row a stacked boundary is grabbed on. With `titlebar = "bar"` - the default -
        /// the handle is the *lower* pane's own titlebar row, not the upper pane's bottom border.
        fn grab_row(tree: &DwindleTree, lower: PaneId) -> u16 {
            let mut backend = backend_with(tree);
            pane_leading_row(&mut backend, lower)
        }

        /// As `grab_row`, for a backend that is already set up.
        fn pane_leading_row(backend: &mut TestBackend<HyprmuxApp>, pane: PaneId) -> u16 {
            let top = backend.state_mut().content_top_offset();
            let state = backend.state_mut();
            let bounds = state.canvas_bounds_from_terminal_viewport(TEST_VIEWPORT);
            let top_gap = state.workspace_top_gap();
            let gap = state.tile_gap();
            let index = state.current().active_workspace;
            let y = crate::layout::workspace_target_rects(
                &state.current().workspaces[index],
                bounds,
                top_gap,
                gap,
            )
            .iter()
            .find(|placement| placement.id == pane)
            .expect("pane placement")
            .rect
            .y;
            y.round() as u16 + top
        }

        /// Panes whose span along the drag axis changed, and by how much at each edge.
        fn shifted(before: &[Span], after: &[Span]) -> Vec<Span> {
            before
                .iter()
                .zip(after.iter())
                .map(|(a, b)| (a.0, b.1 - a.1, b.2 - a.2))
                .filter(|(_, start, end)| *start != 0.0 || *end != 0.0)
                .collect()
        }

        /// Two stacked pairs side by side. Dragging the divider inside one column must leave the
        /// other column's divider and the main divider alone, and must follow the pointer for the
        /// whole gesture rather than stalling when the strip shifts underneath it.
        #[test]
        fn a_column_divider_ignores_the_other_column() {
            in_test_stack(|| {
                let tree = *split(
                    SplitAxis::Horizontal,
                    split(SplitAxis::Vertical, leaf(1), leaf(2)),
                    split(SplitAxis::Vertical, leaf(3), leaf(4)),
                );
                let (before, after) =
                    drag_divider(&tree, (70, grab_row(&tree, 4)), (0, 4), SplitAxis::Vertical);
                assert_eq!(shifted(&before, &after), vec![(3, 0.0, 4.0), (4, 4.0, 0.0)]);

                let (before, after) = drag_divider(
                    &tree,
                    (20, grab_row(&tree, 2)),
                    (0, -4),
                    SplitAxis::Vertical,
                );
                assert_eq!(
                    shifted(&before, &after),
                    vec![(1, 0.0, -4.0), (2, -4.0, 0.0)]
                );
            });
        }

        /// A nested pair beside a single pane: dragging the divider between the two areas must
        /// move only that divider. The nested divider sits inside the area that grows, so without
        /// compensation its proportional ratio would carry it along.
        #[test]
        fn the_main_divider_leaves_a_nested_divider_where_it_is() {
            in_test_stack(|| {
                for vertical_divider in [true, false] {
                    let nested = axis(!vertical_divider);
                    let tree = *split(
                        axis(vertical_divider),
                        split(nested, leaf(1), leaf(2)),
                        leaf(3),
                    );
                    let (grab, delta, moved) = if vertical_divider {
                        ((50, 5), (6, 0), 6.0)
                    } else {
                        ((20, grab_row(&tree, 3)), (0, 4), 4.0)
                    };
                    let (before, after) = drag_divider(&tree, grab, delta, axis(vertical_divider));
                    // Panes 1 and 2 straddle the *nested* divider, which is perpendicular here, so
                    // both grow by the whole amount; pane 3 gives the room up.
                    assert_eq!(
                        shifted(&before, &after),
                        vec![(1, 0.0, moved), (2, 0.0, moved), (3, moved, 0.0)]
                    );
                }
            });
        }

        /// Three panes in a row, `|1|2| 3 |`. The two dividers lie on the same axis, so only the
        /// pane edge the strip was built from tells them apart - and moving the outer one must not
        /// drag the inner one along with it.
        #[test]
        fn same_axis_dividers_stay_independent() {
            in_test_stack(|| {
                for vertical_dividers in [true, false] {
                    let axis = axis(vertical_dividers);
                    let tree = *split(axis, split(axis, leaf(1), leaf(2)), leaf(3));
                    let (inner_grab, main_grab, delta, moved) = if vertical_dividers {
                        ((25, 10), (50, 10), (6, 0), 6.0)
                    } else {
                        (
                            (50, grab_row(&tree, 2)),
                            (50, grab_row(&tree, 3)),
                            (0, 3),
                            3.0,
                        )
                    };

                    let (before, after) = drag_divider(&tree, inner_grab, delta, axis);
                    assert_eq!(
                        shifted(&before, &after),
                        vec![(1, 0.0, moved), (2, moved, 0.0)]
                    );

                    // Only pane 2 - the one against the grabbed divider - takes up the change.
                    let (before, after) = drag_divider(&tree, main_grab, delta, axis);
                    assert_eq!(
                        shifted(&before, &after),
                        vec![(2, 0.0, moved), (3, moved, 0.0)]
                    );
                }
            });
        }

        /// Dragging a divider one cell at a time must move it one cell at a time, on both axes.
        ///
        /// Two things used to break this. The divider was stored as a bare ratio and rendered by
        /// rounding `usable * ratio`, so on the 99-column axis every whole-cell offset sat on a
        /// half-cell tie and f32 noise decided it - a run of single-cell pulls came out
        /// 0,0,+2,+2,+1,+1. And the framework's default drag threshold is 3 columns against 1
        /// row, so the first two columns of a left/right drag were swallowed and the third
        /// arrived already three cells out.
        #[test]
        fn a_divider_drag_tracks_the_pointer_cell_for_cell() {
            in_test_stack(|| {
                for vertical_divider in [true, false] {
                    let axis = axis(vertical_divider);
                    let mut backend = two_pane_backend(axis);
                    let start = first_pane_extent(&mut backend, axis);
                    // The divider sits just past pane 1; a horizontal one is below the workbar.
                    let (grab_x, grab_y) = if vertical_divider {
                        (start.round() as u16, 10)
                    } else {
                        (50, pane_leading_row(&mut backend, 2))
                    };

                    backend
                        .send_mouse(mouse(grab_x, grab_y, MouseKind::Down(MouseButton::Left)))
                        .expect("press");
                    let mut extents = vec![start];
                    for step in 1..=5u16 {
                        let (x, y) = if vertical_divider {
                            (grab_x + step, grab_y)
                        } else {
                            (grab_x, grab_y + step)
                        };
                        backend
                            .send_mouse(mouse(x, y, MouseKind::Drag(MouseButton::Left)))
                            .expect("drag");
                        extents.push(first_pane_extent(&mut backend, axis));
                    }

                    assert_eq!(
                        steps(&extents),
                        vec![1.0; 5],
                        "{axis:?}: extents {extents:?} should follow the pointer one cell per cell"
                    );
                }
            });
        }

        /// A junction moves exactly one divider per axis - the segment under the drag origin -
        /// and keeps that pair for the whole gesture.
        #[test]
        fn a_junction_drag_moves_one_divider_per_axis() {
            in_test_stack(|| {
                let tree = balanced_grid_tree();
                let row = grab_row(&tree, 2);
                let mut backend = backend_with(&tree);
                drag(&mut backend, 50, row, 6, 4);

                let after = ratios(&mut backend);
                let ratio = |path: &[bool]| {
                    after
                        .iter()
                        .find(|(other, _)| other == path)
                        .expect("split")
                        .1
                };
                assert_eq!(
                    divider_cell(ratio(&[]), 99.0),
                    divider_cell(0.5, 99.0) + 6.0
                );
                assert_eq!(
                    divider_cell(ratio(&[false]), 28.0),
                    divider_cell(0.5, 28.0) + 4.0
                );
                assert_ratio_close(ratio(&[true]), 0.5);
            });
        }
    }
}
