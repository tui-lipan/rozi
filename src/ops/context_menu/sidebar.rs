use tui_lipan::prelude::*;

use crate::config::{SidebarPosition, SidebarTab, SidebarTabId};
use crate::state::SettingsAction;
use crate::{AppRoot, update::sidebar};

pub(crate) const HIDE_TAB: &str = "hide-tab";
pub(crate) const SETTINGS: &str = "settings";
pub(crate) const REFRESH: &str = "refresh";
pub(crate) const MOVE_LEFT: &str = "move-left";
pub(crate) const MOVE_RIGHT: &str = "move-right";
pub(crate) const MOVE_PREFIX: &str = "move:";
pub(crate) const SHOW_PREFIX: &str = "show:";
pub(crate) const DOCK_PREFIX: &str = "dock:";
pub(crate) const HIDE_DOCK: &str = "hide-dock";
pub(crate) const MAX_PANELS: usize = 3;

pub(crate) fn can_refresh(tab: &SidebarTab) -> bool {
    matches!(
        tab,
        SidebarTab::Sessions | SidebarTab::Tree { .. } | SidebarTab::Worktrees
    )
}

pub(super) fn tab_entry(ctx: &mut Context<AppRoot>, id: SidebarTabId, entry: &str) -> Update {
    let Some((panel, index)) = super::sidebar_tab_position(&ctx.state, &id) else {
        return Update::full();
    };
    let tab_id = id.as_str();
    if let Some((side, home)) = entry.strip_prefix(MOVE_PREFIX).and_then(move_destination) {
        let source = &ctx.state.sidebar.panels[panel];
        if (source.dock, source.home) == (side, home) {
            return Update::full();
        }
        let layout = &mut ctx.state.config.sidebar.layout;
        let dock = layout.dock_mut(side);
        dock.panel_count = dock.panel_count.max(home + 1);
        layout.place(tab_id, side, home);
        if !ctx.state.sidebar.shown[usize::from(side == SidebarPosition::Right)] {
            ctx.state.sidebar.toggle_dock(side);
        }
        return apply_layout(ctx);
    }
    match entry {
        MOVE_LEFT | MOVE_RIGHT => {
            let to = if entry == MOVE_LEFT {
                index.saturating_sub(1)
            } else {
                index + 1
            };
            if ctx.state.sidebar.reorder_tab(panel, index, to) {
                sidebar::sync_and_persist_panels(ctx);
            }
        }
        HIDE_TAB => {
            ctx.state
                .config
                .sidebar
                .layout
                .hidden
                .push(tab_id.to_string());
            return apply_layout(ctx);
        }
        SETTINGS => return crate::ops::tab_settings::open(ctx, tab_id.to_string()),
        REFRESH if ctx.state.sidebar.active_tab_in(panel) == Some(&id) => {
            match ctx.state.active_sidebar_tab(panel) {
                Some(SidebarTab::Sessions) => {
                    ctx.state.sidebar.invalidate_sessions();
                    sidebar::sessions::ensure_sessions_refresh_armed(ctx);
                }
                Some(SidebarTab::Tree { .. } | SidebarTab::Worktrees) => {
                    sidebar::tree::request_repository_refresh(ctx);
                }
                _ => {}
            }
        }
        _ => {}
    }
    Update::full()
}

pub(super) fn panel_entry(
    ctx: &mut Context<AppRoot>,
    side: SidebarPosition,
    home: usize,
    entry: &str,
) -> Update {
    if let Some(id) = entry.strip_prefix(SHOW_PREFIX) {
        let layout = &mut ctx.state.config.sidebar.layout;
        if layout.hidden.iter().any(|tab| tab == id)
            && ctx
                .state
                .config
                .sidebar
                .tabs
                .iter()
                .any(|tab| tab.id().as_str() == id)
        {
            layout.hidden.retain(|tab| tab != id);
            layout.place(id, side, home);
            return apply_layout(ctx);
        }
    }
    if let Some(dock) = entry
        .strip_prefix(DOCK_PREFIX)
        .and_then(SidebarPosition::parse)
    {
        return sidebar::toggle_dock(ctx, dock);
    }
    match entry {
        HIDE_DOCK => sidebar::toggle_dock(ctx, side),
        SETTINGS => crate::actions::open_settings(ctx, SettingsAction::ToggleSidebarGap),
        _ => Update::full(),
    }
}

fn move_destination(entry: &str) -> Option<(SidebarPosition, usize)> {
    let (side, home) = entry.split_once(':')?;
    let home = home
        .parse::<usize>()
        .ok()
        .filter(|home| *home < MAX_PANELS)?;
    Some((SidebarPosition::parse(side)?, home))
}

fn apply_layout(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    sidebar::save_layout(ctx);
    let update = sidebar::visibility_changed(ctx);
    sidebar::refocus_body(ctx);
    update
}
