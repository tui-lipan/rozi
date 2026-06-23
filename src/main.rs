mod actions;
mod anim;
mod config;
mod copy_mode;
mod focus_ops;
mod geometry;
mod identity_ops;
mod input;
mod key_routing;
mod keymap;
mod layout;
mod pane;
mod pane_lifecycle;
mod profiles;
mod pty_events;
mod resize_move_ops;
mod scratchpad;
mod search_ops;
mod state;
mod theme_ops;
mod tiling;
mod update;
mod view;

use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::input::Action;
use crate::state::{HyprmuxConfig, Pane, PaneId, ResizeCorner, State, ThemePreset};

pub struct HyprmuxApp {
    config: HyprmuxConfig,
    initial_theme: Theme,
    startup_profile: Option<profiles::HyprmuxProfile>,
    startup_messages: Vec<String>,
}

impl Default for HyprmuxApp {
    fn default() -> Self {
        let config = HyprmuxConfig::default();
        Self {
            initial_theme: config.theme.preset.theme(),
            config,
            startup_profile: None,
            startup_messages: Vec::new(),
        }
    }
}

impl HyprmuxApp {
    fn new(
        config: HyprmuxConfig,
        initial_theme: Theme,
        startup_profile: Option<profiles::HyprmuxProfile>,
        startup_messages: Vec<String>,
    ) -> Self {
        Self {
            config,
            initial_theme,
            startup_profile,
            startup_messages,
        }
    }
}

#[derive(Clone)]
pub enum Msg {
    RunAction(Action),
    ClosePalette,
    CloseHelp,
    CloseThemePicker,
    PreviewTheme(ThemePreset),
    ThemeTick,
    BarTick,
    ThemeError(String),
    CloseSearch,
    SearchChanged(InputEvent),
    SearchNext(bool),
    SearchCycleScope,
    CloseRenamePane,
    RenamePaneChanged(InputEvent),
    SubmitRenamePane,
    FocusPane(PaneId),
    HoverPane(PaneId),
    BeginMove(PaneId, FloatRect, u16, u16, u16, u16, bool),
    MovePane(PaneId, i16, i16, bool),
    EndMove(PaneId, u16, u16),
    BeginResize(PaneId, ResizeCorner, bool),
    ResizePane(PaneId, ResizeCorner, i16, i16, bool),
    EndResize(PaneId),
    /// Drag a tiled split boundary: (left/top pane, horizontal_split, dx, dy).
    ResizeSplit(PaneId, bool, i16, i16),
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
        let mut state = if let Some(profile) = self.startup_profile.clone() {
            State::from_profile(self.config.clone(), self.initial_theme.clone(), profile)
        } else {
            State::new(self.config.clone(), self.initial_theme.clone())
        };
        theme_ops::apply_terminal_palette_to_state(&mut state);
        state
    }

    fn init(&mut self, ctx: &mut Context<Self>) -> Option<Command> {
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

        pane_lifecycle::initial_command(
            startup_spawns(&ctx.state),
            ctx.state.theme_watcher.is_some(),
            ctx.state.config.bar.has_clock(),
        )
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        update::handle_msg(self, msg, ctx)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            profiles::persist_session_if_enabled(&ctx.state);
            ctx.quit();
            return KeyUpdate::handled(Update::none());
        }

        key_routing::sync_focus_from_framework(ctx);
        let (handled, mut update) = key_routing::handle_key_routing(ctx, key, None);
        if theme_ops::apply_terminal_palette_to_state(&mut ctx.state) {
            let command = update.command.take();
            update = Update::with_command(command);
        }
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

fn startup_spawns(state: &State) -> Vec<(PaneId, TerminalPtyConfig, Option<Duration>)> {
    state
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .filter(|pane| !pane.closing)
        .map(|pane| {
            (
                pane.id,
                pane_lifecycle::pty_config_for_pane(&state.config, pane),
                Some(Duration::ZERO),
            )
        })
        .collect()
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

    pub(crate) fn scratch_transition_config(&self) -> TransitionConfig {
        let animations = self.config.animations;
        if animations.enabled && animations.tile_float {
            anim::geometry_transition(animations.geometry_duration)
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
        // Only truecolor targets may fade. Named/indexed ANSI colors must be emitted
        // verbatim so the user's terminal palette resolves them; blending them animates
        // through `Color::Rgb` (`blend_toward` always returns Rgb), which bypasses the
        // palette and flips the hue mid-fade — e.g. an ANSI theme's `LightCyan` chrome
        // shows as true cyan while the focus animation runs but as the palette color at
        // rest. Snapping keeps palette themes consistent (and matching the top bar).
        let config = if chrome_color_animates(target) {
            self.focus_chrome_transition_config()
        } else {
            anim::instant_transition()
        };
        ctx.transition(format!("hyprmux-pane-chrome-{pane}-{slot}"), target, config)
    }
}

/// Whether a chrome color target is safe to fade. Only truecolor (`Color::Rgb`) targets
/// animate; named/indexed palette colors snap so the terminal palette stays in control.
pub(crate) fn chrome_color_animates(target: Color) -> bool {
    matches!(target, Color::Rgb(..))
}

pub(crate) fn schedule_theme_tick() -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(Duration::from_millis(150));
        link.send(Msg::ThemeTick);
    })
}

/// Low-frequency repaint so a configured clock segment advances. Only scheduled while a clock
/// segment is present, so an idle app with the default bar never wakes for this.
pub(crate) fn schedule_bar_tick() -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(Duration::from_secs(1));
        link.send(Msg::BarTick);
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
    let mut startup_profile =
        loaded
            .config
            .profile
            .path
            .as_ref()
            .and_then(|path| match profiles::load_profile(path) {
                Ok(profile) => {
                    startup_messages.push(format!("Loaded profile from {}", path.display()));
                    Some(profile)
                }
                Err(err) => {
                    startup_messages.push(format!("Profile load failed: {err}"));
                    None
                }
            });

    // With no explicit profile, restore the autosaved session if one exists.
    if startup_profile.is_none()
        && loaded.config.session.autosave
        && let Some(path) = profiles::session_path(&loaded.config)
        && path.exists()
    {
        match profiles::load_profile(&path) {
            Ok(profile) => {
                startup_messages.push(format!("Restored session from {}", path.display()));
                startup_profile = Some(profile);
            }
            Err(err) => startup_messages.push(format!("Session restore failed: {err}")),
        }
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
        .mount(HyprmuxApp::new(
            config,
            theme,
            startup_profile,
            startup_messages,
        ))
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    #[test]
    fn startup_spawns_include_all_non_closing_panes() {
        let mut config = HyprmuxConfig::default();
        config.shell = Some("/bin/bash".to_string());
        config.cwd = Some("/repo".into());
        let mut state = State::new(config, Theme::default());
        state.workspaces[0].panes.push(Pane::new(2, 100, rect()));
        let mut restored = Pane::new(3, 100, rect());
        restored.identity.cwd = Some("/repo/backend".to_string());
        restored.identity.command = Some("cargo run".to_string());
        state.workspaces[1].panes.push(restored);
        let mut closing = Pane::new(4, 100, rect());
        closing.closing = true;
        state.workspaces[1].panes.push(closing);

        let spawns = startup_spawns(&state);
        let ids: Vec<PaneId> = spawns.iter().map(|(id, _, _)| *id).collect();

        assert_eq!(ids, vec![1, 2, 3]);

        let restored_config = spawns
            .iter()
            .find(|(id, _, _)| *id == 3)
            .map(|(_, config, _)| format!("{config:?}"))
            .expect("restored pane spawn config");

        assert!(restored_config.contains("/bin/bash"), "{restored_config}");
        assert!(restored_config.contains("-lc"), "{restored_config}");
        assert!(restored_config.contains("cargo run"), "{restored_config}");
        assert!(
            restored_config.contains("/repo/backend"),
            "{restored_config}"
        );
    }

    #[test]
    fn chrome_color_snaps_palette_colors_but_fades_truecolor() {
        // Named/indexed colors must not animate: blending always produces Color::Rgb,
        // which bypasses the terminal palette and flips the hue mid-fade. Truecolor
        // targets are safe to fade.
        assert!(!chrome_color_animates(Color::LightCyan));
        assert!(!chrome_color_animates(Color::Black));
        assert!(!chrome_color_animates(Color::Indexed(14)));
        assert!(chrome_color_animates(Color::Rgb(0, 255, 255)));
    }
}
