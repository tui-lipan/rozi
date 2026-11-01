use crate::extension_runtime::supervisor::{
    BackoffPolicy, MAX_FAILURES, RestartOutcome, StopReason,
};
use crate::ops::services::{record_spawn_failure, spawn_service_child};
use crate::state::{DormantReason, DormantService, PendingRestart, RunningService};
use crate::{AppRoot, Msg};
use std::time::{Duration, Instant};
use tui_lipan::prelude::*;

pub(crate) fn handle_tick(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if epoch != ctx.state.services.epoch {
        return Update::none();
    }

    let command_shell = ctx.state.config.command_shell.clone();
    let control_socket = ctx.state.control_socket_path.clone();

    // 1. Reap whatever exited since the last tick. This contains each dead service's leftover
    //    descendants as part of reaping it, so the loop below has nothing left to terminate.
    let mut exited = Vec::new();
    for (name, running) in &mut ctx.state.services.running {
        match running.group.reap_if_exited(&mut running.child) {
            Ok(Some(status)) => {
                exited.push((name.clone(), Ok(status)));
            }
            Err(err) => {
                exited.push((name.clone(), Err(err)));
            }
            Ok(None) => {
                // Still running
            }
        }
    }

    for (name, exit_result) in exited {
        let running = match ctx.state.services.running.remove(&name) {
            Some(r) => r,
            None => continue,
        };

        // Already reaped and contained by `reap_if_exited`; terminating again would signal a group
        // id the kernel is free to have reused.
        let config = running.config.clone();
        let log = running.log;

        let (is_success, failure_detail) = match exit_result {
            Ok(status) => (status.success(), status.to_string()),
            Err(err) => (false, format!("could not read exit status: {err}")),
        };
        // After the service's last output, which its log may still be reading.
        if let Some(log) = log {
            log.finish(&format!("exited: {failure_detail}"));
        }
        let hint = crate::ops::services::log_hint(&config);

        match BackoffPolicy::default().evaluate_exit(
            config.restart,
            is_success,
            Some(running.started_at),
            running.consecutive_failures,
            running.backoff_delay,
            Instant::now(),
        ) {
            RestartOutcome::Stop {
                reason: StopReason::NeverRestart,
                ..
            } => {
                if !is_success {
                    crate::pane::pty_events::notify_error(
                        ctx,
                        "Service failed",
                        format!("`{name}` {failure_detail}\nRestart is disabled{hint}"),
                    );
                }
                ctx.state.services.dormant.insert(
                    name,
                    DormantService {
                        config,
                        reason: DormantReason::NeverRestart,
                    },
                );
            }
            RestartOutcome::Stop {
                reason: StopReason::NormalExit,
                ..
            } => {
                ctx.state.services.dormant.insert(
                    name,
                    DormantService {
                        config,
                        reason: DormantReason::NormalExit,
                    },
                );
            }
            RestartOutcome::Stop {
                reason: StopReason::ExhaustedBackoff,
                ..
            } => {
                crate::pane::pty_events::notify_error(
                    ctx,
                    "Service failed",
                    format!(
                        "`{name}` failed {MAX_FAILURES} times\n\
                         Last result: {failure_detail}{hint}"
                    ),
                );
                ctx.state.services.dormant.insert(
                    name,
                    DormantService {
                        config,
                        reason: DormantReason::ExhaustedBackoff,
                    },
                );
            }
            RestartOutcome::Restart {
                restart_at,
                next_backoff,
                failures,
                ..
            } => {
                ctx.state.services.pending.insert(
                    name,
                    PendingRestart {
                        config,
                        restart_at,
                        backoff_delay: next_backoff,
                        consecutive_failures: failures,
                    },
                );
            }
        }
    }

    // 2. Check pending services due for restart
    let now = Instant::now();
    let due_names: Vec<String> = ctx
        .state
        .services
        .pending
        .iter()
        .filter(|(_, p)| now >= p.restart_at)
        .map(|(name, _)| name.clone())
        .collect();

    for name in due_names {
        if let Some(pending) = ctx.state.services.pending.remove(&name) {
            match spawn_service_child(
                &pending.config,
                command_shell.as_deref(),
                control_socket.as_deref(),
            ) {
                Ok((child, group, log)) => {
                    ctx.state.services.running.insert(
                        name,
                        RunningService {
                            config: pending.config,
                            child,
                            group,
                            log,
                            started_at: Instant::now(),
                            backoff_delay: pending.backoff_delay,
                            consecutive_failures: pending.consecutive_failures,
                        },
                    );
                }
                Err(err) => {
                    if let Some(message) = record_spawn_failure(
                        &mut ctx.state,
                        name,
                        pending.config,
                        pending.consecutive_failures + 1,
                        pending.backoff_delay,
                        &err.to_string(),
                    ) {
                        crate::pane::pty_events::notify_error(ctx, "Service failed", message);
                    }
                }
            }
        }
    }

    // 3. Schedule next tick if any running or pending
    if !ctx.state.services.running.is_empty() || !ctx.state.services.pending.is_empty() {
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
