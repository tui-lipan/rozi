use std::iter::Peekable;
use std::vec::IntoIter;

use super::{
    CliArgs, ListFormat, ParsedCli, SessionCommand, SessionsCommand, parse_list_format,
    require_value,
};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};
use crate::session;

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi sessions <COMMAND> [OPTIONS]", "")],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "list [--format text|json] [--remote <HOST>]",
                "List connectable sessions",
            ),
            row("watch", "Serve the host metadata protocol over stdio"),
            row(
                "attach <NAME> [OPTIONS]",
                "Attach to a running session, never create",
            ),
            row(
                "new <NAME> [--profile <PROFILE>]",
                "Create a session, optionally from a profile",
            ),
            row(
                "new <NAME> --cwd <DIR>",
                "Create a session whose first pane starts in DIR",
            ),
            row(
                "kill <NAME> [--remote <HOST>]",
                "Stop a session and all of its panes",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--read-only", "Attach as a viewer; cannot type or tile"),
            row("--follow", "Follow, overriding session.on_attach"),
            row(
                "--take-control",
                "Request control, overriding session.on_attach",
            ),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi sessions", "manage named sessions");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(super) fn parse(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &mut CliArgs,
) -> std::result::Result<Option<ParsedCli>, String> {
    if iter
        .clone()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(Some(ParsedCli::SessionsHelp));
    }

    match iter.next().as_deref() {
        None => Ok(Some(ParsedCli::SessionsHelp)),
        Some("watch") => {
            if cli.remote.is_some() || iter.next().is_some() {
                return Err("sessions watch accepts no arguments or remote target".into());
            }
            Ok(Some(ParsedCli::Sessions(SessionsCommand::Watch {
                config_path: cli.config_path.clone(),
            })))
        }
        Some("list") => parse_list(iter, cli.remote.clone(), cli.config_path.clone()).map(Some),
        Some("attach") => {
            set_launch_target(iter, cli, SessionCommand::Attach, "sessions attach")?;
            Ok(None)
        }
        Some("new") => {
            set_launch_target(iter, cli, SessionCommand::New, "sessions new")?;
            Ok(None)
        }
        Some("kill") => parse_kill(iter, cli.remote.clone(), cli.config_path.clone()).map(Some),
        Some(other) => Err(format!(
            "unknown sessions command `{other}` (expected list, watch, attach, new, or kill)"
        )),
    }
}

fn parse_list(
    iter: &mut impl Iterator<Item = String>,
    mut remote: Option<String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let mut format = ListFormat::Text;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                format = parse_list_format(&value, "sessions list")?;
            }
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions list --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if remote.replace(target).is_some() {
                    return Err("sessions list --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!("unexpected argument `{other}` after sessions list"));
            }
        }
    }
    Ok(ParsedCli::Sessions(SessionsCommand::List {
        format,
        remote,
        config_path,
    }))
}

fn parse_kill(
    iter: &mut impl Iterator<Item = String>,
    mut remote: Option<String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions kill requires a session name")?;
    if !session::discovery::valid_attach_target(&name) {
        return Err("invalid session name".to_string());
    }
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions kill --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if remote.replace(target).is_some() {
                    return Err("sessions kill --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!("unexpected argument `{other}` after sessions kill"));
            }
        }
    }
    Ok(ParsedCli::Sessions(SessionsCommand::Kill {
        name,
        remote,
        config_path,
    }))
}

pub(super) fn parse_attach_mode(cli: &mut CliArgs, flag: &str) -> Result<(), String> {
    let mode = if flag == "--follow" {
        crate::config::SessionOnAttach::Follow
    } else {
        crate::config::SessionOnAttach::Take
    };
    if cli.on_attach.is_some_and(|previous| previous != mode) {
        return Err("--take-control cannot be combined with --follow".into());
    }
    cli.on_attach = Some(mode);
    Ok(())
}

pub(super) fn validate_attach_mode(cli: &CliArgs) -> Result<(), String> {
    if cli.on_attach.is_none() {
        return Ok(());
    }
    if cli.read_only {
        return Err("--take-control and --follow cannot be combined with --read-only".into());
    }
    if cli.session_command != SessionCommand::Attach {
        return Err("--take-control and --follow can only be used with sessions attach".into());
    }
    Ok(())
}

fn set_launch_target(
    iter: &mut impl Iterator<Item = String>,
    cli: &mut CliArgs,
    command: SessionCommand,
    spelling: &str,
) -> std::result::Result<(), String> {
    if cli.attach_session.is_some() {
        return Err("session target specified more than once".to_string());
    }
    cli.attach_session = Some(require_value(
        iter,
        &format!("{spelling} requires a session name"),
    )?);
    cli.session_command = command;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::parse_cli_args;
    use super::*;

    #[test]
    fn sessions_namespace_parses_commands_and_help() {
        assert!(matches!(
            parse_cli_args(vec!["sessions".into()]).expect("parses"),
            ParsedCli::SessionsHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["sessions".into(), "--help".into()]).expect("parses"),
            ParsedCli::SessionsHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["sessions".into(), "list".into()]).expect("parses"),
            ParsedCli::Sessions(SessionsCommand::List {
                format: ListFormat::Text,
                remote: None,
                ..
            })
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "sessions".into(),
                "list".into(),
                "--format".into(),
                "json".into(),
            ])
            .expect("parses"),
            ParsedCli::Sessions(SessionsCommand::List {
                format: ListFormat::Json,
                ..
            })
        ));
        assert!(matches!(
            parse_cli_args(vec!["sessions".into(), "kill".into(), "dev".into()])
                .expect("parses"),
            ParsedCli::Sessions(SessionsCommand::Kill {
                name,
                remote: None,
                ..
            }) if name == "dev"
        ));
        assert_eq!(
            parse_cli_args(vec!["sessions".into(), "lst".into()]).expect_err("must reject"),
            "unknown sessions command `lst` (expected list, watch, attach, new, or kill)"
        );
    }

    #[test]
    fn session_launch_verbs_keep_trailing_launch_flags() {
        let ParsedCli::Run(attached) = parse_cli_args(vec![
            "sessions".into(),
            "attach".into(),
            "dev".into(),
            "--read-only".into(),
        ])
        .expect("parses") else {
            panic!("expected launch");
        };
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(attached.session_command, SessionCommand::Attach);
        assert!(attached.read_only);

        let ParsedCli::Run(created) = parse_cli_args(vec![
            "sessions".into(),
            "new".into(),
            "dev".into(),
            "--profile".into(),
            "p".into(),
            "--pick".into(),
        ])
        .expect("parses") else {
            panic!("expected launch");
        };
        assert_eq!(created.attach_session.as_deref(), Some("dev"));
        assert_eq!(created.session_command, SessionCommand::New);
        assert_eq!(created.profile.as_deref(), Some("p"));
        assert!(created.pick);
    }

    #[test]
    fn attach_mode_flags_parse_and_reject_conflicts() {
        for (flag, expected) in [
            ("--follow", crate::config::SessionOnAttach::Follow),
            ("--take-control", crate::config::SessionOnAttach::Take),
        ] {
            let ParsedCli::Run(cli) = parse_cli_args(
                ["sessions", "attach", "dev", flag]
                    .map(String::from)
                    .to_vec(),
            )
            .expect("attach mode parses") else {
                panic!("expected attach")
            };
            assert_eq!(cli.on_attach, Some(expected));
            for args in [
                vec!["sessions", "attach", "dev", flag, "--read-only"],
                vec!["--read-only", "sessions", "attach", "dev", flag],
            ] {
                let error =
                    parse_cli_args(args.into_iter().map(String::from).collect()).unwrap_err();
                assert!(
                    error.contains("cannot be combined with --read-only"),
                    "{error}"
                );
            }
            for args in [vec![flag], vec!["sessions", "new", "dev", flag]] {
                let error =
                    parse_cli_args(args.into_iter().map(String::from).collect()).unwrap_err();
                assert!(
                    error.contains("can only be used with sessions attach"),
                    "{error}"
                );
            }
        }
        for flags in [
            ["--follow", "--take-control"],
            ["--take-control", "--follow"],
        ] {
            let mut args = vec!["sessions", "attach", "dev"];
            args.extend(flags);
            assert_eq!(
                parse_cli_args(args.into_iter().map(String::from).collect()).unwrap_err(),
                "--take-control cannot be combined with --follow"
            );
        }
    }

    #[test]
    fn attach_mode_flags_cannot_be_ignored_by_early_return_commands() {
        for flag in ["--follow", "--take-control"] {
            for command in [
                vec!["sessions", "list"],
                vec!["sessions", "watch"],
                vec!["sessions", "kill", "dev"],
                vec!["update", "--check"],
                vec!["install"],
                vec!["api", "describe"],
                vec!["--version"],
                vec!["--server", "dev"],
                vec!["extensions", "list"],
            ] {
                let prefix = [flag];
                let args: Vec<String> = prefix
                    .iter()
                    .chain(&command)
                    .map(|arg| arg.to_string())
                    .collect();
                let error =
                    parse_cli_args(args.clone()).expect_err("must reject ignored attach flag");
                assert!(
                    error.contains("can only be used with sessions attach"),
                    "{args:?}: {error}"
                );
            }
        }
    }

    #[test]
    fn attach_mode_flags_are_rejected_if_an_attach_is_replaced_by_a_subcommand() {
        for flag in ["--follow", "--take-control"] {
            let error = parse_cli_args(
                ["sessions", "attach", "dev", flag, "update", "--check"]
                    .map(String::from)
                    .to_vec(),
            )
            .expect_err("update must not ignore an attach flag");
            assert!(
                error.contains("can only be used with sessions attach"),
                "{error}"
            );
        }
    }

    #[test]
    fn attach_mode_flags_accept_global_and_trailing_positions_on_attach() {
        for (flag, mode) in [
            ("--follow", crate::config::SessionOnAttach::Follow),
            ("--take-control", crate::config::SessionOnAttach::Take),
        ] {
            for args in [
                vec![flag, "sessions", "attach", "dev"],
                vec!["sessions", "attach", "dev", flag],
            ] {
                let ParsedCli::Run(cli) =
                    parse_cli_args(args.into_iter().map(String::from).collect())
                        .expect("attach mode parses")
                else {
                    panic!("expected attach");
                };
                assert_eq!(cli.session_command, SessionCommand::Attach);
                assert_eq!(cli.on_attach, Some(mode));
            }
        }
    }

    #[test]
    fn cwd_seeds_only_a_created_session() {
        let ParsedCli::Run(created) = parse_cli_args(vec![
            "sessions".into(),
            "new".into(),
            "dev".into(),
            "--cwd".into(),
            "~/src/rozi".into(),
        ])
        .expect("parses") else {
            panic!("expected launch");
        };
        assert_eq!(created.cwd.as_deref(), Some("~/src/rozi"));
        assert!(
            created.worktree_checkouts.is_none(),
            "argv never records a worktree origin"
        );

        for (args, message) in [
            (
                vec!["sessions", "attach", "dev", "--cwd", "/src"],
                "--cwd can only be used with sessions new",
            ),
            (
                vec!["--cwd", "/src"],
                "--cwd can only be used with sessions new",
            ),
            (
                vec!["sessions", "new", "dev", "--cwd", "/a", "--profile", "p"],
                "--cwd cannot be combined with --profile",
            ),
            (
                vec!["--cwd", "/src", "list-panes"],
                "--cwd does not apply to control commands",
            ),
        ] {
            assert_eq!(
                parse_cli_args(args.iter().map(|arg| arg.to_string()).collect())
                    .expect_err("must reject"),
                message,
                "{args:?}"
            );
        }
    }

    #[test]
    fn remote_before_sessions_does_not_consume_the_namespace() {
        assert!(matches!(
            parse_cli_args(vec![
                "--remote".into(),
                "host".into(),
                "sessions".into(),
                "list".into(),
            ])
            .expect("parses"),
            ParsedCli::Sessions(SessionsCommand::List {
                remote: Some(remote),
                ..
            }) if remote == "host"
        ));

        assert!(matches!(
            parse_cli_args(vec!["--remote".into(), "sessions".into(), "list".into()])
                .expect("parses"),
            ParsedCli::Sessions(SessionsCommand::List {
                remote: Some(remote),
                ..
            }) if remote.is_empty()
        ));
    }
}
