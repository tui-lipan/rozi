use std::time::Duration;

use tui_lipan::prelude::{Easing, TransitionConfig};

pub const GEOMETRY_MS: u64 = 220;
pub const CLOSE_MS: u64 = 120;
pub const OPEN_DELAY_MS: u64 = 36;
pub const FOCUS_CHROME_MS: u64 = 160;
pub const ALERT_PULSE_MS: u64 = 1600;
pub const ALERT_PULSE_MIN_HALF_MS: u64 = 400;
/// Alert borders remain recognizably alert-colored at the bottom of their breathe.
pub const ALERT_PULSE_BLEND: f32 = 0.55;

/// How much longer a "calm" alert breathes than an urgent one. A finished agent is good news you
/// have not read yet, not a request for an answer, so it should not compete with a blocked pane for
/// attention. An integer multiple keeps the two in a harmonic relationship: they realign every
/// `ALERT_PULSE_CALM_FACTOR` beats instead of drifting past each other, which is what makes two
/// simultaneous breathes read as one system rather than as noise.
pub const ALERT_PULSE_CALM_FACTOR: u32 = 2;

/// How far a marked workspace tab's *background* is tinted toward its alert role at the peak of the
/// breathe. Tabs mark on background rather than foreground: a coloured glyph on the panel surface is
/// too quiet to catch peripheral vision in a one-row bar, while a fully saturated cell block is
/// alarm-grade. A partial tint reads as a filled, marked tab without shouting, and the trough is the
/// untinted panel surface, so the tab breathes between neutral and its role colour.
pub const ALERT_TAB_TINT: f32 = 0.72;
const SCRATCH_DURATION_NUMERATOR: u32 = 2;
const SCRATCH_DURATION_DENOMINATOR: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryAnimation {
    None,
    Spawn,
    Close,
    Fullscreen,
    TileFloat,
    AxisChange,
}

#[derive(Clone, Copy, Debug)]
pub struct WindowAnimationConfig {
    pub enabled: bool,
    pub spawn: bool,
    pub close: bool,
    pub fullscreen: bool,
    pub tile_float: bool,
    pub axis_change: bool,
    pub focus_chrome: bool,
    pub geometry_duration: Duration,
    pub close_duration: Duration,
    pub focus_chrome_duration: Duration,
    pub alert_pulse_duration: Duration,
    pub open_delay: Duration,
}

impl Default for WindowAnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            spawn: true,
            close: true,
            fullscreen: true,
            tile_float: true,
            axis_change: true,
            focus_chrome: true,
            geometry_duration: Duration::from_millis(GEOMETRY_MS),
            close_duration: Duration::from_millis(CLOSE_MS),
            focus_chrome_duration: Duration::from_millis(FOCUS_CHROME_MS),
            alert_pulse_duration: Duration::from_millis(ALERT_PULSE_MS),
            open_delay: Duration::from_millis(OPEN_DELAY_MS),
        }
    }
}

/// Half the configured breathe period, floored to prevent alert colors becoming a strobe.
pub fn alert_pulse_half_period(animations: WindowAnimationConfig) -> Duration {
    (animations.alert_pulse_duration / 2).max(Duration::from_millis(ALERT_PULSE_MIN_HALF_MS))
}

/// Half period for calm alerts. Derived from the urgent half period rather than configured
/// separately, so the two stay an exact multiple apart however `alert_pulse_ms` is set - the tick
/// chain runs at the urgent rate and calm alerts simply flip on every `ALERT_PULSE_CALM_FACTOR`th
/// beat.
pub fn alert_pulse_calm_half_period(animations: WindowAnimationConfig) -> Duration {
    alert_pulse_half_period(animations) * ALERT_PULSE_CALM_FACTOR
}

pub fn geometry_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseInOutCubic,
    }
}

/// Geometry for a pane that is closing. `EaseInOutCubic` ramps in slowly, which is right for a
/// pane settling into a new tile but wrong here: the fade riding on top of the scale is
/// `EaseOutQuad`, so a slow-starting scale is still near full size when the pane has already gone
/// transparent, and the shrink is never actually seen. Match the fade instead.
pub fn close_geometry_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseOutQuad,
    }
}

pub fn instant_transition() -> TransitionConfig {
    TransitionConfig {
        duration: Duration::ZERO,
        easing: Easing::Linear,
    }
}

pub fn open_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.spawn {
        animations.open_delay
    } else {
        Duration::ZERO
    }
}

pub fn activation_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.spawn {
        animations.open_delay + animations.geometry_duration
    } else {
        Duration::ZERO
    }
}

/// How long a closing pane stays described before `Msg::PruneClosed` drops it. The margin
/// covers the frame the animation finishes on.
pub fn retained_pane_timeout(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.close {
        animations.close_duration + Duration::from_millis(20)
    } else {
        Duration::ZERO
    }
}

pub fn scratch_transition_duration(geometry_duration: Duration) -> Duration {
    (geometry_duration / SCRATCH_DURATION_DENOMINATOR) * SCRATCH_DURATION_NUMERATOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_transition_duration_is_two_thirds_of_geometry_duration() {
        assert_eq!(
            scratch_transition_duration(Duration::from_millis(300)),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn retained_pane_timeout_covers_the_close_animation() {
        let animations = WindowAnimationConfig {
            close_duration: Duration::from_millis(80),
            // A longer survivor-expansion duration must not extend how long the closed pane is
            // kept: survivors animate independently of its lifetime.
            geometry_duration: Duration::from_millis(240),
            ..WindowAnimationConfig::default()
        };
        assert_eq!(
            retained_pane_timeout(animations),
            Duration::from_millis(100)
        );
        assert_eq!(
            retained_pane_timeout(WindowAnimationConfig {
                close: false,
                ..animations
            }),
            Duration::ZERO
        );
    }

    #[test]
    fn alert_pulse_half_period_has_an_accessible_floor() {
        let mut animations = WindowAnimationConfig::default();
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(800)
        );
        animations.alert_pulse_duration = Duration::from_millis(100);
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(400)
        );
        animations.alert_pulse_duration = Duration::ZERO;
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(400)
        );
    }
}
