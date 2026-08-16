use crate::session::protocol::AgentKind;

/// Provider identity vocabulary for agents that need no custom screen-state detector.
pub(in crate::agent_detection) fn kind_from_name(value: &str) -> Option<AgentKind> {
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

pub(in crate::agent_detection) fn kind_from_path(value: &str) -> Option<AgentKind> {
    let token = value.trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | '&'));
    kind_from_name(path_basename(token)).or_else(|| {
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

pub(in crate::agent_detection) fn normalized_name(value: &str) -> String {
    let mut value = value.trim().to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".py"] {
        if value.ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    value
}

pub(in crate::agent_detection) fn path_basename(value: &str) -> &str {
    value
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
}
