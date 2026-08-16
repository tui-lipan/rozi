use crate::session::protocol::DetectedAgentState;

use super::super::{AgentDetectionInput, AgentDetector};
use super::generic;

pub(super) struct OpenCodeDetector;

impl AgentDetector for OpenCodeDetector {
    fn detect(&self, input: AgentDetectionInput<'_>) -> Option<DetectedAgentState> {
        if generic::blocked(input) || question_dialog(input.screen) {
            return Some(DetectedAgentState::Blocked);
        }
        if generic::working(input.screen) || progress_bar(input.screen) {
            return Some(DetectedAgentState::Working);
        }
        if subagent_view(input.screen) {
            // The child-session navigator replaces every place the parent run exposes state.
            return None;
        }
        Some(DetectedAgentState::Idle)
    }
}

fn question_dialog(screen: &str) -> bool {
    screen.contains("esc dismiss")
        && ["enter submit", "enter toggle", "enter confirm"]
            .iter()
            .any(|hint| screen.contains(hint))
}

fn progress_bar(screen: &str) -> bool {
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

fn subagent_view(screen: &str) -> bool {
    screen.contains("parent up") && (screen.contains("prev left") || screen.contains("next right"))
}
