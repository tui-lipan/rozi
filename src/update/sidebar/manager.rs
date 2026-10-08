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
    ctx.state.sidebar_manager_presets = false;
    ctx.state.sidebar_manager_tab = None;
    ctx.request_focus("sidebar-manager");
    Update::full()
}

pub(crate) fn manager_back(ctx: &mut Context<AppRoot>) -> Update {
    if std::mem::take(&mut ctx.state.sidebar_manager_presets)
        || ctx.state.sidebar_manager_tab.take().is_some()
    {
        ctx.request_focus(if ctx.state.sidebar_manager_tab.is_some() {
            "sidebar-manager-detail"
        } else {
            "sidebar-manager"
        });
        return Update::full();
    }
    ctx.state.sidebar_manager = false;
    crate::ops::overlay_return::finish(ctx)
}

pub(crate) fn manager_activate(ctx: &mut Context<AppRoot>, value: String) -> Update {
    if ctx.state.sidebar_manager_presets {
        let layout = if value == "default" {
            Some(crate::config::SidebarDockLayout::default())
        } else {
            ctx.state
                .config
                .sidebar
                .presets
                .iter()
                .find(|p| p.name == value)
                .map(|p| p.layout.clone())
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
        ctx.state.sidebar_manager_presets = false;
        ctx.request_focus(if ctx.state.sidebar_manager_tab.is_some() {
            "sidebar-manager-detail"
        } else {
            "sidebar-manager"
        });
        return Update::full();
    }
    let Some(id) = ctx.state.sidebar_manager_tab.clone() else {
        ctx.state.sidebar_manager_tab = Some(value);
        ctx.request_focus("sidebar-manager-detail");
        return Update::full();
    };
    match value.as_str() {
        "visibility" => {
            let hidden = &mut ctx.state.config.sidebar.layout.hidden;
            if hidden.contains(&id) {
                hidden.retain(|tab| tab != &id);
            } else {
                hidden.push(id.clone());
            }
        }
        "locate" => {
            ctx.state
                .config
                .sidebar
                .layout
                .hidden
                .retain(|tab| tab != &id);
            if let Some((side, home)) = ctx.state.config.sidebar.layout.location(&id) {
                let index = usize::from(side == SidebarPosition::Right);
                ctx.state.sidebar.dock_visible[index] = true;
                ctx.state.sidebar_visible = true;
                ctx.state
                    .sidebar
                    .apply_configured_panels(&ctx.state.config.sidebar);
                let count = ctx.state.config.sidebar.layout.dock(side).panel_count;
                if let Some(panel) = ctx
                    .state
                    .sidebar
                    .panels
                    .iter()
                    .position(|p| p.dock == side && p.home == home.min(count - 1))
                {
                    ctx.state.sidebar.active_panel = panel;
                    ctx.state.sidebar.panels[panel].active_tab =
                        Some(crate::config::SidebarTabId::new(&id));
                }
                super::save_layout(ctx);
                ctx.state.sidebar_manager = false;
                ctx.state.sidebar_manager_tab = None;
                ctx.state.overlay_return = None;
                let update = super::visibility_changed(ctx);
                ctx.state.sidebar.focused = true;
                super::refocus_body(ctx);
                return update;
            }
        }
        _ => {
            if let Some((side, panel)) = value.split_once(':')
                && let (Some(side), Ok(panel)) =
                    (SidebarPosition::parse(side), panel.parse::<usize>())
                && panel < ctx.state.config.sidebar.layout.dock(side).panel_count
            {
                ctx.state.config.sidebar.layout.place(&id, side, panel);
            } else {
                return Update::none();
            }
        }
    }
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    super::save_layout(ctx);
    let update = super::visibility_changed(ctx);
    ctx.request_focus("sidebar-manager-detail");
    update
}
