use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tui_lipan::animation::{CubicBezier, Easing};

use crate::layout::anim::{
    AnimationCatalog, AnimationChoice, GlyphPalette, PaneAnimationSpec, PaneAnimationStyle,
    ScanDirection, WindowAnimationConfig, builtin_animation,
};

use super::file::{AnimationFileConfig, CurveFileConfig, PaddingSpec, PaneAnimationFileConfig};

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
    catalog: &AnimationCatalog,
    warnings: &mut Vec<String>,
) {
    apply_animation_durations(target, &raw);
    apply_animation_style(target, raw.pane_style.as_deref(), catalog, warnings);
    apply_animation_flags(target, &raw);
    apply_animation_misc(target, &raw);
}

fn apply_animation_style(
    target: &mut WindowAnimationConfig,
    pane_style: Option<&str>,
    catalog: &AnimationCatalog,
    warnings: &mut Vec<String>,
) {
    let Some(pane_style) = pane_style else { return };
    if let Some(style) = PaneAnimationStyle::parse(pane_style) {
        target.pane_style = style;
        target.pane_animation_id = crate::layout::anim::AnimationId::builtin(style);
        target.pane_animation = builtin_animation(style);
        target.pane_animation.open_duration = target.geometry_duration;
        target.pane_animation.close_duration =
            builtin_close_duration(style, target.close_duration, target.geometry_duration);
    } else if let Some(choice) = catalog
        .choices
        .iter()
        .find(|choice| choice.name == pane_style)
    {
        target.set_selection(choice);
    } else {
        warnings.push(format!(
            "Ignored unknown animations.pane_style \"{pane_style}\" (expected one of: scale, slide, portal, scan, or a valid custom recipe ID)"
        ));
    }
}

fn builtin_close_duration(
    style: PaneAnimationStyle,
    close_duration: Duration,
    geometry_duration: Duration,
) -> Duration {
    if style == PaneAnimationStyle::Scale {
        close_duration
    } else {
        geometry_duration
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

pub(super) fn build_animation_catalog(
    raw: &AnimationFileConfig,
    geometry_duration: Duration,
    close_duration: Duration,
    warnings: &mut Vec<String>,
) -> Arc<AnimationCatalog> {
    let mut catalog = AnimationCatalog::builtin();
    let curves = build_curve_catalog(&raw.curves, warnings);
    for (id, recipe) in &raw.pane_animations {
        if let Some(choice) = build_recipe(
            id,
            recipe,
            &curves,
            geometry_duration,
            close_duration,
            warnings,
        ) {
            catalog.choices.push(choice);
        }
    }
    Arc::new(catalog)
}

fn build_curve_catalog<'a>(
    raw: &'a BTreeMap<String, CurveFileConfig>,
    warnings: &mut Vec<String>,
) -> BTreeMap<&'a str, CubicBezier> {
    raw.iter()
        .filter_map(|(id, curve)| build_curve(id, curve, warnings).map(|value| (&id[..], value)))
        .collect()
}

fn build_curve(
    id: &str,
    curve: &CurveFileConfig,
    warnings: &mut Vec<String>,
) -> Option<CubicBezier> {
    if !valid_animation_id(id) {
        warnings.push(format!("Ignored animations.curves.{id}: ID must be 1..=32 lowercase ASCII letters, digits, '_' or '-'"));
        return None;
    }
    if is_builtin_curve_token(id) {
        warnings.push(format!(
            "Ignored animations.curves.{id}: ID collides with a builtin curve token"
        ));
        return None;
    }
    let Some(values) = curve.bezier else {
        warnings.push(format!(
            "Ignored animations.curves.{id}: missing bezier = [x1, y1, x2, y2]"
        ));
        return None;
    };
    if let Some(error) = validate_curve_values(values) {
        warnings.push(format!("Ignored animations.curves.{id}: {error}"));
        return None;
    }
    match CubicBezier::new(values[0], values[1], values[2], values[3]) {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!("Ignored animations.curves.{id}: {error}"));
            None
        }
    }
}

fn validate_curve_values(values: [f32; 4]) -> Option<&'static str> {
    const Y_LIMIT: f32 = 4.0;
    if values.iter().any(|value| !value.is_finite()) {
        Some("all bezier coordinates must be finite")
    } else if !(0.0..=1.0).contains(&values[0]) || !(0.0..=1.0).contains(&values[2]) {
        Some("bezier x coordinates must be in [0, 1]")
    } else if values[1].abs() > Y_LIMIT || values[3].abs() > Y_LIMIT {
        Some("bezier y coordinates must be in [-4, 4]")
    } else {
        None
    }
}

fn is_builtin_curve_token(id: &str) -> bool {
    matches!(
        id,
        "linear" | "ease_in_quad" | "ease_out_quad" | "ease_in_out_cubic" | "ease_in_out_sine"
    )
}

fn build_recipe(
    id: &str,
    recipe: &PaneAnimationFileConfig,
    curves: &BTreeMap<&str, CubicBezier>,
    geometry_duration: Duration,
    close_duration: Duration,
    warnings: &mut Vec<String>,
) -> Option<AnimationChoice> {
    if !valid_recipe_id(id, warnings) {
        return None;
    }
    let kind = recipe_kind(id, recipe, warnings)?;
    let mut spec = recipe_timing(kind, recipe, geometry_duration, close_duration);
    if !apply_recipe_curves(id, recipe, curves, &mut spec, warnings) {
        return None;
    }
    if !apply_recipe_options(id, kind, recipe, &mut spec, warnings) {
        return None;
    }
    Some(AnimationChoice {
        id: crate::layout::anim::AnimationId::custom(id),
        name: id.to_string(),
        spec,
    })
}

fn valid_recipe_id(id: &str, warnings: &mut Vec<String>) -> bool {
    if !valid_animation_id(id) {
        warnings.push(format!("Ignored animations.pane_animations.{id}: ID must be 1..=32 lowercase ASCII letters, digits, '_' or '-'"));
        false
    } else if PaneAnimationStyle::parse(id).is_some() {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: ID collides with a builtin animation style"
        ));
        false
    } else {
        true
    }
}

fn recipe_kind(
    id: &str,
    recipe: &PaneAnimationFileConfig,
    warnings: &mut Vec<String>,
) -> Option<PaneAnimationStyle> {
    recipe.kind.as_deref().and_then(PaneAnimationStyle::parse).or_else(|| {
        warnings.push(format!("Ignored animations.pane_animations.{id}: kind must be scale, slide, portal, or scan"));
        None
    })
}

fn recipe_timing(
    kind: PaneAnimationStyle,
    recipe: &PaneAnimationFileConfig,
    geometry_duration: Duration,
    close_duration: Duration,
) -> PaneAnimationSpec {
    let mut spec = builtin_animation(kind);
    spec.custom_recipe = true;
    spec.open_duration = recipe
        .open_ms
        .map(Duration::from_millis)
        .unwrap_or(geometry_duration);
    spec.close_duration = recipe
        .close_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| builtin_close_duration(kind, close_duration, geometry_duration));
    spec
}

fn apply_recipe_curves(
    id: &str,
    recipe: &PaneAnimationFileConfig,
    curves: &BTreeMap<&str, CubicBezier>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    if let Some(value) = recipe.curve.as_deref() {
        let Ok((curve, custom)) = resolve_curve(value, curves) else {
            warnings.push(format!(
                "Ignored animations.pane_animations.{id}: unknown curve reference \"{value}\""
            ));
            return false;
        };
        spec.open_curve = curve;
        spec.visual_open_curve = curve;
        let close = if custom {
            reverse_easing(curve)
        } else {
            reverse_builtin_curve(curve)
        };
        spec.close_curve = close;
        spec.visual_close_curve = close;
    }
    if let Some(value) = recipe.close_curve.as_deref() {
        let Ok((curve, _)) = resolve_curve(value, curves) else {
            warnings.push(format!(
                "Ignored animations.pane_animations.{id}: unknown curve reference \"{value}\""
            ));
            return false;
        };
        spec.close_curve = curve;
        spec.visual_close_curve = curve;
    }
    true
}

fn valid_animation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= crate::layout::anim::MAX_ANIMATION_ID_LEN
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

fn resolve_curve(
    value: &str,
    curves: &BTreeMap<&str, CubicBezier>,
) -> Result<(Easing, bool), String> {
    let builtin = match value.to_ascii_lowercase().as_str() {
        "linear" => Some(Easing::Linear),
        "ease_in_quad" => Some(Easing::EaseInQuad),
        "ease_out_quad" => Some(Easing::EaseOutQuad),
        "ease_in_out_cubic" => Some(Easing::EaseInOutCubic),
        "ease_in_out_sine" => Some(Easing::EaseInOutSine),
        _ => None,
    };
    if let Some(builtin) = builtin {
        return Ok((builtin, false));
    }
    curves
        .get(value)
        .copied()
        .map(|curve| (Easing::CubicBezier(curve), true))
        .ok_or_else(|| format!("unknown curve reference \"{value}\""))
}

fn reverse_builtin_curve(curve: Easing) -> Easing {
    match curve {
        Easing::EaseInQuad => Easing::EaseOutQuad,
        Easing::EaseOutQuad => Easing::EaseInQuad,
        other => other,
    }
}

fn reverse_easing(curve: Easing) -> Easing {
    match curve {
        Easing::CubicBezier(curve) => Easing::CubicBezier(curve.reversed()),
        builtin => reverse_builtin_curve(builtin),
    }
}

fn apply_recipe_options(
    id: &str,
    kind: PaneAnimationStyle,
    recipe: &PaneAnimationFileConfig,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    if !apply_scale_option(id, kind, recipe.scale_from, spec, warnings) {
        return false;
    }
    if !apply_portal_origin(id, kind, recipe.origin, spec, warnings) {
        return false;
    }
    if !apply_frontier_option(id, kind, recipe.frontier_width, spec, warnings) {
        return false;
    }
    if !apply_density_option(id, kind, recipe.density, spec, warnings) {
        return false;
    }
    if !apply_glyph_option(id, kind, recipe.glyphs.as_deref(), spec, warnings) {
        return false;
    }
    if !apply_scan_direction(id, kind, recipe.direction.as_deref(), spec, warnings) {
        return false;
    }
    apply_fade_option(id, kind, recipe.fade, spec, warnings)
}

fn relevant_option(
    id: &str,
    kind: PaneAnimationStyle,
    name: &str,
    allowed: bool,
    warnings: &mut Vec<String>,
) -> bool {
    if !allowed {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: {name} is not valid for {kind:?}"
        ));
    }
    allowed
}

fn apply_scale_option(
    id: &str,
    kind: PaneAnimationStyle,
    value: Option<f32>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(value) = value else { return true };
    if !relevant_option(
        id,
        kind,
        "scale_from",
        kind == PaneAnimationStyle::Scale,
        warnings,
    ) {
        return false;
    }
    if !(0.1..=1.0).contains(&value) || !value.is_finite() {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: scale_from must be finite and in [0.1, 1.0]"
        ));
        return false;
    }
    spec.scale_from = value;
    true
}

fn apply_portal_origin(
    id: &str,
    kind: PaneAnimationStyle,
    value: Option<[f32; 2]>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(value) = value else { return true };
    if !relevant_option(
        id,
        kind,
        "origin",
        kind == PaneAnimationStyle::Portal,
        warnings,
    ) {
        return false;
    }
    if value
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: origin values must be finite and in [0, 1]"
        ));
        return false;
    }
    spec.origin = value;
    true
}

fn apply_frontier_option(
    id: &str,
    kind: PaneAnimationStyle,
    value: Option<f32>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(value) = value else { return true };
    if !relevant_option(
        id,
        kind,
        "frontier_width",
        matches!(kind, PaneAnimationStyle::Portal | PaneAnimationStyle::Scan),
        warnings,
    ) {
        return false;
    }
    if !value.is_finite() || !(0.0..=0.5).contains(&value) {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: frontier_width must be finite and in [0, 0.5]"
        ));
        return false;
    }
    spec.frontier_width = value;
    spec.custom_frontier_width = true;
    true
}

fn apply_density_option(
    id: &str,
    kind: PaneAnimationStyle,
    value: Option<f32>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(value) = value else { return true };
    if !relevant_option(
        id,
        kind,
        "density",
        kind == PaneAnimationStyle::Portal,
        warnings,
    ) {
        return false;
    }
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        warnings.push(format!(
            "Ignored animations.pane_animations.{id}: density must be finite and in [0, 1]"
        ));
        return false;
    }
    spec.density = value;
    spec.custom_density = true;
    true
}

fn apply_glyph_option(
    id: &str,
    kind: PaneAnimationStyle,
    glyphs: Option<&[String]>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(glyphs) = glyphs else { return true };
    if !relevant_option(
        id,
        kind,
        "glyphs",
        matches!(kind, PaneAnimationStyle::Portal | PaneAnimationStyle::Scan),
        warnings,
    ) {
        return false;
    }
    let Some(palette) = parse_glyphs(id, glyphs, warnings) else {
        return false;
    };
    spec.glyphs = palette;
    spec.custom_glyphs = true;
    true
}

fn parse_glyphs(id: &str, glyphs: &[String], warnings: &mut Vec<String>) -> Option<GlyphPalette> {
    if glyphs.is_empty() || glyphs.len() > 8 {
        warnings.push(format!("Ignored animations.pane_animations.{id}: glyphs must contain 1..=8 printable ASCII punctuation characters"));
        return None;
    }
    let mut palette_bytes = [0; 8];
    for (index, glyph) in glyphs.iter().enumerate() {
        let bytes = glyph.as_bytes();
        if bytes.len() != 1 || !bytes[0].is_ascii_punctuation() {
            warnings.push(format!("Ignored animations.pane_animations.{id}: glyphs must contain 1..=8 printable ASCII punctuation characters"));
            return None;
        }
        palette_bytes[index] = bytes[0];
    }
    Some(GlyphPalette::custom(&palette_bytes[..glyphs.len()]))
}

fn apply_scan_direction(
    id: &str,
    kind: PaneAnimationStyle,
    direction: Option<&str>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(direction) = direction else {
        return true;
    };
    if !relevant_option(
        id,
        kind,
        "direction",
        kind == PaneAnimationStyle::Scan,
        warnings,
    ) {
        return false;
    }
    let Some(direction) = parse_scan_direction(direction) else {
        warnings.push(format!("Ignored animations.pane_animations.{id}: direction must be top-left, top-right, bottom-left, or bottom-right"));
        return false;
    };
    spec.scan_direction = direction;
    true
}

fn parse_scan_direction(value: &str) -> Option<ScanDirection> {
    match value.to_ascii_lowercase().as_str() {
        "top-left" => Some(ScanDirection::TopLeft),
        "top-right" => Some(ScanDirection::TopRight),
        "bottom-left" => Some(ScanDirection::BottomLeft),
        "bottom-right" => Some(ScanDirection::BottomRight),
        _ => None,
    }
}

fn apply_fade_option(
    id: &str,
    kind: PaneAnimationStyle,
    value: Option<bool>,
    spec: &mut PaneAnimationSpec,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(value) = value else { return true };
    if !relevant_option(
        id,
        kind,
        "fade",
        kind != PaneAnimationStyle::Slide,
        warnings,
    ) {
        return false;
    }
    spec.fade = value;
    true
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
        let catalog = build_animation_catalog(
            &raw,
            animations.geometry_duration,
            animations.close_duration,
            &mut warnings,
        );
        apply_animations(&mut animations, raw, &catalog, &mut warnings);
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
            let catalog = build_animation_catalog(
                &raw,
                animations.geometry_duration,
                animations.close_duration,
                &mut warnings,
            );
            apply_animations(&mut animations, raw, &catalog, &mut warnings);
            assert_eq!(animations.pane_style, style);
        }
        assert!(warnings.is_empty());

        let raw: AnimationFileConfig =
            toml::from_str("pane_style = \"springy\"").expect("config parses");
        let mut animations = WindowAnimationConfig::default();
        let catalog = build_animation_catalog(
            &raw,
            animations.geometry_duration,
            animations.close_duration,
            &mut warnings,
        );
        apply_animations(&mut animations, raw, &catalog, &mut warnings);
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
