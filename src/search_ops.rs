use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::focus_ops::{focus_pane, request_search_focus};
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{PaneId, ScrollbackMatch, ScrollbackSearchState, SearchScope, State};

pub(crate) fn open_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.focused_pane else {
        return Update::full();
    };
    ctx.state.search = Some(ScrollbackSearchState::new(target));
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.mode = crate::state::Mode::Normal;
    request_search_focus(ctx);
    Update::full()
}

/// Cycle the search scope (focused pane → workspace → all) and re-run the search.
pub(crate) fn cycle_search_scope(ctx: &mut Context<HyprmuxApp>) -> Update {
    if let Some(search) = ctx.state.search.as_mut() {
        search.scope = search.scope.cycled();
    }
    recompute_search(ctx)
}

/// Pane ids to scan for the current scope, in a stable order.
fn panes_in_scope(state: &State, target: PaneId, scope: SearchScope) -> Vec<PaneId> {
    match scope {
        SearchScope::FocusedPane => vec![target],
        SearchScope::Workspace => state.workspaces[state.active_workspace]
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .map(|pane| pane.id)
            .collect(),
        SearchScope::All => state
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .filter(|pane| !pane.closing)
            .map(|pane| pane.id)
            .collect(),
    }
}

pub(crate) fn recompute_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some((target, scope, query)) = ctx.state.search.as_ref().map(|search| {
        (
            search.target,
            search.scope,
            search.input.text().trim().to_string(),
        )
    }) else {
        return Update::none();
    };

    let mut matches: Vec<ScrollbackMatch> = Vec::new();
    if !query.is_empty() {
        for pane_id in panes_in_scope(&ctx.state, target, scope) {
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
                matches.extend(pane.terminal.search_scrollback(&query).into_iter().map(
                    |matched| ScrollbackMatch {
                        offset: matched.offset,
                        line: matched.line,
                        pane: pane_id,
                    },
                ));
            }
        }
    }

    if let Some(search) = ctx.state.search.as_mut() {
        search.matches = matches;
        search.current = 0;
        let scope_label = search.scope.label();
        search.status = if query.is_empty() {
            format!("Type to search scrollback ({scope_label})")
        } else if search.matches.is_empty() {
            format!("No matches for `{query}` ({scope_label})")
        } else {
            format!("1 / {} matches ({scope_label})", search.matches.len())
        };
    }

    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

pub(crate) fn search_next(ctx: &mut Context<HyprmuxApp>, backward: bool) -> Update {
    let Some(search) = ctx.state.search.as_mut() else {
        return Update::none();
    };
    if search.matches.is_empty() {
        request_search_focus(ctx);
        return Update::full();
    }
    let len = search.matches.len();
    search.current = if backward {
        search.current.checked_sub(1).unwrap_or(len - 1)
    } else {
        (search.current + 1) % len
    };
    search.status = format!(
        "{} / {len} matches ({})",
        search.current + 1,
        search.scope.label()
    );
    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

fn pane_workspace(state: &State, id: PaneId) -> Option<usize> {
    state
        .workspaces
        .iter()
        .position(|workspace| workspace.panes.iter().any(|pane| pane.id == id))
}

pub(crate) fn jump_to_search_match(ctx: &mut Context<HyprmuxApp>) {
    let Some(matched) = ctx
        .state
        .search
        .as_ref()
        .and_then(|search| search.matches.get(search.current).cloned())
    else {
        return;
    };

    // Bring the matching pane's workspace forward and focus it before scrolling.
    if let Some(workspace_index) = pane_workspace(&ctx.state, matched.pane)
        && workspace_index != ctx.state.active_workspace
    {
        ctx.state.active_workspace = workspace_index;
        ctx.state.animation = GeometryAnimation::None;
    }
    focus_pane(&mut ctx.state, matched.pane);

    if let Some(pane) = find_pane_mut(&mut ctx.state, matched.pane) {
        pane.terminal.set_scrollback(matched.offset);
    }
}
