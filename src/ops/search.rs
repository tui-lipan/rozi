use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::ops::focus::{focus_pane, request_search_focus};
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

/// Open scrollback search from copy mode: keep `Mode::Copy`, scope to the focused pane, and
/// return to copy mode on confirm/cancel with the cursor parked on the match.
pub(crate) fn open_search_from_copy_mode(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx
        .state
        .copy_mode
        .as_ref()
        .map(|copy| copy.target)
        .or(ctx.state.focused_pane)
    else {
        return Update::full();
    };
    ctx.state.search = Some(ScrollbackSearchState::from_copy_mode(target));
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    // Keep Mode::Copy so confirm/cancel can restore the copy-mode cursor path.
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
                        start_col: matched.start_col,
                        end_col: matched.end_col,
                        text: matched.text,
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

pub(crate) fn select_search_match(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(search) = ctx.state.search.as_mut() else {
        return Update::none();
    };
    if index >= search.matches.len() {
        request_search_focus(ctx);
        return Update::full();
    }
    search.current = index;
    search.status = format!(
        "{} / {} matches ({})",
        search.current + 1,
        search.matches.len(),
        search.scope.label()
    );
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
    let from_copy_mode = ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.from_copy_mode);

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

    if from_copy_mode
        && let Some(copy) = ctx.state.copy_mode.as_mut()
        && copy.target == matched.pane
    {
        copy.offset = matched.offset;
        copy.cursor_row = matched.line;
        copy.cursor_col = matched.start_col;
    }
}

/// Finish a copy-mode search: park matches on [`CopyModeState`] for `n`/`N` and clear the overlay.
pub(crate) fn finish_copy_mode_search(ctx: &mut Context<HyprmuxApp>, apply_current: bool) {
    let Some(search) = ctx.state.search.take() else {
        return;
    };
    if !search.from_copy_mode {
        return;
    }
    let matches: Vec<crate::state::CopySearchMatch> = search
        .matches
        .into_iter()
        .map(|matched| crate::state::CopySearchMatch {
            offset: matched.offset,
            line: matched.line,
            start_col: matched.start_col,
            end_col: matched.end_col,
        })
        .collect();
    let current = search.current.min(matches.len().saturating_sub(1));
    let apply = apply_current
        .then(|| matches.get(current).cloned())
        .flatten();
    if let Some(copy) = ctx.state.copy_mode.as_mut() {
        if let Some(matched) = apply.as_ref() {
            copy.offset = matched.offset;
            copy.cursor_row = matched.line;
            copy.cursor_col = matched.start_col;
        }
        copy.search_matches = matches;
        copy.search_current = current;
    }
    if let Some(matched) = apply
        && let Some(target) = ctx.state.copy_mode.as_ref().map(|copy| copy.target)
        && let Some(pane) = find_pane_mut(&mut ctx.state, target)
    {
        pane.terminal.set_scrollback(matched.offset);
    }
    ctx.state.mode = crate::state::Mode::Copy;
    ctx.state.commands_dirty = true;
}
