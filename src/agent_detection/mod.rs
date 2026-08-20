//! Recognizing an agent behind a pane, and reading what state it is in.
//!
//! Everything the detector knows is a [`definition::AgentDefinition`]: which process is this
//! agent, and what its screen looks like when it is working, blocked, or waiting for you. The
//! agents Rozi ships are written in that same format (`builtin.toml`) and merged with the ones you
//! declare in `config.toml` or install with an extension, so teaching Rozi about a new CLI tool is
//! a table, not a plugin process. See `docs/agents.md`.

mod catalog;
mod definition;
mod process;
mod spec;

pub use catalog::AgentCatalog;
pub use definition::{
    AgentDefinition, AgentDetectionInput, AgentStateOutcome, AgentStateRule, FOOTER_ROWS,
    MatchScope, MatchSource, Pattern,
};
pub use spec::{AgentMatchSpec, AgentOrigin, AgentSpec, AgentStateSpec, build_definitions};

use crate::platform::process::ForegroundJob;
use crate::session::protocol::{AgentIdentity, DetectedAgentState};

use definition::evaluate;
use process::identify_job;

/// What one detection sweep saw.
///
/// Deliberately not [`crate::session::protocol::DetectedAgent`]: that type is the wire shape and
/// has no way to say "recognized the agent, learned nothing about its state". The server resolves
/// a `None` state against the pane's held state before anything leaves the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentObservation {
    pub agent: std::sync::Arc<AgentIdentity>,
    pub state: Option<DetectedAgentState>,
}

pub(crate) fn detect(
    catalog: &AgentCatalog,
    job: Option<&ForegroundJob>,
    screen: &str,
    title: &str,
) -> Option<AgentObservation> {
    let definition = identify_job(catalog, job?)?;
    let state = evaluate(
        definition,
        catalog.base(),
        AgentDetectionInput {
            screen: &screen.to_lowercase(),
            title: &title.to_lowercase(),
        },
    );
    Some(AgentObservation {
        agent: definition.identity.clone(),
        state,
    })
}

/// Whether a Claude Code CLI is on `PATH`, using the same name vocabulary as pane detection.
pub(crate) fn claude_cli_available() -> bool {
    process::agent_on_path(
        &AgentCatalog::builtin(),
        "claude",
        &["claude", "claude-code"],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::process::ForegroundProcess;

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

    fn catalog() -> AgentCatalog {
        AgentCatalog::builtin()
    }

    fn identify(process: &ForegroundProcess) -> Option<(String, bool)> {
        let catalog = catalog();
        process::identify_process(&catalog, process)
            .map(|(definition, wrapped)| (definition.id().to_string(), wrapped))
    }

    fn identify_group(job: &ForegroundJob) -> Option<String> {
        let catalog = catalog();
        identify_job(&catalog, job).map(|definition| definition.id().to_string())
    }

    /// The state the built-in definition for `id` reads out of one screen and title.
    fn detect_state(id: &str, screen: &str, title: &str) -> Option<DetectedAgentState> {
        let catalog = catalog();
        let definition = catalog.by_id(id).expect("built-in agent exists");
        evaluate(
            definition,
            catalog.base(),
            AgentDetectionInput {
                screen: &screen.to_lowercase(),
                title: &title.to_lowercase(),
            },
        )
    }

    #[test]
    fn aliases_cover_the_agent_catalog() {
        let catalog = catalog();
        for (name, id) in [
            ("claude-code", "claude"),
            ("open-code", "opencode"),
            ("opencode-tui", "opencode"),
            ("ghcs", "github-copilot"),
            ("antigravity-cli", "antigravity"),
            ("qoderclicn", "qoder-cli"),
            ("aider-chat", "aider"),
        ] {
            assert_eq!(catalog.by_name(name).map(|agent| agent.id()), Some(id));
        }
        assert!(catalog.by_name("bash").is_none());
    }

    #[test]
    fn identifies_node_python_shell_and_package_wrappers() {
        for (name, argv, expected) in [
            (
                "node",
                vec!["node", "/opt/node_modules/@anthropic-ai/claude-code/cli.js"],
                "claude",
            ),
            ("python3", vec!["python3", "/tools/aider-chat.py"], "aider"),
            (
                "bash",
                vec!["bash", "-c", "opencode --continue"],
                "opencode",
            ),
            ("npx", vec!["npx", "@openai/codex"], "codex"),
        ] {
            assert_eq!(
                identify(&process(10, name, &argv, None)).map(|value| value.0),
                Some(expected.to_string())
            );
        }
    }

    #[test]
    fn explicit_hint_wins_and_nonleader_agent_beats_plain_leader() {
        let hinted = job(
            10,
            vec![process(10, "opaque", &["opaque"], Some("kimi-code"))],
        );
        assert_eq!(identify_group(&hinted), Some("kimi".to_string()));
        let wrapped = job(
            10,
            vec![
                process(10, "bash", &["bash"], None),
                process(11, "node", &["node", "/usr/bin/opencode"], None),
            ],
        );
        assert_eq!(identify_group(&wrapped), Some("opencode".to_string()));
    }

    #[test]
    fn identifies_an_agent_invoked_through_an_unrelated_alias() {
        let mut aliased = process(10, "cl", &["cl"], None);
        aliased.executable = Some("/work/target/release/opencode-tui".into());
        assert_eq!(identify(&aliased), Some(("opencode".to_string(), false)));
    }

    #[test]
    fn detect_reports_the_definitions_public_identity() {
        let catalog = catalog();
        let observed = detect(
            &catalog,
            Some(&job(10, vec![process(10, "claude", &["claude"], None)])),
            "",
            "⠋ Working",
        )
        .expect("claude is detected");
        assert_eq!(observed.agent.id, "claude");
        assert_eq!(observed.agent.label, "Claude Code");
        assert_eq!(observed.state, Some(DetectedAgentState::Working));
    }

    #[test]
    fn screen_states_use_blocked_precedence_and_working_evidence() {
        assert_eq!(
            detect_state("codex", "Do you want to proceed?", "codex ⠹ working"),
            Some(DetectedAgentState::Blocked)
        );
        assert_eq!(
            detect_state("claude", "", "⠋ Working"),
            Some(DetectedAgentState::Working)
        );
        assert_eq!(
            detect_state("opencode", "esc to interrupt", ""),
            Some(DetectedAgentState::Working)
        );
        // OpenCode's actual footer spelling, captured from 1.18.16: no "to".
        assert_eq!(
            detect_state("opencode", "  ■■■⬝⬝⬝⬝⬝  esc interrupt", ""),
            Some(DetectedAgentState::Working)
        );
        assert_eq!(
            detect_state("opencode", "status  ■⬝■⬝■", ""),
            Some(DetectedAgentState::Working)
        );
        for screen in [
            "Choose a target\nType your own answer\n↑↓ select  enter submit  esc dismiss",
            "Select checks (select all that apply)\nenter toggle  esc dismiss",
            "Question 1  Question 2  Confirm\nenter confirm  esc dismiss",
            "Review\nScope: (not answered)\nenter submit  esc dismiss",
        ] {
            assert_eq!(
                detect_state("opencode", screen, ""),
                Some(DetectedAgentState::Blocked),
                "question chrome should be blocked: {screen:?}"
            );
        }
        assert_eq!(
            detect_state("opencode", "enter submit", ""),
            Some(DetectedAgentState::Idle),
            "a generic submit hint is not enough to identify the question dialog"
        );
        assert_eq!(
            detect_state("opencode", "log output quoting `esc dismiss`", ""),
            Some(DetectedAgentState::Idle),
            "a transcript quoting one footer token is not a live question dialog"
        );
        assert_eq!(
            detect_state("goose", "plain screen", ""),
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
            detect_state("claude", plan, "⠋ Investigate agent state"),
            Some(DetectedAgentState::Blocked)
        );
        let edit = "Do you want to make this edit to agent_detection.rs?\n❯ 1. Yes\n  2. No";
        assert_eq!(
            detect_state("claude", edit, "⠋ Investigate agent state"),
            Some(DetectedAgentState::Blocked)
        );
        // Arrowing down the list moves the cursor; the dialog is still waiting.
        assert_eq!(
            detect_state("claude", &plan.replace("❯ 1.", "  1.\n   ❯ 2."), ""),
            Some(DetectedAgentState::Blocked)
        );
        assert_eq!(
            detect_state("opencode", plan, ""),
            Some(DetectedAgentState::Idle),
            "Claude's dialog shape must not classify another agent's screen"
        );
    }

    #[test]
    fn a_claude_pane_that_only_discusses_approvals_keeps_working() {
        // Reduced from a pane that read as blocked while it was working: this detector under
        // review, quoting every question it used to match, with no dialog open. It is also why
        // the built-in `claude` definition sets `base = false` - the shared blocked vocabulary
        // matches this transcript on sight.
        let transcript = "\
● Checking the captured pane against each pattern:

  'do you want to proceed?' -> False
  'would you like to proceed?' -> True
  Ready to code? appears only in the plan dialog, whose first option reads 1. Yes

❯ Do you want to proceed with the commit?

  ⠋ Deciding… (esc to interrupt)";
        assert_eq!(
            detect_state("claude", transcript, "⠋ Fix agent state detection"),
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
            detect_state("claude", transcript, ""),
            Some(DetectedAgentState::Idle),
            "an idle pane that merely writes about interrupt hints is not working"
        );

        let working = transcript.replace(
            "(shift+tab to cycle) · ← 1 agent",
            "(shift+tab to cycle) · esc to interrupt · ← 1 agent",
        );
        assert_eq!(
            detect_state("claude", &working, ""),
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
            detect_state("opencode", subagent, "OC | audit the widget layer"),
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
            detect_state("opencode", parent, "OC | audit the widget layer"),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn a_transcript_mentioning_subagents_is_not_the_subagent_view() {
        let transcript = "\
● The subagent finished. Opening it is `ctrl+x down`, and the navigator
  offers Parent up to return to this session.";
        assert_eq!(
            detect_state("opencode", transcript, ""),
            Some(DetectedAgentState::Idle),
            "prose naming one navigator hint is not the navigator itself"
        );
    }

    /// A pane is read by exactly one definition's rules, never the whole catalog's.
    ///
    /// Detection resolves the foreground process to a single [`AgentDefinition`] first and only
    /// then reads the screen, so declaring more agents costs name comparisons - not another pass
    /// of every screen regex over every pane, at the 250 ms runtime poll rate, per pane.
    mod candidate_gating {
        use super::*;
        use crate::agent_detection::definition::PATTERN_EVALUATIONS;

        /// `count` decoy definitions that all claim to recognize a *blocked* screen. None of them
        /// matches the process under test, so none of their rules may ever run.
        fn decoys(count: usize) -> Vec<AgentDefinition> {
            let mut warnings = Vec::new();
            let specs = (0..count)
                .map(|index| {
                    toml::from_str::<spec::AgentsFile>(&format!(
                        r#"
                        [[agents]]
                        id = "decoy-{index}"
                        match = {{ names = ["decoy-{index}"] }}
                        [[agents.states]]
                        state = "blocked"
                        screen = {{ regex = true, any_of = ["thinking", "^.*decoy.*$", "[a-z]+"] }}
                        [[agents.states]]
                        state = "working"
                        title = {{ any_of = ["anything"] }}
                        "#
                    ))
                    .expect("decoy parses")
                    .agents
                    .remove(0)
                })
                .collect();
            let built = build_definitions(specs, AgentOrigin::Config, &[], &mut warnings);
            assert!(warnings.is_empty(), "{warnings:?}");
            built
        }

        /// Every needle a set of rules owns: the ceiling on what evaluating them can cost.
        fn patterns_in(rules: &[AgentStateRule]) -> usize {
            rules
                .iter()
                .map(|rule| rule.all_of.len() + rule.any_of.len() + rule.none_of.len())
                .sum()
        }

        fn mycoolagent() -> Vec<AgentDefinition> {
            let mut warnings = Vec::new();
            let built = build_definitions(
                toml::from_str::<spec::AgentsFile>(
                    r#"
                    [[agents]]
                    id = "mycoolagent"
                    label = "My Cool Agent"
                    match = { names = ["mca"] }
                    [[agents.states]]
                    state = "working"
                    scope = "footer"
                    screen = { any_of = ["thinking…"] }
                    "#,
                )
                .expect("parses")
                .agents,
                AgentOrigin::Config,
                &[],
                &mut warnings,
            );
            assert!(warnings.is_empty(), "{warnings:?}");
            built
        }

        /// Detect one `mca` pane against a catalog carrying `decoy_count` unrelated definitions,
        /// returning the verdict and how many patterns were evaluated getting there.
        fn detect_with_decoys(decoy_count: usize) -> (AgentObservation, usize) {
            let mut definitions = mycoolagent();
            definitions.extend(decoys(decoy_count));
            let catalog = AgentCatalog::with_definitions(definitions);
            let pane = job(7, vec![process(7, "mca", &["mca"], None)]);

            PATTERN_EVALUATIONS.with(|count| count.set(0));
            let observed = detect(&catalog, Some(&pane), "  thinking…", "a title")
                .expect("the pane's own agent is detected");
            let evaluated = PATTERN_EVALUATIONS.with(|count| count.get());
            (observed, evaluated)
        }

        #[test]
        fn screen_work_does_not_scale_with_how_many_agents_are_declared() {
            let (few, few_evaluations) = detect_with_decoys(5);
            let (many, many_evaluations) = detect_with_decoys(500);

            assert_eq!(few.state, Some(DetectedAgentState::Working));
            assert_eq!(many.state, Some(DetectedAgentState::Working));
            assert_eq!(few.agent.id, "mycoolagent");
            assert_eq!(many.agent.id, "mycoolagent");

            // A threshold-free statement of the property: a hundredfold larger catalog costs the
            // pane exactly the same amount of screen matching.
            assert_eq!(
                few_evaluations, many_evaluations,
                "500 unrelated definitions evaluated {} patterns against this pane, 5 evaluated {}",
                many_evaluations, few_evaluations
            );
            // And the work that did happen fits inside the matched definition's own rules plus the
            // shared base - derived from the catalog rather than hardcoded, so this stays exact if
            // the built-in base vocabulary grows.
            let mut definitions = mycoolagent();
            definitions.extend(decoys(5));
            let catalog = AgentCatalog::with_definitions(definitions);
            let budget = patterns_in(catalog.base())
                + patterns_in(&catalog.by_id("mycoolagent").expect("declared").states);
            assert!(
                few_evaluations <= budget,
                "evaluated {few_evaluations} patterns; only the matched definition's {budget} are reachable"
            );
            assert!(few_evaluations > 0, "the pane's own rules did run");
        }

        #[test]
        fn an_unrelated_definitions_rules_cannot_speak_for_this_pane() {
            // Every decoy would read this screen as blocked. The pane is working, because only the
            // definition its process matched is consulted.
            let (observed, _) = detect_with_decoys(50);
            assert_eq!(observed.state, Some(DetectedAgentState::Working));
            assert_eq!(observed.agent.label, "My Cool Agent");
        }

        /// The complement: a pane whose process matches *nothing* reads no screens at all, rather
        /// than falling through to whichever definition happens to recognize its output.
        #[test]
        fn an_unrecognized_process_evaluates_no_patterns_and_is_not_an_agent() {
            let catalog = AgentCatalog::with_definitions(decoys(50));
            let pane = job(7, vec![process(7, "bash", &["bash"], None)]);

            PATTERN_EVALUATIONS.with(|count| count.set(0));
            let observed = detect(&catalog, Some(&pane), "thinking about decoys", "");
            let evaluated = PATTERN_EVALUATIONS.with(|count| count.get());

            assert!(observed.is_none(), "a shell is not an agent");
            assert_eq!(
                evaluated, 0,
                "no definition matched the process, so no screen work should have happened"
            );
        }
    }

    /// The detectors this format replaced, reimplemented verbatim as an oracle.
    ///
    /// Spot-check tests pin the screens someone thought to write down. This pins the *decision
    /// function*: every screen in the corpus below is read by both the old hand-written logic and
    /// the declarative built-ins, and they must agree everywhere. An edit to `builtin.toml` that
    /// quietly widens or narrows an agent's reading fails here even if no existing assertion
    /// happened to cover the screen it changed.
    mod legacy_equivalence {
        use super::*;

        const INTERRUPT_HINTS: &[&str] = &[
            "esc to interrupt",
            "esc again to interrupt",
            "press esc to interrupt",
            "ctrl+c to interrupt",
            "esc interrupt",
        ];

        fn footer_says(screen: &str, needles: &[&str]) -> bool {
            screen
                .lines()
                .filter(|line| !line.trim().is_empty())
                .rev()
                .take(8)
                .any(|line| needles.iter().any(|needle| line.contains(needle)))
        }

        fn generic_working(screen: &str) -> bool {
            footer_says(screen, INTERRUPT_HINTS)
        }

        fn generic_blocked(screen: &str, title: &str) -> bool {
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
        }

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

        fn opencode_question_dialog(screen: &str) -> bool {
            screen.contains("esc dismiss")
                && ["enter submit", "enter toggle", "enter confirm"]
                    .iter()
                    .any(|hint| screen.contains(hint))
        }

        fn opencode_progress_bar(screen: &str) -> bool {
            screen
                .chars()
                .fold((0usize, false), |(run, found), ch| {
                    let run = if matches!(ch, '■' | '⬝') {
                        run + 1
                    } else {
                        0
                    };
                    (run, found || run >= 4)
                })
                .1
        }

        fn opencode_subagent_view(screen: &str) -> bool {
            screen.contains("parent up")
                && (screen.contains("prev left") || screen.contains("next right"))
        }

        /// `detect_state` as it was written before definitions, lowercased inputs and all.
        fn legacy_detect_state(id: &str, screen: &str, title: &str) -> Option<DetectedAgentState> {
            let screen = screen.to_lowercase();
            let title = title.to_lowercase();
            match id {
                "claude" => {
                    if claude_choice_dialog(&screen) {
                        return Some(DetectedAgentState::Blocked);
                    }
                    let title_spinner = title
                        .chars()
                        .next()
                        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch));
                    if title_spinner || generic_working(&screen) {
                        Some(DetectedAgentState::Working)
                    } else {
                        Some(DetectedAgentState::Idle)
                    }
                }
                "opencode" => {
                    if generic_blocked(&screen, &title) || opencode_question_dialog(&screen) {
                        return Some(DetectedAgentState::Blocked);
                    }
                    if generic_working(&screen) || opencode_progress_bar(&screen) {
                        return Some(DetectedAgentState::Working);
                    }
                    if opencode_subagent_view(&screen) {
                        return None;
                    }
                    Some(DetectedAgentState::Idle)
                }
                // Codex and every catalog-only agent shared the generic detector.
                _ => {
                    if generic_blocked(&screen, &title) {
                        Some(DetectedAgentState::Blocked)
                    } else if generic_working(&screen) {
                        Some(DetectedAgentState::Working)
                    } else {
                        Some(DetectedAgentState::Idle)
                    }
                }
            }
        }

        /// Screens chosen so every branch of the old logic fires, and so the branches that
        /// *disagree* between agents are represented: approval prose (blocked for the generic
        /// detector, deliberately not for Claude), the numbered-option cursor, footer versus
        /// transcript placement of an interrupt hint, and the subagent navigator.
        fn corpus() -> Vec<(&'static str, &'static str)> {
            vec![
                ("", ""),
                ("plain screen with nothing on it", ""),
                ("Do you want to proceed?", ""),
                ("permission required", ""),
                ("waiting for permission", "codex ⠹ working"),
                ("Allow command?", ""),
                ("continue? [y/n]", ""),
                ("Yes (y) / No (n)", ""),
                ("nothing here", "action required"),
                ("esc to interrupt", ""),
                ("  ■■■⬝⬝⬝⬝⬝  esc interrupt", ""),
                ("status  ■⬝■⬝■", ""),
                ("press esc to interrupt", ""),
                ("ctrl+c to interrupt", ""),
                ("esc again to interrupt", ""),
                ("", "⠋ Working"),
                ("", "⠹ thinking"),
                ("❯ 1. Yes, and bypass permissions\n  2. No", ""),
                ("  ❯ 2. Yes, manually approve edits", "⠋ Investigate"),
                ("❯ Do you want to proceed with the commit?", "⠋ Fix it"),
                ("❯ 1.no space after the dot", ""),
                (
                    "Choose a target\nType your own answer\n↑↓ select  enter submit  esc dismiss",
                    "",
                ),
                ("Select checks\nenter toggle  esc dismiss", ""),
                ("Question 1  Confirm\nenter confirm  esc dismiss", ""),
                ("enter submit", ""),
                ("log output quoting `esc dismiss`", ""),
                (
                    "  ┃  Subagent (1 of 1) 100 (0%)   Parent up  Prev left  Next right",
                    "OC | audit",
                ),
                ("The navigator offers Parent up to return.", ""),
                (
                    "quoting esc to interrupt high up\nline\nline\nline\nline\nline\nline\nline\nline\ntail",
                    "",
                ),
                (
                    "  ⏵⏵ bypass permissions on (shift+tab to cycle) · esc to interrupt · ← 1 agent",
                    "",
                ),
            ]
        }

        #[test]
        fn declarative_builtins_read_every_screen_the_way_the_old_detectors_did() {
            let catalog = catalog();
            // Every agent that had a named detector, plus one that only ever had the generic one.
            for id in ["claude", "opencode", "codex", "goose"] {
                for (screen, title) in corpus() {
                    assert_eq!(
                        detect_state(id, screen, title),
                        legacy_detect_state(id, screen, title),
                        "{id} disagrees on screen {screen:?} title {title:?}"
                    );
                }
                assert!(catalog.by_id(id).is_some(), "{id} is in the catalog");
            }
        }

        /// The same equivalence over process identity: every name, alias, and package marker the
        /// old `kind_from_name` / `kind_from_path` tables carried still resolves to the agent it
        /// used to, through whatever launcher wraps it.
        #[test]
        fn declarative_builtins_identify_every_process_the_old_catalog_did() {
            let catalog = catalog();
            for (name, id) in [
                ("pi", "pi"),
                ("pi-coding-agent", "pi"),
                ("claude", "claude"),
                ("claude-code", "claude"),
                ("codex", "codex"),
                ("codex-cli", "codex"),
                ("gemini", "gemini"),
                ("gemini-cli", "gemini"),
                ("cursor", "cursor"),
                ("cursor-agent", "cursor"),
                ("devin", "devin"),
                ("devin-cli", "devin"),
                ("agy", "antigravity"),
                ("antigravity", "antigravity"),
                ("antigravity-cli", "antigravity"),
                ("cline", "cline"),
                ("omp", "omp"),
                ("mastracode", "mastracode"),
                ("mastra-code", "mastracode"),
                ("opencode", "opencode"),
                ("open-code", "opencode"),
                ("opencode-tui", "opencode"),
                ("copilot", "github-copilot"),
                ("github-copilot", "github-copilot"),
                ("ghcs", "github-copilot"),
                ("kimi", "kimi"),
                ("kimi-code", "kimi"),
                ("kiro", "kiro"),
                ("kiro-cli", "kiro"),
                ("droid", "droid"),
                ("amp", "amp"),
                ("amp-local", "amp"),
                ("grok", "grok"),
                ("grok-build", "grok"),
                ("hermes", "hermes"),
                ("hermes-agent", "hermes"),
                ("kilo", "kilo"),
                ("kilo-code", "kilo"),
                ("qoder", "qoder-cli"),
                ("qodercn", "qoder-cli"),
                ("qodercli", "qoder-cli"),
                ("qoderclicn", "qoder-cli"),
                ("maki", "maki"),
                ("aider", "aider"),
                ("aider-chat", "aider"),
                ("goose", "goose"),
                ("goose-cli", "goose"),
            ] {
                assert_eq!(
                    catalog.by_name(name).map(|agent| agent.id()),
                    Some(id),
                    "bare name {name}"
                );
                // The suffix stripping the old `normalized_name` did, still applied.
                for suffix in [".exe", ".cmd", ".js", ".py"] {
                    assert_eq!(
                        catalog
                            .by_name(&format!("/opt/bin/{name}{suffix}"))
                            .map(|agent| agent.id()),
                        Some(id),
                        "{name}{suffix} behind a path"
                    );
                }
            }
            for (token, id) in [
                ("@anthropic-ai/claude-code", "claude"),
                ("@openai/codex", "codex"),
                ("opencode-ai", "opencode"),
                ("/opencode/", "opencode"),
                ("@google/gemini-cli", "gemini"),
                ("@github/copilot", "github-copilot"),
                ("pi-coding-agent", "pi"),
            ] {
                assert_eq!(
                    catalog
                        .by_path(&format!("/lib/node_modules/{token}/cli.js"))
                        .map(|agent| agent.id()),
                    Some(id),
                    "package marker {token}"
                );
            }
            for name in ["bash", "vim", "cargo", "ssh", "top"] {
                assert!(catalog.by_name(name).is_none(), "{name} is not an agent");
            }
        }
    }

    #[test]
    fn a_user_definition_detects_a_tool_rozi_ships_nothing_for() {
        let mut warnings = Vec::new();
        let user = build_definitions(
            toml::from_str::<spec::AgentsFile>(
                r#"
                [[agents]]
                id = "mycoolagent"
                label = "My Cool Agent"
                match = { names = ["mca"], paths = ["@acme/mca"] }

                [[agents.states]]
                state = "blocked"
                screen = { all_of = ["esc dismiss"], any_of = ["enter submit"] }

                [[agents.states]]
                state = "working"
                scope = "footer"
                screen = { any_of = ["thinking…"] }
                "#,
            )
            .expect("parses")
            .agents,
            AgentOrigin::Config,
            &[],
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let catalog = AgentCatalog::with_definitions(user);

        let observed = detect(
            &catalog,
            Some(&job(
                7,
                vec![process(7, "node", &["node", "/lib/@acme/mca/cli.js"], None)],
            )),
            "  thinking…",
            "",
        )
        .expect("the user's agent is detected through a node wrapper");
        assert_eq!(observed.agent.id, "mycoolagent");
        assert_eq!(observed.agent.label, "My Cool Agent");
        assert_eq!(observed.state, Some(DetectedAgentState::Working));

        // The shared base vocabulary applies to a definition that did not opt out.
        assert_eq!(
            detect(
                &catalog,
                Some(&job(7, vec![process(7, "mca", &["mca"], None)])),
                "continue? [y/n]",
                "",
            )
            .expect("detected")
            .state,
            Some(DetectedAgentState::Blocked)
        );
    }
}
