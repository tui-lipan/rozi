//! Right-click menus: opening one on a pane, workspace tab, or picker row, and carrying out the
//! entry chosen from it.
//!
//! Each entry does what the matching keyboard action does, but to the menu's target rather than to
//! whatever has focus, and it passes the same gates first (see [`crate::actions::admit_action`]).
//! Choosing an entry is the confirmation, as it is from the command palette, so entries that the
//! keyboard arms on a first press act at once.

use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::input::Action;
use crate::layout::model::SplitAxis;
use crate::pane::id::PaneId;
use crate::state::{ContextMenuState, ContextMenuTarget, State};

#[derive(Clone, Debug)]
pub enum ContextMenuMsg {
    /// Right click on a pane's terminal, title, or border.
    Pane {
        id: PaneId,
        anchor: (u16, u16),
    },
    /// Right click on a workspace tab; `None` is the bar's empty space.
    WorkspaceTab {
        slot: Option<usize>,
        anchor: (u16, u16),
    },
    /// Right click on a Sessions row, by its index into the picker's entries.
    SessionRow {
        index: usize,
        anchor: (u16, u16),
    },
    /// Right click on an Agents row.
    AgentRow {
        location: crate::state::AgentLocation,
        anchor: (u16, u16),
    },
    /// A pane's terminal reported its mouse selection.
    PaneSelection(PaneId, Option<String>),
    Activate(Arc<str>),
    Close,
}

pub(crate) fn handle(ctx: &mut Context<AppRoot>, msg: ContextMenuMsg) -> Update {
    match msg {
        ContextMenuMsg::Pane { id, anchor } => open_pane(ctx, id, anchor),
        ContextMenuMsg::WorkspaceTab { slot, anchor } => open_workspace(ctx, slot, anchor),
        ContextMenuMsg::SessionRow { index, anchor } => open_session(ctx, index, anchor),
        ContextMenuMsg::AgentRow { location, anchor } => {
            open(ctx, ContextMenuTarget::Agent(location), anchor)
        }
        ContextMenuMsg::PaneSelection(id, text) => {
            if let Some(pane) = crate::pane::lifecycle::find_pane_mut(&mut ctx.state, id) {
                pane.mouse_selection = text;
            }
            // Nothing on screen reads it until a menu opens.
            Update::none()
        }
        ContextMenuMsg::Activate(entry) => activate(ctx, &entry),
        ContextMenuMsg::Close => close(ctx),
    }
}

fn open(ctx: &mut Context<AppRoot>, target: ContextMenuTarget, anchor: (u16, u16)) -> Update {
    if !ctx.state.config.mouse.context_menus || !target_live(&ctx.state, &target) {
        return Update::none();
    }
    ctx.state.context_menu = Some(ContextMenuState { target, anchor });
    Update::full()
}

fn open_pane(ctx: &mut Context<AppRoot>, id: PaneId, anchor: (u16, u16)) -> Update {
    // Copy and hint mode hold the keyboard for their own pane until they end.
    if ctx.state.copy_mode.is_some() || ctx.state.hint_mode.is_some() {
        return Update::none();
    }
    let Some(generation) = visible_pane(&ctx.state, id).map(|pane| pane.pty_generation) else {
        return Update::none();
    };
    open(ctx, ContextMenuTarget::Pane { id, generation }, anchor)
}

fn open_workspace(ctx: &mut Context<AppRoot>, slot: Option<usize>, anchor: (u16, u16)) -> Update {
    let Some(id) = slot.and_then(|slot| ctx.state.current().workspaces.get(slot)) else {
        return Update::none();
    };
    let id = id.id;
    open(ctx, ContextMenuTarget::Workspace(id), anchor)
}

fn open_session(ctx: &mut Context<AppRoot>, index: usize, anchor: (u16, u16)) -> Update {
    let Some(entry) = ctx
        .state
        .session_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index))
    else {
        return Update::none();
    };
    let identity = (entry.name.clone(), entry.remote_target.clone());
    open(ctx, ContextMenuTarget::Session(identity), anchor)
}

/// The keyboard way in: the focused pane's menu, at the top-left of its content.
pub(crate) fn open_for_focused(ctx: &mut Context<AppRoot>) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::none();
    };
    let anchor = crate::view::settled_active_pane_rects(&ctx.state, ctx.viewport())
        .into_iter()
        .find(|(pane, _)| *pane == id)
        .map_or_else(
            || {
                let viewport = ctx.viewport();
                (viewport.w / 3, viewport.h / 3)
            },
            |(_, rect)| (rect.x.max(0.0) as u16 + 2, rect.y.max(0.0) as u16 + 1),
        );
    open_pane(ctx, id, anchor)
}

fn close(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.context_menu.take().is_none() {
        return Update::none();
    }
    Update::full()
}

/// Close a menu that has nothing left to act on: menus were turned off, or its target is gone.
pub(crate) fn sync(state: &mut State) -> bool {
    let Some(menu) = state.context_menu.as_ref() else {
        return false;
    };
    if state.config.mouse.context_menus && target_live(state, &menu.target) {
        return false;
    }
    state.context_menu = None;
    true
}

/// A pane the user can see and point at: in the workspace on screen, or in the open scratchpad.
fn visible_pane(state: &State, id: PaneId) -> Option<&crate::state::Pane> {
    state
        .active_workspace_ref()
        .panes
        .iter()
        .find(|pane| pane.id == id && !pane.closing)
}

pub(crate) fn target_live(state: &State, target: &ContextMenuTarget) -> bool {
    match target {
        ContextMenuTarget::Pane { id, generation } => {
            visible_pane(state, *id).is_some_and(|pane| pane.pty_generation == *generation)
        }
        ContextMenuTarget::Workspace(id) => workspace_slot(state, *id).is_some(),
        ContextMenuTarget::Session(identity) => session_index(state, identity).is_some(),
        ContextMenuTarget::Agent(location) => {
            state.agent_picker.is_some() && agent_listed(state, location)
        }
    }
}

pub(crate) fn workspace_slot(state: &State, id: crate::state::WorkspaceId) -> Option<usize> {
    state
        .current()
        .workspaces
        .iter()
        .position(|workspace| workspace.id == id)
}

pub(crate) fn session_index(
    state: &State,
    identity: &crate::state::SessionIdentity,
) -> Option<usize> {
    state
        .session_picker
        .as_ref()?
        .entries
        .iter()
        .position(|entry| entry.name == identity.0 && entry.remote_target == identity.1)
}

fn agent_listed(state: &State, location: &crate::state::AgentLocation) -> bool {
    crate::view::agents::global_agent_rows(state)
        .iter()
        .any(|row| &row.location == location)
}

fn activate(ctx: &mut Context<AppRoot>, entry: &str) -> Update {
    let Some(menu) = ctx.state.context_menu.take() else {
        return Update::none();
    };
    if !target_live(&ctx.state, &menu.target) {
        return Update::full();
    }
    match menu.target {
        ContextMenuTarget::Pane { id, .. } => pane_entry(ctx, id, entry),
        ContextMenuTarget::Workspace(id) => match workspace_slot(&ctx.state, id) {
            Some(slot) => workspace_entry(ctx, slot, entry),
            None => Update::full(),
        },
        ContextMenuTarget::Session(identity) => match session_index(&ctx.state, &identity) {
            Some(index) => session_entry(ctx, index, entry),
            None => Update::full(),
        },
        ContextMenuTarget::Agent(location) => agent_entry(ctx, location, entry),
    }
}

/// Pane menu entry ids, shared with the view that lists them.
pub(crate) mod pane_entries {
    pub(crate) const COPY: &str = "copy";
    pub(crate) const PASTE: &str = "paste";
    pub(crate) const SEARCH: &str = "search";
    pub(crate) const SPLIT_RIGHT: &str = "split-right";
    pub(crate) const SPLIT_DOWN: &str = "split-down";
    pub(crate) const NEW_PANE: &str = "new-pane";
    pub(crate) const RENAME: &str = "rename";
    pub(crate) const ZOOM: &str = "zoom";
    pub(crate) const FLOAT: &str = "float";
    pub(crate) const MOVE_TO: &str = "move-to";
    /// `move-to:<slot>` for each workspace in the submenu.
    pub(crate) const MOVE_TO_PREFIX: &str = "move-to:";
    pub(crate) const OPEN_DIRECTORY: &str = "open-directory";
    pub(crate) const COPY_ID: &str = "copy-id";
    pub(crate) const CLOSE: &str = "close";
}

fn admitted(
    ctx: &mut Context<AppRoot>,
    action: Action,
    run: impl FnOnce(&mut Context<AppRoot>) -> Update,
) -> Update {
    ctx.state.commands_dirty = true;
    match crate::actions::admit_action(ctx, action) {
        Ok(()) => run(ctx),
        Err(update) => update,
    }
}

fn pane_entry(ctx: &mut Context<AppRoot>, id: PaneId, entry: &str) -> Update {
    use pane_entries::*;
    if let Some(slot) = entry
        .strip_prefix(MOVE_TO_PREFIX)
        .and_then(|slot| slot.parse::<usize>().ok())
    {
        return admitted(ctx, Action::MoveToWorkspace(slot), |ctx| {
            crate::ops::focus::move_pane_to_workspace(&mut ctx.state, id, slot);
            crate::ops::focus::request_current_pane_focus(ctx);
            Update::full()
        });
    }
    match entry {
        COPY => {
            let text = crate::pane::lifecycle::find_pane(&ctx.state, id)
                .and_then(|pane| pane.mouse_selection.clone());
            copy_text(ctx, text.as_deref())
        }
        PASTE => crate::actions::paste_into_pane(ctx, id),
        SEARCH => admitted(ctx, Action::OpenSearch, |ctx| {
            crate::ops::search::open_search_in(ctx, id)
        }),
        SPLIT_RIGHT => split(ctx, id, Some(SplitAxis::Horizontal)),
        SPLIT_DOWN => split(ctx, id, Some(SplitAxis::Vertical)),
        NEW_PANE => split(ctx, id, None),
        RENAME => admitted(ctx, Action::RenamePane, |ctx| {
            crate::ops::identity::open_rename_pane_for(ctx, id)
        }),
        ZOOM => admitted(ctx, Action::ToggleFullscreen, |ctx| {
            crate::ops::resize_move::toggle_fullscreen_for(ctx, id)
        }),
        FLOAT => admitted(ctx, Action::ToggleFloat, |ctx| {
            crate::ops::resize_move::toggle_tiling_for(ctx, id);
            Update::full()
        }),
        OPEN_DIRECTORY => match pane_directory(&ctx.state, id) {
            Some(directory) => crate::ops::links::open_path(ctx, id, &directory),
            None => Update::full(),
        },
        COPY_ID => copy_text(ctx, Some(&id.to_string())),
        CLOSE => admitted(ctx, Action::Close, |ctx| {
            crate::ops::exit::clear_pending(ctx);
            crate::pane::lifecycle::close_pane(ctx, id)
        }),
        _ => Update::full(),
    }
}

fn split(ctx: &mut Context<AppRoot>, id: PaneId, axis: Option<SplitAxis>) -> Update {
    admitted(ctx, Action::Spawn, |ctx| {
        crate::pane::lifecycle::split_pane(ctx, id, axis)
    })
}

/// The directory a pane's shell is in, as the server sees it.
pub(crate) fn pane_directory(state: &State, id: PaneId) -> Option<String> {
    let pane = crate::pane::lifecycle::find_pane(state, id)?;
    if state.current().remote_host.is_some() {
        pane.server_cwd_ref().map(str::to_string)
    } else {
        pane.local_cwd_ref().map(str::to_string)
    }
}

fn copy_text(ctx: &mut Context<AppRoot>, text: Option<&str>) -> Update {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Update::full();
    };
    if let Err(err) = ctx.clipboard().copy(text) {
        crate::pane::pty_events::notify_error(ctx, "Copy failed", err.to_string());
    }
    Update::full()
}

/// Workspace menu entry ids.
pub(crate) mod workspace_entries {
    pub(crate) const SWITCH: &str = "switch";
    pub(crate) const RENAME: &str = "rename";
    pub(crate) const MOVE_LEFT: &str = "move-left";
    pub(crate) const MOVE_RIGHT: &str = "move-right";
    pub(crate) const KILL: &str = "kill";
}

fn workspace_entry(ctx: &mut Context<AppRoot>, slot: usize, entry: &str) -> Update {
    use workspace_entries::*;
    match entry {
        SWITCH => crate::actions::execute_action(ctx, Action::SwitchWorkspace(slot)),
        RENAME => admitted(ctx, Action::RenameWorkspace, |ctx| {
            crate::ops::identity::open_rename_workspace_for(ctx, slot)
        }),
        MOVE_LEFT if slot > 0 => {
            crate::actions::execute_action(ctx, Action::MoveWorkspace(slot, slot - 1))
        }
        MOVE_RIGHT => crate::actions::execute_action(ctx, Action::MoveWorkspace(slot, slot + 1)),
        KILL => admitted(ctx, Action::KillWorkspace, |ctx| {
            crate::ops::exit::kill_workspace(ctx, slot)
        }),
        _ => Update::full(),
    }
}

/// Session menu entry ids.
pub(crate) mod session_entries {
    pub(crate) const OPEN: &str = "open";
    pub(crate) const RESTART: &str = "restart";
    pub(crate) const KILL: &str = "kill";
}

fn session_entry(ctx: &mut Context<AppRoot>, index: usize, entry: &str) -> Update {
    use session_entries::*;
    match entry {
        OPEN => crate::ops::session::activate_selected_session(ctx, index),
        RESTART => {
            let Some(entry) = session_entry_at(&ctx.state, index) else {
                return Update::full();
            };
            crate::ops::session::clear_pending_session_arms(ctx);
            crate::ops::session::restart_discovered_session(ctx, entry)
        }
        KILL => crate::ops::session::kill_session_row(ctx, index),
        _ => Update::full(),
    }
}

fn session_entry_at(
    state: &State,
    index: usize,
) -> Option<crate::session::discovery::DiscoveredSession> {
    state.session_picker.as_ref()?.entries.get(index).cloned()
}

/// Agent menu entry ids: what Enter and Ctrl+Enter do on the row.
pub(crate) mod agent_entries {
    pub(crate) const OPEN: &str = "open";
    pub(crate) const OPEN_OTHER: &str = "open-other";
}

fn agent_entry(
    ctx: &mut Context<AppRoot>,
    location: crate::state::AgentLocation,
    entry: &str,
) -> Update {
    match entry {
        agent_entries::OPEN => crate::ops::agents::activate(ctx, location, false),
        agent_entries::OPEN_OTHER => crate::ops::agents::activate(ctx, location, true),
        _ => Update::full(),
    }
}

#[cfg(test)]
mod tests {
    use tui_lipan::{FloatRect, Rect, TestBackend};

    use super::ContextMenuMsg;
    use crate::layout::tiling::append_tiled_window;
    use crate::state::Pane;
    use crate::{AppRoot, Msg};

    fn pane(backend: &TestBackend<AppRoot>, id: u32) -> &Pane {
        let state = backend.state();
        state.current().workspaces[0]
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .expect("pane")
    }

    #[test]
    fn entries_act_on_the_pane_the_menu_opened_on() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                let workspace = state.active_workspace_mut();
                workspace.panes.clear();
                for id in 1..=2 {
                    workspace
                        .panes
                        .push(Pane::new(id, 100, FloatRect::default()));
                    append_tiled_window(workspace, id);
                }
                workspace.focused_pane = Some(1);
                state.current_mut().focused_pane = Some(1);
            }
            backend.render();
            let open = |backend: &mut TestBackend<AppRoot>| {
                backend
                    .dispatch(Msg::ContextMenu(ContextMenuMsg::Pane {
                        id: 2,
                        anchor: (60, 10),
                    }))
                    .expect("open");
            };

            open(&mut backend);
            assert!(backend.state().context_menu.is_some());
            assert_eq!(backend.state().current().focused_pane, Some(1));
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Activate("zoom".into())))
                .expect("zoom");
            assert!(pane(&backend, 2).fullscreen);
            assert!(!pane(&backend, 1).fullscreen);

            // A respawn puts a different program in the pane; the menu opened on the old one closes.
            open(&mut backend);
            backend.state_mut().current_mut().workspaces[0].panes[1].pty_generation += 1;
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Activate("zoom".into())))
                .expect("stale zoom");
            assert!(backend.state().context_menu.is_none());
            assert!(pane(&backend, 2).fullscreen, "a stale entry does nothing");
        });
    }
}
