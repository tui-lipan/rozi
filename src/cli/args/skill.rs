use super::{ParsedCli, SkillCommand};

pub(super) fn parse_skill_args(args: &[String]) -> std::result::Result<ParsedCli, String> {
    let mut global = false;
    let mut command = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::SkillHelp),
            "--global" => {
                if global {
                    return Err("--global specified more than once".to_string());
                }
                global = true;
            }
            "install" | "uninstall" | "status" | "print" if command.is_none() => {
                command = Some(arg.as_str());
            }
            other if command.is_none() => {
                return Err(format!("unknown skill command `{other}`"));
            }
            other => {
                return Err(format!("unexpected argument `{other}` after skill"));
            }
        }
    }
    match command {
        None => {
            if global {
                return Err("--global requires a skill command".to_string());
            }
            Ok(ParsedCli::SkillHelp)
        }
        Some("print") => {
            if global {
                return Err("skill print does not accept --global".to_string());
            }
            Ok(ParsedCli::Skill(SkillCommand::Print))
        }
        Some("install") => Ok(ParsedCli::Skill(SkillCommand::Install { global })),
        Some("uninstall") => Ok(ParsedCli::Skill(SkillCommand::Uninstall { global })),
        Some("status") => Ok(ParsedCli::Skill(SkillCommand::Status { global })),
        Some(other) => Err(format!("unknown skill command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_cli_args;
    use super::*;

    #[test]
    fn cli_skill_is_a_strict_early_variant() {
        assert!(matches!(
            parse_cli_args(vec!["--skill".into()]).expect("parses"),
            ParsedCli::Skill(SkillCommand::Print)
        ));
        assert!(parse_cli_args(vec!["--skill".into(), "extra".into()]).is_err());
        assert!(parse_cli_args(vec!["target".into(), "--skill".into()]).is_err());
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "print".into()]).expect("parses"),
            ParsedCli::Skill(SkillCommand::Print)
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into()]).expect("parses"),
            ParsedCli::SkillHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "-h".into()]).expect("parses"),
            ParsedCli::SkillHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "install".into(), "--global".into()])
                .expect("parses"),
            ParsedCli::Skill(SkillCommand::Install { global: true })
        ));
    }
}
