use std::iter::Peekable;
use std::vec::IntoIter;

use super::{
    CliArgs, ListFormat, NewSession, ParsedCli, SessionCommand, SessionsCommand, parse_list_format,
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
                "new <NAME> --detach [--no-resurrect]",
                "Start without a UI, optionally never saved",
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

/// Parse the `sessions` namespace. Each command reads the rest of the line itself, so an option
/// it does not take is refused wherever it was written.
pub(super) fn parse(
    iter: &mut Peekable<IntoIter<String>>,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    if iter
        .clone()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedCli::SessionsHelp);
    }

    let Some(command) = iter.next() else {
        return Ok(ParsedCli::SessionsHelp);
    };
    let spelling = format!("sessions {command}");
    if matches!(command.as_str(), "watch" | "list" | "kill") {
        if let Some(option) = cli.launch_only_option() {
            return Err(format!("{option} does not apply to {spelling}"));
        }
        if cli.attach_session.is_some() {
            return Err(format!("a session target does not apply to {spelling}"));
        }
    }
    match command.as_str() {
        "watch" => {
            if cli.remote.is_some() || iter.next().is_some() {
                return Err("sessions watch accepts no arguments or remote target".into());
            }
            Ok(ParsedCli::Sessions(SessionsCommand::Watch {
                config_path: cli.config_path,
            }))
        }
        "list" => parse_list(iter, cli.remote, cli.config_path),
        "attach" => parse_attach(iter, cli),
        "new" => parse_new(iter, cli),
        "kill" => parse_kill(iter, cli.remote, cli.config_path),
        other => Err(format!(
            "unknown sessions command `{other}` (expected list, watch, attach, new, or kill)"
        )),
    }
}

/// `sessions attach <NAME> [--read-only | --follow | --take-control]`.
///
/// The three options are one choice of how to join, so any two that disagree are refused,
/// including a `--read-only` written before `sessions`.
fn parse_attach(
    iter: &mut Peekable<IntoIter<String>>,
    mut cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    use crate::config::SessionOnAttach;
    use crate::state::AttachAccess;

    let name = take_target(iter, &cli, "sessions attach")?;
    let mut chosen = cli.read_only().then(|| "--read-only".to_string());
    let mut access = cli.session_command.access();
    while let Some(arg) = iter.next() {
        let requested = match arg.as_str() {
            "--read-only" => AttachAccess::ReadOnly,
            "--follow" => AttachAccess::Writable {
                on_attach: Some(SessionOnAttach::Follow),
            },
            "--take-control" => AttachAccess::Writable {
                on_attach: Some(SessionOnAttach::Take),
            },
            _ if super::take_launch_option(&arg, iter, &mut cli)? => continue,
            "--profile" | "--cwd" => return Err(new_only(&arg)),
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions attach"
                ));
            }
        };
        if let Some(previous) = chosen.as_deref()
            && previous != arg
        {
            return Err(format!("{arg} cannot be combined with {previous}"));
        }
        access = requested;
        chosen = Some(arg);
    }
    cli.attach_session = Some(name);
    cli.session_command = SessionCommand::Attach(access);
    Ok(ParsedCli::Run(cli))
}

/// `sessions new <NAME> [--profile <PROFILE> | --cwd <DIR>]`.
fn parse_new(
    iter: &mut Peekable<IntoIter<String>>,
    mut cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    if cli.read_only() {
        return Err(READ_ONLY_NEW.to_string());
    }
    let name = take_target(iter, &cli, "sessions new")?;
    let mut new = NewSession::default();
    let mut detach = false;
    let mut no_resurrect = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--detach" => detach = true,
            "--no-resurrect" => no_resurrect = true,
            "--profile" => {
                let profile = require_value(iter, "--profile requires a profile name")?;
                if new.profile.replace(profile).is_some() {
                    return Err("--profile specified more than once".to_string());
                }
            }
            "--cwd" => {
                let cwd = require_value(iter, "--cwd requires a directory")?;
                if new.cwd.replace(cwd).is_some() {
                    return Err("--cwd specified more than once".to_string());
                }
            }
            "--read-only" => return Err(READ_ONLY_NEW.to_string()),
            "--follow" | "--take-control" => return Err(attach_only(&arg)),
            _ if super::take_launch_option(&arg, iter, &mut cli)? => {}
            other => return Err(format!("unexpected argument `{other}` after sessions new")),
        }
        if new.cwd.is_some() && new.profile.is_some() {
            return Err("--cwd cannot be combined with --profile".to_string());
        }
    }
    if detach {
        return parse_new_detached(name, &new, !no_resurrect, cli);
    }
    if no_resurrect {
        return Err("--no-resurrect can only be used with sessions new --detach".to_string());
    }
    cli.attach_session = Some(name);
    cli.session_command = SessionCommand::New(new);
    Ok(ParsedCli::Run(cli))
}

/// `sessions new <NAME> --detach`: start the server and return, with no UI and no first pane, so
/// the options that shape a UI or its first pane are refused.
fn parse_new_detached(
    name: String,
    new: &NewSession,
    resurrect: bool,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let refused = if new.profile.is_some() {
        Some("--profile")
    } else if new.cwd.is_some() {
        Some("--cwd")
    } else if cli.remote.is_some() {
        Some("--remote")
    } else {
        cli.launch_only_option()
    };
    if let Some(option) = refused {
        return Err(format!("{option} cannot be combined with --detach"));
    }
    if !session::discovery::valid_session_name(&name) {
        return Err("invalid session name".to_string());
    }
    Ok(ParsedCli::Sessions(SessionsCommand::NewDetached {
        name,
        resurrect,
        config_path: cli.config_path,
    }))
}

const READ_ONLY_NEW: &str = "--read-only cannot be used with sessions new";

/// The refusal for a `sessions attach` option written anywhere else.
pub(super) fn attach_only(flag: &str) -> String {
    format!("{flag} can only be used with sessions attach")
}

/// The refusal for a `sessions new` option written anywhere else.
pub(super) fn new_only(flag: &str) -> String {
    format!("{flag} can only be used with sessions new")
}

fn take_target(
    iter: &mut impl Iterator<Item = String>,
    cli: &CliArgs,
    spelling: &str,
) -> std::result::Result<String, String> {
    if cli.attach_session.is_some() {
        return Err("session target specified more than once".to_string());
    }
    require_value(iter, &format!("{spelling} requires a session name"))
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

    fn parse(args: &[&str]) -> Result<ParsedCli, String> {
        parse_cli_args(args.iter().map(|arg| arg.to_string()).collect())
    }

    fn launch(args: &[&str]) -> CliArgs {
        match parse(args) {
            Ok(ParsedCli::Run(cli)) => cli,
            other => panic!("{args:?} should launch, got {other:?}"),
        }
    }

    fn writable(on_attach: Option<SessionOnAttach>) -> SessionCommand {
        SessionCommand::Attach(AttachAccess::Writable { on_attach })
    }

    use crate::config::SessionOnAttach;
    use crate::state::AttachAccess;

    #[test]
    fn session_launch_verbs_keep_trailing_launch_flags() {
        let attached = launch(&["sessions", "attach", "dev", "--read-only"]);
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(
            attached.session_command,
            SessionCommand::Attach(AttachAccess::ReadOnly)
        );

        let created = launch(&["sessions", "new", "dev", "--profile", "p", "--pick"]);
        assert_eq!(created.attach_session.as_deref(), Some("dev"));
        assert_eq!(
            created.session_command,
            SessionCommand::New(NewSession {
                profile: Some("p".into()),
                ..NewSession::default()
            })
        );
        assert!(created.pick);
    }

    #[test]
    fn launch_options_parse_on_either_side_of_the_session_command() {
        for args in [
            &[
                "--remote", "box", "--config", "c.toml", "sessions", "attach", "dev",
            ][..],
            &[
                "sessions", "attach", "dev", "--remote", "box", "--config", "c.toml",
            ][..],
            &[
                "--remote", "box", "sessions", "attach", "dev", "--config", "c.toml",
            ][..],
        ] {
            let cli = launch(args);
            assert_eq!(cli.remote.as_deref(), Some("box"), "{args:?}");
            assert_eq!(cli.config_path.as_deref(), Some("c.toml"), "{args:?}");
            assert_eq!(cli.session_command, writable(None), "{args:?}");
        }
        for args in [
            &["--remote", "box", "sessions", "new", "dev", "--cwd", "/src"][..],
            &["sessions", "new", "dev", "--cwd", "/src", "--remote", "box"][..],
        ] {
            let cli = launch(args);
            assert_eq!(cli.remote.as_deref(), Some("box"), "{args:?}");
            assert!(
                matches!(&cli.session_command, SessionCommand::New(new) if new.cwd.as_deref() == Some("/src")),
                "{args:?}"
            );
        }
    }

    #[test]
    fn attach_mode_flags_parse_into_the_attach_access() {
        for (flag, mode) in [
            ("--follow", SessionOnAttach::Follow),
            ("--take-control", SessionOnAttach::Take),
        ] {
            let cli = launch(&["sessions", "attach", "dev", flag]);
            assert_eq!(cli.session_command, writable(Some(mode)));
            let repeated = launch(&["sessions", "attach", "dev", flag, flag]);
            assert_eq!(repeated.session_command, writable(Some(mode)));
        }
        let plain = launch(&["sessions", "attach", "dev"]);
        assert_eq!(plain.session_command, writable(None));
        let viewer = launch(&["--read-only", "sessions", "attach", "dev"]);
        assert_eq!(
            viewer.session_command,
            SessionCommand::Attach(AttachAccess::ReadOnly)
        );
    }

    #[test]
    fn conflicting_attach_choices_fail_in_any_order() {
        for (first, second) in [
            ("--follow", "--take-control"),
            ("--follow", "--read-only"),
            ("--take-control", "--read-only"),
        ] {
            for (a, b) in [(first, second), (second, first)] {
                assert_eq!(
                    parse(&["sessions", "attach", "dev", a, b]).unwrap_err(),
                    format!("{b} cannot be combined with {a}"),
                );
            }
        }
        for flag in ["--follow", "--take-control"] {
            assert_eq!(
                parse(&["--read-only", "sessions", "attach", "dev", flag]).unwrap_err(),
                format!("{flag} cannot be combined with --read-only"),
            );
        }
    }

    #[test]
    fn attach_mode_flags_belong_to_sessions_attach_only() {
        for flag in ["--follow", "--take-control"] {
            let expected = format!("{flag} can only be used with sessions attach");
            for args in [
                vec![flag],
                vec![flag, "dev"],
                vec!["dev", flag],
                vec![flag, "sessions", "attach", "dev"],
                vec!["sessions", "new", "dev", flag],
                vec![flag, "sessions", "list"],
                vec![flag, "sessions", "watch"],
                vec![flag, "sessions", "kill", "dev"],
                vec![flag, "update", "--check"],
                vec![flag, "install"],
                vec![flag, "api", "describe"],
                vec![flag, "--version"],
                vec![flag, "--server", "dev"],
                vec![flag, "extensions", "list"],
            ] {
                assert_eq!(parse(&args).unwrap_err(), expected, "{args:?}");
            }
            let error = parse(&["sessions", "attach", "dev", flag, "update", "--check"])
                .expect_err("sessions attach owns the rest of the line");
            assert_eq!(error, "unexpected argument `update` after sessions attach");
        }
    }

    #[test]
    fn new_session_options_belong_to_sessions_new_only() {
        for (args, message) in [
            (
                &["sessions", "attach", "dev", "--cwd", "/src"][..],
                "--cwd can only be used with sessions new",
            ),
            (
                &["sessions", "attach", "dev", "--profile", "p"][..],
                "--profile can only be used with sessions new",
            ),
            (
                &["--cwd", "/src"][..],
                "--cwd can only be used with sessions new",
            ),
            (
                &["--cwd", "/src", "list-panes"][..],
                "--cwd can only be used with sessions new",
            ),
            (
                &["--profile", "p", "sessions", "new", "dev"][..],
                "--profile can only be used with sessions new",
            ),
            (
                &["sessions", "new", "dev", "--cwd", "/a", "--profile", "p"][..],
                "--cwd cannot be combined with --profile",
            ),
            (
                &["sessions", "new", "dev", "--profile", "p", "--cwd", "/a"][..],
                "--cwd cannot be combined with --profile",
            ),
            (
                &["sessions", "new", "dev", "--profile", "a", "--profile", "b"][..],
                "--profile specified more than once",
            ),
            (
                &["sessions", "new", "dev", "--read-only"][..],
                "--read-only cannot be used with sessions new",
            ),
            (
                &["--read-only", "sessions", "new", "dev"][..],
                "--read-only cannot be used with sessions new",
            ),
            (
                &["dev", "sessions", "attach", "other"][..],
                "session target specified more than once",
            ),
            (
                &["--pick", "sessions", "list"][..],
                "--pick does not apply to sessions list",
            ),
            (
                &["sessions", "new", "dev", "--detach", "--cwd", "/a"][..],
                "--cwd cannot be combined with --detach",
            ),
            (
                &["sessions", "new", "dev", "--profile", "p", "--detach"][..],
                "--profile cannot be combined with --detach",
            ),
            (
                &["--remote", "box", "sessions", "new", "dev", "--detach"][..],
                "--remote cannot be combined with --detach",
            ),
            (
                &["sessions", "new", "dev", "--detach", "--pick"][..],
                "--pick cannot be combined with --detach",
            ),
            (
                &["sessions", "new", "eph-1", "--detach"][..],
                "invalid session name",
            ),
            (
                &["sessions", "new", "dev", "--no-resurrect"][..],
                "--no-resurrect can only be used with sessions new --detach",
            ),
        ] {
            assert_eq!(parse(args).expect_err("must reject"), message, "{args:?}");
        }
    }

    #[test]
    fn detach_creates_without_launching() {
        assert!(matches!(
            parse(&["--config", "c.toml", "sessions", "new", "dev", "--detach"]),
            Ok(ParsedCli::Sessions(SessionsCommand::NewDetached {
                name,
                resurrect: true,
                config_path,
            })) if name == "dev" && config_path.as_deref() == Some("c.toml")
        ));
        for args in [
            &["sessions", "new", "dev", "--detach", "--no-resurrect"][..],
            &["sessions", "new", "dev", "--no-resurrect", "--detach"][..],
        ] {
            assert!(
                matches!(
                    parse(args),
                    Ok(ParsedCli::Sessions(SessionsCommand::NewDetached {
                        resurrect: false,
                        ..
                    }))
                ),
                "{args:?}"
            );
        }
    }

    #[test]
    fn cwd_seeds_only_a_created_session() {
        let created = launch(&["sessions", "new", "dev", "--cwd", "~/src/rozi"]);
        assert_eq!(
            created.session_command,
            SessionCommand::New(NewSession {
                cwd: Some("~/src/rozi".into()),
                ..NewSession::default()
            }),
            "argv never records a worktree origin"
        );
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
