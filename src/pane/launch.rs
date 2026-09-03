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

/// What a pane's terminal reports about the program in its foreground right now, reduced to the
/// fields that decide whether that program is worth writing down and replaying.
///
/// Both capture paths - profile save (client-side, from `TerminalPane`) and resurrection snapshot
/// (server-side, from `PaneRuntimeState`) - describe the same thing through different structs, so
/// they meet here rather than growing two answers to one question.
pub struct ForegroundSnapshot<'a> {
    pub command_phase: crate::session::protocol::PaneCommandPhase,
    pub program: Option<&'a str>,
    pub executable: Option<&'a str>,
    pub arguments: &'a [String],
    /// The pane's directory names a filesystem on another host, so neither the resolved executable
    /// path nor the arguments describe anything on the machine that would replay them.
    pub remote: bool,
}

/// The command a pane is running *right now*, as a line an interactive shell would accept, if it
/// is worth replaying on restore.
///
/// `foreground_program` keeps reporting the last executed command's executable while the shell
/// sits idle at a prompt (OSC 133 `rozi_exe=` is only replaced by the next command), so a pane
/// where the user merely changed directories would otherwise capture stale prompt machinery like
/// `__zoxide_hook` and replay it as a pane command. Only trust it while shell integration reports
/// a command mid-flight (`Executing`), or when there is no integration at all (`Unknown`) and the
/// value comes from the process inspector, which reads the live foreground process group.
///
/// What is captured is the whole invocation, not just the program: an agent started with
/// `--dangerously-skip-permissions` is a different pane from the same agent without it. The
/// program is named where a name is enough to find it again and given as a path where it is not -
/// one started through an alias, or straight out of a build tree, restores as `command not found`
/// if only its name is written down.
///
/// Neither the path nor the arguments belong to a pane attached over `--remote`: both describe a
/// process on the far host, which the machine doing the restoring is not. Those panes keep the
/// bare program name, as they did before either was captured.
pub fn replayable_foreground_command(
    snapshot: ForegroundSnapshot<'_>,
    shells: &std::collections::HashSet<String>,
) -> Option<String> {
    use crate::session::protocol::PaneCommandPhase;

    match snapshot.command_phase {
        PaneCommandPhase::Executing | PaneCommandPhase::Unknown => {}
        PaneCommandPhase::Prompt | PaneCommandPhase::Input | PaneCommandPhase::Completed { .. } => {
            return None;
        }
    }
    let program = snapshot.program.filter(|program| {
        !shells.contains(&crate::platform::command::normalized_program_name(program))
    })?;
    if snapshot.remote {
        return Some(program.to_string());
    }
    let program = match snapshot.executable {
        Some(path) => shell_quote(path),
        None => program.to_string(),
    };
    Some(
        std::iter::once(program)
            .chain(
                snapshot
                    .arguments
                    .iter()
                    .map(|argument| shell_quote(argument)),
            )
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Quote `word` so an interactive shell reads it as one literal argument, since the captured
/// command is replayed by typing it at a prompt. Only reached for inspector-reported paths and
/// arguments, which exist on Unix alone - a Windows pane has neither.
pub fn shell_quote(word: &str) -> String {
    let plain = |ch: char| ch.is_ascii_alphanumeric() || "_-./:@%+=".contains(ch);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Program names that mean "this pane is sitting in a shell", not "this pane is running something".
///
/// A configured or resolved interactive shell is added to this by each caller, which knows its own;
/// the list itself is the part neither side should be maintaining separately.
pub fn common_shell_basenames() -> std::collections::HashSet<String> {
    [
        "bash",
        "zsh",
        "fish",
        "sh",
        "dash",
        "ksh",
        "tcsh",
        "csh",
        "nu",
        "pwsh",
        "powershell",
        "cmd",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
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
