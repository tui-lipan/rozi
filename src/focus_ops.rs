use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{canvas_bounds_from_viewport, closest_pane_to_rect, directional_score};
use crate::layout::{placement_for, workspace_target_rects};
use crate::state::{Direction, Pane, PaneId, State, Workspace};
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

pub(crate) fn focus_in_direction(
    state: &mut State,
    direction: Direction,
    viewport: Rect,
) -> Option<PaneId> {
    let bounds = canvas_bounds_from_viewport(viewport);
    let workspace = &state.workspaces[state.active_workspace];
    let placements = workspace_target_rects(workspace, bounds);
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
    let next = candidates
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            directional_score(current.rect, candidate.rect, direction)
                .map(|score| (candidate.id, score))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(id, _)| id)
        .or_else(|| cycle_focus_id(&candidates, focused, direction));

    if let Some(next_id) = next {
        focus_pane(state, next_id);
        Some(next_id)
    } else {
        None
    }
}

pub(crate) fn cycle_focus_id(
    candidates: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let index = candidates
        .iter()
        .position(|candidate| candidate.id == focused)
        .unwrap_or(0);
    let next_index = match direction {
        Direction::Left | Direction::Up => index
            .checked_sub(1)
            .unwrap_or_else(|| candidates.len().saturating_sub(1)),
        Direction::Right | Direction::Down => (index + 1) % candidates.len(),
    };
    candidates.get(next_index).map(|candidate| candidate.id)
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
        true
    } else {
        false
    }
}

pub(crate) fn switch_workspace(state: &mut State, index: usize) {
    if index >= state.workspaces.len() {
        return;
    }
    state.active_workspace = index;
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
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
    if !pane.floating {
        remove_tiled_window(&mut state.workspaces[source_index], pane.id);
    }
    pane.opening = false;
    pane.closing = false;
    state.workspaces[target_index].focused_pane = Some(pane.id);
    if !pane.floating {
        append_tiled_window(&mut state.workspaces[target_index], pane.id);
    }
    state.workspaces[target_index].panes.push(pane);
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
}

pub(crate) fn focus_pane(state: &mut State, id: PaneId) {
    if state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing)
    {
        state.focused_pane = Some(id);
        state.workspaces[state.active_workspace].focused_pane = Some(id);
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
        let bounds = canvas_bounds_from_viewport(viewport);
        let placements = workspace_target_rects(workspace, bounds);
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
        let bounds = canvas_bounds_from_viewport(viewport);
        let placements = workspace_target_rects(workspace, bounds);
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
    ctx.request_focus(view::pane_terminal_key(id));
}

pub(crate) fn request_current_pane_focus(ctx: &mut Context<HyprmuxApp>) {
    if let Some(id) = ctx.state.focused_pane {
        request_pane_focus(ctx, id);
    }
}

pub(crate) fn request_search_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::search_input_key());
}

pub(crate) fn request_rename_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::rename_input_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::theme_picker_key());
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
    use crate::state::Pane;
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

    #[test]
    fn cycle_focus_wraps_in_both_directions() {
        let mut state = state_with_tiled(&[1, 2, 3]);
        state.focused_pane = Some(2);
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(3));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, true), Some(1));
        assert_eq!(cycle_focus_in_tiled_order(&mut state, false), Some(3));
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
}
