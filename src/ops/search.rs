use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::ops::focus::{focus_pane, request_search_focus, switch_workspace};
use crate::pane_lifecycle::{find_pane, find_pane_mut};
use crate::state::{
    MAX_MATCHES, PaneId, ScrollbackMatch, ScrollbackSearchScan, ScrollbackSearchState, SearchScope,
    State,
};

pub const SEARCH_LINES_PER_CHUNK: usize = 512;

fn invalidate_search_scan(state: &mut State) -> u64 {
    state.search_scan_epoch = state.search_scan_epoch.wrapping_add(1);
    state.search_scan_epoch
}

pub(crate) fn open_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.current().focused_pane else {
        return Update::full();
    };
    invalidate_search_scan(&mut ctx.state);
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
    invalidate_search_scan(&mut ctx.state);
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

/// Pane ids to scan for the current scope: the original target first, then stable scope order.
fn panes_in_scope(state: &State, target: PaneId, scope: SearchScope) -> Vec<PaneId> {
    let mut panes = Vec::new();
    let mut push = |pane_id| {
        if !panes.contains(&pane_id) && find_pane(state, pane_id).is_some_and(|pane| !pane.closing)
        {
            panes.push(pane_id);
        }
    };
    push(target);
    match scope {
        SearchScope::FocusedPane => {}
        SearchScope::Workspace => {
            let workspace = state
                .current()
                .workspaces
                .iter()
                .find(|workspace| workspace.panes.iter().any(|pane| pane.id == target))
                .unwrap_or_else(|| &state.current().workspaces[state.current().active_workspace]);
            for pane in &workspace.panes {
                push(pane.id);
            }
        }
        SearchScope::All => {
            for workspace in &state.current().workspaces {
                for pane in &workspace.panes {
                    push(pane.id);
                }
            }
        }
    }
    panes
}

fn purge_missing_search_matches(state: &mut State) -> bool {
    let mut pane_ids = Vec::new();
    if let Some(search) = state.search.as_ref() {
        for matched in &search.matches {
            if !pane_ids.contains(&matched.pane) {
                pane_ids.push(matched.pane);
            }
        }
    }
    let missing = pane_ids
        .into_iter()
        .filter(|pane_id| find_pane(state, *pane_id).is_none())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return false;
    }
    state
        .search
        .as_mut()
        .is_some_and(|search| search.retain_matches(|matched| !missing.contains(&matched.pane)))
}

fn search_scan_command(epoch: u64) -> Command {
    Command::after(
        std::time::Duration::ZERO,
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::SearchScanChunk { epoch });
        },
    )
}

fn schedule_search_scan(state: &mut State, epoch: u64) -> Option<Command> {
    if state.search_scan_scheduled_epoch.is_some() {
        return None;
    }
    state.search_scan_scheduled_epoch = Some(epoch);
    Some(search_scan_command(epoch))
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
            Arc::<str>::from(search.input.text().trim()),
        )
    }) else {
        return Update::none();
    };

    let epoch = invalidate_search_scan(&mut ctx.state);
    let scan_targets = (!query.is_empty()).then(|| {
        let panes = panes_in_scope(&ctx.state, target, scope);
        let pane_ends = panes
            .iter()
            .map(|pane_id| {
                find_pane(&ctx.state, *pane_id).map_or(0, |pane| pane.terminal.search_line_count())
            })
            .collect::<Vec<_>>();
        (panes, pane_ends)
    });
    if let Some(search) = ctx.state.search.as_mut() {
        search.replace_results(Vec::new(), false);
        search.current = 0;
        let scope_label = search.scope.label();
        if query.is_empty() {
            search.scan = None;
            search.status = format!("Type to search scrollback ({scope_label})");
        } else {
            let (panes, pane_ends) = scan_targets.expect("non-empty query captures panes");
            search.scan = Some(ScrollbackSearchScan {
                epoch,
                query,
                panes: panes.into(),
                pane_ends: pane_ends.into(),
                pane_index: 0,
                line_cursor: 0,
                first_jump_done: false,
            });
            search.refresh_match_status();
        }
    }

    request_search_focus(ctx);
    if ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.scan.is_some())
    {
        let command = schedule_search_scan(&mut ctx.state, epoch);
        Update::with_command(command)
    } else {
        Update::full()
    }
}

/// Invalidate coordinates captured from a pane before applying more live output.
///
/// A scan always has at most one queued chunk. If that chunk belongs to an older epoch it will
/// re-arm the newest scan when it arrives, rather than every output frame adding another stale
/// message to the queue.
pub(crate) fn restart_search_after_pane_output(
    ctx: &mut Context<HyprmuxApp>,
    pane_id: PaneId,
) -> Option<Update> {
    if let Some(copy) = ctx.state.copy_mode.as_mut()
        && copy.target == pane_id
        && !copy.search_matches.is_empty()
    {
        copy.search_matches.clear();
        copy.search_current = 0;
        copy.search_truncated = false;
    }

    let affected = ctx.state.search.as_ref().is_some_and(|search| {
        !search.input.text().trim().is_empty()
            && panes_in_scope(&ctx.state, search.target, search.scope).contains(&pane_id)
    });
    affected.then(|| recompute_search(ctx))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScanAdvance {
    Stale,
    Running { first_match: bool, render: bool },
    Complete { first_match: bool },
}

/// Advance the active search by at most `line_budget` retained lines.
///
/// This deterministic seam is used by tests; production always passes
/// [`SEARCH_LINES_PER_CHUNK`].
pub fn advance_search_scan(
    state: &mut State,
    epoch: u64,
    mut line_budget: usize,
) -> SearchScanAdvance {
    let Some(search) = state.search.as_ref() else {
        return SearchScanAdvance::Stale;
    };
    if search.scan.as_ref().is_none_or(|scan| scan.epoch != epoch) {
        return SearchScanAdvance::Stale;
    }
    let purged = purge_missing_search_matches(state);
    let mut scan = state
        .search
        .as_mut()
        .and_then(|search| search.scan.take())
        .expect("validated active scan");
    let mut appended = Vec::new();
    let mut truncated = false;

    while line_budget > 0 && scan.pane_index < scan.panes.len() {
        let pane_id = scan.panes[scan.pane_index];
        let pane_end = scan.pane_ends[scan.pane_index];
        if scan.line_cursor >= pane_end {
            scan.pane_index += 1;
            scan.line_cursor = 0;
            continue;
        }
        let range_end = scan.line_cursor.saturating_add(line_budget).min(pane_end);
        let Some(pane) = find_pane(state, pane_id) else {
            scan.pane_index += 1;
            scan.line_cursor = 0;
            continue;
        };
        let result = pane.terminal.search_scrollback_range(
            &scan.query,
            scan.line_cursor,
            range_end,
            MAX_MATCHES.saturating_sub(
                state
                    .search
                    .as_ref()
                    .map_or(0, |search| search.matches.len() + appended.len()),
            ),
        );
        let consumed = range_end - scan.line_cursor;
        scan.line_cursor = range_end;
        line_budget -= consumed;
        appended.extend(result.matches.into_iter().map(|matched| ScrollbackMatch {
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
        if scan.line_cursor >= pane_end {
            scan.pane_index += 1;
            scan.line_cursor = 0;
        }
    }

    let complete = truncated || scan.pane_index >= scan.panes.len();
    let search = state
        .search
        .as_mut()
        .expect("search survived synchronous scan");
    search.append_results(appended);
    if truncated {
        search.truncated = true;
    }
    let first_match = !scan.first_jump_done && !search.matches.is_empty();
    if first_match {
        scan.first_jump_done = true;
    }
    if complete {
        search.scan = None;
    } else {
        search.scan = Some(scan);
    }
    search.refresh_match_status();

    if complete {
        SearchScanAdvance::Complete { first_match }
    } else {
        SearchScanAdvance::Running {
            first_match,
            render: first_match || purged,
        }
    }
}

pub(crate) fn search_scan_chunk(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if ctx.state.search_scan_scheduled_epoch != Some(epoch) {
        return Update::none();
    }
    ctx.state.search_scan_scheduled_epoch = None;
    let advance = advance_search_scan(&mut ctx.state, epoch, SEARCH_LINES_PER_CHUNK);
    if matches!(
        advance,
        SearchScanAdvance::Running {
            first_match: true,
            ..
        } | SearchScanAdvance::Complete { first_match: true }
    ) {
        jump_to_search_match(ctx);
        request_search_focus(ctx);
    }
    let next_epoch = match advance {
        SearchScanAdvance::Running { .. } => Some(epoch),
        SearchScanAdvance::Stale => ctx
            .state
            .search
            .as_ref()
            .and_then(|search| search.scan.as_ref())
            .map(|scan| scan.epoch),
        SearchScanAdvance::Complete { .. } => None,
    };
    let command = next_epoch.and_then(|epoch| schedule_search_scan(&mut ctx.state, epoch));
    match advance {
        SearchScanAdvance::Stale => command.map_or_else(Update::none, Update::command_only),
        SearchScanAdvance::Running { render: true, .. } => Update::with_command(command),
        SearchScanAdvance::Running { render: false, .. } => {
            command.map_or_else(Update::none, Update::command_only)
        }
        SearchScanAdvance::Complete { .. } => Update::full(),
    }
}

#[cfg(test)]
fn scan_advance_update(advance: SearchScanAdvance, epoch: u64) -> Update {
    match advance {
        SearchScanAdvance::Stale => Update::none(),
        SearchScanAdvance::Running { render: true, .. } => {
            Update::with_command(search_scan_command(epoch))
        }
        SearchScanAdvance::Running { render: false, .. } => {
            Update::command_only(search_scan_command(epoch))
        }
        SearchScanAdvance::Complete { .. } => Update::full(),
    }
}

pub(crate) fn select_search_match(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    if purge_missing_search_matches(&mut ctx.state) {
        jump_to_search_match(ctx);
        request_search_focus(ctx);
        return Update::full();
    }
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
    purge_missing_search_matches(&mut ctx.state);
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
    if let Some(workspace_index) = pane_workspace(&ctx.state, matched.pane) {
        let cross_workspace = workspace_index != ctx.state.current().active_workspace;
        if cross_workspace {
            switch_workspace(&mut ctx.state, workspace_index);
        }
        focus_pane(&mut ctx.state, matched.pane);
        if cross_workspace {
            ctx.state.animation = GeometryAnimation::None;
        }
    } else {
        focus_pane(&mut ctx.state, matched.pane);
    }

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
    purge_missing_search_matches(&mut ctx.state);
    let Some(search) = ctx.state.search.take() else {
        return;
    };
    let target = ctx.state.copy_mode.as_ref().map(|copy| copy.target);
    let truncated = search.truncated || search.scan.is_some();
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

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn search test")
            .join()
            .expect("search test completes");
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

    fn begin_scan(state: &mut State, target: PaneId, scope: SearchScope, query: &str) -> u64 {
        state.search_scan_epoch = state.search_scan_epoch.wrapping_add(1);
        let epoch = state.search_scan_epoch;
        let mut search = ScrollbackSearchState::new(target);
        search.scope = scope;
        search.input.set_text(query);
        let panes = panes_in_scope(state, target, scope);
        let pane_ends = panes
            .iter()
            .map(|pane_id| {
                find_pane(state, *pane_id).map_or(0, |pane| pane.terminal.search_line_count())
            })
            .collect::<Vec<_>>();
        search.scan = Some(ScrollbackSearchScan {
            epoch,
            query: Arc::from(query),
            panes: panes.into(),
            pane_ends: pane_ends.into(),
            pane_index: 0,
            line_cursor: 0,
            first_jump_done: false,
        });
        search.refresh_match_status();
        state.search = Some(search);
        epoch
    }

    fn drain_scan(state: &mut State, epoch: u64, line_budget: usize) {
        loop {
            match advance_search_scan(state, epoch, line_budget) {
                SearchScanAdvance::Running { .. } => {}
                SearchScanAdvance::Complete { .. } => break,
                SearchScanAdvance::Stale => panic!("scan unexpectedly became stale"),
            }
        }
    }

    #[test]
    fn global_match_cap_distinguishes_exactly_2000_from_2001() {
        let mut exact = state_with_two_panes(500);
        let epoch = begin_scan(&mut exact, 1, SearchScope::All, "needle");
        drain_scan(&mut exact, epoch, 317);
        let search = exact.search.as_ref().expect("completed search");
        assert_eq!(search.matches.len(), MAX_MATCHES);
        assert!(!search.truncated);
        assert_eq!(
            search
                .matches
                .iter()
                .filter(|matched| matched.pane == 1)
                .count(),
            1_500
        );
        assert_eq!(
            search
                .matches
                .iter()
                .filter(|matched| matched.pane == 2)
                .count(),
            500
        );

        let mut over = state_with_two_panes(501);
        let epoch = begin_scan(&mut over, 1, SearchScope::All, "needle");
        drain_scan(&mut over, epoch, 317);
        let search = over.search.as_ref().expect("completed search");
        assert_eq!(search.matches.len(), MAX_MATCHES);
        assert!(search.truncated);
        assert_eq!(
            search
                .matches
                .iter()
                .filter(|matched| matched.pane == 2)
                .count(),
            500
        );
    }

    #[test]
    fn scans_target_first_and_skips_a_pane_that_disappears_between_slices() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        find_pane_mut(&mut state, 1)
            .expect("initial pane")
            .terminal
            .process_server_output(b"first needle\r\n");
        let mut second = Pane::new(2, 100, FloatRect::default());
        second.opening = false;
        second.terminal.process_server_output(b"focused needle\r\n");
        state.current_mut().workspaces[0].panes.push(second);

        let epoch = begin_scan(&mut state, 2, SearchScope::All, "needle");
        assert_eq!(
            state
                .search
                .as_ref()
                .and_then(|search| search.scan.as_ref())
                .expect("scan")
                .panes
                .as_ref(),
            &[2, 1]
        );
        drain_scan(&mut state, epoch, 1);
        assert_eq!(
            state
                .search
                .as_ref()
                .expect("search")
                .matches
                .iter()
                .map(|matched| matched.pane)
                .collect::<Vec<_>>(),
            [2, 1]
        );

        let epoch = begin_scan(&mut state, 1, SearchScope::All, "needle");
        let first = advance_search_scan(&mut state, epoch, 1);
        assert!(matches!(first, SearchScanAdvance::Running { .. }));
        state.current_mut().workspaces[0]
            .panes
            .retain(|pane| pane.id != 2);
        drain_scan(&mut state, epoch, 1);
        assert!(
            state
                .search
                .as_ref()
                .expect("search")
                .matches
                .iter()
                .all(|matched| matched.pane == 1)
        );
    }

    #[test]
    fn disappearing_contributor_is_purged_and_selected_stale_row_clamps() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        find_pane_mut(&mut state, 1)
            .expect("initial pane")
            .terminal
            .process_server_output(&output_lines(100));
        for (pane_id, lines) in [(2, 1_500), (3, 1_000)] {
            let mut pane = Pane::new(pane_id, 5_000, FloatRect::default());
            pane.opening = false;
            pane.terminal.process_server_output(&output_lines(lines));
            state.current_mut().workspaces[0].panes.push(pane);
        }

        let epoch = begin_scan(&mut state, 1, SearchScope::All, "needle");
        assert!(matches!(
            advance_search_scan(&mut state, epoch, 600),
            SearchScanAdvance::Running { .. }
        ));
        let selected = state
            .search
            .as_ref()
            .expect("search")
            .matches
            .iter()
            .position(|matched| matched.pane == 2)
            .expect("second pane contributed")
            + 10;
        state.search.as_mut().expect("search").current = selected;
        let retained_before = state
            .search
            .as_ref()
            .expect("search")
            .matches
            .iter()
            .filter(|matched| matched.pane != 2)
            .count();

        state.current_mut().workspaces[0]
            .panes
            .retain(|pane| pane.id != 2);
        assert_eq!(
            advance_search_scan(&mut state, epoch, 512),
            SearchScanAdvance::Running {
                first_match: false,
                render: true,
            }
        );
        {
            let search = state.search.as_ref().expect("search");
            assert!(search.matches.iter().all(|matched| matched.pane != 2));
            assert_eq!(
                search.current,
                selected.min(retained_before.saturating_sub(1))
            );
            assert_eq!(search.matches.len(), search.items.len());
        }

        drain_scan(&mut state, epoch, 512);
        let search = state.search.as_ref().expect("completed search");
        assert_eq!(search.matches.len(), 1_100);
        assert!(!search.truncated);
        assert!(search.matches.iter().all(|matched| matched.pane != 2));
        assert_eq!(
            search.current,
            selected.min(retained_before.saturating_sub(1))
        );
        assert!(
            search
                .items
                .iter()
                .enumerate()
                .all(|(index, item)| item.value == index)
        );
    }

    #[test]
    fn restart_clear_close_and_reopen_reject_stale_scan_epochs() {
        on_large_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
            let target = backend
                .state()
                .current()
                .focused_pane
                .expect("focused pane");
            find_pane_mut(backend.state_mut(), target)
                .expect("target")
                .terminal
                .process_server_output(&output_lines(20));

            let first = begin_scan(
                backend.state_mut(),
                target,
                SearchScope::FocusedPane,
                "needle",
            );
            assert!(matches!(
                advance_search_scan(backend.state_mut(), first, 1),
                SearchScanAdvance::Running { .. }
            ));
            backend
                .dispatch(crate::Msg::SearchQueryChanged("needle-1".to_string()))
                .expect("restart query");
            let second = backend.state().search_scan_epoch;
            assert_ne!(first, second);
            assert_eq!(
                advance_search_scan(backend.state_mut(), first, 1),
                SearchScanAdvance::Stale
            );
            backend
                .dispatch(crate::Msg::SearchCycleScope)
                .expect("restart scope");
            let scoped = backend.state().search_scan_epoch;
            assert_ne!(second, scoped);
            assert_eq!(
                backend.state().search.as_ref().expect("search").scope,
                SearchScope::Workspace
            );
            assert_eq!(
                advance_search_scan(backend.state_mut(), second, 1),
                SearchScanAdvance::Stale
            );

            backend
                .dispatch(crate::Msg::SearchQueryChanged(String::new()))
                .expect("clear query");
            assert!(
                backend
                    .state()
                    .search
                    .as_ref()
                    .expect("search")
                    .scan
                    .is_none()
            );
            assert_eq!(
                advance_search_scan(backend.state_mut(), scoped, 1),
                SearchScanAdvance::Stale
            );

            let reopened = begin_scan(
                backend.state_mut(),
                target,
                SearchScope::FocusedPane,
                "needle",
            );
            backend
                .dispatch(crate::Msg::CloseSearch)
                .expect("close search");
            assert!(backend.state().search.is_none());
            assert_eq!(
                advance_search_scan(backend.state_mut(), reopened, 1),
                SearchScanAdvance::Stale
            );
            let next = begin_scan(
                backend.state_mut(),
                target,
                SearchScope::FocusedPane,
                "needle",
            );
            assert_ne!(reopened, next);
        });
    }

    #[test]
    fn appending_slices_preserves_selection_and_item_alignment() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        find_pane_mut(&mut state, 1)
            .expect("initial pane")
            .terminal
            .process_server_output(&output_lines(700));
        let epoch = begin_scan(&mut state, 1, SearchScope::FocusedPane, "needle");
        assert_eq!(
            advance_search_scan(&mut state, epoch, 100),
            SearchScanAdvance::Running {
                first_match: true,
                render: true,
            }
        );
        {
            let search = state.search.as_mut().expect("search");
            search.current = 50;
            search.refresh_match_status();
        }
        assert_eq!(
            advance_search_scan(&mut state, epoch, 100),
            SearchScanAdvance::Running {
                first_match: false,
                render: false,
            }
        );
        let search = state.search.as_ref().expect("search");
        assert_eq!(search.current, 50);
        assert_eq!(search.matches.len(), search.items.len());
        assert!(
            search
                .items
                .iter()
                .enumerate()
                .all(|(index, item)| item.value == index)
        );
        assert!(search.status.contains('…'));
    }

    #[test]
    fn scan_transition_classes_match_render_contract() {
        let stale = scan_advance_update(SearchScanAdvance::Stale, 1);
        assert_eq!(stale.level(), tui_lipan::UpdateLevel::None);
        assert!(stale.command.is_none());

        let intermediate = scan_advance_update(
            SearchScanAdvance::Running {
                first_match: false,
                render: false,
            },
            1,
        );
        assert_eq!(intermediate.level(), tui_lipan::UpdateLevel::None);
        assert!(intermediate.command.is_some());

        let purged = scan_advance_update(
            SearchScanAdvance::Running {
                first_match: false,
                render: true,
            },
            1,
        );
        assert_eq!(purged.level(), tui_lipan::UpdateLevel::Full);
        assert!(purged.command.is_some());

        let first = scan_advance_update(
            SearchScanAdvance::Running {
                first_match: true,
                render: true,
            },
            1,
        );
        assert_eq!(first.level(), tui_lipan::UpdateLevel::Full);
        assert!(first.command.is_some());

        let complete = scan_advance_update(SearchScanAdvance::Complete { first_match: false }, 1);
        assert_eq!(complete.level(), tui_lipan::UpdateLevel::Full);
        assert!(complete.command.is_none());
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
                let epoch = begin_scan(backend.state_mut(), target, SearchScope::All, "needle");
                drain_scan(backend.state_mut(), epoch, 317);
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
                let epoch = begin_scan(
                    backend.state_mut(),
                    target,
                    SearchScope::FocusedPane,
                    "needle",
                );
                backend
                    .state_mut()
                    .search
                    .as_mut()
                    .expect("search")
                    .from_copy_mode = true;
                drain_scan(backend.state_mut(), epoch, 11);
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
                    .dispatch(crate::Msg::SearchActivate(0))
                    .expect("activate copy match");
                let copy = backend.state().copy_mode.as_ref().expect("copy mode");
                assert_eq!(
                    copy.navigation.cursor(),
                    (selected.line, selected.start_col)
                );
                assert_eq!(copy.navigation.scrollback_offset(), selected.offset);
                assert_eq!(copy.search_matches.len(), 1);
                assert!(!copy.search_truncated);
            })
            .expect("spawn copy search test")
            .join()
            .expect("copy search test completes");
    }

    #[test]
    fn incomplete_copy_search_hands_off_discovered_matches_as_truncated() {
        on_large_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
            let target = backend
                .state()
                .current()
                .focused_pane
                .expect("focused pane");
            find_pane_mut(backend.state_mut(), target)
                .expect("target pane")
                .terminal
                .process_server_output(&output_lines(20));
            backend.state_mut().copy_mode = Some(CopyModeState {
                target,
                navigation: TerminalCopyMode::new(0, 0, 0),
                search_matches: Vec::new(),
                search_current: 0,
                search_truncated: false,
            });
            let epoch = begin_scan(
                backend.state_mut(),
                target,
                SearchScope::FocusedPane,
                "needle",
            );
            backend
                .state_mut()
                .search
                .as_mut()
                .expect("search")
                .from_copy_mode = true;
            assert_eq!(
                advance_search_scan(backend.state_mut(), epoch, 1),
                SearchScanAdvance::Running {
                    first_match: true,
                    render: true,
                }
            );

            backend
                .dispatch(crate::Msg::SearchActivate(0))
                .expect("finish incomplete copy search");
            let copy = backend.state().copy_mode.as_ref().expect("copy mode");
            assert_eq!(copy.search_matches.len(), 1);
            assert!(copy.search_truncated);
            assert!(backend.state().search.is_none());
        });
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
                let epoch = begin_scan(
                    backend.state_mut(),
                    target,
                    SearchScope::FocusedPane,
                    "needle",
                );
                backend
                    .state_mut()
                    .search
                    .as_mut()
                    .expect("search")
                    .from_copy_mode = true;
                drain_scan(backend.state_mut(), epoch, 1);
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

    #[test]
    fn search_jump_scrollable_animation_policy_by_workspace() {
        on_large_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let rect = FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            };
            {
                let state = backend.state_mut();
                state.current_mut().workspaces[0].layout_kind =
                    crate::state::LayoutKind::Scrollable;
                for id in [2, 3, 4] {
                    state.current_mut().workspaces[0]
                        .panes
                        .push(Pane::new(id, 100, rect));
                    crate::tiling::append_tiled_window(&mut state.current_mut().workspaces[0], id);
                }
                state.current_mut().workspaces[1].layout_kind =
                    crate::state::LayoutKind::Scrollable;
                state.current_mut().workspaces[1]
                    .panes
                    .push(Pane::new(10, 100, rect));
                crate::tiling::append_tiled_window(&mut state.current_mut().workspaces[1], 10);
                focus_pane(state, 1);
                state.animation = GeometryAnimation::None;
                let mut search = ScrollbackSearchState::new(1);
                search.matches = vec![
                    ScrollbackMatch {
                        offset: 0,
                        line: 0,
                        start_col: 0,
                        end_col: 1,
                        text: Arc::from("a"),
                        pane: 4,
                    },
                    ScrollbackMatch {
                        offset: 0,
                        line: 0,
                        start_col: 0,
                        end_col: 1,
                        text: Arc::from("b"),
                        pane: 10,
                    },
                ];
                search.current = 0;
                state.search = Some(search);
            }
            backend.render();

            backend
                .dispatch(crate::Msg::SearchSelect(0))
                .expect("same-workspace search jump");
            assert_eq!(backend.state().current().active_workspace, 0);
            assert_eq!(backend.state().current().focused_pane, Some(4));
            assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);

            backend.state_mut().animation = GeometryAnimation::None;
            backend
                .dispatch(crate::Msg::SearchSelect(1))
                .expect("cross-workspace search jump");
            assert_eq!(backend.state().current().active_workspace, 1);
            assert_eq!(backend.state().current().focused_pane, Some(10));
            assert_eq!(
                backend.state().animation,
                GeometryAnimation::None,
                "cross-workspace search jump must finish instant"
            );
        });
    }
}
