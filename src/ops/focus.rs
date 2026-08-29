use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::anim::GeometryAnimation;
use crate::geometry::{closest_pane_to_rect, directional_score, workspace_tile_bounds};
use crate::layout::{
    placement_for, scrollable_viewport_anchor, workspace_target_rects,
    workspace_target_rects_with_visible_bounds,
};
use crate::state::{
    Direction, DirectionalFocusHint, LayoutKind, Pane, PaneId, ScrollableRevealEdge, State,
    Workspace,
};
use crate::tiling::{self, append_tiled_window, remove_tiled_window};
use crate::view;

pub(crate) fn split_axis_for_direction(direction: Direction) -> crate::state::SplitAxis {
    match direction {
        Direction::Left | Direction::Right => crate::state::SplitAxis::Horizontal,
        Direction::Up | Direction::Down => crate::state::SplitAxis::Vertical,
    }
}

pub(crate) fn active_pane_is_fullscreen(state: &State, id: PaneId) -> bool {
    state
        .active_workspace_ref()
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing && pane.fullscreen)
}

/// Record host-window focus.
///
/// Deliberately *not* an acknowledgement: coming back to the window is looking, not doing, and a
/// finished run has to survive that return or the user never sees it. The pane's own attention
/// marks clear when input reaches it (`acknowledge_pane_if_attended` call sites), not when the
/// window regains focus around it.
pub(crate) fn window_focus_changed(ctx: &mut Context<AppRoot>, focused: bool) -> Update {
    ctx.state.window_focused = focused;
    Update::none()
}

/// Acknowledge attention only when the pane is selected in a focused host window.
///
/// Callers are the places the user acts *on this pane*: focusing it, typing into it, pasting into
/// it. Nothing calls this on a timer, on output, or on a redraw, so a mark stays up until it is
/// answered rather than until the next message happens to pass through.
///
/// Arriving somewhere is not acting on anything, so none of the workspace paths call this either.
/// Switching to a marked workspace, carrying a pane to one, or renumbering one is the user asking to
/// *see* what happened - and the mark is the only thing that says which pane it happened in. Clearing
/// it on arrival would delete the answer at the moment it is finally being looked for.
pub(crate) fn acknowledge_pane_if_attended(state: &mut State, pane_id: PaneId) -> bool {
    if !state.is_pane_attended(pane_id) {
        return false;
    }
    acknowledge_pane(state, pane_id)
}

/// Acknowledge a pane because input just reached it.
///
/// Input is stronger evidence than any focus flag: a key or a paste landing in this pane *is* the
/// user sitting at it, so this deliberately skips the [`State::is_pane_attended`] test that the
/// focus-driven path needs. Asking `window_focused` on top of a keystroke can only produce false
/// negatives - a host terminal that never reports focus, or a missed focus-in, would otherwise leave
/// a mark that no amount of typing could clear.
pub(crate) fn acknowledge_pane_input(state: &mut State, pane_id: PaneId) -> bool {
    acknowledge_pane(state, pane_id)
}

/// Clear one pane's attention marks. Resolved across every namespace, so a popup or scratchpad pane
/// answers input the same way a workspace pane does.
fn acknowledge_pane(state: &mut State, pane_id: PaneId) -> bool {
    let Some(pane) = crate::pane_lifecycle::find_pane_mut(state, pane_id) else {
        return false;
    };

    // Only the row the publisher has on screen was actually looked at. The others are behind a
    // tab the user has not visited, so their pulses stay lit until that tab is opened.
    let active_row = pane
        .terminal
        .published_rows
        .iter()
        .find(|row| row.active)
        .map(|row| row.id.clone());
    let row_cleared = active_row
        .and_then(|id| pane.terminal.published_row_ui.get_mut(&id))
        .is_some_and(|ui| std::mem::replace(&mut ui.finished_unseen, false));

    let changed = pane.activity.has_unseen_output
        || pane.activity.bell
        || pane.terminal.finished_unseen
        || row_cleared;
    pane.activity.has_unseen_output = false;
    pane.activity.bell = false;
    pane.terminal.finished_unseen = false;
    changed
}

/// Whether focus is pinned to the pane currently covering the workspace.
///
/// A fullscreen pane hides every tile behind it, so moving focus by direction or cycling order
/// would land on a pane the user cannot see and send their keystrokes there. Fullscreen already
/// locks out moving, resizing, and split dragging (`ops::resize_move`, `promote_focused_to_master`);
/// this is the same lock for focus. Toggling fullscreen off unlocks it.
pub(crate) fn focus_locked_by_fullscreen(state: &State) -> bool {
    state
        .focused_pane()
        .is_some_and(|id| active_pane_is_fullscreen(state, id))
}

/// Focus a live workspace pane regardless of which workspace currently owns the view.
pub(crate) fn focus_pane_anywhere(ctx: &mut Context<AppRoot>, target: PaneId) -> bool {
    let Some(workspace_index) = ctx.state.current().workspaces.iter().position(|workspace| {
        workspace
            .panes
            .iter()
            .any(|pane| pane.id == target && !pane.closing)
    }) else {
        return false;
    };
    let cross_workspace = workspace_index != ctx.state.current().active_workspace;
    if cross_workspace {
        switch_workspace(&mut ctx.state, workspace_index);
    }
    focus_pane(&mut ctx.state, target);
    if cross_workspace {
        // Cross-workspace jumps stay instant even when focus_pane arms Scrollable AxisChange.
        ctx.state.animation = GeometryAnimation::None;
    }
    request_pane_focus(ctx, target);
    true
}

/// Find the next blocked live workspace pane in deterministic workspace/pane order. The focused
/// pane is skipped, so a sole blocked focus is a no-op; with no valid focus, the first blocked pane
/// is selected.
pub(crate) fn next_blocked_pane(state: &State) -> Option<PaneId> {
    let panes = state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .filter(|pane| !pane.closing && pane.id != crate::state::POPUP_PANE_ID)
        .collect::<Vec<_>>();
    if panes.is_empty() {
        return None;
    }
    let start = state
        .current()
        .focused_pane
        .and_then(|focused| panes.iter().position(|pane| pane.id == focused))
        .map_or(0, |index| index + 1);
    (0..panes.len())
        .map(|offset| &panes[(start + offset) % panes.len()])
        .find(|pane| Some(pane.id) != state.current().focused_pane && pane.awaits_input())
        .map(|pane| pane.id)
}

pub(crate) fn focus_in_direction(
    state: &mut State,
    direction: Direction,
    viewport: Rect,
) -> Option<PaneId> {
    focus_in_direction_with_wrap(state, direction, viewport, true)
}

pub(crate) fn focus_in_direction_no_wrap(
    state: &mut State,
    direction: Direction,
    viewport: Rect,
) -> Option<PaneId> {
    focus_in_direction_with_wrap(state, direction, viewport, false)
}

fn focus_in_direction_with_wrap(
    state: &mut State,
    direction: Direction,
    viewport: Rect,
    wrap: bool,
) -> Option<PaneId> {
    if focus_locked_by_fullscreen(state) {
        return None;
    }
    if monocle_order_focus_applies(state) {
        return focus_in_monocle_order(state, direction, wrap);
    }
    let bounds = state.layout_bounds(viewport);
    let workspace = state.active_workspace_ref();
    let placements =
        workspace_target_rects(workspace, bounds, state.layout_top_gap(), state.tile_gap());
    let candidates: Vec<_> = workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .filter_map(|pane| {
            placement_for(&placements, pane.id)
                .map(|rect| tiling::PanePlacement { id: pane.id, rect })
        })
        .collect();

    if candidates.is_empty() {
        if state.scratch_visible {
            state.scratch.focused_pane = None;
        } else {
            state.current_mut().focused_pane = None;
        }
        return None;
    }

    let focused = state.focused_pane().unwrap_or(candidates[0].id);
    let Some(current) = candidates.iter().find(|candidate| candidate.id == focused) else {
        let id = candidates[0].id;
        focus_pane(state, id);
        return Some(id);
    };
    // Across a strip layout's single axis every tiled pane spans the full extent, so no tile sits
    // ahead of another and `directional_score` rightly rejects them all. Wrapping there would fall
    // through to an arbitrary equally-ranked pane, which is why this bounced between two panes no
    // matter how many were open. Walking the whole order is what `cycle-focus` (Tab) already does,
    // so the key simply finds nothing among the tiles instead of lying about their arrangement. A
    // floating pane genuinely across the axis still scores normally and stays reachable.
    let wrap = wrap
        && !(strip_layout_cross_axis(workspace.layout_kind, direction)
            && workspace
                .panes
                .iter()
                .any(|pane| pane.id == focused && !pane.floating && !pane.closing));
    let continue_band = continue_focus_band(workspace, &candidates, focused, direction);
    let geometric = candidates
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            // Require cross-axis overlap so focus stays orthogonal (2→Right wraps to 1, not
            // diagonally to 4). Swap already uses the same rule.
            (cross_axis_overlap(current.rect, candidate.rect, direction) > 0.0)
                .then(|| directional_score(current.rect, candidate.rect, direction))
                .flatten()
                .map(|score| {
                    let band_rank = continue_band.map_or((0, 0.0), |band| {
                        cross_axis_band_rank(band, candidate.rect, direction)
                    });
                    (candidate.id, band_rank, score)
                })
        })
        .min_by(|(_, band_a, score_a), (_, band_b, score_b)| {
            band_a
                .0
                .cmp(&band_b.0)
                .then_with(|| band_a.1.total_cmp(&band_b.1))
                .then_with(|| score_a.total_cmp(score_b))
        })
        .map(|(id, _, _)| id)
        .or_else(|| {
            wrap.then(|| wrapped_focus_id(&candidates, current, direction, continue_band))
                .flatten()
        });
    let remembered = remembered_focus_target(workspace, &candidates, focused, direction);
    let next = prefer_aligned_focus_target(&candidates, current, direction, remembered, geometric);

    if let Some(next_id) = next {
        focus_pane(state, next_id);
        state.active_workspace_mut().last_directional_focus = Some(DirectionalFocusHint {
            pane: next_id,
            entry_direction: direction,
            target: focused,
        });
        Some(next_id)
    } else {
        None
    }
}

/// Whether `direction` runs across a strip layout's single axis: Columns and Scrollable lay panes
/// out left to right, so vertical movement crosses them; Rows is the transpose. Grid and Dwindle
/// arrange both axes, Master stacks beside its master tile, and Monocle has its own order walk.
fn strip_layout_cross_axis(kind: LayoutKind, direction: Direction) -> bool {
    match kind {
        LayoutKind::Columns | LayoutKind::Scrollable => {
            matches!(direction, Direction::Up | Direction::Down)
        }
        LayoutKind::Rows => matches!(direction, Direction::Left | Direction::Right),
        _ => false,
    }
}

/// Whether directional focus should walk the monocle stack instead of scoring geometry.
///
/// Monocle gives every tiled pane the same rect, so `directional_score` rejects every candidate
/// and the wrap fallback ranks them all equally — focus then bounces between the focused pane and
/// whichever pane sorts first. A floating focus keeps the geometric path, since floating panes
/// still have rects of their own to move out of.
fn monocle_order_focus_applies(state: &State) -> bool {
    let workspace = state.active_workspace_ref();
    if workspace.layout_kind != LayoutKind::Monocle {
        return false;
    }
    match state.focused_pane() {
        Some(focused) => workspace
            .panes
            .iter()
            .any(|pane| pane.id == focused && !pane.floating && !pane.closing),
        None => !workspace.tiled_ids().is_empty(),
    }
}

/// Right/Down advance through the monocle stack, Left/Up step back. The directional hint is
/// cleared because it only feeds the geometric path, where a stale entry pane recorded under
/// monocle would misdirect the first move after switching to another layout.
fn focus_in_monocle_order(state: &mut State, direction: Direction, wrap: bool) -> Option<PaneId> {
    let forward = matches!(direction, Direction::Right | Direction::Down);
    let next = step_focus_in_tiled_order(state, forward, wrap)?;
    state.active_workspace_mut().last_directional_focus = None;
    Some(next)
}

fn remembered_focus_target(
    workspace: &Workspace,
    candidates: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let hint = workspace.last_directional_focus?;
    // Only reverse restores the exact entry sibling. Continue uses `continue_focus_band` so wrap
    // stays in the entry row/column (4→3→1→4) instead of jumping to another band (→2) or
    // oscillating on the entry pane (1↔3).
    (hint.pane == focused
        && direction == opposite_direction(hint.entry_direction)
        && candidates
            .iter()
            .any(|candidate| candidate.id == hint.target))
    .then_some(hint.target)
}

/// When continuing in the direction that entered the current pane, remember the entry pane's
/// cross-axis band so edge wrap stays on that row/column.
fn continue_focus_band(
    workspace: &Workspace,
    candidates: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<FloatRect> {
    let hint = workspace.last_directional_focus?;
    (hint.pane == focused && direction == hint.entry_direction)
        .then(|| {
            candidates
                .iter()
                .find(|candidate| candidate.id == hint.target)
                .map(|candidate| candidate.rect)
        })
        .flatten()
}

fn prefer_aligned_focus_target(
    candidates: &[tiling::PanePlacement],
    current: &tiling::PanePlacement,
    direction: Direction,
    remembered: Option<PaneId>,
    geometric: Option<PaneId>,
) -> Option<PaneId> {
    let Some(geometric) = geometric else {
        return remembered;
    };
    let Some(remembered) = remembered else {
        return Some(geometric);
    };
    if remembered == geometric {
        return Some(geometric);
    }
    let remembered_rect = candidates
        .iter()
        .find(|candidate| candidate.id == remembered)?
        .rect;
    let geometric_rect = candidates
        .iter()
        .find(|candidate| candidate.id == geometric)?
        .rect;
    let remembered_in_direction = is_orthogonal_neighbor(current.rect, remembered_rect, direction);
    let geometric_in_direction = is_orthogonal_neighbor(current.rect, geometric_rect, direction);
    // Reverse-sticky may point at a behind-the-back pane on a wrap; a real forward neighbor wins.
    if geometric_in_direction && !remembered_in_direction {
        return Some(geometric);
    }
    if cross_axis_gap(current.rect, remembered_rect, direction)
        <= cross_axis_gap(current.rect, geometric_rect, direction)
    {
        Some(remembered)
    } else {
        Some(geometric)
    }
}

fn wrapped_focus_id(
    candidates: &[tiling::PanePlacement],
    current: &tiling::PanePlacement,
    direction: Direction,
    band: Option<FloatRect>,
) -> Option<PaneId> {
    candidates
        .iter()
        .filter(|candidate| candidate.id != current.id)
        .min_by(|a, b| compare_wrap_candidates(current.rect, a.rect, b.rect, direction, band))
        .map(|candidate| candidate.id)
}

fn compare_wrap_candidates(
    current: FloatRect,
    a: FloatRect,
    b: FloatRect,
    direction: Direction,
    band: Option<FloatRect>,
) -> std::cmp::Ordering {
    let rank = |candidate: FloatRect| {
        let (current_start, current_end, candidate_start, candidate_end, opposite_edge) =
            match direction {
                Direction::Left => (
                    current.y,
                    current.y + current.h,
                    candidate.y,
                    candidate.y + candidate.h,
                    -(candidate.x + candidate.w),
                ),
                Direction::Right => (
                    current.y,
                    current.y + current.h,
                    candidate.y,
                    candidate.y + candidate.h,
                    candidate.x,
                ),
                Direction::Up => (
                    current.x,
                    current.x + current.w,
                    candidate.x,
                    candidate.x + candidate.w,
                    -(candidate.y + candidate.h),
                ),
                Direction::Down => (
                    current.x,
                    current.x + current.w,
                    candidate.x,
                    candidate.x + candidate.w,
                    candidate.y,
                ),
            };
        let band_rank = band.map_or((0, 0.0), |band| {
            cross_axis_band_rank(band, candidate, direction)
        });
        let cross_gap = interval_gap(current_start, current_end, candidate_start, candidate_end);
        let center_offset =
            ((candidate_start + candidate_end) - (current_start + current_end)).abs();
        (
            band_rank.0,
            band_rank.1,
            cross_gap,
            opposite_edge,
            center_offset,
        )
    };
    let a = rank(a);
    let b = rank(b);
    a.0.cmp(&b.0)
        .then_with(|| a.1.total_cmp(&b.1))
        .then_with(|| a.2.total_cmp(&b.2))
        .then_with(|| a.3.total_cmp(&b.3))
        .then_with(|| a.4.total_cmp(&b.4))
}

fn is_orthogonal_neighbor(current: FloatRect, candidate: FloatRect, direction: Direction) -> bool {
    directional_score(current, candidate, direction).is_some()
        && cross_axis_overlap(current, candidate, direction) > 0.0
}

fn opposite_direction(direction: Direction) -> Direction {
    match direction {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
    }
}

fn cross_axis_gap(current: FloatRect, candidate: FloatRect, direction: Direction) -> f32 {
    match direction {
        Direction::Left | Direction::Right => interval_gap(
            current.y,
            current.y + current.h,
            candidate.y,
            candidate.y + candidate.h,
        ),
        Direction::Up | Direction::Down => interval_gap(
            current.x,
            current.x + current.w,
            candidate.x,
            candidate.x + candidate.w,
        ),
    }
}

fn cross_axis_band_rank(band: FloatRect, candidate: FloatRect, direction: Direction) -> (u8, f32) {
    if cross_axis_overlap(band, candidate, direction) > 0.0 {
        (0, 0.0)
    } else {
        (1, cross_axis_gap(band, candidate, direction))
    }
}

fn cross_axis_overlap(current: FloatRect, candidate: FloatRect, direction: Direction) -> f32 {
    match direction {
        Direction::Left | Direction::Right => interval_overlap(
            current.y,
            current.y + current.h,
            candidate.y,
            candidate.y + candidate.h,
        ),
        Direction::Up | Direction::Down => interval_overlap(
            current.x,
            current.x + current.w,
            candidate.x,
            candidate.x + candidate.w,
        ),
    }
}

fn interval_gap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    if b_end < a_start {
        a_start - b_end
    } else if b_start > a_end {
        b_start - a_end
    } else {
        0.0
    }
}

fn interval_overlap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

/// Choose the next/right or previous/left neighbour of a closing Scrollable tile. Call this before
/// the pane is marked closing: [`Workspace::tiled_ids`] then still describes the pre-close strip.
pub(crate) fn scrollable_close_neighbor(workspace: &Workspace, id: PaneId) -> Option<PaneId> {
    if workspace.layout_kind != LayoutKind::Scrollable {
        return None;
    }

    let tiled = workspace.tiled_ids();
    let index = tiled.iter().position(|pane_id| *pane_id == id)?;
    tiled.get(index.checked_add(1)?).copied().or_else(|| {
        index
            .checked_sub(1)
            .and_then(|index| tiled.get(index).copied())
    })
}

/// Move focus to the next/previous tiled pane in `tiled_ids()` order, wrapping around. If
/// the current focus is floating (not part of the tiled order) it falls back to the first
/// tiled pane. Returns the newly focused id, or `None` when there are no tiled panes.
pub(crate) fn cycle_focus_in_tiled_order(state: &mut State, forward: bool) -> Option<PaneId> {
    if focus_locked_by_fullscreen(state) {
        return None;
    }
    step_focus_in_tiled_order(state, forward, true)
}

/// Step focus one pane along `tiled_ids()` order. Without `wrap`, a step past either end is a
/// no-op instead of jumping to the opposite end.
fn step_focus_in_tiled_order(state: &mut State, forward: bool, wrap: bool) -> Option<PaneId> {
    let ids = state.active_workspace_ref().tiled_ids();
    if ids.is_empty() {
        return None;
    }
    let next = match state
        .focused_pane()
        .and_then(|id| ids.iter().position(|c| *c == id))
    {
        Some(index) if forward => match index + 1 {
            next if next < ids.len() => next,
            _ if wrap => 0,
            _ => return None,
        },
        Some(index) => match index.checked_sub(1) {
            Some(previous) => previous,
            None if wrap => ids.len() - 1,
            None => return None,
        },
        None => 0,
    };
    let id = ids[next];
    focus_pane(state, id);
    Some(id)
}

/// Swap the focused pane into the master slot (the first tiled pane), exchanging positions
/// with whatever pane is there. No-op for a floating/fullscreen focus, when the focused pane
/// is not tiled, or when it is already the master. Returns `true` when a swap happened.
pub(crate) fn promote_focused_to_master(state: &mut State) -> bool {
    let Some(focused) = state.focused_pane() else {
        return false;
    };
    if active_pane_is_fullscreen(state, focused) {
        return false;
    }
    let swapped = {
        let workspace = state.active_workspace_mut();
        let ids = workspace.tiled_ids();
        let Some(&master) = ids.first() else {
            return false;
        };
        if master == focused || !ids.contains(&focused) {
            return false;
        }
        if workspace.tile_tree.is_none() {
            workspace.tile_tree = crate::layout::effective_tile_tree(workspace, None);
        }
        let Some(tree) = workspace.tile_tree.as_mut() else {
            return false;
        };
        crate::tiling::swap_tree_leaves(tree, focused, master)
    };
    if !swapped {
        return false;
    }
    if state.scratch_visible {
        state.scratch.focused_pane = Some(focused);
    } else {
        state.current_mut().focused_pane = Some(focused);
    }
    state.active_workspace_mut().focused_pane = Some(focused);
    state.active_workspace_mut().last_move_swap = None;
    state.active_workspace_mut().last_directional_focus = None;
    // Reorder can clip the focused pane under a preserved non-focus anchor.
    sync_scrollable_reveal(state, focused, false);
    true
}

pub(crate) fn switch_workspace(state: &mut State, index: usize) {
    if state.scratch_visible {
        return;
    }
    if index >= state.current().workspaces.len() {
        return;
    }
    let previous = state.current().active_workspace;
    state.current_mut().active_workspace = index;
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
    if let Some(focus) = state.current().focused_pane {
        // Normalize Scrollable viewport for the newly active focus (covers inactive reconcile
        // fallback under a surviving foreign anchor, and other stale local viewport state).
        sync_scrollable_reveal(state, focus, false);
    }
    state.animation = GeometryAnimation::None;
    if previous != index {
        emit_workspace_switched(state, index);
    }
}

/// Emit the public `workspace-switched` event. Every mutation of `active_workspace` must go
/// through this (switch, move-with-pane, relocate) so subscribers never see a stale workspace.
fn emit_workspace_switched(state: &State, index: usize) {
    crate::events::emit(
        state,
        crate::events::Event::new(
            crate::events::EventKind::WorkspaceSwitched,
            vec![("workspace", (index + 1).to_string())],
        ),
    );
}

pub(crate) fn move_focused_to_workspace(state: &mut State, target_index: usize) {
    if target_index >= state.current().workspaces.len() {
        return;
    }
    let source_index = state.current().active_workspace;
    let Some(focused) = state.current().focused_pane else {
        return;
    };
    if source_index == target_index {
        return;
    }

    let Some(position) = state.current().workspaces[source_index]
        .panes
        .iter()
        .position(|pane| pane.id == focused)
    else {
        choose_fallback_focus(state);
        return;
    };

    let mut pane = state.current_mut().workspaces[source_index]
        .panes
        .remove(position);
    let tiled = !pane.floating;
    if tiled {
        remove_tiled_window(&mut state.current_mut().workspaces[source_index], pane.id);
    }
    pane.opening = false;
    pane.closing = false;

    choose_fallback_focus(state);

    if tiled {
        append_tiled_window(&mut state.current_mut().workspaces[target_index], pane.id);
    }
    state.current_mut().workspaces[target_index]
        .panes
        .push(pane);

    state.current_mut().active_workspace = target_index;
    let scrollable = state.current().workspaces[target_index].layout_kind == LayoutKind::Scrollable;
    let (prior_anchor, prior_edge, reveal_decision) = if tiled && scrollable {
        let ws = &state.current().workspaces[target_index];
        let prior = scrollable_viewport_anchor(ws, &ws.tiled_ids());
        let edge = ws.scrollable_reveal_edge;
        // Classify before overwriting target focus so a missing stored anchor still uses the
        // previous tiled focus as the strip reference.
        let decision = classify_scrollable_reveal(state, focused, prior);
        (prior, Some(edge), decision)
    } else {
        (None, None, None)
    };
    state.current_mut().focused_pane = Some(focused);
    state.current_mut().workspaces[target_index].focused_pane = Some(focused);
    if tiled && scrollable {
        apply_scrollable_reveal_decision(
            state,
            focused,
            prior_anchor,
            prior_edge,
            reveal_decision,
            false,
        );
    }
    state.animation = GeometryAnimation::None;
    emit_workspace_switched(state, target_index);
}

/// Move every pane from the active workspace into `target_index`, carry the source workspace
/// name and layout over when set, then switch to the target workspace and keep focus on the
/// previously focused pane when it moved with the batch. An empty target slot receives the
/// source content wholesale; a occupied target swaps content with the source so both layouts
/// stay intact.
pub(crate) fn relocate_active_workspace(state: &mut State, target_index: usize) {
    if target_index >= state.current().workspaces.len() {
        return;
    }
    let source_index = state.current().active_workspace;
    if source_index == target_index {
        return;
    }

    let previous_focus = state.current().focused_pane;
    let source_empty = workspace_is_empty(&state.current().workspaces[source_index]);
    if source_empty {
        state.current_mut().active_workspace = target_index;
        choose_fallback_focus(state);
        state.animation = GeometryAnimation::None;
        emit_workspace_switched(state, target_index);
        return;
    }

    let target_empty = workspace_is_empty(&state.current().workspaces[target_index]);
    if target_empty {
        transfer_workspace_content(state, source_index, target_index);
    } else {
        swap_workspace_content(state, source_index, target_index);
    }

    let target = &mut state.current_mut().workspaces[target_index];
    if let Some(id) = previous_focus
        && target
            .panes
            .iter()
            .any(|pane| pane.id == id && !pane.closing)
    {
        target.focused_pane = Some(id);
    } else if target.focused_pane.is_none() {
        target.focused_pane = first_visible_pane(target);
    }
    let target_focus = target.focused_pane;

    state.current_mut().active_workspace = target_index;
    state.current_mut().focused_pane = target_focus;
    state.animation = GeometryAnimation::None;
    emit_workspace_switched(state, target_index);
}

fn workspace_is_empty(workspace: &Workspace) -> bool {
    !workspace.panes.iter().any(|pane| !pane.closing)
}

fn swap_workspace_content(state: &mut State, source_index: usize, target_index: usize) {
    if source_index < target_index {
        let (left, right) = state.current_mut().workspaces.split_at_mut(target_index);
        swap_workspace_fields(&mut left[source_index], &mut right[0]);
    } else {
        let (left, right) = state.current_mut().workspaces.split_at_mut(source_index);
        swap_workspace_fields(&mut right[0], &mut left[target_index]);
    }
}

fn transfer_workspace_content(state: &mut State, source_index: usize, target_index: usize) {
    if source_index < target_index {
        let (left, right) = state.current_mut().workspaces.split_at_mut(target_index);
        transfer_workspace_fields(&mut left[source_index], &mut right[0]);
        left[source_index] = Workspace::new(source_index);
    } else {
        let (left, right) = state.current_mut().workspaces.split_at_mut(source_index);
        transfer_workspace_fields(&mut right[0], &mut left[target_index]);
        right[0] = Workspace::new(source_index);
    }
}

fn swap_workspace_fields(a: &mut Workspace, b: &mut Workspace) {
    std::mem::swap(&mut a.panes, &mut b.panes);
    std::mem::swap(&mut a.tile_tree, &mut b.tile_tree);
    std::mem::swap(&mut a.focused_pane, &mut b.focused_pane);
    std::mem::swap(&mut a.synchronized, &mut b.synchronized);
    std::mem::swap(&mut a.layout_kind, &mut b.layout_kind);
    std::mem::swap(&mut a.start_axis, &mut b.start_axis);
    std::mem::swap(&mut a.split_ratios, &mut b.split_ratios);
    std::mem::swap(&mut a.last_move_swap, &mut b.last_move_swap);
    std::mem::swap(&mut a.last_directional_focus, &mut b.last_directional_focus);
    std::mem::swap(&mut a.scrollable_anchor, &mut b.scrollable_anchor);
    std::mem::swap(&mut a.scrollable_reveal_edge, &mut b.scrollable_reveal_edge);
    std::mem::swap(&mut a.name, &mut b.name);
}

fn transfer_workspace_fields(from: &mut Workspace, to: &mut Workspace) {
    to.panes = std::mem::take(&mut from.panes);
    to.tile_tree = from.tile_tree.take();
    to.focused_pane = from.focused_pane.take();
    to.synchronized = from.synchronized;
    to.layout_kind = from.layout_kind;
    to.start_axis = from.start_axis;
    to.split_ratios.clone_from(&from.split_ratios);
    to.last_move_swap = from.last_move_swap.take();
    to.last_directional_focus = from.last_directional_focus.take();
    to.scrollable_anchor = from.scrollable_anchor.take();
    to.scrollable_reveal_edge = std::mem::take(&mut from.scrollable_reveal_edge);
    to.name = from.name.take();
}

/// Apply the focus-follows-mouse policy for a pane the pointer is over. Returns a full repaint
/// only when focus actually moved.
///
/// Shared by the widget-level `HoverPane` message and by forwarded pane mouse motion. The latter
/// matters because a pane running mouse tracking (an `AnyEvent` TUI) consumes pointer motion in
/// the framework before the per-pane hover callback can run, so without this path on-hover focus
/// would never fire over a full-screen TUI. Only tiled/floating panes in the active workspace
/// participate; the scratchpad keeps its own focus lifecycle and must not hijack `focused_pane`.
pub(crate) fn hover_focus_pane(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    if !ctx.state.config.pane.focus_on_hover {
        return Update::none();
    }
    // Hover-focus is ambient: it follows the pointer with no intent behind it. While the app owns
    // the keyboard — the sidebar, or resize/copy/hint mode — that must not override the mode the
    // user deliberately entered. Reaching the sidebar with the mouse means crossing panes, and that
    // transit would otherwise hand the keyboard back before the click arrived. Clicking a pane is
    // still a deliberate act and still leaves, so there is always a way out.
    if ctx.state.sidebar.focused || ctx.state.mode != crate::state::Mode::Normal {
        return Update::none();
    }
    if ctx.state.focused_pane() == Some(id) {
        return Update::none();
    }
    let focusable = ctx
        .state
        .active_workspace_ref()
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing);
    if !focusable {
        return Update::none();
    }
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    Update::full()
}

pub(crate) fn focus_pane(state: &mut State, id: PaneId) {
    let previous = state.focused_pane();
    let scrollable = state.active_workspace_ref().layout_kind == LayoutKind::Scrollable;
    // Capture before mutation so same-workspace Scrollable focus-scroll can detect a real
    // anchor/viewport change (and overwrite stale None left by resize) without re-arming on
    // reaffirm or when the target is already fully visible.
    let prior_scrollable_anchor = scrollable
        .then(|| {
            let ws = state.active_workspace_ref();
            scrollable_viewport_anchor(ws, &ws.tiled_ids())
        })
        .flatten();
    let prior_reveal_edge = scrollable.then(|| state.active_workspace_ref().scrollable_reveal_edge);
    let reveal_decision = scrollable
        .then(|| classify_scrollable_reveal(state, id, prior_scrollable_anchor))
        .flatten();
    // Only drop axis sticky when focus actually moves. Framework focus sync (and other
    // re-affirmations of the current pane) call this on every key and must not erase the hint
    // that keeps 4→3→1→4 on the entry row.
    if previous != Some(id) {
        state.active_workspace_mut().last_directional_focus = None;
    }
    let mut anchored_tiled = false;
    if let Some(pane) = state
        .active_workspace_mut()
        .panes
        .iter_mut()
        .find(|pane| pane.id == id && !pane.closing)
    {
        anchored_tiled = !pane.floating;
        if state.scratch_visible {
            state.scratch.focused_pane = Some(id);
        } else {
            state.current_mut().focused_pane = Some(id);
        }
        state.active_workspace_mut().focused_pane = Some(id);
    }
    acknowledge_pane_if_attended(state, id);
    if anchored_tiled {
        if scrollable {
            apply_scrollable_reveal_decision(
                state,
                id,
                prior_scrollable_anchor,
                prior_reveal_edge,
                reveal_decision,
                true,
            );
        } else {
            state.active_workspace_mut().scrollable_anchor = Some(id);
        }
    }
    if previous != state.focused_pane() && state.focused_pane() == Some(id) {
        crate::events::emit(
            state,
            crate::events::Event::new(
                crate::events::EventKind::FocusChanged,
                vec![("pane", id.to_string())],
            ),
        );
    }
}

/// Sync Scrollable local viewport for an already-focused tiled pane (move / reconcile).
/// When `arm_axis_change` is false, animation is left alone for the caller to set.
pub(crate) fn sync_scrollable_reveal(state: &mut State, id: PaneId, arm_axis_change: bool) {
    let ws = state.active_workspace_ref();
    if ws.layout_kind != LayoutKind::Scrollable {
        return;
    }
    if !ws
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.floating && !pane.closing)
    {
        return;
    }
    let prior_scrollable_anchor = scrollable_viewport_anchor(ws, &ws.tiled_ids());
    let prior_reveal_edge = Some(ws.scrollable_reveal_edge);
    let reveal_decision = classify_scrollable_reveal(state, id, prior_scrollable_anchor);
    apply_scrollable_reveal_decision(
        state,
        id,
        prior_scrollable_anchor,
        prior_reveal_edge,
        reveal_decision,
        arm_axis_change,
    );
}

fn apply_scrollable_reveal_decision(
    state: &mut State,
    id: PaneId,
    prior_scrollable_anchor: Option<PaneId>,
    prior_reveal_edge: Option<ScrollableRevealEdge>,
    reveal_decision: Option<ScrollableRevealDecision>,
    arm_axis_change: bool,
) {
    match reveal_decision {
        Some(ScrollableRevealDecision::Preserve) => {
            // Keep the strip put: materialize the pre-focus effective anchor so a
            // None/stale stored value cannot flip the fallback after focus mutates.
            if let Some(anchor) = prior_scrollable_anchor {
                state.active_workspace_mut().set_scrollable_viewport(
                    Some(anchor),
                    prior_reveal_edge.unwrap_or(ScrollableRevealEdge::Left),
                );
            }
        }
        Some(ScrollableRevealDecision::Align(edge)) => {
            state
                .active_workspace_mut()
                .set_scrollable_viewport(Some(id), edge);
            if arm_axis_change {
                let ws = state.active_workspace_ref();
                let new_anchor = scrollable_viewport_anchor(ws, &ws.tiled_ids());
                let edge_changed = prior_reveal_edge.is_some_and(|prior| prior != edge);
                if new_anchor != prior_scrollable_anchor || edge_changed {
                    state.animation = GeometryAnimation::AxisChange;
                }
            }
        }
        None => {
            state
                .active_workspace_mut()
                .set_scrollable_viewport(Some(id), ScrollableRevealEdge::Left);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollableRevealDecision {
    Preserve,
    Align(ScrollableRevealEdge),
}

/// Classify how Scrollable focus should adjust the local viewport before mutation.
///
/// Uses the current rendered visible interval when available; otherwise tiled order relative to
/// the prior effective anchor (leftward → Left, rightward → Right, same → keep current edge).
fn classify_scrollable_reveal(
    state: &State,
    target: PaneId,
    prior_anchor: Option<PaneId>,
) -> Option<ScrollableRevealDecision> {
    let ws = state.active_workspace_ref();
    if ws.layout_kind != LayoutKind::Scrollable {
        return None;
    }
    if !ws
        .panes
        .iter()
        .any(|pane| pane.id == target && !pane.floating && !pane.closing)
    {
        return None;
    }

    let current_edge = ws.scrollable_reveal_edge;
    let tiled = ws.tiled_ids();
    let order_edge =
        || scrollable_reveal_edge_from_order(&tiled, target, prior_anchor, current_edge);

    let Some(viewport) = state.last_viewport.get() else {
        return Some(ScrollableRevealDecision::Align(order_edge()));
    };

    // Same allocation path as `view::render`: canonical/letterbox layout + local visible clamp.
    // The scratchpad has no canonical/local split - it is never letterboxed to a controller - so
    // both are the dropdown's own box. Measuring it against the whole canvas instead would place
    // the pane somewhere it is not, and every focus would read as "clipped" and scroll.
    let (letterbox, local) = if state.scratch_visible {
        let dropdown = state.layout_bounds(viewport);
        (dropdown, dropdown)
    } else {
        (
            crate::view::follower_letterbox_bounds(state, viewport),
            state.canvas_bounds_from_terminal_viewport(viewport),
        )
    };
    let top_gap = state.layout_top_gap();
    let tile_gap = state.tile_gap();
    let placements =
        workspace_target_rects_with_visible_bounds(ws, letterbox, local, top_gap, tile_gap);
    let Some(rect) = placement_for(&placements, target) else {
        return Some(ScrollableRevealDecision::Align(order_edge()));
    };

    let tile_letterbox = workspace_tile_bounds(letterbox, top_gap);
    let tile_local = workspace_tile_bounds(local, top_gap);
    let visible_left = tile_letterbox.x.max(tile_local.x);
    let visible_right = (tile_letterbox.x + tile_letterbox.w).min(tile_local.x + tile_local.w);
    if visible_right <= visible_left {
        return Some(ScrollableRevealDecision::Align(order_edge()));
    }

    // Cell-safe epsilon: whole-cell placement rounding must not flip containment.
    const EPS: f32 = 0.5;
    let left_clipped = rect.x < visible_left - EPS;
    let right_clipped = rect.x + rect.w > visible_right + EPS;
    match (left_clipped, right_clipped) {
        (false, false) => Some(ScrollableRevealDecision::Preserve),
        (true, false) => Some(ScrollableRevealDecision::Align(ScrollableRevealEdge::Left)),
        (false, true) => Some(ScrollableRevealDecision::Align(ScrollableRevealEdge::Right)),
        (true, true) => Some(ScrollableRevealDecision::Align(order_edge())),
    }
}

/// Reveal edge from tiled order when there is no usable rendered clip side (no viewport, both
/// edges clipped / wider than visible, or degenerate visible interval).
fn scrollable_reveal_edge_from_order(
    tiled: &[PaneId],
    target: PaneId,
    prior: Option<PaneId>,
    current_edge: ScrollableRevealEdge,
) -> ScrollableRevealEdge {
    let target_idx = tiled.iter().position(|id| *id == target);
    let prior_idx = prior.and_then(|id| tiled.iter().position(|pane| *pane == id));
    match (target_idx, prior_idx) {
        (Some(t), Some(p)) if t < p => ScrollableRevealEdge::Left,
        (Some(t), Some(p)) if t > p => ScrollableRevealEdge::Right,
        (Some(_), Some(_)) => current_edge,
        (Some(0), _) | (None, _) => ScrollableRevealEdge::Left,
        (Some(_), None) => ScrollableRevealEdge::Right,
    }
}

pub(crate) fn choose_fallback_focus(state: &mut State) {
    choose_fallback_focus_near(state, state.focused_pane(), None);
}

/// Re-resolve focus after the focused pane became unfocusable, landing on the pane nearest
/// `reference_id` rather than on whichever happens to be first in the list.
///
/// Reads the *active* workspace, so the scratchpad resolves through the same geometry as any
/// other. `reference_rect` is optional because a closing pane's frozen `floating_rect` already
/// records where it sat.
pub(crate) fn choose_fallback_focus_near(
    state: &mut State,
    reference_id: Option<PaneId>,
    reference_rect: Option<FloatRect>,
) {
    // Resolved under one immutable borrow, applied after it ends: the write targets differ per
    // branch, and the scratchpad aliases `focused_pane()` onto the workspace's own field.
    enum Fallback {
        MirrorToWorkspace(PaneId),
        MirrorToState(PaneId),
        Both(Option<PaneId>),
    }
    let fallback = {
        let workspace = state.active_workspace_ref();
        let live = |id: PaneId| {
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == id && !pane.closing)
        };
        if let Some(focused) = state.focused_pane().filter(|id| live(*id)) {
            Fallback::MirrorToWorkspace(focused)
        } else if let Some(focused) = workspace.focused_pane.filter(|id| live(*id)) {
            Fallback::MirrorToState(focused)
        } else {
            Fallback::Both(
                reference_id
                    .and_then(|reference_id| {
                        focus_near_pane_in_workspace(state, workspace, reference_id, reference_rect)
                    })
                    .or_else(|| first_visible_pane(workspace)),
            )
        }
    };

    match fallback {
        Fallback::MirrorToWorkspace(focused) => {
            state.active_workspace_mut().focused_pane = Some(focused);
        }
        Fallback::MirrorToState(focused) => state.set_focused_pane(Some(focused)),
        Fallback::Both(focus) => {
            state.active_workspace_mut().focused_pane = focus;
            state.set_focused_pane(focus);
        }
    }
}

pub(crate) fn first_visible_pane(workspace: &Workspace) -> Option<PaneId> {
    workspace
        .panes
        .iter()
        .find(|pane| !pane.closing)
        .map(|pane| pane.id)
}

pub(crate) fn focus_near_pane_in_workspace(
    state: &State,
    workspace: &Workspace,
    reference_id: PaneId,
    reference_rect: Option<FloatRect>,
) -> Option<PaneId> {
    let reference = reference_pane_rect(state, workspace, reference_id, reference_rect)?;
    let candidates: Vec<_> = visible_pane_placements(state, workspace)
        .into_iter()
        .filter(|(id, _)| *id != reference_id)
        .collect();
    closest_pane_to_rect(reference, &candidates)
}

/// The box `workspace` tiles inside, as a `(bounds, top_gap)` pair. Helpers here are handed a
/// workspace rather than reading the active one, so the scratchpad is recognized by identity: it
/// lays out in the dropdown rect, every other workspace in the whole canvas.
fn workspace_layout_box(state: &State, workspace: &Workspace, viewport: Rect) -> (FloatRect, f32) {
    if std::ptr::eq(workspace, &state.scratch) {
        (crate::scratchpad::deployed_rect(state, viewport), 0.0)
    } else {
        (
            state.canvas_bounds_from_terminal_viewport(viewport),
            state.workspace_top_gap(),
        )
    }
}

pub(crate) fn visible_pane_placements(
    state: &State,
    workspace: &Workspace,
) -> Vec<(PaneId, FloatRect)> {
    if let Some(viewport) = state.last_viewport.get() {
        let (bounds, top_gap) = workspace_layout_box(state, workspace, viewport);
        let placements = workspace_target_rects(workspace, bounds, top_gap, state.tile_gap());
        return workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .filter_map(|pane| placement_for(&placements, pane.id).map(|rect| (pane.id, rect)))
            .collect();
    }

    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .map(|pane| (pane.id, pane.floating_rect))
        .collect()
}

pub(crate) fn reference_pane_rect(
    state: &State,
    workspace: &Workspace,
    id: PaneId,
    override_rect: Option<FloatRect>,
) -> Option<FloatRect> {
    if let Some(rect) = override_rect {
        return Some(rect);
    }
    if let Some(viewport) = state.last_viewport.get() {
        let (bounds, top_gap) = workspace_layout_box(state, workspace, viewport);
        let placements = workspace_target_rects(workspace, bounds, top_gap, state.tile_gap());
        if let Some(rect) = placement_for(&placements, id) {
            return Some(rect);
        }
    }
    workspace
        .panes
        .iter()
        .find(|pane| pane.id == id)
        .map(|pane| pane.floating_rect)
}

pub(crate) fn active_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    state
        .active_workspace_mut()
        .panes
        .iter_mut()
        .find(|pane| pane.id == id)
}

pub(crate) fn request_pane_focus(ctx: &mut Context<AppRoot>, id: PaneId) {
    if ctx.state.has_modal_overlay() {
        return;
    }
    if crate::pane_lifecycle::find_pane_mut(&mut ctx.state, id)
        .is_some_and(|pane| pane.terminal_active && !pane.opening && !pane.closing)
    {
        focus_key(ctx, view::pane_terminal_key(id));
    }
}

pub(crate) fn request_current_pane_focus(ctx: &mut Context<AppRoot>) {
    if let Some(id) = ctx.state.focused_pane() {
        request_pane_focus(ctx, id);
    }
}

/// Every "give focus to something that is not the sidebar" goes through here.
///
/// The sidebar body lives in a `FocusScope::Exclude` subtree, and an excluded subtree is invisible
/// to `has_focus_within_key` — so rozi cannot ask the framework whether the sidebar still holds
/// the keyboard. `sidebar.focused` is therefore app-owned intent, and this is the one place that
/// has to retract it.
fn focus_key(ctx: &mut Context<AppRoot>, key: impl Into<tui_lipan::Key>) {
    ctx.state.sidebar.focused = false;
    ctx.request_focus(key);
}

pub(crate) fn request_search_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::search_input_key());
}

pub(crate) fn request_rename_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::rename_input_key());
}

pub(crate) fn request_rename_session_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::rename_session_input_key());
}

pub(crate) fn request_save_profile_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::save_profile_key());
}

pub(crate) fn request_profile_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::profile_picker_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::theme_picker_key());
}

pub(crate) fn request_layout_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::layout_picker_key());
}

pub(crate) fn request_palette_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::palette_key());
}

pub(crate) fn request_session_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::session_picker_key());
}

pub(crate) fn request_remote_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::remote_picker_key());
}

pub(crate) fn request_pick_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::pick_key());
}

/// Focus the text prompt an action raised over the picker. The picker stays mounted underneath,
/// so focus has to move explicitly rather than being inherited.
pub(crate) fn request_pick_prompt_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::pick_prompt_input_key());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{LayoutKind, Pane};
    use crate::tiling::{append_tiled_window, collect_tree_leaves};
    use tui_lipan::prelude::Theme;

    fn state_with_tiled(ids: &[PaneId]) -> State {
        let mut state = State::new(Config::default(), Theme::default());
        // State::new seeds pane 1; clear and rebuild a deterministic tiled set.
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        for &id in ids {
            state.current_mut().workspaces[0]
                .panes
                .push(Pane::new(id, 100, rect));
            append_tiled_window(&mut state.current_mut().workspaces[0], id);
        }
        state.current_mut().next_pane_id = ids.iter().copied().max().unwrap_or(0) + 1;
        state
    }

    /// Input outranks the focus flags. A keystroke landing in a pane is the user sitting at it, so it
    /// answers the mark even when the host window never reported focus - otherwise a terminal that
    /// does not send focus events leaves a mark no amount of typing can clear.
    #[test]
    fn input_acknowledges_a_pane_the_window_focus_gate_would_refuse() {
        let mut state = state_with_tiled(&[1]);
        state.current_mut().focused_pane = Some(1);
        state.current_mut().workspaces[0].focused_pane = Some(1);
        state.window_focused = false;
        {
            let pane = &mut state.current_mut().workspaces[0].panes[0];
            pane.activity.has_unseen_output = true;
            pane.terminal.finished_unseen = true;
        }

        assert!(!acknowledge_pane_if_attended(&mut state, 1));
        assert!(
            state.current().workspaces[0].panes[0]
                .terminal
                .finished_unseen
        );

        assert!(acknowledge_pane_input(&mut state, 1));
        let pane = &state.current().workspaces[0].panes[0];
        assert!(!pane.terminal.finished_unseen);
        assert!(!pane.activity.has_unseen_output);
    }

    /// Resolved across namespaces, so a popup or scratchpad pane answers input like any other. A
    /// lookup restricted to the active workspace refuses those panes without saying so.
    #[test]
    fn input_acknowledges_a_pane_outside_the_active_workspace() {
        let mut state = state_with_tiled(&[1]);
        let mut pane = Pane::new(
            9,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        pane.terminal.finished_unseen = true;
        state.scratch.panes.push(pane);

        assert!(acknowledge_pane_input(&mut state, 9));
        assert!(!state.scratch.panes[0].terminal.finished_unseen);
    }

    /// Switching to a marked workspace is the user asking which pane it was. The mark is the answer,
    /// so arriving must not consume it - only focusing the pane there does.
    #[test]
    fn switching_to_a_marked_workspace_shows_the_mark_without_answering_it() {
        let mut state = state_with_tiled(&[1]);
        let marked = 2;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        let mut pane = Pane::new(marked, 100, rect);
        pane.activity.has_unseen_output = true;
        pane.activity.bell = true;
        pane.terminal.finished_unseen = true;
        state.current_mut().workspaces[1].panes.push(pane);
        append_tiled_window(&mut state.current_mut().workspaces[1], marked);
        state.current_mut().workspaces[1].focused_pane = Some(marked);

        switch_workspace(&mut state, 1);
        assert_eq!(state.focused_pane(), Some(marked));
        let pane = &state.current().workspaces[1].panes[0];
        assert!(pane.activity.has_unseen_output);
        assert!(pane.activity.bell);
        assert!(
            pane.terminal.finished_unseen,
            "arriving on the workspace must leave the mark that identifies the pane"
        );

        focus_pane(&mut state, marked);
        let pane = &state.current().workspaces[1].panes[0];
        assert!(!pane.activity.has_unseen_output);
        assert!(!pane.activity.bell);
        assert!(!pane.terminal.finished_unseen);
    }

    /// Returning to the window is looking, not answering: the mark has to survive it and wait for a
    /// key, which is the whole point of showing it on the pane the user was already sitting in.
    #[test]
    fn window_focus_gain_preserves_attention_until_a_key_answers_it() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::AppRoot;
                use tui_lipan::{KeyCode, KeyEvent, KeyMods, TestBackend};

                let mut backend = TestBackend::new(AppRoot::default());
                let pane_id = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("fresh pane focus");
                let other_id = 2;
                let rect = backend.state().current().workspaces[0].panes[0].floating_rect;
                backend.state_mut().current_mut().workspaces[0]
                    .panes
                    .push(Pane::new(other_id, 100, rect));
                for pane in &mut backend.state_mut().current_mut().workspaces[0].panes {
                    pane.activity.has_unseen_output = true;
                    pane.activity.bell = true;
                    pane.terminal.finished_unseen = true;
                }

                backend
                    .set_window_focused(false)
                    .expect("lose host-window focus");
                backend
                    .set_window_focused(true)
                    .expect("gain host-window focus");

                {
                    let selected = backend.state().current().workspaces[0]
                        .panes
                        .iter()
                        .find(|pane| pane.id == pane_id)
                        .expect("selected pane");
                    assert!(selected.activity.has_unseen_output);
                    assert!(selected.activity.bell);
                    assert!(selected.terminal.finished_unseen);
                }

                backend
                    .dispatch(crate::Msg::PaneKey(
                        pane_id,
                        KeyEvent {
                            code: KeyCode::Char('x'),
                            mods: KeyMods::NONE,
                        },
                    ))
                    .expect("type into the focused pane");

                let panes = &backend.state().current().workspaces[0].panes;
                let selected = panes
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .expect("selected pane");
                assert!(!selected.activity.has_unseen_output);
                assert!(!selected.activity.bell);
                assert!(!selected.terminal.finished_unseen);
                let other = panes
                    .iter()
                    .find(|pane| pane.id == other_id)
                    .expect("other pane");
                assert!(other.activity.has_unseen_output);
                assert!(other.activity.bell);
                assert!(other.terminal.finished_unseen);
            })
            .expect("spawn focus lifecycle test")
            .join()
            .expect("focus lifecycle test completes");
    }

    /// A focus change driven from off-screen (control socket, layout reconciliation) is not the user
    /// looking, so it acknowledges nothing while the window is away — and still nothing when the
    /// window comes back. Only focusing the pane with the window present answers it.
    #[test]
    fn background_programmatic_focus_preserves_attention_until_the_pane_is_focused_in_view() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::AppRoot;
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                let target = 2;
                let rect = backend.state().current().workspaces[0].panes[0].floating_rect;
                let mut pane = Pane::new(target, 100, rect);
                pane.activity.has_unseen_output = true;
                pane.activity.bell = true;
                pane.terminal.finished_unseen = true;
                backend.state_mut().current_mut().workspaces[0]
                    .panes
                    .push(pane);
                let original = backend.state_mut().current_mut().workspaces[0]
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id != target)
                    .expect("original pane");
                original.activity.has_unseen_output = true;
                original.activity.bell = true;
                original.terminal.finished_unseen = true;

                backend
                    .set_window_focused(false)
                    .expect("lose host-window focus");
                focus_pane(backend.state_mut(), target);
                let target_pane = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id == target)
                    .expect("target pane");
                assert!(target_pane.activity.has_unseen_output);
                assert!(target_pane.activity.bell);
                assert!(target_pane.terminal.finished_unseen);

                backend
                    .set_window_focused(true)
                    .expect("gain host-window focus");
                {
                    let target_pane = backend.state().current().workspaces[0]
                        .panes
                        .iter()
                        .find(|pane| pane.id == target)
                        .expect("target pane");
                    assert!(target_pane.activity.has_unseen_output);
                    assert!(target_pane.activity.bell);
                    assert!(target_pane.terminal.finished_unseen);
                }

                // Clicking or navigating to the pane while the window is present: the deliberate
                // focus act that does answer it.
                focus_pane(backend.state_mut(), target);
                let target_pane = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id == target)
                    .expect("target pane");
                assert!(!target_pane.activity.has_unseen_output);
                assert!(!target_pane.activity.bell);
                assert!(!target_pane.terminal.finished_unseen);
                let original = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id != target)
                    .expect("original pane");
                assert!(original.activity.has_unseen_output);
                assert!(original.activity.bell);
                assert!(original.terminal.finished_unseen);
            })
            .expect("spawn background focus test")
            .join()
            .expect("background focus test completes");
    }

    fn state_with_floating(placements: &[(PaneId, FloatRect)]) -> State {
        let ids = placements.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut state = state_with_tiled(&ids);
        for pane in &mut state.current_mut().workspaces[0].panes {
            pane.floating = true;
            pane.floating_rect = placements
                .iter()
                .find_map(|(id, rect)| (*id == pane.id).then_some(*rect))
                .expect("pane placement");
        }
        state
    }

    fn assert_directional_sequence(
        placements: &[(PaneId, FloatRect)],
        start: PaneId,
        directions: &[Direction],
        expected: &[PaneId],
    ) {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_floating(placements);
        state.current_mut().focused_pane = Some(start);
        for (step, (&direction, &expected)) in directions.iter().zip(expected).enumerate() {
            let from = state.current().focused_pane;
            assert_eq!(
                focus_in_direction(&mut state, direction, viewport),
                Some(expected),
                "step {step}: {direction:?} from {from:?}"
            );
        }
    }

    #[test]
    fn cycle_focus_wraps_in_both_directions() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().focused_pane = Some(2);
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(3));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(1));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, false), Some(3));
    }

    /// A strip's cross axis has no tile ahead of any other, so the key finds nothing rather than
    /// bouncing between the two panes that happened to sort first. Tab still walks the full order.
    #[test]
    fn strip_cross_axis_focus_finds_no_tile() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        for (kind, cross) in [
            (LayoutKind::Columns, [Direction::Up, Direction::Down]),
            (LayoutKind::Rows, [Direction::Left, Direction::Right]),
            (LayoutKind::Scrollable, [Direction::Up, Direction::Down]),
        ] {
            let mut state = state_with_tiled(&[1, 2, 3, 4, 5]);
            state.current_mut().workspaces[0].layout_kind = kind;
            state.current_mut().focused_pane = Some(3);

            for direction in cross {
                assert_eq!(
                    focus_in_direction(&mut state, direction, viewport),
                    None,
                    "{kind:?} must not move focus {direction:?}"
                );
                assert_eq!(
                    state.current().focused_pane,
                    Some(3),
                    "{kind:?} must leave focus put on {direction:?}"
                );
            }
        }
    }

    /// The main axis is untouched: it still reaches every pane and still wraps at the edges.
    #[test]
    fn strip_main_axis_focus_still_reaches_every_pane() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        for (kind, forward) in [
            (LayoutKind::Columns, Direction::Right),
            (LayoutKind::Rows, Direction::Down),
        ] {
            let mut state = state_with_tiled(&[1, 2, 3, 4, 5]);
            state.current_mut().workspaces[0].layout_kind = kind;
            state.current_mut().focused_pane = Some(1);

            for expected in [2, 3, 4, 5, 1] {
                assert_eq!(
                    focus_in_direction(&mut state, forward, viewport),
                    Some(expected),
                    "{kind:?} main axis reaches every pane and wraps"
                );
            }
        }
    }

    /// Suppressing the wrap must not strand floating panes: one genuinely above the columns is
    /// still found, because that goes through ordinary directional scoring.
    #[test]
    fn strip_cross_axis_still_reaches_a_floating_pane_across_it() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Columns;
        state.current_mut().focused_pane = Some(2);

        // Park pane 3 as a floating window over the top strip of the canvas.
        let floating = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 6.0,
        };
        for pane in &mut state.current_mut().workspaces[0].panes {
            if pane.id == 3 {
                pane.floating = true;
                pane.floating_rect = floating;
            }
        }

        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(3),
            "a floating pane above the columns is still reachable"
        );
    }

    /// Monocle stacks every tile on one rect, so geometric scoring cannot separate the panes and
    /// used to bounce focus between two of them. Directional keys walk the whole stack instead.
    #[test]
    fn monocle_directional_focus_reaches_every_pane() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2, 3, 4, 5]);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Monocle;
        state.current_mut().focused_pane = Some(1);

        for expected in [2, 3, 4, 5, 1] {
            assert_eq!(
                focus_in_direction(&mut state, Direction::Right, viewport),
                Some(expected)
            );
        }
        assert_eq!(
            focus_in_direction(&mut state, Direction::Down, viewport),
            Some(2)
        );
        for expected in [1, 5, 4] {
            assert_eq!(
                focus_in_direction(&mut state, Direction::Left, viewport),
                Some(expected)
            );
        }
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(3)
        );
    }

    #[test]
    fn monocle_focus_without_wrap_stops_at_the_ends_of_the_stack() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Monocle;
        state.current_mut().focused_pane = Some(3);

        assert_eq!(
            focus_in_direction_no_wrap(&mut state, Direction::Right, viewport),
            None
        );
        assert_eq!(state.current().focused_pane, Some(3));
        for expected in [2, 1] {
            assert_eq!(
                focus_in_direction_no_wrap(&mut state, Direction::Left, viewport),
                Some(expected)
            );
        }
        assert_eq!(
            focus_in_direction_no_wrap(&mut state, Direction::Left, viewport),
            None
        );
    }

    /// A fullscreen pane hides every tile behind it, so moving focus off it would put the keyboard
    /// on a pane the user cannot see. Both movement styles stay put until fullscreen is toggled off.
    #[test]
    fn fullscreen_pins_focus_to_the_pane_covering_the_workspace() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().focused_pane = Some(2);
        state.current_mut().workspaces[0].panes[1].fullscreen = true;

        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            None
        );
        assert_eq!(
            focus_in_direction_no_wrap(&mut state, Direction::Right, viewport),
            None
        );
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), None);
        assert_eq!(state.current().focused_pane, Some(2));

        // Leaving fullscreen releases the lock.
        state.current_mut().workspaces[0].panes[1].fullscreen = false;
        assert!(cycle_focus_in_tiled_order(&mut state, true).is_some());
    }

    /// The lock follows the focused pane, not the workspace: a fullscreen pane in the background
    /// (its own workspace is not active, or the focus sits elsewhere) must not freeze navigation.
    #[test]
    fn a_fullscreen_pane_that_is_not_focused_does_not_lock_focus() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().focused_pane = Some(2);
        state.current_mut().workspaces[0].panes[0].fullscreen = true;

        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(3));
    }

    #[test]
    fn directional_focus_can_disable_edge_wrapping() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2]);
        state.current_mut().focused_pane = Some(2);

        assert_eq!(
            focus_in_direction_no_wrap(&mut state, Direction::Right, viewport),
            None
        );
        assert_eq!(state.current().focused_pane, Some(2));

        assert_eq!(
            focus_in_direction(&mut state, Direction::Right, viewport),
            Some(1)
        );
        assert_eq!(state.current().focused_pane, Some(1));
    }

    #[test]
    fn directional_focus_wraps_within_the_same_row_or_column() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let mut state = state_with_tiled(&[1, 2, 3, 4]);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Grid;

        state.current_mut().focused_pane = Some(2);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Right, viewport),
            Some(1)
        );

        state.current_mut().focused_pane = Some(1);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(2)
        );

        state.current_mut().focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Down, viewport),
            Some(1)
        );

        state.current_mut().focused_pane = Some(1);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(3)
        );
    }

    /// Layout shaped like:
    /// ```text
    ///   |   2
    /// 1 |-------
    ///   | 3 | 4
    /// ```
    /// Edge wrap from the spanning pane stays on the entry row: 4→3→1→4 and 3→4→1→3.
    #[test]
    fn directional_focus_escapes_peer_axis_sticky_when_neighbor_lies_ahead() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let layout = [
            (
                1,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 49.0,
                    h: 29.0,
                },
            ),
            (
                2,
                FloatRect {
                    x: 51.0,
                    y: 0.0,
                    w: 49.0,
                    h: 14.0,
                },
            ),
            (
                3,
                FloatRect {
                    x: 51.0,
                    y: 15.0,
                    w: 24.0,
                    h: 14.0,
                },
            ),
            (
                4,
                FloatRect {
                    x: 76.0,
                    y: 15.0,
                    w: 24.0,
                    h: 14.0,
                },
            ),
        ];

        assert_directional_sequence(&layout, 2, &[Direction::Right], &[1]);
        assert_directional_sequence(&layout, 4, &[Direction::Left], &[3]);
        assert_directional_sequence(
            &layout,
            4,
            &[
                Direction::Left,
                Direction::Left,
                Direction::Left,
                Direction::Left,
                Direction::Left,
            ],
            &[3, 1, 4, 3, 1],
        );
        assert_directional_sequence(
            &layout,
            3,
            &[
                Direction::Right,
                Direction::Right,
                Direction::Right,
                Direction::Right,
                Direction::Right,
            ],
            &[4, 1, 3, 4, 1],
        );
        assert_directional_sequence(&layout, 2, &[Direction::Right, Direction::Left], &[1, 2]);
        assert_directional_sequence(
            &layout,
            3,
            &[Direction::Right, Direction::Left, Direction::Left],
            &[4, 3, 1],
        );
        assert_directional_sequence(
            &layout,
            2,
            &[
                Direction::Down,
                Direction::Left,
                Direction::Right,
                Direction::Right,
            ],
            &[3, 1, 3, 4],
        );

        let mut state = state_with_floating(&layout);
        state.current_mut().focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(1)
        );

        // Framework focus sync re-affirms the current pane between keys; that must not erase the
        // entry-row band that makes the next Left wrap to 4 instead of 2.
        assert_directional_sequence(&layout, 4, &[Direction::Left, Direction::Left], &[3, 1]);
        let mut state = state_with_floating(&layout);
        state.current_mut().focused_pane = Some(4);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(3)
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(1)
        );
        focus_pane(&mut state, 1);
        assert!(
            state
                .active_workspace_ref()
                .last_directional_focus
                .is_some()
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(4)
        );
    }

    #[test]
    fn directional_focus_returns_to_the_pane_that_entered_a_spanning_pane() {
        let viewport = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        };
        let vertical = [
            (
                1,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 49.0,
                    h: 14.0,
                },
            ),
            (
                2,
                FloatRect {
                    x: 51.0,
                    y: 0.0,
                    w: 49.0,
                    h: 14.0,
                },
            ),
            (
                3,
                FloatRect {
                    x: 0.0,
                    y: 15.0,
                    w: 100.0,
                    h: 14.0,
                },
            ),
        ];
        for source in [1, 2] {
            let mut state = state_with_floating(&vertical);
            state.current_mut().focused_pane = Some(source);
            assert_eq!(
                focus_in_direction(&mut state, Direction::Down, viewport),
                Some(3)
            );
            assert_eq!(
                focus_in_direction(&mut state, Direction::Up, viewport),
                Some(source)
            );
        }
        let mut state = state_with_floating(&vertical);
        state.current_mut().focused_pane = Some(3);
        // Full-width spanning pane: Left has no orthogonal neighbor, so wrap hits the rightmost
        // top pane first, then steps left across that row.
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(2)
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(1)
        );
        // Reverse returns to the entry sibling; continuing wraps within the entry band.
        assert_directional_sequence(
            &vertical,
            2,
            &[Direction::Down, Direction::Up, Direction::Down],
            &[3, 2, 3],
        );
        assert_directional_sequence(&vertical, 2, &[Direction::Down, Direction::Down], &[3, 2]);

        let horizontal = [
            (
                1,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 49.0,
                    h: 14.0,
                },
            ),
            (
                2,
                FloatRect {
                    x: 0.0,
                    y: 15.0,
                    w: 49.0,
                    h: 14.0,
                },
            ),
            (
                3,
                FloatRect {
                    x: 51.0,
                    y: 0.0,
                    w: 49.0,
                    h: 29.0,
                },
            ),
        ];
        for source in [1, 2] {
            let mut state = state_with_floating(&horizontal);
            state.current_mut().focused_pane = Some(source);
            assert_eq!(
                focus_in_direction(&mut state, Direction::Right, viewport),
                Some(3)
            );
            assert_eq!(
                focus_in_direction(&mut state, Direction::Left, viewport),
                Some(source)
            );
        }
        let mut state = state_with_floating(&horizontal);
        state.current_mut().focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(2)
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(1)
        );
        assert_directional_sequence(
            &horizontal,
            2,
            &[Direction::Right, Direction::Left, Direction::Right],
            &[3, 2, 3],
        );
        assert_directional_sequence(
            &horizontal,
            2,
            &[Direction::Right, Direction::Right],
            &[3, 2],
        );
        assert_directional_sequence(
            &horizontal,
            3,
            &[Direction::Up, Direction::Down, Direction::Up],
            &[2, 1, 2],
        );
        assert_directional_sequence(
            &horizontal,
            3,
            &[Direction::Down, Direction::Up, Direction::Down],
            &[1, 2, 1],
        );
    }

    #[test]
    fn move_focused_pane_switches_to_target_workspace() {
        let mut state = state_with_tiled(&[1, 2]);
        state.current_mut().workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        append_tiled_window(&mut state.current_mut().workspaces[1], 3);
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(2);

        move_focused_to_workspace(&mut state, 1);

        assert_eq!(state.current().active_workspace, 1);
        assert_eq!(state.current().focused_pane, Some(2));
        assert!(
            state.current().workspaces[0]
                .panes
                .iter()
                .any(|pane| pane.id == 1)
        );
        assert!(
            !state.current().workspaces[0]
                .panes
                .iter()
                .any(|pane| pane.id == 2)
        );
        assert!(
            state.current().workspaces[1]
                .panes
                .iter()
                .any(|pane| pane.id == 2)
        );
    }

    #[test]
    fn move_focused_into_occupied_scrollable_reveals_at_right_edge() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::AppRoot;
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].panes.clear();
                    state.current_mut().workspaces[0]
                        .panes
                        .push(Pane::new(10, 100, rect));
                    append_tiled_window(&mut state.current_mut().workspaces[0], 10);
                    state.current_mut().workspaces[0].focused_pane = Some(10);

                    state.current_mut().workspaces[1].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[1].panes.clear();
                    for id in [1, 2, 3, 4] {
                        state.current_mut().workspaces[1]
                            .panes
                            .push(Pane::new(id, 100, rect));
                        append_tiled_window(&mut state.current_mut().workspaces[1], id);
                    }
                    state.current_mut().active_workspace = 1;
                    focus_pane(state, 4);
                    state.current_mut().active_workspace = 0;
                    state.current_mut().focused_pane = Some(10);
                    state.animation = GeometryAnimation::None;
                }
                backend.state_mut().current_mut().active_workspace = 1;
                backend.render();
                assert_eq!(
                    backend.state().current().workspaces[1].scrollable_anchor,
                    Some(4)
                );
                backend.state_mut().current_mut().active_workspace = 0;
                backend.state_mut().current_mut().focused_pane = Some(10);
                backend.state_mut().animation = GeometryAnimation::None;

                move_focused_to_workspace(backend.state_mut(), 1);
                assert_eq!(backend.state().current().active_workspace, 1);
                assert_eq!(backend.state().current().focused_pane, Some(10));
                assert_eq!(
                    backend.state().animation,
                    GeometryAnimation::None,
                    "cross-workspace move stays instant"
                );
                assert_eq!(
                    backend.state().current().workspaces[1].scrollable_reveal_edge,
                    ScrollableRevealEdge::Right
                );
                assert_eq!(
                    backend.state().current().workspaces[1].scrollable_anchor,
                    Some(10)
                );
                backend.render();
                assert_right_edge_aligned(backend.state(), 10);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn switching_workspaces_restores_each_workspaces_focused_pane() {
        let mut state = state_with_tiled(&[1, 2]);
        focus_pane(&mut state, 2);

        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        for id in [3, 4] {
            state.current_mut().workspaces[1]
                .panes
                .push(Pane::new(id, 100, rect));
            append_tiled_window(&mut state.current_mut().workspaces[1], id);
        }
        state.current_mut().workspaces[1].focused_pane = Some(4);

        switch_workspace(&mut state, 1);
        assert_eq!(state.current().focused_pane, Some(4));

        focus_pane(&mut state, 3);
        switch_workspace(&mut state, 0);
        assert_eq!(state.current().focused_pane, Some(2));

        switch_workspace(&mut state, 1);
        assert_eq!(state.current().focused_pane, Some(3));
    }

    #[test]
    fn relocate_active_workspace_swaps_content_when_target_is_occupied() {
        let mut state = state_with_tiled(&[1, 2]);
        state.current_mut().workspaces[0].name = Some("code".to_string());
        state.current_mut().workspaces[0].split_ratios[0] = 0.71;
        state.current_mut().workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        append_tiled_window(&mut state.current_mut().workspaces[1], 3);
        state.current_mut().workspaces[1].split_ratios[0] = 0.42;
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(2);

        relocate_active_workspace(&mut state, 1);

        assert_eq!(state.current().active_workspace, 1);
        assert_eq!(state.current().focused_pane, Some(2));
        assert_eq!(state.current().workspaces[0].panes.len(), 1);
        assert!(
            state.current().workspaces[0]
                .panes
                .iter()
                .any(|pane| pane.id == 3)
        );
        assert_eq!(state.current().workspaces[0].split_ratios[0], 0.42);
        assert_eq!(state.current().workspaces[1].name.as_deref(), Some("code"));
        assert_eq!(state.current().workspaces[1].split_ratios[0], 0.71);
        assert_eq!(state.current().workspaces[1].panes.len(), 2);
        assert!(
            state.current().workspaces[1]
                .panes
                .iter()
                .any(|pane| pane.id == 1)
        );
        assert!(
            state.current().workspaces[1]
                .panes
                .iter()
                .any(|pane| pane.id == 2)
        );
    }

    #[test]
    fn relocate_active_workspace_preserves_layout_on_empty_target() {
        let mut state = state_with_tiled(&[1, 2]);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Master;
        state.current_mut().workspaces[0].split_ratios[0] = 0.71;
        let source_tree = state.current().workspaces[0].tile_tree.clone();
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(2);

        relocate_active_workspace(&mut state, 2);

        assert_eq!(state.current().active_workspace, 2);
        assert_eq!(
            state.current().workspaces[2].layout_kind,
            LayoutKind::Master
        );
        assert_eq!(state.current().workspaces[2].split_ratios[0], 0.71);
        assert_eq!(state.current().workspaces[2].tile_tree, source_tree);
        assert_eq!(state.current().workspaces[2].panes.len(), 2);
        assert_eq!(state.current().workspaces[0].panes.len(), 0);
    }

    #[test]
    fn promote_swaps_focused_into_master_slot() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.current_mut().focused_pane = Some(3);
        assert!(promote_focused_to_master(&mut state));

        let mut leaves = Vec::new();
        collect_tree_leaves(
            state.current().workspaces[0].tile_tree.as_ref().unwrap(),
            &mut leaves,
        );
        assert_eq!(leaves.first(), Some(&3));
        assert_eq!(state.current().focused_pane, Some(3));

        // Already the master → no-op.
        assert!(!promote_focused_to_master(&mut state));
    }

    #[test]
    fn promote_scrollable_reveals_focused_under_preserved_non_focus_anchor() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2, 3, 4] {
                        let mut pane = Pane::new(id, 100, rect);
                        pane.scrollable_width = 0.30;
                        state.current_mut().workspaces[0].panes.push(pane);
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    focus_pane(state, 4);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                backend
                    .dispatch(Msg::FocusPane(3))
                    .expect("visible under right");
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(4)
                );
                assert!(promote_focused_to_master(backend.state_mut()));
                backend.state_mut().animation = GeometryAnimation::AxisChange;
                assert_eq!(backend.state().current().focused_pane, Some(3));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(3)
                );
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_reveal_edge,
                    ScrollableRevealEdge::Left
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                backend.render();
                assert_left_edge_aligned(backend.state(), 3);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    fn set_reported_status(pane: &mut Pane, value: &str) {
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: value.to_string(),
            reason: None,
            set_at: 1,
        });
    }

    #[test]
    fn next_blocked_pane_wraps_across_workspaces_and_skips_closing() {
        let mut state = state_with_tiled(&[1, 2]);
        set_reported_status(&mut state.current_mut().workspaces[0].panes[0], "blocked");
        set_reported_status(&mut state.current_mut().workspaces[0].panes[1], "BLOCKED");
        state.current_mut().workspaces[0].panes[1].closing = true;
        state.current_mut().workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        set_reported_status(&mut state.current_mut().workspaces[1].panes[0], " blocked ");

        state.current_mut().focused_pane = Some(1);
        assert_eq!(next_blocked_pane(&state), Some(3));
        state.current_mut().focused_pane = Some(3);
        assert_eq!(next_blocked_pane(&state), Some(1));
    }

    #[test]
    fn next_blocked_pane_handles_no_focus_and_no_other_match() {
        let mut state = state_with_tiled(&[1, 2]);
        set_reported_status(&mut state.current_mut().workspaces[0].panes[1], "blocked");
        state.current_mut().focused_pane = None;
        assert_eq!(next_blocked_pane(&state), Some(2));

        state.current_mut().focused_pane = Some(2);
        assert_eq!(next_blocked_pane(&state), None);
        state.current_mut().workspaces[0].panes[1]
            .terminal
            .reported_status = None;
        assert_eq!(next_blocked_pane(&state), None);
    }

    #[test]
    fn next_blocked_pane_uses_effective_status_and_skips_exited() {
        let mut state = state_with_tiled(&[1, 2]);
        {
            let panes = &mut state.current_mut().workspaces[0].panes;
            panes[0].terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
                state: crate::session::protocol::DetectedAgentState::Blocked,
            });
            panes[1].terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
                state: crate::session::protocol::DetectedAgentState::Blocked,
            });
            set_reported_status(&mut panes[0], "idle");
            set_reported_status(&mut panes[1], "working");
        }
        state.current_mut().focused_pane = None;
        assert_eq!(next_blocked_pane(&state), Some(1));

        state.current_mut().workspaces[0].panes[0].terminal.status =
            ManagedTerminalStatus::Exited(0);
        assert_eq!(next_blocked_pane(&state), None);
    }

    fn scrollable_state(ids: &[PaneId], focus: PaneId) -> State {
        let mut state = state_with_tiled(ids);
        state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
        focus_pane(&mut state, focus);
        state.animation = GeometryAnimation::None;
        state
    }

    #[test]
    fn scrollable_close_neighbor_uses_preclose_tree_order() {
        let mut state = state_with_tiled(&[20, 10, 30]);
        {
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.layout_kind = LayoutKind::Scrollable;
            workspace.tile_tree = crate::tiling::build_dwindle_tree(
                &[10, 30, 20],
                crate::state::SplitAxis::Horizontal,
                &[0.5, 0.5],
            );
            workspace.focused_pane = Some(30);
            workspace.scrollable_anchor = Some(30);
        }
        state.current_mut().focused_pane = Some(30);

        assert_eq!(state.current().workspaces[0].tiled_ids(), [10, 30, 20]);
        assert_eq!(
            scrollable_close_neighbor(&state.current().workspaces[0], 30),
            Some(20)
        );

        state.current_mut().focused_pane = Some(20);
        state.current_mut().workspaces[0].focused_pane = Some(20);
        assert_eq!(
            scrollable_close_neighbor(&state.current().workspaces[0], 20),
            Some(30)
        );

        state.current_mut().focused_pane = Some(10);
        state.current_mut().workspaces[0].focused_pane = Some(10);
        assert_eq!(
            scrollable_close_neighbor(&state.current().workspaces[0], 10),
            Some(30)
        );

        state.current_mut().focused_pane = Some(30);
        state.current_mut().workspaces[0].focused_pane = Some(30);
        assert_eq!(
            scrollable_close_neighbor(&state.current().workspaces[0], 10),
            Some(30)
        );
    }

    fn placement_x(state: &State, id: PaneId) -> f32 {
        placement_of(state, id).x
    }

    fn placement_of(state: &State, id: PaneId) -> FloatRect {
        let viewport = state.last_viewport.get().expect("viewport");
        let letterbox = crate::view::follower_letterbox_bounds(state, viewport);
        let local = state.canvas_bounds_from_terminal_viewport(viewport);
        let placements = workspace_target_rects_with_visible_bounds(
            &state.current().workspaces[state.current().active_workspace],
            letterbox,
            local,
            state.workspace_top_gap(),
            state.tile_gap(),
        );
        placement_for(&placements, id).expect("placement")
    }

    fn visible_tile(state: &State) -> FloatRect {
        let viewport = state.last_viewport.get().expect("viewport");
        let letterbox = crate::view::follower_letterbox_bounds(state, viewport);
        let local = state.canvas_bounds_from_terminal_viewport(viewport);
        let top_gap = state.workspace_top_gap();
        let tile_letterbox = workspace_tile_bounds(letterbox, top_gap);
        let tile_local = workspace_tile_bounds(local, top_gap);
        let left = tile_letterbox.x.max(tile_local.x);
        let right = (tile_letterbox.x + tile_letterbox.w).min(tile_local.x + tile_local.w);
        FloatRect {
            x: left,
            y: tile_local.y,
            w: (right - left).max(0.0),
            h: tile_local.h,
        }
    }

    fn assert_fully_in_visible(state: &State, id: PaneId) {
        let visible = visible_tile(state);
        let rect = placement_of(state, id);
        assert!(
            rect.x >= visible.x - 0.5 && rect.x + rect.w <= visible.x + visible.w + 0.5,
            "pane {id} {rect:?} not inside visible {visible:?}"
        );
    }

    fn assert_left_edge_aligned(state: &State, id: PaneId) {
        let visible = visible_tile(state);
        let rect = placement_of(state, id);
        assert!(
            (rect.x - visible.x).abs() < 0.5,
            "pane {id} left edge {rect:?} must meet visible left {visible:?}"
        );
    }

    fn assert_right_edge_aligned(state: &State, id: PaneId) {
        let visible = visible_tile(state);
        let rect = placement_of(state, id);
        assert!(
            (rect.x + rect.w - (visible.x + visible.w)).abs() < 0.5,
            "pane {id} right edge {rect:?} must meet visible right {visible:?}"
        );
    }

    #[test]
    fn scrollable_focus_pane_arms_axis_change_only_when_anchor_moves() {
        let mut state = scrollable_state(&[1, 2, 3, 4], 1);
        assert_eq!(state.animation, GeometryAnimation::None);
        focus_pane(&mut state, 1);
        assert_eq!(
            state.animation,
            GeometryAnimation::None,
            "reaffirming the focused/anchored pane must not re-arm"
        );

        // No last_viewport yet: fall back to scroll-to-target.
        focus_pane(&mut state, 4);
        assert_eq!(state.animation, GeometryAnimation::AxisChange);
        assert_eq!(state.current().workspaces[0].scrollable_anchor, Some(4));

        state.animation = GeometryAnimation::None;
        focus_pane(&mut state, 4);
        assert_eq!(state.animation, GeometryAnimation::None);

        let anchor = state.current().workspaces[0].scrollable_anchor;
        let mut floating = Pane::new(
            99,
            100,
            FloatRect {
                x: 5.0,
                y: 5.0,
                w: 20.0,
                h: 10.0,
            },
        );
        floating.floating = true;
        floating.opening = false;
        state.current_mut().workspaces[0].panes.push(floating);
        state.animation = GeometryAnimation::None;
        focus_pane(&mut state, 99);
        assert_eq!(state.current().focused_pane, Some(99));
        assert_eq!(
            state.current().workspaces[0].scrollable_anchor,
            anchor,
            "floating focus must not rewrite the tiled Scrollable anchor"
        );
        assert_eq!(
            state.animation,
            GeometryAnimation::None,
            "floating focus must not arm strip animation"
        );

        focus_pane(&mut state, 999);
        assert_eq!(state.animation, GeometryAnimation::None);
        assert_eq!(state.current().focused_pane, Some(99));
    }

    #[test]
    fn scrollable_focus_preserves_viewport_for_fully_visible_targets() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    state.current_mut().workspaces[0].tile_tree = None;
                    for id in [1, 2, 3, 4] {
                        state.current_mut().workspaces[0]
                            .panes
                            .push(Pane::new(id, 100, rect));
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();

                let before = placement_x(backend.state(), 1);
                backend.dispatch(Msg::FocusPane(2)).expect("focus visible");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(1),
                    "fully visible focus materializes/preserves the pre-focus effective anchor"
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!(
                    (placement_x(backend.state(), 1) - before).abs() < 1e-5,
                    "fully visible focus must not shift placements"
                );

                let before = placement_x(backend.state(), 1);
                backend.dispatch(Msg::FocusPane(3)).expect("focus clipped");
                assert_eq!(backend.state().current().focused_pane, Some(3));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(3)
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                assert!(
                    (placement_x(backend.state(), 1) - before).abs() > 0.5,
                    "partly/outside focus must scroll the strip"
                );
                assert_right_edge_aligned(backend.state(), 3);
                let sibling = &backend.state().current().workspaces[0].panes[0];
                let cfg =
                    AppRoot::geometry_transition_for_pane(backend.state(), sibling, false, None);
                assert!(cfg.duration > std::time::Duration::ZERO);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    /// Scrollable reveal has to read the workspace that is actually on top and measure it against
    /// the box it occupies. Reading the hidden attachment workspace instead made every scratch
    /// focus fall through to the no-decision path, which anchors the target to the left edge - so
    /// the strip scrolled on every focus even when the target was already fully visible.
    #[test]
    fn scrollable_focus_in_the_scratchpad_preserves_a_fully_visible_target() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    state.scratch.layout_kind = LayoutKind::Scrollable;
                    for id in [1, 2, 3, 4] {
                        let mut pane = Pane::new(id, 100, FloatRect::default());
                        pane.opening = false;
                        state.scratch.panes.push(pane);
                        append_tiled_window(&mut state.scratch, id);
                    }
                    state.scratch_visible = true;
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();

                let placement_x = |state: &State, id: PaneId| {
                    let viewport = state.last_viewport.get().expect("viewport");
                    let bounds = state.layout_bounds(viewport);
                    let placements = workspace_target_rects_with_visible_bounds(
                        &state.scratch,
                        bounds,
                        bounds,
                        state.layout_top_gap(),
                        state.tile_gap(),
                    );
                    placement_for(&placements, id).expect("placement").x
                };

                let before = placement_x(backend.state(), 1);
                backend.dispatch(Msg::FocusPane(2)).expect("focus visible");
                assert_eq!(backend.state().scratch.focused_pane, Some(2));
                assert_eq!(
                    backend.state().scratch.scrollable_anchor,
                    Some(1),
                    "a fully visible target keeps the strip where it is"
                );
                assert!(
                    (placement_x(backend.state(), 1) - before).abs() < 1e-5,
                    "fully visible focus must not shift placements"
                );

                // A target off the right edge still scrolls, exactly as in a workspace.
                backend.dispatch(Msg::FocusPane(4)).expect("focus clipped");
                assert_eq!(backend.state().scratch.scrollable_anchor, Some(4));
                assert!((placement_x(backend.state(), 1) - before).abs() > 0.5);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn scrollable_focus_from_right_scroll_preserves_visible_and_scrolls_clipped() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2, 3, 4] {
                        let mut pane = Pane::new(id, 100, rect);
                        // Narrow enough that panes 2 and 3 stay fully visible under a right
                        // anchor on 4, while pane 1 stays left-clipped.
                        pane.scrollable_width = 0.30;
                        state.current_mut().workspaces[0].panes.push(pane);
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    focus_pane(state, 4);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(4)
                );
                {
                    let visible = visible_tile(backend.state());
                    let p2 = placement_of(backend.state(), 2);
                    let p1 = placement_of(backend.state(), 1);
                    assert!(
                        p2.x >= visible.x - 0.5 && p2.x + p2.w <= visible.x + visible.w + 0.5,
                        "precondition: pane 2 fully visible"
                    );
                    assert!(p1.x < visible.x - 0.5, "precondition: pane 1 left-clipped");
                }

                let before = placement_x(backend.state(), 4);
                backend.dispatch(Msg::FocusPane(3)).expect("focus visible");
                assert_eq!(backend.state().current().focused_pane, Some(3));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(4)
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!((placement_x(backend.state(), 4) - before).abs() < 1e-5);

                // Adjacent still-visible pane to the left of 3 must not move the strip.
                let before = placement_x(backend.state(), 4);
                backend
                    .dispatch(Msg::FocusPane(2))
                    .expect("focus adjacent visible");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(4),
                    "fully visible leftward focus keeps the right-scrolled anchor"
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!((placement_x(backend.state(), 4) - before).abs() < 1e-5);

                let before = placement_x(backend.state(), 4);
                backend
                    .dispatch(Msg::FocusPane(1))
                    .expect("focus left-clipped");
                assert_eq!(backend.state().current().focused_pane, Some(1));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(1)
                );
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_reveal_edge,
                    ScrollableRevealEdge::Left
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                assert!((placement_x(backend.state(), 4) - before).abs() > 0.5);
                assert_left_edge_aligned(backend.state(), 1);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn scrollable_focus_materializes_stale_none_anchor_without_scrolling() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2] {
                        state.current_mut().workspaces[0]
                            .panes
                            .push(Pane::new(id, 100, rect));
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    state.current_mut().workspaces[0].focused_pane = Some(1);
                    state.current_mut().focused_pane = Some(1);
                    state.current_mut().workspaces[0].scrollable_anchor = None;
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                let before = placement_x(backend.state(), 1);
                backend.dispatch(Msg::FocusPane(2)).expect("focus visible");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(1),
                    "None/stale stored anchor must materialize the pre-focus effective fallback"
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!((placement_x(backend.state(), 1) - before).abs() < 1e-5);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn scrollable_follower_letterbox_uses_local_visible_intersection() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::state::SharedSessionState;
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                // Local viewport narrower than the controller canvas → letterbox overhang.
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 50,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().session_attached = true;
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2);
                    shared.canonical_canvas = Some((100, 28));
                    state.current_mut().shared = Some(shared);
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2] {
                        let mut pane = Pane::new(id, 100, rect);
                        // Wide enough that pane 2 sits inside the canonical tile but past the
                        // local visible edge when anchored on pane 1.
                        pane.scrollable_width = 0.45;
                        state.current_mut().workspaces[0].panes.push(pane);
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                let before = placement_of(backend.state(), 2);
                assert!(
                    before.x + before.w
                        > visible_tile(backend.state()).x + visible_tile(backend.state()).w,
                    "precondition: pane 2 must start locally clipped"
                );

                backend
                    .dispatch(Msg::FocusPane(2))
                    .expect("focus locally clipped");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(2),
                    "local letterbox clip must not count as fully visible"
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                let after = placement_of(backend.state(), 2);
                assert!(
                    (after.x - before.x).abs() > 0.5,
                    "follower scroll must actually move placements"
                );
                assert_fully_in_visible(backend.state(), 2);
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);

                // First pane starts left of the local viewport under canonical letterbox; focusing
                // it must use negative scroll range to reveal it.
                {
                    let state = backend.state_mut();
                    focus_pane(state, 2);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                let before_first = placement_of(backend.state(), 1);
                assert!(
                    before_first.x < visible_tile(backend.state()).x - 0.5,
                    "precondition: first pane left-clipped by letterbox"
                );
                backend
                    .dispatch(Msg::FocusPane(1))
                    .expect("reveal first pane");
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(1)
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                assert_fully_in_visible(backend.state(), 1);
                assert!((placement_of(backend.state(), 1).x - before_first.x).abs() > 0.5);

                // Narrower panes with a third column so 1–2 stay preferred (no two-pane flex)
                // and both fit inside the local visible interval under a left anchor.
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    for pane in &mut state.current_mut().workspaces[0].panes {
                        pane.scrollable_width = 0.20;
                    }
                    let mut third = Pane::new(3, 100, rect);
                    third.scrollable_width = 0.20;
                    state.current_mut().workspaces[0].panes.push(third);
                    append_tiled_window(&mut state.current_mut().workspaces[0], 3);
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                let before = placement_x(backend.state(), 1);
                backend
                    .dispatch(Msg::FocusPane(2))
                    .expect("focus locally visible");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(1)
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
                assert!((placement_x(backend.state(), 1) - before).abs() < 1e-5);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn scrollable_focus_reveals_partly_left_clipped_local_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2, 3, 4, 5] {
                        let mut pane = Pane::new(id, 100, rect);
                        pane.scrollable_width = 0.35;
                        state.current_mut().workspaces[0].panes.push(pane);
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    // Anchor near the right so pane 2 straddles the left visible edge.
                    focus_pane(state, 4);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                let visible = visible_tile(backend.state());
                let pane2 = placement_of(backend.state(), 2);
                assert!(
                    pane2.x < visible.x - 0.5 && pane2.x + pane2.w > visible.x + 0.5,
                    "precondition: pane 2 partly left-clipped ({pane2:?} vs {visible:?})"
                );
                let before = pane2.x;
                backend.dispatch(Msg::FocusPane(2)).expect("focus partial");
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_anchor,
                    Some(2)
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                assert!((placement_of(backend.state(), 2).x - before).abs() > 0.5);
                assert_fully_in_visible(backend.state(), 2);
                assert_left_edge_aligned(backend.state(), 2);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn scrollable_focus_wide_pane_reaffirm_keeps_reveal_edge() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[0].panes.clear();
                    for id in [1, 2] {
                        let mut pane = Pane::new(id, 100, rect);
                        // Wider than the local viewport so both edges stay clipped.
                        pane.scrollable_width = 0.80;
                        state.current_mut().workspaces[0].panes.push(pane);
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();
                backend
                    .dispatch(Msg::FocusPane(2))
                    .expect("focus wide rightward");
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_reveal_edge,
                    ScrollableRevealEdge::Right
                );
                assert_eq!(backend.state().animation, GeometryAnimation::AxisChange);
                backend.state_mut().animation = GeometryAnimation::None;
                backend.dispatch(Msg::FocusPane(2)).expect("reaffirm");
                assert_eq!(
                    backend.state().current().workspaces[0].scrollable_reveal_edge,
                    ScrollableRevealEdge::Right
                );
                assert_eq!(
                    backend.state().animation,
                    GeometryAnimation::None,
                    "reaffirming a wide anchored pane must not re-arm"
                );
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    #[test]
    fn focus_pane_anywhere_cross_workspace_finishes_instant() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use std::sync::mpsc;

                use crate::control::{ControlCommand, ControlEnvelope, ControlRequest};
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
                    for id in [2, 3, 4] {
                        state.current_mut().workspaces[0]
                            .panes
                            .push(Pane::new(id, 100, rect));
                        append_tiled_window(&mut state.current_mut().workspaces[0], id);
                    }
                    state.current_mut().workspaces[1].layout_kind = LayoutKind::Scrollable;
                    state.current_mut().workspaces[1]
                        .panes
                        .push(Pane::new(10, 100, rect));
                    append_tiled_window(&mut state.current_mut().workspaces[1], 10);
                    state.current_mut().workspaces[1]
                        .panes
                        .push(Pane::new(11, 100, rect));
                    append_tiled_window(&mut state.current_mut().workspaces[1], 11);
                    focus_pane(state, 1);
                    state.animation = GeometryAnimation::None;
                }
                backend.render();

                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::Focus { target: 10 },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("cross-workspace focus");
                assert!(response.recv().unwrap().ok);
                assert_eq!(backend.state().current().active_workspace, 1);
                assert_eq!(backend.state().current().focused_pane, Some(10));
                assert_eq!(
                    backend.state().animation,
                    GeometryAnimation::None,
                    "cross-workspace focus_pane_anywhere must finish instant"
                );

                backend.state_mut().animation = GeometryAnimation::None;
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::Focus { target: 11 },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("same-workspace focus");
                assert!(response.recv().unwrap().ok);
                assert_eq!(backend.state().current().active_workspace, 1);
                assert_eq!(backend.state().current().focused_pane, Some(11));
                assert_eq!(
                    backend.state().current().workspaces[1].scrollable_anchor,
                    Some(10),
                    "same-workspace fully visible focus preserves the viewport"
                );
                assert_eq!(backend.state().animation, GeometryAnimation::None);
            })
            .expect("spawn")
            .join()
            .expect("join");
    }
}
