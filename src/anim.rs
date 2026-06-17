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
    FloatingMove,
}

#[derive(Clone, Copy, Debug)]
pub struct WindowAnimationConfig {
    pub enabled: bool,
    pub geometry: bool,
    /// Optional escape hatch for tiled size interpolation. It is off by default
    /// because live PTYs should receive a single resize per layout change.
    pub tiled_size: bool,
    pub opacity: bool,
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
            geometry: true,
            tiled_size: false,
            opacity: true,
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

pub fn opacity_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseOutQuad,
    }
}

pub fn focus_chrome_transition(duration: Duration) -> TransitionConfig {
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
    if animations.enabled && animations.opacity {
        animations.open_delay
    } else {
        Duration::ZERO
    }
}

pub fn close_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.opacity {
        animations.close_duration + Duration::from_millis(20)
    } else {
        Duration::ZERO
    }
}
