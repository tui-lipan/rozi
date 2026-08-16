use crate::session::protocol::DetectedAgentState;

use super::super::{AgentDetectionInput, AgentDetector};
use super::generic::GenericDetector;

/// Codex currently uses the shared approval and interrupt vocabulary. Keeping a named provider
/// makes that ownership explicit and gives future Codex-specific evidence one isolated home.
pub(super) struct CodexDetector;

impl AgentDetector for CodexDetector {
    fn detect(&self, input: AgentDetectionInput<'_>) -> Option<DetectedAgentState> {
        GenericDetector.detect(input)
    }
}
