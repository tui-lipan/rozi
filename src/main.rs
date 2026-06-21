mod actions;
mod anim;
mod config;
mod focus_ops;
mod geometry;
mod input;
mod key_routing;
mod layout;
mod pane;
mod pane_lifecycle;
mod pty_events;
mod resize_move_ops;
mod search_ops;
mod state;
mod theme_ops;
mod tiling;
mod update;
mod view;

pub(crate) use actions::execute_action;

use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::geometry::{
    canvas_bounds_from_viewport, canvas_local_point_from_mouse, clamp_float_rect,
    clamp_floating_rect, closest_pane_to_rect, directional_score, grabbed_edge_on_outer_border,
    inset_float_rect, lift_off_float_rect, resize_float_rect_from_corner, tiled_drag_preview_rect,
};
use crate::input::Action;
use crate::layout::{
    insert_tiled_pane_at_point, placement_for, target_tiled_pane_for_drop, workspace_target_rects,
    workspace_target_rects_excluding,
};
use crate::pane_lifecycle::find_pane_mut;
use crate::pty_events::{
    handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll, handle_pty_event,
    handle_pty_ready,
};
use crate::state::{
    Direction, HyprmuxConfig, LayoutKind, MoveSession, OUTER_GAP, Pane, PaneId, RATIO_STEP,
    ResizeCorner, ResizeSession, State, TILE_GAP, ThemePreset, Workspace,
};
use crate::tiling::{
    adjust_ratio_value, adjust_tree_split_for_focused, allocate_dwindle, append_tiled_window,
    flip_tree_split_for_focused, focused_is_first_in_nearest_axis_split,
    move_tiled_window_around_target, nearest_split_available, ratio_at, remove_tiled_window,
    resize_tiled_split,
};

pub struct HyprmuxApp {
    config: HyprmuxConfig,
    initial_theme: Theme,
    startup_messages: Vec<String>,
}

impl Default for HyprmuxApp {
    fn default() -> Self {
        let config = HyprmuxConfig::default();
        Self {
            initial_theme: config.theme.preset.theme(),
            config,
            startup_messages: Vec::new(),
        }
    }
}

impl HyprmuxApp {
    fn new(config: HyprmuxConfig, initial_theme: Theme, startup_messages: Vec<String>) -> Self {
        Self {
            config,
            initial_theme,
            startup_messages,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameworkFocus {
    Preserve,
    Request,
}

#[derive(Clone)]
pub enum Msg {
    RunAction(Action),
    ClosePalette,
    CloseHelp,
    CloseThemePicker,
    ThemePickerSelected(usize),
    ThemePickerActivated(usize),
    ThemeTick,
    ThemeError(String),
    CloseSearch,
    SearchChanged(InputEvent),
    SearchNext(bool),
    FocusPane(PaneId, FrameworkFocus),
    HoverPane(PaneId),
    BeginMove(PaneId, FloatRect, u16, u16, u16, u16, bool),
    MovePane(PaneId, i16, i16, bool),
    EndMove(PaneId, u16, u16),
    BeginResize(PaneId, ResizeCorner, bool),
    ResizePane(PaneId, ResizeCorner, i16, i16, bool),
    EndResize(PaneId),
    FinishOpen(PaneId),
    PruneClosed(PaneId),
    PtyReady(PaneId, TerminalPty),
    PtyEvent(PaneId, TerminalPtyEvent),
    PaneInput(PaneId, TerminalInputEvent),
    PaneKey(PaneId, KeyEvent),
    PaneMouse(PaneId, Vec<u8>),
    PaneResize(PaneId, u16, u16),
    PaneScroll(PaneId, usize),
}

impl Component for HyprmuxApp {
    type Message = Msg;
    type Properties = ();
    type State = State;

    fn create_state(&self, _props: &Self::Properties) -> Self::State {
        let mut state = State::new(self.config.clone(), self.initial_theme.clone());
        theme_ops::apply_terminal_palette_to_state(&mut state);
        state
    }

    fn init(&mut self, ctx: &mut Context<Self>) -> Option<Command> {
        actions::register_commands(ctx);
        for message in std::mem::take(&mut self.startup_messages) {
            ctx.toast().push(pty_events::info_toast(message));
        }

        if let Some(path) = &ctx.state.config.theme.path {
            match ThemeWatcher::new(path.clone(), ctx.state.config.theme.preset.theme()) {
                Ok(watcher) => ctx.state.theme_watcher = Some(watcher),
                Err(err) => {
                    ctx.toast().push(pty_events::error_toast(
                        "Theme Watcher",
                        format!("Could not watch {}: {err}", path.display()),
                    ));
                }
            }
        }

        let spawn = ctx.state.focused_pane.map(|id| {
            (
                id,
                pane_lifecycle::pty_config(&ctx.state.config),
                Some(Duration::ZERO),
            )
        });
        pane_lifecycle::initial_command(spawn, ctx.state.theme_watcher.is_some())
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        match msg {
            Msg::RunAction(action) => {
                ctx.state.show_palette = false;
                let update = actions::execute_action(ctx, action);
                match action {
                    Action::OpenSearch => request_search_focus(ctx),
                    Action::OpenThemePicker => {}
                    _ => request_current_pane_focus(ctx),
                }
                update
            }
            Msg::ClosePalette => {
                ctx.state.show_palette = false;
                request_current_pane_focus(ctx);
                Update::full()
            }
            Msg::CloseHelp => {
                ctx.state.show_help = false;
                request_current_pane_focus(ctx);
                Update::full()
            }
            Msg::CloseThemePicker => {
                ctx.state.show_theme_picker = false;
                request_current_pane_focus(ctx);
                Update::full()
            }
            Msg::ThemePickerSelected(index) => {
                ctx.state.theme_picker_selected = index;
                Update::full()
            }
            Msg::ThemePickerActivated(index) => {
                if let Some(preset) = ThemePreset::all().get(index).copied() {
                    theme_ops::select_theme(ctx, preset);
                    request_current_pane_focus(ctx);
                }
                Update::full()
            }
            Msg::ThemeTick => theme_ops::theme_tick(ctx),
            Msg::ThemeError(message) => {
                ctx.toast().push(theme_ops::theme_error_toast(message));
                Update::full()
            }
            Msg::CloseSearch => {
                ctx.state.search = None;
                request_current_pane_focus(ctx);
                Update::full()
            }
            Msg::SearchChanged(event) => {
                if let Some(search) = ctx.state.search.as_mut() {
                    event.apply_to(&mut search.input);
                }
                search_ops::recompute_search(ctx)
            }
            Msg::SearchNext(backward) => search_ops::search_next(ctx, backward),
            Msg::FocusPane(id, framework_focus) => {
                focus_pane(&mut ctx.state, id);
                if framework_focus == FrameworkFocus::Request {
                    request_pane_focus(ctx, id);
                }
                Update::full()
            }
            Msg::HoverPane(id) => {
                if ctx.state.focused_pane != Some(id) {
                    focus_pane(&mut ctx.state, id);
                    request_pane_focus(ctx, id);
                    Update::full()
                } else {
                    Update::none()
                }
            }
            Msg::BeginMove(
                id,
                current_rect,
                from_local_x,
                from_local_y,
                target_w,
                target_h,
                modified,
            ) => begin_move(
                ctx,
                id,
                current_rect,
                from_local_x,
                from_local_y,
                target_w,
                target_h,
                modified,
            ),
            Msg::MovePane(id, dx, dy, modified) => move_pane(ctx, id, dx, dy, modified),
            Msg::EndMove(id, x, y) => end_move(ctx, id, x, y),
            Msg::BeginResize(id, corner, modified) => begin_resize(ctx, id, corner, modified),
            Msg::ResizePane(id, corner, dx, dy, modified) => {
                resize_pane(ctx, id, corner, dx, dy, modified)
            }
            Msg::EndResize(id) => {
                if ctx
                    .state
                    .resizing_pane
                    .is_some_and(|session| session.id == id)
                {
                    ctx.state.resizing_pane = None;
                }
                Update::full()
            }
            Msg::FinishOpen(id) => {
                if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                    pane.opening = false;
                    if !pane.closing {
                        ctx.state.animation = GeometryAnimation::Spawn;
                    }
                }
                Update::full()
            }
            Msg::PruneClosed(id) => pane_lifecycle::handle_prune_closed(ctx, id),
            Msg::PtyReady(id, pty) => handle_pty_ready(ctx, id, pty),
            Msg::PtyEvent(id, event) => handle_pty_event(ctx, id, event),
            Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
            Msg::PaneKey(id, key) => {
                focus_pane(&mut ctx.state, id);
                let (_handled, update) = key_routing::handle_key_routing(ctx, key, Some(id));
                update
            }
            Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
            Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
            Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            ctx.quit();
            return KeyUpdate::handled(Update::none());
        }

        key_routing::sync_focus_from_framework(ctx);
        let (handled, update) = key_routing::handle_key_routing(ctx, key, None);
        if handled {
            KeyUpdate::handled(update)
        } else {
            KeyUpdate::unhandled(update)
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        view::render(self, ctx)
    }
}

impl HyprmuxApp {
    pub(crate) fn transition_config_for(
        &self,
        ctx: &Context<Self>,
        pane: &Pane,
        viewport_changed: bool,
    ) -> TransitionConfig {
        if viewport_changed
            || ctx
                .state
                .moving_pane
                .is_some_and(|session| session.id == pane.id)
            || ctx
                .state
                .resizing_pane
                .is_some_and(|session| session.id == pane.id)
        {
            return anim::instant_transition();
        }

        let animations = self.config.animations;
        if !animations.enabled {
            return anim::instant_transition();
        }

        let enabled = match ctx.state.animation {
            GeometryAnimation::None => false,
            GeometryAnimation::Spawn => animations.spawn,
            GeometryAnimation::Close => animations.close,
            GeometryAnimation::Fullscreen => animations.fullscreen,
            GeometryAnimation::TileFloat => animations.tile_float,
            GeometryAnimation::AxisChange => animations.axis_change,
        };
        if !enabled {
            return anim::instant_transition();
        }

        let duration = if pane.closing {
            animations.close_duration
        } else {
            animations.geometry_duration
        };
        anim::geometry_transition(duration)
    }

    pub(crate) fn window_opacity_config(&self, pane: &Pane) -> TransitionConfig {
        let animations = self.config.animations;
        if !animations.enabled {
            return anim::instant_transition();
        }
        if pane.closing {
            return if animations.close {
                TransitionConfig {
                    duration: animations.close_duration,
                    easing: Easing::EaseOutQuad,
                }
            } else {
                anim::instant_transition()
            };
        }
        if animations.spawn {
            TransitionConfig {
                duration: animations.close_duration,
                easing: Easing::EaseOutQuad,
            }
        } else {
            anim::instant_transition()
        }
    }

    pub(crate) fn focus_chrome_transition_config(&self) -> TransitionConfig {
        let animations = self.config.animations;
        if animations.enabled && animations.focus_chrome {
            TransitionConfig {
                duration: animations.focus_chrome_duration,
                easing: Easing::EaseInOutCubic,
            }
        } else {
            anim::instant_transition()
        }
    }

    pub(crate) fn chrome_color(
        &self,
        ctx: &Context<Self>,
        pane: PaneId,
        slot: &str,
        target: Color,
    ) -> Color {
        ctx.transition(
            format!("hyprmux-pane-chrome-{pane}-{slot}"),
            target,
            self.focus_chrome_transition_config(),
        )
    }
}

pub(crate) fn schedule_theme_tick() -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(Duration::from_millis(150));
        link.send(Msg::ThemeTick);
    })
}

#[allow(clippy::too_many_arguments)]
fn begin_move(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    current_rect: FloatRect,
    from_local_x: u16,
    from_local_y: u16,
    target_w: u16,
    target_h: u16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let mut session = None;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        if !pane.fullscreen {
            let was_floating = pane.floating;
            let drag_rect = if was_floating {
                current_rect
            } else {
                tiled_drag_preview_rect(
                    current_rect,
                    pane.floating_rect,
                    bounds,
                    from_local_x,
                    from_local_y,
                    target_w,
                    target_h,
                )
            };
            if was_floating {
                pane.floating_rect = drag_rect;
            }
            session = Some(MoveSession {
                id,
                was_floating,
                drag_rect,
            });
        }
    }
    ctx.state.moving_pane = session;
    ctx.state.animation = if session.is_some_and(|session| !session.was_floating) {
        GeometryAnimation::TileFloat
    } else {
        GeometryAnimation::None
    };
    Update::full()
}

fn move_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let mut persisted_floating_rect = None;
    if let Some(session) = ctx
        .state
        .moving_pane
        .as_mut()
        .filter(|session| session.id == id)
    {
        session.drag_rect.x += f32::from(dx);
        session.drag_rect.y += f32::from(dy);
        session.drag_rect = if session.was_floating {
            clamp_floating_rect(session.drag_rect, bounds)
        } else {
            clamp_float_rect(session.drag_rect, bounds)
        };
        if session.was_floating {
            persisted_floating_rect = Some(session.drag_rect);
        }
        ctx.state.animation = if session.was_floating {
            GeometryAnimation::None
        } else {
            GeometryAnimation::TileFloat
        };
    }
    if let Some(rect) = persisted_floating_rect
        && let Some(pane) = active_pane_mut(&mut ctx.state, id)
    {
        pane.floating_rect = rect;
    }
    Update::full()
}

fn end_move(ctx: &mut Context<HyprmuxApp>, id: PaneId, x: u16, y: u16) -> Update {
    let session = ctx.state.moving_pane.filter(|session| session.id == id);
    if session.is_some() {
        ctx.state.moving_pane = None;
    }
    if let Some(session) = session {
        if session.was_floating {
            if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
                pane.floating_rect = session.drag_rect;
            }
        } else {
            let viewport = ctx.viewport();
            drop_tiled_pane_at(&mut ctx.state, id, x, y, viewport);
        }
    }
    Update::full()
}

fn begin_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    ctx.state.animation = GeometryAnimation::None;
    focus_pane(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    ctx.state.resizing_pane = Some(ResizeSession { id, corner });
    Update::full()
}

fn resize_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    if !modified {
        return Update::none();
    }
    ctx.state.animation = GeometryAnimation::None;
    let corner = ctx
        .state
        .resizing_pane
        .filter(|session| session.id == id)
        .map(|session| session.corner)
        .unwrap_or(corner);
    let viewport = ctx.viewport();
    resize_pane_state(&mut ctx.state, id, corner, dx, dy, viewport);
    Update::full()
}

fn resize_pane_state(
    state: &mut State,
    id: PaneId,
    corner: ResizeCorner,
    dx: i16,
    dy: i16,
    viewport: Rect,
) {
    focus_pane(state, id);
    let bounds = canvas_bounds_from_viewport(viewport);
    let Some(pane) = active_pane_mut(state, id) else {
        return;
    };

    if pane.fullscreen {
        return;
    }

    if pane.floating {
        pane.floating_rect = resize_float_rect_from_corner(
            pane.floating_rect,
            corner,
            f32::from(dx),
            f32::from(dy),
            bounds,
        );
        return;
    }

    let effective_dx = match corner {
        ResizeCorner::UpperLeft | ResizeCorner::LowerLeft => -dx,
        ResizeCorner::UpperRight | ResizeCorner::LowerRight => dx,
    };
    let effective_dy = match corner {
        ResizeCorner::UpperLeft | ResizeCorner::UpperRight => -dy,
        ResizeCorner::LowerLeft | ResizeCorner::LowerRight => dy,
    };

    if state.workspaces[state.active_workspace].layout_kind == LayoutKind::Master {
        let bounds = canvas_bounds_from_viewport(viewport);
        let tile_bounds = inset_float_rect(bounds, OUTER_GAP);
        let focused_rect = {
            let placements =
                workspace_target_rects(&state.workspaces[state.active_workspace], bounds);
            placement_for(&placements, id)
        };
        if focused_rect.is_some_and(|rect| {
            grabbed_edge_on_outer_border(rect, tile_bounds, corner, state::SplitAxis::Horizontal)
        }) {
            return;
        }
        resize_master_split_by_pixels(
            &mut state.workspaces[state.active_workspace],
            id,
            f32::from(effective_dx),
            master_available_width(tile_bounds),
        );
        state.animation = GeometryAnimation::None;
        return;
    }

    let tile_bounds = inset_float_rect(bounds, OUTER_GAP);
    let Some(tree) = layout::effective_tile_tree(&state.workspaces[state.active_workspace], None)
    else {
        return;
    };

    // The grabbed corner's edge on each axis. An edge on the terminal boundary has no
    // divider to drag, so skip resizing that axis instead of inverting the inner divider.
    let focused_rect = {
        let mut placements = Vec::new();
        allocate_dwindle(&tree, tile_bounds, TILE_GAP, &mut placements);
        placement_for(&placements, id)
    };

    for (axis, pixels) in [
        (state::SplitAxis::Horizontal, f32::from(effective_dx)),
        (state::SplitAxis::Vertical, f32::from(effective_dy)),
    ] {
        if pixels == 0.0 {
            continue;
        }
        if focused_rect.is_some_and(|r| grabbed_edge_on_outer_border(r, tile_bounds, corner, axis))
        {
            continue;
        }
        if let Some(available) = nearest_split_available(&tree, tile_bounds, TILE_GAP, id, axis) {
            resize_tiled_split(
                &mut state.workspaces[state.active_workspace],
                id,
                axis,
                available,
                pixels,
            );
        }
    }

    state.animation = GeometryAnimation::None;
}

fn drop_tiled_pane_at(state: &mut State, id: PaneId, x: u16, y: u16, viewport: Rect) {
    state.animation = GeometryAnimation::TileFloat;
    let bounds = canvas_bounds_from_viewport(viewport);
    let drop_point = canvas_local_point_from_mouse(x, y, bounds);
    let target = {
        let workspace = &state.workspaces[state.active_workspace];
        let placements = workspace_target_rects_excluding(workspace, bounds, Some(id));
        let tiled_ids: Vec<PaneId> = workspace
            .tiled_ids()
            .into_iter()
            .filter(|target_id| *target_id != id)
            .collect();
        target_tiled_pane_for_drop(&placements, &tiled_ids, drop_point).and_then(|target_id| {
            placement_for(&placements, target_id).map(|rect| (target_id, rect))
        })
    };

    let Some((target_id, target_rect)) = target else {
        return;
    };

    let (axis, moving_first) = layout::drop_split_for_target(target_rect, drop_point);
    let workspace = &mut state.workspaces[state.active_workspace];
    move_tiled_window_around_target(workspace, id, target_id, axis, moving_first);
}

pub(crate) fn toggle_tiling(ctx: &mut Context<HyprmuxApp>) {
    let Some(id) = ctx.state.focused_pane else {
        return;
    };
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let current_rect = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        placement_for(&workspace_target_rects(workspace, bounds), id)
    };

    let mut insert_tiled_at = None;
    let mut remove_from_tiling = false;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        pane.fullscreen = false;
        if pane.floating {
            pane.floating_rect = clamp_float_rect(pane.floating_rect, bounds);
            insert_tiled_at = Some(crate::geometry::rect_center(pane.floating_rect));
            pane.floating = false;
            ctx.state.animation = GeometryAnimation::TileFloat;
        } else {
            pane.floating_rect = match current_rect {
                Some(tile) => lift_off_float_rect(tile, pane.floating_rect, bounds),
                None => clamp_float_rect(pane.floating_rect, bounds),
            };
            pane.floating = true;
            remove_from_tiling = true;
            ctx.state.animation = GeometryAnimation::TileFloat;
        }
    }

    if insert_tiled_at.is_some() || remove_from_tiling {
        let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
        if let Some(point) = insert_tiled_at {
            if insert_tiled_pane_at_point(workspace, id, point, bounds).is_none() {
                append_tiled_window(workspace, id);
            }
        } else if remove_from_tiling {
            remove_tiled_window(workspace, id);
        }
    }
    request_pane_focus(ctx, id);
}

pub(crate) fn toggle_fullscreen(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds)
    };

    let mut toggled = false;
    if let Some(pane) = active_pane_mut(&mut ctx.state, id) {
        pane.opening = false;
        if !pane.fullscreen && pane.floating {
            pane.floating_rect = placement_for(&placements, id).unwrap_or(pane.floating_rect);
        }
        pane.fullscreen = !pane.fullscreen;
        toggled = true;
    }
    if toggled {
        ctx.state.animation = GeometryAnimation::Fullscreen;
        request_pane_focus(ctx, id);
    }
    Update::full()
}

pub(crate) fn toggle_focused_split_axis(state: &mut State) {
    let Some(focused) = state.focused_pane else {
        return;
    };
    let workspace = &mut state.workspaces[state.active_workspace];
    if workspace.layout_kind == LayoutKind::Master {
        return;
    }
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&focused)
    {
        return;
    }
    workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return;
    };
    if flip_tree_split_for_focused(tree, focused, 0).is_some() {
        state.animation = GeometryAnimation::AxisChange;
    }
}

pub(crate) fn adjust_focused_split_ratio(state: &mut State, delta: f32) {
    let Some(focused) = state.focused_pane else {
        return;
    };
    let workspace = &mut state.workspaces[state.active_workspace];
    if workspace.layout_kind == LayoutKind::Master {
        if adjust_master_split_for_focused(workspace, focused, delta) {
            state.animation = GeometryAnimation::None;
        }
        return;
    }
    if workspace.tile_tree.is_none() {
        workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.as_mut() else {
        return;
    };
    if adjust_tree_split_for_focused(tree, focused, delta, 0).is_some() {
        state.animation = GeometryAnimation::None;
    }
}

pub(crate) fn toggle_layout(ctx: &mut Context<HyprmuxApp>) {
    let workspace_index = ctx.state.active_workspace;
    let layout_kind = {
        let workspace = &mut ctx.state.workspaces[workspace_index];
        workspace.layout_kind = workspace.layout_kind.toggled();
        workspace.layout_kind
    };
    ctx.state.animation = GeometryAnimation::AxisChange;
    ctx.toast().push(pty_events::info_toast(format!(
        "Workspace {} layout: {}",
        workspace_index + 1,
        layout_kind.label()
    )));
}

fn adjust_master_split_for_focused(workspace: &mut Workspace, focused: PaneId, delta: f32) -> bool {
    let ids = workspace.tiled_ids();
    if ids.len() < 2 || !ids.contains(&focused) {
        return false;
    }
    let signed_delta = if ids.first() == Some(&focused) {
        delta
    } else {
        -delta
    };
    if workspace.split_ratios.is_empty() {
        workspace.split_ratios.push(crate::state::DEFAULT_RATIO);
    }
    workspace.split_ratios[0] =
        adjust_ratio_value(ratio_at(&workspace.split_ratios, 0), signed_delta);
    true
}

fn resize_master_split_by_pixels(
    workspace: &mut Workspace,
    focused: PaneId,
    pixels: f32,
    available: f32,
) -> bool {
    if pixels == 0.0 || available <= 0.0 {
        return false;
    }
    adjust_master_split_for_focused(workspace, focused, pixels / available.max(1.0))
}

fn master_available_width(tile_bounds: FloatRect) -> f32 {
    let gap = if tile_bounds.w > TILE_GAP {
        TILE_GAP
    } else {
        0.0
    };
    (tile_bounds.w - gap).max(1.0)
}

pub(crate) fn move_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let workspace_index = ctx.state.active_workspace;
    let Some(focused) = ctx.state.focused_pane else {
        return;
    };
    if active_pane_is_fullscreen(&ctx.state, focused) {
        return;
    }

    let target = {
        let workspace = &ctx.state.workspaces[workspace_index];
        let tiled_ids = workspace.active_tiled_ids_by_pane_order();
        if !tiled_ids.contains(&focused) {
            return;
        }
        let placements: Vec<_> = workspace_target_rects(workspace, bounds)
            .into_iter()
            .filter(|placement| tiled_ids.contains(&placement.id))
            .collect();
        directional_neighbor(&placements, focused, direction)
    };

    let Some(target_id) = target else {
        return;
    };
    let axis = split_axis_for_direction(direction);
    let moving_first = match direction {
        Direction::Left | Direction::Up => true,
        Direction::Right | Direction::Down => false,
    };
    let workspace = &mut ctx.state.workspaces[workspace_index];
    if move_tiled_window_around_target(workspace, focused, target_id, axis, moving_first) {
        workspace.focused_pane = Some(focused);
        ctx.state.focused_pane = Some(focused);
        ctx.state.animation = GeometryAnimation::AxisChange;
    }
}

pub(crate) fn resize_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
    let Some(focused) = ctx.state.focused_pane else {
        return;
    };
    if active_pane_is_fullscreen(&ctx.state, focused) {
        return;
    }
    let workspace_index = ctx.state.active_workspace;
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let tile_bounds = inset_float_rect(bounds, OUTER_GAP);
    let workspace = &mut ctx.state.workspaces[workspace_index];
    if !workspace
        .active_tiled_ids_by_pane_order()
        .contains(&focused)
    {
        return;
    }

    if workspace.layout_kind == LayoutKind::Master {
        let axis = split_axis_for_direction(direction);
        if axis != state::SplitAxis::Horizontal {
            return;
        }
        let available = master_available_width(tile_bounds);
        let ids = workspace.tiled_ids();
        let focused_is_first = ids.first() == Some(&focused);
        let pixels = keyboard_resize_pixels(direction, focused_is_first, available);
        if resize_master_split_by_pixels(workspace, focused, pixels, available) {
            ctx.state.animation = GeometryAnimation::None;
        }
        return;
    }

    if workspace.tile_tree.is_none() {
        workspace.tile_tree = layout::effective_tile_tree(workspace, None);
    }
    let Some(tree) = workspace.tile_tree.as_ref() else {
        return;
    };

    let axis = split_axis_for_direction(direction);
    let Some(available) = nearest_split_available(tree, tile_bounds, TILE_GAP, focused, axis)
    else {
        return;
    };
    let Some(focused_is_first) = focused_is_first_in_nearest_axis_split(tree, focused, axis) else {
        return;
    };
    let pixels = keyboard_resize_pixels(direction, focused_is_first, available);
    if resize_tiled_split(workspace, focused, axis, available, pixels) {
        ctx.state.animation = GeometryAnimation::None;
    }
}

fn keyboard_resize_pixels(direction: Direction, focused_is_first: bool, available: f32) -> f32 {
    let grows_focused = match direction {
        Direction::Left | Direction::Up => !focused_is_first,
        Direction::Right | Direction::Down => focused_is_first,
    };
    let pixels = RATIO_STEP * available;
    if grows_focused { pixels } else { -pixels }
}

fn directional_neighbor(
    placements: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let current = placements
        .iter()
        .find(|candidate| candidate.id == focused)?;
    placements
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            directional_score(current.rect, candidate.rect, direction)
                .map(|score| (candidate.id, candidate.rect, score))
        })
        .min_by(|(_, _, a), (_, _, b)| a.total_cmp(b))
        .map(|(id, _, _)| id)
}

fn split_axis_for_direction(direction: Direction) -> state::SplitAxis {
    match direction {
        Direction::Left | Direction::Right => state::SplitAxis::Horizontal,
        Direction::Up | Direction::Down => state::SplitAxis::Vertical,
    }
}

fn active_pane_is_fullscreen(state: &State, id: PaneId) -> bool {
    state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing && pane.fullscreen)
}

pub(crate) fn focus_in_direction(
    state: &mut State,
    direction: Direction,
    viewport: Rect,
) -> Option<PaneId> {
    let bounds = canvas_bounds_from_viewport(viewport);
    let workspace = &state.workspaces[state.active_workspace];
    let placements = workspace_target_rects(workspace, bounds);
    let candidates: Vec<_> = workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .filter_map(|pane| {
            placement_for(&placements, pane.id)
                .map(|rect| tiling::PanePlacement { id: pane.id, rect })
        })
        .collect();

    if candidates.is_empty() {
        state.focused_pane = None;
        return None;
    }

    let focused = state.focused_pane.unwrap_or(candidates[0].id);
    let Some(current) = candidates.iter().find(|candidate| candidate.id == focused) else {
        let id = candidates[0].id;
        focus_pane(state, id);
        return Some(id);
    };
    let next = candidates
        .iter()
        .filter(|candidate| candidate.id != focused)
        .filter_map(|candidate| {
            directional_score(current.rect, candidate.rect, direction)
                .map(|score| (candidate.id, score))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(id, _)| id)
        .or_else(|| cycle_focus_id(&candidates, focused, direction));

    if let Some(next_id) = next {
        focus_pane(state, next_id);
        Some(next_id)
    } else {
        None
    }
}

fn cycle_focus_id(
    candidates: &[tiling::PanePlacement],
    focused: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let index = candidates
        .iter()
        .position(|candidate| candidate.id == focused)
        .unwrap_or(0);
    let next_index = match direction {
        Direction::Left | Direction::Up => index
            .checked_sub(1)
            .unwrap_or_else(|| candidates.len().saturating_sub(1)),
        Direction::Right | Direction::Down => (index + 1) % candidates.len(),
    };
    candidates.get(next_index).map(|candidate| candidate.id)
}

pub(crate) fn switch_workspace(state: &mut State, index: usize) {
    if index >= state.workspaces.len() {
        return;
    }
    state.active_workspace = index;
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
}

pub(crate) fn move_focused_to_workspace(state: &mut State, target_index: usize) {
    if target_index >= state.workspaces.len() {
        return;
    }
    let source_index = state.active_workspace;
    let Some(focused) = state.focused_pane else {
        return;
    };
    if source_index == target_index {
        return;
    }

    let Some(position) = state.workspaces[source_index]
        .panes
        .iter()
        .position(|pane| pane.id == focused)
    else {
        choose_fallback_focus(state);
        return;
    };

    let mut pane = state.workspaces[source_index].panes.remove(position);
    if !pane.floating {
        remove_tiled_window(&mut state.workspaces[source_index], pane.id);
    }
    pane.opening = false;
    pane.closing = false;
    state.workspaces[target_index].focused_pane = Some(pane.id);
    if !pane.floating {
        append_tiled_window(&mut state.workspaces[target_index], pane.id);
    }
    state.workspaces[target_index].panes.push(pane);
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
}

pub(crate) fn focus_pane(state: &mut State, id: PaneId) {
    if state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing)
    {
        state.focused_pane = Some(id);
        state.workspaces[state.active_workspace].focused_pane = Some(id);
    }
}

pub(crate) fn choose_fallback_focus(state: &mut State) {
    choose_fallback_focus_near(state, state.focused_pane, None);
}

pub(crate) fn choose_fallback_focus_near(
    state: &mut State,
    reference_id: Option<PaneId>,
    reference_rect: Option<FloatRect>,
) {
    let workspace_index = state.active_workspace;
    let workspace = &state.workspaces[workspace_index];

    if let Some(focused) = state.focused_pane
        && workspace
            .panes
            .iter()
            .any(|pane| pane.id == focused && !pane.closing)
    {
        state.workspaces[workspace_index].focused_pane = Some(focused);
        return;
    }

    let focus = reference_id
        .and_then(|reference_id| {
            focus_near_pane_in_workspace(state, workspace, reference_id, reference_rect)
        })
        .or_else(|| first_visible_pane(workspace));

    state.workspaces[workspace_index].focused_pane = focus;
    state.focused_pane = focus;
}

pub(crate) fn first_visible_pane(workspace: &Workspace) -> Option<PaneId> {
    workspace
        .panes
        .iter()
        .find(|pane| !pane.closing)
        .map(|pane| pane.id)
}

pub(crate) fn focus_near_pane_in_workspace(
    state: &State,
    workspace: &Workspace,
    reference_id: PaneId,
    reference_rect: Option<FloatRect>,
) -> Option<PaneId> {
    let reference = reference_pane_rect(state, workspace, reference_id, reference_rect)?;
    let candidates: Vec<_> = visible_pane_placements(state, workspace)
        .into_iter()
        .filter(|(id, _)| *id != reference_id)
        .collect();
    closest_pane_to_rect(reference, &candidates)
}

fn visible_pane_placements(state: &State, workspace: &Workspace) -> Vec<(PaneId, FloatRect)> {
    if let Some(viewport) = state.last_viewport.get() {
        let bounds = canvas_bounds_from_viewport(viewport);
        let placements = workspace_target_rects(workspace, bounds);
        return workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .filter_map(|pane| placement_for(&placements, pane.id).map(|rect| (pane.id, rect)))
            .collect();
    }

    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .map(|pane| (pane.id, pane.floating_rect))
        .collect()
}

pub(crate) fn reference_pane_rect(
    state: &State,
    workspace: &Workspace,
    id: PaneId,
    override_rect: Option<FloatRect>,
) -> Option<FloatRect> {
    if let Some(rect) = override_rect {
        return Some(rect);
    }
    if let Some(viewport) = state.last_viewport.get() {
        let bounds = canvas_bounds_from_viewport(viewport);
        let placements = workspace_target_rects(workspace, bounds);
        if let Some(rect) = placement_for(&placements, id) {
            return Some(rect);
        }
    }
    workspace
        .panes
        .iter()
        .find(|pane| pane.id == id)
        .map(|pane| pane.floating_rect)
}

fn active_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    state.workspaces[state.active_workspace]
        .panes
        .iter_mut()
        .find(|pane| pane.id == id)
}

pub(crate) fn request_pane_focus(ctx: &mut Context<HyprmuxApp>, id: PaneId) {
    ctx.request_focus(view::pane_terminal_key(id));
}

pub(crate) fn request_current_pane_focus(ctx: &mut Context<HyprmuxApp>) {
    if let Some(id) = ctx.state.focused_pane {
        request_pane_focus(ctx, id);
    }
}

pub(crate) fn request_search_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::search_input_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::theme_picker_key());
}

fn clipboard_config(config: &HyprmuxConfig) -> ClipboardConfig {
    ClipboardConfig {
        enable_osc52: config.clipboard.enable_osc52,
        ..ClipboardConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_resize_directions_grow_toward_the_nearest_split() {
        let available = 100.0;
        let step = RATIO_STEP * available;

        assert_eq!(
            keyboard_resize_pixels(Direction::Right, true, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Left, true, available),
            -step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Left, false, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Right, false, available),
            -step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Down, true, available),
            step
        );
        assert_eq!(
            keyboard_resize_pixels(Direction::Up, false, available),
            step
        );
    }
}

fn main() -> Result<()> {
    let loaded = config::load_config();
    let loaded_theme = config::load_initial_theme(&loaded.config);
    let mut startup_messages = loaded.warnings;
    startup_messages.extend(loaded_theme.warnings);
    if loaded.found {
        startup_messages.push(format!("Loaded config from {}", loaded.path.display()));
    }
    let config = loaded.config;
    let theme = loaded_theme.theme;
    let terminal_bg = query_host_colors().map(|colors| colors.bg);

    App::new()
        .title("hyprmux")
        .theme(theme.clone())
        .terminal_bg(terminal_bg)
        .toast_placement(ToastPlacement::BottomEnd)
        .clipboard_config(clipboard_config(&config))
        .mouse(true)
        .mount(HyprmuxApp::new(config, theme, startup_messages))
        .run()
}
