//! macOS `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! **Cross-compile checked only** (`cargo check --target x86_64-apple-darwin`) - there is no macOS
//! runtime available in this environment, so the `libproc`-family calls below are unverified at
//! runtime and rely on careful reading of the Darwin `libproc.h` contract.
//!
//! `cwd` uses `proc_pidinfo(PROC_PIDVNODEPATHINFO)` to read the child's current-directory vnode
//! path. `foreground_program` treats the PTY's foreground process-group id as a pid (a process
//! group id is by definition the pid of its group leader) and resolves its name with `proc_name`,
//! mirroring the Linux backend's `/proc/<pgid>/comm` approach without needing a pgid -> member-pids
//! lookup.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

use super::ProcessInspector;

#[derive(Clone, Copy, Debug, Default)]
pub struct MacosProcessInspector;

impl ProcessInspector for MacosProcessInspector {
    fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf> {
        let pid = pty.pid()?;
        cwd_for_pid(pid)
    }

    fn foreground_program(&self, pty: &TerminalPty) -> Option<String> {
        let pgid = pty.foreground_process_group_id()?;
        name_for_pid(pgid)
    }
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

/// Resolve a process name via `proc_name(3)`, truncated by the kernel to `MAXCOMLEN` (16 bytes) -
/// acceptable here since only a normalized executable basename is ever surfaced, never a full path
/// or command line.
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
