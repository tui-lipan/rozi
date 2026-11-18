//! Foreground-process and working-directory inspection fallbacks (cross-platform plan Phase 9).
//!
//! [`ProcessInspector`] is the trait the plan specifies; [`PlatformProcessInspector`] resolves to
//! whichever per-OS implementation applies to the current build target. Every implementation is
//! best-effort and server-side only (the server owns the PTY and process identity). Shell intent,
//! foreground process-group evidence, and replayable launch details are reconciled by
//! `session::server`'s pane-runtime-state computation (Phase 6), not here.
//!
//! - [`linux`] - **implemented**: `/proc/<pid>/cwd` plus bounded foreground process-group records.
//! - [`macos`] - **implemented, unverified** (no macOS runtime in this environment; cross-compile
//!   checked only): `proc_pidvnodepathinfo` for cwd and `libproc` foreground-group enumeration.
//! - [`windows`] - **implemented for agent identity**: the pane's descendant tree from one
//!   ToolHelp snapshot, with each member's executable, creation time, and command line read
//!   through a limited-information handle. No cwd, foreground program, or foreground job.
//!
//! [`tree`] is the bounded descendant walk the Windows backend runs over its snapshot.
//!
//! On Windows every pane is spawned with [`PANE_RUNTIME_ID_ENV`] set to a fresh random marker
//! ([`new_pane_marker`]). Git for Windows' bash starts some programs, such as one it `exec`s,
//! outside its own process tree; [`ProcessInspector::marked_processes_in`] finds those by the marker
//! their environment inherited, reading the environment only of processes already recognized as
//! agents (see [`env_block`]).
//!
//! [`process_gone`] is separate from pane inspection: it asks whether one recorded process ID has
//! exited, as a lock or pidfile left behind by another program names it.

use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: u32,
    /// When the process started, in platform units, so a reused PID is not mistaken for it. `None`
    /// where the platform does not report it.
    pub started: Option<u64>,
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

/// The processes a pane's agent may be among, as one observation.
///
/// Separate from [`ForegroundJob`] because not every platform has one: a POSIX terminal's
/// foreground process group says which job holds the terminal, which navigation, resurrection and
/// launch metadata rely on. Windows has no such group, only the tree of processes descended from
/// the pane's own process, and some of those may be background jobs. That tree is good evidence of
/// *which agent* runs in the pane, but not of what holds its input, so it feeds agent identity only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProcessSet {
    /// The process group ID for [`ProcessTopology::ForegroundGroup`], or the pane's own process
    /// for [`ProcessTopology::DescendantTree`].
    pub root_pid: u32,
    pub topology: ProcessTopology,
    /// Whether every process in the tree is listed. A descendant tree too large to walk whole is
    /// only a prefix of itself, and a choice made on a prefix could pass over the process that
    /// matters. Always `true` for a foreground group.
    pub complete: bool,
    /// Parents before their children for a descendant tree; the group leader first for a group.
    pub processes: Vec<ObservedProcess>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessTopology {
    /// Members of the terminal's foreground process group.
    ForegroundGroup,
    /// The pane process and its descendants, whatever holds the terminal.
    DescendantTree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedProcess {
    pub process: ForegroundProcess,
    /// The parent's PID, where it is known and is itself in the set's tree.
    pub parent_pid: Option<u32>,
}

impl AgentProcessSet {
    pub fn from_foreground_job(job: &ForegroundJob) -> Self {
        Self {
            root_pid: job.process_group_id,
            topology: ProcessTopology::ForegroundGroup,
            complete: true,
            processes: job
                .processes
                .iter()
                .map(|process| ObservedProcess {
                    process: process.clone(),
                    parent_pid: None,
                })
                .collect(),
        }
    }

    /// Whether the process identified by `pid` and `started` is still in the set. A PID whose
    /// recorded start differs from the one observed now belongs to a different process.
    pub fn contains(&self, pid: u32, started: Option<u64>) -> bool {
        self.processes.iter().any(|observed| {
            observed.process.pid == pid
                && match (started, observed.process.started) {
                    (Some(before), Some(now)) => before == now,
                    _ => true,
                }
        })
    }
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
    #[cfg(windows)]
    snapshot: windows::ProcessSnapshot,
}

impl ProcessScan {
    /// Walk the process table now. Linux and Windows reuse this host-wide scan; macOS queries one
    /// process group directly.
    pub fn capture() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            entries: linux::scan_processes(),
            #[cfg(windows)]
            snapshot: windows::ProcessSnapshot::capture(),
        }
    }
}

/// What identifies one process when walking up the process tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub parent: u32,
    /// The kernel's short name for the process, such as `sshd` or `zsh`.
    pub name: String,
    /// `argv[0]` as the process was started; a login shell's begins with `-`. `None` when this
    /// process may not read it, as for another user's.
    pub argv0: Option<String>,
}

/// `pid`'s parent, name, and `argv[0]`, on Linux and macOS; `None` elsewhere or when `pid` is
/// gone.
pub fn identity(pid: u32) -> Option<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    return linux::identity(pid);
    #[cfg(target_os = "macos")]
    return macos::identity(pid);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
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

/// Whether the process `pid` has certainly exited on this host.
///
/// `started` is the start time recorded alongside the PID, in this platform's units: on Linux the
/// clock ticks since boot of `/proc/<pid>/stat`, and on Windows the creation time as a FILETIME,
/// which tells a reused PID from the original. macOS only checks that the PID exists. Any doubt, such as a process this user may not
/// signal, answers `false`.
pub fn process_gone(pid: u32, started: Option<u64>) -> bool {
    #[cfg(unix)]
    {
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if raw <= 0 {
            return false;
        }
        // SAFETY: signal 0 performs only the existence and permission checks.
        if unsafe { libc::kill(raw, 0) } != 0 {
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        }
        #[cfg(target_os = "linux")]
        if let Some(started) = started {
            return linux::start_time(pid).is_some_and(|now| now != started);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = started;
        false
    }
    #[cfg(windows)]
    return windows::process_gone(pid, started);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, started);
        false
    }
}

/// The variable carrying a pane's marker; see [`new_pane_marker`].
pub const PANE_RUNTIME_ID_ENV: &str = "ROZI_PANE_RUNTIME_ID";

/// A fresh, unguessable marker for one pane spawn, where this platform needs one to find the
/// pane's processes; `None` where the process tree already finds them.
///
/// Every spawn gets its own, a respawn included, so a process left over from an earlier spawn
/// never matches the current one.
pub fn new_pane_marker() -> Option<String> {
    #[cfg(windows)]
    {
        use std::fmt::Write;
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).ok()?;
        Some(bytes.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
    }
    #[cfg(not(windows))]
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
pub mod env_block;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
// Compiled everywhere so its tests run on every target; only Windows walks a tree today.
#[cfg_attr(not(windows), allow(dead_code))]
pub mod tree;
#[cfg(windows)]
pub mod windows;

/// Best-effort native view of a pane's working directory and foreground executable.
///
/// Runtime tracking reconciles this with shell-integration reports: the shell preserves logical
/// command identity, while native inspection reports every process eligible to read from a POSIX
/// terminal's foreground group. There is deliberately no claim that one member is the unique input
/// receiver.
pub trait ProcessInspector {
    /// The PTY child's current working directory, if the platform can determine one.
    fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf>;
    /// The process group of a job the PTY child started that now holds the terminal, such as a
    /// program run from a shell. `None` while the child itself holds it, at a prompt or as the
    /// pane's own command, and wherever the platform has no process groups.
    fn shell_job(&self, _pty: &TerminalPty) -> Option<u32> {
        None
    }
    /// The current working directory of process `pid`, where the platform can read another
    /// process's directory.
    fn process_cwd(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
    /// The normalized basename of the process currently in the PTY's foreground process group, if
    /// the platform can determine one. Never a full command line.
    fn foreground_program(&self, pty: &TerminalPty) -> Option<String>;
    /// Group leader identity and invocation read as one observation.
    ///
    /// Platform backends should override this when separate name/launch reads can race a process
    /// group transition. The default preserves compatibility for lightweight/test inspectors.
    fn foreground_process(&self, pty: &TerminalPty) -> Option<ForegroundProcess> {
        let name = self.foreground_program(pty)?;
        let launch = self.foreground_launch(pty).unwrap_or_default();
        Some(ForegroundProcess {
            pid: 0,
            started: None,
            name,
            executable: launch
                .executable
                .map(|path| path.to_string_lossy().into_owned()),
            argv: launch.argv,
            agent_hint: None,
        })
    }
    /// How the process leading that same foreground process group was invoked.
    ///
    /// [`foreground_program`](Self::foreground_program) names what is running; this says where it
    /// lives and which arguments it was given, which is what it takes to *run it again*. A pane
    /// started through a shell alias or from a build tree reports a name nothing on `PATH`
    /// resolves, and a name on its own never carries the flags an agent was launched with.
    ///
    /// Both halves come from the same process and are only meaningful together, so they are read
    /// in one call: an executable path that belongs to a different process than the arguments
    /// would compose into a command nobody ran.
    fn foreground_launch(&self, _pty: &TerminalPty) -> Option<ForegroundLaunch> {
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

    /// The processes this pane's agent may be among, for agent identity only.
    ///
    /// `foreground_job` is what [`foreground_job_in`](Self::foreground_job_in) just returned for
    /// the same pane, so a platform whose foreground group is also its agent evidence answers
    /// without walking again. That is the default. A platform with no foreground group overrides
    /// this to return the pane's descendant tree from `scan` instead.
    fn agent_processes_in(
        &self,
        pty: &TerminalPty,
        scan: &ProcessScan,
        foreground_job: Option<&ForegroundJob>,
    ) -> Option<AgentProcessSet> {
        let _ = (pty, scan);
        foreground_job.map(AgentProcessSet::from_foreground_job)
    }

    /// Agents started from the pane `root` that left its process tree, found by the pane's
    /// `marker` in their environment, each with its own descendants after it. Each escaped
    /// process has no `parent_pid`.
    ///
    /// Only where the platform needs it (see [`new_pane_marker`]); the default finds nothing.
    /// `may_be_agent` is a cheap check on an executable name that picks which processes are read
    /// at all, and `is_agent` the full check on one that was; a process's environment is read only
    /// once both pass. The agent catalog supplies both, so no agent is named here.
    fn marked_processes_in(
        &self,
        scan: &ProcessScan,
        root: u32,
        marker: &str,
        may_be_agent: &dyn Fn(&str) -> bool,
        is_agent: &dyn Fn(&ForegroundProcess) -> bool,
    ) -> Vec<ObservedProcess> {
        let _ = (scan, root, marker, may_be_agent, is_agent);
        Vec::new()
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

/// How the leader of a PTY's foreground process group was invoked, as far as the platform can see.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForegroundLaunch {
    /// Absolute path of the running executable, when the platform can read it.
    pub executable: Option<PathBuf>,
    /// The process's argument vector, `argv[0]` included, bounded in both count and length by the
    /// platform backend. Empty when the platform cannot read arguments at all.
    pub argv: Vec<String>,
}

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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod identity_tests {
    /// A login shell is recognized by its `argv[0]`, so the reader must return what the process
    /// was started with, not its executable's name.
    #[test]
    fn a_process_reports_its_parent_and_the_argv0_it_was_started_with() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new("sleep")
            .arg0("-sleep")
            .arg("5")
            .spawn()
            .expect("sleep starts");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let identity = loop {
            // Until `exec` completes, the child still reports this test binary's arguments.
            let identity = super::identity(child.id()).expect("child is running");
            if identity.argv0.as_deref() == Some("-sleep") || std::time::Instant::now() > deadline {
                break identity;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(identity.argv0.as_deref(), Some("-sleep"));
        assert_eq!(identity.parent, std::process::id());
        assert_eq!(identity.name, "sleep");
    }
}
