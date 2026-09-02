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
    let mut out = format!(
        "{}rozi extensions{} - manage extensions\n",
        styles.title, styles.reset
    );
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn print_check_help() {
    let styles = HelpStyles::detect();
    let mut out = format!(
        "{}rozi extensions check{} - validate an unpacked extension\n",
        styles.title, styles.reset
    );
    append_help_sections(&mut out, CHECK_HELP_SECTIONS, &styles, true);
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
        return if iter.peek().is_some_and(|arg| arg == "check") {
            Ok(ParsedCli::ExtensionsCheckHelp)
        } else {
            Ok(ParsedCli::ExtensionsHelp)
        };
    }

    match iter.next().as_deref() {
        None => Ok(ParsedCli::ExtensionsHelp),
        Some("list") => parse_list(iter, config_path),
        Some("new") => parse_new(iter),
        Some("check") => parse_check(iter),
        Some(other) => Err(format!(
            "unknown extensions command `{other}` (expected list, new, or check)"
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
        assert!(parse_cli_args(vec!["extensions".into(), "new".into()]).is_err());
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
            "unknown extensions command `lst` (expected list, new, or check)"
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
