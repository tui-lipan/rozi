use std::iter::Peekable;
use std::vec::IntoIter;

use super::{CliArgs, ListFormat, ParsedCli, parse_list_format, require_value};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};
use crate::session;

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row(
            "rozi [--remote <HOST>] worktrees <COMMAND> [OPTIONS]",
            "",
        )],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "Paths resolve on the host that owns the repository: `~` is that host's\n    \
               home, and a relative path starts in its working directory.",
        rows: &[
            row(
                "list [--cwd <DIR>] [--format text|json]",
                "List checkouts and their sessions",
            ),
            row(
                "create <BRANCH> [OPTIONS]",
                "Check out BRANCH in a linked worktree",
            ),
            row(
                "open <PATH> [--name <SESSION>]",
                "Open the checkout's session, or create one",
            ),
            row(
                "remove <PATH> [--force]",
                "Remove a linked checkout, never its branch",
            ),
            row(
                "exclude [DIR] [--cwd <DIR>]",
                "Add an in-repo worktree directory to",
            ),
            row("", ".git/info/exclude"),
        ],
    },
    HelpSection {
        heading: "CREATE OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--base <REV>", "Start a new branch from REV (default HEAD)"),
            row("--path <DIR>", "Checkout directory (default from config)"),
            row("--cwd <DIR>", "A directory in the repository"),
            row("--open", "Open the new checkout in a session"),
            row("--format text|json", "Print the new checkout"),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "--remote <HOST>",
                "Run on HOST; also accepted after COMMAND",
            ),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi worktrees", "manage Git worktrees on a session host");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorktreesCommand {
    List {
        cwd: Option<String>,
        format: ListFormat,
    },
    Create {
        branch: String,
        base: String,
        path: Option<String>,
        cwd: Option<String>,
        format: ListFormat,
        open: bool,
    },
    Open {
        path: String,
        name: Option<String>,
    },
    Remove {
        path: String,
        force: bool,
    },
    Exclude {
        directory: Option<String>,
        cwd: Option<String>,
    },
}

/// A `worktrees` command and the global options that shape where and how it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreesCli {
    pub(crate) command: WorktreesCommand,
    pub(crate) remote: Option<String>,
    pub(crate) config_path: Option<String>,
}

pub(super) fn parse(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &mut CliArgs,
) -> std::result::Result<ParsedCli, String> {
    if iter
        .clone()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedCli::WorktreesHelp);
    }
    for (set, flag) in [
        (cli.attach_session.is_some(), "a session target"),
        (cli.profile.is_some(), "--profile"),
        (cli.read_only, "--read-only"),
        (cli.pick, "--pick"),
        (cli.cwd.is_some(), "--cwd"),
    ] {
        if set {
            return Err(format!("{flag} does not apply to worktrees commands"));
        }
    }
    let Some(verb) = iter.next() else {
        return Ok(ParsedCli::WorktreesHelp);
    };
    let mut remote = cli.remote.clone();
    let mut flags = Flags::default();
    let mut operand = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--remote" => {
                let target = require_value(
                    iter,
                    "worktrees --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if remote.replace(target).is_some() {
                    return Err("--remote specified more than once".to_string());
                }
            }
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                once(
                    &mut flags.format,
                    parse_list_format(&value, "worktrees")?,
                    &arg,
                )?;
            }
            "--cwd" => once(
                &mut flags.cwd,
                require_value(iter, "--cwd requires a directory")?,
                &arg,
            )?,
            "--base" => once(
                &mut flags.base,
                require_value(iter, "--base requires a revision")?,
                &arg,
            )?,
            "--path" => once(
                &mut flags.path,
                require_value(iter, "--path requires a directory")?,
                &arg,
            )?,
            "--name" => {
                let name = require_value(iter, "--name requires a session name")?;
                if !session::discovery::valid_session_name(&name) {
                    return Err("invalid session name".to_string());
                }
                once(&mut flags.name, name, &arg)?;
            }
            "--open" => flags.open = true,
            "--force" => flags.force = true,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown flag `{other}` for worktrees {verb}"));
            }
            value => {
                if operand.replace(value.to_string()).is_some() {
                    return Err(format!(
                        "unexpected argument `{value}` after worktrees {verb}"
                    ));
                }
            }
        }
    }
    let command = match verb.as_str() {
        "list" => {
            flags.allow(&verb, &["--cwd", "--format"])?;
            reject_operand(operand, &verb)?;
            WorktreesCommand::List {
                cwd: flags.cwd,
                format: flags.format.unwrap_or_default(),
            }
        }
        "create" => {
            flags.allow(&verb, &["--cwd", "--format", "--base", "--path", "--open"])?;
            let branch = operand.ok_or("worktrees create requires a branch name")?;
            if flags.open && flags.format == Some(ListFormat::Json) {
                return Err("worktrees create --open cannot print JSON".to_string());
            }
            WorktreesCommand::Create {
                branch,
                base: flags.base.unwrap_or_else(|| "HEAD".to_string()),
                path: flags.path,
                cwd: flags.cwd,
                format: flags.format.unwrap_or_default(),
                open: flags.open,
            }
        }
        "open" => {
            flags.allow(&verb, &["--name"])?;
            WorktreesCommand::Open {
                path: operand.ok_or("worktrees open requires a checkout path")?,
                name: flags.name,
            }
        }
        "exclude" => {
            flags.allow(&verb, &["--cwd"])?;
            WorktreesCommand::Exclude {
                directory: operand,
                cwd: flags.cwd,
            }
        }
        "remove" => {
            flags.allow(&verb, &["--force"])?;
            WorktreesCommand::Remove {
                path: operand.ok_or("worktrees remove requires a checkout path")?,
                force: flags.force,
            }
        }
        other => {
            return Err(format!(
                "unknown worktrees command `{other}` (expected list, create, open, remove, or exclude)"
            ));
        }
    };
    Ok(ParsedCli::Worktrees(WorktreesCli {
        command,
        remote,
        config_path: cli.config_path.clone(),
    }))
}

#[derive(Default)]
struct Flags {
    cwd: Option<String>,
    format: Option<ListFormat>,
    base: Option<String>,
    path: Option<String>,
    name: Option<String>,
    open: bool,
    force: bool,
}

impl Flags {
    /// Reject a flag the verb does not take, rather than silently ignoring it.
    fn allow(&self, verb: &str, allowed: &[&str]) -> std::result::Result<(), String> {
        let given = [
            ("--cwd", self.cwd.is_some()),
            ("--format", self.format.is_some()),
            ("--base", self.base.is_some()),
            ("--path", self.path.is_some()),
            ("--name", self.name.is_some()),
            ("--open", self.open),
            ("--force", self.force),
        ];
        match given
            .iter()
            .find(|(flag, set)| *set && !allowed.contains(flag))
        {
            Some((flag, _)) => Err(format!("worktrees {verb} does not take {flag}")),
            None => Ok(()),
        }
    }
}

fn once<T>(slot: &mut Option<T>, value: T, flag: &str) -> std::result::Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{flag} specified more than once"));
    }
    Ok(())
}

fn reject_operand(operand: Option<String>, verb: &str) -> std::result::Result<(), String> {
    match operand {
        Some(value) => Err(format!(
            "unexpected argument `{value}` after worktrees {verb}"
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_cli_args;
    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<ParsedCli, String> {
        parse_cli_args(args.iter().map(|arg| arg.to_string()).collect())
    }

    fn command(args: &[&str]) -> WorktreesCli {
        match parse(args).expect("parses") {
            ParsedCli::Worktrees(cli) => cli,
            other => panic!("expected worktrees, got {other:?}"),
        }
    }

    #[test]
    fn worktrees_namespace_parses_each_verb() {
        assert!(matches!(
            parse(&["worktrees"]),
            Ok(ParsedCli::WorktreesHelp)
        ));
        assert!(matches!(
            parse(&["worktrees", "list", "--help"]),
            Ok(ParsedCli::WorktreesHelp)
        ));
        assert_eq!(
            command(&[
                "worktrees",
                "list",
                "--cwd",
                "~/src/rozi",
                "--format",
                "json"
            ])
            .command,
            WorktreesCommand::List {
                cwd: Some("~/src/rozi".into()),
                format: ListFormat::Json,
            }
        );
        assert_eq!(
            command(&[
                "worktrees",
                "create",
                "feat/x",
                "--path",
                "C:\\wt\\x",
                "--open"
            ])
            .command,
            WorktreesCommand::Create {
                branch: "feat/x".into(),
                base: "HEAD".into(),
                path: Some("C:\\wt\\x".into()),
                cwd: None,
                format: ListFormat::Text,
                open: true,
            }
        );
        assert_eq!(
            command(&["worktrees", "open", "../wt", "--name", "feat-x"]).command,
            WorktreesCommand::Open {
                path: "../wt".into(),
                name: Some("feat-x".into()),
            }
        );
        assert_eq!(
            command(&["worktrees", "exclude"]).command,
            WorktreesCommand::Exclude {
                directory: None,
                cwd: None,
            }
        );
        assert_eq!(
            command(&["worktrees", "exclude", ".worktrees", "--cwd", "~/src/rozi"]).command,
            WorktreesCommand::Exclude {
                directory: Some(".worktrees".into()),
                cwd: Some("~/src/rozi".into()),
            }
        );
        assert_eq!(
            command(&["worktrees", "remove", "/wt/x", "--force"]).command,
            WorktreesCommand::Remove {
                path: "/wt/x".into(),
                force: true,
            }
        );
    }

    #[test]
    fn remote_is_accepted_before_or_after_the_namespace() {
        assert_eq!(
            command(&["--remote", "box", "worktrees", "list"]).remote,
            Some("box".into())
        );
        // A bare `--remote` takes `[remote] default_host` rather than eating the namespace.
        assert_eq!(
            command(&["--remote", "worktrees", "list"]).remote,
            Some(String::new())
        );
        assert_eq!(
            command(&["worktrees", "remove", "/wt/x", "--remote", "box"]).remote,
            Some("box".into())
        );
        assert_eq!(
            parse(&["--remote", "a", "worktrees", "list", "--remote", "b"]).expect_err("twice"),
            "--remote specified more than once"
        );
    }

    #[test]
    fn misplaced_options_are_rejected_rather_than_ignored() {
        for (args, message) in [
            (
                &["worktrees", "list", "--force"][..],
                "worktrees list does not take --force",
            ),
            (
                &["worktrees", "remove", "/wt/x", "--base", "main"][..],
                "worktrees remove does not take --base",
            ),
            (
                &["worktrees", "create"][..],
                "worktrees create requires a branch name",
            ),
            (
                &["worktrees", "remove"][..],
                "worktrees remove requires a checkout path",
            ),
            (
                &["worktrees", "create", "x", "--open", "--format", "json"][..],
                "worktrees create --open cannot print JSON",
            ),
            (
                &["--read-only", "worktrees", "list"][..],
                "--read-only does not apply to worktrees commands",
            ),
            (
                &["worktrees", "prune"][..],
                "unknown worktrees command `prune` (expected list, create, open, remove, or exclude)",
            ),
        ] {
            assert_eq!(parse(args).expect_err("must reject"), message, "{args:?}");
        }
    }
}
