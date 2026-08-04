use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::ops::focus::{focus_pane, request_search_focus};
use crate::pane_lifecycle::{find_pane, find_pane_mut};
use crate::state::{
    MAX_MATCHES, PaneId, ScrollbackMatch, ScrollbackSearchState, SearchScope, State,
};

pub(crate) fn open_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.current().focused_pane else {
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
        .or(ctx.state.current().focused_pane)
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
    let Some(search) = ctx.state.search.as_mut() else {
        return Update::none();
    };
    if search.from_copy_mode {
        return Update::none();
    }
    search.scope = search.scope.cycled();
    recompute_search(ctx)
}

/// Pane ids to scan for the current scope, in a stable order.
fn panes_in_scope(state: &State, target: PaneId, scope: SearchScope) -> Vec<PaneId> {
    match scope {
        SearchScope::FocusedPane => vec![target],
        SearchScope::Workspace => {
            let workspace = &state.current().workspaces[state.current().active_workspace];
            workspace
                .panes
                .iter()
                .filter(|pane| !pane.closing)
                .map(|pane| pane.id)
                .collect()
        }
        SearchScope::All => state
            .current()
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .filter(|pane| !pane.closing)
            .map(|pane| pane.id)
            .collect(),
    }
}

fn matches_in_scope(
    state: &State,
    target: PaneId,
    scope: SearchScope,
    query: &str,
) -> (Vec<ScrollbackMatch>, bool) {
    let mut matches = Vec::new();
    let mut truncated = false;
    for pane_id in panes_in_scope(state, target, scope) {
        let Some(pane) = find_pane(state, pane_id) else {
            continue;
        };
        let result = pane
            .terminal
            .search_scrollback_bounded(query, MAX_MATCHES - matches.len());
        matches.extend(result.matches.into_iter().map(|matched| ScrollbackMatch {
            offset: matched.offset,
            line: matched.line,
            start_col: matched.start_col,
            end_col: matched.end_col,
            text: matched.text,
            pane: pane_id,
        }));
        if result.truncated {
            truncated = true;
            break;
        }
    }
    (matches, truncated)
}

pub(crate) fn recompute_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    if let Some(search) = ctx.state.search.as_mut()
        && search.from_copy_mode
    {
        search.scope = SearchScope::FocusedPane;
    }
    let Some((target, scope, query)) = ctx.state.search.as_ref().map(|search| {
        (
            search.target,
            search.scope,
            search.input.text().trim().to_string(),
        )
    }) else {
        return Update::none();
    };

    let (matches, truncated) = if query.is_empty() {
        (Vec::new(), false)
    } else {
        matches_in_scope(&ctx.state, target, scope, &query)
    };

    if let Some(search) = ctx.state.search.as_mut() {
        search.replace_results(matches, truncated);
        search.current = 0;
        let scope_label = search.scope.label();
        if query.is_empty() {
            search.status = format!("Type to search scrollback ({scope_label})");
        } else if search.matches.is_empty() {
            search.status = format!("No matches for `{query}` ({scope_label})");
        } else {
            search.refresh_match_status();
        }
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
    search.refresh_match_status();
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
    search.refresh_match_status();
    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

fn pane_workspace(state: &State, id: PaneId) -> Option<usize> {
    state
        .current()
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
    if from_copy_mode
        && ctx
            .state
            .copy_mode
            .as_ref()
            .is_none_or(|copy| copy.target != matched.pane)
    {
        return;
    }

    // Bring the matching pane's workspace forward and focus it before scrolling.
    if let Some(workspace_index) = pane_workspace(&ctx.state, matched.pane)
        && workspace_index != ctx.state.current().active_workspace
    {
        ctx.state.current_mut().active_workspace = workspace_index;
        ctx.state.animation = GeometryAnimation::None;
    }
    focus_pane(&mut ctx.state, matched.pane);

    if let Some(pane) = find_pane_mut(&mut ctx.state, matched.pane) {
        pane.terminal.set_scrollback(matched.offset);
    }
}

/// Finish a copy-mode search: park matches on [`CopyModeState`] for `n`/`N` and clear the overlay.
///
/// Only clears the search overlay when it was opened from copy mode; otherwise leaves the
/// normal scrollback-search overlay alone for its own confirm/cancel path.
pub(crate) fn finish_copy_mode_search(ctx: &mut Context<HyprmuxApp>, apply_current: bool) {
    if !ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.from_copy_mode)
    {
        return;
    }
    let Some(search) = ctx.state.search.take() else {
        return;
    };
    let target = ctx.state.copy_mode.as_ref().map(|copy| copy.target);
    let truncated = search.truncated;
    let matches: Vec<crate::state::CopySearchMatch> = search
        .matches
        .into_iter()
        .filter(|matched| Some(matched.pane) == target)
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
    let restore = if let Some(copy) = ctx.state.copy_mode.as_mut() {
        if let Some(matched) = apply.as_ref() {
            copy.navigation
                .goto(matched.line, matched.start_col, matched.offset);
        }
        copy.search_matches = matches;
        copy.search_current = current;
        copy.search_truncated = truncated;
        Some((copy.target, copy.navigation.scrollback_offset()))
    } else {
        None
    };
    if let Some((target, offset)) = restore
        && let Some(pane) = find_pane_mut(&mut ctx.state, target)
    {
        pane.terminal.set_scrollback(offset);
    }
    ctx.state.mode = crate::state::Mode::Copy;
    ctx.state.commands_dirty = true;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::state::{CopyModeState, Pane};

    fn output_lines(count: usize) -> Vec<u8> {
        (0..count)
            .map(|index| format!("needle-{index}\r\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn state_with_two_panes(second_count: usize) -> State {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        find_pane_mut(&mut state, 1)
            .expect("initial pane")
            .terminal
            .process_server_output(&output_lines(1_500));
        let mut second = Pane::new(2, 5_000, FloatRect::default());
        second.opening = false;
        second
            .terminal
            .process_server_output(&output_lines(second_count));
        state.current_mut().workspaces[0].panes.push(second);
        state
    }

    #[test]
    fn global_match_cap_distinguishes_exactly_2000_from_2001() {
        let exact = state_with_two_panes(500);
        let (matches, truncated) = matches_in_scope(&exact, 1, SearchScope::All, "needle");
        assert_eq!(matches.len(), MAX_MATCHES);
        assert!(!truncated);
        assert_eq!(
            matches.iter().filter(|matched| matched.pane == 1).count(),
            1_500
        );
        assert_eq!(
            matches.iter().filter(|matched| matched.pane == 2).count(),
            500
        );

        let over = state_with_two_panes(501);
        let (matches, truncated) = matches_in_scope(&over, 1, SearchScope::All, "needle");
        assert_eq!(matches.len(), MAX_MATCHES);
        assert!(truncated);
        assert_eq!(
            matches.iter().filter(|matched| matched.pane == 2).count(),
            500
        );
    }

    #[test]
    fn truncated_status_and_navigation_use_the_bounded_set() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(&output_lines(MAX_MATCHES + 1));
                let mut search = ScrollbackSearchState::new(target);
                search.scope = SearchScope::All;
                backend.state_mut().search = Some(search);

                backend
                    .dispatch(crate::Msg::SearchQueryChanged("needle".to_string()))
                    .expect("search query");
                let search = backend.state().search.as_ref().expect("search state");
                assert_eq!(search.matches.len(), MAX_MATCHES);
                assert!(search.truncated);
                assert_eq!(search.current, 0);
                assert_eq!(search.status, "1 / 2000+ matches (all)");
                backend.render();

                backend
                    .dispatch(crate::Msg::SearchNext(false))
                    .expect("next match");
                let search = backend.state().search.as_ref().expect("search state");
                assert_eq!(search.current, 1);
                assert_eq!(search.status, "2 / 2000+ matches (all)");

                backend
                    .dispatch(crate::Msg::SearchNext(true))
                    .expect("previous match");
                assert_eq!(backend.state().search.as_ref().expect("search").current, 0);
                backend
                    .dispatch(crate::Msg::SearchNext(true))
                    .expect("wrapped previous match");
                let search = backend.state().search.as_ref().expect("search state");
                assert_eq!(search.current, MAX_MATCHES - 1);
                assert_eq!(search.status, "2000 / 2000+ matches (all)");
            })
            .expect("spawn search test")
            .join()
            .expect("search test completes");
    }

    #[test]
    fn copy_mode_search_stays_on_its_target_pane_and_hands_off_that_match() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let target_output = format!(
                    "target needle\r\n{}",
                    (0..40)
                        .map(|index| format!("filler-{index}\r\n"))
                        .collect::<String>()
                );
                find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(target_output.as_bytes());
                let mut other = Pane::new(2, 100, FloatRect::default());
                other.opening = false;
                other
                    .terminal
                    .process_server_output(b"other needle needle\r\n");
                backend.state_mut().current_mut().workspaces[0]
                    .panes
                    .push(other);
                backend.state_mut().copy_mode = Some(CopyModeState {
                    target,
                    navigation: TerminalCopyMode::new(0, 0, 0),
                    search_matches: Vec::new(),
                    search_current: 0,
                    search_truncated: false,
                });
                let mut search = ScrollbackSearchState::from_copy_mode(target);
                search.scope = SearchScope::All;
                backend.state_mut().search = Some(search);

                backend
                    .dispatch(crate::Msg::SearchQueryChanged("needle".to_string()))
                    .expect("copy search query");
                let search = backend.state().search.as_ref().expect("search state");
                assert_eq!(search.scope, SearchScope::FocusedPane);
                assert!(!search.matches.is_empty());
                assert!(search.matches.iter().all(|matched| matched.pane == target));
                let selected = search.matches[0].clone();
                assert!(selected.offset > 0);
                let copy = backend.state().copy_mode.as_ref().expect("copy mode");
                assert_eq!(copy.navigation.cursor(), (0, 0));
                assert_eq!(copy.navigation.scrollback_offset(), 0);
                backend
                    .state_mut()
                    .search
                    .as_mut()
                    .expect("search")
                    .truncated = true;

                backend
                    .dispatch(crate::Msg::SearchActivate(0))
                    .expect("activate copy match");
                let copy = backend.state().copy_mode.as_ref().expect("copy mode");
                assert_eq!(
                    copy.navigation.cursor(),
                    (selected.line, selected.start_col)
                );
                assert_eq!(copy.navigation.scrollback_offset(), selected.offset);
                assert_eq!(copy.search_matches.len(), 1);
                assert!(copy.search_truncated);
            })
            .expect("spawn copy search test")
            .join()
            .expect("copy search test completes");
    }

    #[test]
    fn copy_mode_tab_and_backtab_do_not_recompute_or_advertise_scope_changes() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(b"needle-one\r\nneedle-two\r\n");
                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::EnterCopyMode))
                    .expect("enter copy mode");
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('/'),
                        mods: KeyMods::NONE,
                    })
                    .expect("open copy search");
                backend
                    .dispatch(crate::Msg::SearchQueryChanged("needle".to_string()))
                    .expect("copy search query");
                backend.render();
                let rendered = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(!rendered.contains("pane tab"), "{rendered}");

                find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(b"needle-added-after-search\r\n");
                let (scope, current, status, matches, items, offset) = {
                    let search = backend.state().search.as_ref().expect("search");
                    (
                        search.scope,
                        search.current,
                        search.status.clone(),
                        search.matches.clone(),
                        Arc::clone(&search.items),
                        find_pane(backend.state(), target)
                            .expect("target pane")
                            .terminal
                            .scrollback_offset(),
                    )
                };

                for code in [KeyCode::Tab, KeyCode::BackTab] {
                    backend
                        .send_key(KeyEvent {
                            code,
                            mods: KeyMods::NONE,
                        })
                        .expect("scope key");
                    let search = backend.state().search.as_ref().expect("search");
                    assert_eq!(search.scope, scope);
                    assert_eq!(search.current, current);
                    assert_eq!(search.status, status);
                    assert_eq!(search.matches, matches);
                    assert!(Arc::ptr_eq(&search.items, &items));
                    assert_eq!(
                        find_pane(backend.state(), target)
                            .expect("target pane")
                            .terminal
                            .scrollback_offset(),
                        offset
                    );
                }
            })
            .expect("spawn copy scope test")
            .join()
            .expect("copy scope test completes");
    }
}
