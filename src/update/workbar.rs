use tui_lipan::prelude::*;

use crate::config::WorkbarSegment;
use crate::{AppRoot, Msg};

const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const COMMAND_CAPTURE_BYTES: usize = 64 * 1024;
const COMMAND_BUSY_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

fn command_interval(ctx: &Context<AppRoot>, command: &str) -> Option<u64> {
    ctx.state
        .config
        .workbar
        .left
        .iter()
        .chain(ctx.state.config.workbar.right.iter())
        .find_map(|item| match &item.segment {
            WorkbarSegment::Command {
                command: configured,
                interval_secs,
            } if configured == command => Some(*interval_secs),
            _ => None,
        })
}

/// Kick every configured command once the app's command link exists and after each config reload.
pub(crate) fn request_command_polls(ctx: &Context<AppRoot>) {
    let Some(link) = ctx.state.command_link.as_ref() else {
        return;
    };
    let epoch = ctx.state.workbar.command_epoch;
    for (command, _) in ctx.state.config.workbar.command_specs() {
        link.send(Msg::WorkbarCommandPoll { epoch, command });
    }
}

pub(super) fn poll_command(ctx: &mut Context<AppRoot>, epoch: u64, command: String) -> Update {
    let configured = command_interval(ctx, &command).is_some();
    match begin_poll(&mut ctx.state.workbar, epoch, &command, configured) {
        PollDecision::Ignored => return Update::none(),
        PollDecision::Busy => {
            return Update::command_only(Command::after(
                COMMAND_BUSY_RETRY,
                move |link: CommandLink<Msg>| {
                    link.send(Msg::WorkbarCommandPoll { epoch, command });
                },
            ));
        }
        PollDecision::Started => {}
    }
    let shell = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    Update::command_only(Command::spawn(move |link: CommandLink<Msg>| {
        let output = first_output_line(crate::platform::command::run_bounded_shell_command(
            &shell,
            &command,
            COMMAND_TIMEOUT,
            COMMAND_CAPTURE_BYTES,
        ));
        link.send(Msg::WorkbarCommandOutput {
            epoch,
            command,
            output,
        });
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PollDecision {
    Ignored,
    Busy,
    Started,
}

fn begin_poll(
    state: &mut crate::state::WorkbarState,
    epoch: u64,
    command: &str,
    configured: bool,
) -> PollDecision {
    if epoch != state.command_epoch || !configured {
        return PollDecision::Ignored;
    }
    if state.command_in_flight.contains_key(command) {
        return PollDecision::Busy;
    }
    state.command_in_flight.insert(command.to_string(), epoch);
    PollDecision::Started
}

fn first_output_line(result: std::io::Result<crate::platform::command::CommandOutput>) -> String {
    result
        .ok()
        .filter(|output| !output.timed_out && output.status == Some(0))
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::trim)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub(super) fn command_output(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    command: String,
    output: String,
) -> Update {
    let interval_secs = command_interval(ctx, &command);
    let Some(changed) = finish_output(
        &mut ctx.state.workbar,
        epoch,
        &command,
        output,
        interval_secs.is_some(),
    ) else {
        return Update::none();
    };
    let next = Command::after(
        std::time::Duration::from_secs(interval_secs.expect("configured command has interval")),
        move |link: CommandLink<Msg>| {
            link.send(Msg::WorkbarCommandPoll { epoch, command });
        },
    );
    if changed {
        Update::with_command(next)
    } else {
        Update::command_only(next)
    }
}

fn finish_output(
    state: &mut crate::state::WorkbarState,
    epoch: u64,
    command: &str,
    output: String,
    configured: bool,
) -> Option<bool> {
    if state.command_in_flight.get(command) == Some(&epoch) {
        state.command_in_flight.remove(command);
    }
    if epoch != state.command_epoch || !configured {
        return None;
    }
    if state.command_output.get(command) == Some(&output) {
        return Some(false);
    }
    state.command_output.insert(command.to_string(), output);
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_and_removed_results_clear_only_matching_runs_and_do_not_replace_output() {
        let mut state = crate::state::WorkbarState {
            command_epoch: 8,
            command_in_flight: std::collections::HashMap::from([
                ("kept".to_string(), 7),
                ("removed".to_string(), 8),
                ("racing".to_string(), 8),
            ]),
            command_output: std::collections::HashMap::from([(
                "kept".to_string(),
                "current".to_string(),
            )]),
        };

        assert_eq!(
            finish_output(&mut state, 7, "kept", "stale".to_string(), true),
            None
        );
        assert_eq!(
            finish_output(&mut state, 8, "removed", "removed".to_string(), false),
            None
        );
        assert_eq!(
            finish_output(&mut state, 7, "racing", "stale".to_string(), false),
            None
        );

        assert!(!state.command_in_flight.contains_key("kept"));
        assert!(!state.command_in_flight.contains_key("removed"));
        assert_eq!(
            state.command_in_flight.get("racing"),
            Some(&8),
            "a stale result must not clear a newer run's guard"
        );
        assert_eq!(state.command_output["kept"], "current");
        assert!(!state.command_output.contains_key("removed"));
    }

    #[test]
    fn poll_rejects_stale_removed_and_overlapping_runs() {
        let mut state = crate::state::WorkbarState {
            command_epoch: 6,
            ..Default::default()
        };

        assert_eq!(
            begin_poll(&mut state, 5, "date", true),
            PollDecision::Ignored
        );
        assert_eq!(
            begin_poll(&mut state, 6, "removed", false),
            PollDecision::Ignored
        );
        assert!(state.command_in_flight.is_empty());

        state.command_in_flight.insert("date".to_string(), 5);
        assert_eq!(begin_poll(&mut state, 6, "date", true), PollDecision::Busy);
        assert_eq!(state.command_in_flight.get("date"), Some(&5));
        assert_eq!(
            begin_poll(&mut state, 6, "new", true),
            PollDecision::Started
        );
        assert_eq!(state.command_in_flight.get("new"), Some(&6));
    }

    #[test]
    fn unchanged_output_is_retained_and_changed_output_replaces_it() {
        let mut state = crate::state::WorkbarState::default();
        state
            .command_output
            .insert("date".to_string(), "one".to_string());

        assert_eq!(
            finish_output(&mut state, 0, "date", "one".to_string(), true),
            Some(false)
        );
        assert_eq!(
            finish_output(&mut state, 0, "date", "two".to_string(), true),
            Some(true)
        );
        assert_eq!(state.command_output["date"], "two");
    }

    #[test]
    fn command_output_uses_first_trimmed_line_and_blanks_failures() {
        let output = |status, timed_out, stdout: &str| crate::platform::command::CommandOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            status,
            timed_out,
        };
        assert_eq!(
            first_output_line(Ok(output(Some(0), false, "  first  \nsecond\n"))),
            "first"
        );
        assert_eq!(first_output_line(Ok(output(Some(1), false, "ignored"))), "");
        assert_eq!(first_output_line(Ok(output(Some(0), true, "ignored"))), "");
        assert_eq!(
            first_output_line(Err(std::io::Error::other("spawn failed"))),
            ""
        );
    }
}
