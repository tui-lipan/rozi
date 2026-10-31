use std::path::PathBuf;
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::Msg;
use crate::config::Config;
use crate::session::bootstrap::SessionStart;
use crate::{cli, config, events, ops, platform, profiles};

#[derive(Clone)]
pub(super) struct StartupProfile {
    pub(super) profile: profiles::Profile,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) records_origin: bool,
}

/// Where a created session's first pane starts, on the session's host.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StartupCwd {
    pub(super) path: String,
    /// Set when `path` is a checkout to record as the session's worktree origin (`worktrees open`).
    pub(super) worktree: Option<StartupWorktree>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct StartupWorktree {
    /// The repository's checkouts on the session host.
    pub(super) checkouts: Vec<String>,
    /// `[worktrees] profile`, rebased onto the checkout when the session is seeded.
    pub(super) profile: Option<profiles::LoadedProfile>,
}

impl StartupCwd {
    /// The attachment and intent this launch seeds its session with.
    pub(super) fn seed(
        &self,
        config: &Config,
    ) -> Option<(crate::state::Attachment, crate::state::AttachIntent)> {
        let worktree = self.worktree.as_ref()?;
        Some(profiles::worktree_session_seed(
            config,
            &self.path,
            &worktree.checkouts,
            worktree.profile.clone(),
        ))
    }
}

pub(super) struct StartupTasks {
    pub(super) enabled: bool,
    /// How long after the startup check the first re-check fires, or `None` when this client does
    /// no release checks at all.
    pub(super) update_check_interval: Option<Duration>,
    /// Whether those checks also report this install to the weekly usage count.
    pub(super) count_usage: bool,
    pub(super) watch_hangup: bool,
    pub(super) control_listener: Option<crate::platform::ipc::IpcListener>,
    pub(super) event_hub: events::EventHub,
    pub(super) start: SessionStart,
    pub(super) remote_config: config::RemoteConfig,
    pub(super) theme_tick: bool,
    pub(super) workbar_tick: bool,
}

impl StartupTasks {
    pub(super) fn run(mut self, link: CommandLink<Msg>) {
        link.send(Msg::CommandLinkReady(link.clone()));
        if !self.enabled {
            return;
        }
        // Keep the global Agents view informed about other local sessions without attaching to
        // them. Discovery probes their semantic summaries; it never requests terminal frames.
        let local_link = link.clone();
        std::thread::spawn(move || {
            let mut previous = None;
            loop {
                let snapshot = crate::session::discovery::discover_host_metadata()
                    .ok()
                    .map(|metadata| crate::session::discovery::LocalAgentSnapshot {
                        sessions: metadata
                            .sessions
                            .into_iter()
                            .filter(|row| !row.ephemeral)
                            .collect(),
                        agents: metadata.agents,
                        projects: metadata.projects,
                    });
                if previous.as_ref() != Some(&snapshot) {
                    local_link.send(Msg::LocalAgentMetadata(snapshot.clone()));
                    previous = Some(snapshot);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        if let Some(interval) = self.update_check_interval {
            let update_link = link.clone();
            let count_usage = self.count_usage;
            std::thread::spawn(move || check_for_update(update_link, interval, count_usage));
        }
        ops::config::spawn_config_watcher(&link);
        ops::config::spawn_extension_store_watcher(&link);
        if self.watch_hangup {
            let hangup_link = link.clone();
            if let Err(err) = platform::server_lifecycle::on_hangup(move || {
                hangup_link.send(Msg::Hangup);
            }) {
                link.send(Msg::BackgroundError {
                    title: "Terminal hangup watch unavailable",
                    message: err.to_string(),
                });
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
        // Bind the askpass broker before the first attach builds its ssh. Otherwise that ssh could
        // spawn before `CommandLinkReady` is handled and, finding no broker, refuse a prompt the
        // user could have answered.
        crate::session::remote::askpass::start(link.clone());
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
            SessionStart::Attach { epoch, request } => {
                let remote_config = self.remote_config;
                std::thread::spawn(move || {
                    crate::session::bootstrap::run_attach(epoch, request, &remote_config, link)
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
                link.send(Msg::RemoteStartupActivate(target));
            }
        }
    }
}

/// What the client does first: attach, or open the picker and let the user choose.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum StartupSession {
    Attach {
        request: crate::state::AttachRequest,
        /// No session was named, so this is the client's own temporary session.
        auto_created: bool,
    },
    RemoteLast {
        target: crate::session::remote::RemoteTarget,
        name: String,
    },
    Picker {
        /// The host whose picker opens, `None` for this machine.
        remote: Option<crate::session::remote::RemoteTarget>,
        /// The row to land on, from a `last` session that could not reopen.
        last_session: Option<String>,
    },
}

/// A launch resolved from its command line and configuration, before the UI starts.
pub(super) struct StartupPlan {
    pub(super) config: Config,
    pub(super) messages: Vec<String>,
    pub(super) profile: Option<StartupProfile>,
    pub(super) cwd: Option<StartupCwd>,
    pub(super) session: StartupSession,
}

impl StartupPlan {
    pub(super) fn resolve(cli: &cli::CliArgs, loaded: config::LoadedConfig) -> Self {
        let mut target = TargetResolution::resolve(cli, loaded);
        let session = target.session(cli);
        Self {
            config: target.config,
            messages: target.messages,
            profile: target.profile,
            cwd: target.cwd,
            session,
        }
    }

    /// The host the launch reaches through SSH, `None` for this machine.
    pub(super) fn remote(&self) -> Option<&crate::session::remote::RemoteTarget> {
        match &self.session {
            StartupSession::Attach { request, .. } => request.remote.as_ref(),
            StartupSession::RemoteLast { target, .. } => Some(target),
            StartupSession::Picker { remote, .. } => remote.as_ref(),
        }
    }
}

/// The working state of [`StartupPlan::resolve`]: which session the launch names, and how it is
/// reached.
struct TargetResolution {
    config: Config,
    messages: Vec<String>,
    attach_session: Option<String>,
    autostart: bool,
    create_only: bool,
    profile: Option<StartupProfile>,
    cwd: Option<StartupCwd>,
    remote: Option<crate::session::remote::RemoteTarget>,
    last_session: Option<String>,
    global_remote_last: bool,
}

impl TargetResolution {
    fn resolve(cli: &cli::CliArgs, loaded: config::LoadedConfig) -> Self {
        let explicit_target = cli.attach_session.is_some();
        let remote = resolve_startup_remote(cli.remote.as_deref(), &loaded.config);
        let mut plan = Self {
            messages: loaded.warnings,
            attach_session: cli.attach_session.clone(),
            autostart: cli.attach_session.is_none(),
            create_only: false,
            profile: None,
            cwd: None,
            remote,
            last_session: None,
            global_remote_last: false,
            config: loaded.config,
        };
        plan.cwd =
            resolve_startup_cwd(cli, plan.remote.is_some(), &plan.config, &mut plan.messages);
        plan.apply_session_policy(cli);
        plan.resolve_session_target(cli, explicit_target);
        plan.load_fallback_profile();
        plan
    }

    fn session(&mut self, cli: &cli::CliArgs) -> StartupSession {
        if self.global_remote_last {
            return StartupSession::RemoteLast {
                target: self.remote.clone().expect("remote last has a host"),
                name: self.last_session.take().expect("remote last has a session"),
            };
        }
        // Picker at startup only for a bare launch. `last` and `profile` reach here when the
        // named session could not be opened. Under `--remote` this opens that host's picker.
        let want_picker = self.attach_session.is_none()
            && (cli.pick
                || matches!(
                    self.config.session.startup,
                    config::SessionStartup::Picker
                        | config::SessionStartup::Last
                        | config::SessionStartup::Profile
                ));
        if want_picker {
            return StartupSession::Picker {
                remote: self.remote.clone(),
                last_session: self.last_session.take(),
            };
        }
        let access = cli.session_command.access();
        let mode = if self.create_only {
            crate::state::AttachMode::Create
        } else if self.autostart && !access.is_read_only() {
            crate::state::AttachMode::JoinOrStart
        } else {
            crate::state::AttachMode::Join
        };
        let auto_created = self.attach_session.is_none();
        let name = self.attach_session.clone().unwrap_or_else(|| {
            if self.remote.is_some() {
                crate::state::remote_ephemeral_session_name()
            } else {
                crate::state::ephemeral_session_name()
            }
        });
        let intent = match (&self.profile, &self.cwd) {
            (Some(profile), _) if profile.records_origin => {
                crate::state::AttachIntent::ProfileSeed {
                    profile: profile.name.clone(),
                    path: profile.path.clone(),
                }
            }
            (_, Some(cwd)) => cwd
                .seed(&self.config)
                .map_or(crate::state::AttachIntent::Plain, |(_, intent)| intent),
            _ => crate::state::AttachIntent::Plain,
        };
        StartupSession::Attach {
            request: crate::state::AttachRequest::new(name, mode)
                .on_host(self.remote.clone())
                .with_access(access)
                .with_intent(intent),
            auto_created,
        }
    }

    fn apply_session_policy(&mut self, cli: &cli::CliArgs) {
        // Startup policy chooses a session only when the user named none. `--pick` asks for the
        // picker by hand, and an explicit session target overrides the policy outright.
        if self.attach_session.is_some() || cli.pick {
            return;
        }
        match self.config.session.startup {
            config::SessionStartup::Last => self.apply_last_session_policy(),
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

    fn apply_last_session_policy(&mut self) {
        if self.remote.is_some() {
            self.last_session = crate::session::read_last_session(self.remote.as_ref());
            return;
        }
        let Some((remote, name)) = crate::session::read_global_last_session() else {
            return;
        };
        if let Some(target) = remote {
            if self.config.remote.reconnect_on_launch
                && crate::session::read_reconnect_hosts().contains(&target)
            {
                self.remote = Some(target);
                self.last_session = Some(name);
                self.global_remote_last = true;
            }
            return;
        }
        match select_last_session_target(name.clone(), session_openable_by_name(&name)) {
            LastSessionTarget::Reopen(name) => {
                self.attach_session = Some(name);
                self.autostart = true;
            }
            LastSessionTarget::Pick(name) => self.last_session = name,
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
        match &cli.session_command {
            cli::SessionCommand::New(new) => {
                self.autostart = true;
                self.create_only = true;
                self.profile = requested_new_profile(new);
            }
            cli::SessionCommand::Attach(_) => self.autostart = false,
            cli::SessionCommand::Dwim { .. } => {
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
        match &cli.session_command {
            cli::SessionCommand::Attach(_) => {
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
            cli::SessionCommand::New(new) => {
                if running {
                    startup_fatal(format!(
                        "Session `{name}` is already running.\nAttach with: rozi sessions attach {name}"
                    ));
                }
                self.autostart = true;
                self.create_only = true;
                self.profile = requested_new_profile(new);
            }
            cli::SessionCommand::Dwim { read_only } if explicit_target => {
                if running {
                    self.autostart = false;
                } else if *read_only {
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
            cli::SessionCommand::Dwim { .. } => {
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

/// `--cwd` for a created session. A local directory is resolved here, where it was typed; a remote
/// one is the far host's path and is passed through untouched.
fn resolve_startup_cwd(
    cli: &cli::CliArgs,
    remote: bool,
    config: &Config,
    messages: &mut Vec<String>,
) -> Option<StartupCwd> {
    let cli::SessionCommand::New(new) = &cli.session_command else {
        return None;
    };
    let raw = new.cwd.as_deref()?;
    let path = if remote || new.worktree_checkouts.is_some() {
        raw.to_string()
    } else {
        let path = crate::session::worktrees::host_path(raw).unwrap_or_else(|err| {
            startup_fatal(format!("Invalid --cwd `{raw}`: {err}"));
        });
        if !path.is_dir() {
            startup_fatal(format!("--cwd `{}` is not a directory.", path.display()));
        }
        path.to_string_lossy().into_owned()
    };
    let worktree = new.worktree_checkouts.clone().map(|checkouts| {
        // A broken or unusable worktree profile still opens the checkout, with a plain shell.
        let profile =
            profiles::load_worktree_profile(config, &checkouts, remote).unwrap_or_else(|message| {
                messages.push(message);
                None
            });
        StartupWorktree { checkouts, profile }
    });
    Some(StartupCwd { path, worktree })
}

fn requested_new_profile(new: &cli::NewSession) -> Option<StartupProfile> {
    let profile_name = new.profile.as_ref()?;
    if !crate::session::discovery::valid_session_name(profile_name) {
        startup_fatal(format!("Invalid profile name `{profile_name}`."));
    }
    let path = config::profile_path_for_name(profile_name);
    if !path.exists() {
        startup_fatal(format!("Profile `{profile_name}` does not exist."));
    }
    Some(load_startup_profile(profile_name, path))
}

/// Look for a newer release and, when `count_usage`, report this week's usage count, then arm the
/// next look.
///
/// The next tick is armed after the check returns rather than alongside it, so a release host that
/// never answers costs one waiting thread instead of a growing pile of them. Blocking is fine
/// here: every caller is already on a worker thread of its own.
pub(super) fn check_for_update(link: CommandLink<Msg>, interval: Duration, count_usage: bool) {
    ops::update_check::announce_available(&link);
    ops::usage_count::report_if_due(&link, count_usage);
    link.send_after(interval, Msg::UpdateCheckTick);
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
/// Sessions on the workbox tab, where the live list is one probe away.
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

    fn plan_for(args: &[&str]) -> StartupPlan {
        let cli::ParsedCli::Run(cli) =
            cli::parse_cli_args(args.iter().map(|arg| arg.to_string()).collect())
                .expect("launch parses")
        else {
            panic!("{args:?} should launch");
        };
        let mut config = Config::default();
        config.session.startup = config::SessionStartup::Ephemeral;
        config.session.autosave = false;
        StartupPlan::resolve(
            &cli,
            config::LoadedConfig {
                config,
                warnings: Vec::new(),
                rejected: false,
            },
        )
    }

    fn last_plan(args: &[&str], enabled: bool) -> StartupPlan {
        let cli::ParsedCli::Run(cli) =
            cli::parse_cli_args(args.iter().map(|arg| arg.to_string()).collect()).unwrap()
        else {
            panic!("expected launch")
        };
        let mut config = Config::default();
        config.session.startup = config::SessionStartup::Last;
        config.session.autosave = false;
        config.remote.reconnect_on_launch = enabled;
        StartupPlan::resolve(
            &cli,
            config::LoadedConfig {
                config,
                warnings: Vec::new(),
                rejected: false,
            },
        )
    }

    #[test]
    fn bare_last_uses_the_global_workplace_only_with_durable_enabled_remote_intent() {
        crate::test_support::isolate_user_dirs();
        let _persist = crate::test_support::lock_persisted_state();
        let _restore = crate::test_support::preserve_persisted_state();
        let target = crate::session::remote::RemoteTarget::Alias("global-lastbox".into());
        crate::session::record_last_session(None, "local-global-last");
        crate::session::record_last_session(Some(&target), "remote-global-last");
        crate::session::forget_reconnect_host(&target).unwrap();
        for (enabled, durable) in [(false, false), (true, false), (false, true), (true, true)] {
            if durable {
                crate::session::add_reconnect_host(&target).unwrap();
            }
            let plan = last_plan(&[], enabled);
            if enabled && durable {
                assert_eq!(
                    plan.session,
                    StartupSession::RemoteLast {
                        target: target.clone(),
                        name: "remote-global-last".into()
                    }
                );
            } else {
                assert!(
                    matches!(plan.session, StartupSession::Picker { remote: None, .. }),
                    "{:?}",
                    plan.session
                );
            }
            let explicit = last_plan(&["--remote", "global-lastbox"], enabled);
            assert_eq!(
                explicit.session,
                StartupSession::Picker {
                    remote: Some(target.clone()),
                    last_session: Some("remote-global-last".into())
                }
            );
        }
        crate::session::record_last_session(None, "local-global-last");
        assert!(
            matches!(last_plan(&[], true).session, StartupSession::Picker { remote: None, last_session: Some(name) } if name == "local-global-last")
        );
    }

    fn startup_request(plan: &StartupPlan) -> &crate::state::AttachRequest {
        match &plan.session {
            StartupSession::Attach { request, .. } => request,
            other => panic!("expected an attach, got {other:?}"),
        }
    }

    /// The attach options on the command line become the request the first attach carries, so
    /// nothing about them has to be remembered anywhere else.
    #[test]
    fn launch_options_resolve_into_the_startup_attach_request() {
        use crate::config::SessionOnAttach::Take;
        use crate::state::{AttachAccess, AttachMode};

        crate::test_support::isolate_user_dirs();
        let workbox = crate::session::remote::RemoteTarget::Alias("box".into());
        for (args, mode, access) in [
            (
                &[
                    "--remote",
                    "box",
                    "sessions",
                    "attach",
                    "dev",
                    "--take-control",
                ][..],
                AttachMode::Join,
                AttachAccess::Writable {
                    on_attach: Some(Take),
                },
            ),
            (
                &[
                    "sessions",
                    "attach",
                    "dev",
                    "--read-only",
                    "--remote",
                    "box",
                ][..],
                AttachMode::Join,
                AttachAccess::ReadOnly,
            ),
            (
                &["--remote", "box", "--read-only", "dev"][..],
                AttachMode::Join,
                AttachAccess::ReadOnly,
            ),
            (
                &["--remote", "box", "dev"][..],
                AttachMode::JoinOrStart,
                AttachAccess::WRITABLE,
            ),
            (
                &["--remote", "box", "sessions", "new", "dev"][..],
                AttachMode::Create,
                AttachAccess::WRITABLE,
            ),
        ] {
            let plan = plan_for(args);
            let request = startup_request(&plan);
            assert_eq!(request.name, "dev", "{args:?}");
            assert_eq!(request.remote.as_ref(), Some(&workbox), "{args:?}");
            assert_eq!(request.mode, mode, "{args:?}");
            assert_eq!(request.access, access, "{args:?}");
            assert!(
                matches!(
                    plan.session,
                    StartupSession::Attach {
                        auto_created: false,
                        ..
                    }
                ),
                "{args:?}"
            );
        }
    }

    #[test]
    fn a_bare_launch_starts_its_own_temporary_session_or_the_picker() {
        crate::test_support::isolate_user_dirs();
        let plan = plan_for(&[]);
        let request = startup_request(&plan);
        assert_eq!(request.name, crate::state::ephemeral_session_name());
        assert_eq!(request.mode, crate::state::AttachMode::JoinOrStart);
        assert!(matches!(
            plan.session,
            StartupSession::Attach {
                auto_created: true,
                ..
            }
        ));

        assert_eq!(
            plan_for(&["--pick", "--remote", "box"]).session,
            StartupSession::Picker {
                remote: Some(crate::session::remote::RemoteTarget::Alias("box".into())),
                last_session: None,
            }
        );
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
}
