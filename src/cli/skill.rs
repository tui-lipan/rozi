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
            row(
                "install [--global] [--force]",
                "Install or refresh the Rozi skill",
            ),
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
            row("    --force", "Replace a modified or untracked skill"),
        ],
    },
];

fn print_skill() {
    skill::print_skill();
}

pub(crate) fn print_skill_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi skill", "install the built-in Rozi agent skill");
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
        SkillCommand::Install { global, force } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let state_dir = crate::platform::paths::state_dir(
                &crate::platform::paths::PlatformEnv::from_process(),
            );
            let managed = skill::install_managed(
                &paths,
                &state_dir,
                crate::agent_detection::claude_cli_available(),
                force,
            )
            .map_err(std::io::Error::other)?;
            let report = skill::InstallReport {
                skill_file: managed.skill_file,
                claude: managed.claude,
            };
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
            let state_dir = crate::platform::paths::state_dir(
                &crate::platform::paths::PlatformEnv::from_process(),
            );
            skill::forget_install(&paths, &state_dir).map_err(std::io::Error::other)?;
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
            "rozi --session dev list-panes",
            "rozi --session dev capture-pane --target <PANE_ID>",
            "rozi --session dev send-keys --target <PANE_ID>",
            "rozi --session dev split",
            "rozi --session dev status --target <PANE_ID>",
            "rozi split [COMMAND]",
            "send-text",
            "send-keys --target <PANE_ID>",
            "capture-pane",
            "status --clear",
            "status --target <PANE_ID>",
            "pty_ready:true",
            "does **not** move focus",
            "queues it as type-ahead",
            "rozi notify 'tests failed'",
            "rozi pick --title Branch",
            "rozi subscribe pane-exited",
            "`publish` is a long-lived",
            "rozi sessions list",
            "rozi sessions kill <NAME>",
            "A session endpoint ignores `ROZI_PANE`",
            "`--socket` and `--session` cannot be combined",
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
