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
    match spec.kind {
        anim::PaneAnimationStyle::Slide if !pane.floating => {}
        anim::PaneAnimationStyle::Off
        | anim::PaneAnimationStyle::Scale
        | anim::PaneAnimationStyle::Portal
        | anim::PaneAnimationStyle::Scan
        | anim::PaneAnimationStyle::Slide => return 1.0,
    }
    let (target, enabled) = open_close_target(pane, animations);
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
/// Timing follows [`anim::lifecycle_motion_enabled`]: a missing snapshot is not a licence to
/// ignore the Animations master switch.
fn open_close_target(pane: &Pane, animations: anim::WindowAnimationConfig) -> (f32, bool) {
    (
        if pane.closing || pane.opening {
            0.0
        } else {
            1.0
        },
        anim::lifecycle_motion_enabled(animations, pane),
    )
}

/// Progress for a centre-scaled pane while its subtree remains at the settled rectangle.
///
/// Call this on every frame the pane is drawn, not only while it animates: the key has to hold
/// 1.0 before a close flips its target, or the close has nothing to depart from. See the note
/// at the call site in `view::render_workspace_panes`.
pub(crate) fn scale_progress(ctx: &Context<AppRoot>, pane: &Pane, key: String) -> f32 {
    let animations = ctx.state.config.animations;
    let spec = anim::pane_animation_for_pane(animations, pane);
    match spec.kind {
        anim::PaneAnimationStyle::Scale => {}
        anim::PaneAnimationStyle::Off
        | anim::PaneAnimationStyle::Slide
        | anim::PaneAnimationStyle::Portal
        | anim::PaneAnimationStyle::Scan => return 1.0,
    }
    let (target, enabled) = open_close_target(pane, animations);
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
    match spec.kind {
        anim::PaneAnimationStyle::Portal | anim::PaneAnimationStyle::Scan => {}
        anim::PaneAnimationStyle::Off
        | anim::PaneAnimationStyle::Scale
        | anim::PaneAnimationStyle::Slide => return 1.0,
    }
    let (target, enabled) = open_close_target(pane, animations);
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
    // would make the leading edge ghostly instead of solid. Off does not fade either; its
    // opacity target is already the settled value, so the transition is instant.
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

/// Timing for focus colour changes on pane chrome, workbar tabs, and sidebar rows.
///
/// Instant on the first frame of a new session view: the incoming session should appear already
/// focused, with only its layer fading in, rather than its chrome animating out of the previous
/// session's colours. See [`crate::state::State::session_view_changed`].
pub(crate) fn focus_chrome_transition_config(ctx: &Context<AppRoot>) -> TransitionConfig {
    let animations = ctx.state.config.animations;
    if animations.enabled && animations.focus_chrome && !ctx.state.session_view_changed.get() {
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

/// How far a newly shown session has taken over the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionReveal {
    /// Opacity of the session content layer (workbar and workspace). Below 1 only while a fade
    /// reveal is running.
    pub(crate) opacity: f32,
    /// How far the portal has opened, `1.0` when there is no portal to draw.
    pub(crate) portal: f32,
}

/// The reveal of the session content layer, for the configured [`anim::SessionAnimationStyle`].
///
/// The attachment has already swapped by the time this runs; only its presentation moves. The
/// first frame after [`State::session_view_revision`] moves restarts the reveal from its beginning:
/// the layer at [`anim::SESSION_REVEAL_FROM`] for a fade, a closed portal for a portal. The very
/// first render has no previous session to delimit from, so the client's initial frame never
/// animates.
///
/// [`State::session_view_revision`]: crate::state::State::session_view_revision
pub(crate) fn session_reveal(ctx: &Context<AppRoot>) -> SessionReveal {
    const KEY: &str = "rozi-session-reveal";
    let revision = ctx.state.session_view_revision;
    let previous = ctx.state.session_reveal_seen.replace(Some(revision));
    let changed = previous.is_some_and(|seen| seen != revision);
    ctx.state.session_view_changed.set(changed);
    if changed {
        // Seeded even when the reveal is off: retargeting from a distinct value is what applies
        // the instant config, so a reveal still in flight when animations were disabled stops
        // rather than finishing on its old duration (see `workspace_offset`).
        ctx.transition(KEY, 0.0, anim::instant_transition());
    }
    let animations = ctx.state.config.animations;
    let config =
        anim::session_reveal_transition(animations).unwrap_or_else(anim::instant_transition);
    let progress = ctx.transition(KEY, 1.0, config);
    match animations.session {
        anim::SessionAnimationStyle::Off | anim::SessionAnimationStyle::Fade => SessionReveal {
            opacity: anim::SESSION_REVEAL_FROM + (1.0 - anim::SESSION_REVEAL_FROM) * progress,
            portal: 1.0,
        },
        anim::SessionAnimationStyle::Portal => SessionReveal {
            opacity: 1.0,
            portal: progress,
        },
    }
}

/// How the outgoing session's layer leaves once a switch replaces it.
///
/// It stays painted beneath its successor, so an opaque successor covers it at once. It matters
/// in two places. During a fresh attach's grace period it stands in for a Connecting scene that
/// would only flash; ease-in keeps it nearly whole for a local attach, which lands in tens of
/// milliseconds, and only a slow one watches it fade into Connecting. Under a portal it is what the
/// portal opens over, so it has to outlast both the grace period and the portal, and it only
/// recedes rather than fading away.
pub(crate) fn session_layer_exit(animations: anim::WindowAnimationConfig) -> ExitAnimation {
    let hold = crate::ops::session::CONNECT_HOLD;
    let millis = |duration: std::time::Duration| duration.as_millis() as u64;
    match anim::session_reveal_transition(animations) {
        Some(portal) if anim::session_portal_enabled(animations) => {
            ExitAnimation::new(millis(hold + portal.duration))
                .opacity(anim::SESSION_PORTAL_RECEDE)
                .easing(Easing::EaseInQuad)
        }
        _ => ExitAnimation::new(millis(hold)).easing(Easing::EaseInQuad),
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::layout::anim;

    use tui_lipan::TestBackend;
    use tui_lipan::prelude::{Color, Rect};

    use crate::AppRoot;

    fn in_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .expect("spawn test thread")
            .join()
            .expect("join test thread");
    }

    const VIEWPORT_W: u16 = 80;
    const VIEWPORT_H: u16 = 24;

    fn backend() -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: VIEWPORT_W,
            h: VIEWPORT_H,
        });
        backend.render();
        backend
    }

    /// Backgrounds only: the workbar clock may change its text between frames, never its colour.
    fn backgrounds(backend: &TestBackend<AppRoot>) -> Vec<Color> {
        backend
            .capture_frame()
            .cells
            .into_iter()
            .map(|cell| cell.bg)
            .collect()
    }

    fn reveal_duration(backend: &TestBackend<AppRoot>) -> Duration {
        crate::layout::anim::session_reveal_transition(backend.state().config.animations)
            .expect("session reveal enabled by default")
            .duration
    }

    #[test]
    fn a_new_session_view_resolves_in_and_then_settles() {
        in_stack(|| {
            let mut backend = backend();
            let settled = backgrounds(&backend);

            backend.state_mut().session_view_revision += 1;
            backend.render();
            assert_ne!(
                backgrounds(&backend),
                settled,
                "the incoming session should start dimmed toward the backdrop"
            );

            let duration = reveal_duration(&backend);
            backend.advance(duration + Duration::from_millis(20));
            assert_eq!(backgrounds(&backend), settled);

            // Nothing new to reveal: a redraw of the same session must not fade again.
            backend.render();
            assert_eq!(backgrounds(&backend), settled);
        });
    }

    /// Two tiled panes, `focus` focused, the session fade off so frames compare exactly.
    fn two_panes(focus: crate::state::PaneId) -> TestBackend<AppRoot> {
        let mut backend = backend();
        {
            let state = backend.state_mut();
            state.config.animations.session = anim::SessionAnimationStyle::Off;
            let workspace = &mut state.current_mut().workspaces[0];
            for id in [1, 2] {
                let mut pane =
                    crate::state::Pane::new(id, 100, tui_lipan::prelude::FloatRect::default());
                pane.opening = false;
                workspace.panes.push(pane);
                crate::layout::tiling::append_tiled_window(workspace, id);
            }
            workspace.focused_pane = Some(focus);
            state.current_mut().focused_pane = Some(focus);
        }
        backend.render();
        backend.advance(Duration::from_secs(1));
        backend
    }

    fn move_focus(backend: &mut TestBackend<AppRoot>, focus: crate::state::PaneId) {
        let state = backend.state_mut();
        state.current_mut().workspaces[0].focused_pane = Some(focus);
        state.current_mut().focused_pane = Some(focus);
    }

    /// Pane ids repeat across sessions, and chrome transitions are keyed by pane id. The incoming
    /// session's focused pane must arrive focused rather than fading out of the colours the
    /// outgoing session's pane with the same id was wearing.
    #[test]
    fn a_new_session_view_arrives_with_its_focus_chrome_settled() {
        in_stack(|| {
            let settled = backgrounds(&two_panes(2));
            let foregrounds = |backend: &TestBackend<AppRoot>| -> Vec<Color> {
                backend
                    .capture_frame()
                    .cells
                    .into_iter()
                    .map(|cell| cell.fg)
                    .collect()
            };
            let settled_fg = foregrounds(&two_panes(2));

            let mut switched = two_panes(1);
            move_focus(&mut switched, 2);
            switched.state_mut().session_view_revision += 1;
            switched.render();
            assert_eq!(backgrounds(&switched), settled);
            assert_eq!(foregrounds(&switched), settled_fg);

            // Control: the same focus move inside one session still animates.
            let mut moved = two_panes(1);
            move_focus(&mut moved, 2);
            moved.render();
            assert_ne!(foregrounds(&moved), settled_fg);
        });
    }

    fn symbols(backend: &TestBackend<AppRoot>) -> Vec<String> {
        backend
            .capture_frame()
            .cells
            .into_iter()
            .map(|cell| cell.symbol)
            .collect()
    }

    /// The real layer stack under a portal switch: the outgoing session is retained beneath the
    /// incoming one, and the portal opens from the centre. Midway, cells near the centre already
    /// show the new session while cells near the edge still show the old one.
    #[test]
    fn a_portal_switch_opens_the_new_session_over_the_old_one() {
        in_stack(|| {
            let mut old = two_panes(1);
            {
                old.state_mut().config.animations.session = anim::SessionAnimationStyle::Portal;
            }
            old.render();
            let before = symbols(&old);

            // Another session takes the foreground: a different attachment under a new id.
            {
                let state = old.state_mut();
                state.attachment = crate::state::Attachment::new();
                state.runtime_epoch = 99;
                state.current_mut().epoch = 99;
                state.session_view_revision += 1;
            }
            old.render();
            let duration = anim::session_reveal_transition(old.state().config.animations)
                .expect("portal enabled")
                .duration;
            old.advance(duration / 2);
            let midway = symbols(&old);
            old.advance(duration + Duration::from_secs(1));
            let after = symbols(&old);

            let width = usize::from(VIEWPORT_W);
            let height = usize::from(VIEWPORT_H);
            let reach = |index: usize| {
                let (x, y) = ((index % width) as f32, (index / width) as f32);
                let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
                let max = cx.hypot(cy * 2.0);
                (x - cx).hypot((y - cy) * 2.0) / max
            };
            let changed: Vec<usize> = (0..before.len())
                .filter(|&index| before[index] != after[index])
                .collect();
            let inner: Vec<usize> = changed
                .iter()
                .copied()
                .filter(|&i| reach(i) < 0.2)
                .collect();
            let outer: Vec<usize> = changed
                .iter()
                .copied()
                .filter(|&i| reach(i) > 0.85)
                .collect();
            assert!(
                !inner.is_empty() && !outer.is_empty(),
                "the two sessions must differ at both"
            );
            for index in inner {
                assert_eq!(
                    midway[index], after[index],
                    "centre cell {index} shows the new session"
                );
            }
            for index in outer {
                assert_eq!(
                    midway[index], before[index],
                    "edge cell {index} still shows the old one"
                );
            }
            assert_ne!(midway, after, "the portal is still opening midway");
        });
    }

    #[test]
    fn a_disabled_session_reveal_snaps() {
        in_stack(|| {
            let mut backend = backend();
            let settled = backgrounds(&backend);
            backend.state_mut().config.animations.session = anim::SessionAnimationStyle::Off;
            backend.state_mut().session_view_revision += 1;
            backend.render();
            assert_eq!(backgrounds(&backend), settled);

            backend.state_mut().config.animations.session = anim::SessionAnimationStyle::Fade;
            backend.state_mut().config.animations.enabled = false;
            backend.state_mut().session_view_revision += 1;
            backend.render();
            assert_eq!(backgrounds(&backend), settled);
        });
    }
}
