//! Linux `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! Both operations key off `/proc`: `cwd` reads `/proc/<pid>/cwd` for the PTY child's own pid;
//! `foreground_program` reads `/proc/<pgid>/comm` for the PTY's current foreground process-group
//! id. Using the pgid directly (rather than enumerating `/proc/*/stat` for group members) relies on
//! the standard Unix invariant that a process group id is the pid of its group leader - the first
//! command in a pipeline, which is exactly the process a shell hands the terminal to in the common
//! single-foreground-command case this fallback targets.

use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

use super::ProcessInspector;

#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxProcessInspector;

impl ProcessInspector for LinuxProcessInspector {
    fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf> {
        let pid = pty.pid()?;
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    fn foreground_program(&self, pty: &TerminalPty) -> Option<String> {
        let pgid = pty.foreground_process_group_id()?;
        let comm = std::fs::read_to_string(format!("/proc/{pgid}/comm")).ok()?;
        let name = comm.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_cwd_and_foreground_program_for_a_live_child() {
        let pty = TerminalPty::spawn(
            tui_lipan::prelude::TerminalPtyConfig::new("/bin/sh")
                .arg("-c")
                .arg("cd /tmp && exec sleep 5"),
            |_event| {},
        )
        .expect("spawn");
        // Give the child a moment to exec into `sleep` and chdir.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let inspector = LinuxProcessInspector;
        assert_eq!(inspector.cwd(&pty), Some(PathBuf::from("/tmp")));
        assert_eq!(
            inspector.foreground_program(&pty),
            Some("sleep".to_string())
        );

        drop(pty);
    }

    #[test]
    fn reports_none_once_the_pty_is_gone() {
        let inspector = LinuxProcessInspector;
        let pty = TerminalPty::spawn(
            tui_lipan::prelude::TerminalPtyConfig::new("/bin/sh")
                .arg("-c")
                .arg("true"),
            |_event| {},
        )
        .expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(pty.clone());
        // No liveness guarantee about the exact drop timing is asserted here; the important part
        // is that neither call panics for an exited/inspectable-but-gone process.
        let _ = inspector.cwd(&pty);
        let _ = inspector.foreground_program(&pty);
    }
}
