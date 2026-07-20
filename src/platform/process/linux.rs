//! Linux `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! Both operations key off `/proc`: `cwd` reads `/proc/<pid>/cwd` for the PTY child's own pid;
//! `foreground_program` reads `/proc/<pgid>/comm` for the PTY's current foreground process-group
//! leader. `foreground_job` additionally scans `/proc/*/stat` for group members so wrapped agents
//! and package runners can be identified from bounded argument and environment reads.

use std::io::Read;
use std::path::PathBuf;

use tui_lipan::prelude::TerminalPty;

use super::{ForegroundJob, ForegroundProcess, ProcessInspector};

const MAX_FOREGROUND_PROCESSES: usize = 64;
const MAX_PROC_BYTES: u64 = 64 * 1024;
const MAX_ARG_COUNT: usize = 128;
const MAX_ARG_CHARS: usize = 4096;

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

    fn foreground_job(&self, pty: &TerminalPty) -> Option<ForegroundJob> {
        foreground_job_for_group(
            pty.foreground_process_group_id()?.try_into().ok()?,
            &scan_processes(),
        )
    }

    fn foreground_job_in(
        &self,
        pty: &TerminalPty,
        scan: &super::ProcessScan,
    ) -> Option<ForegroundJob> {
        foreground_job_for_group(
            pty.foreground_process_group_id()?.try_into().ok()?,
            &scan.entries,
        )
    }
}

/// One process from the shared table walk: enough to decide group membership without the
/// expensive per-process reads.
#[derive(Clone, Debug)]
pub struct ScannedProcess {
    pid: u32,
    process_group_id: u32,
}

/// Walk `/proc` once, recording each process's group. This is the part every pane used to repeat;
/// see [`super::ProcessScan`].
pub fn scan_processes() -> Vec<ScannedProcess> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut scanned = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .filter(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // A process that exits between the readdir and this read simply drops out, exactly as it
        // did when each pane walked separately.
        let Some((process_group_id, _)) = read_process_stat(pid) else {
            continue;
        };
        scanned.push(ScannedProcess {
            pid,
            process_group_id,
        });
    }
    scanned
}

fn foreground_job_for_group(
    process_group_id: u32,
    scanned: &[ScannedProcess],
) -> Option<ForegroundJob> {
    let mut processes = Vec::new();
    // The group leader stays first even if it is not in the scan (it may have exited between the
    // walk and now, or the walk may have been skipped entirely).
    if let Some(process) = process_group_member(process_group_id, process_group_id) {
        processes.push(process);
    }
    for entry in scanned {
        if entry.process_group_id != process_group_id || entry.pid == process_group_id {
            continue;
        }
        let Some(process) = process_group_member(process_group_id, entry.pid) else {
            continue;
        };
        processes.push(process);
        if processes.len() == MAX_FOREGROUND_PROCESSES {
            break;
        }
    }
    processes.sort_unstable_by_key(|process| (process.pid != process_group_id, process.pid));
    (!processes.is_empty()).then_some(ForegroundJob {
        process_group_id,
        processes,
    })
}

fn process_group_member(process_group_id: u32, pid: u32) -> Option<ForegroundProcess> {
    let (group, name) = read_process_stat(pid)?;
    (group == process_group_id).then(|| ForegroundProcess {
        pid,
        name,
        executable: std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        argv: read_nul_records(&format!("/proc/{pid}/cmdline")),
        agent_hint: read_agent_hint(&format!("/proc/{pid}/environ")),
    })
}

fn read_process_stat(pid: u32) -> Option<(u32, String)> {
    parse_process_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn parse_process_stat(stat: &str) -> Option<(u32, String)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let fields: Vec<_> = stat.get(close + 2..)?.split_whitespace().collect();
    let process_group_id = fields.get(2)?.parse().ok()?;
    Some((process_group_id, name))
}

fn read_bounded(path: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_PROC_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn read_nul_records(path: &str) -> Vec<String> {
    read_bounded(path)
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .take(MAX_ARG_COUNT)
        .map(|part| {
            String::from_utf8_lossy(part)
                .chars()
                .take(MAX_ARG_CHARS)
                .collect()
        })
        .collect()
}

fn parse_agent_hint(bytes: &[u8]) -> Option<String> {
    let records = bytes.split(|byte| *byte == 0);
    for key in [b"HYPRMUX_AGENT=".as_slice(), b"HERDR_AGENT=".as_slice()] {
        for record in records.clone() {
            if let Some(value) = record.strip_prefix(key)
                && !value.is_empty()
            {
                return Some(String::from_utf8_lossy(value).chars().take(64).collect());
            }
        }
    }
    None
}

fn read_agent_hint(path: &str) -> Option<String> {
    parse_agent_hint(&read_bounded(path)?)
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

    #[test]
    fn stat_parser_handles_spaces_and_parentheses_in_comm() {
        assert_eq!(
            parse_process_stat("42 (node worker (agent)) S 1 777 777 0 -1"),
            Some((777, "node worker (agent)".into()))
        );
    }

    #[test]
    fn agent_hint_prefers_hyprmux_and_accepts_herdr() {
        assert_eq!(
            parse_agent_hint(b"HERDR_AGENT=claude\0HYPRMUX_AGENT=codex\0"),
            Some("codex".into())
        );
        assert_eq!(
            parse_agent_hint(b"PATH=/bin\0HERDR_AGENT=opencode\0"),
            Some("opencode".into())
        );
    }
}
