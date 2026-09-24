use crate::control::{AgentTarget, AgentWaitCondition, CaptureScrollback, ControlCommand};

use super::{ListFormat, parse_list_format};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row(
            "rozi [--session <NAME>] agents <COMMAND> [OPTIONS]",
            "",
        )],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "Waits and prompts need --session <NAME>; they run in the session server.",
        rows: &[
            row("list [--format text|json]", "List semantic agent occupants"),
            row("get <TARGET> [--format text|json]", "Show one agent record"),
            row(
                "read <TARGET> [--scrollback <N|full>]",
                "Read an agent's terminal",
            ),
            row(
                "wait <TARGET> --until <STATE> [--timeout <DUR>]",
                "Wait until an agent reaches a state",
            ),
            row(
                "prompt <TARGET> [--wait <STATE>] [--timeout <DUR>] [--allow-working] <TEXT>",
                "Safely submit a prompt",
            ),
            row(
                "report --agent <ID> --integration <TOKEN> --state <STATE> --seq <N>",
                "Report integration state",
            ),
            row(
                "release --integration <TOKEN> --seq <N> [--target <PANE>]",
                "Release integration state",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("    --target <PANE>", "TARGET: an agent by pane id"),
            row("    --ref <JSON>", "TARGET: an agent by AgentRef JSON"),
            row("    --until <STATE>", "working, blocked, idle, done,"),
            row("", "quiescent, or gone"),
            row("    --wait <STATE>", "The same states as --until"),
            row("    --state <STATE>", "working, blocked, idle, or done"),
            row("    --reason <TEXT>", "Why the agent is in this state"),
            row("    --native-session <ID>", "The agent's own session id"),
            row("    --format text|json", "Output format"),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi agents", "inspect and drive coding agents");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

/// Whether `rozi agents ...` asked for help rather than a command.
///
/// Any help flag wins, even where a prompt's text would otherwise go, so a mistyped
/// `agents prompt --help` shows help instead of submitting `--help` to an agent.
pub(super) fn wants_help(args: &[String]) -> bool {
    args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help")
}

pub(super) fn parse_agents_args(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut iter = args.into_iter();
    let subcommand = iter.next().ok_or_else(|| {
        "agents requires a subcommand (list, get, read, wait, prompt, report, or release)"
            .to_string()
    })?;
    let args = iter.collect::<Vec<_>>();
    match subcommand.as_str() {
        "list" => {
            let (format, rest) = take_format(args, "agents list")?;
            reject_rest(rest, "agents list")?;
            Ok((ControlCommand::AgentsList, format))
        }
        "get" => {
            let (target, format, rest) = parse_target_and_format(args, "agents get")?;
            reject_rest(rest, "agents get")?;
            Ok((ControlCommand::AgentGet { target }, format))
        }
        "read" => parse_read(args),
        "wait" => parse_wait(args),
        "prompt" => parse_prompt(args),
        "report" => parse_report(args),
        "release" => parse_release(args),
        other => Err(format!(
            "unknown agents subcommand `{other}`; expected list, get, read, wait, prompt, report, or release"
        )),
    }
}

fn parse_report(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut target = None;
    let mut agent = None;
    let mut integration = None;
    let mut state = None;
    let mut reason = None;
    let mut native_session = None;
    let mut seq = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--target" => {
                let value = next_value(&mut iter, "--target requires a pane id")?;
                target = Some(
                    value
                        .parse()
                        .map_err(|_| "--target requires a numeric pane id".to_string())?,
                );
            }
            "--state" => {
                let value = next_value(&mut iter, "--state requires a state")?;
                state = Some(parse_report_state(&value)?);
            }
            "--agent" => agent = Some(next_value(&mut iter, "--agent requires an id")?),
            "--integration" => {
                integration = Some(next_value(&mut iter, "--integration requires a token")?)
            }
            "--reason" => reason = Some(next_value(&mut iter, "--reason requires text")?),
            "--native-session" => {
                native_session = Some(next_value(&mut iter, "--native-session requires a value")?)
            }
            "--seq" => {
                let value = next_value(&mut iter, "--seq requires an integer")?;
                seq = Some(
                    value
                        .parse()
                        .map_err(|_| "--seq requires an unsigned integer".to_string())?,
                );
            }
            other => return Err(format!("unexpected agents report argument `{other}`")),
        }
    }
    Ok((
        ControlCommand::AgentReport {
            target,
            agent: agent.ok_or_else(|| "agents report requires --agent".to_string())?,
            integration: integration
                .ok_or_else(|| "agents report requires --integration".to_string())?,
            state: state.ok_or_else(|| "agents report requires --state".to_string())?,
            reason,
            native_session,
            seq: seq.ok_or_else(|| "agents report requires --seq".to_string())?,
        },
        None,
    ))
}

fn parse_release(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut target = None;
    let mut integration = None;
    let mut seq = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--target" => {
                target = Some(
                    next_value(&mut iter, "--target requires a pane id")?
                        .parse()
                        .map_err(|_| "--target requires a numeric pane id".to_string())?,
                )
            }
            "--seq" => {
                seq = Some(
                    next_value(&mut iter, "--seq requires an integer")?
                        .parse()
                        .map_err(|_| "--seq requires an unsigned integer".to_string())?,
                )
            }
            "--integration" => {
                integration = Some(next_value(&mut iter, "--integration requires a token")?)
            }
            other => return Err(format!("unexpected agents release argument `{other}`")),
        }
    }
    Ok((
        ControlCommand::AgentRelease {
            target,
            integration: integration
                .ok_or_else(|| "agents release requires --integration".to_string())?,
            seq: seq.ok_or_else(|| "agents release requires --seq".to_string())?,
        },
        None,
    ))
}

fn parse_report_state(
    value: &str,
) -> std::result::Result<crate::session::protocol::AgentState, String> {
    match value {
        "working" => Ok(crate::session::protocol::AgentState::Working),
        "blocked" => Ok(crate::session::protocol::AgentState::Blocked),
        "idle" => Ok(crate::session::protocol::AgentState::Idle),
        "done" => Ok(crate::session::protocol::AgentState::Done),
        _ => Err(format!("unknown agent state `{value}`")),
    }
}

fn parse_prompt(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut target = None;
    let mut prompt = None;
    let mut wait = None;
    let mut timeout_ms = None;
    let mut allow_working = false;
    let mut format = None;
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--target" | "--ref" => set_target(&mut target, &argument, &mut iter)?,
            "--wait" => {
                let value = next_value(&mut iter, "--wait requires a condition")?;
                if wait.replace(parse_condition(&value)?).is_some() {
                    return Err("agents prompt --wait specified more than once".into());
                }
            }
            "--timeout" => {
                let value = next_value(&mut iter, "--timeout requires a duration")?;
                if timeout_ms
                    .replace(super::parse_duration_ms(&value, "--timeout")?)
                    .is_some()
                {
                    return Err("agents prompt --timeout specified more than once".into());
                }
            }
            "--allow-working" => allow_working = true,
            "--format" => set_format(&mut format, &mut iter, "agents prompt")?,
            value if prompt.is_none() => prompt = Some(value.to_string()),
            other => return Err(format!("unexpected agents prompt argument `{other}`")),
        }
    }
    Ok((
        ControlCommand::AgentPrompt {
            target: require_target(target, "agents prompt")?,
            prompt: prompt.ok_or_else(|| "agents prompt requires prompt text".to_string())?,
            wait,
            timeout_ms,
            allow_working,
        },
        format,
    ))
}

fn parse_read(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut target = None;
    let mut scrollback = None;
    let mut format = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--target" | "--ref" => set_target(&mut target, &flag, &mut iter)?,
            "--scrollback" => {
                let value = next_value(&mut iter, "--scrollback requires a value")?;
                if scrollback
                    .replace(CaptureScrollback::parse_cli(&value)?)
                    .is_some()
                {
                    return Err("agents read --scrollback specified more than once".into());
                }
            }
            "--format" => set_format(&mut format, &mut iter, "agents read")?,
            other => return Err(format!("unexpected agents read argument `{other}`")),
        }
    }
    Ok((
        ControlCommand::AgentRead {
            target: require_target(target, "agents read")?,
            scrollback,
        },
        format,
    ))
}

fn parse_wait(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut target = None;
    let mut until = None;
    let mut timeout_ms = None;
    let mut format = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--target" | "--ref" => set_target(&mut target, &flag, &mut iter)?,
            "--until" => {
                let value = next_value(&mut iter, "--until requires a condition")?;
                if until.replace(parse_condition(&value)?).is_some() {
                    return Err("agents wait --until specified more than once".into());
                }
            }
            "--timeout" => {
                let value = next_value(&mut iter, "--timeout requires a duration")?;
                if timeout_ms
                    .replace(super::parse_duration_ms(&value, "--timeout")?)
                    .is_some()
                {
                    return Err("agents wait --timeout specified more than once".into());
                }
            }
            "--format" => set_format(&mut format, &mut iter, "agents wait")?,
            other => return Err(format!("unexpected agents wait argument `{other}`")),
        }
    }
    Ok((
        ControlCommand::AgentWait {
            target: require_target(target, "agents wait")?,
            until: until.ok_or_else(|| {
                "agents wait requires --until working|blocked|idle|done|quiescent|gone".to_string()
            })?,
            timeout_ms,
        },
        format,
    ))
}

fn parse_target_and_format(
    args: Vec<String>,
    command: &str,
) -> std::result::Result<(AgentTarget, Option<ListFormat>, Vec<String>), String> {
    let mut target = None;
    let mut format = None;
    let mut rest = Vec::new();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--target" | "--ref" => set_target(&mut target, &flag, &mut iter)?,
            "--format" => set_format(&mut format, &mut iter, command)?,
            _ => rest.push(flag),
        }
    }
    Ok((require_target(target, command)?, format, rest))
}

fn take_format(
    args: Vec<String>,
    command: &str,
) -> std::result::Result<(Option<ListFormat>, Vec<String>), String> {
    let mut format = None;
    let mut rest = Vec::new();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        if flag == "--format" {
            set_format(&mut format, &mut iter, command)?;
        } else {
            rest.push(flag);
        }
    }
    Ok((format, rest))
}

fn set_target(
    target: &mut Option<AgentTarget>,
    flag: &str,
    iter: &mut impl Iterator<Item = String>,
) -> std::result::Result<(), String> {
    if target.is_some() {
        return Err("agent target specified more than once".into());
    }
    let value = next_value(iter, &format!("{flag} requires a value"))?;
    *target = Some(if flag == "--target" {
        AgentTarget::Pane(
            value
                .parse()
                .map_err(|_| "--target requires a numeric pane id".to_string())?,
        )
    } else {
        AgentTarget::Ref(
            serde_json::from_str(&value)
                .map_err(|error| format!("--ref requires an AgentRef JSON object: {error}"))?,
        )
    });
    Ok(())
}

fn set_format(
    format: &mut Option<ListFormat>,
    iter: &mut impl Iterator<Item = String>,
    command: &str,
) -> std::result::Result<(), String> {
    let value = next_value(iter, "--format requires text or json")?;
    if format
        .replace(parse_list_format(&value, command)?)
        .is_some()
    {
        return Err(format!("{command} --format specified more than once"));
    }
    Ok(())
}

fn require_target(
    target: Option<AgentTarget>,
    command: &str,
) -> std::result::Result<AgentTarget, String> {
    target.ok_or_else(|| format!("{command} requires --target PANE or --ref JSON"))
}

fn parse_condition(value: &str) -> std::result::Result<AgentWaitCondition, String> {
    match value {
        "working" => Ok(AgentWaitCondition::Working),
        "blocked" => Ok(AgentWaitCondition::Blocked),
        "idle" => Ok(AgentWaitCondition::Idle),
        "done" => Ok(AgentWaitCondition::Done),
        "quiescent" => Ok(AgentWaitCondition::Quiescent),
        "gone" => Ok(AgentWaitCondition::Gone),
        _ => Err(format!("unknown agent wait condition `{value}`")),
    }
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    message: &str,
) -> std::result::Result<String, String> {
    iter.next().ok_or_else(|| message.to_string())
}

fn reject_rest(rest: Vec<String>, command: &str) -> std::result::Result<(), String> {
    match rest.first() {
        Some(value) => Err(format!("unexpected argument `{value}` after {command}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ParsedCli, parse_cli_args};
    use super::*;

    #[test]
    fn agents_namespace_owns_its_help() {
        for args in [
            vec!["agents"],
            vec!["agents", "--help"],
            vec!["agents", "-h"],
            vec!["agents", "wait", "--help"],
            vec!["--session", "dev", "agents", "--help"],
            // Never submitted to the agent as prompt text.
            vec!["agents", "prompt", "--target", "3", "--help"],
        ] {
            assert!(
                matches!(
                    parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect()),
                    Ok(ParsedCli::AgentsHelp)
                ),
                "{args:?} should print agents help"
            );
        }
    }

    #[test]
    fn parses_wait_with_a_semantic_deadline() {
        let (command, format) = parse_agents_args(vec![
            "wait".into(),
            "--target".into(),
            "3".into(),
            "--until".into(),
            "quiescent".into(),
            "--timeout".into(),
            "30s".into(),
            "--format".into(),
            "json".into(),
        ])
        .unwrap();
        assert_eq!(format, Some(ListFormat::Json));
        assert_eq!(
            command,
            ControlCommand::AgentWait {
                target: AgentTarget::Pane(3),
                until: AgentWaitCondition::Quiescent,
                timeout_ms: Some(30_000),
            }
        );
    }

    #[test]
    fn parses_atomic_prompt_wait() {
        let (command, _) = parse_agents_args(vec![
            "prompt".into(),
            "--target".into(),
            "3".into(),
            "--wait".into(),
            "idle".into(),
            "--timeout".into(),
            "45s".into(),
            "fix the test".into(),
        ])
        .unwrap();
        assert_eq!(
            command,
            ControlCommand::AgentPrompt {
                target: AgentTarget::Pane(3),
                prompt: "fix the test".into(),
                wait: Some(AgentWaitCondition::Idle),
                timeout_ms: Some(45_000),
                allow_working: false,
            }
        );
    }

    #[test]
    fn parses_sequence_fenced_integration_report() {
        let (command, _) = parse_agents_args(vec![
            "report".into(),
            "--target".into(),
            "3".into(),
            "--agent".into(),
            "claude".into(),
            "--integration".into(),
            "hook-abc".into(),
            "--state".into(),
            "working".into(),
            "--native-session".into(),
            "opaque-123".into(),
            "--seq".into(),
            "42".into(),
        ])
        .unwrap();
        assert_eq!(
            command,
            ControlCommand::AgentReport {
                target: Some(3),
                agent: "claude".into(),
                integration: "hook-abc".into(),
                state: crate::session::protocol::AgentState::Working,
                reason: None,
                native_session: Some("opaque-123".into()),
                seq: 42,
            }
        );
    }
}
