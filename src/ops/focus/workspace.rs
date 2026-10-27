use crate::layout::anim::GeometryAnimation;
use crate::layout::scrollable_viewport_anchor;
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
    let carried = gather_pinned_panes(state, index);
    arrive_with_pinned_panes(state, &carried);
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

/// Move every pinned pane that is not on workspace `target` there, returning the ones that moved.
///
/// Pinned panes always sit on the controlling client's active workspace. Switching keeps them
/// there, and [`keep_pinned_panes_on_active_workspace`] restores it after anything else - a
/// takeover, an attach, a scripted move. A follower changes nothing: it sees them wherever the
/// controller has them.
pub(crate) fn gather_pinned_panes(state: &mut State, target: usize) -> Vec<PaneId> {
    if !state.is_controller() || target >= state.current().workspaces.len() {
        return Vec::new();
    }
    let strays: Vec<(usize, PaneId)> = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != target)
        .flat_map(|(index, workspace)| {
            workspace
                .panes
                .iter()
                .filter(|pane| pane.pinned && !pane.closing)
                .map(move |pane| (index, pane.id))
        })
        .collect();
    let mut moved = Vec::new();
    for (source, id) in strays {
        if transfer_pane(state, id, source, target) {
            moved.push((source, id));
        }
    }
    for &(source, id) in &moved {
        let workspace = &mut state.current_mut().workspaces[source];
        if workspace.focused_pane == Some(id) {
            workspace.focused_pane = own_focus(workspace);
        }
    }
    moved.into_iter().map(|(_, id)| id).collect()
}

/// Resolve focus on the active workspace after `carried` pinned panes arrived on it.
///
/// A pinned pane keeps focus only where there is nothing else to focus: arriving somewhere returns
/// to the pane that workspace last had focused.
fn arrive_with_pinned_panes(state: &mut State, carried: &[PaneId]) {
    let destination = state.current().active_workspace_ref();
    let own = destination.focused_pane.filter(|id| {
        !carried.contains(id)
            && destination
                .panes
                .iter()
                .any(|pane| pane.id == *id && !pane.closing)
    });
    if own.is_some()
        && state
            .current()
            .focused_pane
            .is_some_and(|id| carried.contains(&id))
    {
        state.current_mut().focused_pane = own;
    }
    super::choose_fallback_focus(state);
}

/// The pane `workspace` should focus once the pinned panes passing through have left: the last
/// of its own it had focused, or its first.
fn own_focus(workspace: &Workspace) -> Option<PaneId> {
    let live = |id: &PaneId| {
        workspace
            .panes
            .iter()
            .any(|pane| pane.id == *id && !pane.closing && !pane.pinned)
    };
    workspace.unpinned_focus.filter(live).or_else(|| {
        workspace
            .panes
            .iter()
            .find(|pane| !pane.closing && !pane.pinned)
            .map(|pane| pane.id)
    })
}

/// Restore the pinned-pane invariant after any message: see [`gather_pinned_panes`]. Focus is
/// resolved again as a switch would, so an empty workspace that receives a pinned pane focuses it.
/// Returns whether that changed the focused pane, in which case the caller syncs framework focus.
pub(crate) fn keep_pinned_panes_on_active_workspace(state: &mut State) -> bool {
    let active = state.current().active_workspace;
    let stray = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .any(|(index, workspace)| {
            index != active
                && workspace
                    .panes
                    .iter()
                    .any(|pane| pane.pinned && !pane.closing)
        });
    if !stray {
        return false;
    }
    let focused = state.current().focused_pane;
    let carried = gather_pinned_panes(state, active);
    if carried.is_empty() {
        return false;
    }
    arrive_with_pinned_panes(state, &carried);
    state.current().focused_pane != focused
}

/// Remember the focused pane as its workspace's own focus, unless it is a pinned pane.
pub(crate) fn note_unpinned_focus(state: &mut State) {
    let Some(id) = state.current().focused_pane else {
        return;
    };
    let workspace = state.current_mut().active_workspace_mut();
    if workspace
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.pinned && !pane.closing)
    {
        workspace.unpinned_focus = Some(id);
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
    // Sending a pinned pane to a workspace settles it there.
    crate::layout::shared::edit_live_workspace(
        &mut state.current_mut().workspaces[source_index],
        |layout| layout.set_pinned(focused, false),
    );
    transfer_pane(state, focused, source_index, target_index);
    gather_pinned_panes(state, target_index);
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
    if source == target || target >= state.current().workspaces.len() {
        return false;
    }
    if !state.current().workspaces[source]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing)
    {
        return false;
    }
    let changed = crate::layout::shared::transfer_live_pane(state, id, target);
    if changed
        && let Some(pane) = state.current_mut().workspaces[target]
            .panes
            .iter_mut()
            .find(|pane| pane.id == id)
    {
        pane.opening = false;
    }
    changed
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
        let carried = gather_pinned_panes(state, target_index);
        arrive_with_pinned_panes(state, &carried);
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

/// Whether `workspace` holds nothing of its own. A pinned pane only rides along on it.
fn workspace_is_empty(workspace: &Workspace) -> bool {
    !workspace
        .panes
        .iter()
        .any(|pane| !pane.closing && !pane.pinned)
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
    std::mem::swap(&mut a.unpinned_focus, &mut b.unpinned_focus);
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
    to.unpinned_focus = from.unpinned_focus.take();
    to.name = from.name.take();
}
