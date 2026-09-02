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
            row(
                "attach <NAME> [--read-only]",
                "Attach to a running session, never create",
            ),
            row(
                "new <NAME> [--profile <PROFILE>]",
                "Create a session, optionally from a profile",
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
        rows: &[row("-h, --help", "Print help")],
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
            "unknown sessions command `{other}` (expected list, attach, new, or kill)"
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
            "unknown sessions command `lst` (expected list, attach, new, or kill)"
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
