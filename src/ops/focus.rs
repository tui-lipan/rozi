use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{closest_pane_to_rect, directional_score};
use crate::layout::{placement_for, workspace_target_rects};
use crate::state::{Direction, DirectionalFocusHint, Pane, PaneId, State, Workspace};
use crate::tiling::{self, append_tiled_window, remove_tiled_window};
use crate::view;

pub(crate) fn split_axis_for_direction(direction: Direction) -> crate::state::SplitAxis {
    match direction {
        Direction::Left | Direction::Right => crate::state::SplitAxis::Horizontal,
        Direction::Up | Direction::Down => crate::state::SplitAxis::Vertical,
    }
}

pub(crate) fn active_pane_is_fullscreen(state: &State, id: PaneId) -> bool {
    state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing && pane.fullscreen)
}

/// Focus a live workspace pane regardless of which workspace currently owns the view.
pub(crate) fn focus_pane_anywhere(ctx: &mut Context<HyprmuxApp>, target: PaneId) -> bool {
    let Some(workspace_index) = ctx.state.workspaces.iter().position(|workspace| {
        workspace
            .panes
            .iter()
            .any(|pane| pane.id == target && !pane.closing)
    }) else {
        return false;
    };
    ctx.state.active_workspace = workspace_index;
    focus_pane(&mut ctx.state, target);
    if let Some(pane) = crate::pane_lifecycle::find_pane_mut(&mut ctx.state, target) {
        pane.activity.has_unseen_output = false;
    }
    request_pane_focus(ctx, target);
    true
}

/// Find the next blocked live workspace pane in deterministic workspace/pane order. The focused
/// pane is skipped, so a sole blocked focus is a no-op; with no valid focus, the first blocked pane
/// is selected.
pub(crate) fn next_blocked_pane(state: &State) -> Option<PaneId> {
    let panes = state
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .filter(|pane| {
            !pane.closing
                && pane.id != crate::state::SCRATCH_PANE_ID
                && pane.id != crate::state::POPUP_PANE_ID
        })
        .collect::<Vec<_>>();
    if panes.is_empty() {
        return None;
    }
    let start = state
        .focused_pane
        .and_then(|focused| panes.iter().position(|pane| pane.id == focused))
        .map_or(0, |index| index + 1);
    (0..panes.len())
        .map(|offset| &panes[(start + offset) % panes.len()])
        .find(|pane| {
            Some(pane.id) != state.focused_pane
                && pane
                    .terminal
                    .reported_status
                    .as_ref()
                    .is_some_and(|status| status.value.trim().eq_ignore_ascii_case("blocked"))
        })
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
    let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
    let workspace = &state.workspaces[state.active_workspace];
    let placements = workspace_target_rects(
        workspace,
        bounds,
        state.workspace_top_gap(),
        state.tile_gap(),
    );
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
        state.focused_pane = None;
        return None;
    }

    let focused = state.focused_pane.unwrap_or(candidates[0].id);
    let Some(current) = candidates.iter().find(|candidate| candidate.id == focused) else {
        let id = candidates[0].id;
        focus_pane(state, id);
        return Some(id);
    };
    let geometric = candidates
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            directional_score(current.rect, candidate.rect, direction)
                .map(|score| (candidate.id, score))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(id, _)| id)
        .or_else(|| {
            wrap.then(|| wrapped_focus_id(&candidates, current, direction))
                .flatten()
        });
    let remembered = remembered_focus_target(workspace, &candidates, focused, direction);
    let next = prefer_aligned_focus_target(&candidates, current, direction, remembered, geometric);

    if let Some(next_id) = next {
        focus_pane(state, next_id);
        state.workspaces[state.active_workspace].last_directional_focus =
            Some(DirectionalFocusHint {
                pane: next_id,
                entry_direction: direction,
                target: focused,
            });
        Some(next_id)
    } else {
        None
    }
}

fn remembered_focus_target(
    workspace: &Workspace,
    candidates: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let hint = workspace.last_directional_focus?;
    (hint.pane == focused
        && split_axis_for_direction(direction) == split_axis_for_direction(hint.entry_direction)
        && candidates
            .iter()
            .any(|candidate| candidate.id == hint.target))
    .then_some(hint.target)
}

fn prefer_aligned_focus_target(
    candidates: &[tiling::PanePlacement],
    current: &tiling::PanePlacement,
    direction: Direction,
    remembered: Option<PaneId>,
    geometric: Option<PaneId>,
) -> Option<PaneId> {
    let (Some(remembered), Some(geometric)) = (remembered, geometric) else {
        return geometric;
    };
    let remembered_rect = candidates
        .iter()
        .find(|candidate| candidate.id == remembered)?
        .rect;
    let geometric_rect = candidates
        .iter()
        .find(|candidate| candidate.id == geometric)?
        .rect;
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
) -> Option<PaneId> {
    candidates
        .iter()
        .filter(|candidate| candidate.id != current.id)
        .min_by(|a, b| compare_wrap_candidates(current.rect, a.rect, b.rect, direction))
        .map(|candidate| candidate.id)
}

fn compare_wrap_candidates(
    current: FloatRect,
    a: FloatRect,
    b: FloatRect,
    direction: Direction,
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
        let cross_gap = interval_gap(current_start, current_end, candidate_start, candidate_end);
        let center_offset =
            ((candidate_start + candidate_end) - (current_start + current_end)).abs();
        (cross_gap, opposite_edge, center_offset)
    };
    let a = rank(a);
    let b = rank(b);
    a.0.total_cmp(&b.0)
        .then_with(|| a.1.total_cmp(&b.1))
        .then_with(|| a.2.total_cmp(&b.2))
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

fn interval_gap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    if b_end < a_start {
        a_start - b_end
    } else if b_start > a_end {
        b_start - a_end
    } else {
        0.0
    }
}

/// Move focus to the next/previous tiled pane in `tiled_ids()` order, wrapping around. If
/// the current focus is floating (not part of the tiled order) it falls back to the first
/// tiled pane. Returns the newly focused id, or `None` when there are no tiled panes.
pub(crate) fn cycle_focus_in_tiled_order(state: &mut State, forward: bool) -> Option<PaneId> {
    let ids = state.workspaces[state.active_workspace].tiled_ids();
    if ids.is_empty() {
        return None;
    }
    let next = match state
        .focused_pane
        .and_then(|id| ids.iter().position(|c| *c == id))
    {
        Some(index) => {
            if forward {
                (index + 1) % ids.len()
            } else {
                index.checked_sub(1).unwrap_or(ids.len() - 1)
            }
        }
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
    let Some(focused) = state.focused_pane else {
        return false;
    };
    if active_pane_is_fullscreen(state, focused) {
        return false;
    }
    let workspace = &mut state.workspaces[state.active_workspace];
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
    if crate::tiling::swap_tree_leaves(tree, focused, master) {
        state.focused_pane = Some(focused);
        state.workspaces[state.active_workspace].focused_pane = Some(focused);
        state.workspaces[state.active_workspace].last_move_swap = None;
        state.workspaces[state.active_workspace].last_directional_focus = None;
        true
    } else {
        false
    }
}

pub(crate) fn switch_workspace(state: &mut State, index: usize) {
    if index >= state.workspaces.len() {
        return;
    }
    let previous = state.active_workspace;
    state.active_workspace = index;
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
    clear_focused_activity(state);
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
    if target_index >= state.workspaces.len() {
        return;
    }
    let source_index = state.active_workspace;
    let Some(focused) = state.focused_pane else {
        return;
    };
    if source_index == target_index {
        return;
    }

    let Some(position) = state.workspaces[source_index]
        .panes
        .iter()
        .position(|pane| pane.id == focused)
    else {
        choose_fallback_focus(state);
        return;
    };

    let mut pane = state.workspaces[source_index].panes.remove(position);
    let tiled = !pane.floating;
    if tiled {
        remove_tiled_window(&mut state.workspaces[source_index], pane.id);
    }
    pane.opening = false;
    pane.closing = false;

    choose_fallback_focus(state);

    if tiled {
        append_tiled_window(&mut state.workspaces[target_index], pane.id);
    }
    state.workspaces[target_index].panes.push(pane);

    state.active_workspace = target_index;
    state.focused_pane = Some(focused);
    state.workspaces[target_index].focused_pane = Some(focused);
    clear_focused_activity(state);
    state.animation = GeometryAnimation::None;
    emit_workspace_switched(state, target_index);
}

/// Move every pane from the active workspace into `target_index`, carry the source workspace
/// name and layout over when set, then switch to the target workspace and keep focus on the
/// previously focused pane when it moved with the batch. An empty target slot receives the
/// source content wholesale; a occupied target swaps content with the source so both layouts
/// stay intact.
pub(crate) fn relocate_active_workspace(state: &mut State, target_index: usize) {
    if target_index >= state.workspaces.len() {
        return;
    }
    let source_index = state.active_workspace;
    if source_index == target_index {
        return;
    }

    let previous_focus = state.focused_pane;
    let source_empty = workspace_is_empty(&state.workspaces[source_index]);
    if source_empty {
        state.active_workspace = target_index;
        choose_fallback_focus(state);
        state.animation = GeometryAnimation::None;
        emit_workspace_switched(state, target_index);
        return;
    }

    let target_empty = workspace_is_empty(&state.workspaces[target_index]);
    if target_empty {
        transfer_workspace_content(state, source_index, target_index);
    } else {
        swap_workspace_content(state, source_index, target_index);
    }

    let target = &mut state.workspaces[target_index];
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

    state.active_workspace = target_index;
    state.focused_pane = target.focused_pane;
    clear_focused_activity(state);
    state.animation = GeometryAnimation::None;
    emit_workspace_switched(state, target_index);
}

fn workspace_is_empty(workspace: &Workspace) -> bool {
    !workspace.panes.iter().any(|pane| !pane.closing)
}

fn swap_workspace_content(state: &mut State, source_index: usize, target_index: usize) {
    if source_index < target_index {
        let (left, right) = state.workspaces.split_at_mut(target_index);
        swap_workspace_fields(&mut left[source_index], &mut right[0]);
    } else {
        let (left, right) = state.workspaces.split_at_mut(source_index);
        swap_workspace_fields(&mut right[0], &mut left[target_index]);
    }
}

fn transfer_workspace_content(state: &mut State, source_index: usize, target_index: usize) {
    if source_index < target_index {
        let (left, right) = state.workspaces.split_at_mut(target_index);
        transfer_workspace_fields(&mut left[source_index], &mut right[0]);
        left[source_index] = Workspace::new(source_index);
    } else {
        let (left, right) = state.workspaces.split_at_mut(source_index);
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
pub(crate) fn hover_focus_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
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
    if ctx.state.focused_pane == Some(id) {
        return Update::none();
    }
    let focusable = ctx.state.workspaces[ctx.state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing);
    if !focusable {
        return Update::none();
    }
    focus_pane(&mut ctx.state, id);
    if let Some(pane) = crate::pane_lifecycle::find_pane_mut(&mut ctx.state, id) {
        pane.activity.has_unseen_output = false;
        pane.activity.bell = false;
    }
    request_pane_focus(ctx, id);
    Update::full()
}

pub(crate) fn focus_pane(state: &mut State, id: PaneId) {
    let previous = state.focused_pane;
    state.workspaces[state.active_workspace].last_directional_focus = None;
    if let Some(pane) = state.workspaces[state.active_workspace]
        .panes
        .iter_mut()
        .find(|pane| pane.id == id && !pane.closing)
    {
        pane.activity.has_unseen_output = false;
        pane.activity.bell = false;
        state.focused_pane = Some(id);
        state.workspaces[state.active_workspace].focused_pane = Some(id);
    }
    if previous != state.focused_pane && state.focused_pane == Some(id) {
        crate::events::emit(
            state,
            crate::events::Event::new(
                crate::events::EventKind::FocusChanged,
                vec![("pane", id.to_string())],
            ),
        );
    }
}

fn clear_focused_activity(state: &mut State) {
    let Some(id) = state.focused_pane else {
        return;
    };
    if let Some(pane) = state.workspaces[state.active_workspace]
        .panes
        .iter_mut()
        .find(|pane| pane.id == id && !pane.closing)
    {
        pane.activity.has_unseen_output = false;
        pane.activity.bell = false;
    }
}

pub(crate) fn choose_fallback_focus(state: &mut State) {
    choose_fallback_focus_near(state, state.focused_pane, None);
}

pub(crate) fn choose_fallback_focus_near(
    state: &mut State,
    reference_id: Option<PaneId>,
    reference_rect: Option<FloatRect>,
) {
    let workspace_index = state.active_workspace;
    let workspace = &state.workspaces[workspace_index];

    if let Some(focused) = state.focused_pane
        && workspace
            .panes
            .iter()
            .any(|pane| pane.id == focused && !pane.closing)
    {
        state.workspaces[workspace_index].focused_pane = Some(focused);
        return;
    }

    let focus = reference_id
        .and_then(|reference_id| {
            focus_near_pane_in_workspace(state, workspace, reference_id, reference_rect)
        })
        .or_else(|| first_visible_pane(workspace));

    state.workspaces[workspace_index].focused_pane = focus;
    state.focused_pane = focus;
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

pub(crate) fn visible_pane_placements(
    state: &State,
    workspace: &Workspace,
) -> Vec<(PaneId, FloatRect)> {
    if let Some(viewport) = state.last_viewport.get() {
        let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
        let placements = workspace_target_rects(
            workspace,
            bounds,
            state.workspace_top_gap(),
            state.tile_gap(),
        );
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
        let bounds = state.canvas_bounds_from_terminal_viewport(viewport);
        let placements = workspace_target_rects(
            workspace,
            bounds,
            state.workspace_top_gap(),
            state.tile_gap(),
        );
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
    state.workspaces[state.active_workspace]
        .panes
        .iter_mut()
        .find(|pane| pane.id == id)
}

pub(crate) fn request_pane_focus(ctx: &mut Context<HyprmuxApp>, id: PaneId) {
    if crate::pane_lifecycle::find_pane_mut(&mut ctx.state, id)
        .is_some_and(|pane| pane.terminal_active && !pane.opening && !pane.closing)
    {
        focus_key(ctx, view::pane_terminal_key(id));
    }
}

pub(crate) fn request_current_pane_focus(ctx: &mut Context<HyprmuxApp>) {
    if let Some(id) = ctx.state.focused_pane {
        request_pane_focus(ctx, id);
    }
}

/// Every "give focus to something that is not the sidebar" goes through here.
///
/// The sidebar body lives in a `FocusScope::Exclude` subtree, and an excluded subtree is invisible
/// to `has_focus_within_key` — so hyprmux cannot ask the framework whether the sidebar still holds
/// the keyboard. `sidebar.focused` is therefore app-owned intent, and this is the one place that
/// has to retract it.
fn focus_key(ctx: &mut Context<HyprmuxApp>, key: impl Into<tui_lipan::Key>) {
    ctx.state.sidebar.focused = false;
    ctx.request_focus(key);
}

pub(crate) fn request_search_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::search_input_key());
}

pub(crate) fn request_rename_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::rename_input_key());
}

pub(crate) fn request_rename_session_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::rename_session_input_key());
}

pub(crate) fn request_save_profile_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::save_profile_key());
}

pub(crate) fn request_profile_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::profile_picker_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::theme_picker_key());
}

pub(crate) fn request_palette_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::palette_key());
}

pub(crate) fn request_session_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    focus_key(ctx, view::session_picker_key());
}

pub(crate) fn total_visible_panes(state: &State) -> usize {
    state
        .workspaces
        .iter()
        .map(|workspace| workspace.panes.iter().filter(|pane| !pane.closing).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use crate::state::{LayoutKind, Pane};
    use crate::tiling::{append_tiled_window, collect_tree_leaves};
    use tui_lipan::prelude::Theme;

    fn state_with_tiled(ids: &[PaneId]) -> State {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        // State::new seeds pane 1; clear and rebuild a deterministic tiled set.
        state.workspaces[0].panes.clear();
        state.workspaces[0].tile_tree = None;
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        for &id in ids {
            state.workspaces[0].panes.push(Pane::new(id, 100, rect));
            append_tiled_window(&mut state.workspaces[0], id);
        }
        state.next_pane_id = ids.iter().copied().max().unwrap_or(0) + 1;
        state
    }

    fn state_with_floating(placements: &[(PaneId, FloatRect)]) -> State {
        let ids = placements.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut state = state_with_tiled(&ids);
        for pane in &mut state.workspaces[0].panes {
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
        state.focused_pane = Some(start);
        for (&direction, &expected) in directions.iter().zip(expected) {
            assert_eq!(
                focus_in_direction(&mut state, direction, viewport),
                Some(expected)
            );
        }
    }

    #[test]
    fn cycle_focus_wraps_in_both_directions() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.focused_pane = Some(2);
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(3));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(1));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, false), Some(3));
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
        state.focused_pane = Some(2);

        assert_eq!(
            focus_in_direction_no_wrap(&mut state, Direction::Right, viewport),
            None
        );
        assert_eq!(state.focused_pane, Some(2));

        assert_eq!(
            focus_in_direction(&mut state, Direction::Right, viewport),
            Some(1)
        );
        assert_eq!(state.focused_pane, Some(1));
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
        state.workspaces[0].layout_kind = LayoutKind::Grid;

        state.focused_pane = Some(2);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Right, viewport),
            Some(1)
        );

        state.focused_pane = Some(1);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(2)
        );

        state.focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Down, viewport),
            Some(1)
        );

        state.focused_pane = Some(1);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(3)
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
            for return_direction in [Direction::Down, Direction::Up] {
                let mut state = state_with_floating(&vertical);
                state.focused_pane = Some(source);
                assert_eq!(
                    focus_in_direction(&mut state, Direction::Down, viewport),
                    Some(3)
                );
                assert_eq!(
                    focus_in_direction(&mut state, return_direction, viewport),
                    Some(source)
                );
            }
        }
        let mut state = state_with_floating(&vertical);
        state.focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(1)
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Left, viewport),
            Some(2)
        );
        for direction in [Direction::Up, Direction::Down] {
            assert_directional_sequence(&vertical, 2, &[direction; 3], &[3, 2, 3]);
        }

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
            for return_direction in [Direction::Right, Direction::Left] {
                let mut state = state_with_floating(&horizontal);
                state.focused_pane = Some(source);
                assert_eq!(
                    focus_in_direction(&mut state, Direction::Right, viewport),
                    Some(3)
                );
                assert_eq!(
                    focus_in_direction(&mut state, return_direction, viewport),
                    Some(source)
                );
            }
        }
        let mut state = state_with_floating(&horizontal);
        state.focused_pane = Some(3);
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(1)
        );
        assert_eq!(
            focus_in_direction(&mut state, Direction::Up, viewport),
            Some(2)
        );
        for direction in [Direction::Left, Direction::Right] {
            assert_directional_sequence(&horizontal, 2, &[direction; 3], &[3, 2, 3]);
        }
        assert_directional_sequence(
            &horizontal,
            3,
            &[Direction::Up, Direction::Down, Direction::Up],
            &[1, 2, 1],
        );
        assert_directional_sequence(
            &horizontal,
            3,
            &[Direction::Down, Direction::Up, Direction::Down],
            &[2, 1, 2],
        );
    }

    #[test]
    fn move_focused_pane_switches_to_target_workspace() {
        let mut state = state_with_tiled(&[1, 2]);
        state.workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        append_tiled_window(&mut state.workspaces[1], 3);
        state.active_workspace = 0;
        state.focused_pane = Some(2);

        move_focused_to_workspace(&mut state, 1);

        assert_eq!(state.active_workspace, 1);
        assert_eq!(state.focused_pane, Some(2));
        assert!(state.workspaces[0].panes.iter().any(|pane| pane.id == 1));
        assert!(!state.workspaces[0].panes.iter().any(|pane| pane.id == 2));
        assert!(state.workspaces[1].panes.iter().any(|pane| pane.id == 2));
    }

    #[test]
    fn relocate_active_workspace_swaps_content_when_target_is_occupied() {
        let mut state = state_with_tiled(&[1, 2]);
        state.workspaces[0].name = Some("code".to_string());
        state.workspaces[0].split_ratios[0] = 0.71;
        state.workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        append_tiled_window(&mut state.workspaces[1], 3);
        state.workspaces[1].split_ratios[0] = 0.42;
        state.active_workspace = 0;
        state.focused_pane = Some(2);

        relocate_active_workspace(&mut state, 1);

        assert_eq!(state.active_workspace, 1);
        assert_eq!(state.focused_pane, Some(2));
        assert_eq!(state.workspaces[0].panes.len(), 1);
        assert!(state.workspaces[0].panes.iter().any(|pane| pane.id == 3));
        assert_eq!(state.workspaces[0].split_ratios[0], 0.42);
        assert_eq!(state.workspaces[1].name.as_deref(), Some("code"));
        assert_eq!(state.workspaces[1].split_ratios[0], 0.71);
        assert_eq!(state.workspaces[1].panes.len(), 2);
        assert!(state.workspaces[1].panes.iter().any(|pane| pane.id == 1));
        assert!(state.workspaces[1].panes.iter().any(|pane| pane.id == 2));
    }

    #[test]
    fn relocate_active_workspace_preserves_layout_on_empty_target() {
        let mut state = state_with_tiled(&[1, 2]);
        state.workspaces[0].layout_kind = LayoutKind::Master;
        state.workspaces[0].split_ratios[0] = 0.71;
        let source_tree = state.workspaces[0].tile_tree.clone();
        state.active_workspace = 0;
        state.focused_pane = Some(2);

        relocate_active_workspace(&mut state, 2);

        assert_eq!(state.active_workspace, 2);
        assert_eq!(state.workspaces[2].layout_kind, LayoutKind::Master);
        assert_eq!(state.workspaces[2].split_ratios[0], 0.71);
        assert_eq!(state.workspaces[2].tile_tree, source_tree);
        assert_eq!(state.workspaces[2].panes.len(), 2);
        assert_eq!(state.workspaces[0].panes.len(), 0);
    }

    #[test]
    fn promote_swaps_focused_into_master_slot() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.focused_pane = Some(3);
        assert!(promote_focused_to_master(&mut state));

        let mut leaves = Vec::new();
        collect_tree_leaves(state.workspaces[0].tile_tree.as_ref().unwrap(), &mut leaves);
        assert_eq!(leaves.first(), Some(&3));
        assert_eq!(state.focused_pane, Some(3));

        // Already the master → no-op.
        assert!(!promote_focused_to_master(&mut state));
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
        set_reported_status(&mut state.workspaces[0].panes[0], "blocked");
        set_reported_status(&mut state.workspaces[0].panes[1], "BLOCKED");
        state.workspaces[0].panes[1].closing = true;
        state.workspaces[1].panes.push(Pane::new(
            3,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));
        set_reported_status(&mut state.workspaces[1].panes[0], " blocked ");

        state.focused_pane = Some(1);
        assert_eq!(next_blocked_pane(&state), Some(3));
        state.focused_pane = Some(3);
        assert_eq!(next_blocked_pane(&state), Some(1));
    }

    #[test]
    fn next_blocked_pane_handles_no_focus_and_no_other_match() {
        let mut state = state_with_tiled(&[1, 2]);
        set_reported_status(&mut state.workspaces[0].panes[1], "blocked");
        state.focused_pane = None;
        assert_eq!(next_blocked_pane(&state), Some(2));

        state.focused_pane = Some(2);
        assert_eq!(next_blocked_pane(&state), None);
        state.workspaces[0].panes[1].terminal.reported_status = None;
        assert_eq!(next_blocked_pane(&state), None);
    }
}
