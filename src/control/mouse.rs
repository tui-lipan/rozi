//! Mouse gesture vocabulary carried by `send-mouse` control requests and session wire messages.
//!
//! These are Rozi's own serde types so the public control API and session protocol do not couple to
//! `tui-lipan`'s internal event representation; conversion happens only at the encoding boundary in
//! [`crate::input::send_mouse`].

use serde::{Deserialize, Serialize};

/// Maximum repeat count accepted for a single click or scroll gesture.
pub const MAX_MOUSE_COUNT: u16 = 100;

fn default_mouse_count() -> u16 {
    1
}

/// Zero-based cell coordinates relative to a pane's content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct MousePoint {
    pub x: u16,
    pub y: u16,
}

/// Mouse button for a synthetic press, click, or drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
}

impl MouseButton {
    pub fn parse_cli(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "left" => Ok(Self::Left),
            "middle" => Ok(Self::Middle),
            "right" => Ok(Self::Right),
            _ => Err(format!(
                "--button must be left, middle, or right, got `{value}`"
            )),
        }
    }
}

/// Wheel scroll direction for a synthetic scroll gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum MouseScrollDirection {
    Up,
    Down,
}

/// Keyboard modifiers held during a synthetic mouse gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct MouseModifiers {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
}

impl MouseModifiers {
    pub const fn is_empty(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift
    }

    pub fn parse_cli(value: &str) -> Result<Self, String> {
        let mut mods = Self::default();
        if value.trim().is_empty() {
            return Err(
                "--modifiers requires a comma-separated list of ctrl, alt, shift".to_string(),
            );
        }
        for part in value.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                other => {
                    return Err(format!(
                        "--modifiers entries must be ctrl, alt, or shift, got `{other}`"
                    ));
                }
            }
        }
        Ok(mods)
    }
}

/// One synthetic mouse gesture to encode and deliver to a pane's application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum MouseAction {
    /// Press and release a button at `position`, repeated `count` times back to back.
    Click {
        position: MousePoint,
        #[serde(default)]
        button: MouseButton,
        #[serde(default = "default_mouse_count")]
        #[cfg_attr(
            feature = "schema-gen",
            schemars(range(min = 1, max = MAX_MOUSE_COUNT))
        )]
        count: u16,
        #[serde(default, skip_serializing_if = "MouseModifiers::is_empty")]
        modifiers: MouseModifiers,
    },
    /// Send wheel ticks in `direction` at `position`, repeated `count` times.
    Scroll {
        position: MousePoint,
        direction: MouseScrollDirection,
        #[serde(default = "default_mouse_count")]
        #[cfg_attr(
            feature = "schema-gen",
            schemars(range(min = 1, max = MAX_MOUSE_COUNT))
        )]
        count: u16,
        #[serde(default, skip_serializing_if = "MouseModifiers::is_empty")]
        modifiers: MouseModifiers,
    },
    /// Move the pointer to `position` with no button held.
    Move {
        position: MousePoint,
        #[serde(default, skip_serializing_if = "MouseModifiers::is_empty")]
        modifiers: MouseModifiers,
    },
    /// Press `button` at `from`, drag along the cell path to `to`, and release at `to`.
    Drag {
        from: MousePoint,
        to: MousePoint,
        #[serde(default)]
        button: MouseButton,
        #[serde(default, skip_serializing_if = "MouseModifiers::is_empty")]
        modifiers: MouseModifiers,
    },
}

impl MouseAction {
    /// Validate gesture parameters that do not depend on pane state.
    pub fn validate(&self) -> Result<(), String> {
        match *self {
            Self::Click { count, .. } | Self::Scroll { count, .. } => {
                if !(1..=MAX_MOUSE_COUNT).contains(&count) {
                    return Err(format!(
                        "count must be between 1 and {MAX_MOUSE_COUNT}, got {count}"
                    ));
                }
                Ok(())
            }
            Self::Move { .. } | Self::Drag { .. } => Ok(()),
        }
    }
}
