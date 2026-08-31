//! The `rozi skill` subcommand: installing, removing, and printing the embedded agent skill.

use tui_lipan::Result;

use super::args::SkillCommand;
use super::help::{HelpSection, HelpStyles, append_help_sections, row};
use super::output::{OutputStyles, OutputTone, style_first_line};
use crate::skill;

pub(super) const SKILL_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi skill [COMMAND] [OPTIONS]", "")],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "",
        rows: &[
            row("install [--global]", "Install the Rozi skill"),
            row("uninstall [--global]", "Remove the installed Rozi skill"),
            row("status [--global]", "Show skill installation status"),
            row("print", "Print the skill to stdout"),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "    --global",
                "Install, uninstall, or status for this user",
            ),
            row("-h, --help", "Print help"),
        ],
    },
];

fn print_skill() {
    skill::print_skill();
}

pub(crate) fn print_skill_help() {
    let styles = HelpStyles::detect();
    let mut out = format!(
        "{}rozi skill{} - install the built-in Rozi agent skill\n",
        styles.title, styles.reset
    );
    append_help_sections(&mut out, SKILL_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn run_skill_cli(command: SkillCommand) -> Result<()> {
    let styles = OutputStyles::detect();
    match command {
        SkillCommand::Print => {
            print_skill();
            Ok(())
        }
        SkillCommand::Install { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::install(&paths, crate::agent_detection::claude_cli_available())
                .map_err(std::io::Error::other)?;
            print!(
                "{}",
                style_first_line(
                    skill::format_install(&report, &paths),
                    OutputTone::Success,
                    styles
                )
            );
            Ok(())
        }
        SkillCommand::Uninstall { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::uninstall(&paths).map_err(std::io::Error::other)?;
            let tone = if report.removed.is_empty() {
                OutputTone::Warning
            } else {
                OutputTone::Success
            };
            print!(
                "{}",
                style_first_line(skill::format_uninstall(&report, &paths), tone, styles)
            );
            Ok(())
        }
        SkillCommand::Status { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::status(&paths, crate::agent_detection::claude_cli_available());
            print!(
                "{}",
                style_first_line(
                    skill::format_status(&report, &paths),
                    OutputTone::Heading,
                    styles
                )
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::skill::SKILL_MD;

    /// The skill document is the CLI contract coding agents read, so the surfaces it
    /// promises have to still be in it.
    #[test]
    fn the_contract_document_still_describes_every_surface_it_promises() {
        for section in [
            "---\nname: rozi",
            "ROZI=1",
            "ROZI_SOCKET",
            "ROZI_PANE",
            "--socket PATH",
            "rozi --help",
            "rozi list-panes",
            "rozi split [COMMAND]",
            "send-text",
            "send-keys",
            "capture-pane",
            "status --clear",
            "pty_ready:true",
            "does **not** move focus",
            "queued as type-ahead",
            "rozi list-sessions",
            "rozi kill-session <NAME>",
            "read-only",
            "Input lock",
        ] {
            assert!(
                SKILL_MD.contains(section),
                "missing skill section: {section}"
            );
        }
    }
}
