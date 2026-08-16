use crate::config::{ServiceConfig, expand_path};
use crate::platform::command::{CommandGroup, ShellEnv, configure_command_group};
use crate::state::{DormantReason, DormantService, PendingRestart, RunningService, State};
use crate::{AppRoot, Msg};
use std::time::{Duration, Instant};
use tui_lipan::prelude::*;

pub(crate) const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(30);
pub(crate) const UPTIME_RESET_SECS: u64 = 60;
pub(crate) const MAX_FAILURES: u32 = 5;

/// The delay after `current`. Every restart path steps the ladder through here so a change to the
/// curve cannot apply to crashes but miss clean exits, or apply to exits but miss failed spawns.
pub(crate) fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// Whether a dormant service stays dormant across a config reload.
///
/// Exact definition equality is the lifecycle boundary: an unrelated or repeated reload must not
/// revive a crash loop. Editing any material service field drops the old dormant entry and starts
/// one fresh generation with a reset backoff ladder.
pub(crate) fn stays_dormant(dormant: &DormantService, services: &[ServiceConfig]) -> bool {
    services.iter().any(|service| service == &dormant.config)
}

/// Route a service that could not be spawned onto the same ladder a crashed one takes.
///
/// A spawn can fail for reasons that pass: a `cwd` not mounted yet, momentary fork pressure. Going
/// straight to dormant on the first one made startup stricter than mid-session, where the identical
/// failure is retried. `failures` counts this attempt. Returns the give-up message when the ladder
/// is exhausted, so the caller raises it with its own `Context`.
pub(crate) fn record_spawn_failure(
    state: &mut State,
    name: String,
    config: ServiceConfig,
    failures: u32,
    backoff_delay: Duration,
) -> Option<String> {
    if config.restart == crate::config::ServiceRestart::Never {
        state.services.dormant.insert(
            name,
            DormantService {
                config,
                reason: DormantReason::NeverRestart,
            },
        );
        return None;
    }
    if failures >= MAX_FAILURES {
        let message = format!("Service '{name}' failed to start {MAX_FAILURES} times; stopping");
        state.services.dormant.insert(
            name,
            DormantService {
                config,
                reason: DormantReason::ExhaustedBackoff,
            },
        );
        return Some(message);
    }
    state.services.pending.insert(
        name,
        PendingRestart {
            config,
            restart_at: Instant::now() + backoff_delay,
            backoff_delay: next_backoff(backoff_delay),
            consecutive_failures: failures,
        },
    );
    None
}

pub(crate) fn spawn_service_child(
    config: &ServiceConfig,
    command_shell: Option<&[String]>,
    control_socket: Option<&std::path::Path>,
) -> std::io::Result<(std::process::Child, CommandGroup)> {
    let runner =
        crate::platform::command::resolve_command_shell(command_shell, &ShellEnv::from_process());
    let mut command = std::process::Command::new(&runner.program);
    command.args(&runner.args);
    command.arg(&config.run);
    if let Some(cwd) = &config.cwd {
        let expanded = expand_path(cwd);
        command.current_dir(expanded);
    }
    if let Some(socket) = control_socket {
        command.env("ROZI_SOCKET", socket);
    }
    // A service exists to talk back to rozi, so it is the caller least able to assume a `PATH`
    // install. Config `env` is applied after, so a service can override either.
    if let Some(binary) = crate::platform::paths::current_binary() {
        command.env("ROZI_BIN", binary);
    }
    command.env("ROZI", "1");
    command.env("ROZI_SERVICE", &config.name);
    command.envs(&config.env);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    configure_command_group(&mut command);

    let mut child = command.spawn()?;
    let group = match CommandGroup::new(&child) {
        Ok(g) => g,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    Ok((child, group))
}

pub(crate) fn terminate_service(mut running: RunningService) {
    running.group.terminate(&mut running.child);
}

pub(crate) fn terminate_all(state: &mut State) {
    for (_, running) in std::mem::take(&mut state.services.running) {
        terminate_service(running);
    }
    state.services.pending.clear();
    state.services.dormant.clear();
}

/// Kick off services once command link is ready.
pub(crate) fn start_services(ctx: &mut Context<AppRoot>) -> Update {
    let services = ctx.state.config.services.clone();
    if services.is_empty() {
        return Update::none();
    }
    let command_shell = ctx.state.config.command_shell.clone();
    let control_socket = ctx.state.control_socket_path.clone();

    for config in services {
        let name = config.name.clone();
        match spawn_service_child(&config, command_shell.as_deref(), control_socket.as_deref()) {
            Ok((child, group)) => {
                ctx.state.services.running.insert(
                    name,
                    RunningService {
                        config,
                        child,
                        group,
                        started_at: Instant::now(),
                        backoff_delay: INITIAL_BACKOFF,
                        consecutive_failures: 0,
                    },
                );
            }
            Err(_) => {
                if let Some(message) =
                    record_spawn_failure(&mut ctx.state, name, config, 1, INITIAL_BACKOFF)
                {
                    crate::pty_events::notify_error(ctx, "Service failed", message);
                }
            }
        }
    }

    if !ctx.state.services.running.is_empty() || !ctx.state.services.pending.is_empty() {
        let epoch = ctx.state.services.bump_epoch();
        Update::command_only(Command::after(
            Duration::from_secs(1),
            move |link: CommandLink<Msg>| {
                link.send(Msg::ServicesTick { epoch });
            },
        ))
    } else {
        Update::none()
    }
}

/// Reconcile running/pending/dormant services on config reload.
///
/// Arms no tick of its own: the caller already decides whether one is due from the reconciled
/// state, and returning a second `Command` here would double-arm the loop.
pub(crate) fn reconcile_services(ctx: &mut Context<AppRoot>) {
    let new_services = ctx.state.config.services.clone();
    let command_shell = ctx.state.config.command_shell.clone();
    let control_socket = ctx.state.control_socket_path.clone();
    let failures = reconcile_service_set(
        &mut ctx.state,
        new_services,
        command_shell.as_deref(),
        control_socket.as_deref(),
    );
    for message in failures {
        crate::pty_events::notify_error(ctx, "Service failed", message);
    }
}

fn reconcile_service_set(
    state: &mut State,
    new_services: Vec<ServiceConfig>,
    command_shell: Option<&[String]>,
    control_socket: Option<&std::path::Path>,
) -> Vec<String> {
    let mut failures = Vec::new();
    // Invalidate old scheduled ticks. The caller arms the replacement off the new epoch.
    state.services.bump_epoch();

    // 1. Remove/terminate running services that no longer match
    let mut names_to_stop = Vec::new();
    for (name, running) in &state.services.running {
        if !new_services.iter().any(|s| s == &running.config) {
            names_to_stop.push(name.clone());
        }
    }
    for name in names_to_stop {
        if let Some(running) = state.services.running.remove(&name) {
            terminate_service(running);
        }
    }

    // 2. Remove pending services that no longer match
    state
        .services
        .pending
        .retain(|_, p| new_services.iter().any(|s| s == &p.config));

    // 3. Keep exact dormant definitions; a material edit drops the old generation so step 4 can
    //    start its replacement with fresh backoff state.
    state
        .services
        .dormant
        .retain(|_, d| stays_dormant(d, &new_services));

    // 4. Start any new services that are not already running, pending, or dormant
    for config in new_services {
        let name = config.name.clone();
        if state.services.running.contains_key(&name)
            || state.services.pending.contains_key(&name)
            || state.services.dormant.contains_key(&name)
        {
            continue;
        }

        match spawn_service_child(&config, command_shell, control_socket) {
            Ok((child, group)) => {
                state.services.running.insert(
                    name,
                    RunningService {
                        config,
                        child,
                        group,
                        started_at: Instant::now(),
                        backoff_delay: INITIAL_BACKOFF,
                        consecutive_failures: 0,
                    },
                );
            }
            Err(_) => {
                if let Some(message) = record_spawn_failure(state, name, config, 1, INITIAL_BACKOFF)
                {
                    failures.push(message);
                }
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceRestart;
    use std::collections::BTreeMap;

    #[test]
    fn spawn_and_terminate_service() {
        let config = ServiceConfig {
            name: "test-sleeper".to_string(),
            #[cfg(unix)]
            run: "sleep 10".to_string(),
            #[cfg(windows)]
            run: "ping 127.0.0.1 -n 10".to_string(),
            cwd: None,
            restart: ServiceRestart::Never,
            env: BTreeMap::new(),
        };

        let (mut child, group) = spawn_service_child(&config, None, None).expect("spawn child");
        assert!(child.try_wait().unwrap().is_none());

        group.terminate(&mut child);
        assert!(child.try_wait().unwrap().is_some());
    }

    /// A service is the caller least able to assume a `PATH` install, so it is handed the binary
    /// path outright.
    #[cfg(unix)]
    #[test]
    fn a_service_learns_the_binary_path_and_its_own_name() {
        let out = std::env::temp_dir().join(format!("rozi-svc-env-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let config = ServiceConfig {
            name: "env-probe".to_string(),
            run: format!(
                "printf '%s\\n%s\\n%s' \"$ROZI_BIN\" \"$ROZI_SERVICE\" \"$ROZI\" > '{}'",
                out.display()
            ),
            cwd: None,
            restart: ServiceRestart::Never,
            env: BTreeMap::new(),
        };

        let (mut child, _group) = spawn_service_child(&config, None, None).expect("spawn");
        child.wait().expect("probe exits");

        let written = std::fs::read_to_string(&out).expect("probe wrote its environment");
        let mut lines = written.lines();
        assert_eq!(
            std::path::Path::new(lines.next().unwrap()),
            std::env::current_exe().unwrap()
        );
        assert_eq!(lines.next(), Some("env-probe"));
        assert_eq!(lines.next(), Some("1"));
        let _ = std::fs::remove_file(&out);
    }

    #[cfg(unix)]
    #[test]
    fn extension_service_receives_stable_identity_directory_and_default_cwd() {
        let directory = tempfile::tempdir().unwrap();
        let out = directory.path().join("environment.txt");
        let config = ServiceConfig {
            name: "git-tools.watch".to_string(),
            run: format!(
                "printf '%s\\n%s\\n%s' \"$ROZI_EXTENSION\" \"$ROZI_EXTENSION_DIR\" \"$PWD\" > '{}'",
                out.display()
            ),
            cwd: Some(directory.path().display().to_string()),
            restart: ServiceRestart::Never,
            env: BTreeMap::from([
                ("ROZI_EXTENSION".to_string(), "git-tools".to_string()),
                (
                    "ROZI_EXTENSION_DIR".to_string(),
                    directory.path().display().to_string(),
                ),
            ]),
        };
        let (mut child, _group) = spawn_service_child(&config, None, None).expect("spawn");
        assert!(child.wait().unwrap().success());
        let values: Vec<_> = std::fs::read_to_string(out)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(
            values,
            [
                "git-tools".to_string(),
                directory.path().display().to_string(),
                directory.path().display().to_string(),
            ]
        );
    }

    /// `reap_if_exited` answers `None` while the child runs, reaps it once it exits, and is
    /// idempotent afterwards - it must never signal a group id whose pid it has already released.
    #[test]
    fn reap_if_exited_waits_for_the_child_then_reaps_it_once() {
        let config = ServiceConfig {
            name: "test-reaper".to_string(),
            #[cfg(unix)]
            run: "sleep 0.2".to_string(),
            #[cfg(windows)]
            run: "ping 127.0.0.1 -n 2".to_string(),
            cwd: None,
            restart: ServiceRestart::Never,
            env: BTreeMap::new(),
        };

        let (mut child, group) = spawn_service_child(&config, None, None).expect("spawn child");
        assert!(
            group.reap_if_exited(&mut child).expect("peek").is_none(),
            "a live child must not be reaped"
        );

        let status = loop {
            if let Some(status) = group.reap_if_exited(&mut child).expect("reap") {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());

        // Asking again after the pid is released must bail out before signalling anything: on Unix
        // the group id is that pid, and the kernel is free to have handed it to someone else.
        #[cfg(unix)]
        assert!(
            group.reap_if_exited(&mut child).is_err(),
            "a released pid must not reach the kill"
        );
    }

    /// Walks the real ladder rather than restating the formula: a test that recomputes
    /// `(delay * 2).min(cap)` in its own body passes no matter what the restart paths do.
    #[test]
    fn backoff_progression_and_cap() {
        let mut delay = INITIAL_BACKOFF;
        let mut seen = vec![delay];
        for _ in 0..6 {
            delay = next_backoff(delay);
            seen.push(delay);
        }

        assert_eq!(
            seen,
            [1, 2, 4, 8, 16, 30, 30].map(Duration::from_secs).to_vec()
        );
    }

    /// A spawn that fails goes onto the retry ladder, not straight into dormancy - the ladder is
    /// what makes startup behave like mid-session.
    #[test]
    fn a_failed_spawn_is_retried_until_the_ladder_runs_out() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        let config = ServiceConfig {
            name: "flaky".to_string(),
            run: "does-not-matter".to_string(),
            cwd: None,
            restart: ServiceRestart::OnFailure,
            env: BTreeMap::new(),
        };

        let message = record_spawn_failure(
            &mut state,
            config.name.clone(),
            config.clone(),
            1,
            INITIAL_BACKOFF,
        );
        assert!(message.is_none(), "first failure must not give up");
        let pending = state.services.pending.get("flaky").expect("queued a retry");
        assert_eq!(pending.consecutive_failures, 1);
        assert_eq!(pending.backoff_delay, next_backoff(INITIAL_BACKOFF));
        assert!(state.services.dormant.is_empty());

        let message = record_spawn_failure(
            &mut state,
            config.name.clone(),
            config.clone(),
            MAX_FAILURES,
            MAX_BACKOFF,
        );
        assert!(message.is_some(), "the exhausted ladder reports once");
        assert_eq!(
            state.services.dormant.get("flaky").map(|d| d.reason),
            Some(DormantReason::ExhaustedBackoff)
        );
    }

    /// Repeating an unchanged reload must not revive either kind of dormant service. A material
    /// edit releases the old crash-loop generation so reconciliation can start a clean one.
    #[test]
    fn unchanged_reload_keeps_dormant_services_and_material_change_releases_them() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        let config = ServiceConfig {
            name: "looper".to_string(),
            run: "does-not-matter".to_string(),
            cwd: None,
            restart: ServiceRestart::OnFailure,
            env: BTreeMap::new(),
        };
        state.services.dormant.insert(
            "looper".to_string(),
            DormantService {
                config: config.clone(),
                reason: DormantReason::ExhaustedBackoff,
            },
        );
        state.services.dormant.insert(
            "finished".to_string(),
            DormantService {
                config: config.clone(),
                reason: DormantReason::NormalExit,
            },
        );

        let services = [config];
        state
            .services
            .dormant
            .retain(|_, d| stays_dormant(d, &services));

        assert!(state.services.dormant.contains_key("looper"));
        assert!(state.services.dormant.contains_key("finished"));

        let mut changed = services[0].clone();
        changed.restart = ServiceRestart::Always;
        state
            .services
            .dormant
            .retain(|_, dormant| stays_dormant(dormant, &[changed.clone()]));
        assert!(state.services.dormant.is_empty());
    }

    /// `restart = "never"` must not be queued by a failed spawn either.
    #[test]
    fn a_never_restart_service_is_not_retried_after_a_failed_spawn() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        let config = ServiceConfig {
            name: "once".to_string(),
            run: "does-not-matter".to_string(),
            cwd: None,
            restart: ServiceRestart::Never,
            env: BTreeMap::new(),
        };

        let message =
            record_spawn_failure(&mut state, config.name.clone(), config, 1, INITIAL_BACKOFF);
        assert!(message.is_none());
        assert!(state.services.pending.is_empty());
        assert_eq!(
            state.services.dormant.get("once").map(|d| d.reason),
            Some(DormantReason::NeverRestart)
        );
    }

    fn long_running_service(name: &str) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            #[cfg(unix)]
            run: "sleep 30".to_string(),
            #[cfg(windows)]
            run: "ping 127.0.0.1 -n 30".to_string(),
            cwd: None,
            restart: ServiceRestart::OnFailure,
            env: BTreeMap::from([
                ("ROZI_EXTENSION".to_string(), "dashboard".to_string()),
                (
                    "ROZI_EXTENSION_DIR".to_string(),
                    std::env::temp_dir().display().to_string(),
                ),
            ]),
        }
    }

    #[test]
    fn unchanged_extension_service_survives_but_material_change_restarts_once() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        let original = long_running_service("dashboard.watch");
        assert!(reconcile_service_set(&mut state, vec![original.clone()], None, None).is_empty());
        let first_pid = state.services.running["dashboard.watch"].child.id();

        assert!(reconcile_service_set(&mut state, vec![original.clone()], None, None).is_empty());
        assert_eq!(
            state.services.running["dashboard.watch"].child.id(),
            first_pid,
            "an identical definition keeps the live child"
        );

        let mut changed = original;
        changed
            .env
            .insert("DASHBOARD_MODE".to_string(), "detailed".to_string());
        assert!(reconcile_service_set(&mut state, vec![changed.clone()], None, None).is_empty());
        assert_eq!(state.services.running.len(), 1);
        assert_eq!(state.services.running["dashboard.watch"].config, changed);
        assert_ne!(
            state.services.running["dashboard.watch"].child.id(),
            first_pid,
            "one replacement child owns the changed definition"
        );
        terminate_all(&mut state);
    }

    #[test]
    fn removed_extension_service_stops_and_changed_definition_resets_backoff() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        let original = long_running_service("dashboard.watch");
        state.services.pending.insert(
            original.name.clone(),
            PendingRestart {
                config: original.clone(),
                restart_at: Instant::now() + MAX_BACKOFF,
                backoff_delay: MAX_BACKOFF,
                consecutive_failures: 4,
            },
        );
        let mut changed = original.clone();
        changed.restart = ServiceRestart::Always;
        assert!(reconcile_service_set(&mut state, vec![changed.clone()], None, None).is_empty());
        let running = &state.services.running["dashboard.watch"];
        assert_eq!(running.config, changed);
        assert_eq!(running.backoff_delay, INITIAL_BACKOFF);
        assert_eq!(running.consecutive_failures, 0);

        assert!(reconcile_service_set(&mut state, Vec::new(), None, None).is_empty());
        assert!(state.services.running.is_empty());
        assert!(state.services.pending.is_empty());
        assert!(state.services.dormant.is_empty());
    }
}
