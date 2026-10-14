//! Expanding and collapsing rows in the hierarchical row tabs: Projects, JSON command tabs, and
//! extension stream tabs. The file tree is a widget with its own expansion; these are composed rows
//! whose expansion rozi owns, keyed by row identity so it survives the list being rebuilt.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::{TreeExpansionKey, TreeItem};

/// What a key asks of the row under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeKey {
    /// Right or `l`: show children, or step into the first one when they already show.
    Expand,
    /// Left or `h`: hide children, or step out to the parent.
    Collapse,
    /// Space.
    Toggle,
}

/// The visible lines of the active tab in `panel`, when it is hierarchical.
fn panel_items(ctx: &Context<AppRoot>, panel: usize) -> Option<Vec<TreeItem>> {
    let tab = ctx.state.active_sidebar_tab(panel)?;
    ctx.state.tree_items(tab)
}

/// The disclosure toggle on a row was clicked.
pub(crate) fn row_toggle(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    let Some(line) = panel_items(ctx, panel)
        .and_then(|items| items.into_iter().nth(index))
        .and_then(|item| match item {
            TreeItem::Row(line) => Some(line),
            _ => None,
        })
    else {
        return Update::none();
    };
    if !line.expandable {
        return Update::none();
    }
    set_expanded(ctx, line.key, !line.expanded)
}

/// Show or hide a row's children. Expanding a row whose children a producer lists only on request
/// asks it for them.
pub(crate) fn set_expanded(
    ctx: &mut Context<AppRoot>,
    key: TreeExpansionKey,
    expanded: bool,
) -> Update {
    ctx.state.sidebar.pending_row_close = None;
    let previous = ctx
        .state
        .sidebar
        .tree_rows_expanded
        .insert(key.clone(), expanded);
    if previous == Some(expanded) {
        return Update::none();
    }
    if expanded {
        crate::ops::sidebar_streams::expanded(&mut ctx.state, &key);
    }
    Update::full()
}

/// Activating a row that only holds children toggles them.
pub(crate) fn toggle_key(ctx: &mut Context<AppRoot>, key: TreeExpansionKey) -> Update {
    let panel = ctx.state.sidebar.active_panel_index();
    let current = panel_items(ctx, panel).and_then(|items| {
        items.into_iter().find_map(|item| {
            item.line()
                .filter(|line| line.key == key)
                .map(|line| line.expanded)
        })
    });
    match current {
        Some(expanded) => set_expanded(ctx, key, !expanded),
        None => Update::none(),
    }
}

/// A navigation key on a hierarchical tab. `None` when the active tab is not one, so the key
/// keeps whatever meaning it has elsewhere.
pub(crate) fn tree_key(ctx: &mut Context<AppRoot>, key: TreeKey) -> Option<Update> {
    let panel = ctx.state.sidebar.active_panel_index();
    let tab = ctx.state.active_sidebar_tab(panel)?.clone();
    let items = ctx.state.tree_items(&tab)?;
    let projections = ctx.state.sidebar_item_projections(&tab);
    let Some(index) = crate::state::SidebarItemProjection::resolve_cursor(
        ctx.state.sidebar.panels[panel].cursor,
        &projections,
    ) else {
        return Some(Update::none());
    };
    let Some(line) = items.get(index).and_then(TreeItem::line) else {
        return Some(Update::none());
    };
    let update = match key {
        TreeKey::Toggle if line.expandable => set_expanded(ctx, line.key.clone(), !line.expanded),
        TreeKey::Expand if line.expandable && !line.expanded => {
            set_expanded(ctx, line.key.clone(), true)
        }
        TreeKey::Expand if line.expanded => {
            let depth = line.branch.depth();
            match items
                .get(index + 1)
                .and_then(TreeItem::line)
                .filter(|child| child.branch.depth() > depth)
            {
                Some(_) if projections[index + 1].selectable() => {
                    move_cursor_to(ctx, panel, index + 1)
                }
                _ => Update::none(),
            }
        }
        TreeKey::Collapse if line.expandable && line.expanded => {
            set_expanded(ctx, line.key.clone(), false)
        }
        TreeKey::Collapse => match crate::state::parent_index(&items, index) {
            Some(parent) if projections[parent].selectable() => move_cursor_to(ctx, panel, parent),
            _ => Update::none(),
        },
        _ => Update::none(),
    };
    Some(update)
}

fn move_cursor_to(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    ctx.state.sidebar.pending_row_close = None;
    ctx.state.sidebar.panels[panel].cursor = index;
    ctx.state.sidebar.panels[panel].suppress_row_hover = true;
    Update::full()
}
