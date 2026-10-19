use crate::{AppRoot, config::SidebarPosition};
use tui_lipan::prelude::*;

pub(crate) fn open_manager(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.overlay_return = ctx
        .state
        .show_settings
        .then_some(crate::state::OverlayOrigin::Settings);
    ctx.state.show_settings = false;
    ctx.state.show_palette = false;
    ctx.state.sidebar_manager = Some(Default::default());
    ctx.request_focus("sidebar-manager");
    Update::full()
}

pub(crate) fn manager_back(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.sidebar_manager = None;
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn manager_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(manager) = ctx.state.sidebar_manager.as_mut() {
        manager.query = query;
    }
    Update::none()
}

pub(crate) fn manager_select(ctx: &mut Context<AppRoot>, id: String) -> Update {
    if let Some(manager) = ctx.state.sidebar_manager.as_mut() {
        manager.selected = Some(id);
    }
    // The settings action greys out on a tab without settings.
    Update::full()
}

/// Open the highlighted tab's settings. Sidebar tabs stays open underneath, so leaving the settings
/// returns to it on the same row.
pub(crate) fn manager_settings(ctx: &mut Context<AppRoot>) -> Update {
    let Some(tab) = ctx
        .state
        .sidebar_manager
        .as_ref()
        .and_then(|manager| manager.selected.clone())
    else {
        return Update::none();
    };
    crate::ops::tab_settings::open(ctx, tab)
}

pub(crate) fn manager_activate(ctx: &mut Context<AppRoot>, id: String) -> Update {
    // Only registered tabs can be toggled. Unavailable definitions retain all preferences.
    if !ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id().as_str() == id)
    {
        return Update::none();
    }
    let hidden = &mut ctx.state.config.sidebar.layout.hidden;
    if hidden.contains(&id) {
        hidden.retain(|tab| tab != &id);
    } else {
        hidden.push(id);
    }
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    super::save_layout(ctx);
    let update = super::visibility_changed(ctx);
    ctx.request_focus("sidebar-manager");
    update
}

pub(crate) fn apply_layout_preset(ctx: &mut Context<AppRoot>, index: usize) {
    let layout = if index == 0 {
        Some(crate::config::SidebarDockLayout::default())
    } else {
        ctx.state
            .config
            .sidebar
            .presets
            .get(index - 1)
            .map(|preset| preset.layout.clone())
    };
    if let Some(mut layout) = layout {
        // Applying a preset is explicit. Definitions absent from it keep their saved homes;
        // availability is resolved independently, so an absent extension loses nothing.
        let old = &ctx.state.config.sidebar.layout;
        for side in [SidebarPosition::Left, SidebarPosition::Right] {
            for (home, panel) in old.dock(side).panels.iter().enumerate() {
                for id in &panel.tabs {
                    if layout.location(id).is_none() {
                        layout.place(id, side, home);
                    }
                }
            }
        }
        ctx.state.config.sidebar.layout = layout;
        ctx.state
            .sidebar
            .apply_configured_panels(&ctx.state.config.sidebar);
        super::save_layout(ctx);
        let _ = super::visibility_changed(ctx);
    }
}
