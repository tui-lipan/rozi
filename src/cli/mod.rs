//! The `rozi` command line.
//!
//! [`args`] turns an argv into a [`ParsedCli`]; the remaining modules are the subcommand families
//! that run one. `main.rs` is the only caller — it matches on the parse and dispatches here.
//!
//! Human-facing report formatting lives in [`output`] rather than in each subcommand, so a table
//! rendered by `list-sessions` and one rendered by `list-extensions` cannot drift apart.

pub(crate) mod args;
pub(crate) mod control;
pub(crate) mod extension;
pub(crate) mod help;
pub(crate) mod output;
pub(crate) mod session;
pub(crate) mod skill;
pub(crate) mod update;

pub(crate) use args::{CliArgs, ParsedCli, SessionCommand, parse_cli_args};
pub(crate) use control::{run_control_cli, run_pick_cli, run_publish_cli, run_subscribe_cli};
pub(crate) use extension::{
    run_check_extension_cli, run_list_extensions_cli, run_new_extension_cli,
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
        assert_eq!(
            format_extensions_text(&[], false, OutputStyles::plain()),
            "No extensions found.\n"
        );
    }
}
