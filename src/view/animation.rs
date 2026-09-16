use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::anim;
use crate::state::{ChromeSlot, Pane};

pub(crate) fn transition_config_for(
    ctx: &Context<AppRoot>,
    pane: &Pane,
    viewport_changed: bool,
    target_rect: FloatRect,
) -> TransitionConfig {
    anim::geometry_transition_for_pane(&ctx.state, pane, viewport_changed, Some(target_rect))
}

/// Slide progress for an opening or closing pane: `0.0` fully outside its tile, `1.0` deployed.
///
/// Mirrors [`window_opacity_config`]'s mechanism - the target flips when `opening` clears or
/// `closing` is set, and the transition carries the pane the rest of the way. A pane that never
/// slides reads a constant `1.0`, so the key stays alive and a config change mid-life does not make
/// it jump.
pub(crate) fn slide_progress(ctx: &Context<AppRoot>, pane: &Pane, key: String) -> f32 {
    let animations = ctx.state.config.animations;
    let spec = anim::pane_animation_for_pane(animations, pane);
    if spec.kind != anim::PaneAnimationStyle::Slide || pane.floating {
        return 1.0;
    }
    let (target, enabled) = open_close_target(pane);
    // Disabled means no motion, not a pane parked outside its own tile: snap to deployed rather
    // than letting an instant transition land the target of 0.0 and hide it.
    if !enabled {
        return 1.0;
    }
    ctx.transition(key, target, spec.transition(pane.closing))
}

/// Where a pane effect that runs forward on open and backward on close is heading, and whether
/// it is timed at all.
///
/// The target follows the lifecycle flags, never the snapshot. A snapshot outlives
/// `Pane::opening` on purpose - it holds the effect mounted and on its original recipe until
/// the terminal goes live - so reading it here would park every opening pane at its starting
/// value for the whole transition and then snap it to settled when the snapshot cleared.
fn open_close_target(pane: &Pane) -> (f32, bool) {
    if pane.closing {
        (
            0.0,
            pane.closing_animation
                .is_none_or(|snapshot| snapshot.active),
        )
    } else {
        (
            if pane.opening { 0.0 } else { 1.0 },
            pane.opening_animation
                .is_none_or(|snapshot| snapshot.active),
        )
    }
}

/// Progress for a centre-scaled pane while its subtree remains at the settled rectangle.
///
/// Call this on every frame the pane is drawn, not only while it animates: the key has to hold
/// 1.0 before a close flips its target, or the close has nothing to depart from. See the note
/// at the call site in `view::render_workspace_panes`.
pub(crate) fn scale_progress(ctx: &Context<AppRoot>, pane: &Pane, key: String) -> f32 {
    let animations = ctx.state.config.animations;
    let spec = anim::pane_animation_for_pane(animations, pane);
    if spec.kind != anim::PaneAnimationStyle::Scale {
        return 1.0;
    }
    let (target, enabled) = open_close_target(pane);
    if !enabled {
        return 1.0;
    }
    ctx.transition(key, target, spec.transition(pane.closing))
}

/// Progress for a full-size pane paint effect. The same keyed transition runs in reverse when a
/// pane closes, while its rectangle remains at the destination for the whole effect.
pub(crate) fn pane_reveal_progress(
    ctx: &Context<AppRoot>,
    pane: &Pane,
    key: impl Into<tui_lipan::prelude::Key>,
) -> f32 {
    let animations = ctx.state.config.animations;
    let spec = anim::pane_animation_for_pane(animations, pane);
    if !matches!(
        spec.kind,
        anim::PaneAnimationStyle::Portal | anim::PaneAnimationStyle::Scan
    ) {
        return 1.0;
    }
    let (target, enabled) = open_close_target(pane);
    if !enabled {
        return 1.0;
    }
    ctx.transition(key, target, spec.transition(pane.closing))
}

pub(crate) fn window_opacity_config(ctx: &Context<AppRoot>, pane: &Pane) -> TransitionConfig {
    let animations = ctx.state.config.animations;
    let snapshot_active = pane
        .closing_animation
        .or(pane.opening_animation)
        .is_some_and(|snapshot| snapshot.active);
    if !animations.enabled && !snapshot_active {
        return anim::instant_transition();
    }
    // A slide is not faded: it is clipped to its tile, so it genuinely emerges. A fade on top
    // would make the leading edge ghostly instead of solid.
    let spec = anim::pane_animation_for_pane(animations, pane);
    if !anim::pane_opacity_animates(animations, pane) {
        return anim::instant_transition();
    }
    if anim::pane_opening_transition(pane) || pane.closing {
        spec.visual_transition(pane.closing)
    } else {
        anim::instant_transition()
    }
}

pub(crate) fn scratch_transition_config(ctx: &Context<AppRoot>) -> TransitionConfig {
    let animations = ctx.state.config.animations;
    if animations.enabled && animations.tile_float {
        anim::geometry_transition(anim::scratch_transition_duration(
            animations.geometry_duration,
        ))
    } else {
        anim::instant_transition()
    }
}

pub(crate) fn focus_chrome_transition_config(ctx: &Context<AppRoot>) -> TransitionConfig {
    let animations = ctx.state.config.animations;
    if animations.enabled && animations.focus_chrome {
        TransitionConfig {
            duration: animations.focus_chrome_duration,
            easing: Easing::EaseInOutCubic,
        }
    } else {
        anim::instant_transition()
    }
}

/// Breathe timing for an alert. `calm` stretches the fade to the calm half period so a slower
/// alert is genuinely slower rather than a fast fade that then sits still waiting for its beat.
pub(crate) fn alert_pulse_transition_config(
    ctx: &Context<AppRoot>,
    calm: bool,
) -> TransitionConfig {
    let animations = ctx.state.config.animations;
    if animations.enabled && animations.focus_chrome {
        TransitionConfig {
            duration: if calm {
                anim::alert_pulse_calm_half_period(animations)
            } else {
                anim::alert_pulse_half_period(animations)
            },
            easing: Easing::EaseInOutCubic,
        }
    } else {
        anim::instant_transition()
    }
}

/// A pane chrome colour, as a paint the renderer resolves while drawing.
///
/// `animated_color` rather than `transition`: chrome colours only ever land in styles, so naming
/// the fade instead of embedding its current value keeps this element identical for the whole
/// 160ms. Each frame of a focus change is then a repaint rather than a rebuild of every pane,
/// workbar segment and sidebar row in the window.
/// These take the `Pane` rather than its id because the key they need is already built and
/// cached on it. Formatting `rozi-pane-chrome-{id}-{slot}` here cost two allocations per slot
/// per pane per frame, for an identity that is fixed for the pane's whole life.
pub(crate) fn chrome_color(
    ctx: &Context<AppRoot>,
    pane: &Pane,
    slot: ChromeSlot,
    target: Color,
) -> Paint {
    chrome_color_with(ctx, pane, slot, target, focus_chrome_transition_config(ctx))
}

pub(crate) fn chrome_color_with(
    ctx: &Context<AppRoot>,
    pane: &Pane,
    slot: ChromeSlot,
    target: Color,
    config: TransitionConfig,
) -> Paint {
    chrome_color_with_frame_rate(ctx, pane, slot, target, config, None)
}

pub(crate) fn chrome_color_with_frame_rate(
    ctx: &Context<AppRoot>,
    pane: &Pane,
    slot: ChromeSlot,
    target: Color,
    config: TransitionConfig,
    frame_rate: Option<u16>,
) -> Paint {
    chrome_paint_with_frame_rate(
        ctx,
        pane.keys.chrome(slot).clone(),
        target,
        config,
        frame_rate,
    )
}

/// A caller-keyed chrome paint. Palette/indexed colors deliberately snap rather than blend.
pub(crate) fn chrome_paint_with_frame_rate(
    ctx: &Context<AppRoot>,
    key: impl Into<tui_lipan::prelude::Key>,
    target: Color,
    config: TransitionConfig,
    frame_rate: Option<u16>,
) -> Paint {
    // Only truecolor targets may fade. Named/indexed ANSI colors must be emitted
    // verbatim so the user's terminal palette resolves them; blending them animates
    // through `Color::Rgb` (`blend_toward` always returns Rgb), which bypasses the
    // palette and flips the hue mid-fade - e.g. an ANSI theme's `LightCyan` chrome
    // shows as true cyan while the focus animation runs but as the palette color at
    // rest. Snapping keeps palette themes consistent (and matching the workbar).
    let config = if crate::ops::theme::chrome_color_animates(target) {
        config
    } else {
        anim::instant_transition()
    };
    match frame_rate {
        Some(frame_rate) => ctx.animated_color_with_frame_rate(key, target, config, frame_rate),
        None => ctx.animated_color(key, target, config),
    }
}

/// Seed every workspace off-screen on its numbered side. Only the outgoing and active pages
/// travel, so jumping from 1 to 9 does not sweep through the seven intermediate workspaces.
pub(crate) fn workspace_offsets(ctx: &Context<AppRoot>, resized: bool) -> Vec<(usize, f32)> {
    let active = ctx.state.current().active_workspace;
    let epoch = ctx.state.runtime_epoch;
    let previous = ctx.state.workspace_slide.get();
    let outgoing = outgoing_workspace(previous, epoch, active);
    ctx.state
        .workspace_slide
        .set(Some((epoch, active, outgoing)));
    let same_attachment = previous.is_some_and(|(old_epoch, _, _)| old_epoch == epoch);
    let enabled = workspace_motion_enabled(ctx.state.config.animations, resized, same_attachment);
    let mut visible = Vec::with_capacity(2);
    for index in 0..ctx.state.current().workspaces.len() {
        let moving = index == active || index == outgoing;
        let offset = workspace_offset(ctx, index, active, enabled && moving);
        if index == active || (moving && offset.abs() < 1.0) {
            visible.push((index, offset));
        }
    }
    // Keep the destination above the outgoing page when a rapid switch reverses their motion.
    visible.sort_by_key(|(index, _)| *index == active);
    visible
}

fn workspace_motion_enabled(
    animations: anim::WindowAnimationConfig,
    resized: bool,
    same_attachment: bool,
) -> bool {
    animations.enabled
        && animations.workspace
        && !animations.workspace_duration.is_zero()
        && !resized
        && same_attachment
}

fn outgoing_workspace(previous: Option<(u64, usize, usize)>, epoch: u64, active: usize) -> usize {
    match previous {
        Some((old_epoch, old_active, outgoing)) if old_epoch == epoch => {
            if old_active == active {
                outgoing
            } else {
                old_active
            }
        }
        _ => active,
    }
}

fn workspace_offset(ctx: &Context<AppRoot>, index: usize, active: usize, animate: bool) -> f32 {
    let target = match index.cmp(&active) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    };
    let config = if animate {
        TransitionConfig {
            duration: ctx.state.config.animations.workspace_duration,
            easing: tui_lipan::animation::Easing::EaseOutQuad,
        }
    } else {
        anim::instant_transition()
    };
    let key = format!("rozi-workspace-slide-{}-{index}", ctx.state.runtime_epoch);
    if !animate {
        // tui-lipan 0.9.3 only applies a new duration when the target changes and has no
        // transition-reset API. Seed a distinct instant target, then the real target, so an
        // in-flight slide also stops on resize, disable, or attachment replacement.
        ctx.transition(key.clone(), target + 2.0, anim::instant_transition());
    }
    ctx.transition(key, target, config)
}
