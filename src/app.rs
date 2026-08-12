use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::anim::GeometryAnimation;
use crate::config::Config;
use crate::session::bootstrap::{SessionStart, attach_session_client, has_session_candidates};
use crate::state::{Pane, PaneId, State, ThemePreset};
use crate::{
    anim, cli, commands, config, control, events, key_routing, ops, platform, profiles, state,
    update, view,
};

pub struct AppRoot {
    config: Config,
    initial_theme: Theme,
    initial_system_theme: Option<Theme>,
    startup_profile: Option<StartupProfile>,
    startup_messages: Vec<String>,
    control_listener: Option<crate::platform::ipc::IpcListener>,
    control_guard: Option<control::ControlSocketGuard>,
    attach_session: Option<String>,
    startup_autostart: bool,
    startup_create_only: bool,
    read_only: bool,
    /// When set, attach through SSH to this remote target instead of a local endpoint.
    remote: Option<crate::session::remote::RemoteTarget>,
    /// Whether a bare launch should open the session picker instead of attaching (`--pick`, the
    /// default `[session] startup = "picker"`, or a `last` whose session is gone). Only honored
    /// when there is no target/`--session` and there is something to pick at startup; opening it
    /// attaches nothing, so the client starts sessionless.
    want_startup_picker: bool,
    /// Session name the startup picker should land on, from a `last` that could not reopen.
    startup_picker_highlight: Option<String>,
    /// Whether to install the process-global terminal-hangup handler
    /// ([`platform::server_lifecycle::on_hangup`]) at startup. Only the real [`run`] wants this:
    /// a `TestBackend`-driven test constructs its app through [`Default`], and a test process must
    /// not have its `SIGHUP`/`SIGTERM` disposition rewritten out from under the harness (nor can
    /// several parallel tests each claim the one install slot).
    watch_hangup: bool,
    /// Whether the startup task may do anything beyond handing back the command link: watch the
    /// config file, install the hangup handler, serve the control socket, attach to (or create, or
    /// discover) a session server, arm the theme and workbar ticks. Only the real [`run`] wants it.
    ///
    /// A `TestBackend`-driven test builds its app through [`Default`], where none of that is
    /// survivable. Every test in one binary derives the same `eph-<pid>` name, so they would all
    /// race to bootstrap and then share one real server; and any message such a task lands
    /// mid-test arrives inside whatever `pump` happens to be running, which is what made
    /// assertions about panes, hosts, and in-flight polls flake under load.
    startup_tasks: bool,
    event_hub: events::EventHub,
}

#[derive(Clone)]
struct StartupProfile {
    profile: profiles::Profile,
    name: String,
    path: PathBuf,
    records_origin: bool,
}

impl Default for AppRoot {
    fn default() -> Self {
        let config = Config::default();
        Self {
            initial_theme: ThemePreset::Lipan.theme(),
            initial_system_theme: None,
            config,
            startup_profile: None,
            startup_messages: Vec::new(),
            control_listener: None,
            control_guard: None,
            attach_session: None,
            startup_autostart: true,
            startup_create_only: false,
            read_only: false,
            remote: None,
            want_startup_picker: false,
            startup_picker_highlight: None,
            watch_hangup: false,
            startup_tasks: false,
            event_hub: events::EventHub::default(),
        }
    }
}

impl AppRoot {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: Config,
        initial_theme: Theme,
        initial_system_theme: Option<Theme>,
        startup_profile: Option<StartupProfile>,
        startup_messages: Vec<String>,
        control_listener: Option<crate::platform::ipc::IpcListener>,
        control_guard: Option<control::ControlSocketGuard>,
        attach_session: Option<String>,
        startup_autostart: bool,
        startup_create_only: bool,
        read_only: bool,
        remote: Option<crate::session::remote::RemoteTarget>,
        want_startup_picker: bool,
        startup_picker_highlight: Option<String>,
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
            startup_autostart,
            startup_create_only,
            read_only,
            remote,
            want_startup_picker,
            startup_picker_highlight,
            watch_hangup: true,
            startup_tasks: true,
            event_hub: events::EventHub::default(),
        }
    }
}

impl Component for AppRoot {
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
        state.current_mut().deferred_profile_seed =
            self.startup_profile.as_ref().and_then(|profile| {
                profile
                    .records_origin
                    .then(|| (profile.name.clone(), profile.path.clone()))
            });
        ops::theme::apply_terminal_palette_to_state(&mut state);
        state
    }

    fn init(&mut self, ctx: &mut Context<Self>) -> Option<Command> {
        commands::sync(ctx);

        for message in std::mem::take(&mut self.startup_messages) {
            crate::pty_events::notify_info(ctx, message);
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
                    crate::pty_events::notify_error(ctx, "Theme watch failed", err.to_string());
                }
            }
        }

        // Always-server model: with no explicit target, attach to this process's ephemeral
        // session (`eph-<pid>`), autostarting its server. Restored/initial panes are spawned on
        // the server once `Msg::SessionAttached` reports an empty session. Opt-in `--pick` /
        // `[session] startup = "picker"` instead opens the session picker first when a named
        // session exists, so nothing is attached until the user chooses.
        let control_listener = self.control_listener.take();
        let event_hub = self.event_hub.clone();
        let theme_tick = ctx.state.theme_watcher.is_some();
        let workbar_tick = ctx.state.config.workbar.has_clock();

        let start = if self.want_startup_picker && self.remote.is_none() && has_session_candidates()
        {
            // Nothing is attached behind the startup picker, so the panes `create_state` prepared
            // have no session to live in yet. Park them as the launcher seed: choosing a session
            // discards them, while starting a shell gets exactly the layout this launch intended.
            let seed = std::mem::take(ctx.state.current_mut());
            ctx.state.launcher_seed = Some(seed);
            let epoch = ops::session::open_startup_session_picker(
                ctx,
                self.startup_picker_highlight.take(),
            );
            SessionStart::Picker { epoch }
        } else {
            let name = self.attach_session.clone().unwrap_or_else(|| {
                // Under `--remote` a bare `eph-<pid>` could collide with another client that shares
                // the pid on a different machine, since the ephemeral session lives on the remote
                // host. Qualify it with a stable per-client identifier so it stays per-client.
                if self.remote.is_some() {
                    state::remote_ephemeral_session_name()
                } else {
                    state::ephemeral_session_name()
                }
            });
            let epoch = ctx.state.runtime_epoch;
            let autostart = self.startup_autostart && !self.read_only;
            ctx.state.current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch,
                    name: name.clone(),
                    client: None,
                    autostart,
                    read_only: self.read_only,
                    reconnect: false,
                    remote_host: self.remote.as_ref().map(|target| target.display_label()),
                    intent: self.startup_profile.as_ref().map_or(
                        crate::state::AttachIntent::Plain,
                        |profile| {
                            if profile.records_origin {
                                crate::state::AttachIntent::ProfileSeed {
                                    profile: profile.name.clone(),
                                    path: profile.path.clone(),
                                }
                            } else {
                                crate::state::AttachIntent::Plain
                            }
                        },
                    ),
                    left: None,
                    parked_epoch: None,
                });
            ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
            // A bare launch with nothing to pick lands on an ephemeral nobody asked for by name.
            // Mark it so switching away discards it instead of leaving it running behind the
            // session the user actually wanted.
            ctx.state.current_mut().auto_created = self.attach_session.is_none();
            ctx.state.current_mut().remote_host =
                self.remote.as_ref().map(|target| target.display_label());
            ctx.state.current_mut().remote_target = self.remote.clone();
            SessionStart::Attach {
                epoch,
                name,
                autostart,
                create_only: self.startup_create_only,
            }
        };

        let startup_read_only = self.read_only;
        let watch_hangup = self.watch_hangup;
        let startup_tasks = self.startup_tasks;
        let remote = self.remote.clone();
        let remote_config = self.config.remote.clone();
        Some(Command::spawn(move |link: CommandLink<Msg>| {
            link.send(Msg::CommandLinkReady(link.clone()));
            if !startup_tasks {
                return;
            }
            ops::config::spawn_config_watcher(&link);
            if watch_hangup {
                let hangup_link = link.clone();
                if let Err(err) = platform::server_lifecycle::on_hangup(move || {
                    hangup_link.send(Msg::Hangup);
                }) {
                    // Not fatal: without it, an exiting terminal kills the client outright, which
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
                SessionStart::Attach {
                    epoch,
                    name,
                    autostart,
                    create_only,
                } => {
                    let session_link = link.clone();
                    std::thread::spawn(move || {
                        if let Some(remote) = remote {
                            crate::session::bootstrap::attach_remote_session_client(
                                epoch,
                                name,
                                startup_read_only,
                                create_only,
                                remote,
                                remote_config,
                                // Startup: fail fast to the ephemeral fallback if the host is down.
                                false,
                                session_link,
                            );
                        } else if create_only {
                            crate::session::bootstrap::create_session_client(
                                epoch,
                                name,
                                startup_read_only,
                                session_link,
                            )
                        } else {
                            attach_session_client(
                                epoch,
                                name,
                                autostart,
                                startup_read_only,
                                session_link,
                            )
                        }
                    });
                }
                SessionStart::Picker { epoch } => {
                    // Kick off the first discovery tick; `apply_discovered_sessions` re-arms the
                    // auto-refresh loop from there, exactly as an in-app picker opening would.
                    let watch_link = link.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(1500));
                        if let Ok((rows, host_status)) =
                            crate::ops::session::discover_picker_sessions(None, &remote_config)
                        {
                            watch_link.send(Msg::SessionsDiscovered {
                                epoch,
                                rows,
                                host_status,
                            });
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

    fn on_window_focus_changed(&mut self, focused: bool, ctx: &mut Context<Self>) -> Update {
        ops::focus::window_focus_changed(ctx, focused)
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        if ctx.devtools_visible() {
            ctx.set_devtools_metrics(|| {
                crate::runtime_metrics::RuntimeMetrics::capture(ctx.state.current()).devtools_rows()
            });
        }
        view::render(self, ctx)
    }
}

impl AppRoot {
    pub(crate) fn transition_config_for(
        &self,
        ctx: &Context<Self>,
        pane: &Pane,
        viewport_changed: bool,
    ) -> TransitionConfig {
        Self::geometry_transition_for_pane(&ctx.state, pane, viewport_changed)
    }

    /// Geometry transition policy for a pane. Extracted so tests can assert Scrollable resize
    /// instant-vs-AxisChange behavior without constructing a live [`Context`].
    pub(crate) fn geometry_transition_for_pane(
        state: &State,
        pane: &Pane,
        viewport_changed: bool,
    ) -> TransitionConfig {
        if !state.is_controller()
            || viewport_changed
            || state
                .moving_pane
                .is_some_and(|session| session.id == pane.id)
            || state
                .resizing_pane
                .as_ref()
                .is_some_and(|session| session.id == pane.id)
        {
            return anim::instant_transition();
        }

        let animations = state.config.animations;
        if !animations.enabled {
            return anim::instant_transition();
        }

        let enabled = match state.animation {
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

        // The close scale has to finish inside the close animation's own window. Running it at
        // `geometry_duration` (which is longer) means the pane is pruned part-way through, so the
        // slow start of the easing is all that is ever seen.
        if pane.closing {
            return anim::close_geometry_transition(animations.close_duration);
        }
        anim::geometry_transition(animations.geometry_duration)
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
            // The fade rides the close scale, so it has to share its duration.
            return if animations.close {
                TransitionConfig {
                    duration: animations.close_duration,
                    easing: Easing::EaseOutQuad,
                }
            } else {
                anim::instant_transition()
            };
        }
        if pane.opening && animations.spawn {
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

    /// Breathe timing for an alert. `calm` stretches the fade to the calm half period so a slower
    /// alert is genuinely slower rather than a fast fade that then sits still waiting for its beat.
    pub(crate) fn alert_pulse_transition_config(
        &self,
        ctx: &Context<Self>,
        calm: bool,
    ) -> TransitionConfig {
        let animations = ctx.state.config.animations;
        if animations.enabled && animations.focus_chrome {
            TransitionConfig {
                duration: if calm {
                    anim::alert_pulse_calm_half_period(animations)
                } else {
                    anim::alert_pulse_half_period(animations)
                },
                easing: Easing::EaseInOutCubic,
            }
        } else {
            anim::instant_transition()
        }
    }

    /// A pane chrome colour, as a paint the renderer resolves while drawing.
    ///
    /// `animated_color` rather than `transition`: chrome colours only ever land in styles, so naming
    /// the fade instead of embedding its current value keeps this element identical for the whole
    /// 160ms. Each frame of a focus change is then a repaint rather than a rebuild of every pane,
    /// workbar segment and sidebar row in the window.
    pub(crate) fn chrome_color(
        &self,
        ctx: &Context<Self>,
        pane: PaneId,
        slot: &str,
        target: Color,
    ) -> Paint {
        self.chrome_color_with(
            ctx,
            pane,
            slot,
            target,
            self.focus_chrome_transition_config(ctx),
        )
    }

    pub(crate) fn chrome_color_with(
        &self,
        ctx: &Context<Self>,
        pane: PaneId,
        slot: &str,
        target: Color,
        config: TransitionConfig,
    ) -> Paint {
        self.chrome_paint(
            ctx,
            format!("hyprmux-pane-chrome-{pane}-{slot}"),
            target,
            config,
        )
    }

    /// A caller-keyed chrome paint. Palette/indexed colors deliberately snap rather than blend.
    pub(crate) fn chrome_paint(
        &self,
        ctx: &Context<Self>,
        key: String,
        target: Color,
        config: TransitionConfig,
    ) -> Paint {
        // Only truecolor targets may fade. Named/indexed ANSI colors must be emitted
        // verbatim so the user's terminal palette resolves them; blending them animates
        // through `Color::Rgb` (`blend_toward` always returns Rgb), which bypasses the
        // palette and flips the hue mid-fade - e.g. an ANSI theme's `LightCyan` chrome
        // shows as true cyan while the focus animation runs but as the palette color at
        // rest. Snapping keeps palette themes consistent (and matching the workbar).
        let config = if chrome_color_animates(target) {
            config
        } else {
            anim::instant_transition()
        };
        ctx.animated_color(key, target, config)
    }
}

/// Whether a chrome color target is safe to fade. Only truecolor (`Color::Rgb`) targets
/// animate; named/indexed palette colors snap so the terminal palette stays in control.
pub(crate) fn chrome_color_animates(target: Color) -> bool {
    matches!(target, Color::Rgb(..))
}

/// A chrome breathe can move only between distinct truecolor endpoints. Named/indexed palette
/// colors deliberately snap so terminal palette ownership remains intact.
pub(crate) fn chrome_colors_animate(peak: Color, trough: Color) -> bool {
    peak != trough && chrome_color_animates(peak) && chrome_color_animates(trough)
}

pub(crate) fn schedule_theme_tick() -> Command {
    Command::after(Duration::from_millis(150), move |link: CommandLink<Msg>| {
        link.send(Msg::ThemeTick);
    })
}

/// Low-frequency repaint so a configured clock segment advances. Only scheduled while a clock
/// segment is present, so an idle app with the default workbar never wakes for this.
pub(crate) fn schedule_workbar_tick() -> Command {
    Command::after(Duration::from_secs(1), move |link: CommandLink<Msg>| {
        link.send(Msg::WorkbarTick);
    })
}

/// Low-frequency repaint so the Agents sidebar's elapsed-time column advances. Only scheduled
/// while that column is actually on screen, so a hidden sidebar or a screen of idle agents never
/// wakes the app for this.
pub(crate) fn schedule_agent_tick() -> Command {
    Command::after(Duration::from_secs(1), move |link: CommandLink<Msg>| {
        link.send(Msg::AgentTick);
    })
}

pub(crate) fn schedule_alert_pulse_tick(half_period: Duration) -> Command {
    Command::after(half_period, move |link: CommandLink<Msg>| {
        link.send(Msg::AlertPulseTick);
    })
}

fn clipboard_config(config: &Config) -> ClipboardConfig {
    // OSC52 always targets the *local* terminal emulator that hosts this client. Under `--remote`
    // that is what we want: copy from a remote pane reaches the local clipboard. Disabling
    // `enable_osc52` drops OSC52 without redirecting copies to the remote host.
    ClipboardConfig {
        enable_osc52: config.clipboard.enable_osc52,
        ..ClipboardConfig::default()
    }
}

pub(crate) fn clipboard_copy_feedback_duration(config: &Config) -> Duration {
    Duration::from_millis(clipboard_config(config).copy_feedback_duration_ms as u64)
}

pub fn run() -> Result<()> {
    let parsed = match cli::parse_cli_args(std::env::args().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("Run `hyprmux --help` for usage.");
            std::process::exit(1);
        }
    };

    // Reconcile a managed pointer before any command can observe or start the application. This is
    // local-only and makes updater recovery precede ConPTY checks, endpoints, sessions, and the TUI.
    if let Err(message) = cli::recover_managed_installation() {
        eprintln!("hyprmux: {message}");
        std::process::exit(1);
    }

    let parsed = match parsed {
        cli::ParsedCli::Help => {
            cli::print_help();
            return Ok(());
        }
        cli::ParsedCli::Version => {
            cli::print_version();
            return Ok(());
        }
        cli::ParsedCli::Skill => {
            cli::print_skill();
            return Ok(());
        }
        cli::ParsedCli::Install => {
            if let Err(message) = cli::run_install_cli() {
                eprintln!("hyprmux: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        cli::ParsedCli::Update(command) => {
            if let Err(message) = cli::run_update_cli(command) {
                eprintln!("hyprmux: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        parsed => parsed,
    };

    // Every runtime/session command still receives the platform support check; only the updater,
    // help, version, and agent-skill paths above intentionally run before it.
    if let Err(reason) = platform::server_lifecycle::check_host_supported() {
        eprintln!("hyprmux: {reason}");
        std::process::exit(1);
    }

    let cli = match parsed {
        cli::ParsedCli::Control(command) => return cli::run_control_cli(command),
        cli::ParsedCli::AgentSlots(command) => return cli::run_agent_slots_cli(command),
        cli::ParsedCli::Server { name, fresh } => return cli::run_server_cli(&name, fresh),
        cli::ParsedCli::RemoteServe { name } => return cli::run_remote_serve_cli(&name),
        cli::ParsedCli::ListSessions { format, remote } => {
            return cli::run_list_sessions_cli(format, remote.as_deref());
        }
        cli::ParsedCli::KillSession { name, remote } => {
            return cli::run_kill_session_cli(&name, remote.as_deref());
        }
        cli::ParsedCli::Run(args) => args,
        cli::ParsedCli::Help
        | cli::ParsedCli::Version
        | cli::ParsedCli::Skill
        | cli::ParsedCli::Install
        | cli::ParsedCli::Update(_) => unreachable!("early CLI command returned above"),
    };

    if let Some(path) = cli.config_path {
        unsafe {
            std::env::set_var("ROZI_CONFIG", path);
        }
    }

    let loaded = config::load_config();
    let mut startup_messages = loaded.warnings;
    let explicit_target = cli.attach_session.is_some();
    let mut attach_session = cli.attach_session.clone();
    let mut startup_autostart = attach_session.is_none();
    let mut startup_create_only = false;
    // The name the startup picker should land on when `last` could not reopen its session.
    let mut startup_picker_highlight = None;
    if attach_session.is_none()
        && !cli.pick
        && loaded.config.session.startup == config::SessionStartup::Last
    {
        match resolve_last_session_target() {
            LastSessionTarget::Reopen(name) => {
                attach_session = Some(name);
                startup_autostart = true;
            }
            LastSessionTarget::Pick(name) => startup_picker_highlight = name,
        }
    }
    let mut startup_profile = None;
    if let Some(name) = attach_session.as_ref()
        && cli.remote.is_some()
    {
        // Under `--remote` the session lives on the remote host: local discovery and local profiles
        // do not describe it, so none of the checks below apply. The remote `--remote-serve`
        // autostarts the session server, and `New`'s create-only intent is enforced remotely via the
        // preamble's `server_started` flag. Only map the subcommand onto the create-only flag; a
        // `New --profile` still seeds from a locally loaded profile.
        if !crate::session::discovery::valid_session_name(name) {
            startup_fatal(format!("Invalid session name `{name}`."));
        }
        match cli.session_command {
            cli::SessionCommand::New => {
                startup_autostart = true;
                startup_create_only = true;
                if let Some(profile_name) = cli.profile.as_ref() {
                    if !crate::session::discovery::valid_session_name(profile_name) {
                        startup_fatal(format!("Invalid profile name `{profile_name}`."));
                    }
                    let path = config::profile_path_for_name(profile_name);
                    if !path.exists() {
                        startup_fatal(format!("Profile `{profile_name}` does not exist."));
                    }
                    startup_profile = Some(load_startup_profile(profile_name, path));
                }
            }
            cli::SessionCommand::Attach => startup_autostart = false,
            cli::SessionCommand::Dwim => startup_autostart = true,
        }
    } else if let Some(name) = attach_session.as_ref() {
        if !crate::session::discovery::valid_session_name(name) {
            startup_fatal(format!("Invalid session name `{name}`."));
        }
        let running = crate::session::discovery::discover_session(name)
            .ok()
            .flatten()
            .is_some();
        let canonical_path = config::profile_path_for_name(name);
        match cli.session_command {
            cli::SessionCommand::Attach => {
                if !running {
                    let hint = canonical_path
                        .exists()
                        .then(|| format!("\nStart it with: hyprmux {name}"));
                    startup_fatal(format!(
                        "Session `{name}` is not running.{}",
                        hint.unwrap_or_default()
                    ));
                }
                startup_autostart = false;
            }
            cli::SessionCommand::New => {
                if running {
                    startup_fatal(format!(
                        "Session `{name}` is already running.\nAttach with: hyprmux attach {name}"
                    ));
                }
                startup_autostart = true;
                startup_create_only = true;
                if let Some(profile_name) = cli.profile.as_ref() {
                    if !crate::session::discovery::valid_session_name(profile_name) {
                        startup_fatal(format!("Invalid profile name `{profile_name}`."));
                    }
                    let path = config::profile_path_for_name(profile_name);
                    if !path.exists() {
                        startup_fatal(format!("Profile `{profile_name}` does not exist."));
                    }
                    startup_profile = Some(load_startup_profile(profile_name, path));
                }
            }
            cli::SessionCommand::Dwim if explicit_target => {
                if running {
                    startup_autostart = false;
                } else if cli.read_only {
                    startup_fatal(format!("Session `{name}` is not running."));
                } else if canonical_path.exists() {
                    startup_profile = Some(load_startup_profile(name, canonical_path));
                    startup_autostart = true;
                } else {
                    startup_fatal(format!(
                        "No session or profile named `{name}`.\nCreate it with: hyprmux new {name}"
                    ));
                }
            }
            cli::SessionCommand::Dwim => {
                startup_autostart = true;
                if !running && canonical_path.exists() {
                    startup_profile = Some(load_startup_profile(name, canonical_path));
                }
            }
        }
    }

    if attach_session.is_none()
        && startup_profile.is_none()
        && let Some(name) = &loaded.config.profile.default
    {
        let path = config::profile_path_for_name(name);
        match profiles::load_profile(&path) {
            Ok(profile) => {
                startup_profile = Some(StartupProfile {
                    profile,
                    name: name.clone(),
                    path,
                    records_origin: true,
                })
            }
            Err(err) => {
                startup_messages.push(format!("Default profile `{name}` load failed: {err}"))
            }
        }
    }

    // With no explicit profile, restore the autosaved session if one exists.
    if attach_session.is_none()
        && startup_profile.is_none()
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
                    records_origin: false,
                });
            }
            Err(err) => startup_messages.push(format!("Session restore failed: {err}")),
        }
    }
    let config = loaded.config;
    // Open the picker at startup only for a bare launch (no explicit attach target). The
    // "anything to pick" gate is checked in `init` so it reflects live state at mount. `last`
    // reaches here only when its remembered session could not be reopened.
    let want_startup_picker = attach_session.is_none()
        && cli.remote.is_none()
        && (cli.pick
            || matches!(
                config.session.startup,
                config::SessionStartup::Picker | config::SessionStartup::Last
            ));
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

    let remote = match cli.remote.as_deref() {
        Some("") => {
            let Some(default_host) = config.remote.default_host.as_deref() else {
                eprintln!(
                    "--remote requires a host alias or ssh:// URL (or set [remote] default_host)"
                );
                std::process::exit(1);
            };
            match crate::session::remote::parse_remote_target(default_host) {
                Ok(target) => Some(target),
                Err(err) => {
                    eprintln!("[remote] default_host: {err}");
                    std::process::exit(1);
                }
            }
        }
        Some(raw) => match crate::session::remote::parse_remote_target(raw) {
            Ok(target) => Some(target),
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        },
        None => None,
    };

    // Probe / prompt / install on the main thread before the TUI takes stdin (install = "prompt").
    if let Some(ref target) = remote {
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if let Err(err) =
            crate::session::remote::ensure_remote_binary(target, &config.remote, interactive)
        {
            eprintln!("hyprmux: {err}");
            std::process::exit(1);
        }
    }

    app.mount(AppRoot::new(
        config,
        theme,
        startup_system_theme,
        startup_profile,
        startup_messages,
        control_listener,
        control_guard,
        attach_session,
        startup_autostart,
        startup_create_only,
        cli.read_only,
        remote,
        want_startup_picker,
        startup_picker_highlight,
    ))
    .exit_view(crate::exit_view::exit_view)
    .run()
}

fn load_startup_profile(name: &str, path: PathBuf) -> StartupProfile {
    match profiles::load_profile(&path) {
        Ok(profile) => StartupProfile {
            profile,
            name: name.to_string(),
            path,
            records_origin: true,
        },
        Err(err) => startup_fatal(format!("Profile `{name}` load failed: {err}")),
    }
}

fn startup_fatal(message: String) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

/// What `[session] startup = "last"` resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LastSessionTarget {
    /// Reopen this exact session: it is running, or restorable from a snapshot or its canonical
    /// same-name profile.
    Reopen(String),
    /// Nothing reopenable. Fall through to the picker, highlighting the remembered name when there
    /// is one, rather than silently landing on an unrelated session.
    Pick(Option<String>),
}

fn resolve_last_session_target() -> LastSessionTarget {
    let Some(last) = crate::session::read_last_named_session() else {
        return LastSessionTarget::Pick(None);
    };
    let running = crate::session::discovery::discover_session(&last)
        .ok()
        .flatten()
        .is_some();
    let restorable = crate::session::server::list_snapshot_names_by_recency()
        .iter()
        .any(|name| name == &last)
        || config::profile_path_for_name(&last).exists();
    select_last_session_target(last, running || restorable)
}

fn select_last_session_target(last: String, reopenable: bool) -> LastSessionTarget {
    if reopenable {
        LastSessionTarget::Reopen(last)
    } else {
        LastSessionTarget::Pick(Some(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::{TestBackend, UiSnapshotOptions, UiWidgetKind};

    #[test]
    fn profile_picker_hints_reflow_without_splitting_pills() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 52,
                    h: 18,
                });
                let mut picker =
                    crate::state::ProfilePickerState::new(vec![crate::config::ProfileEntry {
                        name: "rust-dev".to_string(),
                        path: PathBuf::from("rust-dev.toml"),
                    }]);
                picker.running.insert(
                    "rust-dev".to_string(),
                    crate::session::discovery::DiscoveredSessionStatus::Running {
                        panes: 2,
                        clients: 1,
                        has_layout: true,
                        created_from_profile: None,
                    },
                );
                backend.state_mut().config.profile.default = Some("rust-dev".to_string());
                backend.state_mut().profile_picker = Some(picker);
                backend.state_mut().show_profile_picker = true;
                backend.render();

                let lines = backend.capture_frame().to_fixed_grid_lines();
                assert!(lines.iter().any(|line| line.contains("attach enter")));
                assert!(lines.iter().any(|line| line.contains("open as ctrl+o")));
                assert!(lines.iter().any(|line| line.contains("default ctrl+f")));
                assert!(lines.iter().any(|line| line.contains("replace ctrl+r")));
                assert!(lines.iter().any(|line| line.contains("• running")));
                assert!(lines.iter().any(|line| line.contains("new ctrl+n")));
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn startup_last_reopens_a_reopenable_session() {
        assert_eq!(
            select_last_session_target("dev".to_string(), true),
            LastSessionTarget::Reopen("dev".to_string())
        );
    }

    /// A remembered session that is gone must not silently become some *other* session: `last`
    /// hands the name to the picker as a highlight instead.
    #[test]
    fn startup_last_defers_an_unavailable_session_to_the_picker() {
        assert_eq!(
            select_last_session_target("dev".to_string(), false),
            LastSessionTarget::Pick(Some("dev".to_string()))
        );
    }

    #[test]
    fn theme_picker_groups_dark_and_light_presets() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 60,
                });
                backend.state_mut().show_theme_picker = true;
                backend.render();

                let rendered = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(rendered.contains("System"));
                assert!(rendered.contains("Dark"));
                assert!(rendered.contains("Light"));
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn selecting_theme_restores_focus_to_the_focused_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                // Selecting a theme persists it; `test_support` has already pointed the writer at
                // this process's scratch root rather than the developer's own config.
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                backend.render();
                backend.focus_next();

                let pane_key = crate::view::pane_terminal_key(1);
                assert_eq!(
                    backend.focused_key().map(|key| key.as_ref()),
                    Some(pane_key.as_str())
                );

                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenThemePicker))
                    .expect("open theme picker");
                backend.dispatch(Msg::SelectTheme(0)).expect("select theme");

                assert_eq!(
                    backend.focused_key().map(|key| key.as_ref()),
                    Some(pane_key.as_str())
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn command_palette_modal_is_capped_to_sixty_five_percent_of_viewport() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
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
                let mut backend = TestBackend::new(AppRoot::default());
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
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                backend.state_mut().show_palette = true;
                backend.render();

                let commands_modal = |backend: &TestBackend<AppRoot>| {
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
    fn scrollback_search_uses_footer_hints_and_highlights_matches() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 120,
                    h: 24,
                });

                let mut search = crate::state::ScrollbackSearchState::new(1);
                search.input.set_text("master");
                search.input.set_cursor(6);
                search.matches.push(crate::state::ScrollbackMatch {
                    offset: 0,
                    line: 1,
                    start_col: 8,
                    end_col: 14,
                    text: std::sync::Arc::from("hyprmux master • prompt"),
                    pane: 1,
                });
                search.rebuild_items();
                search.refresh_match_status();
                backend.state_mut().search = Some(search);
                let match_fg = backend.state().theme.status.info;
                backend.render();

                let snapshot =
                    backend.capture_ui_snapshot_with_options(&UiSnapshotOptions::default());
                let modal = snapshot
                    .widgets
                    .iter()
                    .find(|widget| {
                        widget.kind == UiWidgetKind::Frame
                            && widget.title.as_deref() == Some("Search scrollback")
                    })
                    .expect("scrollback search modal");
                assert_eq!(modal.rect.w, 90);
                assert!(!modal.title.as_deref().unwrap().contains("Tab"));

                let frame = backend.capture_frame();
                let lines = frame.to_fixed_grid_lines();
                let rendered = lines.join("\n");
                assert!(rendered.contains("next ctrl+n"), "{rendered}");
                assert!(rendered.contains("previous ctrl+p"), "{rendered}");
                assert!(rendered.contains("pane tab"), "{rendered}");
                assert!(rendered.contains("1 / 1 matches (pane)"), "{rendered}");
                assert!(!rendered.contains("scope:"), "{rendered}");

                let row = lines
                    .iter()
                    .position(|line| line.contains("hyprmux master"))
                    .expect("matching result row") as u16;
                let matched = lines[row as usize].find("master").expect("match column") as u16;
                let plain = lines[row as usize].find("hyprmux").expect("plain column") as u16;
                assert_ne!(
                    frame.cell(matched, row).fg,
                    frame.cell(plain, row).fg,
                    "selected row must preserve the query-match foreground"
                );
                assert_eq!(frame.cell(matched, row).fg, match_fg);
            })
            .expect("spawn snapshot test thread")
            .join()
            .expect("snapshot test thread completes");
    }

    #[test]
    fn scrollback_search_keeps_metadata_visible_beside_long_line_labels() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 24,
                });

                let mut search = crate::state::ScrollbackSearchState::new(1);
                search.input.set_text("needle");
                search.input.set_cursor(6);
                search.matches.push(crate::state::ScrollbackMatch {
                    offset: 0,
                    line: 8,
                    start_col: 12,
                    end_col: 18,
                    text: std::sync::Arc::from(format!(
                        "needle {}",
                        "a very long terminal line that must yield to metadata".repeat(3)
                    )),
                    pane: 1,
                });
                search.rebuild_items();
                search.refresh_match_status();
                backend.state_mut().search = Some(search);
                let match_fg = backend.state().theme.status.info;
                backend.render();

                let frame = backend.capture_frame();
                let lines = frame.to_fixed_grid_lines();
                let row_index = lines
                    .iter()
                    .position(|line| line.contains("pane 1 · row 9 · col 13"))
                    .unwrap_or_else(|| {
                        panic!("long result row with complete metadata: {lines:#?}")
                    });
                let row = &lines[row_index];
                assert!(
                    row.contains("needle"),
                    "query prefix should remain visible: {row}"
                );
                assert!(row.contains('…'), "long label should be truncated: {row}");
                let query_column = row.find("needle").expect("query prefix") as u16;
                assert_eq!(frame.cell(query_column, row_index as u16).fg, match_fg);
            })
            .expect("spawn metadata priority test")
            .join()
            .expect("metadata priority test completes");
    }

    #[test]
    fn scrollback_search_renders_newest_matches_first_and_keeps_navigation_aligned() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 120,
                    h: 24,
                });
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let output = (0..150)
                    .map(|index| match index {
                        0 => "needle alpha\r\n".to_string(),
                        1 => "needle bravo\r\n".to_string(),
                        149 => "needle\r\n".to_string(),
                        index => format!("needle filler-{index:03}\r\n"),
                    })
                    .collect::<String>();
                crate::pane_lifecycle::find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(output.as_bytes());

                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenSearch))
                    .expect("open search");
                for ch in "needle".chars() {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Char(ch),
                            mods: KeyMods::NONE,
                        })
                        .expect("type search query");
                }
                while backend
                    .state()
                    .search
                    .as_ref()
                    .is_some_and(|search| search.scan.is_some())
                {
                    let epoch = backend.state().search_scan_epoch;
                    let _ = crate::ops::search::advance_search_scan(
                        backend.state_mut(),
                        epoch,
                        crate::ops::search::SEARCH_LINES_PER_CHUNK,
                    );
                }
                backend.render();

                let lines = backend.capture_frame().to_fixed_grid_lines();
                let result_rows: Vec<_> = lines
                    .iter()
                    .filter(|line| line.contains("pane 1 · row"))
                    .collect();
                assert!(
                    result_rows.len() >= 3,
                    "expected visible result rows: {lines:#?}"
                );
                assert!(
                    result_rows[0].contains("needle") && !result_rows[0].contains("filler-"),
                    "{result_rows:#?}"
                );
                assert!(
                    result_rows[1].contains("needle filler-148"),
                    "{result_rows:#?}"
                );
                assert!(
                    result_rows[2].contains("needle filler-147"),
                    "{result_rows:#?}"
                );

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('n'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("next scanned row");
                assert_eq!(backend.state().search.as_ref().expect("search").current, 1);
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('p'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("previous scanned row");
                assert_eq!(backend.state().search.as_ref().expect("search").current, 0);

                let expected_offset =
                    backend.state().search.as_ref().expect("search").matches[149].offset;
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::End,
                        mods: KeyMods::NONE,
                    })
                    .expect("select last scanned row");
                assert_eq!(
                    backend.state().search.as_ref().expect("search").current,
                    149
                );
                assert!(
                    backend.state().search.as_ref().expect("search").matches[149]
                        .text
                        .contains("needle alpha")
                );
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("activate selected row");
                assert!(backend.state().search.is_none());
                assert_eq!(
                    crate::pane_lifecycle::find_pane(backend.state(), target)
                        .expect("target pane")
                        .terminal
                        .scrollback_offset(),
                    expected_offset
                );
            })
            .expect("spawn scanned-order test")
            .join()
            .expect("scanned-order test completes");
    }

    #[test]
    fn scrollback_palette_keeps_selection_and_activation_aligned_past_100_rows() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let output = (0..150)
                    .map(|index| format!("needle-{index:03}\r\n"))
                    .collect::<String>();
                crate::pane_lifecycle::find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(output.as_bytes());

                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenSearch))
                    .expect("open search");
                backend
                    .dispatch(Msg::SearchQueryChanged("needle".to_string()))
                    .expect("search query");
                // Read the generation rather than the live scan: `recompute_search` queues the
                // first chunk as a command, so a small corpus can finish the whole scan before
                // this line runs and leave `scan` already `None`.
                let epoch = backend.state().search_scan_epoch;
                while backend
                    .state()
                    .search
                    .as_ref()
                    .is_some_and(|search| search.scan.is_some())
                {
                    let _ = crate::ops::search::advance_search_scan(
                        backend.state_mut(),
                        epoch,
                        crate::ops::search::SEARCH_LINES_PER_CHUNK,
                    );
                }
                backend.render();
                assert_eq!(
                    backend.state().search.as_ref().expect("search").items.len(),
                    150
                );

                backend
                    .dispatch(Msg::SearchSelect(120))
                    .expect("select row past sync default");
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('n'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("next row");
                assert_eq!(
                    backend.state().search.as_ref().expect("search").current,
                    121
                );
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('p'),
                        mods: KeyMods::CTRL,
                    })
                    .expect("previous row");
                let expected = {
                    let search = backend.state().search.as_ref().expect("search");
                    assert_eq!(search.current, 120);
                    search.matches[120].clone()
                };

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("activate selected row");
                assert!(backend.state().search.is_none());
                assert_eq!(
                    crate::pane_lifecycle::find_pane(backend.state(), target)
                        .expect("target pane")
                        .terminal
                        .scrollback_offset(),
                    expected.offset
                );
            })
            .expect("spawn palette selection test")
            .join()
            .expect("palette selection test completes");
    }

    #[test]
    fn scrollback_search_empty_state_tracks_restart_progress_and_completion() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenSearch))
                    .expect("open search");
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let pane_end = crate::pane_lifecycle::find_pane(backend.state(), target)
                    .expect("target pane")
                    .terminal
                    .search_line_count();
                let epoch = backend.state().search_scan_epoch.wrapping_add(1);
                backend.state_mut().search_scan_epoch = epoch;
                {
                    let search = backend.state_mut().search.as_mut().expect("search");
                    search.input.set_text("absent");
                    search.scan = Some(crate::state::ScrollbackSearchScan {
                        epoch,
                        query: std::sync::Arc::from("absent"),
                        panes: std::sync::Arc::from([target]),
                        pane_ends: std::sync::Arc::from([pane_end]),
                        pane_index: 0,
                        line_cursor: 0,
                        first_jump_done: false,
                    });
                    search.refresh_match_status();
                }

                backend.render();
                let started = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(started.contains("Scanning…"), "{started}");
                assert!(!started.contains("No matches"), "{started}");
                assert_eq!(
                    backend.state().search.as_ref().expect("search").status,
                    "0 matches… (pane)"
                );

                assert!(matches!(
                    crate::ops::search::advance_search_scan(backend.state_mut(), epoch, 1),
                    crate::ops::search::SearchScanAdvance::Running { .. }
                ));
                backend.render();
                let progressing = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(progressing.contains("Scanning…"), "{progressing}");
                assert!(!progressing.contains("No matches"), "{progressing}");

                while backend
                    .state()
                    .search
                    .as_ref()
                    .is_some_and(|search| search.scan.is_some())
                {
                    let _ = crate::ops::search::advance_search_scan(
                        backend.state_mut(),
                        epoch,
                        crate::ops::search::SEARCH_LINES_PER_CHUNK,
                    );
                }
                backend.render();
                let completed = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(completed.contains("No matches for `absent`"), "{completed}");
                assert!(!completed.contains("Scanning…"), "{completed}");
                assert_eq!(
                    backend.state().search.as_ref().expect("search").status,
                    "0 matches (pane)"
                );

                let restarted_epoch = backend.state().search_scan_epoch.wrapping_add(1);
                backend.state_mut().search_scan_epoch = restarted_epoch;
                {
                    let search = backend.state_mut().search.as_mut().expect("search");
                    search.input.set_text("still-absent");
                    search.replace_results(Vec::new(), false);
                    search.current = 0;
                    search.scan = Some(crate::state::ScrollbackSearchScan {
                        epoch: restarted_epoch,
                        query: std::sync::Arc::from("still-absent"),
                        panes: std::sync::Arc::from([target]),
                        pane_ends: std::sync::Arc::from([pane_end]),
                        pane_index: 0,
                        line_cursor: 0,
                        first_jump_done: false,
                    });
                    search.refresh_match_status();
                }
                backend.render();
                let restarted = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(restarted.contains("Scanning…"), "{restarted}");
                assert!(!restarted.contains("No matches"), "{restarted}");
            })
            .expect("spawn empty-state search test")
            .join()
            .expect("empty-state search test completes");
    }

    #[test]
    fn progressive_search_append_keeps_controlled_selection_bound_to_source_match() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let output = (0..700)
                    .map(|index| {
                        if index < 520 {
                            format!("zzzz candidate {index:03} needle\r\n")
                        } else {
                            format!("needle {index:03}\r\n")
                        }
                    })
                    .collect::<String>();
                crate::pane_lifecycle::find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(output.as_bytes());
                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenSearch))
                    .expect("open search");
                let pane_end = crate::pane_lifecycle::find_pane(backend.state(), target)
                    .expect("target pane")
                    .terminal
                    .search_line_count();
                let epoch = backend.state().search_scan_epoch.wrapping_add(1);
                backend.state_mut().search_scan_epoch = epoch;
                {
                    let search = backend.state_mut().search.as_mut().expect("search");
                    search.input.set_text("needle");
                    search.scan = Some(crate::state::ScrollbackSearchScan {
                        epoch,
                        query: std::sync::Arc::from("needle"),
                        panes: std::sync::Arc::from([target]),
                        pane_ends: std::sync::Arc::from([pane_end]),
                        pane_index: 0,
                        line_cursor: 0,
                        first_jump_done: false,
                    });
                    search.refresh_match_status();
                }

                let _ = crate::ops::search::advance_search_scan(
                    backend.state_mut(),
                    epoch,
                    crate::ops::search::SEARCH_LINES_PER_CHUNK,
                );
                backend.render();
                let first_len = backend
                    .state()
                    .search
                    .as_ref()
                    .expect("search")
                    .matches
                    .len();
                assert!(first_len > 100);
                backend
                    .dispatch(Msg::SearchSelect(120))
                    .expect("select source row");
                backend.render();
                let expected = {
                    let search = backend.state().search.as_ref().expect("search");
                    assert_eq!(search.current, 120);
                    search.matches[120].clone()
                };

                let _ = crate::ops::search::advance_search_scan(
                    backend.state_mut(),
                    epoch,
                    crate::ops::search::SEARCH_LINES_PER_CHUNK,
                );
                backend.render();
                let search = backend.state().search.as_ref().expect("search");
                assert!(search.matches.len() > first_len);
                assert_eq!(search.current, 120);
                assert_eq!(search.matches[search.current], expected);

                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("activate controlled selection");
                assert!(backend.state().search.is_none());
                assert_eq!(
                    crate::pane_lifecycle::find_pane(backend.state(), target)
                        .expect("target pane")
                        .terminal
                        .scrollback_offset(),
                    expected.offset
                );
            })
            .expect("spawn progressive selection test")
            .join()
            .expect("progressive selection test completes");
    }

    #[test]
    fn session_picker_shows_clients_on_other_sessions_and_aligns_descriptions() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 30,
                });

                let session_name = "eph-test".to_string();
                backend.state_mut().current_mut().session_name = Some(session_name.clone());
                backend.state_mut().current_mut().session_attached = true;
                backend.state_mut().show_session_picker = true;
                backend.state_mut().session_picker =
                    Some(crate::state::SessionPickerState::new(vec![
                        crate::session::discovery::DiscoveredSession {
                            name: session_name,
                            ephemeral: true,
                            host: None,
                            remote_target: None,
                            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                                panes: 1,
                                clients: 1,
                                has_layout: true,
                                created_from_profile: None,
                            },
                        },
                        crate::session::discovery::DiscoveredSession {
                            name: "shared-dev".to_string(),
                            ephemeral: false,
                            host: None,
                            remote_target: None,
                            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                                panes: 2,
                                clients: 1,
                                has_layout: true,
                                created_from_profile: None,
                            },
                        },
                        crate::session::discovery::DiscoveredSession {
                            name: "remote-dev".to_string(),
                            ephemeral: false,
                            host: Some("workbox".to_string()),
                            remote_target: Some(crate::session::remote::RemoteTarget::Alias(
                                "workbox".to_string(),
                            )),
                            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                                panes: 3,
                                clients: 0,
                                has_layout: true,
                                created_from_profile: None,
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
                let current_col = row.find('●').expect("current gutter marker");
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
                    shared_row.contains("2 panes · shared with 1 other"),
                    "occupied session should identify the other client sharing it\n{shared_row}"
                );
                assert!(
                    lines.iter().any(|line| line.contains("LOCAL")),
                    "local group header missing\n{}",
                    lines.join("\n")
                );
                assert!(
                    lines.iter().any(|line| line.contains("REMOTE · workbox")),
                    "remote group header missing\n{}",
                    lines.join("\n")
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
