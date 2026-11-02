use std::iter::Peekable;
use std::path::PathBuf;
use std::vec::IntoIter;

use super::{
    CliArgs, ControlCli, ControlEndpoint, ExtensionsCommand, ExtensionsDeployCli, ListFormat,
    ParsedCli, control_request, reject_trailing_control_args, require_value,
};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi extensions <COMMAND> [OPTIONS]", "")],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "list [--verbose] [--json]",
                "Show extension discovery status",
            ),
            row("install <SOURCE>", "Install and enable an extension"),
            row(
                "install --link <PATH>",
                "Link and enable a development checkout",
            ),
            row("update <ID>|--all", "Update managed Git extensions"),
            row("remove <ID> [--purge]", "Remove an installed extension"),
            row("new <ID>", "Create a valid extension scaffold"),
            row("check <PATH> [--json]", "Validate an unpacked extension"),
            row(
                "status [--json] [--session <NAME>]",
                "Show placed processes, or a session's deployments",
            ),
            row(
                "deploy <ID> --session <NAME>",
                "Store an extension on a session server",
            ),
            row(
                "undeploy <ID> --session <NAME>",
                "Remove an extension from a session server",
            ),
            row(
                "logs <ID> [--unit <U>] [-n <N>]",
                "Show an extension's recent output",
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

const INSTALL_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row("rozi extensions install <SOURCE>", ""),
            row("rozi extensions install --link <PATH>", ""),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--link <PATH>", "Link a local checkout without copying it"),
            row("-h, --help", "Print help"),
        ],
    },
];

const REMOVE_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi extensions remove <ID> [--purge]", "")],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "--purge",
                "Also delete the extension's state on this machine",
            ),
            row("-h, --help", "Print help"),
        ],
    },
];

const UPDATE_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row("rozi extensions update <ID>", ""),
            row("rozi extensions update --all", ""),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--all", "Update every Git-managed extension"),
            row("-h, --help", "Print help"),
        ],
    },
];

const CHECK_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi extensions check <PATH> [--json]", "")],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("--json", "Print the validation report as JSON"),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi extensions", "manage extensions");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn print_check_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi extensions check", "validate an unpacked extension");
    append_help_sections(&mut out, CHECK_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn print_install_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi extensions install", "install and enable an extension");
    append_help_sections(&mut out, INSTALL_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn print_remove_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi extensions remove", "remove an installed extension");
    append_help_sections(&mut out, REMOVE_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn print_update_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi extensions update", "update managed Git extensions");
    append_help_sections(&mut out, UPDATE_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(super) fn parse(
    iter: &mut Peekable<IntoIter<String>>,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let config_path = cli.config_path.clone();
    if iter
        .clone()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return match iter.peek().map(String::as_str) {
            Some("check") => Ok(ParsedCli::ExtensionsCheckHelp),
            Some("install") => Ok(ParsedCli::ExtensionsInstallHelp),
            Some("remove") => Ok(ParsedCli::ExtensionsRemoveHelp),
            Some("update") => Ok(ParsedCli::ExtensionsUpdateHelp),
            _ => Ok(ParsedCli::ExtensionsHelp),
        };
    }

    match iter.next().as_deref() {
        None => Ok(ParsedCli::ExtensionsHelp),
        Some("list") => parse_list(iter, config_path),
        Some("install") => parse_install(iter, config_path),
        Some("update") => parse_update(iter, config_path),
        Some("remove") => parse_remove(iter, config_path),
        Some("new") => parse_new(iter),
        Some("check") => parse_check(iter),
        Some("logs") => parse_logs(iter),
        Some("deploy") => parse_deploy(iter, cli),
        Some("undeploy") => parse_undeploy(iter, cli),
        Some("status") => parse_status(iter, cli),
        Some("runtime") => {
            if iter.next().is_some() {
                return Err("extensions runtime accepts no arguments".to_string());
            }
            Ok(ParsedCli::Extensions(ExtensionsCommand::Runtime))
        }
        Some(other) => Err(format!(
            "unknown extensions command `{other}` (expected list, install, update, remove, new, check, deploy, undeploy, status, or logs)"
        )),
    }
}

/// Where a session-owned extension command goes: one named session, on this machine or through
/// `--remote`.
struct SessionTarget {
    session: String,
    remote: Option<String>,
}

/// `--session` and `--remote`, from either side of the `extensions` namespace. A target given before
/// it lives on `cli`; one given after it is collected here.
struct SessionTargetFlags {
    session: Option<String>,
    flag_target: bool,
    remote: Option<String>,
    command: &'static str,
}

impl SessionTargetFlags {
    fn new(cli: &CliArgs, command: &'static str) -> Self {
        Self {
            session: cli.attach_session.clone(),
            flag_target: cli.session_flag_target,
            remote: cli.remote.clone(),
            command,
        }
    }

    /// Take `arg` and its value if it is one of these flags. `Ok(false)` leaves it to the caller.
    fn accept(
        &mut self,
        arg: &str,
        iter: &mut impl Iterator<Item = String>,
    ) -> std::result::Result<bool, String> {
        let command = self.command;
        match arg {
            "--session" => {
                let missing = format!("extensions {command} --session requires a name");
                if self
                    .session
                    .replace(require_value(iter, &missing)?)
                    .is_some()
                {
                    return Err(format!(
                        "extensions {command} names a session more than once"
                    ));
                }
                self.flag_target = true;
            }
            "--remote" => {
                if self.remote.is_some() {
                    return Err(format!(
                        "extensions {command} names a remote host more than once"
                    ));
                }
                let missing = format!("extensions {command} --remote requires a host");
                let target = require_value(iter, &missing)?;
                crate::session::remote::parse_remote_target(&target)?;
                self.remote = Some(target);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The session named, or `None` when no flag named anything to address.
    fn target(self) -> std::result::Result<Option<SessionTarget>, String> {
        let Some(session) = self.session else {
            if self.remote.is_some() {
                return Err(format!(
                    "extensions {} --remote needs --session <NAME>: deployments belong to a named session",
                    self.command
                ));
            }
            return Ok(None);
        };
        if !self.flag_target {
            return Err(format!(
                "`{session}` is a launch target; write `--session {session}` to address that session"
            ));
        }
        Ok(Some(SessionTarget {
            session,
            remote: self.remote,
        }))
    }
}

/// The target of a command that accepts nothing else and has no use without one.
fn session_target(
    cli: &CliArgs,
    iter: &mut impl Iterator<Item = String>,
    command: &'static str,
) -> std::result::Result<SessionTarget, String> {
    let mut flags = SessionTargetFlags::new(cli, command);
    while let Some(arg) = iter.next() {
        if !flags.accept(&arg, iter)? {
            return Err(format!(
                "unexpected argument `{arg}` after extensions {command}"
            ));
        }
    }
    flags.target()?.ok_or_else(|| {
        format!(
            "extensions {command} needs --session <NAME>: deployments belong to a session server"
        )
    })
}

fn parse_deploy(
    iter: &mut Peekable<IntoIter<String>>,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let Some(first) = iter.next() else {
        return Err("extensions deploy requires an extension id".to_string());
    };
    if first.starts_with('-') {
        return Err(format!("unknown extensions deploy option `{first}`"));
    }
    let target = session_target(&cli, iter, "deploy")?;
    Ok(ParsedCli::ExtensionsDeploy(ExtensionsDeployCli {
        id: first,
        session: target.session,
        remote: target.remote,
        config_path: cli.config_path,
    }))
}

fn parse_undeploy(
    iter: &mut Peekable<IntoIter<String>>,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let Some(first) = iter.next() else {
        return Err("extensions undeploy requires an extension id".to_string());
    };
    if first.starts_with('-') {
        return Err(format!("unknown extensions undeploy option `{first}`"));
    }
    let target = session_target(&cli, iter, "undeploy")?;
    let endpoint = endpoint_for(&target);
    Ok(ParsedCli::Control(ControlCli {
        config_path: cli.config_path,
        endpoint,
        request: control_request(crate::control::ControlCommand::ExtensionsUndeploy { id: first }),
        output_format: None,
        output: None,
    }))
}

fn parse_status(
    iter: &mut Peekable<IntoIter<String>>,
    cli: CliArgs,
) -> std::result::Result<ParsedCli, String> {
    let mut format = None;
    let mut flags = SessionTargetFlags::new(&cli, "status");
    while let Some(arg) = iter.next() {
        if arg == "--json" && format.is_none() {
            format = Some(ListFormat::Json);
        } else if !flags.accept(&arg, iter)? {
            return Err(format!(
                "unexpected argument `{arg}` after extensions status"
            ));
        }
    }
    let (endpoint, command) = match flags.target()? {
        None => (
            ControlEndpoint::Ui(None),
            crate::control::ControlCommand::ExtensionRuntimeStatus,
        ),
        Some(target) => (
            endpoint_for(&target),
            crate::control::ControlCommand::ExtensionsStatus,
        ),
    };
    Ok(ParsedCli::Control(ControlCli {
        config_path: cli.config_path,
        endpoint,
        request: control_request(command),
        output_format: format,
        output: None,
    }))
}

fn endpoint_for(target: &SessionTarget) -> ControlEndpoint {
    match &target.remote {
        Some(host) => ControlEndpoint::Remote {
            target: host.clone(),
            session: target.session.clone(),
        },
        None => ControlEndpoint::Session(target.session.clone()),
    }
}

/// Lines `extensions logs` prints per unit unless told otherwise.
const DEFAULT_LOG_LINES: usize = 50;

fn parse_logs(iter: &mut impl Iterator<Item = String>) -> std::result::Result<ParsedCli, String> {
    let mut id = None;
    let mut unit = None;
    let mut lines = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--unit" if unit.is_none() => {
                unit = Some(require_value(
                    iter,
                    "extensions logs --unit requires a unit name",
                )?);
            }
            "--unit" => return Err("extensions logs --unit specified more than once".to_string()),
            "--lines" | "-n" if lines.is_none() => {
                let value = require_value(iter, "extensions logs --lines requires a number")?;
                lines = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|count| *count > 0)
                        .ok_or_else(|| {
                            format!(
                                "extensions logs --lines expects a positive number, got `{value}`"
                            )
                        })?,
                );
            }
            "--lines" | "-n" => {
                return Err("extensions logs --lines specified more than once".to_string());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown extensions logs option `{other}`"));
            }
            _ if id.is_none() => id = Some(arg),
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after extensions logs"
                ));
            }
        }
    }
    let id = id.ok_or_else(|| "extensions logs requires an extension id".to_string())?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::Logs {
        id,
        unit,
        lines: lines.unwrap_or(DEFAULT_LOG_LINES),
    }))
}

fn parse_list(
    iter: &mut impl Iterator<Item = String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let mut json = false;
    let mut verbose = false;
    for flag in iter {
        match flag.as_str() {
            "--json" if !json => json = true,
            "--json" => {
                return Err("extensions list --json specified more than once".to_string());
            }
            "--verbose" if !verbose => verbose = true,
            "--verbose" => {
                return Err("extensions list --verbose specified more than once".to_string());
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after extensions list"
                ));
            }
        }
    }
    Ok(ParsedCli::Extensions(ExtensionsCommand::List {
        json,
        verbose,
        config_path,
    }))
}

fn parse_install(
    iter: &mut impl Iterator<Item = String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let first = iter
        .next()
        .ok_or_else(|| "extensions install requires a source".to_string())?;
    let (source, link) = if first == "--link" {
        (
            require_value(iter, "extensions install --link requires a local path")?,
            true,
        )
    } else if first.starts_with('-') {
        return Err(format!("unknown extensions install option `{first}`"));
    } else {
        (first, false)
    };
    reject_trailing_control_args(iter, "extensions install")?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::Install {
        source,
        link,
        config_path,
    }))
}

fn parse_update(
    iter: &mut impl Iterator<Item = String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let missing = "extensions update requires an extension id or --all";
    let id = match iter.next() {
        Some(arg) if arg == "--all" => None,
        Some(arg) if arg.starts_with('-') => {
            return Err(format!("unknown extensions update option `{arg}`"));
        }
        Some(arg) => Some(arg),
        None => return Err(missing.to_string()),
    };
    reject_trailing_control_args(iter, "extensions update")?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::Update {
        id,
        config_path,
    }))
}

fn parse_remove(
    iter: &mut impl Iterator<Item = String>,
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
    let mut id = None;
    let mut purge = false;
    for arg in iter {
        match arg.as_str() {
            "--purge" if !purge => purge = true,
            "--purge" => {
                return Err("extensions remove --purge specified more than once".to_string());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown extensions remove option `{other}`"));
            }
            _ if id.is_none() => id = Some(arg),
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after extensions remove"
                ));
            }
        }
    }
    let id = id.ok_or_else(|| "extensions remove requires an extension id".to_string())?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::Remove {
        id,
        purge,
        config_path,
    }))
}

fn parse_new(iter: &mut impl Iterator<Item = String>) -> std::result::Result<ParsedCli, String> {
    let id = require_value(iter, "extensions new requires an extension id")?;
    reject_trailing_control_args(iter, "extensions new")?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::New { id }))
}

fn parse_check(iter: &mut impl Iterator<Item = String>) -> std::result::Result<ParsedCli, String> {
    let path = PathBuf::from(require_value(
        iter,
        "extensions check requires an extension directory or extension.toml path",
    )?);
    let mut json = false;
    for flag in iter {
        match flag.as_str() {
            "--json" if !json => json = true,
            "--json" => {
                return Err("extensions check --json specified more than once".to_string());
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}` after extensions check"
                ));
            }
        }
    }
    Ok(ParsedCli::Extensions(ExtensionsCommand::Check {
        path,
        json,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::parse_cli_args;
    use super::*;

    #[test]
    fn status_asks_the_ui_and_the_runtime_subcommand_takes_no_arguments() {
        let parse =
            |args: &[&str]| parse_cli_args(args.iter().map(|arg| arg.to_string()).collect());
        let Ok(ParsedCli::Control(cli)) = parse(&["extensions", "status", "--json"]) else {
            panic!("status is a control command");
        };
        assert_eq!(
            cli.request.command,
            crate::control::ControlCommand::ExtensionRuntimeStatus
        );
        assert_eq!(cli.endpoint, ControlEndpoint::Ui(None));
        assert_eq!(cli.output_format, Some(ListFormat::Json));
        assert!(parse(&["extensions", "status", "--verbose"]).is_err());
        assert!(matches!(
            parse(&["extensions", "runtime"]),
            Ok(ParsedCli::Extensions(ExtensionsCommand::Runtime))
        ));
        assert!(parse(&["extensions", "runtime", "extra"]).is_err());
    }

    #[test]
    fn deployment_commands_target_one_named_session() {
        let parse =
            |args: &[&str]| parse_cli_args(args.iter().map(|arg| arg.to_string()).collect());

        let Ok(ParsedCli::ExtensionsDeploy(command)) =
            parse(&["extensions", "deploy", "demo", "--session", "work"])
        else {
            panic!("deploy is its own command");
        };
        assert_eq!(command.id, "demo");
        assert_eq!(command.session, "work");
        assert_eq!(command.remote, None);

        // The session may also come before the namespace, where the global parser sees it.
        let Ok(ParsedCli::ExtensionsDeploy(command)) =
            parse(&["--session", "work", "extensions", "deploy", "demo"])
        else {
            panic!("deploy accepts the session before `extensions`");
        };
        assert_eq!(command.session, "work");

        let Ok(ParsedCli::ExtensionsDeploy(command)) = parse(&[
            "extensions",
            "deploy",
            "demo",
            "--remote",
            "build-host",
            "--session",
            "work",
        ]) else {
            panic!("deploy accepts --remote");
        };
        assert_eq!(command.remote.as_deref(), Some("build-host"));

        // A deployment belongs to one session server, so there is no default target.
        let error = parse(&["extensions", "deploy", "demo"]).expect_err("no session");
        assert!(error.contains("--session"), "{error}");
        let error = parse(&["extensions", "deploy", "demo", "--remote", "build-host"])
            .expect_err("a remote host is not a session");
        assert!(error.contains("--session"), "{error}");
        let error = parse(&[
            "extensions",
            "deploy",
            "demo",
            "--session",
            "work",
            "--session",
            "other",
        ])
        .expect_err("one session at a time");
        assert!(error.contains("more than once"), "{error}");
        let error = parse(&[
            "extensions",
            "deploy",
            "demo",
            "--session",
            "work",
            "--json",
        ])
        .expect_err("deploy streams a bundle, not a JSON report");
        assert!(error.contains("unexpected argument"), "{error}");

        let Ok(ParsedCli::Control(cli)) =
            parse(&["extensions", "undeploy", "demo", "--session", "work"])
        else {
            panic!("undeploy is a control command");
        };
        assert_eq!(cli.endpoint, ControlEndpoint::Session("work".to_string()));
        assert_eq!(
            cli.request.command,
            crate::control::ControlCommand::ExtensionsUndeploy {
                id: "demo".to_string()
            }
        );

        let Ok(ParsedCli::Control(cli)) =
            parse(&["extensions", "status", "--session", "work", "--json"])
        else {
            panic!("status with a session is a control command");
        };
        assert_eq!(cli.endpoint, ControlEndpoint::Session("work".to_string()));
        assert_eq!(
            cli.request.command,
            crate::control::ControlCommand::ExtensionsStatus
        );
        assert_eq!(cli.output_format, Some(ListFormat::Json));
    }

    #[test]
    fn extensions_namespace_parses_commands_and_help() {
        for args in [vec!["extensions"], vec!["extensions", "--help"]] {
            assert!(matches!(
                parse_cli_args(args.into_iter().map(str::to_string).collect()).expect("parses"),
                ParsedCli::ExtensionsHelp
            ));
        }
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "check".into(), "--help".into(),])
                .expect("parses"),
            ParsedCli::ExtensionsCheckHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "install".into(), "--help".into(),])
                .expect("parses"),
            ParsedCli::ExtensionsInstallHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "remove".into(), "--help".into(),])
                .expect("parses"),
            ParsedCli::ExtensionsRemoveHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "update".into(), "--help".into(),])
                .expect("parses"),
            ParsedCli::ExtensionsUpdateHelp
        ));

        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "list".into(),
                "--verbose".into(),
                "--json".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::List {
                json: true,
                verbose: true,
                ..
            })
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "check".into(),
                "./git-tools".into(),
                "--json".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Check { path, json: true })
                if path == std::path::Path::new("./git-tools")
        ));
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "new".into(), "git-tools".into()])
                .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::New { id }) if id == "git-tools"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "install".into(),
                "https://github.com/user/git-tools.git".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Install { source, link: false, .. })
                if source == "https://github.com/user/git-tools.git"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "install".into(),
                "--link".into(),
                "./git-tools".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Install { source, link: true, .. })
                if source == "./git-tools"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "update".into(),
                "git-tools".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Update { id, .. })
                if id.as_deref() == Some("git-tools")
        ));
        assert!(matches!(
            parse_cli_args(vec!["extensions".into(), "update".into(), "--all".into()])
                .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Update { id: None, .. })
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "remove".into(),
                "git-tools".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Remove { id, purge: false, .. }) if id == "git-tools"
        ));
        for args in [
            ["extensions", "remove", "git-tools", "--purge"],
            ["extensions", "remove", "--purge", "git-tools"],
        ] {
            assert!(matches!(
                parse_cli_args(args.iter().map(|arg| arg.to_string()).collect()).expect("parses"),
                ParsedCli::Extensions(ExtensionsCommand::Remove { id, purge: true, .. })
                    if id == "git-tools"
            ));
        }
        for args in [
            vec!["extensions", "remove", "git-tools", "--purge", "--purge"],
            vec!["extensions", "remove", "git-tools", "--force"],
            vec!["extensions", "remove", "git-tools", "other"],
            vec!["extensions", "remove", "--purge"],
        ] {
            assert!(
                parse_cli_args(args.iter().map(|arg| arg.to_string()).collect()).is_err(),
                "{args:?}"
            );
        }
        assert!(parse_cli_args(vec!["extensions".into(), "new".into()]).is_err());
        assert!(
            parse_cli_args(vec!["extensions".into(), "install".into(), "--link".into()]).is_err()
        );
        assert!(parse_cli_args(vec!["extensions".into(), "remove".into()]).is_err());
        assert!(parse_cli_args(vec!["extensions".into(), "update".into()]).is_err());
        for args in [
            vec!["--all", "git-tools"],
            vec!["git-tools", "--all"],
            vec!["--every"],
        ] {
            let mut argv = vec!["extensions".to_string(), "update".to_string()];
            argv.extend(args.iter().map(|arg| arg.to_string()));
            assert!(parse_cli_args(argv).is_err(), "{args:?}");
        }
        assert!(
            parse_cli_args(vec![
                "extensions".into(),
                "new".into(),
                "one".into(),
                "two".into(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_cli_args(vec!["extensions".into(), "lst".into()]).expect_err("must reject"),
            "unknown extensions command `lst` (expected list, install, update, remove, new, check, deploy, undeploy, status, or logs)"
        );
    }

    #[test]
    fn extensions_list_carries_the_config_path() {
        assert!(matches!(
            parse_cli_args(vec![
                "--config".into(),
                "/tmp/alt.toml".into(),
                "extensions".into(),
                "list".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::List { config_path, .. })
                if config_path.as_deref() == Some("/tmp/alt.toml")
        ));
    }

    #[test]
    fn logs_takes_an_id_with_an_optional_unit_and_a_positive_line_count() {
        let parse = |args: &[&str]| {
            let mut full = vec!["extensions".to_string(), "logs".to_string()];
            full.extend(args.iter().map(|arg| arg.to_string()));
            parse_cli_args(full)
        };
        assert!(matches!(
            parse(&["tool"]),
            Ok(ParsedCli::Extensions(ExtensionsCommand::Logs { id, unit: None, lines }))
                if id == "tool" && lines == DEFAULT_LOG_LINES
        ));
        assert!(matches!(
            parse(&["--unit", "service.watch", "tool", "-n", "5"]),
            Ok(ParsedCli::Extensions(ExtensionsCommand::Logs { id, unit: Some(unit), lines: 5 }))
                if id == "tool" && unit == "service.watch"
        ));
        for bad in [
            &[][..],
            &["tool", "--lines", "0"],
            &["tool", "--lines", "many"],
            &["tool", "--unit"],
            &["tool", "--unit", "a", "--unit", "b"],
            &["tool", "other"],
            &["tool", "--follow"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
