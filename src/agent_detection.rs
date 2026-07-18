use crate::platform::process::{ForegroundJob, ForegroundProcess};
use crate::session::protocol::{AgentKind, DetectedAgent, DetectedAgentState};

pub(crate) fn detect(
    job: Option<&ForegroundJob>,
    screen: &str,
    title: &str,
) -> Option<DetectedAgent> {
    let kind = identify_job(job?)?;
    Some(DetectedAgent {
        kind,
        state: detect_state(kind, screen, title),
    })
}

fn identify_job(job: &ForegroundJob) -> Option<AgentKind> {
    for process in &job.processes {
        if let Some(kind) = process.agent_hint.as_deref().and_then(parse_agent) {
            return Some(kind);
        }
    }

    job.processes
        .iter()
        .filter_map(|process| {
            let (kind, wrapped) = identify_process(process)?;
            let leader = process.pid == job.process_group_id;
            Some(((leader as u8) * 2 + wrapped as u8, kind))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, kind)| kind)
}

fn identify_process(process: &ForegroundProcess) -> Option<(AgentKind, bool)> {
    if let Some(kind) = process
        .executable
        .as_deref()
        .and_then(|path| parse_agent(path_basename(path)))
    {
        return Some((kind, false));
    }
    let argv0 = process
        .argv
        .first()
        .map(String::as_str)
        .unwrap_or(&process.name);
    if let Some(kind) = parse_agent(path_basename(argv0)).or_else(|| parse_agent(&process.name)) {
        return Some((kind, false));
    }
    identify_wrapped(&process.argv).map(|kind| (kind, true))
}

fn identify_wrapped(argv: &[String]) -> Option<AgentKind> {
    let runtime = argv
        .first()
        .map(|value| normalized_name(path_basename(value)))?;
    let mut candidates: Vec<&str> = Vec::new();
    match runtime.as_str() {
        "node" | "nodejs" | "bun" | "deno" | "python" | "python3" | "ruby" => {
            candidates.extend(
                argv.iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with('-'))
                    .map(String::as_str),
            );
        }
        "sh" | "bash" | "zsh" | "fish" | "cmd" | "powershell" | "pwsh" => {
            for command in argv.iter().skip(1).filter(|arg| !arg.starts_with('-')) {
                candidates.extend(command.split_whitespace());
            }
        }
        "env" | "npm" | "npx" | "pnpm" | "pnpx" | "yarn" | "bunx" | "uv" | "uvx" => {
            candidates.extend(
                argv.iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with('-'))
                    .map(String::as_str),
            );
        }
        _ => candidates.extend(argv.iter().map(String::as_str)),
    }
    candidates.into_iter().find_map(agent_from_path)
}

fn agent_from_path(value: &str) -> Option<AgentKind> {
    let token = value.trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | '&'));
    parse_agent(path_basename(token)).or_else(|| {
        let normalized = token.replace('\\', "/").to_ascii_lowercase();
        [
            ("@anthropic-ai/claude-code", AgentKind::Claude),
            ("@openai/codex", AgentKind::Codex),
            ("opencode-ai", AgentKind::OpenCode),
            ("/opencode/", AgentKind::OpenCode),
            ("@google/gemini-cli", AgentKind::Gemini),
            ("@github/copilot", AgentKind::GithubCopilot),
            ("pi-coding-agent", AgentKind::Pi),
        ]
        .into_iter()
        .find_map(|(needle, kind)| normalized.contains(needle).then_some(kind))
    })
}

fn parse_agent(value: &str) -> Option<AgentKind> {
    match normalized_name(path_basename(value)).as_str() {
        "pi" | "pi-coding-agent" => Some(AgentKind::Pi),
        "claude" | "claude-code" => Some(AgentKind::Claude),
        "codex" | "codex-cli" => Some(AgentKind::Codex),
        "gemini" | "gemini-cli" => Some(AgentKind::Gemini),
        "cursor" | "cursor-agent" => Some(AgentKind::Cursor),
        "devin" | "devin-cli" => Some(AgentKind::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(AgentKind::Antigravity),
        "cline" => Some(AgentKind::Cline),
        "omp" => Some(AgentKind::Omp),
        "mastracode" | "mastra-code" => Some(AgentKind::Mastracode),
        "opencode" | "open-code" | "opencode-tui" => Some(AgentKind::OpenCode),
        "copilot" | "github-copilot" | "ghcs" => Some(AgentKind::GithubCopilot),
        "kimi" | "kimi-code" => Some(AgentKind::Kimi),
        "kiro" | "kiro-cli" => Some(AgentKind::Kiro),
        "droid" => Some(AgentKind::Droid),
        "amp" | "amp-local" => Some(AgentKind::Amp),
        "grok" | "grok-build" => Some(AgentKind::Grok),
        "hermes" | "hermes-agent" => Some(AgentKind::Hermes),
        "kilo" | "kilo-code" => Some(AgentKind::Kilo),
        "qoder" | "qodercn" | "qodercli" | "qoderclicn" => Some(AgentKind::QoderCli),
        "maki" => Some(AgentKind::Maki),
        "aider" | "aider-chat" => Some(AgentKind::Aider),
        "goose" | "goose-cli" => Some(AgentKind::Goose),
        _ => None,
    }
}

fn normalized_name(value: &str) -> String {
    let mut value = value.trim().to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".py"] {
        if value.ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    value
}

fn path_basename(value: &str) -> &str {
    value
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
}

fn detect_state(kind: AgentKind, screen: &str, title: &str) -> DetectedAgentState {
    let screen = screen.to_lowercase();
    let title = title.to_lowercase();
    let blocked = [
        "permission required",
        "action required",
        "do you want to proceed?",
        "waiting for permission",
        "allow command?",
        "enter to submit answer",
        "enter to submit all",
        "enter to confirm or esc to cancel",
        "[y/n]",
        "yes (y)",
        "review your answers",
    ]
    .iter()
    .any(|pattern| screen.contains(pattern))
        || title.contains("action required")
        || (screen.contains("esc to cancel")
            && (screen.contains("enter to select") || screen.contains("enter confirm")));
    if blocked {
        return DetectedAgentState::Blocked;
    }

    let interrupt_hint = [
        "esc to interrupt",
        "esc again to interrupt",
        "press esc to interrupt",
        "ctrl+c to interrupt",
    ]
    .iter()
    .any(|pattern| screen.contains(pattern));
    let title_spinner = title.chars().any(|ch| {
        matches!(
            ch,
            '\u{280b}'
                | '\u{2819}'
                | '\u{2839}'
                | '\u{2838}'
                | '\u{283c}'
                | '\u{2834}'
                | '\u{2826}'
                | '\u{2827}'
                | '\u{2807}'
                | '\u{280f}'
        )
    });
    let opencode_progress = screen
        .chars()
        .fold((0usize, false), |(run, found), ch| {
            let run = if matches!(ch, '■' | '⬝') {
                run + 1
            } else {
                0
            };
            (run, found || run >= 4)
        })
        .1;
    let agent_working = match kind {
        AgentKind::Claude => title
            .chars()
            .next()
            .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch)),
        AgentKind::OpenCode => interrupt_hint || opencode_progress || title_spinner,
        AgentKind::Codex => title_spinner,
        _ => false,
    };
    if interrupt_hint || agent_working {
        DetectedAgentState::Working
    } else {
        DetectedAgentState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, argv: &[&str], hint: Option<&str>) -> ForegroundProcess {
        ForegroundProcess {
            pid,
            name: name.into(),
            executable: None,
            argv: argv.iter().map(|value| (*value).into()).collect(),
            agent_hint: hint.map(str::to_string),
        }
    }

    fn job(process_group_id: u32, processes: Vec<ForegroundProcess>) -> ForegroundJob {
        ForegroundJob {
            process_group_id,
            processes,
        }
    }

    #[test]
    fn aliases_cover_the_agent_catalog() {
        assert_eq!(parse_agent("claude-code"), Some(AgentKind::Claude));
        assert_eq!(parse_agent("open-code"), Some(AgentKind::OpenCode));
        assert_eq!(parse_agent("opencode-tui"), Some(AgentKind::OpenCode));
        assert_eq!(parse_agent("ghcs"), Some(AgentKind::GithubCopilot));
        assert_eq!(parse_agent("antigravity-cli"), Some(AgentKind::Antigravity));
        assert_eq!(parse_agent("qoderclicn"), Some(AgentKind::QoderCli));
        assert_eq!(parse_agent("aider-chat"), Some(AgentKind::Aider));
        assert_eq!(parse_agent("bash"), None);
    }

    #[test]
    fn identifies_node_python_shell_and_package_wrappers() {
        for (name, argv, expected) in [
            (
                "node",
                vec!["node", "/opt/node_modules/@anthropic-ai/claude-code/cli.js"],
                AgentKind::Claude,
            ),
            (
                "python3",
                vec!["python3", "/tools/aider-chat.py"],
                AgentKind::Aider,
            ),
            (
                "bash",
                vec!["bash", "-c", "opencode --continue"],
                AgentKind::OpenCode,
            ),
            ("npx", vec!["npx", "@openai/codex"], AgentKind::Codex),
        ] {
            assert_eq!(
                identify_process(&process(10, name, &argv, None)).map(|value| value.0),
                Some(expected)
            );
        }
    }

    #[test]
    fn explicit_hint_wins_and_nonleader_agent_beats_plain_leader() {
        let hinted = job(
            10,
            vec![process(10, "opaque", &["opaque"], Some("kimi-code"))],
        );
        assert_eq!(identify_job(&hinted), Some(AgentKind::Kimi));
        let wrapped = job(
            10,
            vec![
                process(10, "bash", &["bash"], None),
                process(11, "node", &["node", "/usr/bin/opencode"], None),
            ],
        );
        assert_eq!(identify_job(&wrapped), Some(AgentKind::OpenCode));
    }

    #[test]
    fn identifies_an_agent_invoked_through_an_unrelated_alias() {
        let mut aliased = process(10, "cl", &["cl"], None);
        aliased.executable = Some("/work/target/release/opencode-tui".into());
        assert_eq!(
            identify_process(&aliased),
            Some((AgentKind::OpenCode, false))
        );
    }

    #[test]
    fn screen_states_use_blocked_precedence_and_working_evidence() {
        assert_eq!(
            detect_state(AgentKind::Claude, "Do you want to proceed?", "⠋ Working"),
            DetectedAgentState::Blocked
        );
        assert_eq!(
            detect_state(AgentKind::Claude, "", "⠋ Working"),
            DetectedAgentState::Working
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "esc to interrupt", ""),
            DetectedAgentState::Working
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "status  ■⬝■⬝■", ""),
            DetectedAgentState::Working
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "", "⠹ OpenCode"),
            DetectedAgentState::Working
        );
        assert_eq!(
            detect_state(AgentKind::Codex, "", "codex ⠹ working"),
            DetectedAgentState::Working
        );
        assert_eq!(
            detect_state(AgentKind::Goose, "plain screen", ""),
            DetectedAgentState::Idle
        );
    }
}
