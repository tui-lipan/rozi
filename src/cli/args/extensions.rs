use std::iter::Peekable;
use std::path::PathBuf;
use std::vec::IntoIter;

use super::{ExtensionsCommand, ParsedCli, reject_trailing_control_args, require_value};
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
            row("update <ID>", "Update a managed Git extension"),
            row("remove <ID>", "Remove an installed extension"),
            row("new <ID>", "Create a valid extension scaffold"),
            row("check <PATH> [--json]", "Validate an unpacked extension"),
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
        rows: &[row("rozi extensions remove <ID>", "")],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[row("-h, --help", "Print help")],
    },
];

const UPDATE_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi extensions update <ID>", "")],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[row("-h, --help", "Print help")],
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
    let mut out = styles.title_line("rozi extensions update", "update a managed Git extension");
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
        Some(other) => Err(format!(
            "unknown extensions command `{other}` (expected list, install, update, remove, new, or check)"
        )),
    }
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
    let id = require_value(iter, "extensions update requires an extension id")?;
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
    let id = require_value(iter, "extensions remove requires an extension id")?;
    reject_trailing_control_args(iter, "extensions remove")?;
    Ok(ParsedCli::Extensions(ExtensionsCommand::Remove {
        id,
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
            ParsedCli::Extensions(ExtensionsCommand::Update { id, .. }) if id == "git-tools"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "extensions".into(),
                "remove".into(),
                "git-tools".into(),
            ])
            .expect("parses"),
            ParsedCli::Extensions(ExtensionsCommand::Remove { id, .. }) if id == "git-tools"
        ));
        assert!(parse_cli_args(vec!["extensions".into(), "new".into()]).is_err());
        assert!(
            parse_cli_args(vec!["extensions".into(), "install".into(), "--link".into()]).is_err()
        );
        assert!(parse_cli_args(vec!["extensions".into(), "remove".into()]).is_err());
        assert!(parse_cli_args(vec!["extensions".into(), "update".into()]).is_err());
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
            "unknown extensions command `lst` (expected list, install, update, remove, new, or check)"
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
}
