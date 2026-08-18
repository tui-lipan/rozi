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
    pub executable: Option<String>,
    pub argv: Vec<String>,
    pub agent_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundJob {
    pub process_group_id: u32,
    pub processes: Vec<ForegroundProcess>,
}

/// One pass over the host's process table, shared by every pane in a poll cycle.
///
/// Finding a pane's process-group members means examining every process on the host, and the
/// server resolves a foreground job per pane. Done independently that repeats the same walk once
/// per pane, so idle cost scaled with pane count (~2% of a core each). Capturing the walk once and
/// answering each pane from it keeps the cost flat.
///
/// Only the cheap identifying pass is shared. The expensive per-process reads (executable,
/// argv, agent hint) still happen only for the groups actually asked about.
///
/// Capture is lazy at the call site: a poll where every pane's detection is already cached must
/// not pay for a walk nobody reads. See [`ProcessScan::capture`].
#[derive(Debug, Default)]
pub struct ProcessScan {
    #[cfg(target_os = "linux")]
    entries: Vec<linux::ScannedProcess>,
}

impl ProcessScan {
    /// Walk the process table now. Platforms whose `foreground_job` does not enumerate processes
    /// (macOS reads the group leader directly; Windows reports unavailable) capture nothing.
    pub fn capture() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            entries: linux::scan_processes(),
        }
    }
}

/// A [`ProcessScan`] captured on first use and then reused, so a poll cycle walks at most once and
/// only when some pane actually needs it.
#[derive(Debug, Default)]
pub struct LazyProcessScan(Option<ProcessScan>);

impl LazyProcessScan {
    pub fn get(&mut self) -> &ProcessScan {
        self.0.get_or_insert_with(ProcessScan::capture)
    }

    /// Whether the walk actually happened, for tests and diagnostics.
    pub fn captured(&self) -> bool {
        self.0.is_some()
    }
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
    /// Absolute path of the executable behind that same foreground process group.
    ///
    /// The basename alone is enough to *name* a running program, but not always enough to *run*
    /// it again: a pane started through a shell alias or from a build tree reports a name nothing
    /// on `PATH` resolves. Callers use this to replay such a program by path. It is a path, never
    /// a command line - arguments are deliberately not exposed here.
    fn foreground_executable(&self, _pty: &TerminalPty) -> Option<PathBuf> {
        None
    }
    fn foreground_job(&self, _pty: &TerminalPty) -> Option<ForegroundJob> {
        None
    }

    /// Resolve the foreground job against an already-captured [`ProcessScan`].
    ///
    /// Implementations that enumerate the process table should override this and read `scan`
    /// instead of walking again; the default ignores it, which is correct for platforms that
    /// resolve the group leader directly.
    fn foreground_job_in(&self, pty: &TerminalPty, scan: &ProcessScan) -> Option<ForegroundJob> {
        let _ = scan;
        self.foreground_job(pty)
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
