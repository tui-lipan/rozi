use crate::session::protocol::DetectedAgentState;

use super::super::{AgentDetectionInput, AgentDetector};
use super::generic;

pub(super) struct ClaudeDetector;

impl AgentDetector for ClaudeDetector {
    fn detect(&self, input: AgentDetectionInput<'_>) -> Option<DetectedAgentState> {
        if choice_dialog(input.screen) {
            return Some(DetectedAgentState::Blocked);
        }
        let title_spinner = input
            .title
            .chars()
            .next()
            .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch));
        if title_spinner || generic::working(input.screen) {
            Some(DetectedAgentState::Working)
        } else {
            Some(DetectedAgentState::Idle)
        }
    }
}

/// Match the selection cursor on a numbered option, the stable structure of Claude's approval
/// dialogs. Question prose is too broad: ordinary transcripts frequently quote it.
fn choice_dialog(screen: &str) -> bool {
    screen.lines().any(|line| {
        let Some(option) = line.trim_start().strip_prefix('❯') else {
            return false;
        };
        let option = option.trim_start();
        let index: String = option.chars().take_while(char::is_ascii_digit).collect();
        !index.is_empty() && option[index.len()..].starts_with(". ")
    })
}
