//! macOS `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! **Cross-compile checked only** (`cargo check --target x86_64-apple-darwin`) - there is no macOS
//! runtime available in this environment, so the `libproc`-family calls below are unverified at
//! runtime and rely on careful reading of the Darwin `libproc.h` contract.
//!
//! `cwd` uses `proc_pidinfo(PROC_PIDVNODEPATHINFO)` to read the child's current-directory vnode
//! path. Foreground inspection starts from the PTY's process-group id and uses
//! `proc_listpgrppids` so wrappers, pipelines, and groups whose original leader exited retain the
//! same process-set semantics as Linux.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

use super::{ForegroundJob, ForegroundLaunch, ForegroundProcess, ProcessInspector};

const MAX_FOREGROUND_PROCESSES: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
pub struct MacosProcessInspector;

impl ProcessInspector for MacosProcessInspector {
    fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf> {
        let pid = pty.pid()?;
        cwd_for_pid(pid)
    }

    fn foreground_program(&self, pty: &TerminalPty) -> Option<String> {
        let pgid = pty.foreground_process_group_id()?;
        process_for_pid(pgid).map(|process| process.name)
    }

    fn foreground_process(&self, pty: &TerminalPty) -> Option<ForegroundProcess> {
        process_for_pid(pty.foreground_process_group_id()?)
    }

    /// Path only: reading another process's arguments on Darwin means `KERN_PROCARGS2`, which is
    /// a different contract from every other call in this file and cannot be verified here.
    /// Callers already treat empty arguments as "not available", which is also the Windows answer.
    fn foreground_launch(&self, pty: &TerminalPty) -> Option<ForegroundLaunch> {
        Some(ForegroundLaunch {
            executable: Some(path_for_pid(pty.foreground_process_group_id()?)?),
            argv: Vec::new(),
        })
    }

    fn foreground_job(&self, pty: &TerminalPty) -> Option<ForegroundJob> {
        let process_group_id = pty.foreground_process_group_id()?.try_into().ok()?;
        let mut processes: Vec<_> = process_group_pids(process_group_id)
            .into_iter()
            .filter_map(process_for_pid)
            .collect();
        if processes.is_empty()
            && let Some(leader) = process_for_pid(process_group_id as i32)
        {
            processes.push(leader);
        }
        processes.sort_unstable_by_key(|process| (process.pid != process_group_id, process.pid));
        (!processes.is_empty()).then_some(ForegroundJob {
            process_group_id,
            processes,
        })
    }
}

fn process_group_pids(process_group_id: u32) -> Vec<i32> {
    let process_group_id = process_group_id as libc::pid_t;
    let needed = unsafe { libc::proc_listpgrppids(process_group_id, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let pid_size = std::mem::size_of::<libc::pid_t>();
    let capacity = ((needed as usize / pid_size) + 8).min(MAX_FOREGROUND_PROCESSES);
    let mut pids = vec![0 as libc::pid_t; capacity];
    let buffer_bytes = (pids.len() * pid_size).min(libc::c_int::MAX as usize) as libc::c_int;
    let written = unsafe {
        libc::proc_listpgrppids(process_group_id, pids.as_mut_ptr().cast(), buffer_bytes)
    };
    if written <= 0 {
        return Vec::new();
    }
    pids.truncate((written as usize / pid_size).min(pids.len()));
    pids.into_iter().filter(|pid| *pid > 0).collect()
}

fn process_for_pid(pid: i32) -> Option<ForegroundProcess> {
    let executable = path_for_pid(pid);
    let name = executable
        .as_deref()
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(|| name_for_pid(pid))?;
    Some(ForegroundProcess {
        pid: pid.try_into().ok()?,
        name,
        executable: executable.map(|path| path.to_string_lossy().into_owned()),
        argv: Vec::new(),
        agent_hint: None,
    })
}

/// Read `pid`'s current working directory via `proc_pidinfo(PROC_PIDVNODEPATHINFO)`.
///
/// `pvi_cdir.vip_path` is a `MAXPATHLEN`-sized, NUL-terminated C string laid out by `libc` as
/// `[[c_char; 32]; 32]` (a workaround for an old-rustc array-size limitation, per the `libc` crate
/// source) rather than the natural `[c_char; 1024]` - flatten it back before reading the C string.
fn cwd_for_pid(pid: u32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size as libc::c_int,
        )
    };
    if written as usize != size {
        return None;
    }
    let flat: Vec<libc::c_char> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flat_map(|chunk| chunk.iter().copied())
        .collect();
    let bytes: Vec<u8> = flat.iter().map(|&byte| byte as u8).collect();
    let cstr = CStr::from_bytes_until_nul(&bytes).ok()?;
    let text = cstr.to_str().ok()?;
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// Read `pid`'s executable path via `proc_pidpath`.
///
/// The buffer is `PROC_PIDPATHINFO_MAXSIZE` (4 * `MAXPATHLEN`), the size Darwin documents as
/// always sufficient; it is spelled out here rather than taken from `libc` so the size does not
/// depend on which constants that crate exposes for this target.
fn path_for_pid(pid: i32) -> Option<PathBuf> {
    const MAX_PATH_BYTES: usize = 4 * 1024;

    let mut buf = [0u8; MAX_PATH_BYTES];
    let written =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if written <= 0 {
        return None;
    }
    let cstr = CStr::from_bytes_until_nul(&buf).ok()?;
    let path = PathBuf::from(cstr.to_str().ok()?);
    path.is_absolute().then_some(path)
}

/// Resolve a fallback process name via `proc_name(3)`. The executable path above is preferred
/// because this value may be truncated by the kernel.
fn name_for_pid(pid: i32) -> Option<String> {
    let mut buf = [0u8; 64];
    let written =
        unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if written <= 0 {
        return None;
    }
    let cstr = CStr::from_bytes_until_nul(&buf).ok()?;
    let name = cstr.to_str().ok()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}
