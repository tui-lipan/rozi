pub(super) mod catalog;
mod claude;
mod codex;
mod generic;
mod opencode;

use crate::session::protocol::AgentKind;

use super::AgentDetector;

static CLAUDE: claude::ClaudeDetector = claude::ClaudeDetector;
static CODEX: codex::CodexDetector = codex::CodexDetector;
static OPENCODE: opencode::OpenCodeDetector = opencode::OpenCodeDetector;
static GENERIC: generic::GenericDetector = generic::GenericDetector;

pub(super) fn detector(kind: AgentKind) -> &'static dyn AgentDetector {
    match kind {
        AgentKind::Claude => &CLAUDE,
        AgentKind::Codex => &CODEX,
        AgentKind::OpenCode => &OPENCODE,
        _ => &GENERIC,
    }
}
