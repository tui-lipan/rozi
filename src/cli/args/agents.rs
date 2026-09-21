use crate::control::{AgentTarget, AgentWaitCondition, CaptureScrollback, ControlCommand};

use super::{ListFormat, parse_list_format};

pub(super) fn parse_agents_args(
    args: Vec<String>,
) -> std::result::Result<(ControlCommand, Option<ListFormat>), String> {
    let mut iter = args.into_iter();
    let subcommand = iter.next().ok_or_else(|| {
        "agents requires a subcommand (list, get, read, wait, or prompt)".to_string()
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
        other => Err(format!(
            "unknown agents subcommand `{other}`; expected list, get, read, wait, or prompt"
        )),
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
                if timeout_ms.replace(parse_timeout(&value)?).is_some() {
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
                if timeout_ms.replace(parse_timeout(&value)?).is_some() {
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

fn parse_timeout(value: &str) -> std::result::Result<u64, String> {
    let (number, multiplier) = if let Some(value) = value.strip_suffix("ms") {
        (value, 1)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60_000)
    } else {
        (value, 1_000)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| "--timeout requires a duration such as 30s, 500ms, or 2m".to_string())
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
    use super::*;

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
}
