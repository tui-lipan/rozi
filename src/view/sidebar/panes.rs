use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;

pub(super) fn panes_rows(ctx: &Context<HyprmuxApp>) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    for (workspace_index, workspace) in ctx.state.current().workspaces.iter().enumerate() {
        let panes: Vec<_> = workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .collect();
        if panes.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(SidebarRow::spacer());
        }
        let workspace_label = super::workspace_heading(&ctx.state, workspace_index);
        rows.push(SidebarRow::header(row::header(ctx, workspace_label, false)));
        for pane in panes {
            let id = pane.id;
            let program = pane
                .terminal
                .foreground_program
                .as_deref()
                .unwrap_or("shell");
            let row = Row::new(pane_identity(pane))
                .active(ctx.state.current().focused_pane == Some(id))
                .title_style(super::super::fg_only(&ctx.state.theme.primary));
            let row = match pane_location(pane) {
                Some(location) => row
                    .badge_text(program, super::super::fg_only(&ctx.state.theme.muted).dim())
                    .detail(
                        row::truncate_start(
                            &location,
                            location_budget(ctx.state.config.sidebar.width),
                        ),
                        super::super::fg_only(&ctx.state.theme.muted),
                    ),
                // Nothing is known about where the pane is, so what it runs takes the line the
                // location would have used rather than leaving the row a lone title.
                None => row.detail(program, super::super::fg_only(&ctx.state.theme.muted)),
            };
            rows.push(
                SidebarRow::item(row, RowTarget::Pane(id))
                    .closable(crate::state::SidebarClose::Pane(id)),
            );
        }
    }
    rows
}

/// What a pane row calls itself.
///
/// A user-set title always wins. After that the terminal title is only trusted when a program
/// actually chose it (`nvim src/main.rs`): a shell sets OSC 2 to its prompt, which is the same
/// `user@host:` on every row followed by a path that clips before it says anything. For those the
/// working directory's leaf is the honest identity — it is the thing that differs between panes.
fn pane_identity(pane: &crate::state::Pane) -> String {
    if let Some(custom) = pane.identity.custom_title.as_deref() {
        return custom.to_string();
    }
    if let Some(title) = pane.terminal.title()
        && prompt_echo_path(&title).is_none()
    {
        return title;
    }
    if let Some(leaf) =
        pane_cwd(pane).and_then(|cwd| crate::platform::paths::path_leaf(&cwd).map(str::to_string))
    {
        return leaf;
    }
    pane.display_title(pane.terminal.title())
}

/// The pane's working directory: the one the server reports live, else the directory it was
/// launched in, else whatever a shell prompt put in the title. The last two matter on a host where
/// hyprmux cannot inspect the process for a live cwd — without them such a pane would know nothing
/// about itself.
fn pane_cwd(pane: &crate::state::Pane) -> Option<String> {
    let usable = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    usable(pane.terminal.cwd.as_deref())
        .or_else(|| usable(pane.identity.cwd.as_deref()))
        .or_else(|| {
            pane.terminal
                .title()
                .and_then(|title| prompt_echo_path(&title).map(str::to_string))
        })
}

/// Where the pane is, for the detail line: its working directory with `$HOME` compressed to `~`,
/// and an `@host` suffix when the shell reports a directory on another machine — whose home is not
/// this machine's, so it is left uncompressed.
fn pane_location(pane: &crate::state::Pane) -> Option<String> {
    let cwd = pane_cwd(pane)?;
    Some(match pane.terminal.cwd_host.as_deref() {
        Some(host) if !host.is_empty() => format!("{cwd}@{host}"),
        _ => crate::platform::paths::compress_home(&cwd),
    })
}

/// Character budget for the location line: the sidebar's configured width less the gutter, the
/// detail line's leading indent, the divider and the scrollbar column.
fn location_budget(width: u16) -> usize {
    usize::from(width).saturating_sub(4).max(8)
}

/// The working directory a shell prompt echoed into OSC 2, if that is what the title is.
///
/// Shells conventionally set the title to `user@host:cwd`, or to a bare `cwd`. Neither is an
/// identity: the `user@host` half is the same on every row, and the path is what the location line
/// already says — so as a title it clips away the leaf and tells you nothing. Recognizing it also
/// recovers a working directory on a host where hyprmux cannot inspect the process for one.
///
/// Matched by shape and deliberately narrow: the remainder must be a bare path and nothing else, so
/// a real title that merely mentions one (`nvim ~/Work/hyprmux/src/main.rs`) is left alone.
fn prompt_echo_path(title: &str) -> Option<&str> {
    crate::pane::shell_title_parts(title).map(|(_, cwd)| cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Pane;
    use tui_lipan::prelude::FloatRect;

    fn pane(title: Option<&str>, cwd: Option<&str>) -> Pane {
        let mut pane = Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 20.0,
                h: 10.0,
            },
        );
        pane.terminal.title = title.map(str::to_string);
        pane.terminal.cwd = cwd.map(str::to_string);
        pane
    }

    #[test]
    fn a_shell_prompt_title_is_recognized_by_shape_and_yields_its_path() {
        // The two conventional spellings, with and without the `user@host:` half.
        assert_eq!(
            prompt_echo_path("razuer@razuer:~/Work/Projects/hyprmux"),
            Some("~/Work/Projects/hyprmux")
        );
        assert_eq!(prompt_echo_path("/home/you/repo"), Some("/home/you/repo"));
        assert_eq!(prompt_echo_path("~"), Some("~"));

        // A title a program chose is left alone, even when it names a path.
        assert_eq!(prompt_echo_path("nvim ~/Work/hyprmux/src/main.rs"), None);
        assert_eq!(prompt_echo_path("cargo test"), None);
        // A colon that is not a `user@host` separator must not be eaten.
        assert_eq!(prompt_echo_path("make: *** [all] Error 1"), None);
        assert_eq!(prompt_echo_path("~/a:b"), Some("~/a:b"));
    }

    #[test]
    fn identity_prefers_a_real_title_and_falls_back_to_the_directory_leaf() {
        // A prompt echo is not an identity: the directory leaf is what differs between panes.
        assert_eq!(
            pane_identity(&pane(
                Some("razuer@razuer:~/Work/Projects/hyprmux"),
                Some("/home/razuer/Work/Projects/hyprmux")
            )),
            "hyprmux"
        );
        // A title a program set describes the pane better than its directory does.
        assert_eq!(
            pane_identity(&pane(Some("nvim src/main.rs"), Some("/home/you/repo"))),
            "nvim src/main.rs"
        );
        // A user-set title outranks both.
        let mut named = pane(Some("nvim src/main.rs"), Some("/home/you/repo"));
        named.set_custom_title("build");
        assert_eq!(pane_identity(&named), "build");
    }

    #[test]
    fn location_recovers_a_directory_from_a_prompt_title_and_marks_remote_ones() {
        // No reported cwd, but the prompt title carries one.
        assert_eq!(
            pane_location(&pane(Some("you@host:~/repo"), None)).as_deref(),
            Some("~/repo")
        );
        // A remote directory keeps its host and is never compressed against this machine's home.
        let mut remote = pane(None, Some("/srv/build"));
        remote.terminal.cwd_host = Some("builder".into());
        assert_eq!(
            pane_location(&remote).as_deref(),
            Some("/srv/build@builder")
        );
        // Nothing known at all: the row falls back to its program instead.
        assert_eq!(pane_location(&pane(None, None)), None);
    }
}
