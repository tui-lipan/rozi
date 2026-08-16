use std::collections::HashSet;

use crate::input::Action;

use super::file::NamedCommandFileConfig;
use super::input::parse_user_command_action;
use super::schema::NamedCommand;

pub(crate) fn build_named_commands(
    raw: Vec<NamedCommandFileConfig>,
    warnings: &mut Vec<String>,
) -> Vec<NamedCommand> {
    let mut commands = Vec::new();
    let mut seen = HashSet::new();

    for command in raw {
        let Some(id) = command
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
        else {
            warnings.push("named command missing required field `id`; skipped".to_string());
            continue;
        };
        if ["workspace.", "user.", "app.", "rozi."]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        {
            warnings.push(format!(
                "named command id `{id}` uses a reserved prefix; skipped"
            ));
            continue;
        }
        if id.contains('.') {
            warnings.push(format!(
                "named command id `{id}` contains a reserved `.`; skipped"
            ));
            continue;
        }
        if !valid_command_segment(&id) {
            warnings.push(format!(
                "named command id `{id}` must match [a-z0-9_-]+; skipped"
            ));
            continue;
        }
        if Action::from_id(&id).is_some() {
            warnings.push(format!(
                "named command id `{id}` collides with a built-in action; skipped"
            ));
            continue;
        }
        if !seen.insert(id.clone()) {
            warnings.push(format!("duplicate named command id `{id}`; skipped"));
            continue;
        }

        let label = command
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string);
        let Some(action) =
            parse_user_command_action(command.action(), &format!("Named command `{id}`"), warnings)
        else {
            continue;
        };
        commands.push(NamedCommand {
            id,
            label,
            action,
            category: "Custom".to_string(),
            env: Vec::new(),
        });
    }

    commands
}

pub(crate) fn valid_command_segment(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: Option<&str>, run: Option<&str>) -> NamedCommandFileConfig {
        NamedCommandFileConfig {
            id: id.map(str::to_string),
            run: run.map(str::to_string),
            ..NamedCommandFileConfig::default()
        }
    }

    #[test]
    fn valid_named_command_builds() {
        let mut warnings = Vec::new();
        let commands = build_named_commands(
            vec![raw(Some("branches"), Some("git branch"))],
            &mut warnings,
        );
        assert!(warnings.is_empty());
        assert_eq!(commands[0].id, "branches");
        assert_eq!(commands[0].category, "Custom");
    }

    #[test]
    fn invalid_ids_are_dropped() {
        for id in [
            None,
            Some(""),
            Some("Bad"),
            Some("has.dot"),
            Some("has space"),
        ] {
            let mut warnings = Vec::new();
            assert!(build_named_commands(vec![raw(id, Some("true"))], &mut warnings).is_empty());
            assert_eq!(warnings.len(), 1, "{id:?}");
        }
    }

    #[test]
    fn builtin_collision_is_dropped() {
        let mut warnings = Vec::new();
        assert!(
            build_named_commands(vec![raw(Some("spawn"), Some("true"))], &mut warnings).is_empty()
        );
        assert!(warnings[0].contains("built-in"));
    }

    #[test]
    fn reserved_prefix_is_dropped() {
        let mut warnings = Vec::new();
        assert!(
            build_named_commands(
                vec![raw(Some("workspace.custom"), Some("true"))],
                &mut warnings
            )
            .is_empty()
        );
        assert!(warnings[0].contains("reserved prefix"));
    }

    #[test]
    fn duplicate_id_is_dropped() {
        let mut warnings = Vec::new();
        let commands = build_named_commands(
            vec![
                raw(Some("same"), Some("one")),
                raw(Some("same"), Some("two")),
            ],
            &mut warnings,
        );
        assert_eq!(commands.len(), 1);
        assert!(warnings[0].contains("duplicate"));
    }

    #[test]
    fn missing_or_conflicting_verb_is_dropped() {
        let mut warnings = Vec::new();
        assert!(build_named_commands(vec![raw(Some("empty"), None)], &mut warnings).is_empty());
        let mut conflicting = raw(Some("conflict"), Some("one"));
        conflicting.exec = Some("two".to_string());
        assert!(build_named_commands(vec![conflicting], &mut warnings).is_empty());
        assert_eq!(warnings.len(), 2);
    }
}
