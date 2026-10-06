use tui_lipan::prelude::*;

use super::row::{Priority, Row, RowTarget, SidebarRow};
use crate::AppRoot;
use crate::state::{WorktreeTabItem, WorktreeTabRow};

/// The Worktrees tab: the focused pane's repository, then one row per checkout.
///
/// Each checkout reads like a session row. The branch is the title, PR/CI is the detail, and
/// the right edge says what the checkout is to Rozi: the session using it, or `primary`/`locked`/
/// `stale lock`.
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
            // The host is short and says which machine every row below lives on; the repository
            // name is the one that can afford to clip.
            WorktreeTabItem::Header { repository, host } => SidebarRow::header(
                super::row::header_with_note(ctx, repository, host, Priority::Description),
            ),
            WorktreeTabItem::Checkout(row) => checkout_row(ctx, row),
            WorktreeTabItem::Creating { branch } => SidebarRow::item(
                Row::new(branch)
                    .title_style(muted)
                    .priority(Priority::Description)
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
    // The right edge is the checkout's status rail and, under the pointer, what Enter does. Both
    // are short and both are the point of the row, so the branch name truncates before they do.
    let mut item = Row::new(branch)
        .active(row.current)
        .title_style(title_style)
        .priority(Priority::Description);

    let strongest = row
        .sessions
        .iter()
        .map(|session| presence(ctx, session))
        .max();
    // The right edge is one status rail: what the checkout is, then whether a session uses it.
    let state = if !tree.linked {
        Some(("primary", muted))
    } else if tree.lock.as_ref().is_some_and(|lock| lock.stale) {
        Some(("stale lock", Style::new().fg(theme.status.warning)))
    } else if tree.lock.is_some() {
        Some(("locked", muted))
    } else if tree.prunable {
        Some(("prunable", Style::new().fg(theme.status.warning)))
    } else {
        None
    };
    let marker = strongest.map(|strongest| {
        let count = row.sessions.len();
        let text = if count > 1 {
            format!("{}{count}", strongest.marker())
        } else {
            strongest.marker().to_string()
        };
        let style = match strongest {
            SessionPresence::Current => Style::new().fg(theme.status.success),
            SessionPresence::Running => super::super::fg_only(&theme.accent),
            SessionPresence::Background | SessionPresence::Restorable => muted,
        };
        (text, style)
    });
    item = match (row.removing, state, marker) {
        (true, _, _) => item.badge_text("removing…", muted),
        (false, Some((state, state_style)), Some((marker, marker_style))) => {
            item.badge_parts([(state.to_string(), state_style), (marker, marker_style)])
        }
        (false, None, Some((marker, style))) => item.badge_text(marker, style),
        (false, Some((state, style)), None) => item.badge_text(state, style),
        (false, None, None) => item,
    };
    // Under the pointer the rail says what Enter does. A closable row's hover is its ✕ instead.
    if let Some(strongest) = strongest {
        item = item.hover_badge_text(strongest.action(row.sessions.len() > 1), muted);
    } else if !row.closable && !row.removing {
        item = item.hover_badge_text("new session", muted);
    }

    let statuses = &ctx.state.sidebar.worktrees.statuses;
    let detail_dim = 0.4;
    #[cfg(feature = "ui-snapshot")]
    let detail_dim = ctx
        .state
        .sidebar
        .worktrees
        .detail_dim_preview
        .unwrap_or(detail_dim);
    let detail_style = |style: Style| style.transform_fg(ColorTransform::dim(detail_dim));
    if let Some(pr) = statuses.checkouts.get(&tree.path) {
        item = item.detail(
            format!("#{} · {}", pr.number, pr.status.label()),
            detail_style(crate::view::worktree_status_style(theme, pr.status)),
        );
    } else if tree.linked && tree.branch.is_some() && statuses.unavailable {
        item = item.detail("PR unavailable", detail_style(muted));
    }
    if row.force {
        item = item.armed_prompt("Dirty · again to force");
    } else if tree.lock.is_some() {
        // Only a stale lock leaves the row closable. The prompt names the removal, the destructive
        // part; the lock is lifted on the way.
        item = item.armed_prompt("Stale lock · again to remove");
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
