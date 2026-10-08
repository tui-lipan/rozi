mod manager;
pub(crate) use manager::*;
pub(crate) mod activation;
pub(crate) mod navigation;
pub(crate) mod polling;
pub(crate) mod sessions;
pub(crate) mod tree;
pub(crate) mod worktrees;

#[cfg(test)]
mod tests;

pub(crate) use activation::*;
pub(crate) use navigation::*;
pub(crate) use polling::*;
pub(crate) use sessions::*;
pub(crate) use tree::*;
pub(crate) use worktrees::sync_worktrees_tab;

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
        ctx.state.sidebar.layout_epoch = ctx.state.sidebar.layout_epoch.wrapping_add(1);
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
    let Some(from_panel) = crate::view::sidebar::panel_from_bar_id(&ctx.state, &event.from_bar)
    else {
        return Update::none();
    };
    let Some(to_panel) = crate::view::sidebar::panel_from_bar_id(&ctx.state, &event.to_bar) else {
        return Update::none();
    };
    if !ctx
        .state
        .sidebar
        .transfer_tab(from_panel, to_panel, event.from, event.to)
    {
        return Update::none();
    }
    ctx.state.sidebar.layout_epoch = ctx.state.sidebar.layout_epoch.wrapping_add(1);
    ctx.state.sidebar.active_panel = to_panel;
    sync_and_persist_panels(ctx);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn dock_panels_resized(
    ctx: &mut Context<AppRoot>,
    side: crate::config::SidebarPosition,
    event: SplitterResizeEvent,
) -> Update {
    resize_weights(ctx, side, event.weights)
}

fn resize_weights(
    ctx: &mut Context<AppRoot>,
    side: crate::config::SidebarPosition,
    weights: Vec<f32>,
) -> Update {
    let dock = ctx.state.config.sidebar.layout.dock_mut(side);
    if weights.len() != dock.panel_count || weights.iter().any(|w| !w.is_finite() || *w <= 0.0) {
        return Update::none();
    }
    for (index, weight) in weights.iter().enumerate() {
        let indices: Vec<_> = (index..dock.panels.len())
            .filter(|i| (*i).min(dock.panel_count - 1) == index)
            .collect();
        let total: f64 = indices
            .iter()
            .map(|i| f64::from(dock.panels[*i].weight))
            .sum();
        for i in indices {
            dock.panels[i].weight =
                ((f64::from(dock.panels[i].weight) / total * f64::from(*weight)) as f32)
                    .max(f32::MIN_POSITIVE);
        }
    }
    save_layout(ctx);
    Update::full()
}

pub(crate) fn panels_resized(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    let side = active_side(ctx);
    dock_panels_resized(ctx, side, event)
}

fn widths_from_resize_event(
    ctx: &Context<AppRoot>,
    event: &SplitterResizeEvent,
) -> [Option<u16>; 2] {
    use crate::config::SidebarPosition::{Left, Right};
    let viewport = ctx.viewport();
    let visible = [
        ctx.state.dock_reserved_width(viewport, Left) > 0,
        ctx.state.dock_reserved_width(viewport, Right) > 0,
    ];
    let handles = visible.iter().filter(|v| **v).count() as u16;
    if event.weights.len() != usize::from(handles) + 1 {
        return [None, None];
    }
    let available = viewport.w.saturating_sub(handles);
    [0, 1].map(|side| {
        if !visible[side] {
            return None;
        }
        let index = if side == 0 {
            0
        } else {
            event.weights.len().checked_sub(1)?
        };
        event
            .weights
            .get(index)
            .map(|w| ((*w * f32::from(available)).round() as u16).saturating_add(1))
    })
}

fn detect_width_drag(ctx: &mut Context<AppRoot>, event: &SplitterResizeEvent) {
    if ctx.state.sidebar.width_drag_side.is_some() {
        return;
    }
    let widths = widths_from_resize_event(ctx, event);
    for (index, side) in [
        crate::config::SidebarPosition::Left,
        crate::config::SidebarPosition::Right,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(width) = widths[index]
            && width != ctx.state.dock_reserved_width(ctx.viewport(), side)
        {
            ctx.state.sidebar.width_drag_side = Some(side);
            break;
        }
    }
}

pub(crate) fn width_resizing(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    detect_width_drag(ctx, &event);
    let widths = widths_from_resize_event(ctx, &event);
    ctx.state.sidebar.width_preview = widths[0];
    ctx.state.sidebar.right_width_preview = widths[1];
    Update::full()
}

pub(crate) fn width_resized(ctx: &mut Context<AppRoot>, event: SplitterResizeEvent) -> Update {
    detect_width_drag(ctx, &event);
    let changed_side = ctx.state.sidebar.width_drag_side.take();
    let widths = widths_from_resize_event(ctx, &event);
    ctx.state.sidebar.width_preview = None;
    ctx.state.sidebar.right_width_preview = None;
    for (index, side) in [
        crate::config::SidebarPosition::Left,
        crate::config::SidebarPosition::Right,
    ]
    .into_iter()
    .enumerate()
    {
        if changed_side == Some(side)
            && let Some(width) = widths[index]
        {
            ctx.state.config.sidebar.layout.dock_mut(side).width = width.clamp(
                crate::config::SIDEBAR_MIN_WIDTH,
                crate::config::SIDEBAR_MAX_WIDTH,
            );
        }
    }
    save_layout(ctx);
    Update::full()
}

pub(crate) fn save_layout(ctx: &mut Context<AppRoot>) {
    persist_sidebar_preference(
        ctx,
        crate::config::persist_sidebar_layout(&ctx.state.config.sidebar.layout),
    );
}

pub(crate) fn active_side(ctx: &Context<AppRoot>) -> crate::config::SidebarPosition {
    ctx.state
        .sidebar
        .active_panel()
        .map(|p| p.dock)
        .unwrap_or_default()
}

pub(crate) fn sync_and_persist_panels(ctx: &mut Context<AppRoot>) {
    let layout = &mut ctx.state.config.sidebar.layout;
    // A move changes the saved home only when it crosses a resolved panel boundary. Within a
    // compacted panel, order each home's subsequence independently; hidden/unavailable IDs keep
    // their slots. Expanding therefore restores ownership deterministically.
    for panel in &ctx.state.sidebar.panels {
        let count = layout.dock(panel.dock).panel_count;
        for id in &panel.tabs {
            if layout
                .location(id.as_str())
                .is_none_or(|(side, home)| side != panel.dock || home.min(count - 1) != panel.home)
            {
                layout.place(id.as_str(), panel.dock, panel.home);
            }
        }
        for saved in &mut layout.dock_mut(panel.dock).panels {
            let order: Vec<_> = panel
                .tabs
                .iter()
                .map(|id| id.as_str().to_string())
                .filter(|id| saved.tabs.contains(id))
                .collect();
            let mut order = order.into_iter();
            for slot in &mut saved.tabs {
                if panel.tabs.iter().any(|id| id.as_str() == slot) {
                    *slot = order.next().unwrap();
                }
            }
        }
    }
    save_layout(ctx);
}

pub(crate) fn set_panel_count(
    ctx: &mut Context<AppRoot>,
    side: crate::config::SidebarPosition,
    count: usize,
) {
    let dock = ctx.state.config.sidebar.layout.dock_mut(side);
    if dock.panel_count > 1 {
        dock.expanded_panel_count = dock.panel_count;
    }
    dock.panel_count = count.clamp(1, 3);
    if dock.panel_count > 1 {
        dock.expanded_panel_count = dock.panel_count;
    }
    dock.panels.resize_with(
        dock.panels.len().max(dock.panel_count),
        crate::config::SidebarDockPanel::default,
    );
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    save_layout(ctx);
}

pub(crate) fn persist_sidebar_preference(
    ctx: &mut Context<AppRoot>,
    result: std::result::Result<std::path::PathBuf, String>,
) {
    if let Err(error) = result {
        crate::pane::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Sidebar preference not saved".to_string()),
            error,
        );
    }
}

/// Visibility is client-local view chrome, like the active tab or a tree's expanded directories:
/// it is never written back to `config.toml`. `layout.<dock>.visible` is the startup default only, so
/// two clients sharing one config can disagree, and a toggle costs no disk write on a key that is
/// pressed constantly.
pub(crate) fn toggle_visible(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.sidebar_visible {
        ctx.state.sidebar.restore_docks = ctx.state.sidebar.dock_visible;
        ctx.state.sidebar_visible = false;
    } else {
        let restored = ctx.state.sidebar.restore_docks;
        ctx.state.sidebar.dock_visible = if restored.iter().any(|v| *v) {
            restored
        } else {
            [true, false]
        };
        ctx.state.sidebar_visible = true;
    }
    visibility_changed(ctx)
}

pub(crate) fn toggle_dock(
    ctx: &mut Context<AppRoot>,
    side: crate::config::SidebarPosition,
) -> Update {
    let index = usize::from(side == crate::config::SidebarPosition::Right);
    if !ctx.state.sidebar_visible {
        ctx.state.sidebar.dock_visible = [false, false];
    }
    ctx.state.sidebar.dock_visible[index] = !ctx.state.sidebar.dock_visible[index];
    ctx.state.sidebar_visible = ctx.state.sidebar.dock_visible.iter().any(|v| *v);
    visibility_changed(ctx)
}

/// Settings edits a startup preference and applies it to this client. Runtime commands use
/// `toggle_dock` and never change the shared configuration file.
pub(crate) fn toggle_startup_dock(
    ctx: &mut Context<AppRoot>,
    side: crate::config::SidebarPosition,
) -> Update {
    let dock = ctx.state.config.sidebar.layout.dock_mut(side);
    dock.visible = !dock.visible;
    let visible = dock.visible;
    if !ctx.state.sidebar_visible {
        ctx.state.sidebar.dock_visible = [false, false];
    }
    ctx.state.sidebar.dock_visible[usize::from(side == crate::config::SidebarPosition::Right)] =
        visible;
    ctx.state.sidebar_visible = ctx.state.sidebar.dock_visible.iter().any(|v| *v);
    save_layout(ctx);
    visibility_changed(ctx)
}

pub(crate) fn toggle_split(ctx: &mut Context<AppRoot>) -> Update {
    let side = active_side(ctx);
    let dock = ctx.state.config.sidebar.layout.dock(side);
    let count = if dock.panel_count == 1 {
        dock.expanded_panel_count
    } else {
        1
    };
    set_panel_count(ctx, side, count);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn resize_width(ctx: &mut Context<AppRoot>, handle_right: bool) -> Update {
    let side = active_side(ctx);
    let wider = if side == crate::config::SidebarPosition::Left {
        handle_right
    } else {
        !handle_right
    };
    let width = ctx
        .state
        .config
        .sidebar
        .layout
        .dock(side)
        .width
        .saturating_add_signed(if wider { 2 } else { -2 });
    set_width(ctx, width)
}

pub(crate) fn set_width(ctx: &mut Context<AppRoot>, width: u16) -> Update {
    let side = active_side(ctx);
    ctx.state.config.sidebar.layout.dock_mut(side).width = width.clamp(
        crate::config::SIDEBAR_MIN_WIDTH,
        crate::config::SIDEBAR_MAX_WIDTH,
    );
    save_layout(ctx);
    Update::full()
}

pub(crate) fn resize_panel_split(ctx: &mut Context<AppRoot>, down: bool) -> Update {
    let Some(panel) = ctx.state.sidebar.active_panel() else {
        return Update::none();
    };
    let side = panel.dock;
    let dock = ctx.state.config.sidebar.layout.dock(side);
    if dock.panel_count < 2 {
        return Update::none();
    }
    let boundary = panel.home.min(dock.panel_count - 2);
    let mut weights = dock.displayed_weights();
    let delta = if down {
        0.05_f32.min((weights[boundary + 1] - 0.1).max(0.0))
    } else {
        -0.05_f32.min((weights[boundary] - 0.1).max(0.0))
    };
    weights[boundary] += delta;
    weights[boundary + 1] -= delta;
    resize_weights(ctx, side, weights)
}
