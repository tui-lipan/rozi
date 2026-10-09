use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::UserCommandAction;
use crate::state::{PaneId, PaneIdentity};

pub(crate) fn execute(ctx: &mut Context<AppRoot>, action: &UserCommandAction) -> Update {
    execute_with_env(ctx, action, Vec::new())
}

/// Run a user command with extra environment for this spawn only.
///
/// `env` is how a caller hands an untrusted value — a filename from the file tree, say — to a
/// command line without ever putting it *in* that command line. The command references it as
/// `"$VAR"`, which the shell expands as a single word rather than re-parsing for command syntax.
/// `Send` ignores it: it starts no process, it only types text into an existing one.
pub(crate) fn execute_with_env(
    ctx: &mut Context<AppRoot>,
    action: &UserCommandAction,
    env: Vec<(String, String)>,
) -> Update {
    execute_with_env_from_pane(ctx, action, env, None)
}

pub(crate) fn execute_with_env_from_pane(
    ctx: &mut Context<AppRoot>,
    action: &UserCommandAction,
    env: Vec<(String, String)>,
    source: Option<PaneId>,
) -> Update {
    let spawn_cwd = match source {
        Some(id) => crate::pane::lifecycle::find_pane(&ctx.state, id)
            .and_then(|pane| {
                if ctx.state.current().remote_host.is_some() {
                    pane.server_cwd_ref()
                } else {
                    pane.local_cwd_ref()
                }
            })
            .map(str::to_string),
        None => crate::pane::lifecycle::focused_spawn_cwd(&ctx.state),
    };
    let local_cwd = match source {
        Some(_) if ctx.state.current().remote_host.is_some() => None,
        Some(id) => crate::pane::lifecycle::find_pane(&ctx.state, id)
            .and_then(|pane| pane.local_cwd_ref())
            .map(str::to_string),
        None => crate::pane::lifecycle::focused_local_cwd(&ctx.state),
    };
    match action {
        // `Exec` needs no session: it starts no PTY, so there is nothing for a session to own.
        UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. } => {}
        UserCommandAction::Run { .. } | UserCommandAction::Popup { .. } => {
            if let Some(update) = crate::ops::session::ensure_session_for_pty(
                ctx,
                crate::state::PendingSessionAction::UserCommand {
                    action: action.clone(),
                    env: env.clone(),
                },
            ) {
                return update;
            }
        }
        UserCommandAction::Send(_) => {}
    }
    match action {
        UserCommandAction::Run { command, keep_open } => {
            let identity = PaneIdentity {
                launch: Some(crate::pane::launch::PaneLaunch::shell(command.clone())),
                keep_open: *keep_open,
                // `cargo build` means "build the project I am looking at"; without this the command
                // runs wherever the session server was started.
                cwd: spawn_cwd,
                env,
                ..PaneIdentity::default()
            };
            if ctx.state.scratch_visible {
                return crate::pane::lifecycle::spawn_pane_in_scratch(
                    ctx,
                    ctx.state.scratch.focused_pane,
                    identity,
                )
                .1;
            }
            crate::pane::lifecycle::spawn_interactive_pane(
                ctx,
                ctx.state.current().active_workspace,
                None,
                identity,
            )
            .1
        }
        UserCommandAction::Send(text) => {
            let Some(id) = ctx.state.focused_pane() else {
                return Update::full();
            };
            if let Err(err) =
                crate::pane::pty_events::send_pane_bytes(ctx, id, text.as_bytes().to_vec())
            {
                crate::pane::pty_events::notify_error(ctx, "Command failed", err);
            }
            Update::full()
        }
        UserCommandAction::Exec { command } => exec_shell(ctx, command, env, local_cwd),
        UserCommandAction::ExecDirect { argv } => exec_direct(ctx, argv, env, local_cwd),
        UserCommandAction::Popup { command, keep_open } => crate::ops::popup::open(
            ctx,
            command.clone(),
            spawn_cwd,
            None,
            None,
            None,
            *keep_open,
            env,
        )
        .unwrap_or_else(|error| {
            crate::pane::pty_events::notify_error(ctx, "Popup failed", error);
            Update::full()
        }),
    }
}

/// Run an `exec` command detached: no pane, no popup, output discarded.
///
/// Spawned like a hook - one thread, `command_shell`, null stdio - but unlike a hook it is waited
/// on, so a non-zero exit can raise a toast. A binding that quietly does nothing when its command
/// is missing is indistinguishable from a binding that is not wired up at all.
fn exec_shell(
    ctx: &mut Context<AppRoot>,
    command: &str,
    env: Vec<(String, String)>,
    cwd: Option<String>,
) -> Update {
    let runner = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    let mut argv = vec![runner.program];
    argv.extend(runner.args);
    argv.push(command.to_string());
    exec_argv(
        ctx,
        argv,
        crate::config::truncate_for_label(command),
        env,
        cwd,
    )
}

fn exec_direct(
    ctx: &mut Context<AppRoot>,
    argv: &[String],
    env: Vec<(String, String)>,
    cwd: Option<String>,
) -> Update {
    exec_argv(
        ctx,
        argv.to_vec(),
        crate::config::truncate_for_label(&argv.join(" ")),
        env,
        cwd,
    )
}

fn exec_argv(
    ctx: &mut Context<AppRoot>,
    argv: Vec<String>,
    label: String,
    env: Vec<(String, String)>,
    cwd: Option<String>,
) -> Update {
    let Some((program, args)) = argv.split_first() else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command failed",
            "direct command argv is empty",
        );
        return Update::none();
    };
    // The cwd belongs to the machine running the process. This one runs here, on the client, so it
    // gets the focused pane's directory only when that pane is local too. Under `--remote` the
    // pane's path names a directory on the session host; handed to a local spawn it would either
    // fail or, where the same path exists here, run in the wrong directory without a word.
    let mut environment = vec![
        (
            "ROZI_SOURCE_PANE".to_string(),
            ctx.state
                .focused_pane()
                .map(|id| id.to_string())
                .unwrap_or_default(),
        ),
        (
            "ROZI_SOURCE_SESSION".to_string(),
            ctx.state
                .current()
                .session_instance
                .as_ref()
                .map(|instance| instance.as_str().to_string())
                .unwrap_or_default(),
        ),
        ("ROZI".to_string(), "1".to_string()),
        (
            "ROZI_PANE".to_string(),
            ctx.state
                .focused_pane()
                .map(|id| id.to_string())
                .unwrap_or_default(),
        ),
        (
            "ROZI_REMOTE_HOST".to_string(),
            ctx.state.current().remote_host.clone().unwrap_or_default(),
        ),
    ];
    if let Some(path) = ctx.state.control_socket_path.as_deref() {
        environment.push(("ROZI_SOCKET".to_string(), path.display().to_string()));
    }
    if let Some(path) = crate::platform::paths::current_binary() {
        environment.push(("ROZI_BIN".to_string(), path.display().to_string()));
    }
    // Per-spawn additions last so a caller-supplied value wins, matching `pane_env`.
    environment.extend(env);

    let Some(link) = ctx.state.command_link.clone() else {
        return Update::none();
    };
    let program = program.clone();
    let args = args.to_vec();
    let overflow_link = link.clone();
    let overflow_label = label.clone();
    let command_palette_handoff = ctx
        .state
        .command_palette_handoff
        .as_ref()
        .map(|handoff| handoff.epoch);
    if !crate::jobs::try_spawn(move || {
        let mut process = std::process::Command::new(program);
        process
            .args(args)
            .envs(environment)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let failure = match process.spawn() {
            Ok(mut child) => match child.wait() {
                Ok(status) if status.success() => None,
                Ok(status) => Some(match status.code() {
                    Some(code) => format!("`{label}` exited with status {code}"),
                    None => format!("`{label}` was terminated by a signal"),
                }),
                Err(err) => Some(format!("`{label}` could not be waited on: {err}")),
            },
            Err(err) => Some(format!("`{label}` could not start: {err}")),
        };
        if let Some(message) = failure {
            link.send(crate::Msg::UserCommandFailed { message });
        }
        if let Some(epoch) = command_palette_handoff {
            link.send(crate::Msg::CommandPaletteHandoffFinished { epoch });
        }
    }) {
        overflow_link.send(crate::Msg::UserCommandFailed {
            message: format!(
                "`{overflow_label}` was not started; too many detached commands are already running"
            ),
        });
        if let Some(epoch) = command_palette_handoff {
            overflow_link.send(crate::Msg::CommandPaletteHandoffFinished { epoch });
        }
    }
    Update::none()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tui_lipan::TestBackend;

    fn settled_backend() -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline, "no command link");
            backend.pump().expect("settle the mount");
            std::thread::yield_now();
        }
        backend
    }

    /// Run `exec` from a pane whose directory is `pane_cwd` and return the directory the command
    /// actually started in.
    fn exec_cwd(remote: bool, pane_cwd: &str) -> String {
        let scratch = tempfile::tempdir().unwrap();
        let out = scratch.path().join("pwd");
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            if remote {
                state.current_mut().remote_host = Some("pc".to_string());
            }
            let focused = state.current().workspaces[0].panes[0].id;
            state.current_mut().focused_pane = Some(focused);
            state.current_mut().workspaces[0].focused_pane = Some(focused);
            state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(pane_cwd.to_string());
        }
        backend
            .state_mut()
            .config
            .commands
            .push(crate::config::NamedCommand {
                id: "probe".to_string(),
                label: None,
                action: UserCommandAction::Exec {
                    command: format!("pwd -P > '{}'", out.display()),
                },
                category: "Test".to_string(),
                env: Vec::new(),
                default_key: None,
                hidden: false,
            });
        let index = backend.state().config.commands.len() - 1;
        backend
            .dispatch(crate::Msg::RunAction(
                crate::input::Action::RunNamedCommand(index),
            ))
            .expect("run the command");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let _ = backend.pump();
            if let Ok(text) = std::fs::read_to_string(&out)
                && text.ends_with('\n')
            {
                return text.trim_end().to_string();
            }
            assert!(std::time::Instant::now() < deadline, "exec never ran");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn canonical(path: &std::path::Path) -> String {
        std::fs::canonicalize(path).unwrap().display().to_string()
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    /// A local pane's directory is a directory here, so a client-run command starts in it.
    #[test]
    fn exec_from_a_local_pane_starts_in_its_directory() {
        on_test_thread(|| {
            let project = tempfile::tempdir().unwrap();
            let cwd = exec_cwd(false, &project.path().display().to_string());
            assert_eq!(cwd, canonical(project.path()));
        });
    }

    /// A remote pane's directory does not exist here. The command still runs - on the client, in
    /// the client's own directory - instead of failing to start over a path from another machine.
    #[test]
    fn exec_from_a_remote_pane_whose_directory_is_missing_here_still_runs_on_the_client() {
        on_test_thread(|| {
            let cwd = exec_cwd(true, "/nonexistent/rozi/remote/project");
            assert_eq!(cwd, canonical(&std::env::current_dir().unwrap()));
        });
    }

    /// The dangerous case: the remote path happens to exist on the client too. Running there would
    /// look right and act on the wrong machine's files, so the client must not adopt it.
    #[test]
    fn exec_from_a_remote_pane_never_adopts_a_path_that_also_exists_here() {
        on_test_thread(|| {
            let same_path = tempfile::tempdir().unwrap();
            let cwd = exec_cwd(true, &same_path.path().display().to_string());
            assert_ne!(cwd, canonical(same_path.path()));
            assert_eq!(cwd, canonical(&std::env::current_dir().unwrap()));
        });
    }
}
