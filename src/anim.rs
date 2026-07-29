use std::time::Duration;

use tui_lipan::prelude::{Easing, TransitionConfig};

pub const GEOMETRY_MS: u64 = 220;
pub const CLOSE_MS: u64 = 120;
pub const OPEN_DELAY_MS: u64 = 36;
pub const FOCUS_CHROME_MS: u64 = 160;
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
            open_delay: Duration::from_millis(OPEN_DELAY_MS),
        }
    }
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
}
