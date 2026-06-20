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

use std::sync::Arc;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{GeometryAnimation, WindowAnimationConfig};
use crate::geometry::{
    canvas_bounds_from_viewport, canvas_local_point_from_mouse, clamp_float_rect,
    clamp_floating_rect, closest_pane_to_rect, default_floating_rect, directional_score,
    grabbed_edge_on_outer_border, inset_float_rect, lift_off_float_rect,
    resize_float_rect_from_corner, tiled_drag_preview_rect,
};
use crate::input::Action;
use crate::layout::{
    insert_tiled_pane_at_point, place_spawned_pane, placement_for, target_tiled_pane_for_drop,
    workspace_target_rects, workspace_target_rects_excluding,
};
use crate::pane::PaneEventOutcome;
use crate::state::{
    Direction, HyprmuxConfig, LayoutKind, Mode, MoveSession, OUTER_GAP, Pane, PaneId, RATIO_STEP,
    ResizeCorner, ResizeSession, ScrollbackMatch, ScrollbackSearchState, State, TILE_GAP,
    ThemePreset, Workspace,
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
        apply_terminal_palette_to_state(&mut state);
        state
    }

    fn init(&mut self, ctx: &mut Context<Self>) -> Option<Command> {
        register_commands(ctx);
        for message in std::mem::take(&mut self.startup_messages) {
            ctx.toast().push(info_toast(message));
        }

        if let Some(path) = &ctx.state.config.theme.path {
            match ThemeWatcher::new(path.clone(), ctx.state.config.theme.preset.theme()) {
                Ok(watcher) => ctx.state.theme_watcher = Some(watcher),
                Err(err) => {
                    ctx.toast().push(error_toast(
                        "Theme Watcher",
                        format!("Could not watch {}: {err}", path.display()),
                    ));
                }
            }
        }

        let spawn = ctx
            .state
            .focused_pane
            .map(|id| (id, pty_config(&ctx.state.config), Some(Duration::ZERO)));
        initial_command(spawn, ctx.state.theme_watcher.is_some())
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        match msg {
            Msg::RunAction(action) => {
                ctx.state.show_palette = false;
                let update = execute_action(ctx, action);
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
                    select_theme(ctx, preset);
                    request_current_pane_focus(ctx);
                }
                Update::full()
            }
            Msg::ThemeTick => handle_theme_tick(ctx),
            Msg::ThemeError(message) => {
                ctx.toast().push(error_toast("Theme Reload", message));
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
                recompute_search(ctx)
            }
            Msg::SearchNext(backward) => search_next(ctx, backward),
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
            Msg::PruneClosed(id) => {
                remove_pane(&mut ctx.state, id);
                if ctx
                    .state
                    .search
                    .as_ref()
                    .is_some_and(|search| search.target == id)
                {
                    ctx.state.search = None;
                }
                if total_visible_panes(&ctx.state) == 0 {
                    ctx.quit();
                    return Update::none();
                }
                request_current_pane_focus(ctx);
                Update::full()
            }
            Msg::PtyReady(id, pty) => {
                let mut error = None;
                if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                    if let Err(message) = pane.terminal.set_pty(pty) {
                        error = Some(message.clone());
                        pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
                    }
                }
                if let Some(message) = error {
                    ctx.toast().push(error_toast(format!("Pane {id}"), message));
                }
                Update::full()
            }
            Msg::PtyEvent(id, event) => handle_pty_event(ctx, id, event),
            Msg::PaneInput(id, input) => handle_terminal_input(ctx, id, input),
            Msg::PaneKey(id, key) => {
                focus_pane(&mut ctx.state, id);
                let (_handled, update) = handle_key_routing(ctx, key, Some(id));
                update
            }
            Msg::PaneMouse(id, bytes) => {
                let mut error = None;
                if let Some(pane) = find_pane_mut(&mut ctx.state, id)
                    && let Err(message) = pane.terminal.send_bytes(&bytes)
                {
                    error = Some(message.clone());
                    pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
                }
                if let Some(message) = error {
                    ctx.toast().push(error_toast(format!("Pane {id}"), message));
                    Update::full()
                } else {
                    Update::none()
                }
            }
            Msg::PaneResize(id, cols, rows) => {
                if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                    match pane.terminal.resize(cols, rows) {
                        Ok(true) => Update::full(),
                        Ok(false) => Update::none(),
                        Err(message) => {
                            let toast_message = message.clone();
                            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
                            ctx.toast()
                                .push(error_toast(format!("Pane {id}"), toast_message));
                            Update::full()
                        }
                    }
                } else {
                    Update::none()
                }
            }
            Msg::PaneScroll(id, offset) => {
                if let Some(pane) = find_pane_mut(&mut ctx.state, id)
                    && pane.terminal.set_scrollback(offset)
                {
                    return Update::full();
                }
                Update::none()
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            ctx.quit();
            return KeyUpdate::handled(Update::none());
        }

        sync_focus_from_framework(ctx);
        let (handled, update) = handle_key_routing(ctx, key, None);
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

fn handle_key_routing(
    ctx: &mut Context<HyprmuxApp>,
    key: KeyEvent,
    source_pane: Option<PaneId>,
) -> (bool, Update) {
    match ctx.state.mode {
        Mode::Normal => {
            if input::is_prefix_key(key, ctx.state.config.input) {
                ctx.state.mode = Mode::Prefix;
                return (true, Update::full());
            }

            if let Some(action) = input::action_for_held(key, ctx.state.config.input) {
                return (true, execute_action(ctx, action));
            }

            if let Some(id) = source_pane {
                return (true, forward_key_to_pane(ctx, id, key));
            }

            (false, Update::none())
        }
        Mode::Prefix => {
            ctx.state.mode = Mode::Normal;
            if input::is_prefix_key(key, ctx.state.config.input) {
                let id = source_pane.or(ctx.state.focused_pane);
                let update = id
                    .map(|id| forward_key_to_pane(ctx, id, key))
                    .unwrap_or_else(Update::none);
                return (true, update);
            }

            if key.is(KeyCode::Esc) {
                return (true, Update::full());
            }

            if let Some(action) = input::action_for_prefix(key) {
                return (true, execute_action(ctx, action));
            }

            let id = source_pane.or(ctx.state.focused_pane);
            let update = id
                .map(|id| forward_key_to_pane(ctx, id, key))
                .unwrap_or_else(Update::none);
            (true, update)
        }
        Mode::Resize => handle_resize_mode_key(ctx, key),
    }
}

fn handle_resize_mode_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Esc) || key.is(KeyCode::Enter) {
        ctx.state.mode = Mode::Normal;
        request_current_pane_focus(ctx);
        return (true, Update::full());
    }

    let direction = match key.code {
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => Some(Direction::Left),
        KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => Some(Direction::Down),
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => Some(Direction::Up),
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => Some(Direction::Right),
        _ => None,
    };

    if let Some(direction) = direction {
        resize_focused_in_direction(ctx, direction);
        return (true, Update::full());
    }

    (true, Update::none())
}

fn execute_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update {
    match action {
        Action::Spawn => spawn_pane(ctx),
        Action::Close => close_focused_pane(ctx),
        Action::Focus(direction) => {
            let viewport = ctx.viewport();
            if let Some(id) = focus_in_direction(&mut ctx.state, direction, viewport) {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Action::Move(direction) => {
            move_focused_in_direction(ctx, direction);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::SwitchWorkspace(index) => {
            switch_workspace(&mut ctx.state, index);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::MoveToWorkspace(index) => {
            move_focused_to_workspace(&mut ctx.state, index);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Action::ToggleFloat => {
            toggle_tiling(ctx);
            Update::full()
        }
        Action::ToggleFullscreen => toggle_fullscreen(ctx),
        Action::FlipSplit => {
            toggle_focused_split_axis(&mut ctx.state);
            Update::full()
        }
        Action::AdjustRatio(delta) => {
            adjust_focused_split_ratio(&mut ctx.state, delta);
            Update::full()
        }
        Action::EnterResizeMode => {
            ctx.state.mode = Mode::Resize;
            ctx.state.show_help = false;
            ctx.state.show_palette = false;
            Update::full()
        }
        Action::ToggleLayout => {
            toggle_layout(ctx);
            Update::full()
        }
        Action::OpenSearch => open_search(ctx),
        Action::OpenThemePicker => open_theme_picker(ctx),
        Action::SelectTheme(preset) => {
            select_theme(ctx, preset);
            Update::full()
        }
        Action::TogglePalette => {
            ctx.state.show_palette = !ctx.state.show_palette;
            if ctx.state.show_palette {
                ctx.state.show_help = false;
            }
            Update::full()
        }
        Action::ToggleHelp => {
            ctx.state.show_help = !ctx.state.show_help;
            if ctx.state.show_help {
                ctx.state.show_palette = false;
            }
            Update::full()
        }
        Action::ToggleTitles => {
            ctx.state.show_titles = !ctx.state.show_titles;
            Update::full()
        }
    }
}

fn register_commands(ctx: &mut Context<HyprmuxApp>) {
    let registry = ctx.command_registry();
    for binding in input::command_bindings()
        .into_iter()
        .filter(|binding| binding.palette)
    {
        let action = binding.action;
        let link = ctx.link().clone();
        registry.register(
            CommandEntry::builder(binding.id)
                .label(binding.label)
                .category(binding.category)
                .keybinding(binding.keys)
                .handler(Callback::new(move |_| link.send(Msg::RunAction(action))))
                .build(),
        );
    }
}

fn initial_command(
    spawn: Option<(PaneId, TerminalPtyConfig, Option<Duration>)>,
    theme_tick: bool,
) -> Option<Command> {
    if spawn.is_none() && !theme_tick {
        return None;
    }
    Some(Command::spawn(move |link: CommandLink<Msg>| {
        if let Some((id, config, finish_open_after)) = spawn {
            spawn_pty(id, config, link.clone());
            if let Some(delay) = finish_open_after {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                link.send(Msg::FinishOpen(id));
            }
        }
        if theme_tick {
            std::thread::sleep(Duration::from_millis(150));
            link.send(Msg::ThemeTick);
        }
    }))
}

fn schedule_theme_tick() -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(Duration::from_millis(150));
        link.send(Msg::ThemeTick);
    })
}

fn handle_theme_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(watcher) = ctx.state.theme_watcher.as_ref() else {
        return Update::none();
    };

    let mut newest_theme = None;
    while let Some(theme) = watcher.try_recv() {
        newest_theme = Some(theme);
    }
    let mut errors = Vec::new();
    while let Some(err) = watcher.try_recv_error() {
        errors.push(err);
    }

    for err in errors {
        ctx.link().send(Msg::ThemeError(err));
    }

    if let Some(theme) = newest_theme {
        ctx.state.theme = theme;
        apply_terminal_palette_to_state(&mut ctx.state);
        return Update::with_command(schedule_theme_tick());
    }
    Update::command_only(schedule_theme_tick())
}

fn open_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.focused_pane else {
        return Update::full();
    };
    ctx.state.search = Some(ScrollbackSearchState::new(target));
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.mode = Mode::Normal;
    request_search_focus(ctx);
    Update::full()
}

fn open_theme_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_theme_picker = true;
    ctx.state.theme_picker_selected = ctx.state.config.theme.preset.index();
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_theme_picker_focus(ctx);
    Update::full()
}

fn recompute_search(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some((target, query)) = ctx
        .state
        .search
        .as_ref()
        .map(|search| (search.target, search.input.text().to_string()))
    else {
        return Update::none();
    };

    let query = query.trim().to_string();
    let matches: Vec<ScrollbackMatch> = if query.is_empty() {
        Vec::new()
    } else {
        find_pane_mut(&mut ctx.state, target)
            .map(|pane| {
                pane.terminal
                    .search_scrollback(&query)
                    .into_iter()
                    .map(|matched| ScrollbackMatch {
                        offset: matched.offset,
                        line: matched.line,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    if let Some(search) = ctx.state.search.as_mut() {
        search.matches = matches;
        search.current = 0;
        search.status = if query.is_empty() {
            "Type to search scrollback".to_string()
        } else if search.matches.is_empty() {
            format!("No matches for `{query}`")
        } else {
            format!("1 / {} matches", search.matches.len())
        };
    }

    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

fn search_next(ctx: &mut Context<HyprmuxApp>, backward: bool) -> Update {
    let Some(search) = ctx.state.search.as_mut() else {
        return Update::none();
    };
    if search.matches.is_empty() {
        request_search_focus(ctx);
        return Update::full();
    }
    let len = search.matches.len();
    search.current = if backward {
        search.current.checked_sub(1).unwrap_or(len - 1)
    } else {
        (search.current + 1) % len
    };
    search.status = format!("{} / {len} matches", search.current + 1);
    jump_to_search_match(ctx);
    request_search_focus(ctx);
    Update::full()
}

fn jump_to_search_match(ctx: &mut Context<HyprmuxApp>) {
    let Some((target, matched)) = ctx.state.search.as_ref().and_then(|search| {
        search
            .matches
            .get(search.current)
            .cloned()
            .map(|matched| (search.target, matched))
    }) else {
        return;
    };
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(matched.offset);
    }
}

fn info_toast(message: impl Into<String>) -> Toast {
    Toast::new(message.into()).duration(3.0)
}

fn error_toast(title: impl Into<String>, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .title(Some(title.into()))
        .duration(6.0)
        .border(true)
}

fn forward_key_to_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId, key: KeyEvent) -> Update {
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Update::none();
    };

    match pane.terminal.send_key(key) {
        Ok(result) => {
            if result.repaint {
                Update::full()
            } else {
                Update::none()
            }
        }
        Err(message) => {
            let toast_message = message.clone();
            pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
            ctx.toast()
                .push(error_toast(format!("Pane {id}"), toast_message));
            Update::full()
        }
    }
}

fn handle_pty_event(ctx: &mut Context<HyprmuxApp>, id: PaneId, event: TerminalPtyEvent) -> Update {
    let pty_error = match &event {
        TerminalPtyEvent::Error(message) => Some(message.to_string()),
        _ => None,
    };
    let (outcome, was_closing, status_text) = {
        let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
            return Update::none();
        };
        let outcome = pane.terminal.handle_pty_event(event);
        (outcome, pane.closing, pane.terminal.status_text())
    };
    match outcome {
        PaneEventOutcome::Repaint => Update::full(),
        PaneEventOutcome::StatusChanged => {
            if let Some(message) =
                pty_error.or_else(|| status_text.strip_prefix("error: ").map(str::to_string))
            {
                ctx.toast().push(error_toast(format!("Pane {id}"), message));
            }
            Update::full()
        }
        PaneEventOutcome::Exited(code) => {
            if was_closing {
                return Update::full();
            }
            ctx.toast()
                .push(info_toast(format!("Pane {id} exited with code {code}")));
            begin_close_pane(ctx, id, ctx.state.config.animations)
        }
    }
}

fn spawn_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let id = ctx.state.next_pane_id;
    ctx.state.next_pane_id = ctx.state.next_pane_id.saturating_add(1);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.terminal
        .set_palette(terminal_palette(&ctx.state.theme));
    pane.opening = true;

    let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
    let previous_focused = workspace.focused_pane;
    workspace.panes.push(pane);
    place_spawned_pane(workspace, id, previous_focused, bounds);
    workspace.focused_pane = Some(id);
    ctx.state.focused_pane = Some(id);
    request_pane_focus(ctx, id);
    ctx.state.animation = GeometryAnimation::Spawn;

    Update::with_command(spawn_pty_command(
        id,
        pty_config(&ctx.state.config),
        Some(anim::open_delay(ctx.state.config.animations)),
    ))
}

fn begin_close_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    animations: WindowAnimationConfig,
) -> Update {
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds)
    };
    let mut closed = false;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        pane.floating_rect = placement_for(&placements, id).unwrap_or(pane.floating_rect);
        pane.opening = false;
        pane.closing = true;
        pane.terminal.kill();
        closed = true;
    }

    if closed {
        ctx.state.animation = GeometryAnimation::Close;
        choose_fallback_focus(&mut ctx.state);
        request_current_pane_focus(ctx);
        Update::with_command(prune_closed_command(id, anim::close_delay(animations)))
    } else {
        Update::full()
    }
}

fn handle_terminal_input(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    if matches!(input.kind, TerminalInputKind::Key) {
        // Key input is routed through Msg::PaneKey so prefix and held-modifier
        // bindings can intercept before bytes reach the PTY. Keeping on_input
        // installed still enables bracketed paste and focus reports.
        return Update::none();
    }

    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && let Err(message) = pane.terminal.send_bytes(&input.bytes)
    {
        let toast_message = message.clone();
        pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
        ctx.toast()
            .push(error_toast(format!("Pane {id}"), toast_message));
        return Update::full();
    }
    Update::none()
}

fn close_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    begin_close_pane(ctx, id, ctx.state.config.animations)
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

fn toggle_tiling(ctx: &mut Context<HyprmuxApp>) {
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

fn toggle_fullscreen(ctx: &mut Context<HyprmuxApp>) -> Update {
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

fn toggle_focused_split_axis(state: &mut State) {
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

fn adjust_focused_split_ratio(state: &mut State, delta: f32) {
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

fn toggle_layout(ctx: &mut Context<HyprmuxApp>) {
    let workspace_index = ctx.state.active_workspace;
    let layout_kind = {
        let workspace = &mut ctx.state.workspaces[workspace_index];
        workspace.layout_kind = workspace.layout_kind.toggled();
        workspace.layout_kind
    };
    ctx.state.animation = GeometryAnimation::AxisChange;
    ctx.toast().push(info_toast(format!(
        "Workspace {} layout: {}",
        workspace_index + 1,
        layout_kind.label()
    )));
}

fn select_theme(ctx: &mut Context<HyprmuxApp>, preset: ThemePreset) {
    ctx.state.config.theme.preset = preset;
    ctx.state.config.theme.path = None;
    ctx.state.theme_watcher = None;
    ctx.state.theme = preset.theme();
    apply_terminal_palette_to_state(&mut ctx.state);
    ctx.state.show_theme_picker = false;
    ctx.toast()
        .push(info_toast(format!("Theme: {}", preset.label())));
}

fn apply_terminal_palette_to_state(state: &mut State) {
    let palette = terminal_palette(&state.theme);
    for workspace in &mut state.workspaces {
        for pane in &mut workspace.panes {
            pane.terminal.set_palette(palette);
        }
    }
}

fn terminal_palette(theme: &Theme) -> TerminalColorPalette {
    let foreground = style_fg(theme.primary).unwrap_or(Color::White);
    let background = clean_terminal_color(theme.surface.backdrop, Color::Black);
    let muted = style_fg(theme.muted).unwrap_or(theme.surface.menu);
    let accent = style_fg(theme.accent).unwrap_or(theme.border_active);
    let purple = theme.file_icons.purple;
    let cyan = theme.file_icons.cyan;

    TerminalColorPalette::new(
        foreground,
        background,
        [
            background,
            theme.status.error,
            theme.status.success,
            theme.status.warning,
            theme.status.info,
            purple,
            cyan,
            foreground,
            muted,
            theme.status.error.lighten_by(0.18),
            theme.status.success.lighten_by(0.18),
            theme.status.warning.lighten_by(0.18),
            accent.lighten_by(0.12),
            purple.lighten_by(0.18),
            cyan.lighten_by(0.18),
            foreground.lighten_by(0.12),
        ],
    )
}

fn style_fg(style: Style) -> Option<Color> {
    style
        .fg
        .map(|paint| clean_terminal_color(paint.color(), Color::Reset))
        .filter(|color| *color != Color::Reset)
}

fn clean_terminal_color(color: Color, fallback: Color) -> Color {
    match color {
        Color::Reset | Color::Backdrop | Color::Transparent => fallback,
        _ => color,
    }
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

fn move_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
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

fn resize_focused_in_direction(ctx: &mut Context<HyprmuxApp>, direction: Direction) {
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

fn focus_in_direction(state: &mut State, direction: Direction, viewport: Rect) -> Option<PaneId> {
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

fn switch_workspace(state: &mut State, index: usize) {
    if index >= state.workspaces.len() {
        return;
    }
    state.active_workspace = index;
    state.animation = GeometryAnimation::None;
    choose_fallback_focus(state);
}

fn move_focused_to_workspace(state: &mut State, target_index: usize) {
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

fn focus_pane(state: &mut State, id: PaneId) {
    if state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.closing)
    {
        state.focused_pane = Some(id);
        state.workspaces[state.active_workspace].focused_pane = Some(id);
    }
}

fn choose_fallback_focus(state: &mut State) {
    choose_fallback_focus_near(state, state.focused_pane, None);
}

fn choose_fallback_focus_near(
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

fn first_visible_pane(workspace: &Workspace) -> Option<PaneId> {
    workspace
        .panes
        .iter()
        .find(|pane| !pane.closing)
        .map(|pane| pane.id)
}

fn focus_near_pane_in_workspace(
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

fn reference_pane_rect(
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

fn find_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    state
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

fn remove_pane(state: &mut State, id: PaneId) {
    if state.moving_pane.is_some_and(|session| session.id == id) {
        state.moving_pane = None;
    }
    if state.resizing_pane.is_some_and(|session| session.id == id) {
        state.resizing_pane = None;
    }

    let removed_rect =
        reference_pane_rect(state, &state.workspaces[state.active_workspace], id, None);
    let focus_updates: Vec<(usize, Option<PaneId>)> = state
        .workspaces
        .iter()
        .enumerate()
        .filter_map(|(workspace_index, workspace)| {
            if workspace.focused_pane != Some(id) {
                return None;
            }
            Some((
                workspace_index,
                focus_near_pane_in_workspace(state, workspace, id, removed_rect)
                    .or_else(|| first_visible_pane(workspace)),
            ))
        })
        .collect();

    for workspace in &mut state.workspaces {
        remove_tiled_window(workspace, id);
        workspace.panes.retain(|pane| pane.id != id);
    }

    for (workspace_index, focus) in focus_updates {
        state.workspaces[workspace_index].focused_pane = focus;
        if workspace_index == state.active_workspace {
            state.focused_pane = focus;
        }
    }
}

fn total_visible_panes(state: &State) -> usize {
    state.workspaces.iter().map(Workspace::visible_count).sum()
}

fn framework_focused_pane(ctx: &Context<HyprmuxApp>, workspace: &Workspace) -> Option<PaneId> {
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .find(|pane| ctx.has_focus_within_key(view::pane_window_key(pane.id)))
        .map(|pane| pane.id)
}

fn sync_focus_from_framework(ctx: &mut Context<HyprmuxApp>) {
    let framework_focus = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        framework_focused_pane(ctx, workspace)
    };
    if let Some(id) = framework_focus {
        focus_pane(&mut ctx.state, id);
    }
}

fn request_pane_focus(ctx: &mut Context<HyprmuxApp>, id: PaneId) {
    ctx.request_focus(view::pane_terminal_key(id));
}

fn request_current_pane_focus(ctx: &mut Context<HyprmuxApp>) {
    if let Some(id) = ctx.state.focused_pane {
        request_pane_focus(ctx, id);
    }
}

fn request_search_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::search_input_key());
}

fn request_theme_picker_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.request_focus(view::theme_picker_key());
}

fn pty_config(config: &HyprmuxConfig) -> TerminalPtyConfig {
    let mut pty_config = if let Some(shell) = &config.shell {
        TerminalPtyConfig::new(shell.clone())
    } else {
        TerminalPtyConfig::default()
    }
    .term("xterm-256color");

    if let Some(cwd) = &config.cwd {
        pty_config = pty_config.cwd(cwd.clone());
    }

    pty_config
}

fn spawn_pty_command(
    id: PaneId,
    config: TerminalPtyConfig,
    finish_open_after: Option<Duration>,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        spawn_pty(id, config, link.clone());
        if let Some(delay) = finish_open_after {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            link.send(Msg::FinishOpen(id));
        }
    })
}

fn spawn_pty(id: PaneId, config: TerminalPtyConfig, link: CommandLink<Msg>) {
    let event_link = link.clone();
    match TerminalPty::spawn(config, move |event| {
        event_link.send(Msg::PtyEvent(id, event));
    }) {
        Ok(pty) => link.send(Msg::PtyReady(id, pty)),
        Err(err) => link.send(Msg::PtyEvent(
            id,
            TerminalPtyEvent::Error(err.to_string().into()),
        )),
    }
}

fn prune_closed_command(id: PaneId, delay: Duration) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        link.send(Msg::PruneClosed(id));
    })
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
