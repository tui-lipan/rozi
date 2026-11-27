use tui_lipan::prelude::*;

use crate::config::{SidebarPosition, SidebarTabId};
use crate::ops::context_menu::{sidebar::*, sidebar_tab_position};
use crate::state::State;

pub(super) fn tab_menu(state: &State, id: &SidebarTabId) -> Vec<ContextMenuEntry> {
    let Some((panel, index)) = sidebar_tab_position(state, id) else {
        return Vec::new();
    };
    let source = &state.sidebar.panels[panel];
    let docks = [SidebarPosition::Left, SidebarPosition::Right].map(|side| {
        ContextMenuEntry::submenu(
            side.id(),
            format!("{} sidebar", side.label()),
            (0..MAX_PANELS).map(|home| {
                ContextMenuEntry::action(
                    format!("{MOVE_PREFIX}{}:{home}", side.id()),
                    format!("Panel {}", home + 1),
                )
                .disabled((source.dock, source.home) == (side, home))
            }),
        )
    });
    let mut entries = vec![
        ContextMenuEntry::action(MOVE_LEFT, "Move left").disabled(index == 0),
        ContextMenuEntry::action(MOVE_RIGHT, "Move right").disabled(index + 1 == source.tabs.len()),
        ContextMenuEntry::submenu("move", "Move to", docks),
        ContextMenuEntry::separator(),
    ];
    if source.active_tab.as_ref() == Some(id)
        && state.active_sidebar_tab(panel).is_some_and(can_refresh)
    {
        entries.push(ContextMenuEntry::action(REFRESH, "Refresh"));
    }
    if crate::ops::tab_settings::tab_has_settings(state, id.as_str()) {
        entries.push(ContextMenuEntry::action(SETTINGS, "Tab settings…"));
    }
    entries.push(ContextMenuEntry::action(HIDE_TAB, "Hide tab"));
    entries
}

pub(super) fn panel_menu(state: &State) -> Vec<ContextMenuEntry> {
    let hidden = &state.config.sidebar.layout.hidden;
    let tabs: Vec<_> = state
        .config
        .sidebar
        .tabs
        .iter()
        .filter_map(|tab| {
            let id = tab.id();
            hidden.iter().any(|hidden| hidden == id.as_str()).then(|| {
                ContextMenuEntry::action(
                    format!("{SHOW_PREFIX}{}", id.as_str()),
                    tab.label().to_string(),
                )
            })
        })
        .collect();
    let docks = [SidebarPosition::Left, SidebarPosition::Right].map(|side| {
        ContextMenuEntry::action(format!("{DOCK_PREFIX}{}", side.id()), side.label())
            .checked(state.sidebar.shown[usize::from(side == SidebarPosition::Right)])
    });
    let mut entries = Vec::new();
    if !tabs.is_empty() {
        entries.push(ContextMenuEntry::submenu("show", "Show tab", tabs));
    }
    entries.extend([
        ContextMenuEntry::submenu("docks", "Visible sidebars", docks),
        ContextMenuEntry::separator(),
        ContextMenuEntry::action(SETTINGS, "Sidebar settings…"),
        ContextMenuEntry::action(HIDE_DOCK, "Hide sidebar"),
    ]);
    entries
}
