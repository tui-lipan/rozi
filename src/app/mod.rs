use std::cell::Cell;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::config::Config;
use crate::input::routing;
use crate::session::bootstrap::SessionStart;
use crate::state::{State, ThemePreset};
use crate::{commands, config, control, events, ops, state, update, view};

mod entry;
mod startup;

pub use entry::run;
pub(crate) use entry::{clipboard_config, clipboard_copy_feedback_duration};
use startup::{StartupProfile, StartupTasks};

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
    /// Whether this client may contact the public release host at all, for the check it runs at
    /// startup and for the periodic ones after it. Integration apps keep their session/control
    /// workers but must never reach GitHub, whatever `[updates]` says.
    update_checks: bool,
    event_hub: events::EventHub,
    render_host_terminal_color_generation: Cell<u64>,
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
            update_checks: false,
            event_hub: events::EventHub::default(),
            render_host_terminal_color_generation: Cell::new(0),
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
            update_checks: true,
            event_hub: events::EventHub::default(),
            render_host_terminal_color_generation: Cell::new(0),
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
        app.update_checks = false;
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
        match ThemeWatcher::new(path, config::custom_theme_base()) {
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
        state.update_checks_allowed = self.update_checks;
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

        let messages = std::mem::take(&mut self.startup_messages);
        if !messages.is_empty() {
            crate::config::log_config_warnings(&messages);
            crate::pane::pty_events::notify_error(ctx, "Startup warning", messages.join("\n"));
        }
        Self::start_theme_watcher(ctx);
        let start = self.prepare_session_start(ctx);
        let tasks = StartupTasks {
            enabled: self.startup_tasks,
            update_check_interval: ctx.state.update_check_interval(),
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
        let (handled, mut update) = routing::handle_key_routing(ctx, key, None);
        if ops::theme::apply_terminal_palette_to_state(&mut ctx.state) {
            let command = update.command.take();
            update = Update::with_command(command);
        }
        commands::sync_if_needed(ctx);
        crate::update::sync_modifier_key_reporting(ctx);
        // Key routing can mutate the layout without going through `handle_msg` (prefix-mode window
        // management), so schedule the same commit chokepoint here to publish those changes.
        crate::ops::session::schedule_layout_commit(ctx);
        if handled {
            KeyUpdate::handled(update)
        } else {
            KeyUpdate::unhandled(update)
        }
    }

    fn on_modifiers_changed(&mut self, modifiers: KeyMods, ctx: &mut Context<Self>) -> Update {
        let Some(keybindings) = ctx.state.keybindings.as_mut() else {
            return Update::none();
        };
        if !keybindings.is_capturing() || keybindings.held_modifiers == modifiers {
            return Update::none();
        }
        keybindings.held_modifiers = modifiers;
        Update::full()
    }

    fn on_window_focus_changed(&mut self, focused: bool, ctx: &mut Context<Self>) -> Update {
        ops::focus::window_focus_changed(ctx, focused)
    }

    fn on_focus_changed(&self, change: &FocusChanged) -> Option<Self::Message> {
        framework_focus_message(change)
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        let host_color_generation = ctx.host_terminal_color_generation();
        if host_color_generation > self.render_host_terminal_color_generation.get() {
            self.render_host_terminal_color_generation
                .set(host_color_generation);
            ctx.link().send(Msg::HostTerminalColorsChanged);
        }
        if ctx.devtools_visible() {
            ctx.set_devtools_metrics(|| {
                crate::runtime_metrics::RuntimeMetrics::capture(ctx.state.current()).devtools_rows()
            });
        }
        view::render(ctx)
    }
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

pub(crate) fn check_for_update_and_reschedule(interval: Duration) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| startup::check_for_update(link, interval))
}

pub(crate) fn schedule_alert_pulse_tick(half_period: Duration) -> Command {
    Command::after(half_period, move |link: CommandLink<Msg>| {
        link.send(Msg::AlertPulseTick);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
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
                assert!(lines.iter().any(|line| line.contains("launch as Ctrl+O")));
                assert!(lines.iter().any(|line| line.contains("default Ctrl+F")));
                assert!(lines.iter().any(|line| line.contains("replace Ctrl+R")));
                assert!(lines.iter().any(|line| line.contains("• running")));
                assert!(lines.iter().any(|line| line.contains("new Ctrl+N")));
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
                assert!(lines.iter().any(|line| line.contains("launch as Ctrl+O")));
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

                assert!(
                    modal.rect.h <= 26,
                    "commands modal height {} exceeded 65% of 40-row viewport\n{}",
                    modal.rect.h,
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
                let settings_rect = frames[settings].rect;
                let rect = frames[padding].rect;
                assert_eq!(
                    rect.y,
                    settings_rect.y + 1,
                    "padding card sits one row below Settings, like Change keybinding on Keybindings"
                );
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
                // The modal hugs the filtered rows (well under the cap)…
                assert!(
                    filtered.h < unfiltered.h,
                    "filtered modal height {} should shrink below the capped {}",
                    filtered.h,
                    unfiltered.h
                );
                // …while its top edge stays put instead of re-centering.
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
    fn scrollback_search_uses_shared_navigation_and_highlights_matches() {
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
                search.input.set_text("r");
                search.input.set_cursor(1);
                let text: std::sync::Arc<str> = std::sync::Arc::from("rozi master • prompt");
                search.matches.push(crate::state::ScrollbackMatch {
                    offset: 0,
                    line: 1,
                    end_line: 1,
                    start_col: 0,
                    end_col: 1,
                    start_byte: 0,
                    end_byte: 1,
                    text: std::sync::Arc::clone(&text),
                    pane: 1,
                });
                search.matches.push(crate::state::ScrollbackMatch {
                    offset: 0,
                    line: 1,
                    end_line: 1,
                    start_col: 10,
                    end_col: 11,
                    start_byte: 10,
                    end_byte: 11,
                    text,
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
                            && widget.title.as_deref() == Some("Search scrollback · pane")
                    })
                    .expect("scrollback search modal");
                assert_eq!(modal.rect.w, 90);
                assert!(!modal.title.as_deref().unwrap().contains("Tab"));

                let frame = backend.capture_frame();
                let lines = frame.to_fixed_grid_lines();
                let rendered = lines.join("\n");
                assert!(!rendered.contains("next Ctrl+N"), "{rendered}");
                assert!(!rendered.contains("previous Ctrl+P"), "{rendered}");
                assert!(rendered.contains("change scope Tab"), "{rendered}");
                assert!(rendered.contains("2/2"), "{rendered}");
                assert!(!rendered.contains("scope:"), "{rendered}");
                assert!(!rendered.contains("pane 1 · row"), "{rendered}");

                let rows = lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.contains("rozi master"))
                    .map(|(row, _)| row)
                    .collect::<Vec<_>>();
                assert_eq!(rows.len(), 2, "{rendered}");
                let second_row = rows[1];
                let label_byte = lines[second_row].find("rozi master").expect("result label");
                let label = lines[second_row][..label_byte].chars().count();
                let description_byte = lines[second_row].find("row 2").expect("description");
                let description = lines[second_row][..description_byte].chars().count();
                assert_ne!(frame.cell(label as u16, second_row as u16).fg, match_fg);
                assert_eq!(
                    frame.cell((label + 10) as u16, second_row as u16).fg,
                    match_fg,
                    "the exact second occurrence should be highlighted"
                );
                assert_ne!(
                    frame.cell(description as u16, second_row as u16).fg,
                    match_fg,
                    "metadata should not inherit query highlighting"
                );
            })
            .expect("spawn snapshot test thread")
            .join()
            .expect("snapshot test thread completes");
    }

    #[test]
    fn scrollback_search_groups_broader_scopes_by_visible_workspace_and_pane_identity() {
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
                let title = crate::pane::lifecycle::find_pane(backend.state(), target)
                    .expect("target pane")
                    .titlebar_title(false);
                let mut search = crate::state::ScrollbackSearchState::new(target);
                search.scope = crate::state::SearchScope::Workspace;
                search.input.set_text("needle");
                search.matches.push(crate::state::ScrollbackMatch {
                    offset: 0,
                    line: 2,
                    end_line: 2,
                    start_col: 0,
                    end_col: 6,
                    start_byte: 0,
                    end_byte: 6,
                    text: std::sync::Arc::from("needle result"),
                    pane: target,
                });
                search.rebuild_items();
                backend.state_mut().search = Some(search);
                backend.render();

                let workspace = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(
                    workspace.contains(&format!("Pane 1 · {title}")),
                    "{workspace}"
                );
                assert!(workspace.contains("row 3 · col 1"), "{workspace}");
                assert!(!workspace.contains("pane 1 · row"), "{workspace}");

                backend.state_mut().current_mut().workspaces[0].name = Some("dev".to_string());
                backend.state_mut().search.as_mut().expect("search").scope =
                    crate::state::SearchScope::All;
                backend.render();
                let all = backend.capture_frame().to_fixed_grid_lines().join("\n");
                assert!(
                    all.contains(&format!("Workspace 1:dev · Pane 1 · {title}")),
                    "{all}"
                );
            })
            .expect("spawn grouped search test")
            .join()
            .expect("grouped search test completes");
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
                    end_line: 8,
                    start_col: 0,
                    end_col: 6,
                    start_byte: 0,
                    end_byte: 6,
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
                    .position(|line| line.contains("row 9 · col 1"))
                    .unwrap_or_else(|| {
                        panic!("long result row with complete metadata: {lines:#?}")
                    });
                let row = &lines[row_index];
                assert!(
                    row.contains("needle"),
                    "query prefix should remain visible: {row}"
                );
                assert!(row.contains('…'), "long label should be truncated: {row}");
                let query_byte = row.find("needle").expect("query prefix");
                let query_column = row[..query_byte].chars().count() as u16;
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
                    .filter(|line| line.contains("row ") && line.contains(" · col "))
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
                        code: KeyCode::Down,
                        mods: KeyMods::NONE,
                    })
                    .expect("next scanned row");
                assert_eq!(backend.state().search.as_ref().expect("search").current, 1);
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Up,
                        mods: KeyMods::NONE,
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
                        code: KeyCode::Down,
                        mods: KeyMods::NONE,
                    })
                    .expect("next row");
                assert_eq!(
                    backend.state().search.as_ref().expect("search").current,
                    121
                );
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Up,
                        mods: KeyMods::NONE,
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
                backend.state_mut().config.pane.picker_selection_style = CapStyle::Arrow;
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
                    row.contains("\u{e0b2}● ephemeral"),
                    "selection cap should sit flush against the status marker\n{row}"
                );
                assert!(
                    !row.contains("\u{e0b2} ●"),
                    "selection cap should not leave a gap before the marker\n{row}"
                );
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
                    lines.iter().any(|line| line.contains("forget Ctrl+K")),
                    "restorable Ctrl+K should forget the snapshot\n{joined}"
                );
                assert!(
                    lines.iter().any(|line| line.contains("new Ctrl+N")),
                    "{joined}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("name current Ctrl+S")),
                    "{joined}"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("remote hosts Ctrl+R")),
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
}
