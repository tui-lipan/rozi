use tui_lipan::prelude::*;

use super::row::{Priority, Row, RowTarget, Shape, SidebarRow};
use crate::state::{State, WorktreeTabItem, WorktreeTabRow};
use crate::view::vocabulary::{self, CheckoutKind, SessionPresence};

/// The Worktrees tab: the focused pane's repository as a heading, with one row per checkout
/// beneath it.
///
/// Each checkout reads like a session row. The branch is the title, PR/CI is the detail, and
/// the right edge says what the checkout is to Rozi: what kind of checkout it is, then the marker of
/// the session using it.
/// The gutter bar marks the checkout the focused pane is in. Under the pointer or cursor the right
/// edge names what Enter opens, or with Ctrl held what Ctrl+Enter opens, and the ✕ beside it
/// removes the checkout behind the same two-step confirmation every destructive sidebar row uses.
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
                        .badge_text("Creating…", accent),
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

/// How one session using a checkout stands: exactly what the Sessions picker would say of it. The
/// rail has room for the marker alone, so it shows a held connection that is reconnecting or lost
/// by its marker rather than passing it off as one in the background.
fn presence(state: &State, session: &crate::session::protocol::WorktreeSession) -> SessionPresence {
    let target = state
        .sidebar
        .worktrees
        .source
        .as_ref()
        .and_then(|(target, _)| target.as_ref());
    match vocabulary::session_presence_for(state, &session.name, target) {
        presence if presence.is_held() => presence,
        _ if session.running => SessionPresence::Running,
        _ => SessionPresence::Restorable,
    }
}

fn checkout_row(state: &State, row: WorktreeTabRow) -> SidebarRow {
    let theme = &state.theme;
    let muted = super::super::fg_only(&theme.muted);
    let tree = &row.tree;
    // A prunable checkout's directory is gone; its branch reads as a leftover, not a place to go.
    let title_style = if tree.prunable {
        muted
    } else {
        super::super::fg_only(&theme.primary)
    };
    // Keep branch identity visible alongside the status rail. Short status tokens leave the
    // branch its natural width; longer descriptions cannot consume its last 40%.
    let mut item = Row::new(vocabulary::checkout_branch(tree))
        .active(row.current)
        .title_style(title_style)
        .priority(Priority::Balanced);

    // Strongest first, so the smallest presence is the one the marker shows.
    let strongest = row
        .sessions
        .iter()
        .map(|session| presence(state, session))
        .min();
    // The right edge is one status rail: what the checkout is, then whether a session uses it.
    let kind = CheckoutKind::of(tree).map(|kind| (kind.label(), kind.style(theme)));
    let marker = strongest.map(|strongest| {
        let count = row.sessions.len();
        let text = if count > 1 {
            format!("{}{count}", strongest.marker())
        } else {
            strongest.marker().to_string()
        };
        (
            text,
            strongest.style(vocabulary::SessionStatusStyles::from_theme(theme)),
        )
    });
    item = match (row.removing, kind, marker) {
        (true, _, _) => item.badge(vocabulary::picker_circle_spinner(muted).label("Removing…")),
        (false, Some((kind, kind_style)), Some((marker, marker_style))) => {
            item.badge_parts([(kind.to_string(), kind_style), (marker, marker_style)])
        }
        (false, None, Some((marker, style))) => item.badge_text(marker, style),
        (false, Some((kind, style)), None) => item.badge_text(kind, style),
        (false, None, None) => item,
    };
    // Under the pointer or cursor the rail says what a click opens, beside the ✕ when there is one.
    // Holding Ctrl turns it into what Ctrl+click opens instead, in the accent.
    if !row.removing {
        let alternate = state.sidebar.ctrl_held;
        let mut open = state.config.worktrees.sidebar_open;
        if alternate {
            open = open.other();
        }
        let verb = match (open, strongest) {
            (crate::config::WorktreeOpen::Pane, _) => "new pane",
            (crate::config::WorktreeOpen::Session, None) => "new session",
            (crate::config::WorktreeOpen::Session, Some(_)) if row.sessions.len() > 1 => "choose",
            (crate::config::WorktreeOpen::Session, Some(strongest)) => {
                strongest.verb().unwrap_or("open")
            }
        };
        let style = if alternate {
            super::super::fg_only(&theme.accent)
        } else {
            muted
        };
        item = item.hover_badge_text(verb, style);
    }

    let detail_dim = 0.4;
    #[cfg(feature = "ui-snapshot")]
    let detail_dim = state
        .sidebar
        .worktrees
        .detail_dim_preview
        .unwrap_or(detail_dim);
    if let Some((text, status)) =
        vocabulary::pull_request_text(tree, &state.sidebar.worktrees.statuses)
    {
        let style = status.map_or(muted, |status| vocabulary::work_status_style(theme, status));
        item = item.detail(text, style.transform_fg(ColorTransform::dim(detail_dim)));
    }
    if row.force {
        item = item.armed_prompt("Dirty · again to force");
    } else if tree.lock.is_some() {
        // Only a stale lock leaves the row closable. The prompt names the removal, the destructive
        // part; the lock is lifted on the way.
        item = item.armed_prompt("Stale lock · again to remove");
    }
    // A checkout being removed cannot be opened: a session started in it would outlive the directory.
    let target = if row.removing {
        RowTarget::Inert
    } else {
        RowTarget::Worktree(tree.path.clone())
    };
    let sidebar_row = SidebarRow::item(item, target);
    if row.closable {
        sidebar_row.closable(crate::state::SidebarClose::Worktree {
            path: tree.path.clone(),
            force: row.force,
        })
    } else {
        sidebar_row
    }
}
