use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::UserCommandAction;
use crate::state::PaneIdentity;

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
                cwd: crate::pane::lifecycle::focused_spawn_cwd(&ctx.state),
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
        UserCommandAction::Exec { command } => exec_shell(ctx, command, env),
        UserCommandAction::ExecDirect { argv } => exec_direct(ctx, argv, env),
        UserCommandAction::Popup { command, keep_open } => crate::ops::popup::open(
            ctx,
            command.clone(),
            None,
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
fn exec_shell(ctx: &mut Context<AppRoot>, command: &str, env: Vec<(String, String)>) -> Update {
    let runner = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    let mut argv = vec![runner.program];
    argv.extend(runner.args);
    argv.push(command.to_string());
    exec_argv(ctx, argv, crate::config::truncate_for_label(command), env)
}

fn exec_direct(ctx: &mut Context<AppRoot>, argv: &[String], env: Vec<(String, String)>) -> Update {
    exec_argv(
        ctx,
        argv.to_vec(),
        crate::config::truncate_for_label(&argv.join(" ")),
        env,
    )
}

fn exec_argv(
    ctx: &mut Context<AppRoot>,
    argv: Vec<String>,
    label: String,
    env: Vec<(String, String)>,
) -> Update {
    let Some((program, args)) = argv.split_first() else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command failed",
            "direct command argv is empty",
        );
        return Update::none();
    };
    let cwd = crate::pane::lifecycle::focused_spawn_cwd(&ctx.state);
    let mut environment = vec![("ROZI".to_string(), "1".to_string())];
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
    }) {
        overflow_link.send(crate::Msg::UserCommandFailed {
            message: format!(
                "`{overflow_label}` was not started; too many detached commands are already running"
            ),
        });
    }
    Update::none()
}
