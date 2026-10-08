use crate::{AppRoot, config::SidebarPosition};
use tui_lipan::prelude::*;

pub(crate) fn open_manager(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.overlay_return = ctx
        .state
        .show_settings
        .then_some(crate::state::OverlayOrigin::Settings);
    ctx.state.show_settings = false;
    ctx.state.show_palette = false;
    ctx.state.sidebar_manager = true;
    ctx.request_focus("sidebar-manager");
    Update::full()
}

pub(crate) fn manager_back(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.sidebar_manager = false;
    crate::ops::overlay_return::finish(ctx)
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
        ctx.state.sidebar.dock_visible = [layout.left.visible, layout.right.visible];
        ctx.state.sidebar_visible = ctx.state.sidebar.dock_visible.iter().any(|v| *v);
        ctx.state.config.sidebar.layout = layout;
        ctx.state
            .sidebar
            .apply_configured_panels(&ctx.state.config.sidebar);
        super::save_layout(ctx);
        let _ = super::visibility_changed(ctx);
    }
}
