use tui_lipan::prelude::CapStyle as TuiCapStyle;

use super::file::{PaneFileConfig, WorkbarFileConfig, WorkbarSegmentSpec};
use super::schema::{
    BadgeColor, PaneAlertColors, PaneConfig, WorkbarConfig, WorkbarItem, WorkbarSegment,
};
use crate::state::parse_cap_style;

pub(super) fn apply_workbar_config(
    workbar: &mut WorkbarConfig,
    raw: WorkbarFileConfig,
    warnings: &mut Vec<String>,
) {
    fn parse_segments(
        raw: Vec<WorkbarSegmentSpec>,
        region: &str,
        warnings: &mut Vec<String>,
    ) -> Vec<WorkbarItem> {
        raw.into_iter()
            .filter_map(|spec| {
                let (name, color_name) = match spec {
                    WorkbarSegmentSpec::Name(name) => (name, None),
                    WorkbarSegmentSpec::Table { segment, color } => (segment, color),
                };
                let segment = match WorkbarSegment::parse(&name) {
                    Some(segment) => segment,
                    None => {
                        warnings.push(format!(
                            "Unknown {region} workbar segment `{name}`; skipped"
                        ));
                        return None;
                    }
                };
                // An unknown role falls back to the segment's curated default.
                let color = match color_name {
                    Some(color_name) => match BadgeColor::parse(&color_name) {
                        Some(color) => Some(color),
                        None => {
                            warnings.push(format!(
                                "Unknown {region} workbar color `{color_name}` for `{name}` (expected one of: {}); using default",
                                BadgeColor::NAMES
                            ));
                            None
                        }
                    },
                    None => None,
                };
                Some(WorkbarItem { segment, color })
            })
            .collect()
    }

    if let Some(left) = raw.left {
        workbar.left = parse_segments(left, "left", warnings);
    }
    if let Some(right) = raw.right {
        workbar.right = parse_segments(right, "right", warnings);
    }
    if let Some(format) = super::file::non_empty(raw.clock_format) {
        // Reject invalid strftime so a clock segment can't panic at render time.
        if chrono::format::StrftimeItems::new(&format).parse().is_ok() {
            workbar.clock_format = format;
        } else {
            warnings.push(format!(
                "Invalid clock_format `{format}`; keeping `{}`",
                workbar.clock_format
            ));
        }
    }
    if let Some(value) = raw.alert.bell {
        workbar.alert.bell = value;
    }
    if let Some(value) = raw.alert.blocked {
        workbar.alert.blocked = value;
    }
    if let Some(value) = raw.alert.finished {
        workbar.alert.finished = value;
    }
    if let Some(value) = raw.alert.working {
        workbar.alert.working = value;
    }
    if let Some(value) = raw.alert.idle {
        workbar.alert.idle = value;
    }
    if let Some(value) = super::file::non_empty(raw.alert.mode) {
        match crate::state::AlertMode::parse(&value) {
            Some(mode) => workbar.alert.mode = mode,
            None => warnings.push(format!(
                "Unknown workbar alert mode `{value}` (expected one of: off, static, pulse); using `{}`",
                workbar.alert.mode.id()
            )),
        }
    }
    if let Some(value) = super::file::non_empty(raw.alert.paint) {
        match crate::state::AlertPaint::parse(&value) {
            Some(paint) => workbar.alert.paint = paint,
            None => warnings.push(format!(
                "Unknown workbar alert paint `{value}` (expected one of: background, text); using `{}`",
                workbar.alert.paint.id()
            )),
        }
    }
}

pub(super) fn apply_pane_alert_colors(
    colors: &mut PaneAlertColors,
    raw: super::file::PaneAlertFileConfig,
    warnings: &mut Vec<String>,
) {
    fn apply(
        target: &mut Option<BadgeColor>,
        value: Option<String>,
        state: &str,
        warnings: &mut Vec<String>,
    ) {
        let Some(value) = value else {
            return;
        };
        if value.trim().eq_ignore_ascii_case("off") {
            *target = None;
        } else if let Some(color) = BadgeColor::parse(&value) {
            *target = Some(color);
        } else {
            warnings.push(format!(
                "Unknown pane alert color `{value}` for `{state}` (expected one of: {}, off); using default",
                BadgeColor::NAMES
            ));
        }
    }

    apply(&mut colors.blocked, raw.blocked, "blocked", warnings);
    apply(&mut colors.finished, raw.finished, "finished", warnings);
    apply(&mut colors.working, raw.working, "working", warnings);
    apply(&mut colors.idle, raw.idle, "idle", warnings);
}

pub(super) fn apply_workbar_style_config(
    config: &mut PaneConfig,
    parsed: &PaneFileConfig,
    warnings: &mut Vec<String>,
) {
    if let Some(workbar_badge_style) = parsed.workbar_badge_style.as_deref() {
        match parse_cap_style(workbar_badge_style) {
            Some(TuiCapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_badge_style \"{workbar_badge_style}\" (half block is not available for workbar badges)"
            )),
            Some(style) => {
                config.workbar_badge_style = style;
                if parsed.workbar_tab_style.is_none() {
                    config.workbar_tab_style = style;
                }
            }
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_badge_style \"{workbar_badge_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_tab_style) = parsed.workbar_tab_style.as_deref() {
        match parse_cap_style(workbar_tab_style) {
            Some(TuiCapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_tab_style \"{workbar_tab_style}\" (half block is not available for tab bars)"
            )),
            Some(style) => config.workbar_tab_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_tab_style \"{workbar_tab_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_style) = parsed.workbar_style.as_deref() {
        match parse_cap_style(workbar_style) {
            Some(style) => config.workbar_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_style \"{workbar_style}\" (expected one of: padded, half, round, arrow)"
            )),
        }
    }
    if let Some(workbar_powerline) = parsed.workbar_powerline {
        config.workbar_powerline = workbar_powerline;
    }
    if let Some(opacity) = parsed.toast_opacity {
        if opacity.is_finite() && (0.0..=1.0).contains(&opacity) {
            config.toast_opacity = opacity;
        } else {
            warnings.push(format!(
                "Ignored pane.toast_opacity {opacity} (expected a number between 0.0 and 1.0)"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::prelude::CapStyle as TuiCapStyle;

    #[test]
    fn workbar_segment_table_form_overrides_color() {
        let raw: WorkbarFileConfig =
            toml::from_str(r#"right = [{ segment = "clock", color = "info" }, "session"]"#)
                .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, raw, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            workbar.right,
            vec![
                WorkbarItem {
                    segment: WorkbarSegment::Clock,
                    color: Some(BadgeColor::Info),
                },
                WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                },
            ]
        );
    }

    #[test]
    fn workbar_unknown_color_warns_and_falls_back_to_default() {
        let raw: WorkbarFileConfig =
            toml::from_str(r#"right = [{ segment = "clock", color = "chartreuse" }]"#)
                .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, raw, &mut warnings);
        assert_eq!(
            workbar.right,
            vec![WorkbarItem {
                segment: WorkbarSegment::Clock,
                color: None,
            }]
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn workbar_alert_config_overrides_each_trigger() {
        let raw: WorkbarFileConfig = toml::from_str(
            "[alert]\nbell = false\nblocked = false\nfinished = false\nworking = true\nidle = true\nmode = \"static\"",
        )
        .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        apply_workbar_config(&mut workbar, raw, &mut Vec::new());
        assert_eq!(
            workbar.alert,
            super::super::schema::WorkbarAlertConfig {
                bell: false,
                blocked: false,
                finished: false,
                working: true,
                idle: true,
                mode: crate::state::AlertMode::Static,
                paint: crate::state::AlertPaint::Background,
            }
        );
    }

    #[test]
    fn workbar_alert_mode_warns_and_keeps_the_default_when_unknown() {
        let raw: WorkbarFileConfig =
            toml::from_str("[alert]\nmode = \"blinky\"").expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, raw, &mut warnings);
        assert_eq!(workbar.alert.mode, crate::state::AlertMode::Pulse);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("blinky"), "{warnings:?}");
    }

    #[test]
    fn pane_alert_colors_parse_roles_and_preserve_invalid_defaults() {
        let raw: PaneFileConfig = toml::from_str(
            "[alert]\nblocked = \"warning\"\nfinished = \"off\"\nworking = \"info\"\nidle = \"chartreuse\"",
        )
        .expect("config parses");
        let mut colors = PaneAlertColors::default();
        let mut warnings = Vec::new();
        apply_pane_alert_colors(&mut colors, raw.alert, &mut warnings);
        assert_eq!(colors.blocked, Some(BadgeColor::Warning));
        assert_eq!(colors.finished, None);
        assert_eq!(colors.working, Some(BadgeColor::Info));
        assert_eq!(colors.idle, None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains(BadgeColor::NAMES));
    }

    #[test]
    fn workbar_powerline_parses_and_applies() {
        let parsed: PaneFileConfig =
            toml::from_str("workbar_powerline = false").expect("config parses");
        let mut pane = PaneConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);
        assert!(warnings.is_empty());
        assert!(!pane.workbar_powerline);
    }

    #[test]
    fn toast_opacity_defaults_to_glass_and_accepts_a_unit_fraction() {
        assert_eq!(
            PaneConfig::default().toast_opacity,
            0.8,
            "toasts default to tinted glass rather than a solid panel",
        );

        let parsed: PaneFileConfig = toml::from_str("toast_opacity = 0.82").expect("config parses");
        let mut pane = PaneConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(pane.toast_opacity, 0.82);
    }

    #[test]
    fn toast_opacity_outside_the_unit_range_warns_and_keeps_the_default() {
        for raw in ["toast_opacity = 1.5", "toast_opacity = -0.2"] {
            let parsed: PaneFileConfig = toml::from_str(raw).expect("config parses");
            let mut pane = PaneConfig::default();
            let mut warnings = Vec::new();
            apply_workbar_style_config(&mut pane, &parsed, &mut warnings);
            assert_eq!(warnings.len(), 1, "{raw} should warn");
            assert_eq!(pane.toast_opacity, 0.8, "{raw} must not take effect");
        }
    }

    #[test]
    fn workbar_badge_style_backfills_workbar_tabs_when_tabs_are_unset() {
        let parsed: PaneFileConfig =
            toml::from_str(r#"workbar_badge_style = "arrow""#).expect("config parses");
        let mut pane = PaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, TuiCapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, TuiCapStyle::Arrow);
    }

    #[test]
    fn explicit_workbar_tab_style_overrides_only_tabs() {
        let parsed: PaneFileConfig = toml::from_str(
            r#"
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            "#,
        )
        .expect("config parses");
        let mut pane = PaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, TuiCapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, TuiCapStyle::Round);
    }
}
