mod actions;
mod anim;
mod commands;
mod config;
mod config_ops;
mod control;
mod control_ops;
mod copy_mode;
mod exit_ops;
mod focus_ops;
mod geometry;
mod identity_ops;
mod input;
mod key_routing;
mod layout;
mod pane;
mod pane_lifecycle;
mod profile_ops;
mod profiles;
mod pty_events;
mod resize_move_ops;
mod scratchpad;
mod search_ops;
mod session;
mod session_ops;
mod shared_layout;
mod state;
mod theme_ops;
mod tiling;
mod update;
mod view;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::config::HyprmuxConfig;
use crate::input::Action;
use crate::state::{Pane, PaneId, ResizeCorner, State, ThemePreset};

pub struct HyprmuxApp {
    config: HyprmuxConfig,
    initial_theme: Theme,
    initial_system_theme: Option<Theme>,
    startup_profile: Option<profiles::HyprmuxProfile>,
    startup_messages: Vec<String>,
    control_listener: Option<std::os::unix::net::UnixListener>,
    control_guard: Option<control::ControlSocketGuard>,
    attach_session: Option<String>,
    read_only: bool,
    /// Whether a bare launch should open the session picker before attaching (`--pick` or
    /// `[session] startup = "picker"`). Only honored when there is no `--attach`/`--session` and at
    /// least one named session exists at startup.
    want_startup_picker: bool,
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
        }
    }
}

impl HyprmuxApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: HyprmuxConfig,
        initial_theme: Theme,
        initial_system_theme: Option<Theme>,
        startup_profile: Option<profiles::HyprmuxProfile>,
        startup_messages: Vec<String>,
        control_listener: Option<std::os::unix::net::UnixListener>,
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
        }
    }
}

#[derive(Clone)]
pub enum Msg {
    CommandLinkReady(CommandLink<Msg>),
    RunAction(Action),
    ClosePalette,
    CloseHelp,
    CloseAppearance,
    AppearanceActivate(crate::state::AppearanceAction),
    ClosePanePaddingEditor,
    PanePaddingVerticalChanged(InputEvent),
    PanePaddingHorizontalChanged(InputEvent),
    AdvancePanePadding,
    SubmitPanePadding,
    CloseThemePicker,
    /// Index into [`config::theme_choices`]: preview the highlighted theme.
    PreviewTheme(usize),
    /// Index into [`config::theme_choices`]: commit the chosen theme.
    SelectTheme(usize),
    ThemeTick,
    WorkbarTick,
    /// The config watcher saw `hyprmux.toml` change on disk; reload it if the content differs.
    ConfigFileChanged,
    /// A `WorkbarSegment::Command` poller produced fresh output: (command string, first output line).
    WorkbarCommandOutput(String, String),
    ThemeError(String),
    CloseSearch,
    SearchQueryChanged(String),
    SearchNext(bool),
    SearchSelect(usize),
    SearchActivate(usize),
    SearchCycleScope,
    CloseRenamePane,
    RenamePaneChanged(InputEvent),
    SubmitRenamePane,
    CloseRenameSession,
    RenameSessionChanged(InputEvent),
    SubmitRenameSession,
    CloseSaveProfile,
    SaveProfileNameChanged(InputEvent),
    SubmitSaveProfile,
    CloseProfilePicker,
    ProfilePickerQueryChanged(String),
    ProfilePickerSelect(usize),
    ProfilePickerSetDefault,
    ProfilePickerDelete,
    SelectProfile(usize),
    CloseSessionPicker,
    /// Off-thread auto-refresh results for the open session picker, tagged with the opening's epoch.
    SessionsDiscovered {
        epoch: u64,
        rows: Vec<crate::session::discovery::DiscoveredSession>,
    },
    SessionPickerQueryChanged(String),
    SessionPickerSelect(usize),
    SessionPickerActivate(usize),
    SessionPickerCreateFromQuery,
    SessionPickerDetachCurrent,
    SessionPickerKillSelected,
    SessionPickerNameCurrent,
    CloseClientList,
    ClientListSelect(usize),
    ClientListGrant(usize),
    ClientListDecline(usize),
    FocusPane(PaneId),
    HoverPane(PaneId),
    BeginMove(PaneId, FloatRect, u16, u16, u16, u16, bool),
    MovePane(PaneId, i16, i16, bool),
    EndMove(PaneId, u16, u16),
    BeginResize(PaneId, ResizeCorner, u16, u16, bool),
    ResizePane(PaneId, ResizeCorner, u16, u16, u16, u16, bool),
    EndResize(PaneId),
    BeginResizeSplit(PaneId, bool, u16, u16),
    /// Drag a tiled split boundary: (left/top pane, horizontal_split, from_x, from_y, x, y).
    ResizeSplit(PaneId, bool, u16, u16, u16, u16),
    BeginResizeSplitJunction(PaneId, PaneId, u16, u16),
    /// Drag a tiled split junction: (left pane, top pane, from_x, from_y, x, y).
    ResizeSplitJunction(PaneId, PaneId, u16, u16, u16, u16),
    EndResizeSplit,
    /// Grab the scratchpad's top edge to resize its height: drag origin y (root coordinates).
    BeginScratchResize(u16),
    /// Drag the scratchpad's top edge: (from_y, y) in root coordinates.
    ScratchResize(u16, u16),
    EndScratchResize,
    FinishOpen(u64, PaneId, u64),
    ActivatePane(u64, PaneId, u64),
    PruneClosed(u64, PaneId, u64),
    PaneInput(PaneId, TerminalInputEvent),
    PaneKey(PaneId, KeyEvent),
    PaneMouse(PaneId, Vec<u8>),
    PaneResize(PaneId, u16, u16),
    PaneScroll(PaneId, usize),
    CopyFlashExpired(PaneId, u64),
    ControlRequest(control::ControlEnvelope),
    SessionConnected {
        epoch: u64,
        name: String,
        client: session::client::SessionClient,
    },
    SessionDisconnected {
        epoch: u64,
        name: String,
    },
    SessionAttachFailed {
        epoch: u64,
        message: String,
    },
    SessionAttached {
        epoch: u64,
        session: String,
        client_id: shared_layout::ClientId,
        panes: Vec<session::protocol::PaneMeta>,
        layout_rev: u64,
        layout: Option<shared_layout::SharedLayout>,
        controller: Option<shared_layout::ClientId>,
        clients: Vec<session::protocol::ClientInfo>,
        input_locked: bool,
        read_only: bool,
    },
    SessionLayoutCommitted {
        epoch: u64,
        rev: u64,
        author: shared_layout::ClientId,
        layout: shared_layout::SharedLayout,
    },
    SessionLayoutRejected {
        epoch: u64,
        current_rev: u64,
        layout: Option<shared_layout::SharedLayout>,
    },
    SessionControllerChanged {
        epoch: u64,
        controller: Option<shared_layout::ClientId>,
        reason: session::protocol::ControllerChangeReason,
    },
    SessionClientsChanged {
        epoch: u64,
        clients: Vec<session::protocol::ClientInfo>,
        input_locked: bool,
    },
    /// Another client asked this (controller) client for the layout-control lease.
    SessionControlRequested {
        epoch: u64,
        from: shared_layout::ClientId,
    },
    /// This client's pending control request was declined by the controller.
    SessionControlDeclined {
        epoch: u64,
    },
    SessionPing {
        epoch: u64,
        seq: u64,
    },
    /// Trailing-edge flush of debounced controller pane resizes (see `pty_events::handle_pane_resize`).
    FlushPaneResizes {
        epoch: u64,
    },
    /// Trailing-edge flush of the controller's shared layout.
    FlushLayoutCommit {
        epoch: u64,
    },
    SessionSpawnResult {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        pid: Option<u32>,
        ok: bool,
        error: Option<String>,
    },
    SessionOutput {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        bytes: Vec<u8>,
    },
    SessionResized {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    SessionExited {
        epoch: u64,
        pane_id: PaneId,
        generation: u64,
        code: i32,
    },
    SessionError {
        epoch: u64,
        message: String,
    },
    SessionRenamed {
        epoch: u64,
        session: String,
    },
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
        state.system_theme = self.initial_system_theme.clone();
        state.control_socket_path = self
            .control_guard
            .as_ref()
            .map(|guard| guard.path().to_path_buf());
        theme_ops::apply_terminal_palette_to_state(&mut state);
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
        let theme_tick = ctx.state.theme_watcher.is_some();
        let workbar_tick = ctx.state.config.workbar.has_clock();
        let workbar_commands = ctx.state.config.workbar.command_specs();
        ctx.state.workbar_commands_running =
            workbar_commands.iter().map(|(c, _)| c.clone()).collect();

        let start = if self.want_startup_picker && has_named_session() {
            let epoch = session_ops::open_startup_session_picker(ctx);
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
        Some(Command::spawn(move |link: CommandLink<Msg>| {
            link.send(Msg::CommandLinkReady(link.clone()));
            config_ops::spawn_config_watcher(&link);
            if let Some(listener) = control_listener {
                let listener_link = link.clone();
                std::thread::spawn(move || crate::control::run_listener(listener, listener_link));
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
            pane_lifecycle::spawn_workbar_command_pollers(workbar_commands, &link);
        }))
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        update::handle_msg(self, msg, ctx)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
        key_routing::sync_focus_from_framework(ctx);
        let (handled, mut update) = key_routing::handle_key_routing(ctx, key, None);
        if theme_ops::apply_terminal_palette_to_state(&mut ctx.state) {
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

/// How a launch begins its session: either attach straight to a session, or show the startup
/// picker and defer attaching until the user chooses.
enum SessionStart {
    Attach { epoch: u64, name: String },
    Picker { epoch: u64 },
}

/// Whether any *named* (non-ephemeral) session is currently discoverable. Used to gate the startup
/// picker: with no named session to reattach to, a bare launch skips the picker and attaches to an
/// ephemeral session as usual.
fn has_named_session() -> bool {
    session::discovery::discover_sessions()
        .map(|rows| rows.iter().any(|row| !row.ephemeral))
        .unwrap_or(false)
}

pub(crate) fn attach_session_client(
    epoch: u64,
    name: String,
    autostart: bool,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let Ok(path) = session::server::session_socket_path(&name) else {
        link.send(Msg::SessionAttachFailed {
            epoch,
            message: format!("Invalid session name `{name}`"),
        });
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut spawned = false;
    let mut server_child: Option<std::process::Child> = None;
    loop {
        let (tx, rx) = mpsc::channel();
        match session::client::SessionClient::connect_attached(&path, name.clone(), tx, read_only) {
            Ok((client, attached)) => {
                link.send(Msg::SessionConnected {
                    epoch,
                    name: name.clone(),
                    client,
                });
                link.send(server_message_to_msg(
                    epoch,
                    session::protocol::Frame::Control(attached),
                ));
                for message in rx {
                    link.send(server_message_to_msg(epoch, message));
                }
                link.send(Msg::SessionDisconnected { epoch, name });
                return;
            }
            Err(err) => {
                if is_busy_attach_error(&err) {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}` is busy or not accepting clients"),
                    });
                    return;
                }
                if is_handshake_rejected(&err) {
                    // The server answered but refused the attach (version mismatch, unknown
                    // session). Retrying to the deadline would only stall; the error is already a
                    // complete, actionable sentence, so surface it now.
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}`: {err}"),
                    });
                    return;
                }
                if !autostart && should_autostart_session(&err) {
                    // A named session that isn't running should surface as an error, not a silent
                    // empty resurrection. Only ephemeral/initial attaches autostart a server.
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}` is not running"),
                    });
                    return;
                }
                if !spawned && should_autostart_session(&err) {
                    spawned = true;
                    if path.exists() {
                        let _ = std::fs::remove_file(&path);
                    }
                    // Surface the real reason autostart failed instead of letting it
                    // resurface later as a generic connect NotFound after the deadline.
                    let exe = match std::env::current_exe() {
                        Ok(exe) => exe,
                        Err(exe_err) => {
                            link.send(Msg::SessionAttachFailed {
                                epoch,
                                message: format!(
                                    "Could not start server for `{name}`: unable to locate hyprmux executable: {exe_err}"
                                ),
                            });
                            return;
                        }
                    };
                    match std::process::Command::new(&exe)
                        .arg("--session")
                        .arg(&name)
                        .arg("--server")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                    {
                        Ok(child) => server_child = Some(child),
                        Err(spawn_err) => {
                            link.send(Msg::SessionAttachFailed {
                                epoch,
                                message: format!(
                                    "Could not start server for `{name}` ({}): {spawn_err}",
                                    exe.display()
                                ),
                            });
                            return;
                        }
                    }
                }
                // If the server we launched has already exited, it will never bind the
                // socket; report its exit status rather than waiting for the deadline.
                let early_exit = server_child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten());
                if Instant::now() >= deadline || early_exit.is_some() {
                    let detail = match early_exit {
                        Some(status) => {
                            format!("session server exited before it was ready ({status})")
                        }
                        None => err.to_string(),
                    };
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Could not attach to `{name}`: {detail}"),
                    });
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn should_autostart_session(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn is_busy_attach_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// A server that answered the connection but refused our attach handshake - a version mismatch or
/// an unknown session (both surface as [`std::io::ErrorKind::InvalidData`] from the client). Such a
/// server will never accept us, so the attach loop reports it immediately instead of retrying to the
/// connect deadline.
fn is_handshake_rejected(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::InvalidData
}

fn server_message_to_msg(
    epoch: u64,
    frame: session::protocol::Frame<session::protocol::ServerMessage>,
) -> Msg {
    use session::protocol::{Frame, ServerMessage};
    match frame {
        Frame::PaneBytes {
            pane_id,
            generation,
            bytes,
        } => Msg::SessionOutput {
            epoch,
            pane_id,
            generation,
            bytes,
        },
        Frame::Control(message) => match message {
            ServerMessage::Attached {
                session,
                client_id,
                panes,
                layout_rev,
                layout,
                controller,
                clients,
                input_locked,
                ..
            } => Msg::SessionAttached {
                epoch,
                session,
                client_id,
                panes,
                layout_rev,
                layout,
                controller,
                read_only: clients
                    .iter()
                    .find(|client| client.id == client_id)
                    .is_some_and(|client| client.read_only),
                clients,
                input_locked,
            },
            ServerMessage::SessionInfo { .. } => {
                // Query-only probe reply; an attached client never sees this. Route to a harmless
                // no-op error with an empty message that the update loop drops on epoch mismatch.
                Msg::SessionError {
                    epoch,
                    message: String::new(),
                }
            }
            ServerMessage::LayoutCommitted {
                rev,
                author,
                layout,
            } => Msg::SessionLayoutCommitted {
                epoch,
                rev,
                author,
                layout,
            },
            ServerMessage::LayoutRejected {
                current_rev,
                layout,
            } => Msg::SessionLayoutRejected {
                epoch,
                current_rev,
                layout,
            },
            ServerMessage::ControllerChanged { controller, reason } => {
                Msg::SessionControllerChanged {
                    epoch,
                    controller,
                    reason,
                }
            }
            ServerMessage::ClientsChanged {
                clients,
                input_locked,
            } => Msg::SessionClientsChanged {
                epoch,
                clients,
                input_locked,
            },
            ServerMessage::ControlRequested { from } => {
                Msg::SessionControlRequested { epoch, from }
            }
            ServerMessage::ControlDeclined => Msg::SessionControlDeclined { epoch },
            ServerMessage::Ping { seq } => Msg::SessionPing { epoch, seq },
            ServerMessage::Resized {
                pane_id,
                generation,
                cols,
                rows,
            } => Msg::SessionResized {
                epoch,
                pane_id,
                generation,
                cols,
                rows,
            },
            ServerMessage::Exited {
                pane_id,
                generation,
                code,
            } => Msg::SessionExited {
                epoch,
                pane_id,
                generation,
                code,
            },
            ServerMessage::SpawnResult {
                pane_id,
                generation,
                pid,
                ok,
                error,
            } => Msg::SessionSpawnResult {
                epoch,
                pane_id,
                generation,
                pid,
                ok,
                error,
            },
            ServerMessage::Error { message, .. } => Msg::SessionError { epoch, message },
            ServerMessage::Renamed { session } => Msg::SessionRenamed { epoch, session },
        },
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

fn main() -> Result<()> {
    let cli = match parse_cli_args(std::env::args().skip(1).collect()) {
        Ok(ParsedCli::Help) => {
            print_help();
            return Ok(());
        }
        Ok(ParsedCli::Version) => {
            print_version();
            return Ok(());
        }
        Ok(ParsedCli::Control(command)) => return run_control_cli(command),
        Ok(ParsedCli::Server { name }) => return run_server_cli(&name),
        Ok(ParsedCli::ListSessions) => return run_list_sessions_cli(),
        Ok(ParsedCli::KillSession { name }) => return run_kill_session_cli(&name),
        Ok(ParsedCli::Run(args)) => args,
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
            Ok(profile) => Some(profile),
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
            Ok(profile) => startup_profile = Some(profile),
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
                startup_profile = Some(profile);
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
    let startup_system_theme = startup_host_colors.map(theme_ops::system_theme_from_host_colors);
    let resolved_theme = config::resolve_theme(&config.theme.name, startup_system_theme.as_ref());
    startup_messages.extend(resolved_theme.warnings);
    let theme = theme_ops::apply_backdrop_policy(
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CliArgs {
    profile: Option<String>,
    config_path: Option<String>,
    attach_session: Option<String>,
    /// Open the session picker at startup instead of silently attaching to an ephemeral session
    /// (also enabled by `[session] startup = "picker"`). Ignored when `--attach`/`--session` is
    /// given or no named session exists.
    pick: bool,
    read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ControlCli {
    socket: Option<PathBuf>,
    request: control::ControlRequest,
}

#[derive(Debug)]
enum ParsedCli {
    Help,
    Version,
    Run(CliArgs),
    Control(ControlCli),
    Server { name: String },
    ListSessions,
    KillSession { name: String },
}

fn parse_cli_args(args: Vec<String>) -> std::result::Result<ParsedCli, String> {
    let mut cli = CliArgs::default();
    let mut socket: Option<PathBuf> = None;
    let mut socket_flag_seen = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParsedCli::Help),
            "--version" | "-V" => return Ok(ParsedCli::Version),
            "list-sessions" => {
                reject_trailing_control_args(&mut iter, "list-sessions")?;
                return Ok(ParsedCli::ListSessions);
            }
            "kill-session" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "kill-session requires a session name".to_string())?;
                reject_trailing_control_args(&mut iter, "kill-session")?;
                return Ok(ParsedCli::KillSession { name });
            }
            "--server" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--server requires a session name".to_string())?;
                reject_trailing_control_args(&mut iter, "--server")?;
                return Ok(ParsedCli::Server { name });
            }
            "--session" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--session requires a session name".to_string())?;
                match iter.next().as_deref() {
                    Some("--server") => {
                        reject_trailing_control_args(&mut iter, "--server")?;
                        return Ok(ParsedCli::Server { name });
                    }
                    Some("--read-only") => {
                        cli.attach_session = Some(name);
                        cli.read_only = true;
                    }
                    Some(other) => {
                        return Err(format!(
                            "unexpected argument `{other}` after --session <NAME>"
                        ));
                    }
                    None => {
                        cli.attach_session = Some(name);
                    }
                }
            }
            "--attach" => {
                cli.attach_session = Some(
                    iter.next()
                        .ok_or_else(|| "--attach requires a session name".to_string())?,
                );
            }
            "--pick" => {
                cli.pick = true;
            }
            "--read-only" => {
                cli.read_only = true;
            }
            "--profile" | "-p" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--profile requires a profile name".to_string())?;
                if cli.profile.is_some() {
                    return Err("profile name specified more than once".to_string());
                }
                cli.profile = Some(name);
            }
            "--config" => {
                let path = iter
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                if cli.config_path.is_some() {
                    return Err("--config specified more than once".to_string());
                }
                cli.config_path = Some(path);
            }
            "--socket" => {
                socket_flag_seen = true;
                socket = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--socket requires a path".to_string())?,
                ));
            }
            "list" | "list-panes" => {
                reject_trailing_control_args(&mut iter, "list-panes")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::ListPanes),
                }));
            }
            "focus" => {
                let target = iter
                    .next()
                    .ok_or_else(|| "focus requires a pane id".to_string())?
                    .parse()
                    .map_err(|_| "focus requires a numeric pane id".to_string())?;
                reject_trailing_control_args(&mut iter, "focus")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::Focus { target }),
                }));
            }
            "send-text" => {
                let text = iter
                    .next()
                    .ok_or_else(|| "send-text requires literal text".to_string())?;
                reject_trailing_control_args(&mut iter, "send-text")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::SendText {
                        target: None,
                        text,
                    }),
                }));
            }
            "send-keys" => {
                let text = iter.next().ok_or_else(|| {
                    "send-keys requires literal text (named keys are not implemented)".to_string()
                })?;
                reject_trailing_control_args(&mut iter, "send-keys")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::SendText {
                        target: None,
                        text,
                    }),
                }));
            }
            "split" | "new-pane" => {
                let command = iter.next();
                reject_trailing_control_args(&mut iter, "split")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::NewPane {
                        command,
                        cwd: None,
                        title: None,
                        keep_open: false,
                    }),
                }));
            }
            "run-action" => {
                let action = iter
                    .next()
                    .ok_or_else(|| "run-action requires an action id".to_string())?;
                reject_trailing_control_args(&mut iter, "run-action")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::RunAction { action }),
                }));
            }
            "capture-pane" => {
                let mut target = None;
                if let Some(next) = iter.next() {
                    if next == "--target" {
                        let value = iter
                            .next()
                            .ok_or_else(|| "--target requires a pane id".to_string())?;
                        target = Some(
                            value
                                .parse()
                                .map_err(|_| "--target requires a numeric pane id".to_string())?,
                        );
                    } else {
                        return Err(format!("unexpected argument `{next}` after capture-pane"));
                    }
                }
                reject_trailing_control_args(&mut iter, "capture-pane")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::CapturePane { target }),
                }));
            }
            "switch-workspace" => {
                let index = iter
                    .next()
                    .ok_or_else(|| "switch-workspace requires a workspace number".to_string())?
                    .parse()
                    .map_err(|_| {
                        "switch-workspace requires a numeric workspace number".to_string()
                    })?;
                reject_trailing_control_args(&mut iter, "switch-workspace")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::SwitchWorkspace { index }),
                }));
            }
            "move-to-workspace" => {
                let index = iter
                    .next()
                    .ok_or_else(|| "move-to-workspace requires a workspace number".to_string())?
                    .parse()
                    .map_err(|_| {
                        "move-to-workspace requires a numeric workspace number".to_string()
                    })?;
                reject_trailing_control_args(&mut iter, "move-to-workspace")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::MoveToWorkspace { index }),
                }));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`"));
            }
            name => {
                if socket_flag_seen {
                    return Err(format!(
                        "--socket requires a control command before `{name}`"
                    ));
                }
                if cli.profile.is_some() {
                    return Err(format!("unexpected argument `{name}`"));
                }
                cli.profile = Some(name.to_string());
            }
        }
    }
    if socket_flag_seen {
        return Err("--socket requires a control command".to_string());
    }
    if cli.read_only && cli.attach_session.is_none() {
        return Err("--read-only requires --attach or --session".to_string());
    }
    Ok(ParsedCli::Run(cli))
}

fn reject_trailing_control_args(
    iter: &mut std::vec::IntoIter<String>,
    command: &str,
) -> std::result::Result<(), String> {
    if let Some(extra) = iter.next() {
        Err(format!("unexpected argument `{extra}` after {command}"))
    } else {
        Ok(())
    }
}

fn control_request(command: control::ControlCommand) -> control::ControlRequest {
    control::ControlRequest {
        command,
        source_pane: std::env::var("HYPRMUX_PANE")
            .ok()
            .and_then(|v| v.parse().ok()),
    }
}

fn discover_socket(explicit: Option<PathBuf>) -> std::result::Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("HYPRMUX_SOCKET").map(PathBuf::from) {
        return Ok(path);
    }
    let dir =
        control::runtime_dir().map_err(|err| format!("could not inspect runtime dir: {err}"))?;
    let live: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|err| format!("could not read {}: {err}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("control-") && n.ends_with(".sock"))
                && UnixStream::connect(p).is_ok()
        })
        .collect();
    match live.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(
            "no live hyprmux control socket found (set HYPRMUX_SOCKET or pass --socket)"
                .to_string(),
        ),
        _ => Err("multiple live hyprmux sockets found; pass --socket PATH".to_string()),
    }
}

fn run_control_cli(command: ControlCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&command.request).unwrap()
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        eprintln!("empty response from hyprmux");
        std::process::exit(2);
    }
    println!("{}", line.trim_end());
    let value: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("invalid JSON response: {err}");
            std::process::exit(2);
        }
    };
    if value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
    Ok(())
}

fn run_server_cli(name: &str) -> Result<()> {
    session::server::run_named_session(name)?;
    Ok(())
}

fn run_list_sessions_cli() -> Result<()> {
    for session in session::discovery::discover_sessions()? {
        match session.status {
            session::discovery::DiscoveredSessionStatus::Running {
                panes,
                clients,
                has_layout,
            } => println!(
                "{}\trunning\tpanes={}\tclients={}\tlayout={}",
                session.name,
                panes,
                clients,
                if has_layout { "yes" } else { "no" }
            ),
            session::discovery::DiscoveredSessionStatus::Busy => {
                println!("{}\tbusy\tpanes=?\tclients=?\tlayout=?", session.name)
            }
            session::discovery::DiscoveredSessionStatus::Unknown => {
                println!("{}\tunknown\tpanes=?\tclients=?\tlayout=?", session.name)
            }
        }
    }
    Ok(())
}

fn run_kill_session_cli(name: &str) -> Result<()> {
    use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

    let path = session::server::session_socket_path(name)?;
    if !path.exists() {
        eprintln!("session {name:?} is not running");
        std::process::exit(1);
    }
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
            session::protocol::write_frame(
                &mut stream,
                &ClientMessage::Attach {
                    session: name.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    label: std::env::var("USER").unwrap_or_else(|_| "client".to_string()),
                    read_only: false,
                },
            )?;
            match session::protocol::read_frame::<_, ServerMessage>(&mut stream)? {
                ServerMessage::Attached { .. } => {
                    session::protocol::write_frame(&mut stream, &ClientMessage::Shutdown)?;
                    use std::io::Write;
                    stream.flush()?;
                }
                other => {
                    eprintln!("could not attach to session {name:?}: {other:?}");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("could not attach to session {name:?}: {err}");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!(
        "\
hyprmux - Hyprland-style tiling terminal multiplexer

USAGE:
    hyprmux [PROFILE]
    hyprmux --profile <NAME>
    hyprmux -p <NAME>
    hyprmux [--socket PATH] list|list-panes
    hyprmux [--socket PATH] focus <PANE_ID>
    hyprmux [--socket PATH] send-text <TEXT>
    hyprmux [--socket PATH] split [COMMAND]
    hyprmux [--socket PATH] run-action <ACTION_ID>
    hyprmux [--socket PATH] capture-pane [--target <PANE_ID>]
    hyprmux [--socket PATH] switch-workspace <1-9>
    hyprmux [--socket PATH] move-to-workspace <1-9>
    hyprmux --attach <NAME> [--read-only]
    hyprmux --session <NAME> [--read-only]
    hyprmux --pick
    hyprmux list-sessions
    hyprmux kill-session <NAME>
    hyprmux --server <NAME>
    hyprmux --session <NAME> --server

OPTIONS:
    -h, --help            Print help
    -V, --version         Print version
    -p, --profile <NAME>  Load a named profile from ~/.config/hyprmux/profiles/<NAME>.toml
        --config <PATH>   Use an alternate hyprmux.toml (sets HYPRMUX_CONFIG)
        --socket <PATH>   Connect CLI control command to this socket
        --pick            Open the session picker at startup when a named session exists
        --read-only       Attach as a viewer that cannot type or control the layout

A bare PROFILE positional is equivalent to --profile PROFILE.
Leave the running app with prefix d (detach) or a configured quit binding."
    );
}

fn print_version() {
    println!("hyprmux {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::{TestBackend, UiSnapshotOptions, UiWidgetKind};

    #[test]
    fn cli_parses_profile_flag_and_positional() {
        let flag =
            expect_run(parse_cli_args(vec!["--profile".into(), "dev".into()]).expect("parses"));
        assert_eq!(flag.profile.as_deref(), Some("dev"));

        let positional = expect_run(parse_cli_args(vec!["dev".into()]).expect("parses"));
        assert_eq!(positional.profile.as_deref(), Some("dev"));
    }

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
    fn cli_help_and_version_are_early_exit_variants() {
        assert!(matches!(
            parse_cli_args(vec!["--help".into()]).expect("parses"),
            ParsedCli::Help
        ));
        assert!(matches!(
            parse_cli_args(vec!["-V".into()]).expect("parses"),
            ParsedCli::Version
        ));
    }

    #[test]
    fn cli_reserved_control_commands_do_not_parse_as_profiles() {
        let parsed = parse_cli_args(vec!["list-panes".into()]).expect("parses");
        assert!(matches!(parsed, ParsedCli::Control(_)));

        let profile = expect_run(
            parse_cli_args(vec!["--profile".into(), "list-panes".into()]).expect("parses"),
        );
        assert_eq!(profile.profile.as_deref(), Some("list-panes"));
    }

    #[test]
    fn cli_control_socket_flag_is_preserved() {
        let parsed = parse_cli_args(vec![
            "--socket".into(),
            "/tmp/hyprmux.sock".into(),
            "send-text".into(),
            "hi".into(),
        ])
        .expect("parses");
        let ParsedCli::Control(control) = parsed else {
            panic!("expected control");
        };
        assert_eq!(
            control.socket.as_deref(),
            Some(std::path::Path::new("/tmp/hyprmux.sock"))
        );
        assert!(matches!(
            control.request.command,
            control::ControlCommand::SendText { .. }
        ));
    }

    #[test]
    fn cli_parses_run_action_capture_pane_and_workspace_commands() {
        let ParsedCli::Control(run_action) =
            parse_cli_args(vec!["run-action".into(), "toggle-float".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            run_action.request.command,
            control::ControlCommand::RunAction {
                action: "toggle-float".to_string()
            }
        );

        let ParsedCli::Control(capture) =
            parse_cli_args(vec!["capture-pane".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture.request.command,
            control::ControlCommand::CapturePane { target: None }
        );

        let ParsedCli::Control(capture_target) =
            parse_cli_args(vec!["capture-pane".into(), "--target".into(), "7".into()])
                .expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture_target.request.command,
            control::ControlCommand::CapturePane { target: Some(7) }
        );

        let ParsedCli::Control(switch) =
            parse_cli_args(vec!["switch-workspace".into(), "3".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            switch.request.command,
            control::ControlCommand::SwitchWorkspace { index: 3 }
        );

        let ParsedCli::Control(move_to) =
            parse_cli_args(vec!["move-to-workspace".into(), "4".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            move_to.request.command,
            control::ControlCommand::MoveToWorkspace { index: 4 }
        );

        assert!(parse_cli_args(vec!["run-action".into()]).is_err());
        assert!(parse_cli_args(vec!["switch-workspace".into(), "nope".into()]).is_err());
        assert!(parse_cli_args(vec!["capture-pane".into(), "--bogus".into()]).is_err());
    }

    #[test]
    fn cli_control_commands_reject_trailing_args() {
        assert!(parse_cli_args(vec!["focus".into(), "1".into(), "garbage".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "list-panes".into(),
                "--socket".into(),
                "/tmp/x".into()
            ])
            .is_err()
        );
        assert!(parse_cli_args(vec!["send-text".into(), "hi".into(), "extra".into()]).is_err());
    }

    #[test]
    fn cli_parses_session_verbs_and_attach() {
        assert!(matches!(
            parse_cli_args(vec!["list-sessions".into()]).expect("parses"),
            ParsedCli::ListSessions
        ));
        assert!(matches!(
            parse_cli_args(vec!["kill-session".into(), "dev".into()]).expect("parses"),
            ParsedCli::KillSession { name } if name == "dev"
        ));
        let attached =
            expect_run(parse_cli_args(vec!["--attach".into(), "dev".into()]).expect("parses"));
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        let session =
            expect_run(parse_cli_args(vec!["--session".into(), "dev".into()]).expect("parses"));
        assert_eq!(session.attach_session.as_deref(), Some("dev"));
        assert!(parse_cli_args(vec!["kill-session".into()]).is_err());
    }

    #[test]
    fn cli_read_only_requires_attach_target() {
        assert!(parse_cli_args(vec!["--read-only".into()]).is_err());
        let args = expect_run(
            parse_cli_args(vec!["--attach".into(), "dev".into(), "--read-only".into()])
                .expect("parses"),
        );
        assert_eq!(args.attach_session.as_deref(), Some("dev"));
        assert!(args.read_only);
    }

    #[test]
    fn cli_socket_without_control_command_errors() {
        assert!(parse_cli_args(vec!["--socket".into(), "/tmp/x".into()]).is_err());
        assert!(parse_cli_args(vec!["--socket".into(), "/tmp/x".into(), "dev".into()]).is_err());
    }

    #[test]
    fn cli_rejects_unknown_flags() {
        assert!(parse_cli_args(vec!["--nope".into()]).is_err());
    }

    #[test]
    fn cli_parses_pick_flag() {
        let default = expect_run(parse_cli_args(vec![]).expect("parses"));
        assert!(!default.pick);
        let picked = expect_run(parse_cli_args(vec!["--pick".into()]).expect("parses"));
        assert!(picked.pick);
        // `--pick` composes with a profile positional.
        let with_profile =
            expect_run(parse_cli_args(vec!["--pick".into(), "dev".into()]).expect("parses"));
        assert!(with_profile.pick);
        assert_eq!(with_profile.profile.as_deref(), Some("dev"));
    }

    fn expect_run(parsed: ParsedCli) -> CliArgs {
        match parsed {
            ParsedCli::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
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
