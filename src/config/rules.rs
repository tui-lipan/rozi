use regex_lite::Regex;

use super::file::{HintFileConfig, RuleFileConfig};
use super::schema::{HyprmuxHintConfig, HyprmuxRuleConfig, RuleMatcher};

pub(super) fn build_rules(
    raw: Vec<RuleFileConfig>,
    warnings: &mut Vec<String>,
) -> Vec<HyprmuxRuleConfig> {
    raw.into_iter()
        .filter_map(|rule| {
            let matches = rule.matches.trim().to_string();
            let match_regex = rule
                .match_regex
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());

            let matcher = match (!matches.is_empty(), match_regex) {
                (true, None) => RuleMatcher::Substring(matches.clone()),
                (false, Some(pattern)) => match Regex::new(&pattern) {
                    Ok(regex) => RuleMatcher::Regex(regex),
                    Err(err) => {
                        warnings.push(format!(
                            "Ignored rule with invalid match_regex `{pattern}`: {err}"
                        ));
                        return None;
                    }
                },
                (false, None) => {
                    warnings.push(
                        "Ignored rule with neither `match` nor `match_regex` set".to_string(),
                    );
                    return None;
                }
                (true, Some(_)) => {
                    warnings.push(format!(
                        "Ignored rule `{matches}`: set exactly one of `match` or `match_regex`"
                    ));
                    return None;
                }
            };

            let label = matcher.label();
            let clamp = |name: &str, value: Option<f32>, warnings: &mut Vec<String>| {
                value.map(|value| {
                    let clamped = value.clamp(0.1, 1.0);
                    if (clamped - value).abs() > f32::EPSILON {
                        warnings.push(format!(
                            "Rule `{label}` {name} {value} out of range; clamped to {clamped}"
                        ));
                    }
                    clamped
                })
            };
            let workspace = rule.workspace.and_then(|workspace| {
                if (1..=crate::state::WORKSPACE_COUNT).contains(&workspace) {
                    Some(workspace - 1)
                } else {
                    warnings.push(format!(
                        "Ignored rule `{label}` workspace {workspace} (expected 1..={})",
                        crate::state::WORKSPACE_COUNT
                    ));
                    None
                }
            });
            Some(HyprmuxRuleConfig {
                width: clamp("width", rule.width, warnings),
                height: clamp("height", rule.height, warnings),
                matcher,
                float: rule.float,
                workspace,
                focus: rule.focus,
                fullscreen: rule.fullscreen,
            })
        })
        .collect()
}

pub(super) fn build_hints(
    raw: Vec<HintFileConfig>,
    warnings: &mut Vec<String>,
) -> Vec<HyprmuxHintConfig> {
    raw.into_iter()
        .filter_map(|hint| {
            let pattern = hint.pattern.trim().to_string();
            if pattern.is_empty() {
                warnings.push("Ignored hint with an empty pattern".to_string());
                return None;
            }
            match Regex::new(&pattern) {
                Ok(regex) => Some(HyprmuxHintConfig {
                    pattern: regex,
                    open: hint.open,
                }),
                Err(err) => {
                    warnings.push(format!(
                        "Ignored hint with invalid pattern `{pattern}`: {err}"
                    ));
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct RulesOnly {
        rules: Vec<RuleFileConfig>,
    }

    #[derive(Deserialize)]
    struct HintsOnly {
        hints: Vec<HintFileConfig>,
    }

    #[test]
    fn rules_parse_and_merge_with_clamps_and_workspace_remap() {
        let parsed: RulesOnly = toml::from_str(
            r#"
            [[rules]]
            match = "btop"
            float = true
            width = 2.0
            height = 0.05
            workspace = 9
            focus = false
            fullscreen = true
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let rules = build_rules(parsed.rules, &mut warnings);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].width, Some(1.0));
        assert_eq!(rules[0].height, Some(0.1));
        assert_eq!(rules[0].workspace, Some(8));
        assert!(!rules[0].focus);
        assert!(rules[0].fullscreen);
        assert!(warnings.iter().any(|w| w.contains("width")));
        assert!(warnings.iter().any(|w| w.contains("height")));
    }

    #[test]
    fn rules_require_exactly_one_matcher() {
        let parsed: RulesOnly = toml::from_str(
            r#"
            [[rules]]
            match = "btop"
            match_regex = "btop$"
            [[rules]]
            float = true
            [[rules]]
            match_regex = "("
            [[rules]]
            match_regex = "^cargo\\s"
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let rules = build_rules(parsed.rules, &mut warnings);
        assert_eq!(rules.len(), 1);
        assert!(matches!(rules[0].matcher, RuleMatcher::Regex(_)));
        assert_eq!(warnings.len(), 3);
    }

    fn invalid_rules_are_skipped_or_lose_invalid_workspace() {
        let parsed: RulesOnly = toml::from_str(
            r#"
            [[rules]]
            match = ""
            [[rules]]
            match = "cargo watch"
            workspace = 10
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let rules = build_rules(parsed.rules, &mut warnings);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].workspace, None);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn build_hints_warns_and_skips_invalid_patterns() {
        let parsed: HintsOnly = toml::from_str(
            r#"
            [[hints]]
            pattern = ""
            [[hints]]
            pattern = "("
            [[hints]]
            pattern = "\\b(?:[0-9]{1,3}\\.){3}[0-9]{1,3}\\b"
            open = true
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let hints = build_hints(parsed.hints, &mut warnings);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].open);
        assert_eq!(warnings.len(), 2);
    }
}
