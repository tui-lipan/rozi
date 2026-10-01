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

/// Whether `pid` has certainly exited: no such process, or one that has finished but whose handle
/// is still held. A process this user may not open is not known to be gone.
pub(super) fn process_gone(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    // SAFETY: the handle is checked before use and closed exactly once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let mut code = 0;
        let exited = GetExitCodeProcess(handle, &mut code) != 0 && code != STILL_ACTIVE as u32;
        CloseHandle(handle);
        exited
    }
}
