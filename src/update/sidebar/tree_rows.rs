//! Expanding and collapsing rows in the hierarchical row tabs: Projects, JSON command tabs, and
//! extension stream tabs. Tree owns navigation and disclosure interaction; rozi stores expansion
//! under data identities so it survives producer updates and notifies producers of lazy requests.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::{TreeExpansionKey, TreeItem};

/// The visible lines of the active tab in `panel`, when it is hierarchical.
fn panel_items(ctx: &Context<AppRoot>, panel: usize) -> Option<Vec<TreeItem>> {
    let tab = ctx.state.active_sidebar_tab(panel)?;
    ctx.state.tree_items(tab)
}

/// A framework Tree expansion event, applied to the current semantic row identity.
pub(crate) fn row_expanded(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    index: usize,
    expanded: bool,
) -> Update {
    // A harness in the Activity tab folds the sessions one pane publishes.
    if matches!(
        ctx.state.active_sidebar_tab(panel),
        Some(crate::config::SidebarTab::Activity)
    ) {
        return match crate::view::sidebar::agents::activity_lines(&ctx.state).get(index) {
            Some(crate::view::sidebar::agents::ActivityLine::Harness { harness, .. }) => {
                let key = crate::view::sidebar::agents::harness_key(harness.pane_id);
                set_expanded(ctx, key, expanded)
            }
            _ => Update::none(),
        };
    }
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
    set_expanded(ctx, line.key, expanded)
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
pub(crate) fn toggle_key(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    key: TreeExpansionKey,
) -> Update {
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

/// Mirror the Tree selection so extension actions and confirmation target the visible cursor.
pub(crate) fn row_selected(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    let Some(tab) = ctx.state.active_sidebar_tab(panel) else {
        return Update::none();
    };
    if !ctx
        .state
        .sidebar_item_projections(tab)
        .get(index)
        .is_some_and(|row| row.selectable())
    {
        return Update::none();
    }
    ctx.state.sidebar.pending_row_close = None;
    ctx.state.sidebar.panels[panel].cursor = index;
    // Tree emits selection for pointer clicks as well as keyboard navigation. Mirroring its
    // selection must not disable the hover effect underneath a stationary pointer.
    Update::full()
}
