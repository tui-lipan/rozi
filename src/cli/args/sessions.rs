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
                "new <NAME> --detach [--no-resurrect] [--remote <HOST>]",
                "Start without a UI, optionally never saved",
            ),
            row(
                "restart <NAME> [--no-resurrect] [--remote <HOST>]",
                "Restart a running session, ending its programs",
            ),
            row(
                "upgrade <NAME> [--remote <HOST>]",
                "Restart a session on this rozi from its snapshot",
            ),
            row(
                "kill <NAME> [--remote <HOST>]",
                "Stop a session and all of its panes",
            ),
            row(
                "share <NAME> --user <USER> [--role <ROLE>]",
                "Share a session with another user",
            ),
            row(
                "unshare <NAME> (--user <USER> | --all)",
                "Revoke a user's access to a session",
            ),
            row(
                "shares <NAME> [--format text|json]",
                "List who a session is shared with",
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
    if matches!(
        command.as_str(),
        "watch" | "list" | "restart" | "upgrade" | "kill" | "share" | "unshare" | "shares"
    ) {
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
        "restart" => parse_restart(iter, cli.remote, cli.config_path),
        "upgrade" => parse_upgrade(iter, cli.remote, cli.config_path),
        "kill" => parse_kill(iter, cli.remote, cli.config_path),
        "share" => parse_share(iter, cli),
        "unshare" => parse_unshare(iter, cli),
        "shares" => parse_shares(iter, cli),
        other => Err(format!(
            "unknown sessions command `{other}` (expected list, watch, attach, new, restart, upgrade, kill, share, unshare, or shares)"
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
    if session::naming::is_shared_target(&name) {
        return Err(format!("cannot create another user's session `{name}`"));
    }
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
    } else {
        cli.launch_only_option()
    };
    if let Some(option) = refused {
        return Err(format!("{option} cannot be combined with --detach"));
    }
    validate_own_session_target(&name, "create", true)?;
    Ok(ParsedCli::Sessions(SessionsCommand::NewDetached {
        name,
        resurrect,
        remote: cli.remote,
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

/// `sessions restart <NAME> [--no-resurrect] [--remote <HOST>]`.
fn parse_restart(
    iter: &mut impl Iterator<Item = String>,
    mut remote: Option<String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions restart requires a session name")?;
    validate_own_session_target(&name, "restart", true)?;
    let mut resurrect = true;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--no-resurrect" => resurrect = false,
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions restart --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if remote.replace(target).is_some() {
                    return Err("sessions restart --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions restart"
                ));
            }
        }
    }
    Ok(ParsedCli::Sessions(SessionsCommand::Restart {
        name,
        resurrect,
        remote,
        config_path,
    }))
}

/// `sessions upgrade <NAME> [--remote <HOST>]`.
fn parse_upgrade(
    iter: &mut impl Iterator<Item = String>,
    mut remote: Option<String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions upgrade requires a session name")?;
    validate_own_session_target(&name, "upgrade", true)?;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions upgrade --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if remote.replace(target).is_some() {
                    return Err("sessions upgrade --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions upgrade"
                ));
            }
        }
    }
    Ok(ParsedCli::Sessions(SessionsCommand::Upgrade {
        name,
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
    validate_own_session_target(&name, "kill", false)?;
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

fn validate_own_session_target(
    name: &str,
    verb: &str,
    require_named: bool,
) -> std::result::Result<(), String> {
    if session::naming::is_shared_target(name) {
        return Err(format!("cannot {verb} another user's session `{name}`"));
    }
    let valid = if require_named {
        session::discovery::valid_session_name(name)
    } else {
        session::discovery::valid_attach_target(name)
    };
    if !valid {
        return Err("invalid session name".to_string());
    }
    Ok(())
}

fn share_control_cli(
    name: String,
    mut remote: Option<String>,
    config_path: Option<String>,
    command: crate::control::ControlCommand,
    output_format: Option<ListFormat>,
) -> ParsedCli {
    let endpoint = match remote.take() {
        Some(target) => super::ControlEndpoint::Remote {
            target,
            session: name,
        },
        None => super::ControlEndpoint::Session(name),
    };
    ParsedCli::Control(super::ControlCli {
        config_path,
        endpoint,
        request: super::control_request(command),
        output_format: Some(output_format.unwrap_or(ListFormat::Text)),
        output: None,
    })
}

fn parse_share_role(value: &str) -> std::result::Result<session::protocol::SessionRole, String> {
    use session::protocol::SessionRole;
    match value {
        "viewer" => Ok(SessionRole::Viewer),
        "editor" => Ok(SessionRole::Editor),
        "controller" => Ok(SessionRole::Controller),
        other => Err(format!(
            "unknown share role `{other}` (expected viewer, editor, or controller)"
        )),
    }
}

/// `sessions share <NAME> --user <USER> [--role viewer|editor|controller] [--format text|json] [--remote <HOST>]`.
fn parse_share(
    iter: &mut impl Iterator<Item = String>,
    mut cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions share requires a session name")?;
    validate_own_session_target(&name, "share", false)?;
    let mut user = None;
    let mut role = None;
    let mut output_format = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--user" => {
                let value = require_value(iter, "sessions share --user requires a user name")?;
                if value.is_empty() {
                    return Err("sessions share --user requires a user name".to_string());
                }
                if user.replace(value).is_some() {
                    return Err("sessions share --user specified more than once".to_string());
                }
            }
            "--role" => {
                let value = require_value(
                    iter,
                    "sessions share --role requires viewer, editor, or controller",
                )?;
                let parsed = parse_share_role(&value)?;
                if role.replace(parsed).is_some() {
                    return Err("sessions share --role specified more than once".to_string());
                }
            }
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                let parsed = parse_list_format(&value, "sessions share")?;
                if output_format.replace(parsed).is_some() {
                    return Err("sessions share --format specified more than once".to_string());
                }
            }
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions share --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if cli.remote.replace(target).is_some() {
                    return Err("sessions share --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions share"
                ));
            }
        }
    }
    let user = user.ok_or_else(|| "sessions share requires --user <USER>".to_string())?;
    Ok(share_control_cli(
        name,
        cli.remote,
        cli.config_path,
        crate::control::ControlCommand::ShareGrant {
            user,
            role: role.unwrap_or(session::protocol::SessionRole::Viewer),
        },
        output_format,
    ))
}

/// `sessions unshare <NAME> (--user <USER> | --all) [--format text|json] [--remote <HOST>]`.
fn parse_unshare(
    iter: &mut impl Iterator<Item = String>,
    mut cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions unshare requires a session name")?;
    validate_own_session_target(&name, "unshare", false)?;
    let mut user = None;
    let mut all = false;
    let mut output_format = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--user" => {
                if all {
                    return Err("--user cannot be combined with --all".to_string());
                }
                let value = require_value(iter, "sessions unshare --user requires a user name")?;
                if value.is_empty() {
                    return Err("sessions unshare --user requires a user name".to_string());
                }
                if user.replace(value).is_some() {
                    return Err("sessions unshare --user specified more than once".to_string());
                }
            }
            "--all" => {
                if user.is_some() {
                    return Err("--all cannot be combined with --user".to_string());
                }
                all = true;
            }
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                let parsed = parse_list_format(&value, "sessions unshare")?;
                if output_format.replace(parsed).is_some() {
                    return Err("sessions unshare --format specified more than once".to_string());
                }
            }
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions unshare --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if cli.remote.replace(target).is_some() {
                    return Err("sessions unshare --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions unshare"
                ));
            }
        }
    }
    if !all && user.is_none() {
        return Err("sessions unshare requires --user <USER> or --all".to_string());
    }
    Ok(share_control_cli(
        name,
        cli.remote,
        cli.config_path,
        crate::control::ControlCommand::ShareRevoke { user },
        output_format,
    ))
}

/// `sessions shares <NAME> [--format text|json] [--remote <HOST>]`.
fn parse_shares(
    iter: &mut impl Iterator<Item = String>,
    mut cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let name = require_value(iter, "sessions shares requires a session name")?;
    validate_own_session_target(&name, "list shares for", false)?;
    let mut output_format = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                let parsed = parse_list_format(&value, "sessions shares")?;
                if output_format.replace(parsed).is_some() {
                    return Err("sessions shares --format specified more than once".to_string());
                }
            }
            "--remote" => {
                let target = require_value(
                    iter,
                    "sessions shares --remote requires a host alias or ssh:// URL",
                )?;
                session::remote::parse_remote_target(&target)?;
                if cli.remote.replace(target).is_some() {
                    return Err("sessions shares --remote specified more than once".to_string());
                }
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after sessions shares"
                ));
            }
        }
    }
    Ok(share_control_cli(
        name,
        cli.remote,
        cli.config_path,
        crate::control::ControlCommand::ShareList,
        output_format,
    ))
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
            "unknown sessions command `lst` (expected list, watch, attach, new, restart, upgrade, kill, share, unshare, or shares)"
        );
    }

    #[test]
    fn sessions_upgrade_parses_its_options_and_refuses_the_rest() {
        assert!(matches!(
            parse(&["sessions", "upgrade", "dev"]),
            Ok(ParsedCli::Sessions(SessionsCommand::Upgrade {
                name,
                remote: None,
                ..
            })) if name == "dev"
        ));
        for args in [
            &["sessions", "upgrade", "dev", "--remote", "workbox"][..],
            &["--remote", "workbox", "sessions", "upgrade", "dev"][..],
        ] {
            assert!(matches!(
                parse(args),
                Ok(ParsedCli::Sessions(SessionsCommand::Upgrade { remote: Some(host), .. }))
                    if host == "workbox"
            ));
        }
        assert_eq!(
            parse(&["sessions", "upgrade"]).expect_err("needs a name"),
            "sessions upgrade requires a session name"
        );
        assert_eq!(
            parse(&["sessions", "upgrade", "dev;rm"]).expect_err("hostile name"),
            "invalid session name"
        );
        assert_eq!(
            parse(&["sessions", "upgrade", "dev", "--no-resurrect"])
                .expect_err("upgrade always restores from snapshot"),
            "unexpected argument `--no-resurrect` after sessions upgrade"
        );
    }

    #[test]
    fn sessions_restart_parses_its_options_and_refuses_the_rest() {
        assert!(matches!(
            parse(&["sessions", "restart", "dev"]),
            Ok(ParsedCli::Sessions(SessionsCommand::Restart {
                name,
                resurrect: true,
                remote: None,
                ..
            })) if name == "dev"
        ));
        for args in [
            &["sessions", "restart", "dev", "--remote", "workbox"][..],
            &["--remote", "workbox", "sessions", "restart", "dev"][..],
        ] {
            assert!(matches!(
                parse(args),
                Ok(ParsedCli::Sessions(SessionsCommand::Restart { remote: Some(host), .. }))
                    if host == "workbox"
            ));
        }
        assert!(matches!(
            parse(&["sessions", "restart", "dev", "--no-resurrect"]),
            Ok(ParsedCli::Sessions(SessionsCommand::Restart {
                resurrect: false,
                ..
            }))
        ));
        assert_eq!(
            parse(&["sessions", "restart"]).expect_err("needs a name"),
            "sessions restart requires a session name"
        );
        assert_eq!(
            parse(&["sessions", "restart", "dev;rm"]).expect_err("hostile name"),
            "invalid session name"
        );
        assert_eq!(
            parse(&["sessions", "restart", "dev", "--detach"]).expect_err("not an option here"),
            "unexpected argument `--detach` after sessions restart"
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
                remote: None,
                config_path,
            })) if name == "dev" && config_path.as_deref() == Some("c.toml")
        ));
        for args in [
            &["--remote", "box", "sessions", "new", "dev", "--detach"][..],
            &["sessions", "new", "dev", "--detach", "--remote", "box"][..],
        ] {
            assert!(
                matches!(
                    parse(args),
                    Ok(ParsedCli::Sessions(SessionsCommand::NewDetached {
                        remote: Some(host),
                        ..
                    })) if host == "box"
                ),
                "{args:?}"
            );
        }
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

    #[test]
    fn share_unshare_and_shares_commands_parse() {
        use super::super::{ControlEndpoint, ListFormat};

        assert!(matches!(
            parse(&["sessions", "share", "dev", "--user", "bob"]),
            Ok(ParsedCli::Control(cli)) if matches!(
                (&cli.endpoint, &cli.request.command, cli.output_format),
                (
                    ControlEndpoint::Session(s),
                    crate::control::ControlCommand::ShareGrant {
                        user,
                        role: crate::session::protocol::SessionRole::Viewer,
                    },
                    Some(ListFormat::Text),
                ) if s == "dev" && user == "bob"
            )
        ));

        assert!(matches!(
            parse(&[
                "--remote",
                "workbox",
                "sessions",
                "share",
                "dev",
                "--user",
                "bob",
                "--role",
                "editor",
                "--format",
                "json",
            ]),
            Ok(ParsedCli::Control(cli)) if matches!(
                (&cli.endpoint, &cli.request.command, cli.output_format),
                (
                    ControlEndpoint::Remote { target, session },
                    crate::control::ControlCommand::ShareGrant {
                        user,
                        role: crate::session::protocol::SessionRole::Editor,
                    },
                    Some(ListFormat::Json),
                ) if target == "workbox" && session == "dev" && user == "bob"
            )
        ));

        assert!(matches!(
            parse(&["sessions", "unshare", "dev", "--all"]),
            Ok(ParsedCli::Control(cli)) if matches!(
                (&cli.endpoint, &cli.request.command),
                (
                    ControlEndpoint::Session(s),
                    crate::control::ControlCommand::ShareRevoke { user: None },
                ) if s == "dev"
            )
        ));

        assert!(matches!(
            parse(&["sessions", "shares", "dev", "--format", "json"]),
            Ok(ParsedCli::Control(cli)) if matches!(
                (&cli.endpoint, &cli.request.command, cli.output_format),
                (
                    ControlEndpoint::Session(s),
                    crate::control::ControlCommand::ShareList,
                    Some(ListFormat::Json),
                ) if s == "dev"
            )
        ));

        assert!(parse(&["sessions", "share", "dev"]).is_err());
        assert!(
            parse(&[
                "sessions", "share", "dev", "--user", "bob", "--role", "owner"
            ])
            .is_err()
        );
        assert!(parse(&["sessions", "unshare", "dev"]).is_err());
        assert!(parse(&["sessions", "unshare", "dev", "--user", "bob", "--all"]).is_err());
    }

    #[test]
    fn owner_qualified_targets_are_accepted_for_attach_and_refused_for_owner_commands() {
        let attached = launch(&["sessions", "attach", "alice/dev"]);
        assert_eq!(attached.attach_session.as_deref(), Some("alice/dev"));
        assert!(matches!(
            attached.session_command,
            SessionCommand::Attach(_)
        ));

        let bare = launch(&["alice/dev"]);
        assert_eq!(bare.attach_session.as_deref(), Some("alice/dev"));

        for args in [
            &["sessions", "new", "alice/dev"][..],
            &["sessions", "new", "alice/dev", "--detach"][..],
            &["sessions", "kill", "alice/dev"][..],
            &["sessions", "restart", "alice/dev"][..],
            &["sessions", "upgrade", "alice/dev"][..],
            &["sessions", "share", "alice/dev", "--user", "bob"][..],
            &["sessions", "unshare", "alice/dev", "--all"][..],
            &["sessions", "shares", "alice/dev"][..],
        ] {
            let err = parse(args).expect_err("must refuse owner/name");
            assert!(
                err.contains("another user's session"),
                "unexpected error for {args:?}: {err}"
            );
        }
    }
}
