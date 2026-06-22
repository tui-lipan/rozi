use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::focus_ops::{request_current_pane_focus, request_pane_focus};
use crate::geometry::{canvas_bounds_from_viewport, inset_float_rect};
use crate::pane_lifecycle::{pty_config_for_pane, spawn_pty_command};
use crate::state::{
    HyprmuxConfig, OUTER_GAP, Pane, SCRATCH_PANE_ID, SCRATCHPAD_MAX_HEIGHT, SCRATCHPAD_MIN_HEIGHT,
};
use crate::theme_ops::terminal_palette;
use crate::view;

/// Top-anchored dropdown rect: full tile width, `height_fraction` of the tile height.
pub(crate) fn scratch_rect(bounds: FloatRect, height_fraction: f32) -> FloatRect {
    let inset = inset_float_rect(bounds, OUTER_GAP);
    let fraction = height_fraction.clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
    let h = (inset.h * fraction).round().max(1.0);
    FloatRect {
        x: inset.x,
        y: inset.y,
        w: inset.w,
        h,
    }
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
        let mut pane = Pane::new(SCRATCH_PANE_ID, ctx.state.config.scrollback, rect);
        pane.terminal
            .set_palette(terminal_palette(&ctx.state.theme));
        // No spawn fade — the dropdown slides in via the rect transition instead, and there
        // is no FinishOpen(scratch) message to clear an `opening` flag.
        pane.opening = false;
        ctx.state.scratch = Some(pane);
        return Update::with_command(spawn_pty_command(
            SCRATCH_PANE_ID,
            scratch_pty_config(&ctx.state.config),
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
fn scratch_pty_config(config: &HyprmuxConfig) -> TerminalPtyConfig {
    let rect = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 80.0,
        h: 24.0,
    };
    let mut pane = Pane::new(SCRATCH_PANE_ID, config.scrollback, rect);
    pane.identity.command = config.scratchpad.command.clone();
    pane.identity.cwd = config.scratchpad.cwd.clone();
    pty_config_for_pane(config, &pane)
}

/// The animated placement and element for the scratchpad, to be added on top of the
/// workspace canvas. Returns `None` when the scratchpad is hidden.
pub(crate) fn scratch_placement(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
) -> Option<(FloatRect, Element)> {
    if !ctx.state.scratch_visible {
        return None;
    }
    let pane = ctx.state.scratch.as_ref()?;
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let target = scratch_rect(bounds, ctx.state.config.scratchpad.height);
    let animated = ctx.transition(
        "hyprmux-scratch-rect",
        target,
        app.scratch_transition_config(),
    );
    let element = view::pane_element(app, ctx, pane, animated, Some(SCRATCH_PANE_ID));
    Some((animated, element))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_rect_is_top_anchored_full_width() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let rect = scratch_rect(bounds, 0.4);
        let inset = inset_float_rect(bounds, OUTER_GAP);
        assert_eq!(rect.x, inset.x);
        assert_eq!(rect.y, inset.y);
        assert_eq!(rect.w, inset.w);
        assert!((rect.h - inset.h * 0.4).abs() <= 1.0);
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
