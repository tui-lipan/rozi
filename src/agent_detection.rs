mod process;
mod providers;

use crate::platform::process::ForegroundJob;
#[cfg(test)]
use crate::platform::process::ForegroundProcess;
use crate::session::protocol::{AgentKind, DetectedAgentState};

use process::identify_job;

#[derive(Clone, Copy)]
struct AgentDetectionInput<'a> {
    screen: &'a str,
    title: &'a str,
}

/// One provider's translation from generic pane observations into the shared agent-status
/// vocabulary. Providers remain in core because their evidence includes privileged screen and
/// process state that the public extension surface intentionally does not expose.
trait AgentDetector: Sync {
    fn detect(&self, input: AgentDetectionInput<'_>) -> Option<DetectedAgentState>;
}

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

#[cfg(test)]
fn identify_process(
    process: &crate::platform::process::ForegroundProcess,
) -> Option<(AgentKind, bool)> {
    process::identify_process(process)
}

#[cfg(test)]
fn parse_agent(value: &str) -> Option<AgentKind> {
    providers::catalog::kind_from_name(value)
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
    providers::detector(kind).detect(AgentDetectionInput {
        screen: &screen,
        title: &title,
    })
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
