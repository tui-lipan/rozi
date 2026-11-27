//! Right-click menus: opening one on a pane, workspace tab, or picker row, and carrying out the
//! entry chosen from it.
//!
//! Each entry does what the matching keyboard action does, but to the menu's target rather than to
//! whatever has focus, and it passes the same gates first (see [`crate::actions::admit_action`]).
//! Choosing an entry is the confirmation, as it is from the command palette, so entries that the
//! keyboard arms on a first press act at once.

use std::{borrow::Cow, sync::Arc};

use tui_lipan::TerminalSelection;

use tui_lipan::prelude::*;

use crate::AppRoot;

pub(crate) mod sidebar;
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
    SidebarRow(crate::state::RowTarget, (u16, u16)),
    SidebarTab(crate::config::SidebarTabId, (u16, u16)),
    SidebarPanel(crate::config::SidebarPosition, usize, (u16, u16)),
    EmptyWorkspace(crate::state::WorkspaceId, (u16, u16)),
    /// A pane's terminal reported its mouse selection.
    PaneSelection(PaneId, u64, Option<TerminalSelection>),
    Activate(Arc<str>),
    Close,
}

pub(crate) fn handle(ctx: &mut Context<AppRoot>, msg: ContextMenuMsg) -> Update {
    match msg {
        ContextMenuMsg::Pane { id, anchor } => open_pane(ctx, id, anchor, false),
        ContextMenuMsg::WorkspaceTab { slot, anchor } => open_workspace(ctx, slot, anchor),
        ContextMenuMsg::SessionRow { index, anchor } => open_session(ctx, index, anchor),
        ContextMenuMsg::AgentRow { location, anchor } => {
            open(ctx, agent_target(&ctx.state, location, false), anchor)
        }
        ContextMenuMsg::SidebarRow(row, anchor) => open_sidebar_row(ctx, row, anchor),
        ContextMenuMsg::SidebarTab(id, anchor) => {
            open(ctx, ContextMenuTarget::SidebarTab(id), anchor)
        }
        ContextMenuMsg::SidebarPanel(side, home, anchor) => {
            open(ctx, ContextMenuTarget::SidebarPanel(side, home), anchor)
        }
        ContextMenuMsg::EmptyWorkspace(id, anchor) => {
            open(ctx, ContextMenuTarget::EmptyWorkspace(id), anchor)
        }
        ContextMenuMsg::PaneSelection(id, generation, selection) => {
            if let Some(pane) = crate::pane::lifecycle::find_pane_mut(&mut ctx.state, id)
                && pane.pty_generation == generation
            {
                pane.mouse_selection = selection.filter(|range| !range.is_empty()).map(|range| {
                    crate::state::PaneMouseSelection {
                        terminal_id: pane.terminal.stable_terminal_id(),
                        range,
                        lineage: pane.terminal.scrollback_lineage(),
                    }
                });
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
    let pane_selection = match &target {
        ContextMenuTarget::Pane { id, .. } => {
            visible_pane(&ctx.state, *id).and_then(crate::state::Pane::mouse_selection_text)
        }
        _ => None,
    };
    ctx.state.context_menu = Some(ContextMenuState {
        target,
        anchor,
        pane_selection,
    });
    Update::full()
}

fn open_pane(ctx: &mut Context<AppRoot>, id: PaneId, anchor: (u16, u16), sidebar: bool) -> Update {
    // Copy and hint mode hold the keyboard for their own pane until they end.
    if ctx.state.copy_mode.is_some() || ctx.state.hint_mode.is_some() {
        return Update::none();
    }
    let Some(generation) = menu_pane(&ctx.state, id, sidebar).map(|pane| pane.pty_generation)
    else {
        return Update::none();
    };
    let epoch = ctx.state.runtime_epoch;
    open(
        ctx,
        ContextMenuTarget::Pane {
            id,
            generation,
            epoch,
            sidebar,
        },
        anchor,
    )
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
    let target = session_target(entry, false);
    open(ctx, target, anchor)
}

fn session_target(
    entry: &crate::session::discovery::DiscoveredSession,
    sidebar: bool,
) -> ContextMenuTarget {
    ContextMenuTarget::Session {
        identity: (entry.name.clone(), entry.remote_target.clone()),
        sidebar,
    }
}

fn agent_target(
    state: &State,
    location: crate::state::AgentLocation,
    sidebar: bool,
) -> ContextMenuTarget {
    ContextMenuTarget::Agent {
        location,
        sidebar,
        epoch: state.runtime_epoch,
    }
}

fn open_sidebar_row(
    ctx: &mut Context<AppRoot>,
    row: crate::state::RowTarget,
    anchor: (u16, u16),
) -> Update {
    use crate::state::RowTarget;
    let target = match row {
        RowTarget::Pane(id) => return open_pane(ctx, id, anchor, true),
        RowTarget::Agent(location) => agent_target(&ctx.state, location, true),
        RowTarget::Session(entry) => session_target(&entry, true),
        _ => return Update::none(),
    };
    open(ctx, target, anchor)
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
    open_pane(ctx, id, anchor, false)
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
pub(crate) fn visible_pane(state: &State, id: PaneId) -> Option<&crate::state::Pane> {
    state
        .active_workspace_ref()
        .panes
        .iter()
        .find(|pane| pane.id == id && !pane.closing)
}

fn menu_pane(state: &State, id: PaneId, sidebar: bool) -> Option<&crate::state::Pane> {
    if sidebar {
        crate::pane::lifecycle::find_pane_in_namespace(state, id, false)
            .filter(|pane| !pane.closing)
    } else {
        visible_pane(state, id)
    }
}

fn sidebar_tab_visible(state: &State, id: &str) -> bool {
    state
        .visible_sidebar_tabs()
        .any(|active| active.as_str() == id)
}

pub(crate) fn target_live(state: &State, target: &ContextMenuTarget) -> bool {
    match target {
        ContextMenuTarget::Pane {
            id,
            generation,
            epoch,
            sidebar,
        } => {
            state.runtime_epoch == *epoch
                && (!sidebar || sidebar_tab_visible(state, "panes"))
                && menu_pane(state, *id, *sidebar)
                    .is_some_and(|pane| pane.pty_generation == *generation)
        }
        ContextMenuTarget::Workspace(id) => workspace_slot(state, *id).is_some(),
        ContextMenuTarget::EmptyWorkspace(id) => {
            empty_workspace(state).is_some_and(|workspace| workspace == *id)
        }
        ContextMenuTarget::Session { identity, sidebar } => {
            session_row(state, identity, *sidebar).is_some()
        }
        ContextMenuTarget::Agent {
            location,
            sidebar,
            epoch,
        } => {
            let source_open = if *sidebar {
                sidebar_tab_visible(state, "agents")
            } else {
                state.agent_picker.is_some()
            };
            state.runtime_epoch == *epoch && source_open && agent_listed(state, location)
        }
        ContextMenuTarget::SidebarTab(id) => sidebar_tab_position(state, id)
            .is_some_and(|(panel, _)| state.sidebar_panel_visible(panel)),
        ContextMenuTarget::SidebarPanel(side, home) => {
            state
                .sidebar
                .panels
                .iter()
                .enumerate()
                .any(|(index, panel)| {
                    panel.dock == *side && panel.home == *home && state.sidebar_panel_visible(index)
                })
        }
    }
}

pub(crate) fn empty_workspace(state: &State) -> Option<crate::state::WorkspaceId> {
    if state.is_launcher() || state.scratch_visible || crate::view::workspace_is_covered(state) {
        return None;
    }
    let workspace = state.active_workspace_ref();
    workspace
        .panes
        .iter()
        .all(|pane| pane.closing || pane.pinned)
        .then_some(workspace.id)
}

pub(crate) fn sidebar_tab_position(
    state: &State,
    id: &crate::config::SidebarTabId,
) -> Option<(usize, usize)> {
    state
        .sidebar
        .panels
        .iter()
        .enumerate()
        .find_map(|(index, panel)| {
            panel
                .tabs
                .iter()
                .position(|tab| tab == id)
                .map(|position| (index, position))
        })
}

pub(crate) fn workspace_slot(state: &State, id: crate::state::WorkspaceId) -> Option<usize> {
    state
        .current()
        .workspaces
        .iter()
        .position(|workspace| workspace.id == id)
}

pub(crate) fn session_row<'a>(
    state: &'a State,
    identity: &crate::state::SessionIdentity,
    sidebar: bool,
) -> Option<Cow<'a, crate::session::discovery::DiscoveredSession>> {
    let matches = |entry: &crate::session::discovery::DiscoveredSession| {
        entry.name == identity.0 && entry.remote_target == identity.1
    };
    if !sidebar {
        return state
            .session_picker
            .as_ref()?
            .entries
            .iter()
            .find(|entry| matches(entry))
            .map(Cow::Borrowed);
    }
    if !sidebar_tab_visible(state, "sessions") {
        return None;
    }
    // Offline rows may come directly from the sidebar's host cache, without a discovered entry.
    state
        .sidebar_rows(&crate::config::SidebarTab::Sessions)
        .into_iter()
        .find_map(|row| match row.target {
            crate::state::RowTarget::Session(entry) if matches(&entry) => Some(Cow::Owned(*entry)),
            _ => None,
        })
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
    let update = match menu.target {
        ContextMenuTarget::Pane { id, sidebar, .. } => {
            if sidebar && entry == pane_entries::GO {
                return crate::update::sidebar::focus_pane(ctx, id).merge(Update::full());
            }
            if visible_pane(&ctx.state, id).is_some() {
                pane_entry(ctx, id, entry, menu.pane_selection.as_deref())
            } else {
                Update::full()
            }
        }
        ContextMenuTarget::Workspace(id) => match workspace_slot(&ctx.state, id) {
            Some(slot) => workspace_entry(ctx, slot, entry),
            None => Update::full(),
        },
        ContextMenuTarget::EmptyWorkspace(_) => match entry {
            workspace_entries::NEW_FLOAT => crate::actions::execute_action(ctx, Action::SpawnFloat),
            _ => workspace_entry(ctx, ctx.state.current().active_workspace, entry),
        },
        ContextMenuTarget::Session { identity, sidebar } => {
            session_entry(ctx, &identity, sidebar, entry)
        }
        ContextMenuTarget::Agent {
            location, sidebar, ..
        } => agent_entry(ctx, location, sidebar, entry),
        ContextMenuTarget::SidebarTab(id) => sidebar::tab_entry(ctx, id, entry),
        ContextMenuTarget::SidebarPanel(side, home) => sidebar::panel_entry(ctx, side, home, entry),
    };
    update.merge(Update::full())
}

/// Pane menu entry ids, shared with the view that lists them.
pub(crate) mod pane_entries {
    pub(crate) const GO: &str = "go";
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

fn pane_entry(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    entry: &str,
    selection: Option<&str>,
) -> Update {
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
        COPY => copy_text(ctx, selection),
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
    if axis.is_some() && !pane_can_split(&ctx.state, id) {
        return Update::full();
    }
    admitted(ctx, Action::Spawn, |ctx| {
        crate::pane::lifecycle::split_pane(ctx, id, axis)
    })
}

pub(crate) fn pane_can_split(state: &State, id: PaneId) -> bool {
    !state.scratch_visible
        && state.active_workspace_ref().layout_kind == crate::layout::model::LayoutKind::Dwindle
        && visible_pane(state, id).is_some_and(|pane| !pane.floating)
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
    pub(crate) const NEW: &str = "new-pane";
    pub(crate) const NEW_FLOAT: &str = "new-floating-pane";
    pub(crate) const LAYOUT_PREFIX: &str = "layout:";
    pub(crate) const SYNC: &str = "synchronize";
    pub(crate) const SWITCH: &str = "switch";
    pub(crate) const RENAME: &str = "rename";
    pub(crate) const MOVE_LEFT: &str = "move-left";
    pub(crate) const MOVE_RIGHT: &str = "move-right";
    pub(crate) const KILL: &str = "kill";
}

fn workspace_entry(ctx: &mut Context<AppRoot>, slot: usize, entry: &str) -> Update {
    use workspace_entries::*;
    if let Some(kind) = entry
        .strip_prefix(LAYOUT_PREFIX)
        .and_then(crate::layout::model::LayoutKind::from_label)
    {
        return admitted(ctx, Action::ToggleLayout, |ctx| {
            if slot == ctx.state.current().active_workspace {
                crate::ops::resize_move::set_layout(ctx, kind, false);
            } else {
                crate::ops::resize_move::set_workspace_layout(
                    &mut ctx.state.current_mut().workspaces[slot],
                    kind,
                );
            }
            Update::full()
        });
    }
    match entry {
        NEW if slot == ctx.state.current().active_workspace => {
            crate::actions::execute_action(ctx, Action::Spawn)
        }
        NEW => admitted(ctx, Action::Spawn, |ctx| {
            crate::update::sidebar::spawn_new_pane(ctx, slot)
        }),
        SYNC => admitted(ctx, Action::TogglePaneSynchronization, |ctx| {
            let workspace = &mut ctx.state.current_mut().workspaces[slot];
            workspace.synchronized = !workspace.synchronized;
            Update::full()
        }),
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
    pub(crate) const DISCONNECT: &str = "disconnect";
    pub(crate) const KILL: &str = "kill";
}

fn session_entry(
    ctx: &mut Context<AppRoot>,
    identity: &crate::state::SessionIdentity,
    sidebar: bool,
    action: &str,
) -> Update {
    use crate::ops::session;
    use session_entries::*;
    let Some(entry) = session_row(&ctx.state, identity, sidebar).map(Cow::into_owned) else {
        return Update::full();
    };
    match action {
        OPEN => {
            session::clear_pending_session_arms(ctx);
            session::activate_discovered_session(ctx, entry)
        }
        RESTART => session::restart_discovered_session(ctx, entry),
        KILL => session::kill_or_forget_session(ctx, entry),
        DISCONNECT => session::disconnect_discovered_attachment(ctx, entry),
        _ => Update::full(),
    }
}

/// Agent menu entry ids: what Enter and Ctrl+Enter do on the row.
pub(crate) mod agent_entries {
    pub(crate) const OPEN: &str = "open";
    pub(crate) const OPEN_OTHER: &str = "open-other";
}

fn agent_entry(
    ctx: &mut Context<AppRoot>,
    location: crate::state::AgentLocation,
    sidebar: bool,
    entry: &str,
) -> Update {
    let alternate = match entry {
        agent_entries::OPEN => false,
        agent_entries::OPEN_OTHER => true,
        _ => return Update::full(),
    };
    if sidebar {
        crate::ops::agents::activate_from_sidebar(ctx, location, alternate)
    } else {
        crate::ops::agents::activate(ctx, location, alternate)
    }
}

#[cfg(test)]
mod tests {
    use tui_lipan::core::event::{MouseButton, MouseKind};
    use tui_lipan::{FloatRect, KeyMods, MouseEvent, Rect, TestBackend};

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

    fn mouse(backend: &mut TestBackend<AppRoot>, kind: MouseKind, x: u16, y: u16) {
        backend
            .send_mouse(MouseEvent {
                x,
                y,
                kind,
                mods: KeyMods::NONE,
            })
            .expect("mouse input");
    }

    #[test]
    fn pane_menu_copies_the_selection_it_opened_on_and_drops_it_after_remount() {
        use std::{cell::RefCell, rc::Rc};
        use tui_lipan::{ClipboardError, ClipboardProvider};

        struct Clipboard(Rc<RefCell<String>>);
        impl ClipboardProvider for Clipboard {
            fn read_clipboard_text(&mut self) -> Result<String, ClipboardError> {
                Ok(self.0.borrow().clone())
            }

            fn write_clipboard_text(&mut self, text: &str) -> Result<(), ClipboardError> {
                self.0.replace(text.to_string());
                Ok(())
            }
        }

        crate::test_support::on_large_stack(|| {
            let copied = Rc::new(RefCell::new(String::new()));
            let app = tui_lipan::App::new().clipboard_provider(Clipboard(copied.clone()));
            let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let id = backend.state().focused_pane().expect("focused pane");
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            let terminal = crate::pane::lifecycle::find_pane_mut(state, id).expect("terminal");
            terminal.opening = false;
            terminal.terminal_active = true;
            terminal.terminal.process_server_output(b"alpha beta");
            let generation = terminal.pty_generation;
            backend.render();
            backend.pump().expect("report the initial viewport");

            // TestBackend handles terminal context menus but only the live runner produces
            // terminal drag selection events. Deliver the event the terminal would emit here.
            let selection = tui_lipan::TerminalSelection {
                anchor: tui_lipan::TerminalPos { line: 0, col: 6 },
                cursor: tui_lipan::TerminalPos { line: 0, col: 10 },
            };
            let select = |backend: &mut TestBackend<AppRoot>, selection| {
                backend
                    .dispatch(Msg::ContextMenu(ContextMenuMsg::PaneSelection(
                        id, generation, selection,
                    )))
                    .expect("terminal selection event");
            };
            select(&mut backend, Some(selection));
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Pane {
                    id,
                    anchor: (20, 3),
                }))
                .expect("open pane menu");
            select(&mut backend, None);
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Activate("copy".into())))
                .expect("copy selection from the menu");
            assert_eq!(*copied.borrow(), "beta");

            select(&mut backend, Some(selection));
            // Change only the widget key. The PTY and pane state survive its replacement.
            let terminal =
                crate::pane::lifecycle::find_pane_mut(backend.state_mut(), id).expect("pane");
            terminal.keys.terminal = "replacement-terminal".into();
            backend.render();
            backend.pump().expect("report the replacement viewport");
            assert_eq!(pane(&backend, id).mouse_selection_text(), None);
        });
    }

    #[test]
    fn a_floating_pane_in_dwindle_offers_new_pane_instead_of_splits() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let id = backend.state().focused_pane().expect("focused pane");
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            state.active_workspace_mut().layout_kind = crate::layout::model::LayoutKind::Dwindle;
            crate::pane::lifecycle::find_pane_mut(state, id)
                .expect("pane")
                .floating = true;
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Pane {
                    id,
                    anchor: (12, 3),
                }))
                .expect("open menu");
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(frame.contains("New pane"), "{frame}");
            assert!(
                !frame.contains("Split right") && !frame.contains("Split down"),
                "{frame}"
            );
        });
    }

    #[test]
    fn right_click_in_an_empty_workspace_applies_the_selected_layout() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let state = backend.state_mut();
            state.window_focused = true;
            state.current_mut().session_attached = true;
            state.current_mut().focused_pane = None;
            state.active_workspace_mut().panes.clear();
            let workspace = state.active_workspace_ref().id;
            backend.render();
            mouse(&mut backend, MouseKind::Down(MouseButton::Right), 90, 22);
            mouse(&mut backend, MouseKind::Up(MouseButton::Right), 90, 22);
            assert_eq!(
                backend
                    .state()
                    .context_menu
                    .as_ref()
                    .map(|menu| &menu.target),
                Some(&crate::state::ContextMenuTarget::EmptyWorkspace(workspace)),
            );
            backend
                .dispatch(Msg::ContextMenu(ContextMenuMsg::Activate(
                    "layout:grid".into(),
                )))
                .expect("choose Grid");
            assert_eq!(
                backend.state().active_workspace_ref().layout_kind,
                crate::layout::model::LayoutKind::Grid
            );
        });
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
