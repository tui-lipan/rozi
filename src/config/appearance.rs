use std::time::Duration;

use crate::anim::{PaneAnimationStyle, WindowAnimationConfig};

use super::file::{AnimationFileConfig, PaddingSpec};

/// Cap defensively: padding eats terminal grid on every side, so a large value would leave no
/// usable pane. 8 cells is already generous for a cosmetic inset.
pub const MAX_PANE_PADDING: u16 = 8;

fn clamp_pane_padding(value: u16, warnings: &mut Vec<String>) -> u16 {
    if value > MAX_PANE_PADDING {
        warnings.push(format!(
            "Clamped pane.padding {value} to the maximum of {MAX_PANE_PADDING}"
        ));
        MAX_PANE_PADDING
    } else {
        value
    }
}

/// Resolve a `[pane] padding` spec into `(top, right, bottom, left)` cells using CSS shorthand: one
/// value applies to all sides, two are `[vertical, horizontal]`, four are `[top, right, bottom,
/// left]`. Other array lengths are rejected with a warning and leave the default untouched.
pub(super) fn resolve_pane_padding(
    spec: PaddingSpec,
    warnings: &mut Vec<String>,
) -> Option<(u16, u16, u16, u16)> {
    let sides = match spec {
        PaddingSpec::All(value) => vec![value],
        PaddingSpec::Sides(values) => values,
    };
    match sides.as_slice() {
        [all] => {
            let all = clamp_pane_padding(*all, warnings);
            Some((all, all, all, all))
        }
        [vertical, horizontal] => {
            let vertical = clamp_pane_padding(*vertical, warnings);
            let horizontal = clamp_pane_padding(*horizontal, warnings);
            Some((vertical, horizontal, vertical, horizontal))
        }
        [top, right, bottom, left] => Some((
            clamp_pane_padding(*top, warnings),
            clamp_pane_padding(*right, warnings),
            clamp_pane_padding(*bottom, warnings),
            clamp_pane_padding(*left, warnings),
        )),
        other => {
            warnings.push(format!(
                "Ignored pane.padding with {} value(s) (expected 1, 2, or 4)",
                other.len()
            ));
            None
        }
    }
}

pub(super) fn apply_animations(
    target: &mut WindowAnimationConfig,
    raw: AnimationFileConfig,
    warnings: &mut Vec<String>,
) {
    if let Some(pane_style) = raw.pane_style.as_deref() {
        match PaneAnimationStyle::parse(pane_style) {
            Some(style) => target.pane_style = style,
            None => warnings.push(format!(
                "Ignored unknown animations.pane_style \"{pane_style}\" (expected one of: scale, slide)"
            )),
        }
    }
    if let Some(value) = raw.enabled {
        target.enabled = value;
    }
    if let Some(value) = raw.spawn {
        target.spawn = value;
    }
    if let Some(value) = raw.close {
        target.close = value;
    }
    if let Some(value) = raw.fullscreen {
        target.fullscreen = value;
    }
    if let Some(value) = raw.tile_float {
        target.tile_float = value;
    }
    if let Some(value) = raw.axis_change {
        target.axis_change = value;
    }
    if let Some(value) = raw.focus_chrome {
        target.focus_chrome = value;
    }
    if let Some(value) = raw.geometry_ms {
        target.geometry_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.close_ms {
        target.close_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.focus_chrome_ms {
        target.focus_chrome_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.alert_pulse_ms {
        target.alert_pulse_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.open_delay_ms {
        target.open_delay = Duration::from_millis(value);
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct PaddingOnly {
        padding: PaddingSpec,
    }

    #[test]
    fn pane_padding_accepts_scalar_and_array_forms() {
        let scalar: PaddingOnly = toml::from_str("padding = 3").expect("scalar parses");
        assert_eq!(scalar.padding, PaddingSpec::All(3));

        let pair: PaddingOnly = toml::from_str("padding = [0, 1]").expect("pair parses");
        assert_eq!(pair.padding, PaddingSpec::Sides(vec![0, 1]));

        let quad: PaddingOnly = toml::from_str("padding = [1, 2, 3, 4]").expect("quad parses");
        assert_eq!(quad.padding, PaddingSpec::Sides(vec![1, 2, 3, 4]));
    }

    #[test]
    fn resolve_pane_padding_maps_css_shorthand() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(2), &mut warnings),
            Some((2, 2, 2, 2))
        );
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![0, 1]), &mut warnings),
            Some((0, 1, 0, 1))
        );
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3, 4]), &mut warnings),
            Some((1, 2, 3, 4))
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn resolve_pane_padding_clamps_and_rejects_bad_lengths() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(99), &mut warnings),
            Some((8, 8, 8, 8))
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3]), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(Vec::new()), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn animations_apply_alert_pulse_duration() {
        let raw: AnimationFileConfig =
            toml::from_str("alert_pulse_ms = 2400").expect("config parses");
        let mut animations = WindowAnimationConfig::default();
        let mut warnings = Vec::new();
        apply_animations(&mut animations, raw, &mut warnings);
        assert_eq!(animations.alert_pulse_duration, Duration::from_millis(2400));
        assert!(warnings.is_empty());
    }

    #[test]
    fn animations_apply_pane_style_and_warn_on_an_unknown_one() {
        let raw: AnimationFileConfig =
            toml::from_str("pane_style = \"slide\"").expect("config parses");
        let mut animations = WindowAnimationConfig::default();
        let mut warnings = Vec::new();
        apply_animations(&mut animations, raw, &mut warnings);
        assert_eq!(animations.pane_style, PaneAnimationStyle::Slide);
        assert!(warnings.is_empty());

        let raw: AnimationFileConfig =
            toml::from_str("pane_style = \"springy\"").expect("config parses");
        let mut animations = WindowAnimationConfig::default();
        apply_animations(&mut animations, raw, &mut warnings);
        // An unknown token leaves the default rather than silently disabling pane animation.
        assert_eq!(animations.pane_style, PaneAnimationStyle::Scale);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("scale, slide"),
            "the warning should list the accepted values: {}",
            warnings[0]
        );
    }
}
