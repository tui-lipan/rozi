use tui_lipan::prelude::*;

use crate::state::{ScrollbackMatch, ScrollbackSearchState};
use crate::{HyprmuxApp, find_pane_mut, request_search_focus};

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

pub(crate) fn recompute_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some((target, query)) = ctx
        .state
        .search
        .as_ref()
        .map(|search| (search.target, search.input.text().to_string()))
    else {
        return Update::none();
    };

    let query = query.trim().to_string();
    let matches: Vec<ScrollbackMatch> = if query.is_empty() {
        Vec::new()
    } else {
        find_pane_mut(&mut ctx.state, target)
            .map(|pane| {
                pane.terminal
                    .search_scrollback(&query)
                    .into_iter()
                    .map(|matched| ScrollbackMatch {
                        offset: matched.offset,
                        line: matched.line,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    if let Some(search) = ctx.state.search.as_mut() {
        search.matches = matches;
        search.current = 0;
        search.status = if query.is_empty() {
            "Type to search scrollback".to_string()
        } else if search.matches.is_empty() {
            format!("No matches for `{query}`")
        } else {
            format!("1 / {} matches", search.matches.len())
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
    search.status = format!("{} / {len} matches", search.current + 1);
    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

pub(crate) fn jump_to_search_match(ctx: &mut Context<HyprmuxApp>) {
    let Some((target, matched)) = ctx.state.search.as_ref().and_then(|search| {
        search
            .matches
            .get(search.current)
            .cloned()
            .map(|matched| (search.target, matched))
    }) else {
        return;
    };
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(matched.offset);
    }
}
