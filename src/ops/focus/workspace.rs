use crate::layout::anim::GeometryAnimation;
use crate::layout::scrollable_viewport_anchor;
use crate::layout::tiling::{append_tiled_window, remove_tiled_window};
use crate::state::{LayoutKind, PaneId, State, Workspace};

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
    super::choose_fallback_focus(state);
    if let Some(focus) = state.current().focused_pane {
        // Normalize Scrollable viewport for the newly active focus (covers inactive reconcile
        // fallback under a surviving foreign anchor, and other stale local viewport state).
        super::sync_scrollable_reveal(state, focus, false);
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

    let Some(tiled) = state.current().workspaces[source_index]
        .panes
        .iter()
        .find(|pane| pane.id == focused)
        .map(|pane| !pane.floating)
    else {
        super::choose_fallback_focus(state);
        return;
    };
    transfer_pane(state, focused, source_index, target_index);
    super::choose_fallback_focus(state);

    state.current_mut().active_workspace = target_index;
    let scrollable = state.current().workspaces[target_index].layout_kind == LayoutKind::Scrollable;
    let (prior_anchor, prior_edge, reveal_decision) = if tiled && scrollable {
        let ws = &state.current().workspaces[target_index];
        let prior = scrollable_viewport_anchor(ws, &ws.tiled_ids());
        let edge = ws.scrollable_reveal_edge;
        // Classify before overwriting target focus so a missing stored anchor still uses the
        // previous tiled focus as the strip reference.
        let decision = super::classify_scrollable_reveal(state, focused, prior);
        (prior, Some(edge), decision)
    } else {
        (None, None, None)
    };
    state.current_mut().focused_pane = Some(focused);
    state.current_mut().workspaces[target_index].focused_pane = Some(focused);
    if tiled && scrollable {
        super::apply_scrollable_reveal_decision(
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

/// Move pane `id` from workspace `source` to the end of workspace `target`, leaving focus and the
/// active workspace alone. A tiled pane joins the end of the target's tiling order - appended to
/// the target's settled tree, as a session server appends it to the shared document - and a
/// floating pane keeps its rect. Returns whether the pane was there to move.
///
/// The structural half of moving a pane. The interactive move follows it with focus; `pane move`
/// leaves focus where it was.
pub(crate) fn transfer_pane(state: &mut State, id: PaneId, source: usize, target: usize) -> bool {
    let workspaces = &mut state.current_mut().workspaces;
    if source == target || target >= workspaces.len() {
        return false;
    }
    let Some(position) = workspaces[source]
        .panes
        .iter()
        .position(|pane| pane.id == id)
    else {
        return false;
    };
    let mut pane = workspaces[source].panes.remove(position);
    let tiled = !pane.floating;
    if tiled {
        remove_tiled_window(&mut workspaces[source], id);
    }
    pane.opening = false;
    pane.closing = false;
    let destination = &mut workspaces[target];
    if tiled {
        destination.tile_tree = crate::layout::effective_tile_tree(destination, None);
        append_tiled_window(destination, id);
    }
    // One fullscreen pane per workspace: an arriving fullscreen pane takes over, as a spawned one
    // does.
    let fullscreen = pane.fullscreen;
    destination.panes.push(pane);
    if fullscreen {
        crate::ops::resize_move::clear_other_fullscreen(destination, id);
    }
    true
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
        super::choose_fallback_focus(state);
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
        target.focused_pane = super::first_visible_pane(target);
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
