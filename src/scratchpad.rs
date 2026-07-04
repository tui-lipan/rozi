use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::config::{HyprmuxConfig, SCRATCHPAD_MAX_HEIGHT, SCRATCHPAD_MIN_HEIGHT};
use crate::focus_ops::{request_current_pane_focus, request_pane_focus};
use crate::geometry::{canvas_bounds_from_viewport, inset_float_rect};
use crate::pane_lifecycle::{pty_config_for_pane, spawn_pty_command};
use crate::state::{OUTER_GAP, Pane, SCRATCH_PANE_ID};
use crate::theme_ops::{pane_frame_background, terminal_palette};
use crate::view;

/// Below this much progress the dropdown is treated as fully retracted and not rendered.
const SCRATCH_ANIM_EPSILON: f32 = 0.01;

/// Bottom-anchored dropdown rect when fully deployed: full tile width, `height_fraction` of
/// the tile height, flush with the bottom inset. The pane slides up from below to reach it.
pub(crate) fn scratch_rect(bounds: FloatRect, height_fraction: f32) -> FloatRect {
    let inset = inset_float_rect(bounds, OUTER_GAP);
    let fraction = height_fraction.clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
    let h = (inset.h * fraction).round().max(1.0);
    FloatRect {
        x: inset.x,
        y: inset.y + inset.h - h,
        w: inset.w,
        h,
    }
}

/// The deployed rect translated straight down so its top edge sits at the bottom inset (fully
/// off-screen). At `progress == 0.0` the dropdown is here; at `1.0` it is at `scratch_rect`.
/// Only the `y` position moves - width/height are constant, so the PTY never resizes mid-slide.
fn scratch_slide_rect(bounds: FloatRect, height_fraction: f32, progress: f32) -> FloatRect {
    let shown = scratch_rect(bounds, height_fraction);
    let inset = inset_float_rect(bounds, OUTER_GAP);
    let hidden_y = inset.y + inset.h;
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
        app.scratch_transition_config(),
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
    request_pane_focus(ctx, SCRATCH_PANE_ID);

    if ctx.state.scratch.is_none() {
        let bounds = canvas_bounds_from_viewport(ctx.viewport());
        let rect = scratch_rect(bounds, ctx.state.config.scratchpad.height);
        let generation = ctx.state.next_pty_generation;
        ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
        let mut pane = Pane::new(SCRATCH_PANE_ID, ctx.state.config.scrollback, rect);
        pane.pty_generation = generation;
        pane.terminal.bind_session(SCRATCH_PANE_ID, generation);
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
        ctx.state.scratch = Some(pane);
        return Update::with_command(spawn_pty_command(
            ctx.state.runtime_epoch,
            SCRATCH_PANE_ID,
            generation,
            scratch_pty_config(&ctx.state.config, ctx.state.control_socket_path.as_deref()),
            None,
        ));
    }

    Update::full()
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

/// Build the scratch PTY config, honoring `[scratchpad]` command/cwd overrides by reusing the
/// per-pane config path (a command is wrapped in `shell -lc`).
fn scratch_pty_config(
    config: &HyprmuxConfig,
    control_socket_path: Option<&std::path::Path>,
) -> TerminalPtyConfig {
    let rect = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 80.0,
        h: 24.0,
    };
    let mut pane = Pane::new(SCRATCH_PANE_ID, config.scrollback, rect);
    pane.identity.command = config.scratchpad.command.clone();
    pane.identity.cwd = config.scratchpad.cwd.clone();
    pty_config_for_pane(config, control_socket_path, &pane)
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
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    // A transparent full-canvas catcher: it swallows clicks meant for the dimmed panes and
    // dismisses the scratchpad when clicked. It paints nothing - an opaque scrim would occlude
    // the panes' text and borders, so the "focused layer" cue is the panes dimming instead
    // (see `scratch_dim`, applied to each pane in `view::render`).
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

/// Opacity multiplier for the workspace panes while a focused layer (the scratchpad, or a
/// modal dialog) is up. Dimming the panes toward the backdrop - rather than overlaying an
/// opaque layer - signals the focused layer while keeping the panes' text legible, and tracks
/// the animation `progress` so the dim eases in and out with it. `progress` is `0.0` (no dim)
/// to `1.0` (fully deployed/open).
pub(crate) fn backdrop_dim(progress: f32) -> f32 {
    1.0 - 0.5 * progress.clamp(0.0, 1.0)
}

/// The placement and element for the scratchpad, to be added on top of the workspace canvas.
/// `progress` drives the slide; the pane stays mounted while it animates back down on hide,
/// and is dropped once fully retracted.
pub(crate) fn scratch_placement(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    progress: f32,
) -> Option<(FloatRect, Element)> {
    if progress <= SCRATCH_ANIM_EPSILON && !ctx.state.scratch_visible {
        return None;
    }
    let pane = ctx.state.scratch.as_ref()?;
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let rect = scratch_slide_rect(bounds, ctx.state.config.scratchpad.height, progress);
    let element = view::pane_element(app, ctx, pane, rect, Some(SCRATCH_PANE_ID), "S");
    Some((rect, element))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_rect_is_bottom_anchored_full_width() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let rect = scratch_rect(bounds, 0.4);
        let inset = inset_float_rect(bounds, OUTER_GAP);
        assert_eq!(rect.x, inset.x);
        assert_eq!(rect.w, inset.w);
        assert!((rect.h - inset.h * 0.4).abs() <= 1.0);
        // Flush with the bottom inset.
        assert!(((rect.y + rect.h) - (inset.y + inset.h)).abs() <= 1.0);
    }

    #[test]
    fn scratch_slide_starts_below_and_ends_deployed() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let shown = scratch_rect(bounds, 0.4);
        let deployed = scratch_slide_rect(bounds, 0.4, 1.0);
        let hidden = scratch_slide_rect(bounds, 0.4, 0.0);
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
        let tall = scratch_rect(bounds, 5.0);
        let inset = inset_float_rect(bounds, OUTER_GAP);
        assert!(tall.h <= inset.h * SCRATCHPAD_MAX_HEIGHT + 1.0);
    }
}
