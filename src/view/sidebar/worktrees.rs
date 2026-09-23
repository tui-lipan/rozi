use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, SidebarRow};
use crate::AppRoot;
use crate::state::{WorktreeTabItem, WorktreeTabRow};

/// Cells a row's detail line loses to the gutter, its indent, and the scrollbar.
const DETAIL_INSET: usize = 5;

/// The Worktrees tab: the focused pane's repository, then one row per checkout.
///
/// Each checkout reads like a session row. The branch is the title, the path is the detail, and
/// the right edge says what the checkout is to Rozi: the session using it, or `primary`/`locked`.
/// The gutter bar marks the checkout the focused pane is in. Enter opens it, and the ✕ removes it
/// behind the same two-step confirmation every destructive sidebar row uses.
pub(super) fn worktrees_rows(ctx: &Context<AppRoot>) -> Vec<SidebarRow> {
    let theme = &ctx.state.theme;
    let muted = super::super::fg_only(&theme.muted);
    let accent = super::super::fg_only(&theme.accent);
    ctx.state
        .worktree_tab_items()
        .into_iter()
        .map(|item| match item {
            WorktreeTabItem::Header { repository, host } => {
                SidebarRow::header(super::row::header_with_note(ctx, repository, host))
            }
            WorktreeTabItem::Checkout(row) => checkout_row(ctx, row),
            WorktreeTabItem::Creating { branch } => SidebarRow::item(
                Row::new(branch)
                    .title_style(muted)
                    .badge_text("creating…", accent),
                RowTarget::Inert,
            ),
            WorktreeTabItem::Message(text) => {
                SidebarRow::item(Row::new(text).title_style(muted), RowTarget::Inert)
            }
            WorktreeTabItem::New => SidebarRow::item(
                Row::new("+ New worktree").title_style(accent),
                RowTarget::NewWorktree,
            ),
        })
        .collect()
}

/// How the sessions using a checkout stand, strongest first: the one on screen, one this client
/// holds in the background, one running on the host, or one that can only be restored. The Sessions
/// tab's markers, so a checkout says the same thing about its session as that session's own row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SessionPresence {
    Restorable,
    Running,
    Background,
    Current,
}

impl SessionPresence {
    fn marker(self) -> &'static str {
        match self {
            Self::Current | Self::Running => "●",
            Self::Background => "◐",
            Self::Restorable => "○",
        }
    }

    /// What activating the row does, shown in place of the marker under the pointer.
    fn action(self, several: bool) -> &'static str {
        match (self, several) {
            (_, true) => "choose",
            (Self::Current, false) => "current",
            (Self::Background, false) => "switch",
            (Self::Running, false) => "attach",
            (Self::Restorable, false) => "restore",
        }
    }
}

fn presence(
    ctx: &Context<AppRoot>,
    session: &crate::session::protocol::WorktreeSession,
) -> SessionPresence {
    let target = ctx
        .state
        .sidebar
        .worktrees
        .source
        .as_ref()
        .and_then(|(target, _)| target.as_ref());
    let current = ctx.state.current();
    if current.session_name.as_deref() == Some(session.name.as_str())
        && current.remote_target.as_ref() == target
    {
        SessionPresence::Current
    } else if ctx
        .state
        .attachment_by_identity(&session.name, target)
        .is_some()
    {
        SessionPresence::Background
    } else if session.running {
        SessionPresence::Running
    } else {
        SessionPresence::Restorable
    }
}

/// The checkout's folder, when its name says something the branch does not. Most checkouts are
/// named after their branch (`feat/login` in `feat-login`, `worktree-settings-regroup` in
/// `settings-regroup`), and repeating that costs the row a whole line.
fn folder_note(tree: &crate::git::worktrees::WorktreeInfo) -> Option<String> {
    let folder = tree
        .path
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())?;
    let Some(branch) = tree.branch.as_deref() else {
        return Some(folder.to_string());
    };
    let folder_lower = folder.to_ascii_lowercase();
    let slug = branch.replace('/', "-").to_ascii_lowercase();
    let last = branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .to_ascii_lowercase();
    let implied =
        slug.contains(&folder_lower) || (!last.is_empty() && folder_lower.contains(&last));
    (!implied).then(|| folder.to_string())
}

fn checkout_row(ctx: &Context<AppRoot>, row: WorktreeTabRow) -> SidebarRow {
    let theme = &ctx.state.theme;
    let muted = super::super::fg_only(&theme.muted);
    let tree = &row.tree;
    let branch = match (&tree.branch, tree.bare) {
        (_, true) => "(bare)".to_string(),
        (Some(branch), false) => branch.clone(),
        (None, false) => "(detached)".to_string(),
    };
    // A prunable checkout's directory is gone; its branch reads as a leftover, not a place to go.
    let title_style = if tree.prunable {
        muted
    } else {
        super::super::fg_only(&theme.primary)
    };
    let mut item = Row::new(branch)
        .active(row.current)
        .title_style(title_style);

    let strongest = row
        .sessions
        .iter()
        .map(|session| presence(ctx, session))
        .max();
    if row.removing {
        item = item.badge_text("removing…", muted);
    } else if let Some(strongest) = strongest {
        let count = row.sessions.len();
        let marker = if count > 1 {
            format!("{}{count}", strongest.marker())
        } else {
            strongest.marker().to_string()
        };
        let style = match strongest {
            SessionPresence::Current => Style::new().fg(theme.status.success),
            SessionPresence::Running => super::super::fg_only(&theme.accent),
            SessionPresence::Background | SessionPresence::Restorable => muted,
        };
        item = item
            .badge_text(marker, style)
            .hover_badge_text(strongest.action(count > 1), muted);
    } else {
        let state = if !tree.linked {
            Some(("primary", muted))
        } else if tree.locked {
            Some(("locked", muted))
        } else if tree.prunable {
            Some(("prunable", Style::new().fg(theme.status.warning)))
        } else {
            None
        };
        if let Some((state, style)) = state {
            item = item.badge_text(state, style);
        }
        // Rows without a ✕ say what Enter does instead; a closable row's hover is its ✕.
        if !row.closable {
            item = item.hover_badge_text("new session", muted);
        }
    }

    // The primary checkout is the repository the header already names.
    if tree.linked
        && let Some(folder) = folder_note(tree)
    {
        let budget = usize::from(ctx.state.sidebar_requested_width()).saturating_sub(DETAIL_INSET);
        item = item.detail(super::row::truncate_start(&folder, budget), muted);
    }
    if row.force {
        item = item.armed_prompt("Dirty · again to force");
    }
    let sidebar_row = SidebarRow::item(item, RowTarget::Worktree(tree.path.clone()));
    if row.closable {
        sidebar_row.closable(crate::state::SidebarClose::Worktree {
            path: tree.path.clone(),
            force: row.force,
        })
    } else {
        sidebar_row
    }
}

#[cfg(test)]
mod tests {
    use super::folder_note;

    fn tree(path: &str, branch: Option<&str>) -> crate::git::worktrees::WorktreeInfo {
        crate::git::worktrees::WorktreeInfo {
            path: path.into(),
            branch: branch.map(Into::into),
            detached: branch.is_none(),
            bare: false,
            prunable: false,
            linked: true,
            locked: false,
        }
    }

    #[test]
    fn a_folder_is_noted_only_when_the_branch_does_not_already_say_it() {
        for (path, branch) in [
            ("/src/rozi-worktrees/feat-login", "feat/login"),
            (
                "/src/rozi/.claude/worktrees/settings-regroup",
                "worktree-settings-regroup",
            ),
            ("/src/rozi/.worktrees/login", "feat/login"),
            ("C:\\wt\\Fix-Ssh", "fix/ssh"),
            ("/src/rozi-worktrees/release", "release/0.1"),
            ("/src/rozi-worktrees/rozi-feat-login", "feat/login"),
        ] {
            assert_eq!(folder_note(&tree(path, Some(branch))), None, "{path}");
        }
        assert_eq!(
            folder_note(&tree(
                "/src/rozi/.claude/worktrees/session-fade-duration",
                Some("fix/config-test-race")
            ))
            .as_deref(),
            Some("session-fade-duration")
        );
        assert_eq!(
            folder_note(&tree("/src/wt/scratch", None)).as_deref(),
            Some("scratch"),
            "a detached checkout has no branch to name it"
        );
    }
}
