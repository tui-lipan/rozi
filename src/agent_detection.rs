use crate::platform::process::{ForegroundJob, ForegroundProcess};
use crate::session::protocol::{AgentKind, DetectedAgentState};

/// What one detection sweep saw.
///
/// Deliberately not [`crate::session::protocol::DetectedAgent`]: that type is the wire shape and
/// has no way to say "recognized the agent, learned nothing about its state". The server resolves
/// a `None` state against the pane's held state before anything leaves the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentObservation {
    pub kind: AgentKind,
    pub state: Option<DetectedAgentState>,
}

pub(crate) fn detect(
    job: Option<&ForegroundJob>,
    screen: &str,
    title: &str,
) -> Option<AgentObservation> {
    let kind = identify_job(job?)?;
    Some(AgentObservation {
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

/// The dialog drawn on screen is the only evidence that a Claude Code run stopped for an answer:
/// it reports nothing through its title (2.1.227 emits no `OSC 0` at all, spinner or otherwise).
///
/// Match its *structure* - the selection cursor sitting on a numbered choice - rather than the
/// question above it. Those questions are ordinary English that varies per dialog ("do you want to
/// proceed?" for a command, "would you like to proceed?" under a written plan, the file's own name
/// for an edit), and worse, any pane can have them on screen without a dialog being open: a
/// transcript that discusses approvals reads as blocked under prose matching. A cursored numbered
/// option is drawn only while a dialog is actually waiting.
fn claude_choice_dialog(screen: &str) -> bool {
    screen.lines().any(|line| {
        let Some(option) = line.trim_start().strip_prefix('❯') else {
            return false;
        };
        let option = option.trim_start();
        let index: String = option.chars().take_while(char::is_ascii_digit).collect();
        !index.is_empty() && option[index.len()..].starts_with(". ")
    })
}

/// Screen text that means a run is in flight.
///
/// `esc interrupt` is OpenCode's current spelling; the longer forms belong to Claude Code and
/// Codex. OpenCode's is not a prefix of any other, so matching it costs nothing here.
const INTERRUPT_HINTS: &[&str] = &[
    "esc to interrupt",
    "esc again to interrupt",
    "press esc to interrupt",
    "ctrl+c to interrupt",
    "esc interrupt",
];

/// How far above the last written row an agent's status chrome can sit.
///
/// Claude Code's shortcut row is the final row and its spinner line rides just above the composer;
/// OpenCode's shortcut row sits above its model and cwd rows. Eight covers both with room to spare
/// while staying far from the transcript, which is the point: an interrupt hint is chrome, and
/// chrome is drawn at the bottom of the screen. Found the hard way - an idle pane whose transcript
/// discussed these very hints, 34 rows up, read as a run in flight.
const FOOTER_ROWS: usize = 8;

/// Whether the agent's *footer* says one of `needles`, as opposed to its transcript quoting it.
fn footer_says(screen: &str, needles: &[&str]) -> bool {
    screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(FOOTER_ROWS)
        .any(|line| needles.iter().any(|needle| line.contains(needle)))
}

/// OpenCode's subagent view, which replaces the composer and the status line - the only places a
/// run's progress bar and interrupt hint are ever drawn - with a child-session navigator.
///
/// The parent's run continues underneath it. This screen simply cannot report on it, which is a
/// different fact from seeing an idle prompt, and conflating the two turned every visit to a
/// subagent into a finished run: the run clock restarted and a completion alert fired for work
/// that never stopped. Match the navigator's own hint row rather than the heading, so a transcript
/// that merely mentions a subagent is not mistaken for the view itself.
fn opencode_subagent_view(screen: &str) -> bool {
    screen.contains("parent up") && (screen.contains("prev left") || screen.contains("next right"))
}

/// The agent state a pane's screen provides evidence for.
///
/// `None` means the agent was recognized but the screen said nothing either way - it is *not*
/// "idle". Only a positively observed prompt is idle. The server holds the previous state across
/// a `None` (see `resolve_detected_agent`), which is what lets a run survive the user opening a
/// different view inside the agent.
fn detect_state(kind: AgentKind, screen: &str, title: &str) -> Option<DetectedAgentState> {
    let screen = screen.to_lowercase();
    let title = title.to_lowercase();
    // Claude Code has an exact structural signature, so it decides on that alone. The loose prose
    // patterns are the fallback for agents whose prompts we cannot recognize by shape; on a Claude
    // pane they only add false positives, since "do you want to proceed?" is Claude's own wording
    // and reads as blocked from any transcript that merely quotes it.
    let blocked = if kind == AgentKind::Claude {
        claude_choice_dialog(&screen)
    } else {
        [
            "permission required",
            "action required",
            "do you want to proceed?",
            "waiting for permission",
            "allow command?",
            "[y/n]",
            "yes (y)",
        ]
        .iter()
        .any(|pattern| screen.contains(pattern))
            || title.contains("action required")
            // Current OpenCode question prompts use this footer in every state: single choice,
            // multi-select, custom answer, and review. Scope it to OpenCode so another program's
            // ordinary dismiss hint cannot masquerade as an agent waiting for input.
            || (kind == AgentKind::OpenCode
                && screen.contains("esc dismiss")
                && ["enter submit", "enter toggle", "enter confirm"]
                    .iter()
                    .any(|hint| screen.contains(hint)))
    };
    if blocked {
        return Some(DetectedAgentState::Blocked);
    }

    let interrupt_hint = footer_says(&screen, INTERRUPT_HINTS);
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
        AgentKind::OpenCode => opencode_progress,
        _ => false,
    };
    if interrupt_hint || agent_working {
        return Some(DetectedAgentState::Working);
    }
    if kind == AgentKind::OpenCode && opencode_subagent_view(&screen) {
        return None;
    }
    Some(DetectedAgentState::Idle)
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
            detect_state(
                AgentKind::Codex,
                "Do you want to proceed?",
                "codex ⠹ working"
            ),
            Some(DetectedAgentState::Blocked)
        );
        assert_eq!(
            detect_state(AgentKind::Claude, "", "⠋ Working"),
            Some(DetectedAgentState::Working)
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "esc to interrupt", ""),
            Some(DetectedAgentState::Working)
        );
        // OpenCode's actual footer spelling, captured from 1.18.16: no "to".
        assert_eq!(
            detect_state(AgentKind::OpenCode, "  ■■■⬝⬝⬝⬝⬝  esc interrupt", ""),
            Some(DetectedAgentState::Working)
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "status  ■⬝■⬝■", ""),
            Some(DetectedAgentState::Working)
        );
        for screen in [
            "Choose a target\nType your own answer\n↑↓ select  enter submit  esc dismiss",
            "Select checks (select all that apply)\nenter toggle  esc dismiss",
            "Question 1  Question 2  Confirm\nenter confirm  esc dismiss",
            "Review\nScope: (not answered)\nenter submit  esc dismiss",
        ] {
            assert_eq!(
                detect_state(AgentKind::OpenCode, screen, ""),
                Some(DetectedAgentState::Blocked),
                "question chrome should be blocked: {screen:?}"
            );
        }
        assert_eq!(
            detect_state(AgentKind::OpenCode, "enter submit", ""),
            Some(DetectedAgentState::Idle),
            "a generic submit hint is not enough to identify the question dialog"
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, "log output quoting `esc dismiss`", ""),
            Some(DetectedAgentState::Idle),
            "a transcript quoting one footer token is not a live question dialog"
        );
        assert_eq!(
            detect_state(AgentKind::Goose, "plain screen", ""),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn claude_approval_dialogs_block_over_any_working_evidence() {
        // Captured from the plan dialog, which is what a finished plan-mode run stops on.
        let plan = "\
   Ready to code?

   Here is Claude's plan:
   ...
   Claude has written up a plan and is ready to execute. Would you like to proceed?

   ❯ 1. Yes, and bypass permissions
     2. Yes, manually approve edits
     3. Tell Claude what to change";
        assert_eq!(
            detect_state(AgentKind::Claude, plan, "⠋ Investigate agent state"),
            Some(DetectedAgentState::Blocked)
        );
        let edit = "Do you want to make this edit to agent_detection.rs?\n❯ 1. Yes\n  2. No";
        assert_eq!(
            detect_state(AgentKind::Claude, edit, "⠋ Investigate agent state"),
            Some(DetectedAgentState::Blocked)
        );
        // Arrowing down the list moves the cursor; the dialog is still waiting.
        assert_eq!(
            detect_state(
                AgentKind::Claude,
                &plan.replace("❯ 1.", "  1.\n   ❯ 2."),
                ""
            ),
            Some(DetectedAgentState::Blocked)
        );
        assert_eq!(
            detect_state(AgentKind::OpenCode, plan, ""),
            Some(DetectedAgentState::Idle),
            "Claude's dialog shape must not classify another agent's screen"
        );
    }

    #[test]
    fn a_claude_pane_that_only_discusses_approvals_keeps_working() {
        // Reduced from a pane that read as blocked while it was working: this detector under
        // review, quoting every question it used to match, with no dialog open.
        let transcript = "\
● Checking the captured pane against each pattern:

  'do you want to proceed?' -> False
  'would you like to proceed?' -> True
  Ready to code? appears only in the plan dialog, whose first option reads 1. Yes

❯ Do you want to proceed with the commit?

  ⠋ Deciding… (esc to interrupt)";
        assert_eq!(
            detect_state(AgentKind::Claude, transcript, "⠋ Fix agent state detection"),
            Some(DetectedAgentState::Working)
        );
    }

    /// Reduced from two live panes: an idle Claude Code pane that read as a run in flight because
    /// its transcript discussed interrupt hints, and the working pane beside it whose footer was
    /// the real thing. The two screens differ only in where the hint sits.
    #[test]
    fn an_interrupt_hint_counts_only_in_the_footer() {
        let transcript = "\
     OpenCode's footer reads esc interrupt, not esc to interrupt. None of the
     existing hints matched it, so detection was riding on the progress bar alone.

     The dead code

     title_spinner matched a braille spinner in the pane title for OpenCode and
     Codex. Neither ever emits one, so the branch could not fire.

     What I changed

     detect_state now returns Option<DetectedAgentState>, where None means the
     agent was recognized but the screen said nothing either way.

     Verify this in a pane before you rely on it.
   ✻ Cogitated for 1h 41m 48s
   ──────────────────────────────────────────────────────────────────────────────
   ❯
   ──────────────────────────────────────────────────────────────────────────────
     ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 1 agent";
        assert_eq!(
            detect_state(AgentKind::Claude, transcript, ""),
            Some(DetectedAgentState::Idle),
            "an idle pane that merely writes about interrupt hints is not working"
        );

        let working = transcript.replace(
            "(shift+tab to cycle) · ← 1 agent",
            "(shift+tab to cycle) · esc to interrupt · ← 1 agent",
        );
        assert_eq!(
            detect_state(AgentKind::Claude, &working, ""),
            Some(DetectedAgentState::Working),
            "the same hint in the footer is the live one"
        );
    }

    #[test]
    fn opencodes_subagent_view_reports_no_evidence_rather_than_idle() {
        // Captured from opencode 1.18.16 (`ctrl+x down` from a session). The composer and the
        // status line - the only places the run's progress bar and interrupt hint are drawn - are
        // both replaced, so nothing on this screen speaks for the parent run still in flight.
        let subagent = "\
     ✓ Researcher Task — Audit src/widgets for unused exports
       ↳ 2 toolcalls · 12.0s

  ┃
  ┃  Subagent (1 of 1) 100 (0%)                     Parent up  Prev left  Next right
  ┃";
        assert_eq!(
            detect_state(AgentKind::OpenCode, subagent, "OC | audit the widget layer"),
            None,
            "a subagent view knows nothing about the parent run and must not read as idle"
        );

        // The parent view of the same session, idle: composer and status line are back, so the
        // absence of working evidence is now a real observation.
        let parent = "\
  ┃
  ┃  Coder · GPT-4o OpenAI
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /mock/project                                    tab agents  ctrl+p commands";
        assert_eq!(
            detect_state(AgentKind::OpenCode, parent, "OC | audit the widget layer"),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn a_transcript_mentioning_subagents_is_not_the_subagent_view() {
        let transcript = "\
● The subagent finished. Opening it is `ctrl+x down`, and the navigator
  offers Parent up to return to this session.";
        assert_eq!(
            detect_state(AgentKind::OpenCode, transcript, ""),
            Some(DetectedAgentState::Idle),
            "prose naming one navigator hint is not the navigator itself"
        );
    }
}
