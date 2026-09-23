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
    let (state, state_style) = match row.sessions.as_slice() {
        _ if row.removing => ("removing…".to_string(), muted),
        [] if !tree.linked => ("primary".to_string(), muted),
        [] if tree.locked => ("locked".to_string(), muted),
        [] if tree.prunable => (
            "prunable".to_string(),
            Style::new().fg(theme.status.warning),
        ),
        [] => (String::new(), muted),
        [only] => (only.clone(), super::super::fg_only(&theme.accent)),
        [first, rest @ ..] => (
            format!("{first} +{}", rest.len()),
            super::super::fg_only(&theme.accent),
        ),
    };
    let path = crate::view::overlays::short_checkout_path(&tree.path, row.primary.as_deref());
    let budget = usize::from(ctx.state.sidebar_requested_width()).saturating_sub(DETAIL_INSET);
    let mut item = Row::new(branch)
        .active(row.current)
        .title_style(title_style);
    // The primary checkout's path is the repository itself, which the header already names.
    if tree.linked {
        item = item.detail(super::row::truncate_start(&path, budget), muted);
    }
    if !state.is_empty() {
        item = item.badge_text(state, state_style);
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
