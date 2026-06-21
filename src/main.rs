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
use crate::input::Action;
use crate::state::{HyprmuxConfig, Pane, PaneId, ResizeCorner, State};

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
        update::handle_msg(self, msg, ctx)
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

fn clipboard_config(config: &HyprmuxConfig) -> ClipboardConfig {
    ClipboardConfig {
        enable_osc52: config.clipboard.enable_osc52,
        ..ClipboardConfig::default()
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
