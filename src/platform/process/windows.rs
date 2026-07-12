//! Windows `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! Explicitly unavailable, per the plan: "Windows process inspection intentionally unsupported (no
//! PEB or process-tree probing)." Foreground-executable and CWD fallback on Windows rely entirely
//! on shell-reported OSC metadata (Phase 8); when that is absent, callers see conservative
//! "unknown program" / launch-directory-only behavior rather than best-effort native probing.

use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

use super::ProcessInspector;

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsProcessInspector;

impl ProcessInspector for WindowsProcessInspector {
    fn cwd(&self, _pty: &TerminalPty) -> Option<PathBuf> {
        None
    }

    fn foreground_program(&self, _pty: &TerminalPty) -> Option<String> {
        None
    }
}
