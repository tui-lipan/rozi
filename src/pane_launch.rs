use serde::{Deserialize, Serialize};

/// How a pane's initial child process is launched.
///
/// Shell commands are intentionally distinct from direct argv. Direct execution never joins or
/// quotes its arguments into a command line, so spaces, Unicode, and shell metacharacters retain
/// their literal process-argument meaning on every platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PaneLaunch {
    Shell { command: String },
    Direct { argv: Vec<String> },
}

impl PaneLaunch {
    pub fn shell(command: impl Into<String>) -> Self {
        Self::Shell {
            command: command.into(),
        }
    }

    pub fn direct(argv: Vec<String>) -> Result<Self, String> {
        validate_argv(&argv)?;
        Ok(Self::Direct { argv })
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Shell { .. } => Ok(()),
            Self::Direct { argv } => validate_argv(argv),
        }
    }

    /// Human-facing launch text for rules, events, and diagnostics. Never used for execution.
    pub fn display(&self) -> String {
        match self {
            Self::Shell { command } => command.clone(),
            Self::Direct { argv } => argv.join(" "),
        }
    }

    pub fn shell_command(&self) -> Option<&str> {
        match self {
            Self::Shell { command } => Some(command),
            Self::Direct { .. } => None,
        }
    }

    pub fn argv(&self) -> Option<&[String]> {
        match self {
            Self::Shell { .. } => None,
            Self::Direct { argv } => Some(argv),
        }
    }
}

fn validate_argv(argv: &[String]) -> Result<(), String> {
    let Some(program) = argv.first() else {
        return Err("new-pane argv requires an executable".to_string());
    };
    if program.is_empty() {
        return Err("new-pane argv executable must not be empty".to_string());
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err("new-pane argv must not contain NUL bytes".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_launch_requires_a_real_program_and_preserves_arguments() {
        assert!(PaneLaunch::direct(Vec::new()).is_err());
        assert!(PaneLaunch::direct(vec![String::new()]).is_err());
        let launch = PaneLaunch::direct(vec![
            "printf".into(),
            "space and 'quotes' $stay literal".into(),
        ])
        .unwrap();
        assert_eq!(
            launch.argv(),
            Some(
                ["printf", "space and 'quotes' $stay literal"]
                    .map(String::from)
                    .as_slice()
            )
        );
    }

    #[test]
    fn direct_launch_display_matches_command_oriented_rules_and_events() {
        let launch =
            PaneLaunch::direct(vec!["ssh".into(), "--".into(), "host with spaces".into()]).unwrap();

        assert_eq!(launch.display(), "ssh -- host with spaces");
    }
}
