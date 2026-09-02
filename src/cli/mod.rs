//! The `rozi` command line.
//!
//! [`args`] turns an argv into a [`ParsedCli`]; the remaining modules are the subcommand families
//! that run one. `main.rs` is the only caller — it matches on the parse and dispatches here.
//!
//! Human-facing report formatting lives in [`output`] rather than in each subcommand, so a table
//! rendered by `sessions list` and one rendered by `extensions list` cannot drift apart.

pub(crate) mod args;
pub(crate) mod control;
pub(crate) mod extension;
pub(crate) mod help;
pub(crate) mod output;
pub(crate) mod session;
pub(crate) mod skill;
pub(crate) mod update;

pub(crate) use args::{
    CliArgs, ExtensionsCommand, ParsedCli, SessionCommand, SessionsCommand, parse_cli_args,
    print_extensions_check_help, print_extensions_help, print_extensions_install_help,
    print_extensions_remove_help, print_sessions_help,
};
pub(crate) use control::{run_control_cli, run_pick_cli, run_publish_cli, run_subscribe_cli};
pub(crate) use extension::{
    run_check_extension_cli, run_install_extension_cli, run_list_extensions_cli,
    run_new_extension_cli, run_remove_extension_cli,
};
pub(crate) use help::{print_help, print_version};
pub(crate) use session::{
    run_kill_session_cli, run_list_sessions_cli, run_remote_serve_cli, run_server_cli,
};
pub(crate) use skill::{print_skill_help, run_skill_cli};
pub(crate) use update::{recover_managed_installation, run_install_cli, run_update_cli};

#[cfg(test)]
mod tests {
    use super::extension::format_extensions_text;
    use super::output::{OutputStyles, format_panes_text};
    use super::session::format_sessions_text;

    /// Every human report says why it is empty rather than printing nothing, and they all say it
    /// the same way. The wording is shared convention, not a per-command choice, so it is asserted
    /// across the three together.
    #[test]
    fn empty_human_reports_say_what_was_not_found() {
        assert_eq!(
            format_sessions_text(&[], OutputStyles::plain()),
            "No sessions found.\n"
        );
        assert_eq!(
            format_panes_text(None, OutputStyles::plain()),
            "No panes found.\n"
        );
        // Extensions add the one thing that answers the question: extensions live in the data
        // directory, and the config directory next to it is the obvious wrong guess.
        assert_eq!(
            format_extensions_text(
                &[],
                std::path::Path::new("/data/rozi/extensions"),
                false,
                OutputStyles::plain()
            ),
            "No extensions found in /data/rozi/extensions.\n"
        );
    }
}
