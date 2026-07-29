use tui_lipan::prelude::CapStyle as TuiCapStyle;

use super::file::{PaneFileConfig, WorkbarFileConfig, WorkbarSegmentSpec};
use super::schema::{BadgeColor, HyprmuxPaneConfig, WorkbarConfig, WorkbarItem, WorkbarSegment};
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
}

pub(super) fn apply_workbar_style_config(
    config: &mut HyprmuxPaneConfig,
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
    fn workbar_powerline_parses_and_applies() {
        let parsed: PaneFileConfig =
            toml::from_str("workbar_powerline = false").expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);
        assert!(warnings.is_empty());
        assert!(!pane.workbar_powerline);
    }

    #[test]
    fn workbar_badge_style_backfills_workbar_tabs_when_tabs_are_unset() {
        let parsed: PaneFileConfig =
            toml::from_str(r#"workbar_badge_style = "arrow""#).expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
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
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, TuiCapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, TuiCapStyle::Round);
    }
}
