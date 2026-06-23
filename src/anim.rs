use std::time::Duration;

use tui_lipan::prelude::{Easing, TransitionConfig};

pub const GEOMETRY_MS: u64 = 220;
pub const CLOSE_MS: u64 = 120;
pub const OPEN_DELAY_MS: u64 = 36;
pub const FOCUS_CHROME_MS: u64 = 160;

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

/// How long to keep a closed pane in state before pruning it, so its own exit
/// animation (shrink toward [`close_rect`] + opacity fade, both run for
/// `close_duration`) can finish first. Panes surviving the close expand into the
/// freed space on their own `geometry_duration` transition, independently of
/// whether the closed pane is still present, so this does not need to wait for
/// them.
///
/// [`close_rect`]: crate::geometry::close_rect
pub fn close_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.close {
        animations.close_duration + Duration::from_millis(20)
    } else {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_delay_covers_the_closing_panes_exit_animation() {
        let animations = WindowAnimationConfig {
            close_duration: Duration::from_millis(80),
            // A longer survivor-expansion duration must not extend the prune delay;
            // survivors animate independently of the closed pane's lifetime.
            geometry_duration: Duration::from_millis(240),
            ..WindowAnimationConfig::default()
        };

        assert_eq!(close_delay(animations), Duration::from_millis(100));
    }

    #[test]
    fn close_delay_is_zero_when_close_animation_disabled() {
        let animations = WindowAnimationConfig {
            close: false,
            ..WindowAnimationConfig::default()
        };

        assert_eq!(close_delay(animations), Duration::ZERO);
    }
}
