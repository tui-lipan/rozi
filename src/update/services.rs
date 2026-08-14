use crate::config::ServiceRestart;
use crate::ops::services::{
    INITIAL_BACKOFF, MAX_FAILURES, next_backoff, record_spawn_failure, spawn_service_child,
    terminate_service,
};
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

    // 1. Check all running services with try_wait()
    let mut exited = Vec::new();
    for (name, running) in &mut ctx.state.services.running {
        match running.child.try_wait() {
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

        let mut consecutive_failures = running.consecutive_failures;
        let mut backoff_delay = running.backoff_delay;
        if running.started_at.elapsed()
            >= Duration::from_secs(crate::ops::services::UPTIME_RESET_SECS)
        {
            consecutive_failures = 0;
            backoff_delay = INITIAL_BACKOFF;
        }
        let config = running.config.clone();
        terminate_service(running);

        let is_success = match exit_result {
            Ok(status) => status.success(),
            Err(_) => false,
        };

        match config.restart {
            ServiceRestart::Never => {
                ctx.state.services.dormant.insert(
                    name,
                    DormantService {
                        config,
                        reason: DormantReason::NeverRestart,
                    },
                );
            }
            ServiceRestart::OnFailure => {
                if is_success {
                    ctx.state.services.dormant.insert(
                        name,
                        DormantService {
                            config,
                            reason: DormantReason::NormalExit,
                        },
                    );
                } else {
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_FAILURES {
                        crate::pty_events::notify_error(
                            ctx,
                            "Service failed",
                            format!("Service '{name}' failed {MAX_FAILURES} times; stopping"),
                        );
                        ctx.state.services.dormant.insert(
                            name,
                            DormantService {
                                config,
                                reason: DormantReason::ExhaustedBackoff,
                            },
                        );
                    } else {
                        let restart_at = Instant::now() + backoff_delay;
                        let next_backoff = next_backoff(backoff_delay);
                        ctx.state.services.pending.insert(
                            name,
                            PendingRestart {
                                config,
                                restart_at,
                                backoff_delay: next_backoff,
                                consecutive_failures,
                            },
                        );
                    }
                }
            }
            ServiceRestart::Always => {
                if is_success {
                    // A clean exit is not a failure, so it never counts toward dormancy - but it
                    // still steps the ladder. Without that, a service that exits 0 immediately
                    // respawns once a second for the life of the session, silently. The uptime
                    // reset above is what returns a healthy service to a 1s restart.
                    let restart_at = Instant::now() + backoff_delay;
                    ctx.state.services.pending.insert(
                        name,
                        PendingRestart {
                            config,
                            restart_at,
                            backoff_delay: next_backoff(backoff_delay),
                            consecutive_failures,
                        },
                    );
                } else {
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_FAILURES {
                        crate::pty_events::notify_error(
                            ctx,
                            "Service failed",
                            format!("Service '{name}' failed {MAX_FAILURES} times; stopping"),
                        );
                        ctx.state.services.dormant.insert(
                            name,
                            DormantService {
                                config,
                                reason: DormantReason::ExhaustedBackoff,
                            },
                        );
                    } else {
                        let restart_at = Instant::now() + backoff_delay;
                        let next_backoff = next_backoff(backoff_delay);
                        ctx.state.services.pending.insert(
                            name,
                            PendingRestart {
                                config,
                                restart_at,
                                backoff_delay: next_backoff,
                                consecutive_failures,
                            },
                        );
                    }
                }
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
                Ok((child, group)) => {
                    ctx.state.services.running.insert(
                        name,
                        RunningService {
                            config: pending.config,
                            child,
                            group,
                            started_at: Instant::now(),
                            backoff_delay: pending.backoff_delay,
                            consecutive_failures: pending.consecutive_failures,
                        },
                    );
                }
                Err(_) => {
                    if let Some(message) = record_spawn_failure(
                        &mut ctx.state,
                        name,
                        pending.config,
                        pending.consecutive_failures + 1,
                        pending.backoff_delay,
                    ) {
                        crate::pty_events::notify_error(ctx, "Service failed", message);
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
