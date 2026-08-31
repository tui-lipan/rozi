use crate::config::RuleConfig;
use crate::pane::lifecycle::{SpawnFloat, SpawnPlacement};

pub(crate) fn placement_for_command(
    rules: &[RuleConfig],
    command: &str,
) -> (Option<usize>, SpawnPlacement) {
    let Some(rule) = rules.iter().find(|rule| rule.matcher.matches(command)) else {
        return (None, SpawnPlacement::default());
    };
    let float = rule.float.then(|| SpawnFloat {
        width: rule.width.unwrap_or(0.6),
        height: rule.height.unwrap_or(0.6),
        position: rule.position,
        pointer: None,
    });
    (
        rule.workspace,
        SpawnPlacement {
            float,
            fullscreen: rule.fullscreen,
            focus: rule.focus,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::RuleMatcher;
    use regex_lite::Regex;

    fn substring_rule(matches: &str) -> RuleConfig {
        RuleConfig {
            matcher: RuleMatcher::Substring(matches.to_string()),
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
            position: crate::config::FloatPosition::Center,
        }
    }

    fn regex_rule(pattern: &str) -> RuleConfig {
        RuleConfig {
            matcher: RuleMatcher::Regex(Regex::new(pattern).unwrap()),
            float: true,
            width: Some(0.5),
            height: Some(0.5),
            workspace: Some(2),
            focus: false,
            fullscreen: false,
            position: crate::config::FloatPosition::Center,
        }
    }

    #[test]
    fn substring_match_is_case_sensitive_first_match_wins() {
        let rules = vec![substring_rule("top"), substring_rule("btop")];
        // "btop" contains "top", so the first rule wins.
        let (workspace, placement) = placement_for_command(&rules, "btop");
        assert_eq!(workspace, None);
        assert!(placement.float.is_none());
    }

    #[test]
    fn match_regex_avoids_substring_footgun() {
        let rules = vec![regex_rule(r"(^|[^\w])btop($|[^\w])"), substring_rule("top")];
        let (workspace, placement) = placement_for_command(&rules, "btop --version");
        assert_eq!(workspace, Some(2));
        assert!(placement.float.is_some());
        assert!(!placement.focus);

        let (workspace, _) = placement_for_command(&rules, "htop");
        assert_eq!(workspace, None);
    }

    #[test]
    fn float_rule_carries_cursor_position() {
        let mut rule = substring_rule("btop");
        rule.float = true;
        rule.position = crate::config::FloatPosition::Cursor;
        let (_, placement) = placement_for_command(&[rule], "btop");
        let float = placement.float.expect("float");
        assert_eq!(float.position, crate::config::FloatPosition::Cursor);
        assert_eq!(float.pointer, None);
    }
}
