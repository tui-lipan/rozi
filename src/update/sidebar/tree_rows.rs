//! Expanding and collapsing sidebar rows. Tree owns navigation and disclosure interaction; rozi
//! stores expansion under data identities so it survives updates, and notifies producers of lazy
//! requests.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::TreeExpansionKey;

/// A framework Tree expansion event, applied to the current semantic row identity.
pub(crate) fn row_expanded(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    index: usize,
    expanded: bool,
) -> Update {
    let key = ctx
        .state
        .active_sidebar_tab(panel)
        .and_then(|tab| ctx.state.sidebar_rows(tab).into_iter().nth(index))
        .and_then(|row| row.expansion);
    match key {
        Some(key) => set_expanded(ctx, key, expanded),
        None => Update::none(),
    }
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
    let current = ctx.state.active_sidebar_tab(panel).and_then(|tab| {
        ctx.state
            .sidebar_rows(tab)
            .into_iter()
            .find(|row| row.expansion.as_ref() == Some(&key))
            .and_then(|row| row.expanded())
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
        .sidebar_rows(tab)
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
