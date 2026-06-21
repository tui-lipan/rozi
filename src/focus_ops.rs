use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{canvas_bounds_from_viewport, closest_pane_to_rect, directional_score};
use crate::layout::{placement_for, workspace_target_rects};
use crate::state::{Direction, Pane, PaneId, State, Workspace};
use crate::tiling::{self, append_tiled_window, remove_tiled_window};
use crate::view;

pub(crate) fn directional_neighbor(
    placements: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let current = placements
        .iter()
        .find(|candidate| candidate.id == focused)?;
    placements
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            directional_score(current.rect, candidate.rect, direction)
                .map(|score| (candidate.id, candidate.rect, score))
        })
        .min_by(|(_, _, a), (_, _, b)| a.total_cmp(b))
        .map(|(id, _, _)| id)
}

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
