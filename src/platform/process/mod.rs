//! Foreground-process and working-directory inspection fallbacks (cross-platform plan Phase 9).
//!
//! [`ProcessInspector`] is the trait the plan specifies; [`PlatformProcessInspector`] resolves to
//! whichever per-OS implementation applies to the current build target. Every implementation is
//! best-effort and server-side only (the server owns the PTY and process identity) - CWD/foreground
//! precedence (OSC report first, this trait second, launch config last) is decided by
//! `session::server`'s pane-runtime-state computation (Phase 6), not here.
//!
//! - [`linux`] - **implemented**: `/proc/<pid>/cwd` plus bounded foreground process-group records.
//! - [`macos`] - **implemented, unverified** (no macOS runtime in this environment; cross-compile
//!   checked only): `proc_pidvnodepathinfo` for cwd, `proc_name` for the foreground executable.
//! - [`windows`] - **implemented as explicit unavailable**, per the plan (no PEB/process-tree
//!   probing on Windows).

use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
    pub argv: Vec<String>,
    pub agent_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundJob {
    pub process_group_id: u32,
    pub processes: Vec<ForegroundProcess>,
}

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

/// Best-effort native fallback for a pane's working directory and foreground executable, used
/// when the child has not (yet) reported either via shell-integration OSC sequences.
pub trait ProcessInspector {
    /// The PTY child's current working directory, if the platform can determine one.
    fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf>;
    /// The normalized basename of the process currently in the PTY's foreground process group, if
    /// the platform can determine one. Never a full command line.
    fn foreground_program(&self, pty: &TerminalPty) -> Option<String>;
    fn foreground_job(&self, _pty: &TerminalPty) -> Option<ForegroundJob> {
        None
    }
}

#[cfg(target_os = "linux")]
pub type PlatformProcessInspector = linux::LinuxProcessInspector;
#[cfg(target_os = "macos")]
pub type PlatformProcessInspector = macos::MacosProcessInspector;
#[cfg(windows)]
pub type PlatformProcessInspector = windows::WindowsProcessInspector;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub type PlatformProcessInspector = NullProcessInspector;

/// Fallback for any Unix-like target that is neither Linux nor macOS (e.g. the BSDs), which the
/// plan does not otherwise commit to supporting - always unavailable, but keeps the crate building
/// there instead of failing to resolve [`PlatformProcessInspector`].
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
#[derive(Clone, Copy, Debug, Default)]
pub struct NullProcessInspector;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
impl ProcessInspector for NullProcessInspector {
    fn cwd(&self, _pty: &TerminalPty) -> Option<PathBuf> {
        None
    }
    fn foreground_program(&self, _pty: &TerminalPty) -> Option<String> {
        None
    }
}
