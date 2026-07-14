use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::anim::GeometryAnimation;
use crate::config::HyprmuxConfig;
use crate::session::bootstrap::{SessionStart, attach_session_client, has_named_session};
use crate::state::{Pane, PaneId, State, ThemePreset};
use crate::{
    anim, cli, commands, config, control, events, key_routing, ops, pane_lifecycle, platform,
    profiles, pty_events, state, update, view,
};

pub struct HyprmuxApp {
    config: HyprmuxConfig,
    initial_theme: Theme,
    initial_system_theme: Option<Theme>,
    startup_profile: Option<StartupProfile>,
    startup_messages: Vec<String>,
    control_listener: Option<crate::platform::ipc::IpcListener>,
    control_guard: Option<control::ControlSocketGuard>,
    attach_session: Option<String>,
    read_only: bool,
    /// Whether a bare launch should open the session picker before attaching (`--pick` or
    /// `[session] startup = "picker"`). Only honored when there is no `--attach`/`--session` and at
    /// least one named session exists at startup.
    want_startup_picker: bool,
    /// Whether to install the process-global terminal-hangup handler
    /// ([`platform::server_lifecycle::on_hangup`]) at startup. Only the real [`run`] wants this:
    /// a `TestBackend`-driven test constructs its app through [`Default`], and a test process must
    /// not have its `SIGHUP`/`SIGTERM` disposition rewritten out from under the harness (nor can
    /// several parallel tests each claim the one install slot).
    watch_hangup: bool,
    event_hub: events::EventHub,
}

#[derive(Clone)]
struct StartupProfile {
    profile: profiles::HyprmuxProfile,
    name: String,
    path: PathBuf,
}

impl Default for HyprmuxApp {
    fn default() -> Self {
        let config = HyprmuxConfig::default();
        Self {
            initial_theme: ThemePreset::Lipan.theme(),
            initial_system_theme: None,
            config,
            startup_profile: None,
            startup_messages: Vec::new(),
            control_listener: None,
            control_guard: None,
            attach_session: None,
            read_only: false,
            want_startup_picker: false,
            watch_hangup: false,
            event_hub: events::EventHub::default(),
        }
    }
}

impl HyprmuxApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: HyprmuxConfig,
        initial_theme: Theme,
        initial_system_theme: Option<Theme>,
        startup_profile: Option<StartupProfile>,
        startup_messages: Vec<String>,
        control_listener: Option<crate::platform::ipc::IpcListener>,
        control_guard: Option<control::ControlSocketGuard>,
        attach_session: Option<String>,
        read_only: bool,
        want_startup_picker: bool,
    ) -> Self {
        Self {
            config,
            initial_theme,
            initial_system_theme,
            startup_profile,
            startup_messages,
            control_listener,
            control_guard,
            attach_session,
            read_only,
            want_startup_picker,
            watch_hangup: true,
            event_hub: events::EventHub::default(),
        }
    }
}

impl Component for HyprmuxApp {
    type Message = Msg;
    type Properties = ();
    type State = State;

    fn create_state(&self, _props: &Self::Properties) -> Self::State {
        let mut state = if let Some(startup) = self.startup_profile.clone() {
            State::from_profile(
                self.config.clone(),
                self.initial_theme.clone(),
                startup.profile,
            )
        } else {
            State::new(self.config.clone(), self.initial_theme.clone())
        };
        state.system_theme = self.initial_system_theme.clone();
        state.control_socket_path = self
            .control_guard
            .as_ref()
            .map(|guard| guard.path().to_path_buf());
        state.event_hub = self.event_hub.clone();
        if let Some(startup) = &self.startup_profile {
            events::emit(
                &state,
                events::Event::new(
                    events::EventKind::ProfileLoaded,
                    vec![
                        ("profile", startup.name.clone()),
                        ("path", startup.path.display().to_string()),
                    ],
                ),
            );
        }
        ops::theme::apply_terminal_palette_to_state(&mut state);
        state
    }

    fn init(&mut self, ctx: &mut Context<Self>) -> Option<Command> {
        commands::sync(ctx);

        for message in std::mem::take(&mut self.startup_messages) {
            ctx.toast()
                .push(pty_events::info_toast(&ctx.state.theme, message));
        }

        if let Some(path) =
            config::resolve_choice(&ctx.state.config.theme.name).and_then(|choice| match choice {
                config::ThemeChoice::Custom { path, .. } => Some(path),
                _ => None,
            })
        {
            match ThemeWatcher::new(path, ThemePreset::Lipan.theme()) {
                Ok(watcher) => ctx.state.theme_watcher = Some(watcher),
                Err(err) => {
                    ctx.toast().push(pty_events::error_toast(
                        &ctx.state.theme,
                        "Theme Watcher",
                        format!("Can't watch theme file: {err}"),
                    ));
                }
            }
        }

        // Always-server model: with no explicit `--attach`, attach to this process's ephemeral
        // session (`eph-<pid>`), autostarting its server. Restored/initial panes are spawned on
        // the server once `Msg::SessionAttached` reports an empty session. Opt-in `--pick` /
        // `[session] startup = "picker"` instead opens the session picker first when a named
        // session exists, so nothing is attached until the user chooses.
        let control_listener = self.control_listener.take();
        let event_hub = self.event_hub.clone();
        let theme_tick = ctx.state.theme_watcher.is_some();
        let workbar_tick = ctx.state.config.workbar.has_clock();
        let workbar_commands = ctx.state.config.workbar.command_specs();
        ctx.state.workbar_commands_running =
            workbar_commands.iter().map(|(c, _)| c.clone()).collect();
        let command_shell = crate::platform::command::resolve_command_shell(
            ctx.state.config.command_shell.as_deref(),
            &crate::platform::command::ShellEnv::from_process(),
        )
        .as_argv();

        let start = if self.want_startup_picker && has_named_session() {
            let epoch = ops::session::open_startup_session_picker(ctx);
            SessionStart::Picker { epoch }
        } else {
            let name = self
                .attach_session
                .clone()
                .unwrap_or_else(state::ephemeral_session_name);
            let epoch = ctx.state.runtime_epoch;
            ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch,
                name: name.clone(),
                client: None,
                autostart: true,
                read_only: self.read_only,
            });
            SessionStart::Attach { epoch, name }
        };

        let startup_read_only = self.read_only;
        let watch_hangup = self.watch_hangup;
        Some(Command::spawn(move |link: CommandLink<Msg>| {
            link.send(Msg::CommandLinkReady(link.clone()));
            ops::config::spawn_config_watcher(&link);
            if watch_hangup {
                let hangup_link = link.clone();
                if let Err(err) = platform::server_lifecycle::on_hangup(move || {
                    hangup_link.send(Msg::Hangup);
                }) {
                    // Not fatal: without it, a closing terminal kills the client outright, which
                    // loses the detach-time layout mirror but leaves a named session's server (and
                    // its PTYs) running exactly as before.
                    eprintln!("hyprmux: could not watch for terminal hangup: {err}");
                }
            }
            if let Some(listener) = control_listener {
                let listener_link = link.clone();
                std::thread::spawn(move || {
                    crate::control::run_listener(listener, listener_link, event_hub)
                });
            }
            match start {
                SessionStart::Attach { epoch, name } => {
                    let session_link = link.clone();
                    std::thread::spawn(move || {
                        attach_session_client(epoch, name, true, startup_read_only, session_link)
                    });
                }
                SessionStart::Picker { epoch } => {
                    // Kick off the first discovery tick; `apply_discovered_sessions` re-arms the
                    // auto-refresh loop from there, exactly as an in-app picker opening would.
                    let watch_link = link.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(1500));
                        if let Ok(rows) =
                            crate::session::discovery::discover_sessions_excluding(None)
                        {
                            watch_link.send(Msg::SessionsDiscovered { epoch, rows });
                        }
                    });
                }
            }
            if theme_tick {
                std::thread::sleep(std::time::Duration::from_millis(150));
                link.send(Msg::ThemeTick);
            }
            if workbar_tick {
                link.send(Msg::WorkbarTick);
            }
            pane_lifecycle::spawn_workbar_command_pollers(workbar_commands, command_shell, &link);
        }))
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        update::handle_msg(self, msg, ctx)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        key_routing::sync_focus_from_framework(ctx);
        let (handled, mut update) = key_routing::handle_key_routing(ctx, key, None);
        if ops::theme::apply_terminal_palette_to_state(&mut ctx.state) {
            let command = update.command.take();
            update = Update::with_command(command);
        }
        if ctx.state.commands_dirty {
            ctx.state.commands_dirty = false;
            commands::sync(ctx);
        }
        // Key routing can mutate the layout without going through `handle_msg` (prefix-mode window
        // management), so schedule the same commit chokepoint here to publish those changes.
        update::schedule_layout_commit(ctx);
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
        if !ctx.state.is_controller()
            || viewport_changed
            || ctx
                .state
                .moving_pane
                .is_some_and(|session| session.id == pane.id)
            || ctx
                .state
                .resizing_pane
                .as_ref()
                .is_some_and(|session| session.id == pane.id)
        {
            return anim::instant_transition();
        }

        let animations = ctx.state.config.animations;
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

    pub(crate) fn window_opacity_config(
        &self,
        ctx: &Context<Self>,
        pane: &Pane,
    ) -> TransitionConfig {
        let animations = ctx.state.config.animations;
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
                duration: animations.geometry_duration,
                easing: Easing::EaseOutQuad,
            }
        } else {
            anim::instant_transition()
        }
    }

    pub(crate) fn scratch_transition_config(&self, ctx: &Context<Self>) -> TransitionConfig {
        let animations = ctx.state.config.animations;
        if animations.enabled && animations.tile_float {
            anim::geometry_transition(anim::scratch_transition_duration(
                animations.geometry_duration,
            ))
        } else {
            anim::instant_transition()
        }
    }

    pub(crate) fn focus_chrome_transition_config(&self, ctx: &Context<Self>) -> TransitionConfig {
        let animations = ctx.state.config.animations;
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
        // palette and flips the hue mid-fade - e.g. an ANSI theme's `LightCyan` chrome
        // shows as true cyan while the focus animation runs but as the palette color at
        // rest. Snapping keeps palette themes consistent (and matching the workbar).
        let config = if chrome_color_animates(target) {
            self.focus_chrome_transition_config(ctx)
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
/// segment is present, so an idle app with the default workbar never wakes for this.
pub(crate) fn schedule_workbar_tick() -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::sleep(Duration::from_secs(1));
        link.send(Msg::WorkbarTick);
    })
}

fn clipboard_config(config: &HyprmuxConfig) -> ClipboardConfig {
    ClipboardConfig {
        enable_osc52: config.clipboard.enable_osc52,
        ..ClipboardConfig::default()
    }
}

pub fn run() -> Result<()> {
    // Checked before anything else: on a host with no ConPTY there is no pane hyprmux could open,
    // and saying so once is far kinder than failing on every spawn (cross-platform plan Phase 10).
    if let Err(reason) = platform::server_lifecycle::check_host_supported() {
        eprintln!("hyprmux: {reason}");
        std::process::exit(1);
    }

    let cli = match cli::parse_cli_args(std::env::args().skip(1).collect()) {
        Ok(cli::ParsedCli::Help) => {
            cli::print_help();
            return Ok(());
        }
        Ok(cli::ParsedCli::Version) => {
            cli::print_version();
            return Ok(());
        }
        Ok(cli::ParsedCli::Control(command)) => return cli::run_control_cli(command),
        Ok(cli::ParsedCli::Server { name }) => return cli::run_server_cli(&name),
        Ok(cli::ParsedCli::ListSessions) => return cli::run_list_sessions_cli(),
        Ok(cli::ParsedCli::KillSession { name }) => return cli::run_kill_session_cli(&name),
        Ok(cli::ParsedCli::Run(args)) => args,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("Run `hyprmux --help` for usage.");
            std::process::exit(1);
        }
    };

    if let Some(path) = cli.config_path {
        unsafe {
            std::env::set_var("HYPRMUX_CONFIG", path);
        }
    }

    let loaded = config::load_config();
    let mut startup_messages = loaded.warnings;
    let mut startup_profile = cli.profile.as_ref().and_then(|name| {
        let path = config::profile_path_for_name(name);
        match profiles::load_profile(&path) {
            Ok(profile) => Some(StartupProfile {
                profile,
                name: name.clone(),
                path,
            }),
            Err(err) => {
                startup_messages.push(format!("Profile `{name}` load failed: {err}"));
                None
            }
        }
    });

    if startup_profile.is_none()
        && let Some(name) = &loaded.config.profile.default
    {
        let path = config::profile_path_for_name(name);
        match profiles::load_profile(&path) {
            Ok(profile) => {
                startup_profile = Some(StartupProfile {
                    profile,
                    name: name.clone(),
                    path,
                })
            }
            Err(err) => {
                startup_messages.push(format!("Default profile `{name}` load failed: {err}"))
            }
        }
    }

    // With no explicit profile, restore the autosaved session if one exists.
    if startup_profile.is_none()
        && loaded.config.session.autosave
        && let Some(path) = profiles::session_path(&loaded.config)
        && path.exists()
    {
        match profiles::load_profile(&path) {
            Ok(profile) => {
                startup_profile = Some(StartupProfile {
                    profile,
                    name: path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("session")
                        .to_string(),
                    path,
                });
            }
            Err(err) => startup_messages.push(format!("Session restore failed: {err}")),
        }
    }
    let config = loaded.config;
    let attach_session = cli.attach_session.clone();
    // Open the picker at startup only for a bare launch (no explicit attach target). The
    // "any named session exists" gate is checked in `init` so it reflects live state at mount.
    let want_startup_picker = attach_session.is_none()
        && (cli.pick || config.session.startup == config::SessionStartup::Picker);
    let startup_host_colors = query_host_colors();
    let terminal_bg = startup_host_colors.map(|colors| colors.bg);
    let startup_system_theme = startup_host_colors.map(ops::theme::system_theme_from_host_colors);
    let resolved_theme = config::resolve_theme(&config.theme.name, startup_system_theme.as_ref());
    startup_messages.extend(resolved_theme.warnings);
    let theme = ops::theme::apply_backdrop_policy(
        resolved_theme.theme,
        terminal_bg,
        config.pane.background_follows_terminal,
    );

    let (control_listener, control_guard) = match control::bind_control_socket() {
        Ok((listener, guard)) => (Some(listener), Some(guard)),
        Err(err) => {
            startup_messages.push(format!("Control socket unavailable: {err}"));
            (None, None)
        }
    };

    let app = App::new()
        .title("hyprmux")
        .theme(theme.clone())
        .terminal_bg(terminal_bg)
        .toast_placement(ToastPlacement::BottomEnd)
        .toast_margin((1, 2, 1, 1))
        .clipboard_config(clipboard_config(&config))
        .mouse(true)
        // Leader chords (`ctrl-a c`) and WM-modifier chords (`alt-c`) are executable command
        // shortcuts (see `commands.rs`), not a framework keymap file - resolve them ahead of
        // focused widgets/terminal passthrough so they win regardless of what has focus.
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
        // Ctrl-q is unbound: hyprmux's own `quit`/`detach` commands own client lifecycle exits.
        .global_quit(None);

    app.mount(HyprmuxApp::new(
        config,
        theme,
        startup_system_theme,
        startup_profile,
        startup_messages,
        control_listener,
        control_guard,
        attach_session,
        cli.read_only,
        want_startup_picker,
    ))
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::{TestBackend, UiSnapshotOptions, UiWidgetKind};

    #[test]
    fn command_palette_modal_is_capped_to_sixty_five_percent_of_viewport() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                backend.state_mut().show_palette = true;
                backend.render();

                let snapshot =
                    backend.capture_ui_snapshot_with_options(&UiSnapshotOptions::default());
                let modal = snapshot
                    .widgets
                    .iter()
                    .find(|widget| {
                        widget.kind == UiWidgetKind::Frame
                            && widget.title.as_deref() == Some("Commands")
                    })
                    .expect("commands modal frame");
                let content_frame = snapshot
                    .widgets
                    .iter()
                    .find(|widget| {
                        widget.kind == UiWidgetKind::Frame
                            && widget.title.is_none()
                            && widget.rect.x >= modal.rect.x
                            && widget.rect.y > modal.rect.y
                            && widget.rect.w <= modal.rect.w
                            && widget.rect.h <= 26
                    })
                    .unwrap_or_else(|| {
                        panic!("commands palette content frame\n{}", snapshot.to_markdown())
                    });

                assert!(
                    modal.rect.h <= 26,
                    "commands modal height {} exceeded 65% of 40-row viewport\n{}",
                    modal.rect.h,
                    snapshot.to_markdown()
                );
                assert!(
                    content_frame.rect.h <= 26,
                    "commands content frame height {} exceeded 65% of 40-row viewport\n{}",
                    content_frame.rect.h,
                    snapshot.to_markdown()
                );
            })
            .expect("spawn snapshot test thread")
            .join()
            .expect("snapshot test thread completes");
    }

    #[test]
    fn terminal_padding_editor_test_backend_flow_and_bounds() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                backend.state_mut().show_appearance = true;
                backend
                    .dispatch(Msg::AppearanceActivate(
                        crate::state::AppearanceAction::EditPadding,
                    ))
                    .expect("open padding editor");

                assert_eq!(
                    backend.focused_key().map(|key| key.as_ref()),
                    Some(crate::view::pane_padding_vertical_key())
                );
                let editor = backend
                    .state()
                    .pane_padding_editor
                    .as_ref()
                    .expect("editor state");
                assert_eq!(editor.vertical.selection(), Some((0, 1)));

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("advance to horizontal");
                assert_eq!(
                    backend.focused_key().map(|key| key.as_ref()),
                    Some(crate::view::pane_padding_horizontal_key())
                );
                assert_eq!(
                    backend
                        .state()
                        .pane_padding_editor
                        .as_ref()
                        .unwrap()
                        .horizontal
                        .selection(),
                    Some((0, 1))
                );

                // A pasted multi-character value is rejected without replacing the selected value.
                backend.send_paste("12").expect("paste reaches input");
                assert_eq!(
                    backend
                        .state()
                        .pane_padding_editor
                        .as_ref()
                        .unwrap()
                        .horizontal
                        .text(),
                    "0"
                );

                let snapshot =
                    backend.capture_ui_snapshot_with_options(&UiSnapshotOptions::default());
                let frames: Vec<_> = snapshot
                    .widgets
                    .iter()
                    .filter(|w| w.kind == UiWidgetKind::Frame)
                    .collect();
                let appearance = frames
                    .iter()
                    .position(|w| w.title.as_deref() == Some("Change appearance"))
                    .expect("appearance frame");
                let padding = frames
                    .iter()
                    .position(|w| w.title.as_deref() == Some("Terminal padding"))
                    .expect("padding frame");
                assert!(
                    padding > appearance,
                    "padding editor must be the topmost modal"
                );
                let rect = frames[padding].rect;
                assert!(
                    rect.w <= 46 && rect.x + rect.w as i16 <= 96 && rect.y + rect.h as i16 <= 40,
                    "editor must fit wide viewport"
                );

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Esc,
                        mods: KeyMods::NONE,
                    })
                    .expect("cancel editor");
                assert_eq!(backend.state().config.pane.padding, (0, 0, 0, 0));
                assert!(backend.state().pane_padding_editor.is_none());

                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 28,
                    h: 14,
                });
                backend
                    .dispatch(Msg::AppearanceActivate(
                        crate::state::AppearanceAction::EditPadding,
                    ))
                    .expect("reopen editor");
                let snapshot =
                    backend.capture_ui_snapshot_with_options(&UiSnapshotOptions::default());
                let rect = snapshot
                    .widgets
                    .iter()
                    .find(|w| {
                        w.kind == UiWidgetKind::Frame
                            && w.title.as_deref() == Some("Terminal padding")
                    })
                    .expect("narrow editor frame")
                    .rect;
                assert!(
                    rect.x + rect.w as i16 <= 28 && rect.y + rect.h as i16 <= 14,
                    "editor must fit narrow viewport"
                );

                backend.state_mut().config.pane.padding = (1, 2, 3, 4);
                backend
                    .dispatch(Msg::AppearanceActivate(
                        crate::state::AppearanceAction::EditPadding,
                    ))
                    .expect("open asymmetric editor");
                let editor = backend.state().pane_padding_editor.as_ref().unwrap();
                assert!(editor.vertical.text().is_empty() && editor.horizontal.text().is_empty());
                assert!(editor.normalizes_asymmetric);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread completes");
    }

    #[test]
    fn command_palette_shrinks_to_filtered_matches_without_moving_its_top() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                backend.state_mut().show_palette = true;
                backend.render();

                let commands_modal = |backend: &TestBackend<HyprmuxApp>| {
                    backend
                        .capture_ui_snapshot_with_options(&UiSnapshotOptions::default())
                        .widgets
                        .iter()
                        .find(|w| {
                            w.kind == UiWidgetKind::Frame && w.title.as_deref() == Some("Commands")
                        })
                        .expect("commands modal frame")
                        .rect
                };

                // Unfiltered: the full command list overflows, so the modal is capped at 65% of
                // the 40-row viewport (26 rows).
                let unfiltered = commands_modal(&backend);
                assert_eq!(unfiltered.h, 26, "unfiltered modal should hit the 65% cap");

                // Type a query that narrows to a couple of matches.
                for c in ['q', 'u', 'i', 't'] {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Char(c),
                            mods: KeyMods::NONE,
                        })
                        .expect("send key");
                }
                backend.render();

                let filtered = commands_modal(&backend);
                // The modal hugs the filtered rows (well under the cap)...
                assert!(
                    filtered.h < unfiltered.h,
                    "filtered modal height {} should shrink below the capped {}",
                    filtered.h,
                    unfiltered.h
                );
                // ...while its top edge stays put instead of re-centering.
                assert_eq!(
                    filtered.y, unfiltered.y,
                    "filtered modal top drifted from {} to {}",
                    unfiltered.y, filtered.y
                );
            })
            .expect("spawn snapshot test thread")
            .join()
            .expect("snapshot test thread completes");
    }

    #[test]
    fn session_picker_shows_clients_on_other_sessions_and_aligns_descriptions() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 30,
                });

                let session_name = "eph-test".to_string();
                backend.state_mut().session_name = Some(session_name.clone());
                backend.state_mut().session_attached = true;
                backend.state_mut().show_session_picker = true;
                backend.state_mut().session_picker =
                    Some(crate::state::SessionPickerState::new(vec![
                        crate::session::discovery::DiscoveredSession {
                            name: session_name,
                            ephemeral: true,
                            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                                panes: 1,
                                clients: 1,
                                has_layout: true,
                            },
                        },
                        crate::session::discovery::DiscoveredSession {
                            name: "shared-dev".to_string(),
                            ephemeral: false,
                            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                                panes: 2,
                                clients: 1,
                                has_layout: true,
                            },
                        },
                    ]));
                backend.render();

                let selection_bg = backend.state().theme.border_active;
                let frame = backend.capture_frame();
                let lines = frame.to_fixed_grid_lines();
                let row = lines
                    .iter()
                    .find(|line| line.contains("ephemeral") && line.contains("1 pane"))
                    .unwrap_or_else(|| {
                        panic!("session row missing pane count\n{}", lines.join("\n"))
                    });
                let current_col = row.find("current").expect("current marker");
                let pane_col = row.find("1 pane").expect("right pane count");
                assert!(
                    pane_col > current_col + 12,
                    "pane count should be right-aligned, not inline\n{row}"
                );
                let shared_row = lines
                    .iter()
                    .find(|line| line.contains("shared-dev"))
                    .unwrap_or_else(|| panic!("shared session row missing\n{}", lines.join("\n")));
                assert!(
                    shared_row.contains("2 panes · 1 other client"),
                    "occupied session should identify its attached client\n{shared_row}"
                );

                let ephemeral_y = lines
                    .iter()
                    .position(|line| line.contains("ephemeral"))
                    .expect("ephemeral row") as u16;
                let ephemeral_byte_x = lines[ephemeral_y as usize]
                    .find("ephemeral")
                    .expect("ephemeral column");
                let ephemeral_x = lines[ephemeral_y as usize][..ephemeral_byte_x]
                    .chars()
                    .count() as u16;
                assert_eq!(
                    frame.cell(ephemeral_x, ephemeral_y).bg,
                    selection_bg,
                    "selected custom-rendered row should use the picker selection background"
                );

                backend
                    .state_mut()
                    .session_picker
                    .as_mut()
                    .expect("session picker")
                    .selected = 1;
                backend.render();
                let frame = backend.capture_frame();
                let lines = frame.to_fixed_grid_lines();
                let shared_y = lines
                    .iter()
                    .position(|line| line.contains("shared-dev"))
                    .expect("shared session row") as u16;
                let shared_byte_x = lines[shared_y as usize]
                    .find("shared-dev")
                    .expect("shared session column");
                let shared_x = lines[shared_y as usize][..shared_byte_x].chars().count() as u16;
                assert_eq!(
                    frame.cell(shared_x, shared_y).bg,
                    selection_bg,
                    "selected default-rendered row should match the custom-rendered row"
                );
            })
            .expect("spawn snapshot test thread")
            .join()
            .expect("snapshot test thread completes");
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
