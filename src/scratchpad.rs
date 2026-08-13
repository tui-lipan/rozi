use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::anim::GeometryAnimation;
use crate::config::{SCRATCHPAD_MAX_HEIGHT, SCRATCHPAD_MIN_HEIGHT};
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::{request_current_pane_focus, request_pane_focus};
use crate::pane_lifecycle::spawn_pane_in_scratch;
use crate::state::PaneIdentity;
use crate::view;

/// Below this much progress the dropdown is treated as fully retracted and not rendered.
const SCRATCH_ANIM_EPSILON: f32 = 0.01;

/// Bottom-anchored dropdown rect when fully deployed: full tile width, `height_fraction` of
/// the tile height, flush with the bottom tile edge. The pane slides up from below to reach it.
pub(crate) fn scratch_rect(bounds: FloatRect, height_fraction: f32, top_gap: f32) -> FloatRect {
    let tile_bounds = workspace_tile_bounds(bounds, top_gap);
    let fraction = height_fraction.clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
    let h = (tile_bounds.h * fraction).round().max(1.0);
    FloatRect {
        x: tile_bounds.x,
        y: tile_bounds.y + tile_bounds.h - h,
        w: tile_bounds.w,
        h,
    }
}

/// How far down the whole dropdown is translated for the slide, in rows. `0.0` when fully deployed;
/// at `progress == 0.0` it is pushed far enough that its top edge sits on the bottom tile edge and
/// the dropdown is entirely off-screen.
///
/// The slide is a rigid translation applied *after* each pane's own geometry transition, never a
/// moving transition target. A transition chasing an animating target settles on its own, longer
/// curve instead of the slide's, which is what made the dropdown crawl on the way in and get
/// unmounted mid-motion on the way out.
pub(crate) fn slide_offset(
    bounds: FloatRect,
    height_fraction: f32,
    progress: f32,
    top_gap: f32,
) -> f32 {
    let shown = scratch_rect(bounds, height_fraction, top_gap);
    let tile_bounds = workspace_tile_bounds(bounds, top_gap);
    let hidden_y = tile_bounds.y + tile_bounds.h;
    (hidden_y - shown.y) * (1.0 - progress.clamp(0.0, 1.0))
}

/// The deployed dropdown rect for the current viewport: the box the scratch workspace tiles inside.
pub(crate) fn deployed_rect(state: &crate::state::State, viewport: Rect) -> FloatRect {
    scratch_rect(
        state.canvas_bounds_from_terminal_viewport(viewport),
        scratch_height_fraction(state),
        state.workspace_top_gap(),
    )
}

/// Slide progress for the dropdown: `1.0` fully deployed, `0.0` hidden below the bottom edge.
/// Sampled every frame from `render` (even while closed) so the keyed transition is seeded at
/// `0.0` from startup - that way the very first open still slides up instead of snapping in.
pub(crate) fn scratch_progress(app: &AppRoot, ctx: &Context<AppRoot>) -> f32 {
    let target = if ctx.state.scratch_visible && !ctx.state.scratch.panes.is_empty() {
        1.0
    } else {
        0.0
    };
    ctx.transition::<f32>(
        "rozi-scratch-progress",
        target,
        app.scratch_transition_config(ctx),
    )
}

/// Toggle the scratchpad in/out of view. The first show spawns its shell; later shows reuse
/// the same live PTY. Hiding keeps the PTY alive and restores the previously focused pane.
pub(crate) fn toggle(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.commands_dirty = true;
    // Both layers share one pointer session. Settle whatever is in flight before the active
    // workspace changes underneath it, or a drag started in one would land in the other.
    crate::ops::resize_move::finish_pointer_layout_interaction(ctx);
    if ctx.state.scratch_visible {
        ctx.state.scratch_visible = false;
        ctx.state.animation = GeometryAnimation::TileFloat;
        if let Some(prev) = ctx.state.scratch_return_focus.take() {
            crate::ops::focus::focus_pane(&mut ctx.state, prev);
            request_pane_focus(ctx, prev);
        } else {
            request_current_pane_focus(ctx);
        }
        return Update::full();
    }

    ctx.state.scratch_return_focus = ctx.state.current().focused_pane;
    ctx.state.scratch_visible = true;
    ctx.state.animation = GeometryAnimation::TileFloat;

    if ctx.state.scratch.panes.is_empty() {
        if let Some(update) = crate::ops::session::ensure_session_for_pty(
            ctx,
            crate::state::PendingSessionAction::ToggleScratchpad,
        ) {
            // Not visible yet — the deferred open will show it after attach.
            ctx.state.scratch_visible = false;
            ctx.state.scratch_return_focus = None;
            return update;
        }
        ctx.state.scratch.layout_kind = ctx.state.config.layout.default;
        let identity = PaneIdentity {
            command: ctx.state.config.scratchpad.command.clone(),
            cwd: ctx.state.config.scratchpad.cwd.clone(),
            ..PaneIdentity::default()
        };
        return spawn_pane_in_scratch(ctx, None, identity).1;
    }

    // Focus only after the pane exists in state: `request_pane_focus` looks the pane up and no-ops
    // when it is missing, so requesting before the first-open insert (as this used to) silently
    // dropped focus on the initial toggle. `Context::request_focus` records the target key and the
    // renderer applies it once the scratch terminal node mounts, so this is safe on a fresh spawn.
    if let Some(id) = ctx.state.scratch.focused_pane {
        request_pane_focus(ctx, id);
    }
    Update::full()
}

/// The scratchpad height as a fraction of the tile height: the drag-adjusted runtime override
/// when present, otherwise the configured default.
pub(crate) fn scratch_height_fraction(state: &crate::state::State) -> f32 {
    state
        .scratch_height
        .unwrap_or(state.config.scratchpad.height)
}

pub(crate) fn contains(state: &crate::state::State, id: crate::state::PaneId) -> bool {
    state.scratch.panes.iter().any(|pane| pane.id == id)
}

pub(crate) fn after_pane_removed(ctx: &mut Context<AppRoot>) {
    if ctx.state.scratch.panes.iter().all(|pane| pane.closing) {
        ctx.state.scratch_visible = false;
        ctx.state.scratch.focused_pane = None;
        ctx.state.commands_dirty = true;
        if let Some(prev) = ctx.state.scratch_return_focus.take() {
            crate::ops::focus::focus_pane(&mut ctx.state, prev);
            request_pane_focus(ctx, prev);
        } else {
            request_current_pane_focus(ctx);
        }
    }
}

/// Scratchpads are current-view overlays rather than attachment state. Tear the server pane down
/// before switching so no local scratch PTY remains addressed to the old session client.
pub(crate) fn close_for_session_switch(ctx: &mut Context<AppRoot>) {
    if let Some(client) = ctx.state.current().session_client.as_ref() {
        for pane in &ctx.state.scratch.panes {
            client.kill(pane.id, pane.pty_generation, true);
        }
    }
    ctx.state.scratch = crate::state::Workspace::new(0);
    ctx.state.scratch_visible = false;
    ctx.state.scratch_return_focus = None;
    ctx.state.scratch_resize_start = None;
    ctx.state.moving_pane = None;
    ctx.state.resizing_pane = None;
    ctx.state.split_drag = None;
}

/// Grab the scratchpad's top edge: remember the current height fraction so the drag recomputes
/// from this origin rather than accumulating per-move deltas.
pub(crate) fn begin_resize(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.scratch_resize_start = Some(scratch_height_fraction(&ctx.state));
    // A pointer resize snaps, like every other one. `toggle` arms `TileFloat` for the slide, and
    // that policy would otherwise still be live here: each drag event would move the panes' target
    // and they would ease toward it over the geometry duration, so the dropdown would trail the
    // pointer instead of tracking it.
    ctx.state.animation = GeometryAnimation::None;
    Update::none()
}

/// Drag the scratchpad's top edge to a new height. `from_y`/`y` are root-space rows; dragging up
/// (smaller `y`) grows the bottom-anchored dropdown. The new height is stored as a runtime
/// override and clamped to the configured bounds.
pub(crate) fn resize(ctx: &mut Context<AppRoot>, from_y: u16, y: u16) -> Update {
    let start = match ctx.state.scratch_resize_start {
        Some(start) => start,
        None => scratch_height_fraction(&ctx.state),
    };
    let viewport = ctx.viewport();
    // Also per event, not just at drag start: a drag that began before the slide finished would
    // otherwise re-arm nothing and keep whatever policy the toggle left behind.
    ctx.state.animation = GeometryAnimation::None;
    set_height_from(
        &mut ctx.state,
        start,
        f32::from(from_y) - f32::from(y),
        viewport,
    );
    Update::full()
}

pub(crate) fn end_resize(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.scratch_resize_start = None;
    Update::none()
}

/// The row extent the height fraction is measured against: the whole tile area, not the dropdown.
fn height_extent(state: &crate::state::State, viewport: Rect) -> f32 {
    workspace_tile_bounds(
        state.canvas_bounds_from_terminal_viewport(viewport),
        state.workspace_top_gap(),
    )
    .h
}

/// Set the dropdown's height to `start` grown by `rows` (positive grows it upward), clamped to the
/// configured bounds. Every height gesture - the top-edge strip, resize mode, and a right-drag on
/// a pane against the top edge - recomputes from its own origin through here, so none of them
/// accumulate rounding per pointer event or drift past a clamp and back.
///
/// Returns whether the height actually changed, so a caller can fall through to its own resize
/// when the dropdown is already at a clamp.
pub(crate) fn set_height_from(
    state: &mut crate::state::State,
    start: f32,
    rows: f32,
    viewport: Rect,
) -> bool {
    let tile_h = height_extent(state, viewport);
    if tile_h <= 0.0 {
        return false;
    }
    let fraction =
        ((start * tile_h + rows) / tile_h).clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
    if (fraction - scratch_height_fraction(state)).abs() < f32::EPSILON {
        return false;
    }
    state.scratch_height = Some(fraction);
    true
}

/// Move the dropdown's top edge by whole rows from where it currently sits.
pub(crate) fn resize_by_rows(ctx: &mut Context<AppRoot>, rows: f32) -> bool {
    let start = scratch_height_fraction(&ctx.state);
    let viewport = ctx.viewport();
    set_height_from(&mut ctx.state, start, rows, viewport)
}

/// One keyboard resize step for the dropdown's own height, in rows.
pub(crate) fn height_step_rows(state: &crate::state::State, viewport: Rect) -> f32 {
    crate::ops::resize_move::keyboard_step_cells(height_extent(state, viewport))
}

/// Whether `id` sits against the dropdown's top edge, which is the border the scratchpad's own
/// height resize moves. Panes further down have an ordinary split above them instead.
pub(crate) fn pane_touches_top_edge(state: &crate::state::State, id: crate::state::PaneId) -> bool {
    let Some(viewport) = state.last_viewport.get() else {
        return false;
    };
    let deployed = deployed_rect(state, viewport);
    let placements =
        crate::layout::workspace_target_rects(&state.scratch, deployed, 0.0, state.tile_gap());
    crate::layout::placement_for(&placements, id)
        .is_some_and(|rect| (rect.y - deployed.y).abs() < 1.5)
}

/// A dimming scrim covering the whole canvas, drawn behind the dropdown so the scratchpad
/// reads as the focused layer. Clicking it dismisses the scratchpad. Returns `None` when the
/// scratchpad is hidden.
pub(crate) fn scratch_backdrop(
    ctx: &Context<AppRoot>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress <= SCRATCH_ANIM_EPSILON {
        return None;
    }
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    // A transparent full-canvas catcher: it swallows clicks meant for the dimmed panes and
    // dismisses the scratchpad when clicked. It paints nothing - an opaque scrim would occlude
    // the panes' text and borders, so the "focused layer" cue is the workspace layer dimming
    // instead (see `backdrop_dim`, applied in `view::render`).
    let region: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(
            ctx.link()
                .callback(|_| crate::Msg::RunAction(crate::input::Action::ToggleScratchpad)),
        )
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into();
    Some((bounds, region.key("rozi-scratch-scrim")))
}

/// A transparent catcher covering the dropdown itself, between the dismiss scrim and the panes.
///
/// The dropdown is a workspace, so the space *inside* it - tile gaps, divider rows, the strip
/// beside a floating pane - belongs to the workspace, not to the dismiss scrim underneath. Without
/// this, every click that missed a pane by a cell (grabbing a split boundary, for one) fell through
/// to the scrim and closed the scratchpad. Clicks outside the dropdown still dismiss it.
pub(crate) fn scratch_shield(
    ctx: &Context<AppRoot>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress <= SCRATCH_ANIM_EPSILON || ctx.state.scratch.panes.is_empty() {
        return None;
    }
    let deployed = deployed_rect(&ctx.state, ctx.viewport());
    let offset = slide_offset(
        ctx.state
            .canvas_bounds_from_terminal_viewport(ctx.viewport()),
        scratch_height_fraction(&ctx.state),
        progress,
        ctx.state.workspace_top_gap(),
    );
    let rect = FloatRect {
        y: deployed.y + offset,
        ..deployed
    };
    // A capture region only reroutes clicks when it carries a left-button callback, so the
    // no-op has to be a real one: re-focusing the pane that already has focus consumes the
    // press and changes nothing.
    let focused = ctx.state.scratch.focused_pane.unwrap_or_default();
    let region: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(ctx.link().callback(move |_| crate::Msg::FocusPane(focused)))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into();
    Some((rect, region.key("rozi-scratch-shield")))
}

/// Opacity multiplier for a layer sitting under a focused layer (the scratchpad, or a modal
/// dialog). Dimming toward the backdrop - rather than overlaying an opaque layer - signals
/// the focused layer while keeping the dimmed content legible, and tracks the animation
/// `progress` so the dim eases in and out with it. `progress` is `0.0` (no dim) to `1.0`
/// (fully deployed/open).
pub(crate) fn backdrop_dim(progress: f32) -> f32 {
    1.0 - 0.5 * progress.clamp(0.0, 1.0)
}

/// The placement (in canvas coordinates) and element for the scratchpad, rendered above the
/// workspace layer. `progress` drives the slide; the pane stays mounted while it animates
/// back down on hide, and is dropped once fully retracted.
pub(crate) fn scratch_panes(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    canvas: Canvas,
    progress: f32,
) -> Canvas {
    if (progress <= SCRATCH_ANIM_EPSILON && !ctx.state.scratch_visible)
        || ctx.state.scratch.panes.is_empty()
    {
        return canvas;
    }
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let height_fraction = scratch_height_fraction(&ctx.state);
    let deployed = scratch_rect(bounds, height_fraction, top_gap);
    let offset_y = slide_offset(bounds, height_fraction, progress, top_gap);
    view::render_workspace_panes(
        app,
        ctx,
        canvas,
        &view::WorkspaceLayer {
            workspace: &ctx.state.scratch,
            bounds: deployed,
            visible_bounds: None,
            // `deployed` already sits inside the tile area, so insetting again would double the
            // workbar gap.
            top_gap: 0.0,
            // A fullscreen scratch pane fills the dropdown, not the terminal: it is still a layer
            // above the workspace, and covering the whole screen would make the two layers
            // indistinguishable.
            fullscreen_bounds: view::canvas_rect_to_root(deployed, ctx.state.content_top_offset()),
            // Scratch floats are already canvas-absolute; see `WorkspaceLayer::float_origin`.
            float_origin: (0.0, 0.0),
            offset_y,
            scratch: true,
            viewport_changed: false,
        },
    )
}

/// A thin drag handle sitting over the scratchpad's top chrome (title row and top border) that
/// resizes its height, mirroring the tiled split-drag strips. Only shown once fully deployed so
/// it never floats detached from the sliding pane. Returns its placement in canvas coordinates.
pub(crate) fn scratch_resize_strip(
    ctx: &Context<AppRoot>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress < 1.0 - SCRATCH_ANIM_EPSILON || !ctx.state.scratch_visible {
        return None;
    }
    (!ctx.state.scratch.panes.is_empty()).then_some(())?;
    let deployed = deployed_rect(&ctx.state, ctx.viewport());
    // A separate title bar and a retained frame each contribute a top chrome row. Borderless
    // frames still reserve one row for compact headers, so every other combination needs one.
    // Covering the top panes' titlebar costs nothing: a pane is moved by modifier-dragging it
    // anywhere, not by its titlebar specifically.
    let special_frame = ctx.state.config.pane.border_mode.draws_frames()
        || ctx.state.config.pane.keep_special_borders;
    let strip_h: f32 = if special_frame
        && ctx.state.config.pane.show_titles
        && ctx.state.config.pane.titlebar.takes_outer_row()
    {
        2.0
    } else {
        1.0
    };
    let strip = FloatRect {
        h: strip_h.min(deployed.h),
        ..deployed
    };
    let focused = ctx.state.scratch.focused_pane.unwrap_or_default();
    // Capture clicks so a plain click on the handle keeps the scratchpad focused instead of
    // falling through to the dismiss-scrim beneath it; drags resize.
    let region: Element = MouseRegion::new()
        .capture_click(true)
        // A handle with no click gesture to disambiguate from: track the pointer from its first
        // step instead of stalling a row and then jumping.
        .drag_threshold(1, 1)
        .on_mouse_down(ctx.link().callback(move |_| crate::Msg::FocusPane(focused)))
        .on_drag_start(
            ctx.link()
                .callback(|event: MouseDragEvent| crate::Msg::BeginScratchResize(event.from_y)),
        )
        .on_drag(
            ctx.link()
                .callback(|event: MouseDragEvent| crate::Msg::ScratchResize(event.from_y, event.y)),
        )
        .on_drag_end(ctx.link().callback(|_| crate::Msg::EndScratchResize))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into();
    Some((strip, region.key("rozi-scratch-resize-strip")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::OUTER_GAP;

    #[test]
    fn scratch_rect_is_bottom_anchored_full_width() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let rect = scratch_rect(bounds, 0.4, OUTER_GAP);
        let tile_bounds = workspace_tile_bounds(bounds, OUTER_GAP);
        assert_eq!(rect.x, tile_bounds.x);
        assert_eq!(rect.w, tile_bounds.w);
        assert!((rect.h - tile_bounds.h * 0.4).abs() <= 1.0);
        // Flush with the bottom tile edge.
        assert!(((rect.y + rect.h) - (tile_bounds.y + tile_bounds.h)).abs() <= 1.0);
    }

    #[test]
    fn scratch_slide_starts_below_and_ends_deployed() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let shown = scratch_rect(bounds, 0.4, OUTER_GAP);
        // Fully deployed is the untranslated rect; hidden pushes it entirely below the tile area.
        assert_eq!(slide_offset(bounds, 0.4, 1.0, OUTER_GAP), 0.0);
        let hidden_y = shown.y + slide_offset(bounds, 0.4, 0.0, OUTER_GAP);
        assert!(hidden_y >= shown.y + shown.h - 1.0);
    }

    #[test]
    fn scratch_rect_clamps_extreme_fractions() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let tall = scratch_rect(bounds, 5.0, OUTER_GAP);
        let tile_bounds = workspace_tile_bounds(bounds, OUTER_GAP);
        assert!(tall.h <= tile_bounds.h * SCRATCHPAD_MAX_HEIGHT + 1.0);
    }

    const VIEWPORT: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 40,
    };

    fn state_with_scratch(panes: &[crate::state::PaneId]) -> crate::state::State {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        state.last_viewport.set(Some(VIEWPORT));
        for id in panes {
            state
                .scratch
                .panes
                .push(crate::state::Pane::new(*id, 100, FloatRect::default()));
            crate::tiling::append_tiled_window(&mut state.scratch, *id);
        }
        state.scratch.focused_pane = panes.last().copied();
        state.scratch_visible = true;
        state
    }

    /// Every layout computation reads its box from `layout_bounds`, so this is what makes the
    /// scratch workspace tile inside the dropdown instead of across the whole canvas.
    #[test]
    fn a_visible_scratchpad_is_the_active_layout_box() {
        let state = state_with_scratch(&[1]);
        assert_eq!(
            state.layout_bounds(VIEWPORT),
            deployed_rect(&state, VIEWPORT)
        );
        assert_eq!(state.layout_top_gap(), 0.0);

        let mut hidden = state;
        hidden.scratch_visible = false;
        assert_eq!(
            hidden.layout_bounds(VIEWPORT),
            hidden.canvas_bounds_from_terminal_viewport(VIEWPORT)
        );
    }

    /// `toggle` arms `TileFloat` so the dropdown slides. A pointer resize has to clear it, or every
    /// drag event would move the panes' target and leave them easing toward it over the geometry
    /// duration - the dropdown trailing the pointer in slow motion until some other pointer gesture
    /// (a split drag) reset the policy.
    #[test]
    fn dragging_the_top_edge_snaps_instead_of_easing() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                backend.set_viewport(VIEWPORT);
                {
                    let state = backend.state_mut();
                    state
                        .scratch
                        .panes
                        .push(crate::state::Pane::new(1, 100, FloatRect::default()));
                    crate::tiling::append_tiled_window(&mut state.scratch, 1);
                    state.scratch.focused_pane = Some(1);
                    state.scratch_visible = true;
                    state.animation = GeometryAnimation::TileFloat;
                }
                backend.render();
                let start = scratch_height_fraction(backend.state());

                backend
                    .dispatch(crate::Msg::BeginScratchResize(24))
                    .expect("begin scratch resize");
                assert_eq!(backend.state().animation, GeometryAnimation::None);

                backend
                    .dispatch(crate::Msg::ScratchResize(24, 20))
                    .expect("drag scratch edge");
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!(scratch_height_fraction(backend.state()) > start);
            })
            .expect("spawn scratch resize test thread")
            .join()
            .expect("scratch resize test thread panicked");
    }

    /// A left|right split puts both panes on the dropdown's top border; a stacked one puts only
    /// the upper pane there. Resize mode reads this to decide between moving the dropdown's own
    /// edge and an inner split.
    #[test]
    fn only_panes_against_the_dropdown_top_edge_own_it() {
        let mut state = state_with_scratch(&[1, 2]);
        state.scratch.tile_tree = Some(crate::tiling::DwindleTree::Split {
            axis: crate::state::SplitAxis::Vertical,
            ratio: 0.5,
            first: Box::new(crate::tiling::DwindleTree::Leaf(1)),
            second: Box::new(crate::tiling::DwindleTree::Leaf(2)),
        });
        assert!(pane_touches_top_edge(&state, 1));
        assert!(!pane_touches_top_edge(&state, 2));

        state.scratch.tile_tree = Some(crate::tiling::DwindleTree::Split {
            axis: crate::state::SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(crate::tiling::DwindleTree::Leaf(1)),
            second: Box::new(crate::tiling::DwindleTree::Leaf(2)),
        });
        assert!(pane_touches_top_edge(&state, 1));
        assert!(pane_touches_top_edge(&state, 2));
    }
    /// A closing pane shrinks toward its own centre, exactly as in a workspace.
    ///
    /// The scratch workspace stores its floating rects in plain canvas coordinates, so the layer
    /// must not add its own origin back the way the follower letterbox does. When it did, the
    /// frozen rect landed a dropdown's height below the pane and the close read as a fast slide
    /// off the bottom rather than a shrink in place.
    #[test]
    fn a_closing_scratch_pane_shrinks_where_it_sits() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                backend.set_viewport(VIEWPORT);
                let generation = {
                    let state = backend.state_mut();
                    for id in 1..=2 {
                        let mut pane = crate::state::Pane::new(id, 100, FloatRect::default());
                        pane.opening = false;
                        state.scratch.panes.push(pane);
                        crate::tiling::append_tiled_window(&mut state.scratch, id);
                    }
                    state.scratch.focused_pane = Some(2);
                    state.scratch_visible = true;
                    state.scratch.panes[1].pty_generation
                };
                backend.render();
                // The slide transition reads ~0.0 on the frame the toggle lands on, so the layer
                // must not be gated on it having moved yet.
                assert!(
                    backend
                        .rect_of_key(&view::pane_window_key(2, generation).into())
                        .is_some(),
                    "the scratch layer must mount on the frame the toggle lands on"
                );
                // Let the dropdown finish deploying before measuring the close.
                backend.advance(std::time::Duration::from_millis(400));

                let dropdown = deployed_rect(backend.state(), VIEWPORT);
                let live = backend
                    .rect_of_key(&view::pane_window_key(2, generation).into())
                    .expect("pane 2 is on screen");

                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                    .expect("close the focused scratch pane");
                backend.advance(std::time::Duration::from_millis(400));

                let closing = backend
                    .rect_of_key(&view::pane_window_key(2, generation).into())
                    .expect("a closing pane stays mounted for its exit animation");
                // Inside the dropdown, and shrunk toward where it was - not translated away.
                let top = dropdown.y + f32::from(backend.state().content_top_offset());
                let bottom = f32::from(closing.y) + f32::from(closing.h);
                assert!(
                    f32::from(closing.y) >= top - 1.0 && bottom <= top + dropdown.h + 1.0,
                    "closing {closing:?} left the dropdown at y {top}..{}",
                    top + dropdown.h
                );
                assert!(
                    closing.w < live.w && closing.h <= live.h,
                    "closing {closing:?} should be shrinking from {live:?}"
                );
            })
            .expect("spawn closing pane test thread")
            .join()
            .expect("closing pane test thread panicked");
    }
    /// Closing a pane hands the keyboard to its nearest neighbour, exactly as in a workspace.
    /// The scratch path used to shortcut to `first_visible_pane`, so focus jumped to the top-left
    /// pane no matter which one had just closed.
    #[test]
    fn closing_a_scratch_pane_focuses_the_nearest_neighbour() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                backend.set_viewport(VIEWPORT);
                {
                    let state = backend.state_mut();
                    for id in 1..=3 {
                        let mut pane = crate::state::Pane::new(id, 100, FloatRect::default());
                        pane.opening = false;
                        state.scratch.panes.push(pane);
                    }
                    // 1 | (2 over 3): closing 3 must land on 2, its own split partner, not on 1.
                    state.scratch.tile_tree = Some(crate::tiling::DwindleTree::Split {
                        axis: crate::state::SplitAxis::Horizontal,
                        ratio: 0.5,
                        first: Box::new(crate::tiling::DwindleTree::Leaf(1)),
                        second: Box::new(crate::tiling::DwindleTree::Split {
                            axis: crate::state::SplitAxis::Vertical,
                            ratio: 0.5,
                            first: Box::new(crate::tiling::DwindleTree::Leaf(2)),
                            second: Box::new(crate::tiling::DwindleTree::Leaf(3)),
                        }),
                    });
                    state.scratch.focused_pane = Some(3);
                    state.scratch_visible = true;
                }
                backend.render();

                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                    .expect("close the focused scratch pane");
                assert_eq!(backend.state().scratch.focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().focused_pane,
                    Some(1),
                    "the hidden workspace keeps its own focus"
                );
            })
            .expect("spawn close focus test thread")
            .join()
            .expect("close focus test thread panicked");
    }
}
