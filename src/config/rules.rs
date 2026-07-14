use super::file::RuleFileConfig;
use super::schema::HyprmuxRuleConfig;

pub(super) fn build_rules(
    raw: Vec<RuleFileConfig>,
    warnings: &mut Vec<String>,
) -> Vec<HyprmuxRuleConfig> {
    raw.into_iter()
        .filter_map(|rule| {
            let matches = rule.matches.trim().to_string();
            if matches.is_empty() {
                warnings.push("Ignored rule with an empty match".to_string());
                return None;
            }
            let clamp = |name: &str, value: Option<f32>, warnings: &mut Vec<String>| {
                value.map(|value| {
                    let clamped = value.clamp(0.1, 1.0);
                    if (clamped - value).abs() > f32::EPSILON {
                        warnings.push(format!(
                            "Rule `{matches}` {name} {value} out of range; clamped to {clamped}"
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
                        "Ignored rule `{matches}` workspace {workspace} (expected 1..={})",
                        crate::state::WORKSPACE_COUNT
                    ));
                    None
                }
            });
            Some(HyprmuxRuleConfig {
                width: clamp("width", rule.width, warnings),
                height: clamp("height", rule.height, warnings),
                matches,
                float: rule.float,
                workspace,
                focus: rule.focus,
                fullscreen: rule.fullscreen,
            })
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
        assert_eq!(warnings.len(), 2);
    }

    #[test]
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
}
