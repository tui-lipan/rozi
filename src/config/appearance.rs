use std::time::Duration;

use tui_lipan::animation::{CubicBezier, Easing};

use crate::layout::anim::{
    PaneAnimationOverrides, PaneAnimationStyle, ScanDirection, WindowAnimationConfig,
};

use super::file::{AnimationFileConfig, CurveSpec, PaddingSpec};

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
    apply_animation_durations(target, &raw);
    apply_animation_style(target, raw.pane_style.as_deref(), warnings);
    apply_animation_flags(target, &raw);
    apply_animation_misc(target, &raw);
    target.pane_overrides = resolve_pane_overrides(&raw, warnings);
}

fn apply_animation_style(
    target: &mut WindowAnimationConfig,
    pane_style: Option<&str>,
    warnings: &mut Vec<String>,
) {
    let Some(pane_style) = pane_style else { return };
    match PaneAnimationStyle::parse(pane_style) {
        Some(style) => target.pane_style = style,
        None => warnings.push(format!(
            "Ignored unknown animations.pane_style \"{pane_style}\" (expected one of: scale, slide, portal, scan)"
        )),
    }
}

fn apply_animation_flags(target: &mut WindowAnimationConfig, raw: &AnimationFileConfig) {
    for (value, target_value) in [
        (raw.enabled, &mut target.enabled),
        (raw.spawn, &mut target.spawn),
        (raw.close, &mut target.close),
        (raw.fullscreen, &mut target.fullscreen),
        (raw.tile_float, &mut target.tile_float),
        (raw.axis_change, &mut target.axis_change),
        (raw.sidebar, &mut target.sidebar),
        (raw.focus_chrome, &mut target.focus_chrome),
    ] {
        if let Some(value) = value {
            *target_value = value;
        }
    }
}

fn apply_animation_misc(target: &mut WindowAnimationConfig, raw: &AnimationFileConfig) {
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

fn apply_animation_durations(target: &mut WindowAnimationConfig, raw: &AnimationFileConfig) {
    if let Some(value) = raw.geometry_ms {
        target.geometry_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.close_ms {
        target.close_duration = Duration::from_millis(value);
    }
}

/// Resolve the flat `[animations]` overrides into the typed form the layout reads.
///
/// Every value is checked and, if out of range, dropped with a warning rather than clamped: a
/// silently corrected `scale_from = 1.4` looks like the setting had no effect.
fn resolve_pane_overrides(
    raw: &AnimationFileConfig,
    warnings: &mut Vec<String>,
) -> PaneAnimationOverrides {
    PaneAnimationOverrides {
        curve: raw
            .curve
            .as_ref()
            .and_then(|spec| resolve_curve("curve", spec, warnings)),
        close_curve: raw
            .close_curve
            .as_ref()
            .and_then(|spec| resolve_curve("close_curve", spec, warnings)),
        fade: raw.fade,
        scale_from: raw
            .scale_from
            .and_then(|value| bounded("scale_from", value, 0.1, 1.0, warnings)),
        portal_origin: raw.portal_origin.and_then(|value| {
            let x = bounded("portal_origin", value[0], 0.0, 1.0, warnings)?;
            let y = bounded("portal_origin", value[1], 0.0, 1.0, warnings)?;
            Some([x, y])
        }),
        scan_direction: raw
            .scan_direction
            .as_deref()
            .and_then(|value| parse_scan_direction(value, warnings)),
    }
}

fn bounded(key: &str, value: f32, low: f32, high: f32, warnings: &mut Vec<String>) -> Option<f32> {
    if value.is_finite() && (low..=high).contains(&value) {
        Some(value)
    } else {
        warnings.push(format!(
            "Ignored animations.{key} {value}: must be finite and in [{low}, {high}]"
        ));
        None
    }
}

/// A curve is either the name of a builtin easing or CSS cubic-Bezier control points.
fn resolve_curve(key: &str, spec: &CurveSpec, warnings: &mut Vec<String>) -> Option<Easing> {
    match spec {
        CurveSpec::Named(name) => match name.trim().to_ascii_lowercase().as_str() {
            "linear" => Some(Easing::Linear),
            "ease_in_quad" => Some(Easing::EaseInQuad),
            "ease_out_quad" => Some(Easing::EaseOutQuad),
            "ease_in_out_cubic" => Some(Easing::EaseInOutCubic),
            "ease_in_out_sine" => Some(Easing::EaseInOutSine),
            _ => {
                warnings.push(format!(
                    "Ignored animations.{key} \"{name}\" (expected linear, ease_in_quad, \
                     ease_out_quad, ease_in_out_cubic, ease_in_out_sine, or [x1, y1, x2, y2])"
                ));
                None
            }
        },
        CurveSpec::Bezier(values) => resolve_bezier(key, *values, warnings),
    }
}

fn resolve_bezier(key: &str, values: [f32; 4], warnings: &mut Vec<String>) -> Option<Easing> {
    /// CSS pins the control points' x to [0, 1] so the curve stays a function of time. y is free to
    /// leave [0, 1] - that is what produces an overshoot - but not without limit.
    const Y_LIMIT: f32 = 4.0;
    let error = if values.iter().any(|value| !value.is_finite()) {
        Some("all coordinates must be finite".to_string())
    } else if !(0.0..=1.0).contains(&values[0]) || !(0.0..=1.0).contains(&values[2]) {
        Some("x coordinates must be in [0, 1]".to_string())
    } else if values[1].abs() > Y_LIMIT || values[3].abs() > Y_LIMIT {
        Some(format!("y coordinates must be in [-{Y_LIMIT}, {Y_LIMIT}]"))
    } else {
        CubicBezier::new(values[0], values[1], values[2], values[3])
            .err()
            .map(|error| error.to_string())
    };
    match error {
        Some(error) => {
            warnings.push(format!("Ignored animations.{key} {values:?}: {error}"));
            None
        }
        None => CubicBezier::new(values[0], values[1], values[2], values[3])
            .ok()
            .map(Easing::CubicBezier),
    }
}

fn parse_scan_direction(value: &str, warnings: &mut Vec<String>) -> Option<ScanDirection> {
    match value.trim().to_ascii_lowercase().as_str() {
        "top-left" => Some(ScanDirection::TopLeft),
        "top-right" => Some(ScanDirection::TopRight),
        "bottom-left" => Some(ScanDirection::BottomLeft),
        "bottom-right" => Some(ScanDirection::BottomRight),
        _ => {
            warnings.push(format!(
                "Ignored animations.scan_direction \"{value}\" (expected top-left, top-right, \
                 bottom-left, or bottom-right)"
            ));
            None
        }
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
        let mut animations = WindowAnimationConfig::default();
        let mut warnings = Vec::new();
        for style in PaneAnimationStyle::all().iter().copied() {
            let token = style.id().to_ascii_uppercase();
            let raw: AnimationFileConfig =
                toml::from_str(&format!("pane_style = \"{token}\"")).expect("config parses");
            apply_animations(&mut animations, raw, &mut warnings);
            assert_eq!(animations.pane_style, style);
        }
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
        assert!(warnings[0].contains("portal, scan"));
    }
}
