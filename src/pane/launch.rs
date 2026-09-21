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

/// How a shell reads a quoted word.
///
/// These families are not cosmetic variations of one another. `cmd.exe` gives single quotes no
/// meaning at all, so a POSIX-quoted line typed there is a *different command* rather than an ugly
/// one; PowerShell doubles an embedded apostrophe where POSIX closes, escapes, and reopens; and
/// fish treats backslash as an escape inside single quotes where POSIX leaves it alone. Anything
/// rendered for a human to press Enter on has to know which of these it is writing for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptQuoting {
    /// `sh`, `bash`, `zsh`, `dash`, `ksh`: `'…'` is literal, and an embedded `'` closes, escapes,
    /// and reopens.
    Posix,
    /// `fish`: `'…'` is literal too, but `\` and `'` are the two escapes inside it, so a backslash
    /// has to be doubled where POSIX leaves it alone.
    Fish,
    /// PowerShell: `'…'` is literal and an embedded `'` is doubled.
    PowerShell,
    /// `cmd.exe`: single quotes mean nothing. `"…"` groups, and an embedded `"` is doubled.
    Cmd,
}

impl PromptQuoting {
    /// Pick the rules from the shell a pane is actually running.
    ///
    /// An unrecognized program falls back to what an unknown shell on that platform is
    /// overwhelmingly likely to be, which is the safer guess than assuming POSIX everywhere: on
    /// Windows a wrong POSIX guess silently changes the command, while on Unix a wrong `cmd` guess
    /// would do the same in reverse.
    pub fn of(shell: Option<&str>) -> Self {
        let name = shell.map(crate::platform::command::normalized_program_name);
        match name.as_deref() {
            Some("fish") => Self::Fish,
            Some("powershell" | "pwsh") => Self::PowerShell,
            Some("cmd") => Self::Cmd,
            Some("sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "busybox") => Self::Posix,
            _ if cfg!(windows) => Self::Cmd,
            _ => Self::Posix,
        }
    }

    /// Characters that need no quoting in any of these shells.
    fn is_plain(self, ch: char) -> bool {
        if ch.is_ascii_alphanumeric() {
            return true;
        }
        match self {
            // `\` is plain because a Windows path is mostly backslashes and `cmd` gives them no
            // special meaning. `%` and `!` are deliberately *absent*: quoting cannot stop either
            // from expanding, but excluding them at least sends the word through the quoted branch
            // so it stays one argument, and `hold` never submits a line unseen.
            Self::Cmd => "_-./:\\".contains(ch),
            _ => "_-./:@%+=".contains(ch),
        }
    }
}

/// Quote `word` so a prompt running `quoting`'s shell reads it as one literal argument.
///
/// Best effort on `cmd.exe` alone, where `%VAR%` and `!VAR!` expand inside double quotes and the
/// interactive prompt offers no escape for either. That is survivable because the only caller that
/// can reach `cmd` is the held-command path, which writes the line and waits for a person to read
/// it before pressing Enter.
pub fn quote_for_prompt(word: &str, quoting: PromptQuoting) -> String {
    if !word.is_empty() && word.chars().all(|ch| quoting.is_plain(ch)) {
        return word.to_string();
    }
    match quoting {
        PromptQuoting::Posix => format!("'{}'", word.replace('\'', r"'\''")),
        PromptQuoting::Fish => format!("'{}'", word.replace('\\', r"\\").replace('\'', r"\'")),
        PromptQuoting::PowerShell => format!("'{}'", word.replace('\'', "''")),
        PromptQuoting::Cmd => format!("\"{}\"", word.replace('"', "\"\"")),
    }
}

/// Render `argv` as one line the shell behind `quoting` runs as exactly that command.
///
/// Quoting each word is not sufficient on PowerShell: a quoted word in command position is a
/// *string expression*, so a line starting `'C:\Program Files\claude'` prints the path instead of
/// running it. The call operator is what makes it an invocation, and it is harmless on a bare name,
/// so it is always emitted rather than only when the program needed quoting.
pub fn prompt_line(argv: &[String], quoting: PromptQuoting) -> String {
    let line = argv
        .iter()
        .map(|argument| quote_for_prompt(argument, quoting))
        .collect::<Vec<_>>()
        .join(" ");
    match quoting {
        PromptQuoting::PowerShell => format!("& {line}"),
        _ => line,
    }
}

/// POSIX quoting, for the capture path.
///
/// Kept separate from [`quote_for_prompt`] only by name: the inspector-reported paths and
/// arguments this quotes exist on Unix alone, so the shell behind them is a POSIX one.
pub fn shell_quote(word: &str) -> String {
    quote_for_prompt(word, PromptQuoting::Posix)
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

    /// The three cases a held resume line actually meets: an executable path with spaces, an
    /// opaque session reference with spaces, and one carrying the quote character each family
    /// escapes differently. A line quoted for the wrong family is a different command, not an
    /// ugly one, so each is pinned rather than described.
    #[test]
    fn each_shell_family_quotes_the_way_that_shell_reads() {
        let cases = [
            (
                PromptQuoting::Posix,
                r"'/opt/Program Files/claude' --resume 'it'\''s here'",
            ),
            (
                PromptQuoting::Fish,
                r"'/opt/Program Files/claude' --resume 'it\'s here'",
            ),
            // The call operator, without which PowerShell evaluates the quoted path as a string
            // and prints it instead of running anything.
            (
                PromptQuoting::PowerShell,
                "& '/opt/Program Files/claude' --resume 'it''s here'",
            ),
            (
                PromptQuoting::Cmd,
                "\"/opt/Program Files/claude\" --resume \"it's here\"",
            ),
        ];

        for (quoting, expected) in cases {
            let argv = [
                "/opt/Program Files/claude".to_string(),
                "--resume".to_string(),
                "it's here".to_string(),
            ];
            assert_eq!(prompt_line(&argv, quoting), expected, "{quoting:?}");
        }
    }

    /// A double quote is inert in a POSIX single-quoted word and is the grouping character in
    /// `cmd`, so it is the one character the two families disagree about in the other direction.
    ///
    /// The second pair has no space in it, which is the case that would slip through if `"` were
    /// ever treated as needing no quoting: cmd's parsing state would change mid-word.
    #[test]
    fn a_double_quote_survives_cmds_own_grouping() {
        assert_eq!(
            quote_for_prompt(r#"say "hi""#, PromptQuoting::Cmd),
            r#""say ""hi""""#
        );
        assert_eq!(
            quote_for_prompt(r#"say "hi""#, PromptQuoting::Posix),
            r#"'say "hi"'"#
        );

        assert_eq!(
            quote_for_prompt(r#"abc"def"#, PromptQuoting::Cmd),
            r#""abc""def""#
        );
        assert_eq!(
            quote_for_prompt(r#"abc"def"#, PromptQuoting::Posix),
            r#"'abc"def'"#
        );
    }

    /// `%` and `!` cannot be escaped at an interactive `cmd` prompt, so the most that can be done
    /// is keep them inside the quoted word rather than letting them end it. Pinned because the
    /// tempting simplification - treating them as ordinary characters - splits the argument.
    #[test]
    fn cmd_still_groups_a_word_it_cannot_fully_escape() {
        assert_eq!(
            quote_for_prompt("50%-done", PromptQuoting::Cmd),
            r#""50%-done""#
        );
        assert_eq!(
            quote_for_prompt("bang!ref", PromptQuoting::Cmd),
            r#""bang!ref""#
        );
        // A plain Windows path needs no quoting at all: backslash means nothing to `cmd`.
        assert_eq!(
            quote_for_prompt(r"C:\Users\me\claude.exe", PromptQuoting::Cmd),
            r"C:\Users\me\claude.exe"
        );
    }

    /// A backslash is an ordinary character inside POSIX single quotes and an escape inside fish's,
    /// which is exactly what a Windows path dragged onto a fish prompt would run into.
    #[test]
    fn fish_doubles_a_backslash_where_posix_leaves_it_alone() {
        let path = r"C:\Program Files\claude.exe";

        assert_eq!(
            quote_for_prompt(path, PromptQuoting::Fish),
            r"'C:\\Program Files\\claude.exe'"
        );
        assert_eq!(
            quote_for_prompt(path, PromptQuoting::Posix),
            r"'C:\Program Files\claude.exe'"
        );
    }

    /// The shell a pane runs decides the rules, and an unknown program falls back to what an
    /// unknown shell on this platform is likely to be - not to POSIX everywhere, which is what
    /// silently mis-quoted a Windows prompt.
    #[test]
    fn quoting_follows_the_shell_the_pane_runs() {
        assert_eq!(PromptQuoting::of(Some("/bin/zsh")), PromptQuoting::Posix);
        assert_eq!(
            PromptQuoting::of(Some("/usr/bin/fish")),
            PromptQuoting::Fish
        );
        assert_eq!(PromptQuoting::of(Some("cmd.exe")), PromptQuoting::Cmd);
        // A configured Windows shell is usually a full path, and only Windows' own path rules
        // split it into a basename.
        #[cfg(windows)]
        assert_eq!(
            PromptQuoting::of(Some(r"C:\Windows\System32\cmd.exe")),
            PromptQuoting::Cmd
        );
        assert_eq!(
            PromptQuoting::of(Some("PowerShell.EXE")),
            PromptQuoting::PowerShell
        );
        assert_eq!(PromptQuoting::of(Some("pwsh")), PromptQuoting::PowerShell);

        let fallback = if cfg!(windows) {
            PromptQuoting::Cmd
        } else {
            PromptQuoting::Posix
        };
        assert_eq!(PromptQuoting::of(None), fallback);
        assert_eq!(PromptQuoting::of(Some("some-new-shell")), fallback);
    }
}
