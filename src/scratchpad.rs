use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::config::{SCRATCHPAD_MAX_HEIGHT, SCRATCHPAD_MIN_HEIGHT};
use crate::focus_ops::{request_current_pane_focus, request_pane_focus};
use crate::geometry::workspace_tile_bounds;
use crate::pane_lifecycle::{pane_env, request_pane_spawn};
use crate::state::{Pane, SCRATCH_PANE_ID};
use crate::theme_ops::{pane_frame_background, terminal_palette};
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

/// The deployed rect translated straight down so its top edge sits at the bottom tile edge (fully
/// off-screen). At `progress == 0.0` the dropdown is here; at `1.0` it is at `scratch_rect`.
/// Only the `y` position moves - width/height are constant, so the PTY never resizes mid-slide.
fn scratch_slide_rect(
    bounds: FloatRect,
    height_fraction: f32,
    progress: f32,
    top_gap: f32,
) -> FloatRect {
    let shown = scratch_rect(bounds, height_fraction, top_gap);
    let tile_bounds = workspace_tile_bounds(bounds, top_gap);
    let hidden_y = tile_bounds.y + tile_bounds.h;
    let y = hidden_y + (shown.y - hidden_y) * progress.clamp(0.0, 1.0);
    FloatRect { y, ..shown }
}

/// Slide progress for the dropdown: `1.0` fully deployed, `0.0` hidden below the bottom edge.
/// Sampled every frame from `render` (even while closed) so the keyed transition is seeded at
/// `0.0` from startup - that way the very first open still slides up instead of snapping in.
pub(crate) fn scratch_progress(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> f32 {
    let target = if ctx.state.scratch_visible && ctx.state.scratch.is_some() {
        1.0
    } else {
        0.0
    };
    ctx.transition::<f32>(
        "hyprmux-scratch-progress",
        target,
        app.scratch_transition_config(ctx),
    )
}

/// Toggle the scratchpad in/out of view. The first show spawns its shell; later shows reuse
/// the same live PTY. Hiding keeps the PTY alive and restores the previously focused pane.
pub(crate) fn toggle(ctx: &mut Context<HyprmuxApp>) -> Update {
    if ctx.state.scratch_visible {
        ctx.state.scratch_visible = false;
        ctx.state.animation = GeometryAnimation::TileFloat;
        if let Some(prev) = ctx.state.scratch_return_focus.take() {
            crate::focus_ops::focus_pane(&mut ctx.state, prev);
            request_pane_focus(ctx, prev);
        } else {
            request_current_pane_focus(ctx);
        }
        return Update::full();
    }

    ctx.state.scratch_return_focus = ctx.state.focused_pane;
    ctx.state.scratch_visible = true;
    ctx.state.animation = GeometryAnimation::TileFloat;

    if ctx.state.scratch.is_none() {
        let bounds = ctx.state.canvas_bounds(ctx.viewport());
        let top_gap = ctx.state.workspace_top_gap();
        let rect = scratch_rect(bounds, scratch_height_fraction(&ctx.state), top_gap);
        let generation = ctx.state.next_pty_generation;
        ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
        let mut pane = Pane::new(SCRATCH_PANE_ID, ctx.state.config.scrollback, rect);
        pane.pty_generation = generation;
        pane.identity.command = ctx.state.config.scratchpad.command.clone();
        pane.identity.cwd = ctx.state.config.scratchpad.cwd.clone();
        pane.terminal
            .bind_server_backend(SCRATCH_PANE_ID, generation);
        pane.terminal.set_palette(terminal_palette(
            &ctx.state.theme,
            pane_frame_background(
                &ctx.state.theme,
                true,
                ctx.state.config.pane.highlight_focused_background,
            ),
        ));
        // No spawn fade - the dropdown slides in via the rect transition instead, and there
        // is no FinishOpen(scratch) message to clear an `opening` flag.
        pane.opening = false;
        pane.terminal_active = true;
        let env = pane_env(ctx.state.control_socket_path.as_deref(), &pane);
        let command = pane.identity.command.clone();
        let cwd = pane.identity.cwd.clone();
        let cols = pane.terminal.cols;
        let rows = pane.terminal.rows;
        ctx.state.scratch = Some(pane);
        request_pane_spawn(
            &mut ctx.state,
            SCRATCH_PANE_ID,
            generation,
            command,
            cwd,
            cols,
            rows,
            false,
            env,
            None,
        );
    }

    // Focus only after the pane exists in state: `request_pane_focus` looks the pane up and no-ops
    // when it is missing, so requesting before the first-open insert (as this used to) silently
    // dropped focus on the initial toggle. `Context::request_focus` records the target key and the
    // renderer applies it once the scratch terminal node mounts, so this is safe on a fresh spawn.
    request_pane_focus(ctx, SCRATCH_PANE_ID);
    Update::full()
}

/// The scratchpad height as a fraction of the tile height: the drag-adjusted runtime override
/// when present, otherwise the configured default.
pub(crate) fn scratch_height_fraction(state: &crate::state::State) -> f32 {
    state
        .scratch_height
        .unwrap_or(state.config.scratchpad.height)
}

/// The scratch shell exited: drop it so the next toggle re-spawns a fresh one, and hide it.
pub(crate) fn handle_scratch_exit(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.scratch = None;
    ctx.state.scratch_visible = false;
    if let Some(prev) = ctx.state.scratch_return_focus.take() {
        crate::focus_ops::focus_pane(&mut ctx.state, prev);
        request_pane_focus(ctx, prev);
    } else {
        request_current_pane_focus(ctx);
    }
    Update::full()
}

pub(crate) fn is_scratch(id: crate::state::PaneId) -> bool {
    id == SCRATCH_PANE_ID
}

/// Grab the scratchpad's top edge: remember the current height fraction so the drag recomputes
/// from this origin rather than accumulating per-move deltas.
pub(crate) fn begin_resize(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.scratch_resize_start = Some(scratch_height_fraction(&ctx.state));
    Update::none()
}

/// Drag the scratchpad's top edge to a new height. `from_y`/`y` are root-space rows; dragging up
/// (smaller `y`) grows the bottom-anchored dropdown. The new height is stored as a runtime
/// override and clamped to the configured bounds.
pub(crate) fn resize(ctx: &mut Context<HyprmuxApp>, from_y: u16, y: u16) -> Update {
    let start = match ctx.state.scratch_resize_start {
        Some(start) => start,
        None => scratch_height_fraction(&ctx.state),
    };
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let tile_h = workspace_tile_bounds(bounds, ctx.state.workspace_top_gap()).h;
    if tile_h <= 0.0 {
        return Update::none();
    }
    let grow_px = f32::from(from_y) - f32::from(y);
    let fraction =
        ((start * tile_h + grow_px) / tile_h).clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
    ctx.state.scratch_height = Some(fraction);
    Update::full()
}

pub(crate) fn end_resize(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.scratch_resize_start = None;
    Update::none()
}

/// A dimming scrim covering the whole canvas, drawn behind the dropdown so the scratchpad
/// reads as the focused layer. Clicking it dismisses the scratchpad. Returns `None` when the
/// scratchpad is hidden.
pub(crate) fn scratch_backdrop(
    ctx: &Context<HyprmuxApp>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress <= SCRATCH_ANIM_EPSILON {
        return None;
    }
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
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
    Some((bounds, region.key("hyprmux-scratch-scrim")))
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
pub(crate) fn scratch_placement(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress <= SCRATCH_ANIM_EPSILON && !ctx.state.scratch_visible {
        return None;
    }
    let pane = ctx.state.scratch.as_ref()?;
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let rect = scratch_slide_rect(
        bounds,
        scratch_height_fraction(&ctx.state),
        progress,
        top_gap,
    );
    let element = view::pane_element(
        app,
        ctx,
        pane,
        rect,
        Some(SCRATCH_PANE_ID),
        "S",
        view::PaneMerge::default(),
    );
    Some((rect, element))
}

/// A thin drag handle sitting over the scratchpad's top chrome (title row and top border) that
/// resizes its height, mirroring the tiled split-drag strips. Only shown once fully deployed so
/// it never floats detached from the sliding pane. Returns its placement in canvas coordinates.
pub(crate) fn scratch_resize_strip(
    ctx: &Context<HyprmuxApp>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress < 1.0 - SCRATCH_ANIM_EPSILON || !ctx.state.scratch_visible {
        return None;
    }
    ctx.state.scratch.as_ref()?;
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let deployed = scratch_rect(bounds, scratch_height_fraction(&ctx.state), top_gap);
    // Title row (when shown) plus the frame's top border row.
    let strip_h: f32 = if ctx.state.config.pane.show_titles {
        2.0
    } else {
        1.0
    };
    let strip = FloatRect {
        h: strip_h.min(deployed.h),
        ..deployed
    };
    // Capture clicks so a plain click on the handle keeps the scratchpad focused instead of
    // falling through to the dismiss-scrim beneath it; drags resize.
    let region: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(
            ctx.link()
                .callback(|_| crate::Msg::FocusPane(SCRATCH_PANE_ID)),
        )
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
    Some((strip, region.key("hyprmux-scratch-resize-strip")))
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
        let deployed = scratch_slide_rect(bounds, 0.4, 1.0, OUTER_GAP);
        let hidden = scratch_slide_rect(bounds, 0.4, 0.0, OUTER_GAP);
        // Fully deployed matches scratch_rect; the slide only moves y, never the size.
        assert_eq!(deployed.y, shown.y);
        assert_eq!(deployed.h, shown.h);
        assert_eq!(hidden.h, shown.h);
        // Hidden sits entirely below the deployed position.
        assert!(hidden.y >= shown.y + shown.h - 1.0);
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
}
