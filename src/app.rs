use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::config::Config;
use crate::input::routing;
use crate::layout::anim::{self, GeometryAnimation};
use crate::session::bootstrap::{SessionStart, attach_session_client};
use crate::state::{Pane, PaneId, State, ThemePreset};
use crate::{cli, commands, config, control, events, ops, platform, profiles, state, update, view};

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
    /// when there is no target/`--session`; opening it attaches nothing, so the client starts
    /// sessionless even when the list is empty.
    want_startup_picker: bool,
    /// Session name the startup picker should land on, from a `last` that could not reopen.
    startup_last_session: Option<String>,
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
    /// Whether real startup tasks include the public release check. Integration apps keep their
    /// session/control workers but must never contact GitHub.
    startup_update_check: bool,
    event_hub: events::EventHub,
}

#[derive(Clone)]
struct StartupProfile {
    profile: profiles::Profile,
    name: String,
    path: PathBuf,
    records_origin: bool,
}

struct StartupTasks {
    enabled: bool,
    update_check: bool,
    watch_hangup: bool,
    control_listener: Option<crate::platform::ipc::IpcListener>,
    event_hub: events::EventHub,
    start: SessionStart,
    read_only: bool,
    remote: Option<crate::session::remote::RemoteTarget>,
    remote_config: config::RemoteConfig,
    theme_tick: bool,
    workbar_tick: bool,
}

impl StartupTasks {
    fn run(mut self, link: CommandLink<Msg>) {
        link.send(Msg::CommandLinkReady(link.clone()));
        if !self.enabled {
            return;
        }
        if self.update_check {
            let update_link = link.clone();
            std::thread::spawn(move || {
                if let Some(update) = ops::update_check::check_startup() {
                    let compatibility_warning = update.compatibility_warning();
                    update_link.send(Msg::StartupUpdateAvailable {
                        latest: update.latest,
                        hint: update.hint,
                        compatibility_warning,
                    });
                }
            });
        }
        ops::config::spawn_config_watcher(&link);
        if self.watch_hangup {
            let hangup_link = link.clone();
            if let Err(err) = platform::server_lifecycle::on_hangup(move || {
                hangup_link.send(Msg::Hangup);
            }) {
                eprintln!("rozi: could not watch for terminal hangup: {err}");
            }
        }
        if let Some(listener) = self.control_listener.take() {
            let listener_link = link.clone();
            let event_hub = self.event_hub.clone();
            std::thread::spawn(move || {
                crate::control::run_listener(listener, listener_link, event_hub)
            });
        }
        let theme_tick = self.theme_tick;
        let workbar_tick = self.workbar_tick;
        self.start_session(link.clone());
        if theme_tick {
            std::thread::sleep(Duration::from_millis(150));
            link.send(Msg::ThemeTick);
        }
        if workbar_tick {
            link.send(Msg::WorkbarTick);
        }
    }

    fn start_session(self, link: CommandLink<Msg>) {
        match self.start {
            SessionStart::Attach {
                epoch,
                name,
                autostart,
                create_only,
            } => {
                std::thread::spawn(move || {
                    if let Some(remote) = self.remote {
                        crate::session::bootstrap::attach_remote_session_client(
                            epoch,
                            name,
                            self.read_only,
                            create_only,
                            remote,
                            self.remote_config,
                            false,
                            link,
                        );
                    } else if create_only {
                        crate::session::bootstrap::create_session_client(
                            epoch,
                            name,
                            self.read_only,
                            link,
                        )
                    } else {
                        attach_session_client(epoch, name, autostart, self.read_only, link)
                    }
                });
            }
            SessionStart::Picker { epoch } => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    if let Ok((rows, host_status)) =
                        crate::ops::session::discover_picker_sessions(None)
                    {
                        link.send(Msg::SessionsDiscovered {
                            epoch,
                            rows,
                            host_status,
                        });
                    }
                });
            }
            SessionStart::RemotePicker { target } => {
                link.send(Msg::RemotePickerHostActivate(target));
            }
        }
    }
}

struct StartupPlan {
    config: Config,
    messages: Vec<String>,
    attach_session: Option<String>,
    autostart: bool,
    create_only: bool,
    profile: Option<StartupProfile>,
    remote: Option<crate::session::remote::RemoteTarget>,
    last_session: Option<String>,
    want_picker: bool,
}

impl StartupPlan {
    fn resolve(cli: &cli::CliArgs, loaded: config::LoadedConfig) -> Self {
        let explicit_target = cli.attach_session.is_some();
        let remote = resolve_startup_remote(cli.remote.as_deref(), &loaded.config);
        let mut plan = Self {
            messages: loaded.warnings,
            attach_session: cli.attach_session.clone(),
            autostart: cli.attach_session.is_none(),
            create_only: false,
            profile: None,
            remote,
            last_session: None,
            want_picker: false,
            config: loaded.config,
        };
        plan.apply_session_policy(cli);
        plan.resolve_session_target(cli, explicit_target);
        plan.load_fallback_profile();
        // Picker at startup only for a bare launch. `last` and `profile` reach here when the
        // named session could not be opened. Under `--remote` this opens that host's picker.
        plan.want_picker = plan.attach_session.is_none()
            && (cli.pick
                || matches!(
                    plan.config.session.startup,
                    config::SessionStartup::Picker
                        | config::SessionStartup::Last
                        | config::SessionStartup::Profile
                ));
        plan
    }

    fn apply_session_policy(&mut self, cli: &cli::CliArgs) {
        // Startup policy chooses a session only when the user named none. `--pick` asks for the
        // picker by hand, and an explicit session target overrides the policy outright.
        if self.attach_session.is_some() || cli.pick {
            return;
        }
        match self.config.session.startup {
            // `last` reopens a session, it never revives one. Locally that is settled here.
            // Remotely it cannot be: answering "is `backend` still on workbox?" means an SSH round
            // trip before the first frame. The name rides with the host picker instead, and is
            // attached only if the host still lists it.
            config::SessionStartup::Last if self.remote.is_some() => {
                self.last_session = crate::session::read_last_session(self.remote.as_ref());
            }
            config::SessionStartup::Last => match resolve_last_session_target() {
                LastSessionTarget::Reopen(name) => {
                    self.attach_session = Some(name);
                    self.autostart = true;
                }
                LastSessionTarget::Pick(name) => self.last_session = name,
            },
            config::SessionStartup::Profile => {
                match resolve_profile_session_target(&self.config, self.remote.as_ref()) {
                    Ok(name) => {
                        self.attach_session = Some(name);
                        self.autostart = true;
                    }
                    Err(warning) => self.messages.push(warning),
                }
            }
            config::SessionStartup::Picker | config::SessionStartup::Ephemeral => {}
        }
    }

    fn resolve_session_target(&mut self, cli: &cli::CliArgs, explicit_target: bool) {
        let Some(name) = self.attach_session.clone() else {
            return;
        };
        if !crate::session::discovery::valid_session_name(&name) {
            startup_fatal(format!("Invalid session name `{name}`."));
        }
        if self.remote.is_some() {
            self.resolve_remote_session(cli, &name, explicit_target);
        } else {
            self.resolve_local_session(cli, &name, explicit_target);
        }
    }

    fn resolve_remote_session(&mut self, cli: &cli::CliArgs, name: &str, explicit_target: bool) {
        // Under `--remote` the session lives on the far host. Local discovery and local profiles
        // do not describe it. `New` is still create-only, enforced remotely via `server_started`.
        match cli.session_command {
            cli::SessionCommand::New => {
                self.autostart = true;
                self.create_only = true;
                self.profile = requested_new_profile(cli);
            }
            cli::SessionCommand::Attach => self.autostart = false,
            cli::SessionCommand::Dwim => {
                self.autostart = true;
                // A name startup policy chose (`last`, `profile`), not one the user typed. If the
                // host has no session under it, this is a launch rather than an attach, and it gets
                // the same canonical `profiles/<name>.toml` a local one would.
                let path = config::profile_path_for_name(name);
                if !explicit_target
                    && path.exists()
                    && self
                        .remote
                        .as_ref()
                        .is_some_and(|target| !remote_session_known(target, name))
                {
                    match try_load_startup_profile(name, path) {
                        Ok(profile) => self.profile = Some(profile),
                        Err(message) => self.messages.push(message),
                    }
                }
            }
        }
    }

    fn resolve_local_session(&mut self, cli: &cli::CliArgs, name: &str, explicit_target: bool) {
        let running = crate::session::discovery::discover_session(name)
            .ok()
            .flatten()
            .is_some();
        let path = config::profile_path_for_name(name);
        match cli.session_command {
            cli::SessionCommand::Attach => {
                if !running {
                    let hint = path
                        .exists()
                        .then(|| format!("\nStart it with: rozi {name}"));
                    startup_fatal(format!(
                        "Session `{name}` is not running.{}",
                        hint.unwrap_or_default()
                    ));
                }
                self.autostart = false;
            }
            cli::SessionCommand::New => {
                if running {
                    startup_fatal(format!(
                        "Session `{name}` is already running.\nAttach with: rozi sessions attach {name}"
                    ));
                }
                self.autostart = true;
                self.create_only = true;
                self.profile = requested_new_profile(cli);
            }
            cli::SessionCommand::Dwim if explicit_target => {
                if running {
                    self.autostart = false;
                } else if cli.read_only {
                    startup_fatal(format!("Session `{name}` is not running."));
                } else if path.exists() {
                    self.profile = Some(load_startup_profile(name, path));
                    self.autostart = true;
                } else {
                    startup_fatal(format!(
                        "No session or profile named `{name}`.\nCreate it with: rozi sessions new {name}"
                    ));
                }
            }
            // Reached only for a target startup policy chose (`last`, `profile`), never one the
            // user typed: a profile that will not load reports the failure and opens the session
            // blank, rather than refusing to launch rozi at all until the file is fixed.
            cli::SessionCommand::Dwim => {
                self.autostart = true;
                if !running && path.exists() {
                    match try_load_startup_profile(name, path) {
                        Ok(profile) => self.profile = Some(profile),
                        Err(message) => self.messages.push(message),
                    }
                }
            }
        }
    }

    fn load_fallback_profile(&mut self) {
        if self.attach_session.is_some() || self.profile.is_some() {
            return;
        }
        if let Some(name) = &self.config.profile.default {
            let path = config::profile_path_for_name(name);
            match profiles::load_profile(&path) {
                Ok(profile) => {
                    self.profile = Some(StartupProfile {
                        profile,
                        name: name.clone(),
                        path,
                        records_origin: true,
                    });
                    return;
                }
                Err(err) => self
                    .messages
                    .push(format!("Default profile `{name}` load failed: {err}")),
            }
        }
        if !self.config.session.autosave {
            return;
        }
        let Some(path) = profiles::session_path(&self.config).filter(|path| path.exists()) else {
            return;
        };
        match profiles::load_profile(&path) {
            Ok(profile) => {
                self.profile = Some(StartupProfile {
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
            Err(err) => self.messages.push(format!("Session restore failed: {err}")),
        }
    }
}

fn requested_new_profile(cli: &cli::CliArgs) -> Option<StartupProfile> {
    let profile_name = cli.profile.as_ref()?;
    if !crate::session::discovery::valid_session_name(profile_name) {
        startup_fatal(format!("Invalid profile name `{profile_name}`."));
    }
    let path = config::profile_path_for_name(profile_name);
    if !path.exists() {
        startup_fatal(format!("Profile `{profile_name}` does not exist."));
    }
    Some(load_startup_profile(profile_name, path))
}

impl Default for AppRoot {
    fn default() -> Self {
        let config = Config::default();
        Self {
            initial_theme: ThemePreset::Rozi.theme(),
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
            startup_last_session: None,
            watch_hangup: false,
            startup_tasks: false,
            startup_update_check: false,
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
        startup_last_session: Option<String>,
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
            startup_last_session,
            watch_hangup: true,
            startup_tasks: true,
            startup_update_check: true,
            event_hub: events::EventHub::default(),
        }
    }

    pub(crate) fn configured_for_test(
        config: Config,
        listener: crate::platform::ipc::IpcListener,
        guard: control::ControlSocketGuard,
    ) -> Self {
        let mut app = Self::new(
            config,
            ThemePreset::Lipan.theme(),
            None,
            None,
            Vec::new(),
            Some(listener),
            Some(guard),
            None,
            false,
            false,
            false,
            None,
            false,
            None,
        );
        app.startup_update_check = false;
        app
    }

    fn start_theme_watcher(ctx: &mut Context<Self>) {
        let Some(path) =
            config::resolve_choice(&ctx.state.config.theme.name).and_then(|choice| match choice {
                config::ThemeChoice::Custom { path, .. } => Some(path),
                _ => None,
            })
        else {
            return;
        };
        match ThemeWatcher::new(path, ThemePreset::Lipan.theme()) {
            Ok(watcher) => ctx.state.theme_watcher = Some(watcher),
            Err(err) => {
                crate::pane::pty_events::notify_error(ctx, "Theme watch failed", err.to_string());
            }
        }
    }

    fn prepare_session_start(&mut self, ctx: &mut Context<Self>) -> SessionStart {
        if self.want_startup_picker {
            let seed = std::mem::take(ctx.state.current_mut());
            ctx.state.launcher_seed = Some(seed);
            return match self.remote.clone() {
                Some(target) => {
                    ctx.state.launcher_scope = Some(target.clone());
                    ops::session::open_startup_remote_picker(
                        ctx,
                        target.clone(),
                        self.startup_last_session.take(),
                    );
                    SessionStart::RemotePicker { target }
                }
                None => {
                    let epoch = ops::session::open_startup_session_picker(
                        ctx,
                        self.startup_last_session.take(),
                    );
                    SessionStart::Picker { epoch }
                }
            };
        }

        let name = self.attach_session.clone().unwrap_or_else(|| {
            if self.remote.is_some() {
                state::remote_ephemeral_session_name()
            } else {
                state::ephemeral_session_name()
            }
        });
        let epoch = ctx.state.runtime_epoch;
        let autostart = self.startup_autostart && !self.read_only;
        let intent =
            self.startup_profile
                .as_ref()
                .map_or(crate::state::AttachIntent::Plain, |profile| {
                    if profile.records_origin {
                        crate::state::AttachIntent::ProfileSeed {
                            profile: profile.name.clone(),
                            path: profile.path.clone(),
                        }
                    } else {
                        crate::state::AttachIntent::Plain
                    }
                });
        let remote_host = self.remote.as_ref().map(|target| target.display_label());
        ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
            epoch,
            name: name.clone(),
            client: None,
            autostart,
            read_only: self.read_only,
            reconnect: false,
            remote_host: remote_host.clone(),
            intent,
            left: None,
            parked_epoch: None,
        });
        ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
        ctx.state.current_mut().auto_created = self.attach_session.is_none();
        ctx.state.current_mut().remote_host = remote_host;
        ctx.state.current_mut().remote_target = self.remote.clone();
        SessionStart::Attach {
            epoch,
            name,
            autostart,
            create_only: self.startup_create_only,
        }
    }
}

fn framework_focus_message(change: &FocusChanged) -> Option<Msg> {
    let new_pane = change.new.as_ref().and_then(|entry| {
        entry
            .keys()
            .find_map(|key| view::pane_id_from_window_key(key.as_ref()))
    });
    if let Some(id) = new_pane {
        return Some(Msg::FrameworkFocusEnteredPane(Some(id)));
    }

    let left_sidebar = change
        .old
        .as_ref()
        .is_some_and(|entry| entry.is_within_key(view::sidebar_region_key()))
        && !change
            .new
            .as_ref()
            .is_some_and(|entry| entry.is_within_key(view::sidebar_region_key()));
    left_sidebar.then_some(Msg::FrameworkFocusEnteredPane(None))
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
            crate::pane::pty_events::notify_error(ctx, "Startup warning", message);
        }
        Self::start_theme_watcher(ctx);
        let start = self.prepare_session_start(ctx);
        let tasks = StartupTasks {
            enabled: self.startup_tasks,
            update_check: self.startup_update_check,
            watch_hangup: self.watch_hangup,
            control_listener: self.control_listener.take(),
            event_hub: self.event_hub.clone(),
            start,
            read_only: self.read_only,
            remote: self.remote.clone(),
            remote_config: self.config.remote.clone(),
            theme_tick: ctx.state.theme_watcher.is_some(),
            workbar_tick: ctx.state.config.workbar.has_clock(),
        };
        Some(Command::spawn(move |link| tasks.run(link)))
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        update::handle_msg(self, msg, ctx)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        if let Some(update) = Self::handle_help_overlay_key(ctx, key) {
            return update;
        }
        let (handled, mut update) = routing::handle_key_routing(ctx, key, None);
        if ops::theme::apply_terminal_palette_to_state(&mut ctx.state) {
            let command = update.command.take();
            update = Update::with_command(command);
        }
        commands::sync_if_needed(ctx);
        // Key routing can mutate the layout without going through `handle_msg` (prefix-mode window
        // management), so schedule the same commit chokepoint here to publish those changes.
        crate::ops::session::schedule_layout_commit(ctx);
        if handled {
            KeyUpdate::handled(update)
        } else {
            KeyUpdate::unhandled(update)
        }
    }

    fn on_window_focus_changed(&mut self, focused: bool, ctx: &mut Context<Self>) -> Update {
        ops::focus::window_focus_changed(ctx, focused)
    }

    fn on_focus_changed(&self, change: &FocusChanged) -> Option<Self::Message> {
        framework_focus_message(change)
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
        target_rect: FloatRect,
    ) -> TransitionConfig {
        Self::geometry_transition_for_pane(&ctx.state, pane, viewport_changed, Some(target_rect))
    }

    /// Geometry transition policy for a pane. Extracted so tests can assert Scrollable resize
    /// instant-vs-AxisChange behavior without constructing a live [`Context`].
    ///
    /// `target_rect` sizes the Slide spring's amplitude and is only known to the view; without it the
    /// spring degrades to the plain geometry curve rather than guessing an amplitude.
    fn handle_help_overlay_key(ctx: &mut Context<Self>, key: KeyEvent) -> Option<KeyUpdate> {
        if !ctx.state.show_help {
            return None;
        }
        if ctx.has_focus_within_key(crate::view::help_filter_key())
            && key.code == KeyCode::Enter
            && !key.mods.ctrl
            && !key.mods.alt
            && !key.mods.super_key
        {
            ctx.request_focus(crate::view::help_scroll_key());
            return Some(KeyUpdate::handled(Update::full()));
        }
        if key.is(KeyCode::Esc) {
            ctx.link().send(Msg::HelpEscape);
            return Some(KeyUpdate::handled(Update::full()));
        }
        if !ctx.has_focus_within_key(crate::view::help_filter_key())
            && key.code == KeyCode::Char('/')
            && key.mods == KeyMods::NONE
        {
            ctx.request_focus(crate::view::help_filter_key());
            return Some(KeyUpdate::handled(Update::full()));
        }
        None
    }

    fn geometry_animation_enabled(state: &State, pane: &Pane, viewport_changed: bool) -> bool {
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
            return false;
        }
        let animations = state.config.animations;
        if !animations.enabled {
            return false;
        }
        match state.animation {
            GeometryAnimation::None => false,
            GeometryAnimation::Spawn => animations.spawn,
            GeometryAnimation::Close => animations.close,
            GeometryAnimation::Fullscreen => animations.fullscreen,
            GeometryAnimation::TileFloat => animations.tile_float,
            GeometryAnimation::AxisChange => animations.axis_change,
        }
    }

    pub(crate) fn geometry_transition_for_pane(
        state: &State,
        pane: &Pane,
        viewport_changed: bool,
        target_rect: Option<FloatRect>,
    ) -> TransitionConfig {
        if !Self::geometry_animation_enabled(state, pane, viewport_changed) {
            return anim::instant_transition();
        }

        let animations = state.config.animations;
        // An arriving or leaving pane that slides does not animate its rectangle at all: it is placed
        // at its destination and a rigid offset carries it in from the edge, so it keeps its final
        // size the whole way. Animating the rect too would fight the offset for the same motion.
        if (pane.opening || pane.closing) && anim::pane_slides(animations, pane) {
            return anim::instant_transition();
        }

        // The close scale has to finish inside the close animation's own window. Running it at
        // `geometry_duration` (which is longer) means the pane is pruned part-way through, so the
        // slow start of the easing is all that is ever seen.
        if pane.closing {
            return anim::close_geometry_transition(animations.close_duration);
        }

        // Under Slide, the tiles *around* an arriving or leaving pane are where the spring lives:
        // this is the tile that gave up the space, or the one taking it back.
        if animations.pane_style == anim::PaneAnimationStyle::Slide
            && matches!(
                state.animation,
                GeometryAnimation::Spawn | GeometryAnimation::Close
            )
            && let Some(rect) = target_rect
        {
            return anim::spring_geometry_transition(
                animations.geometry_duration,
                anim::spring_extent(rect),
            );
        }
        anim::geometry_transition(animations.geometry_duration)
    }

    /// Slide progress for an opening or closing pane: `0.0` fully outside its tile, `1.0` deployed.
    ///
    /// Mirrors [`window_opacity_config`](Self::window_opacity_config)'s mechanism - the target flips
    /// when `opening` clears or `closing` is set, and the transition carries the pane the rest of the
    /// way. A pane that never slides reads a constant `1.0`, so the key stays alive and a config
    /// change mid-life does not make it jump.
    pub(crate) fn slide_progress(&self, ctx: &Context<Self>, pane: &Pane, key: String) -> f32 {
        let animations = ctx.state.config.animations;
        if !anim::pane_slides(animations, pane) {
            return 1.0;
        }
        let (target, enabled) = if pane.closing {
            (0.0, animations.close)
        } else {
            (if pane.opening { 0.0 } else { 1.0 }, animations.spawn)
        };
        // Disabled means no motion, not a pane parked outside its own tile: snap to deployed rather
        // than letting an instant transition land the target of 0.0 and hide it.
        if !animations.enabled || !enabled {
            return 1.0;
        }
        let duration = anim::slide_duration(animations);
        ctx.transition(key, target, anim::slide_transition(duration))
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
        // A slide is not faded: it is clipped to its tile, so it genuinely emerges. A fade on top
        // would make the leading edge ghostly instead of solid.
        if anim::pane_slides(animations, pane) {
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
        self.chrome_color_with_frame_rate(ctx, pane, slot, target, config, None)
    }

    pub(crate) fn chrome_color_with_frame_rate(
        &self,
        ctx: &Context<Self>,
        pane: PaneId,
        slot: &str,
        target: Color,
        config: TransitionConfig,
        frame_rate: Option<u16>,
    ) -> Paint {
        self.chrome_paint_with_frame_rate(
            ctx,
            format!("rozi-pane-chrome-{pane}-{slot}"),
            target,
            config,
            frame_rate,
        )
    }

    /// A caller-keyed chrome paint. Palette/indexed colors deliberately snap rather than blend.
    pub(crate) fn chrome_paint_with_frame_rate(
        &self,
        ctx: &Context<Self>,
        key: String,
        target: Color,
        config: TransitionConfig,
        frame_rate: Option<u16>,
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
        match frame_rate {
            Some(frame_rate) => ctx.animated_color_with_frame_rate(key, target, config, frame_rate),
            None => ctx.animated_color(key, target, config),
        }
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

/// Point config loading at `--config <PATH>` before any command reads it.
///
/// Every entry point that loads config goes through here, so a server, a remote `sessions list`,
/// and the UI all honour the same flag rather than the UI alone.
fn apply_config_path(path: Option<String>) {
    if let Some(path) = path {
        unsafe {
            std::env::set_var("ROZI_CONFIG", path);
        }
    }
}

pub fn run() -> Result<()> {
    // Ahead of argument parsing: `ssh` re-executes this binary as its askpass helper with a prompt
    // where rozi expects a subcommand. The environment it was handed says so unambiguously, and
    // only `session::remote::askpass::configure` ever sets it.
    if let Some(helper) = crate::session::remote::askpass::helper_invocation() {
        helper.run();
    }

    let parsed = match cli::parse_cli_args(std::env::args().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("Run `rozi --help` for usage.");
            std::process::exit(1);
        }
    };

    // Pure extension diagnostics, namespace help, and the built-in skill do not need a runnable
    // terminal host or a healthy managed binary. Keeping them first makes them useful while
    // repairing either.
    let parsed = match parsed {
        cli::ParsedCli::Extensions(command) => match command {
            cli::ExtensionsCommand::List {
                json,
                verbose,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_list_extensions_cli(json, verbose);
            }
            cli::ExtensionsCommand::New { id } => {
                return cli::run_new_extension_cli(&id);
            }
            cli::ExtensionsCommand::Check { path, json } => {
                if !cli::run_check_extension_cli(&path, json)? {
                    std::process::exit(1);
                }
                return Ok(());
            }
            cli::ExtensionsCommand::Install {
                source,
                link,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_install_extension_cli(&source, link);
            }
            cli::ExtensionsCommand::Remove { id, config_path } => {
                apply_config_path(config_path);
                return cli::run_remove_extension_cli(&id);
            }
            cli::ExtensionsCommand::Update { id, config_path } => {
                apply_config_path(config_path);
                return cli::run_update_extension_cli(&id);
            }
        },
        cli::ParsedCli::ExtensionsHelp => {
            cli::print_extensions_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsCheckHelp => {
            cli::print_extensions_check_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsInstallHelp => {
            cli::print_extensions_install_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsRemoveHelp => {
            cli::print_extensions_remove_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsUpdateHelp => {
            cli::print_extensions_update_help();
            return Ok(());
        }
        cli::ParsedCli::SessionsHelp => {
            cli::print_sessions_help();
            return Ok(());
        }
        cli::ParsedCli::SkillHelp => {
            cli::print_skill_help();
            return Ok(());
        }
        cli::ParsedCli::Skill(command) => {
            if let Err(message) = cli::run_skill_cli(command) {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        // Ahead of recovery on purpose. Both print and exit without consulting the managed layout,
        // and they are what someone runs to work out why an installation is unhappy - a misconfigured
        // one must not be able to take them down with it.
        cli::ParsedCli::Help { advanced } => {
            cli::print_help(advanced);
            return Ok(());
        }
        cli::ParsedCli::Version => {
            cli::print_version();
            return Ok(());
        }
        parsed => parsed,
    };

    // Control clients only bridge one request to an already-running UI. Managed-installation
    // recovery verifies retained binaries and can take hundreds of milliseconds; doing that before
    // every `run-action` makes editor edge navigation visibly stall without making the request any
    // safer. The serving UI already passed recovery when it started.
    let parsed = match parsed {
        cli::ParsedCli::Control(command) => return cli::run_control_cli(command),
        cli::ParsedCli::Publish(command) => return cli::run_publish_cli(command),
        cli::ParsedCli::Subscribe(command) => return cli::run_subscribe_cli(command),
        cli::ParsedCli::Pick(command) => return cli::run_pick_cli(command),
        parsed => parsed,
    };

    // Reconcile a managed pointer before commands can update or start the application. This keeps
    // updater recovery ahead of ConPTY checks, endpoints, sessions, and the TUI.
    if let Err(message) = cli::recover_managed_installation() {
        eprintln!("rozi: {message}");
        std::process::exit(1);
    }

    let parsed = match parsed {
        cli::ParsedCli::Install => {
            if let Err(message) = cli::run_install_cli() {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        cli::ParsedCli::Update(command) => {
            if let Err(message) = cli::run_update_cli(command) {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        parsed => parsed,
    };

    // Runtime/session commands receive the host support check. Pure installation and extension
    // diagnostics above intentionally remain usable on a host that cannot launch the TUI.
    if let Err(reason) = platform::server_lifecycle::check_host_supported() {
        eprintln!("rozi: {reason}");
        std::process::exit(1);
    }

    let cli = match parsed {
        cli::ParsedCli::Server {
            name,
            fresh,
            config_path,
        } => {
            apply_config_path(config_path);
            return cli::run_server_cli(&name, fresh);
        }
        cli::ParsedCli::RemoteServe { name } => return cli::run_remote_serve_cli(&name),
        cli::ParsedCli::Sessions(command) => match command {
            cli::SessionsCommand::List {
                format,
                remote,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_list_sessions_cli(format, remote.as_deref());
            }
            cli::SessionsCommand::Kill {
                name,
                remote,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_kill_session_cli(&name, remote.as_deref());
            }
        },
        cli::ParsedCli::Run(args) => args,
        cli::ParsedCli::Control(_)
        | cli::ParsedCli::Publish(_)
        | cli::ParsedCli::Subscribe(_)
        | cli::ParsedCli::Pick(_)
        | cli::ParsedCli::Help { .. }
        | cli::ParsedCli::Version
        | cli::ParsedCli::Skill(_)
        | cli::ParsedCli::SkillHelp
        | cli::ParsedCli::SessionsHelp
        | cli::ParsedCli::Extensions(_)
        | cli::ParsedCli::ExtensionsHelp
        | cli::ParsedCli::ExtensionsCheckHelp
        | cli::ParsedCli::ExtensionsInstallHelp
        | cli::ParsedCli::ExtensionsRemoveHelp
        | cli::ParsedCli::ExtensionsUpdateHelp
        | cli::ParsedCli::Install
        | cli::ParsedCli::Update(_) => unreachable!("early CLI command returned above"),
    };

    apply_config_path(cli.config_path.clone());

    let mut plan = StartupPlan::resolve(&cli, config::load_config());
    let startup_host_colors = query_host_colors();
    let terminal_bg = startup_host_colors.map(|colors| colors.bg);
    let startup_system_theme = startup_host_colors.map(ops::theme::system_theme_from_host_colors);
    let resolved_theme =
        config::resolve_theme(&plan.config.theme.name, startup_system_theme.as_ref());
    plan.messages.extend(resolved_theme.warnings);
    let theme = ops::theme::apply_backdrop_policy(
        resolved_theme.theme,
        terminal_bg,
        plan.config.pane.background_follows_terminal,
    );

    let (control_listener, control_guard) = match control::bind_control_socket() {
        Ok((listener, guard)) => (Some(listener), Some(guard)),
        Err(err) => {
            plan.messages
                .push(format!("Control socket unavailable: {err}"));
            (None, None)
        }
    };

    let app = App::new()
        .title("rozi")
        .theme(theme.clone())
        .terminal_bg(terminal_bg)
        .toast_placement(ToastPlacement::BottomEnd)
        .toast_margin((1, 2, 1, 1))
        .clipboard_config(clipboard_config(&plan.config))
        // Read once at startup: the runner turns this into its poll interval, so a live config
        // reload cannot move it. Detaching and reattaching picks up a new value.
        .frame_rate(plan.config.frame_rate)
        .mouse(true)
        // Leader chords (`ctrl-a c`) and WM-modifier chords (`alt-c`) are executable command
        // shortcuts (see `commands.rs`), not a framework keymap file - resolve them ahead of
        // focused widgets/terminal passthrough so they win regardless of what has focus.
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
        // Pressing the prefix is an explicit entry into rozi's command state, so the next key
        // belongs to rozi whatever it turns out to be: it runs a binding, cancels with Esc, is
        // forwarded by the explicit `<prefix> <prefix>` command, or - being unbound - does nothing.
        // The default policy instead replays an unbound key into the pane, which makes a mistyped
        // chord type a stray character into the shell.
        .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly)
        // How long the prefix is held before the which-key strip appears. Only that strip reads the
        // delayed signal; the PREFIX badge, the withheld caret, and prefix mouse gestures all stay
        // on the instant `command_chord_pending`.
        .command_chord_reveal_delay(plan.config.input.which_key.reveal_delay())
        // Ctrl-q is unbound: rozi's own `quit`/`detach` commands own client lifecycle exits.
        .global_quit(None);

    // Probe / prompt / install before the TUI takes stdin (install = "prompt").
    if let Some(ref target) = plan.remote {
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if let Err(err) =
            crate::session::remote::ensure_remote_binary(target, &plan.config.remote, interactive)
        {
            eprintln!("rozi: {err}");
            std::process::exit(1);
        }
    }

    let outcome = app
        .mount(AppRoot::new(
            plan.config,
            theme,
            startup_system_theme,
            plan.profile,
            plan.messages,
            control_listener,
            control_guard,
            plan.attach_session,
            plan.autostart,
            plan.create_only,
            cli.read_only,
            plan.remote,
            plan.want_picker,
            plan.last_session,
        ))
        .exit_view(crate::view::exit::exit_view)
        .run();
    // The control socket has a guard the app owns; the askpass endpoint is reached from worker
    // threads with no such owner, so it is retired here.
    crate::session::remote::askpass::shutdown();
    outcome
}

/// Load a profile the user named explicitly. A target typed on the command line is a request that
/// either happens or fails, so an unreadable profile ends the launch.
fn load_startup_profile(name: &str, path: PathBuf) -> StartupProfile {
    match try_load_startup_profile(name, path) {
        Ok(profile) => profile,
        Err(message) => startup_fatal(message),
    }
}

fn try_load_startup_profile(
    name: &str,
    path: PathBuf,
) -> std::result::Result<StartupProfile, String> {
    match profiles::load_profile(&path) {
        Ok(profile) => Ok(StartupProfile {
            profile,
            name: name.to_string(),
            path,
            records_origin: true,
        }),
        Err(err) => Err(format!("Profile `{name}` load failed: {err}")),
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

/// Resolve `--remote`'s argument into the host the launch is scoped to. `None` for a local launch;
/// a bare `--remote` takes `[remote] default_host`. Exits on an unusable target, the way every other
/// malformed command line does — the alternative is a TUI that comes up pointed nowhere.
fn resolve_startup_remote(
    raw: Option<&str>,
    config: &config::Config,
) -> Option<crate::session::remote::RemoteTarget> {
    let (raw, label) = match raw? {
        "" => {
            let Some(default_host) = config.remote.default_host.as_deref() else {
                eprintln!(
                    "--remote requires a host alias or ssh:// URL (or set [remote] default_host)"
                );
                std::process::exit(1);
            };
            (default_host, "[remote] default_host: ")
        }
        raw => (raw, ""),
    };
    match crate::session::remote::parse_remote_target(raw) {
        Ok(target) => Some(target),
        Err(err) => {
            eprintln!("{label}{err}");
            std::process::exit(1);
        }
    }
}

/// `startup = "last"` for a local launch, where openability is a local question with a local answer.
/// The remote scope resolves through host discovery instead — see the `Last` arm of the startup
/// policy.
fn resolve_last_session_target() -> LastSessionTarget {
    let Some(last) = crate::session::read_last_session(None) else {
        return LastSessionTarget::Pick(None);
    };
    let reopenable = session_openable_by_name(&last);
    select_last_session_target(last, reopenable)
}

/// Whether attaching to this name would land on something: a running server, a resurrection
/// snapshot the server can restore, or its canonical same-name profile to launch from.
fn session_openable_by_name(name: &str) -> bool {
    crate::session::discovery::discover_session(name)
        .ok()
        .flatten()
        .is_some()
        || crate::session::server::list_snapshot_names_by_recency()
            .iter()
            .any(|snapshot| snapshot == name)
        || config::profile_path_for_name(name).exists()
}

/// Whether `name` is a session rozi has seen on `target`, from the persisted per-host cache.
///
/// The cache is the only thing that can answer this at launch: the host has not been contacted yet,
/// and adding an SSH round trip before the first frame would trade a host-scoped `last` for several
/// seconds of black terminal. Being a cache, it can be wrong in both directions — and both are
/// recoverable. A session that has since died is autostarted by name on the far host, which is what
/// `rozi --remote workbox dev` does anyway; one the cache has never heard of drops the launch into
/// `Sessions · workbox`, where the live list is one probe away.
fn remote_session_known(target: &crate::session::remote::RemoteTarget, name: &str) -> bool {
    let cache = crate::session::read_host_session_cache();
    crate::session::host_sessions_for(&cache, target)
        .is_some_and(|sessions| sessions.iter().any(|session| session.name == name))
}

/// `[session] startup = "profile"`: the session named after `[profile] default`, in the scope the
/// launch names. Returns the warning to report when there is nothing to open under that name; the
/// launch then takes that scope's picker rather than attaching some other session. No picker
/// highlight: an unresolvable name has no row to land on.
///
/// Under `--remote` the session lives on the host but the profile is a local file, so either one
/// makes the name openable: a session of that name already on the host, or a profile here to seed a
/// new one with. The profile itself is never sent — it is launch intent the client replays.
///
/// Nothing is written back. Settings withholds this mode until a default profile exists and clears it
/// when one goes away, so reaching the first case means the config was hand-written or synced in,
/// where the profile may be missing only on this machine or only until a checkout finishes.
/// Overwriting it there would discard an intent that is still in use.
fn resolve_profile_session_target(
    config: &config::Config,
    scope: Option<&crate::session::remote::RemoteTarget>,
) -> std::result::Result<String, String> {
    let Some(name) = config.profile.default.as_deref() else {
        return Err(
            "Startup mode needs a default profile: set one in Profiles with ctrl+f.".to_string(),
        );
    };
    if !crate::session::discovery::valid_session_name(name) {
        return Err(format!(
            "Profile `{name}` is not a usable session name; ignored session.startup = \"profile\"."
        ));
    }
    let openable = match scope {
        None => session_openable_by_name(name),
        Some(target) => {
            remote_session_known(target, name) || config::profile_path_for_name(name).exists()
        }
    };
    if !openable {
        let where_ = scope
            .map(|target| format!(" on `{}`", target.display_label()))
            .unwrap_or_default();
        return Err(format!(
            "No session or profile named `{name}`{where_}; ignored session.startup = \"profile\"."
        ));
    }
    Ok(name.to_string())
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
    fn requested_startup_picker_opens_even_without_candidates() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let root = AppRoot {
                    want_startup_picker: true,
                    ..Default::default()
                };

                let backend = TestBackend::new(root);

                assert!(backend.state().show_session_picker);
                assert!(backend.state().is_launcher());
                assert!(backend.state().current().pending_session_attach.is_none());
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

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
                backend.state_mut().current_mut().session_attached = true;
                backend.state_mut().profile_picker = Some(picker);
                backend.state_mut().show_profile_picker = true;
                backend.render();

                let lines = backend.capture_frame().to_fixed_grid_lines();
                assert!(lines.iter().any(|line| line.contains("attach Enter")));
                assert!(lines.iter().any(|line| line.contains("launch as Ctrl+o")));
                assert!(lines.iter().any(|line| line.contains("default Ctrl+f")));
                assert!(lines.iter().any(|line| line.contains("replace Ctrl+r")));
                assert!(lines.iter().any(|line| line.contains("• running")));
                assert!(lines.iter().any(|line| line.contains("new Ctrl+n")));
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn profile_picker_omits_replace_in_the_launcher() {
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
                backend.state_mut().profile_picker =
                    Some(crate::state::ProfilePickerState::new(vec![
                        crate::config::ProfileEntry {
                            name: "rust-dev".to_string(),
                            path: PathBuf::from("rust-dev.toml"),
                        },
                    ]));
                backend.state_mut().show_profile_picker = true;
                assert!(!backend.state().current().session_attached);
                backend.render();

                let lines = backend.capture_frame().to_fixed_grid_lines();
                assert!(lines.iter().any(|line| line.contains("launch as Ctrl+o")));
                assert!(
                    lines.iter().all(|line| !line.contains("replace")),
                    "replace is not offered until a session is attached\n{}",
                    lines.join("\n")
                );
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

    /// `last` reads the scope the launch names, and never the other one: a session remembered on
    /// `workbox` must not become what a bare `rozi` reaches for.
    #[test]
    fn startup_last_reads_only_its_own_scope() {
        crate::test_support::isolate_user_dirs();
        let workbox = crate::session::remote::RemoteTarget::Alias("startup-scope-box".into());
        crate::session::record_last_session(Some(&workbox), "backend");

        assert_eq!(
            crate::session::read_last_session(Some(&workbox)).as_deref(),
            Some("backend"),
            "the remote scope carries the name the host's picker will try to resume"
        );
        assert_eq!(
            resolve_last_session_target(),
            LastSessionTarget::Pick(None),
            "and the local scope has learned nothing from it"
        );
    }

    /// Under `--remote` a local profile of that name is enough to open the default-profile session
    /// on the host: the profile is launch intent the client replays, not a file the host needs.
    #[test]
    fn startup_profile_resolves_against_the_host_or_a_local_profile() {
        crate::test_support::isolate_user_dirs();
        let workbox = crate::session::remote::RemoteTarget::Alias("profile-scope-box".into());
        let mut config = crate::config::Config::default();
        config.profile.default = Some("profile-scope-session".to_string());

        assert!(
            resolve_profile_session_target(&config, Some(&workbox))
                .expect_err("neither on the host nor here")
                .contains("profile-scope-box"),
            "the warning has to name the host it looked on"
        );

        let profiles = crate::config::profiles_dir();
        std::fs::create_dir_all(&profiles).expect("profiles dir");
        let path = profiles.join("profile-scope-session.toml");
        crate::profiles::save_profile(&path, &crate::profiles::Profile::default())
            .expect("write profile");
        let resolved = resolve_profile_session_target(&config, Some(&workbox));
        let _ = std::fs::remove_file(&path);
        assert_eq!(resolved, Ok("profile-scope-session".to_string()));
    }

    /// `profile` mode resolves to the canonical session name so the untargeted `Dwim` path can
    /// attach it or launch it from `profiles/<name>.toml`.
    #[test]
    fn startup_profile_targets_the_session_named_after_the_default_profile() {
        let profiles = crate::config::profiles_dir();
        std::fs::create_dir_all(&profiles).expect("profiles dir");
        let path = profiles.join("startup-profile-mode.toml");
        crate::profiles::save_profile(&path, &crate::profiles::Profile::default())
            .expect("write profile");

        let mut config = crate::config::Config::default();
        config.profile.default = Some("startup-profile-mode".to_string());
        let resolved = resolve_profile_session_target(&config, None);

        let _ = std::fs::remove_file(&path);
        assert_eq!(resolved, Ok("startup-profile-mode".to_string()));
    }

    /// The mode reads a config key it cannot validate at parse time, so every unusable value falls
    /// through to the picker with a warning instead of exiting or attaching something else.
    #[test]
    fn startup_profile_defers_to_the_picker_without_a_usable_default() {
        let config = crate::config::Config::default();
        assert!(config.profile.default.is_none());
        // Actionable rather than a config-key restatement: the fix is one keypress in Profiles.
        assert!(
            resolve_profile_session_target(&config, None)
                .expect_err("no default configured")
                .contains("set one in Profiles")
        );

        let mut config = crate::config::Config::default();
        config.profile.default = Some("not a session name".to_string());
        assert!(
            resolve_profile_session_target(&config, None)
                .expect_err("invalid session name")
                .contains("not a usable session name")
        );

        let mut config = crate::config::Config::default();
        config.profile.default = Some("rozi-no-such-profile-xyzzy".to_string());
        assert!(
            resolve_profile_session_target(&config, None)
                .expect_err("nothing to open")
                .contains("No session or profile named")
        );
    }

    #[test]
    fn theme_picker_separates_groups_and_marks_signature_themes() {
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

                let lines = backend.capture_frame().to_fixed_grid_lines();
                let rendered = lines.join("\n");
                assert!(rendered.contains("System"));
                assert!(rendered.contains("Dark"));
                assert!(rendered.contains("Light"));
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("Rozi") && line.contains("signature")),
                    "{rendered}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("Lipan") && line.contains("signature")),
                    "{rendered}"
                );

                let row = |needle: &str| {
                    let prefix = format!("│ {needle}");
                    lines
                        .iter()
                        .position(|line| line.contains(&prefix))
                        .unwrap_or_else(|| panic!("missing {needle}: {rendered}"))
                };
                assert!(row("Dark") >= row("System") + 2, "{rendered}");
                assert!(row("Light") >= row("Zenburn") + 2, "{rendered}");
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
    fn command_palette_empty_query_starts_with_new_pane() {
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

                let before = backend.state().current().workspaces[0].panes.len();
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("activate first command");

                assert!(!backend.state().show_palette);
                let spawned = backend.state().focused_pane().expect("spawn takes focus");
                let panes = &backend.state().current().workspaces[0].panes;
                assert_eq!(panes.len(), before + 1);
                let pane = panes
                    .iter()
                    .find(|pane| pane.id == spawned)
                    .expect("spawned pane");
                assert!(!pane.floating);
            })
            .expect("spawn palette selection test thread")
            .join()
            .expect("palette selection test thread completes");
    }

    #[test]
    fn command_palette_sidebar_query_selects_live_toggle_first() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                for initially_visible in [false, true] {
                    let mut backend = TestBackend::new(AppRoot::default());
                    backend.set_viewport(Rect {
                        x: 0,
                        y: 0,
                        w: 96,
                        h: 40,
                    });
                    if initially_visible {
                        backend
                            .dispatch(Msg::RunAction(crate::input::Action::ToggleSidebar))
                            .expect("show sidebar");
                    }
                    backend
                        .dispatch(Msg::RunAction(crate::input::Action::TogglePalette))
                        .expect("open command palette");
                    backend.render();

                    for ch in "sidebar".chars() {
                        backend
                            .send_key(KeyEvent {
                                code: KeyCode::Char(ch),
                                mods: KeyMods::NONE,
                            })
                            .expect("type sidebar query");
                    }
                    backend.render();

                    let rows: Vec<_> = backend
                        .capture_frame()
                        .to_fixed_grid_lines()
                        .into_iter()
                        .filter(|line| {
                            [
                                "Enable sidebar",
                                "Disable sidebar",
                                "sidebar split",
                                "Focus sidebar",
                                "Next sidebar",
                                "Previous sidebar",
                            ]
                            .iter()
                            .any(|label| line.contains(label))
                        })
                        .collect();
                    let expected = if initially_visible {
                        "Disable sidebar"
                    } else {
                        "Enable sidebar"
                    };
                    assert!(
                        rows.first().is_some_and(|row| row.contains(expected)),
                        "live toggle should be the first sidebar match: {rows:#?}"
                    );

                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Enter,
                            mods: KeyMods::NONE,
                        })
                        .expect("activate sidebar toggle");
                    assert_eq!(backend.state().sidebar_visible, !initially_visible);
                    assert!(!backend.state().show_palette);
                }
            })
            .expect("spawn sidebar ranking test thread")
            .join()
            .expect("sidebar ranking test thread completes");
    }

    #[test]
    fn clearing_sidebar_query_restores_initial_command_selection() {
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

                for ch in "sidebar".chars() {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Char(ch),
                            mods: KeyMods::NONE,
                        })
                        .expect("type sidebar query");
                }
                for _ in 0.."sidebar".len() {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Backspace,
                            mods: KeyMods::NONE,
                        })
                        .expect("clear sidebar query");
                }
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Enter,
                        mods: KeyMods::NONE,
                    })
                    .expect("activate restored first command");

                assert!(!backend.state().show_palette);
                let spawned = backend.state().focused_pane().expect("spawn takes focus");
                let pane = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id == spawned)
                    .expect("spawned pane");
                assert!(!pane.floating);
            })
            .expect("spawn palette query reset test thread")
            .join()
            .expect("palette query reset test thread completes");
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
                backend.state_mut().show_settings = true;
                backend
                    .dispatch(Msg::SettingsActivate(
                        crate::state::SettingsAction::EditPadding,
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
                let settings = frames
                    .iter()
                    .position(|w| w.title.as_deref() == Some("Settings"))
                    .expect("settings frame");
                let padding = frames
                    .iter()
                    .position(|w| w.title.as_deref() == Some("Terminal padding"))
                    .expect("padding frame");
                assert!(
                    padding > settings,
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
                    .dispatch(Msg::SettingsActivate(
                        crate::state::SettingsAction::EditPadding,
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
                    .dispatch(Msg::SettingsActivate(
                        crate::state::SettingsAction::EditPadding,
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
                    text: std::sync::Arc::from("rozi master • prompt"),
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
                assert!(rendered.contains("next Ctrl+n"), "{rendered}");
                assert!(rendered.contains("previous Ctrl+p"), "{rendered}");
                assert!(rendered.contains("pane Tab"), "{rendered}");
                assert!(rendered.contains("1 / 1 matches (pane)"), "{rendered}");
                assert!(!rendered.contains("scope:"), "{rendered}");

                let row = lines
                    .iter()
                    .position(|line| line.contains("rozi master"))
                    .expect("matching result row") as u16;
                let matched = lines[row as usize].find("master").expect("match column") as u16;
                let plain = lines[row as usize].find("rozi").expect("plain column") as u16;
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
                crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target)
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
                    crate::pane::lifecycle::find_pane(backend.state(), target)
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
                crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target)
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
                    crate::pane::lifecycle::find_pane(backend.state(), target)
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
                let pane_end = crate::pane::lifecycle::find_pane(backend.state(), target)
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
                crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target)
                    .expect("target pane")
                    .terminal
                    .process_server_output(output.as_bytes());
                backend
                    .dispatch(Msg::RunAction(crate::input::Action::OpenSearch))
                    .expect("open search");
                let pane_end = crate::pane::lifecycle::find_pane(backend.state(), target)
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
                    crate::pane::lifecycle::find_pane(backend.state(), target)
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
    fn session_picker_restorable_hints_lead_with_restore_and_forget() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 18,
                });
                backend.state_mut().current_mut().session_name = Some("eph-test".to_string());
                backend.state_mut().current_mut().session_attached = true;
                backend.state_mut().show_session_picker = true;
                backend.state_mut().session_picker =
                    Some(crate::state::SessionPickerState::new(vec![
                        crate::session::discovery::DiscoveredSession {
                            name: "saved".to_string(),
                            ephemeral: false,
                            host: None,
                            remote_target: None,
                            status: crate::session::discovery::DiscoveredSessionStatus::Restorable,
                        },
                    ]));
                backend.render();

                let lines = backend.capture_frame().to_fixed_grid_lines();
                let joined = lines.join("\n");
                assert!(
                    lines.iter().any(|line| line.contains("restore Enter")),
                    "restorable Enter should restore, not connect\n{joined}"
                );
                assert!(
                    lines.iter().any(|line| line.contains("forget Ctrl+k")),
                    "restorable Ctrl+K should forget the snapshot\n{joined}"
                );
                assert!(
                    lines.iter().any(|line| line.contains("new Ctrl+n")),
                    "{joined}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("name current Ctrl+s")),
                    "{joined}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("remote hosts Ctrl+r")),
                    "{joined}"
                );
                assert!(
                    lines.iter().all(|line| !line.contains("restart")),
                    "a snapshot has no live server to restart\n{joined}"
                );
                assert!(
                    lines.iter().all(|line| !line.contains("kill")),
                    "forgetting a snapshot is not a live kill\n{joined}"
                );
            })
            .expect("spawn restorable-hint test")
            .join()
            .expect("restorable-hint test completes");
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

    /// Under Slide the spring belongs to the tiles *making room*, never to the pane travelling into
    /// place: an arriving pane is clipped to its tile, so carrying it past its destination would open
    /// a gap at the edge it entered from rather than read as a bounce.
    #[test]
    fn slide_springs_the_neighbours_and_leaves_the_travelling_pane_alone() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = anim::PaneAnimationStyle::Slide;
        state.animation = GeometryAnimation::Spawn;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        for id in [1, 2] {
            let mut pane = Pane::new(id, 100, FloatRect::default());
            pane.opening = id == 2;
            workspace.panes.push(pane);
        }

        let tile = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 20.0,
        };
        let settled = &state.current().workspaces[0].panes[0];
        let arriving = &state.current().workspaces[0].panes[1];

        let neighbour = AppRoot::geometry_transition_for_pane(&state, settled, false, Some(tile));
        assert!(
            matches!(
                neighbour.easing,
                Easing::EaseOutBack { overshoot_permille } if overshoot_permille > 0
            ),
            "the tile making room springs, got {:?}",
            neighbour.easing
        );
        assert_eq!(
            neighbour.duration,
            state.config.animations.geometry_duration
        );

        // A bigger tile asks for a proportionally *smaller* amplitude, which is what keeps the nudge
        // a couple of cells instead of a tenth of the pane.
        let wide = FloatRect { w: 300.0, ..tile };
        let wide_neighbour =
            AppRoot::geometry_transition_for_pane(&state, settled, false, Some(wide));
        let amplitude = |config: TransitionConfig| match config.easing {
            Easing::EaseOutBack { overshoot_permille } => overshoot_permille,
            other => panic!("expected a spring, got {other:?}"),
        };
        assert!(amplitude(wide_neighbour) < amplitude(neighbour));

        // Without a rect there is no amplitude to size, so the spring degrades rather than guessing.
        let unsized_neighbour = AppRoot::geometry_transition_for_pane(&state, settled, false, None);
        assert_eq!(unsized_neighbour.easing, Easing::EaseInOutCubic);

        // The travelling pane's rectangle does not animate at all - `slide_offset` moves it.
        let travelling = AppRoot::geometry_transition_for_pane(&state, arriving, false, Some(tile));
        assert_eq!(travelling.duration, Duration::ZERO);

        // Scale is untouched: neighbours keep the plain geometry curve.
        state.config.animations.pane_style = anim::PaneAnimationStyle::Scale;
        let settled = &state.current().workspaces[0].panes[0];
        let scale_neighbour =
            AppRoot::geometry_transition_for_pane(&state, settled, false, Some(tile));
        assert_eq!(scale_neighbour.easing, Easing::EaseInOutCubic);
    }

    /// The spring is scoped to spawn and close. A fullscreen toggle or an axis flip under Slide is
    /// still an ordinary geometry move, and springing those would make the whole layout wobble
    /// whenever anything changed shape.
    #[test]
    fn slide_does_not_spring_unrelated_geometry_animations() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = anim::PaneAnimationStyle::Slide;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = false;
        workspace.panes.push(pane);

        for animation in [
            GeometryAnimation::Fullscreen,
            GeometryAnimation::TileFloat,
            GeometryAnimation::AxisChange,
        ] {
            state.animation = animation;
            let pane = &state.current().workspaces[0].panes[0];
            let config = AppRoot::geometry_transition_for_pane(
                &state,
                pane,
                false,
                Some(FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 30.0,
                    h: 20.0,
                }),
            );
            assert_eq!(
                config.easing,
                Easing::EaseInOutCubic,
                "{animation:?} is not a spawn or close"
            );
        }
    }

    /// A floating pane has no tile edge to emerge from and no neighbour to take space from, so it
    /// keeps the scale whatever the style says - and therefore keeps its fade.
    #[test]
    fn floating_panes_never_slide() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = anim::PaneAnimationStyle::Slide;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = true;
        pane.floating = true;
        workspace.panes.push(pane);

        let pane = &state.current().workspaces[0].panes[0];
        assert!(!anim::pane_slides(state.config.animations, pane));
    }
}
