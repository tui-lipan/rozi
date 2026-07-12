use crate::config::HyprmuxRuleConfig;
use crate::pane_lifecycle::{SpawnFloat, SpawnPlacement};

pub(crate) fn placement_for_command(
    rules: &[HyprmuxRuleConfig],
    command: &str,
) -> (Option<usize>, SpawnPlacement) {
    let Some(rule) = rules.iter().find(|rule| command.contains(&rule.matches)) else {
        return (None, SpawnPlacement::default());
    };
    let float = rule.float.then(|| SpawnFloat {
        width: rule.width.unwrap_or(0.6),
        height: rule.height.unwrap_or(0.6),
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

    fn rule(matches: &str) -> HyprmuxRuleConfig {
        HyprmuxRuleConfig {
            matches: matches.into(),
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
        }
    }

    #[test]
    fn substring_first_match_wins_and_remaps_workspace() {
        let mut first = rule("cargo");
        first.workspace = Some(8);
        first.focus = false;
        let mut second = rule("cargo watch");
        second.workspace = Some(2);
        let (workspace, placement) = placement_for_command(&[first, second], "cargo watch -x test");
        assert_eq!(workspace, Some(8));
        assert!(!placement.focus);
    }

    #[test]
    fn defaults_apply_when_no_rule_matches() {
        assert_eq!(
            placement_for_command(&[rule("btop")], "bash"),
            (None, SpawnPlacement::default())
        );
    }

    #[test]
    fn floating_rule_uses_dimensions_and_flags() {
        let mut configured = rule("btop");
        configured.float = true;
        configured.width = Some(0.7);
        configured.height = Some(0.8);
        configured.fullscreen = true;
        let (_, placement) = placement_for_command(&[configured], "exec btop");
        assert_eq!(
            placement.float,
            Some(SpawnFloat {
                width: 0.7,
                height: 0.8
            })
        );
        assert!(placement.fullscreen);
    }
}
