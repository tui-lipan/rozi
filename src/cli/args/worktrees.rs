use std::iter::Peekable;
use std::vec::IntoIter;

use super::{CliArgs, ListFormat, ParsedCli, parse_list_format, require_value};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};
use crate::session;
use crate::session::worktrees::CheckoutSelector;

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
               home, and a relative path starts in its working directory.\n    \
               CHECKOUT is a branch or a path inside the checkout. One starting with\n    \
               ./, ../, /, or ~ is only a path; any other must not name two checkouts.",
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
                "open <CHECKOUT> [OPTIONS]",
                "Open the checkout's session, or create one",
            ),
            row("remove <CHECKOUT> [OPTIONS]", "Remove a linked checkout"),
            row("unlock <CHECKOUT> [OPTIONS]", "Lift a checkout's Git lock"),
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
        heading: "OPEN, REMOVE, AND UNLOCK OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "--branch <NAME>",
                "Select by branch only, instead of CHECKOUT",
            ),
            row("--cwd <DIR>", "A directory in the repository"),
            row("--name <SESSION>", "open: the session to open or create"),
        ],
    },
    HelpSection {
        heading: "REMOVE OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--force", "Let Git remove a dirty checkout"),
            row(
                "--delete-branch <WHAT>",
                "Delete merged branches: local, remote,",
            ),
            row("", "or local,remote"),
            row("--keep-branch", "Keep its branch (overrides config)"),
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
        checkout: CheckoutSelector,
        cwd: Option<String>,
        name: Option<String>,
    },
    Remove {
        checkout: CheckoutSelector,
        cwd: Option<String>,
        force: bool,
        /// `None` follows `[worktrees] delete_branch` on the host.
        delete_branch: Option<crate::git::worktrees::BranchDeletion>,
    },
    Unlock {
        checkout: CheckoutSelector,
        cwd: Option<String>,
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
    let offender = if cli.attach_session.is_some() {
        Some("a session target")
    } else {
        cli.launch_only_option()
    };
    if let Some(flag) = offender {
        return Err(format!("{flag} does not apply to worktrees commands"));
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
            "--branch" => once(
                &mut flags.branch,
                require_value(iter, "--branch requires a branch name")?,
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
            "--delete-branch" => {
                let list = require_value(
                    iter,
                    "--delete-branch requires local, remote, or local,remote",
                )?;
                let selection = crate::git::worktrees::BranchDeletion::parse(&list)?;
                once(&mut flags.delete_branch, selection, &arg)?;
            }
            "--keep-branch" => once(
                &mut flags.delete_branch,
                crate::git::worktrees::BranchDeletion::NONE,
                &arg,
            )?,
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
            flags.allow(&verb, &["--branch", "--cwd", "--name"])?;
            WorktreesCommand::Open {
                checkout: checkout_selector(operand, flags.branch, &verb)?,
                cwd: flags.cwd,
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
            flags.allow(
                &verb,
                &[
                    "--branch",
                    "--cwd",
                    "--force",
                    "--delete-branch",
                    "--keep-branch",
                ],
            )?;
            WorktreesCommand::Remove {
                checkout: checkout_selector(operand, flags.branch, &verb)?,
                cwd: flags.cwd,
                force: flags.force,
                delete_branch: flags.delete_branch,
            }
        }
        "unlock" => {
            flags.allow(&verb, &["--branch", "--cwd"])?;
            WorktreesCommand::Unlock {
                checkout: checkout_selector(operand, flags.branch, &verb)?,
                cwd: flags.cwd,
            }
        }
        other => {
            return Err(format!(
                "unknown worktrees command `{other}` (expected list, create, open, remove, unlock, or exclude)"
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
    branch: Option<String>,
    name: Option<String>,
    open: bool,
    force: bool,
    delete_branch: Option<crate::git::worktrees::BranchDeletion>,
}

impl Flags {
    /// Reject a flag the verb does not take, rather than silently ignoring it.
    fn allow(&self, verb: &str, allowed: &[&str]) -> std::result::Result<(), String> {
        let given = [
            ("--cwd", self.cwd.is_some()),
            ("--format", self.format.is_some()),
            ("--base", self.base.is_some()),
            ("--path", self.path.is_some()),
            ("--branch", self.branch.is_some()),
            ("--name", self.name.is_some()),
            ("--open", self.open),
            ("--force", self.force),
            ("--delete-branch", self.delete_branch.is_some()),
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

/// The checkout an `open`, `remove`, or `unlock` names: the operand, which the host reads as a path
/// or a branch, or `--branch`, which only matches a branch.
fn checkout_selector(
    operand: Option<String>,
    branch: Option<String>,
    verb: &str,
) -> std::result::Result<CheckoutSelector, String> {
    match (operand, branch) {
        (Some(_), Some(_)) => Err(format!(
            "worktrees {verb} takes a checkout or --branch, not both"
        )),
        (Some(operand), None) => Ok(CheckoutSelector::Auto(operand)),
        (None, Some(branch)) => Ok(CheckoutSelector::Branch(branch)),
        (None, None) => Err(format!(
            "worktrees {verb} requires a checkout path or branch"
        )),
    }
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
                checkout: CheckoutSelector::Auto("../wt".into()),
                cwd: None,
                name: Some("feat-x".into()),
            }
        );
        assert_eq!(
            command(&[
                "worktrees",
                "open",
                "--branch",
                "feat/x",
                "--cwd",
                "~/src/rozi"
            ])
            .command,
            WorktreesCommand::Open {
                checkout: CheckoutSelector::Branch("feat/x".into()),
                cwd: Some("~/src/rozi".into()),
                name: None,
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
        for (flags, local, remote) in [
            (&["--delete-branch", "local"][..], true, false),
            (&["--delete-branch", "remote"][..], false, true),
            (&["--delete-branch", "local,remote"][..], true, true),
            (&["--keep-branch"][..], false, false),
        ] {
            let mut args = vec!["worktrees", "remove", "/wt/x"];
            args.extend_from_slice(flags);
            assert_eq!(
                command(&args).command,
                WorktreesCommand::Remove {
                    checkout: CheckoutSelector::Auto("/wt/x".into()),
                    cwd: None,
                    force: false,
                    delete_branch: Some(crate::git::worktrees::BranchDeletion { local, remote }),
                }
            );
        }
        assert!(parse(&["worktrees", "remove", "/wt/x", "--delete-branch", "all"]).is_err());
        assert!(parse(&["worktrees", "remove", "/wt/x", "--delete-branch"]).is_err());
        assert_eq!(
            command(&["worktrees", "remove", "feat/x", "--force"]).command,
            WorktreesCommand::Remove {
                checkout: CheckoutSelector::Auto("feat/x".into()),
                cwd: None,
                force: true,
                delete_branch: None,
            }
        );
        assert_eq!(
            command(&["worktrees", "unlock", "~/wt/x"]).command,
            WorktreesCommand::Unlock {
                checkout: CheckoutSelector::Auto("~/wt/x".into()),
                cwd: None,
            }
        );
        assert_eq!(
            command(&["worktrees", "unlock", "--branch", "agent", "--cwd", "repo"]).command,
            WorktreesCommand::Unlock {
                checkout: CheckoutSelector::Branch("agent".into()),
                cwd: Some("repo".into()),
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
                "worktrees remove requires a checkout path or branch",
            ),
            (
                &["worktrees", "remove", "feat/x", "--branch", "feat/x"][..],
                "worktrees remove takes a checkout or --branch, not both",
            ),
            (
                &["worktrees", "create", "x", "--branch", "y"][..],
                "worktrees create does not take --branch",
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
                "unknown worktrees command `prune` (expected list, create, open, remove, unlock, or exclude)",
            ),
            (
                &["worktrees", "unlock", "/wt/x", "--force"][..],
                "worktrees unlock does not take --force",
            ),
            (
                &["worktrees", "unlock"][..],
                "worktrees unlock requires a checkout path or branch",
            ),
        ] {
            assert_eq!(parse(args).expect_err("must reject"), message, "{args:?}");
        }
    }
}
