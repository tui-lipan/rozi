use tui_lipan::prelude::*;

use super::row::{Priority, Row, RowTarget, Shape, SidebarRow};
use crate::state::{State, WorktreeTabItem, WorktreeTabRow};

/// The Worktrees tab: the focused pane's repository as a heading, with one row per checkout
/// beneath it.
///
/// Each checkout reads like a session row. The branch is the title, PR/CI is the detail, and
/// the right edge says what the checkout is to Rozi: the session using it, or `primary`/`locked`/
/// `stale lock`.
/// The gutter bar marks the checkout the focused pane is in. Enter opens it, and the ✕ removes it
/// behind the same two-step confirmation every destructive sidebar row uses.
pub(super) fn worktrees_rows(state: &State) -> Vec<SidebarRow> {
    let theme = &state.theme;
    let muted = super::super::fg_only(&theme.muted);
    let accent = super::super::fg_only(&theme.accent);
    state
        .worktree_tab_items()
        .into_iter()
        .enumerate()
        .map(|(index, item)| match item {
            // The host is short and says which machine every row below lives on; the repository
            // name is the one that can afford to clip.
            WorktreeTabItem::Header { repository, host } => SidebarRow::header(
                super::row::header_with_note(state, repository, host, Priority::Description),
            )
            .shape(Shape::Heading)
            .keyed("repository"),
            WorktreeTabItem::Checkout(row) => {
                let key = format!("checkout:{}", row.tree.path);
                checkout_row(state, row).at(1).keyed(key)
            }
            WorktreeTabItem::Creating { branch } => {
                let key = format!("creating:{branch}");
                SidebarRow::item(
                    Row::new(branch)
                        .title_style(muted)
                        .priority(Priority::Description)
                        .badge_text("creating…", accent),
                    RowTarget::Inert,
                )
                .at(1)
                .keyed(key)
            }
            WorktreeTabItem::Message(text) => {
                SidebarRow::item(Row::new(text).title_style(muted), RowTarget::Inert)
                    .at(1)
                    .keyed(format!("message:{index}"))
            }
            WorktreeTabItem::New => {
                SidebarRow::action(state, "New worktree", RowTarget::NewWorktree).keyed("new")
            }
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

fn presence(state: &State, session: &crate::session::protocol::WorktreeSession) -> SessionPresence {
    let target = state
        .sidebar
        .worktrees
        .source
        .as_ref()
        .and_then(|(target, _)| target.as_ref());
    let current = state.current();
    if current.session_name.as_deref() == Some(session.name.as_str())
        && current.remote_target.as_ref() == target
    {
        SessionPresence::Current
    } else if state
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

fn checkout_row(state: &State, row: WorktreeTabRow) -> SidebarRow {
    let theme = &state.theme;
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
    // Keep branch identity visible alongside the status rail. Short status tokens leave the
    // branch its natural width; longer descriptions cannot consume its last 40%.
    let mut item = Row::new(branch)
        .active(row.current)
        .title_style(title_style)
        .priority(Priority::Balanced);

    let strongest = row
        .sessions
        .iter()
        .map(|session| presence(state, session))
        .max();
    // The right edge is one status rail: what the checkout is, then whether a session uses it.
    let kind = if !tree.linked {
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
    item = match (row.removing, kind, marker) {
        (true, _, _) => {
            item.badge(crate::view::session_status::picker_circle_spinner(muted).label("removing…"))
        }
        (false, Some((kind, kind_style)), Some((marker, marker_style))) => {
            item.badge_parts([(kind.to_string(), kind_style), (marker, marker_style)])
        }
        (false, None, Some((marker, style))) => item.badge_text(marker, style),
        (false, Some((kind, style)), None) => item.badge_text(kind, style),
        (false, None, None) => item,
    };
    // Under the pointer the rail says what Enter does. A closable row's hover is its ✕ instead.
    if let Some(strongest) = strongest {
        item = item.hover_badge_text(strongest.action(row.sessions.len() > 1), muted);
    } else if !row.closable && !row.removing {
        item = item.hover_badge_text("new session", muted);
    }

    let statuses = &state.sidebar.worktrees.statuses;
    let detail_dim = 0.4;
    #[cfg(feature = "ui-snapshot")]
    let detail_dim = state
        .sidebar
        .worktrees
        .detail_dim_preview
        .unwrap_or(detail_dim);
    let detail_style = |style: Style| style.transform_fg(ColorTransform::dim(detail_dim));
    if let Some(pr) = statuses.checkouts.get(&tree.path) {
        item = item.detail(
            format!(
                "#{} · {}{}",
                pr.number,
                pr.status.label(),
                if statuses.unavailable {
                    " · stale"
                } else {
                    ""
                }
            ),
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
