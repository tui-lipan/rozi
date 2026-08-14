pub(crate) mod activation;
pub(crate) mod navigation;
pub(crate) mod polling;
pub(crate) mod sessions;
pub(crate) mod tree;

#[cfg(test)]
mod tests;

pub(crate) use activation::*;
pub(crate) use navigation::*;
pub(crate) use polling::*;
pub(crate) use sessions::*;
pub(crate) use tree::*;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::ToastChannel;

pub(crate) fn tab_selected(ctx: &mut Context<AppRoot>, panel: usize, index: usize) -> Update {
    let Some(id) = ctx
        .state
        .sidebar
        .panels
        .get(panel)
        .and_then(|panel| panel.tabs.get(index))
        .cloned()
    else {
        return Update::none();
    };
    if ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id() == id)
    {
        if ctx.state.sidebar.active_tab_in(panel) == Some(&id) {
            let changed_panel = ctx.state.sidebar.active_panel != panel;
            ctx.state.sidebar.active_panel = panel;
            if changed_panel {
                refocus_body(ctx);
                return Update::full();
            }
            return Update::none();
        }
        let Some(panel_state) = ctx.state.sidebar.panels.get_mut(panel) else {
            return Update::none();
        };
        if !panel_state.tabs.contains(&id) {
            return Update::none();
        }
        ctx.state.sidebar.invalidate_sessions();
        ctx.state.sidebar.invalidate_commands();
        ctx.state.sidebar.active_panel = panel;
        let panel_state = &mut ctx.state.sidebar.panels[panel];
        panel_state.active_tab = Some(id);
        // A different tab is a different row list; carrying the old index over would drop the
        // cursor somewhere arbitrary.
        panel_state.cursor = 0;
        panel_state.suppress_row_hover = true;
        panel_state.hovered_row = None;
        // Clicking the tab strip does not move focus — the strip is not focusable and the sidebar
        // is outside click-to-focus — but the body it was on unmounts, and focus goes with it. The
        // file tree feels this worst: each tree keys on its root, so even Files -> Git is a
        // remount, and without this the keyboard would be left pointing at nothing.
        refocus_body(ctx);
        arm_agent_tick(ctx);
        refresh_active_tabs(ctx)
    } else {
        Update::none()
    }
}

pub(crate) fn tab_reordered(
    ctx: &mut Context<AppRoot>,
    panel: usize,
    event: DraggableTabReorderEvent,
) -> Update {
    if !ctx.state.sidebar.reorder_tab(panel, event.from, event.to) {
        return Update::none();
    }
    sync_and_persist_panels(ctx);
    Update::layout()
}

pub(crate) fn tab_transferred(
    ctx: &mut Context<AppRoot>,
    event: DraggableTabTransferEvent,
) -> Update {
    let Some(from_panel) = crate::view::sidebar::panel_from_bar_id(&event.from_bar) else {
        return Update::none();
    };
    let Some(to_panel) = crate::view::sidebar::panel_from_bar_id(&event.to_bar) else {
        return Update::none();
    };
    if !ctx
        .state
        .sidebar
        .transfer_tab(from_panel, to_panel, event.from, event.to)
    {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = to_panel;
    sync_and_persist_panels(ctx);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn panels_resized(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    let Some(ratio) = event.weights.first().copied() else {
        return Update::none();
    };
    set_split_ratio(ctx, ratio)
}

fn width_from_resize_event(ctx: &Context<AppRoot>, event: &SplitterResizeEvent) -> Option<u16> {
    let viewport = ctx.viewport();
    let sidebar_index =
        usize::from(ctx.state.config.sidebar.position == crate::config::SidebarPosition::Right);
    let weight = event.weights.get(sidebar_index).copied()?;
    let available = viewport.w.saturating_sub(1);
    let pane_width = (weight * f32::from(available)).round() as u16;
    Some(pane_width.saturating_add(1).clamp(
        crate::config::SIDEBAR_MIN_WIDTH,
        crate::config::SIDEBAR_MAX_WIDTH,
    ))
}

pub(crate) fn width_resizing(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    let Some(width) = width_from_resize_event(ctx, &event) else {
        return Update::none();
    };
    if ctx.state.sidebar.width_preview == Some(width) {
        return Update::none();
    }
    ctx.state.sidebar.width_preview = Some(width);
    Update::full()
}

pub(crate) fn width_resized(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    let Some(width) = width_from_resize_event(ctx, &event) else {
        ctx.state.sidebar.width_preview = None;
        return Update::full();
    };
    ctx.state.sidebar.width_preview = None;
    set_width(ctx, width)
}

pub(crate) fn sync_and_persist_panels(ctx: &mut Context<AppRoot>) {
    let panels = persisted_panel_ids(
        ctx.state.sidebar.panel_ids(),
        &ctx.state.config.sidebar.panels,
        ctx.state.config.sidebar.split,
    );
    ctx.state.config.sidebar.panels = panels.clone();
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_panels(&panels));
}

pub(crate) fn persisted_panel_ids(
    mut displayed: Vec<Vec<crate::config::SidebarTabId>>,
    configured: &[Vec<crate::config::SidebarTabId>],
    split: bool,
) -> Vec<Vec<crate::config::SidebarTabId>> {
    if split || configured.len() < 2 || displayed.len() != 1 {
        return displayed;
    }

    let flat = displayed.pop().unwrap_or_default();
    let mut offset: usize = 0;
    configured
        .iter()
        .enumerate()
        .map(|(index, panel)| {
            let end = if index + 1 == configured.len() {
                flat.len()
            } else {
                offset.saturating_add(panel.len()).min(flat.len())
            };
            let tabs = flat[offset..end].to_vec();
            offset = end;
            tabs
        })
        .collect()
}

pub(crate) fn set_split_enabled(ctx: &mut Context<AppRoot>, split: bool) {
    if ctx.state.config.sidebar.split == split {
        return;
    }
    ctx.state.config.sidebar.split = split;
    if split && ctx.state.config.sidebar.panels.len() == 1 {
        ctx.state.config.sidebar.panels.push(Vec::new());
    }
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_split(split));
}

pub(crate) fn persist_sidebar_preference(
    ctx: &mut Context<AppRoot>,
    result: std::result::Result<std::path::PathBuf, String>,
) {
    if let Err(error) = result {
        crate::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Sidebar preference not saved".to_string()),
            error,
        );
    }
}

pub(crate) fn toggle_visible(ctx: &mut Context<AppRoot>) -> Update {
    set_visible(ctx, !ctx.state.sidebar_visible);
    visibility_changed(ctx)
}

pub(crate) fn set_visible(ctx: &mut Context<AppRoot>, visible: bool) {
    ctx.state.sidebar_visible = visible;
    if ctx.state.config.sidebar.visible == visible {
        return;
    }
    ctx.state.config.sidebar.visible = visible;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_visible(visible));
}

pub(crate) fn toggle_split(ctx: &mut Context<AppRoot>) -> Update {
    let split = !ctx.state.config.sidebar.split;
    set_split_enabled(ctx, split);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn resize_width(ctx: &mut Context<AppRoot>, handle_right: bool) -> Update {
    let wider = match ctx.state.config.sidebar.position {
        crate::config::SidebarPosition::Left => handle_right,
        crate::config::SidebarPosition::Right => !handle_right,
    };
    let delta = if wider { 2 } else { -2 };
    let width = ctx.state.config.sidebar.width.saturating_add_signed(delta);
    set_width(ctx, width)
}

pub(crate) fn set_width(ctx: &mut Context<AppRoot>, width: u16) -> Update {
    let width = width.clamp(
        crate::config::SIDEBAR_MIN_WIDTH,
        crate::config::SIDEBAR_MAX_WIDTH,
    );
    ctx.state.sidebar.invalidate_outer_splitter();
    if ctx.state.config.sidebar.width == width {
        return Update::layout();
    }
    ctx.state.config.sidebar.width = width;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_width(width));
    Update::full()
}

pub(crate) fn resize_panel_split(ctx: &mut Context<AppRoot>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() < 2 {
        return Update::none();
    }
    let delta = if down { 0.05 } else { -0.05 };
    set_split_ratio(ctx, ctx.state.config.sidebar.split_ratio + delta)
}

pub(crate) fn set_split_ratio(ctx: &mut Context<AppRoot>, ratio: f32) -> Update {
    let ratio = ratio.clamp(
        crate::config::SIDEBAR_MIN_SPLIT_RATIO,
        crate::config::SIDEBAR_MAX_SPLIT_RATIO,
    );
    ctx.state.sidebar.invalidate_panel_splitter();
    if (ctx.state.config.sidebar.split_ratio - ratio).abs() < 0.001 {
        return Update::layout();
    }
    ctx.state.config.sidebar.split_ratio = ratio;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_split_ratio(ratio));
    Update::full()
}
