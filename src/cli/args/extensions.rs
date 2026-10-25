use std::iter::Peekable;
use std::path::PathBuf;
use std::vec::IntoIter;

use super::{
    ControlCli, ControlEndpoint, ExtensionsCommand, ListFormat, ParsedCli, control_request,
    reject_trailing_control_args, require_value,
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
                "status [--json]",
                "Show where placed extension processes run",
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
    config_path: Option<String>,
) -> std::result::Result<ParsedCli, String> {
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
        Some("status") => {
            let mut format = None;
            for flag in iter {
                match flag.as_str() {
                    "--json" if format.is_none() => format = Some(ListFormat::Json),
                    other => {
                        return Err(format!(
                            "unexpected argument `{other}` after extensions status"
                        ));
                    }
                }
            }
            Ok(ParsedCli::Control(ControlCli {
                config_path,
                endpoint: ControlEndpoint::Ui(None),
                request: control_request(crate::control::ControlCommand::ExtensionRuntimeStatus),
                output_format: format,
                output: None,
            }))
        }
        Some("runtime") => {
            if iter.next().is_some() {
                return Err("extensions runtime accepts no arguments".to_string());
            }
            Ok(ParsedCli::Extensions(ExtensionsCommand::Runtime))
        }
        Some(other) => Err(format!(
            "unknown extensions command `{other}` (expected list, install, update, remove, new, check, status, or logs)"
        )),
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
            "unknown extensions command `lst` (expected list, install, update, remove, new, check, status, or logs)"
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
