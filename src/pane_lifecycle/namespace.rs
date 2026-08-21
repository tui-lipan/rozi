use crate::ops::focus::{first_visible_pane, focus_near_pane_in_workspace, reference_pane_rect};
use crate::state::{Pane, PaneId, State};
use crate::tiling::remove_tiled_window;

pub(crate) fn find_pane(state: &State, id: PaneId) -> Option<&Pane> {
    if let Some(pane) = state.popup.as_ref().filter(|pane| pane.id == id) {
        return Some(pane);
    }
    // The scratch workspace lives outside attachment workspaces; route its events here too.
    if let Some(pane) = state.scratch.panes.iter().find(|pane| pane.id == id) {
        return Some(pane);
    }
    state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == id)
}

pub(crate) fn find_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    if state.popup.as_ref().is_some_and(|pane| pane.id == id) {
        return state.popup.as_mut();
    }
    if let Some(index) = state.scratch.panes.iter().position(|pane| pane.id == id) {
        return state.scratch.panes.get_mut(index);
    }
    state
        .current_mut()
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

/// Whether `id` currently lives in the client-local namespace (popup or scratch).
pub(crate) fn pane_is_local(state: &State, id: PaneId) -> bool {
    state.popup.as_ref().is_some_and(|pane| pane.id == id) || crate::scratchpad::contains(state, id)
}

/// Resolve a pane in the namespace named on the wire. Local events never search attachment
/// workspaces, and shared events never search scratch/popup, even when numeric ids collide.
pub(crate) fn find_pane_in_namespace(state: &State, id: PaneId, local: bool) -> Option<&Pane> {
    if local {
        if let Some(pane) = state.popup.as_ref().filter(|pane| pane.id == id) {
            return Some(pane);
        }
        return state.scratch.panes.iter().find(|pane| pane.id == id);
    }
    state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == id)
}

pub(crate) fn find_pane_in_namespace_mut(
    state: &mut State,
    id: PaneId,
    local: bool,
) -> Option<&mut Pane> {
    if local {
        if state.popup.as_ref().is_some_and(|pane| pane.id == id) {
            return state.popup.as_mut();
        }
        let index = state.scratch.panes.iter().position(|pane| pane.id == id)?;
        return state.scratch.panes.get_mut(index);
    }
    state
        .current_mut()
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

pub(crate) fn remove_pane(state: &mut State, id: PaneId) {
    let removed_rect = reference_pane_rect(
        state,
        &state.current().workspaces[state.current().active_workspace],
        id,
        None,
    );
    remove_pane_with_reference(state, id, removed_rect);
}

pub(crate) fn remove_pane_with_reference(
    state: &mut State,
    id: PaneId,
    removed_rect: Option<tui_lipan::prelude::FloatRect>,
) {
    if state.moving_pane.is_some_and(|session| session.id == id) {
        state.moving_pane = None;
    }
    if state
        .resizing_pane
        .as_ref()
        .is_some_and(|session| session.id == id)
    {
        state.resizing_pane = None;
    }

    let focus_updates: Vec<(usize, Option<PaneId>)> = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .filter_map(|(workspace_index, workspace)| {
            if workspace.focused_pane != Some(id) {
                return None;
            }
            Some((
                workspace_index,
                focus_near_pane_in_workspace(state, workspace, id, removed_rect)
                    .or_else(|| first_visible_pane(workspace)),
            ))
        })
        .collect();

    for workspace in &mut state.current_mut().workspaces {
        remove_tiled_window(workspace, id);
        workspace.panes.retain(|pane| pane.id != id);
    }
    clear_pane_local_state(state, id);

    for (workspace_index, focus) in focus_updates {
        state.current_mut().workspaces[workspace_index].focused_pane = focus;
        if workspace_index == state.current().active_workspace {
            state.current_mut().focused_pane = focus;
        }
    }
}

pub(crate) fn clear_pane_local_state(state: &mut State, id: PaneId) {
    if state
        .search
        .as_ref()
        .is_some_and(|search| search.target == id)
    {
        state.search = None;
        state.commands_dirty = true;
    }
    if state
        .copy_mode
        .as_ref()
        .is_some_and(|copy| copy.target == id)
    {
        state.copy_mode = None;
        state.mode = crate::state::Mode::Normal;
        state.commands_dirty = true;
    }
    if state
        .hint_mode
        .as_ref()
        .is_some_and(|hints| hints.target == id)
    {
        state.hint_mode = None;
        state.mode = crate::state::Mode::Normal;
        state.commands_dirty = true;
    }
    if state
        .rename
        .as_ref()
        .is_some_and(|rename| rename.target == id)
    {
        state.rename = None;
    }
    if state
        .copy_feedback_target
        .is_some_and(|(epoch, target)| epoch == state.runtime_epoch && target == id)
    {
        state.copy_feedback_target = None;
        state.copy_feedback_epoch = state.copy_feedback_epoch.wrapping_add(1);
    }
}

pub(crate) fn pane_env(
    control_socket_path: Option<&std::path::Path>,
    pane: &Pane,
    remote_attached: bool,
    forwarded_environment: &[String],
) -> Vec<(String, String)> {
    // A persistent server may have been created by an older SSH or desktop client. Sample the
    // client that initiates this spawn instead, but never send local capabilities to another host.
    let mut env = if remote_attached {
        Vec::new()
    } else {
        crate::platform::environment::forwarded_client_environment(forwarded_environment)
    };
    env.extend([
        ("ROZI".to_string(), "1".to_string()),
        ("ROZI_PANE".to_string(), pane.id.to_string()),
    ]);
    // Under `--remote`, the control socket lives on the client machine and must not be advertised
    // into remote PTYs (it may collide with an unrelated path on the remote host).
    if !remote_attached && let Some(path) = control_socket_path {
        env.push(("ROZI_SOCKET".to_string(), path.display().to_string()));
    }
    // Same reasoning for the binary: a remote PTY runs on the other host, where this client's own
    // path means nothing. A remote pane falls back to whatever `rozi` the remote has on `PATH`.
    if !remote_attached && let Some(path) = crate::platform::paths::current_binary() {
        env.push(("ROZI_BIN".to_string(), path.display().to_string()));
    }
    // Per-spawn additions last so a caller-supplied value wins over the standard set.
    env.extend(pane.identity.env.iter().cloned());
    env
}
