use crate::session::protocol::DetectedAgentState;

use super::super::{AgentDetectionInput, AgentDetector};

pub(super) struct GenericDetector;

impl AgentDetector for GenericDetector {
    fn detect(&self, input: AgentDetectionInput<'_>) -> Option<DetectedAgentState> {
        if blocked(input) {
            Some(DetectedAgentState::Blocked)
        } else if working(input.screen) {
            Some(DetectedAgentState::Working)
        } else {
            Some(DetectedAgentState::Idle)
        }
    }
}

pub(super) fn blocked(input: AgentDetectionInput<'_>) -> bool {
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
    .any(|pattern| input.screen.contains(pattern))
        || input.title.contains("action required")
}

/// Screen text that means a run is in flight.
const INTERRUPT_HINTS: &[&str] = &[
    "esc to interrupt",
    "esc again to interrupt",
    "press esc to interrupt",
    "ctrl+c to interrupt",
    "esc interrupt",
];

/// How far above the last written row an agent's status chrome can sit.
const FOOTER_ROWS: usize = 8;

/// Whether the agent's footer says one of `needles`, as opposed to its transcript quoting it.
fn footer_says(screen: &str, needles: &[&str]) -> bool {
    screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(FOOTER_ROWS)
        .any(|line| needles.iter().any(|needle| line.contains(needle)))
}

pub(super) fn working(screen: &str) -> bool {
    footer_says(screen, INTERRUPT_HINTS)
}
