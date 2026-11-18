//! Windows `ProcessInspector` implementation.
//!
//! Windows has no foreground process group, so this backend answers only agent identity: the
//! pane's process and its descendants, read from one ToolHelp snapshot per poll cycle. Each tree
//! member is opened with `PROCESS_QUERY_LIMITED_INFORMATION` alone, which another process's memory
//! cannot be read through, for its executable path, creation time, and command line
//! (`NtQueryInformationProcess`'s `ProcessCommandLineInformation`). A process that cannot be opened
//! keeps the name the snapshot gave it.
//!
//! Git for Windows' bash (MSYS2) starts some programs, such as one it `exec`s, outside its own
//! process tree. When a pane's tree holds such a shell, processes elsewhere on the host that the
//! agent catalog recognizes have their environment read for the pane's marker
//! ([`PANE_RUNTIME_ID_ENV`](super::PANE_RUNTIME_ID_ENV)), through a handle that adds
//! `PROCESS_VM_READ`. Those reads are cached per process.
//!
//! Working directory, foreground program and foreground job stay unavailable: they come from
//! shell-reported OSC metadata, and without it callers see conservative "unknown program" /
//! launch-directory-only behavior.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tui_lipan::prelude::TerminalPty;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, FILETIME,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

use super::tree::{MAX_TREE_PROCESSES, ProcessTable, TableEntry};
use super::{
    AgentProcessSet, ForegroundJob, ForegroundProcess, ObservedProcess, ProcessInspector,
    ProcessScan, ProcessTopology,
};

const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;
const MAX_ARG_COUNT: usize = 128;
const MAX_ARG_CHARS: usize = 4096;
/// The most processes outside a pane's tree read for its marker in one lookup. A host with more
/// that could be agents gets no lookup at all.
const MAX_MARKED_CANDIDATES: usize = 64;
const MAX_ENVIRONMENT_BYTES: usize = 64 * 1024;

/// The host's process table, captured once per poll cycle. See [`ProcessScan`].
#[derive(Debug, Default)]
pub struct ProcessSnapshot {
    table: ProcessTable,
}

impl ProcessSnapshot {
    /// One ToolHelp pass recording each process's PID, parent PID and executable name. No process
    /// is opened here; an unavailable snapshot is an empty table.
    pub fn capture() -> Self {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };

        let mut entries = Vec::new();
        // SAFETY: the snapshot handle is checked before use and closed exactly once; `entry` is a
        // properly sized `PROCESSENTRY32W` the calls fill in place.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Self::default();
            }
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                entries.push(TableEntry {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    name: wide_until_nul(&entry.szExeFile),
                });
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        Self {
            table: ProcessTable::new(entries),
        }
    }

    /// `root` and its descendants, each read through a limited-information handle.
    fn descendant_set(&self, root: u32) -> Option<AgentProcessSet> {
        let walk = self.table.descendants(root, |entry| {
            let process = observe(entry);
            let started = process.started;
            (process, started)
        })?;
        Some(AgentProcessSet {
            root_pid: root,
            topology: ProcessTopology::DescendantTree,
            complete: walk.complete,
            processes: walk
                .members
                .into_iter()
                .map(|member| ObservedProcess {
                    process: member.observed,
                    parent_pid: member.parent_pid,
                })
                .collect(),
        })
    }
}

impl ProcessSnapshot {
    /// See [`ProcessInspector::marked_processes_in`].
    fn marked_processes(
        &self,
        root: u32,
        marker: &str,
        may_be_agent: &dyn Fn(&str) -> bool,
        is_agent: &dyn Fn(&ForegroundProcess) -> bool,
    ) -> Option<Vec<ObservedProcess>> {
        let Some(tree) = self.table.descendants(root, |_| ((), None)) else {
            return Some(Vec::new());
        };
        let in_tree: HashSet<u32> = tree.members.iter().map(|member| member.pid).collect();
        let has_msys_shell = self
            .table
            .entries()
            .iter()
            .any(|entry| in_tree.contains(&entry.pid) && is_msys_shell(entry));
        if !has_msys_shell {
            return Some(Vec::new());
        }
        let candidates = self.table.candidates(
            &in_tree,
            |entry| may_be_agent(&entry.name),
            MAX_MARKED_CANDIDATES,
        )?;
        let marked: Vec<u32> = candidates
            .into_iter()
            .filter(|entry| {
                let process = observe(entry);
                process.started.is_some_and(|started| {
                    is_agent(&process) && carries_marker(entry.pid, started, marker)
                })
            })
            .map(|entry| entry.pid)
            .collect();
        let members = self.table.trees_of(
            &self.table.outermost(&marked),
            |entry| {
                let process = observe(entry);
                let started = process.started;
                (process, started)
            },
            MAX_TREE_PROCESSES,
        )?;
        Some(
            members
                .into_iter()
                .map(|member| ObservedProcess {
                    process: member.observed,
                    parent_pid: member.parent_pid,
                })
                .collect(),
        )
    }
}

/// Whether `entry` is an MSYS2 or Cygwin shell, such as Git for Windows' bash, which can start a
/// program outside its own process tree.
///
/// By name, plus its image path to set aside `System32\bash.exe`: that one is WSL's launcher,
/// whose programs are Linux processes this backend never sees.
fn is_msys_shell(entry: &TableEntry) -> bool {
    let name = entry.name.to_ascii_lowercase();
    if name != "bash.exe" && name != "sh.exe" {
        return false;
    }
    let Some(path) = LimitedHandle::open(entry.pid)
        .ok()
        .and_then(|handle| handle.image_path())
    else {
        return false;
    };
    let directory = path
        .rsplit_once('\\')
        .map_or("", |(directory, _)| directory)
        .rsplit('\\')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !matches!(directory.as_str(), "system32" | "syswow64" | "sysnative")
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsProcessInspector;

impl ProcessInspector for WindowsProcessInspector {
    fn cwd(&self, _pty: &TerminalPty) -> Option<PathBuf> {
        None
    }

    fn foreground_program(&self, _pty: &TerminalPty) -> Option<String> {
        None
    }

    fn agent_processes_in(
        &self,
        pty: &TerminalPty,
        scan: &ProcessScan,
        _foreground_job: Option<&ForegroundJob>,
    ) -> Option<AgentProcessSet> {
        scan.snapshot.descendant_set(pty.pid()?)
    }

    fn marked_processes_in(
        &self,
        scan: &ProcessScan,
        root: u32,
        marker: &str,
        may_be_agent: &dyn Fn(&str) -> bool,
        is_agent: &dyn Fn(&ForegroundProcess) -> bool,
    ) -> Option<Vec<ObservedProcess>> {
        scan.snapshot
            .marked_processes(root, marker, may_be_agent, is_agent)
    }
}

/// How long a process found without a marker is not read again. One may be read before its
/// environment is in place, so the answer is retried rather than kept.
const UNMARKED_RETRY: Duration = Duration::from_secs(10);
const MARKER_CACHE_LIMIT: usize = 256;

/// The marker each process read carried, by PID and creation time, so the identity refresh does not
/// read the same environments every time.
static MARKERS: Mutex<Option<MarkerCache>> = Mutex::new(None);

/// The marker read, if any, and when, by PID and creation time.
type MarkerCache = HashMap<(u32, u64), (Option<String>, Instant)>;

/// Whether the process `pid`, created at `started`, carries `marker` in its environment.
fn carries_marker(pid: u32, started: u64, marker: &str) -> bool {
    let key = (pid, started);
    let mut markers = MARKERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let markers = markers.get_or_insert_with(HashMap::new);
    if let Some((value, read_at)) = markers.get(&key)
        && (value.is_some() || read_at.elapsed() < UNMARKED_RETRY)
    {
        return value.as_deref() == Some(marker);
    }
    let value = read_marker(pid, started);
    let matches = value.as_deref() == Some(marker);
    if markers.len() >= MARKER_CACHE_LIMIT {
        markers.retain(|_, (value, read_at)| value.is_some() || read_at.elapsed() < UNMARKED_RETRY);
        if markers.len() >= MARKER_CACHE_LIMIT {
            markers.clear();
        }
    }
    markers.insert(key, (value, Instant::now()));
    matches
}

/// The pane marker in the environment of process `pid`, provided it is still the process created
/// at `started`.
fn read_marker(pid: u32, started: u64) -> Option<String> {
    let handle = MemoryHandle::open(pid)?;
    if handle.0.creation_time() != Some(started) {
        return None;
    }
    let block = handle.environment_block()?;
    super::env_block::find_value(&block, super::PANE_RUNTIME_ID_ENV)
}

/// A process handle that may also read the process's memory, opened only for a process already
/// recognized as an agent.
struct MemoryHandle(LimitedHandle);

impl MemoryHandle {
    fn open(pid: u32) -> Option<Self> {
        use windows_sys::Win32::System::Threading::PROCESS_VM_READ;
        if pid == 0 {
            return None;
        }
        // SAFETY: plain FFI call; a null result is the failure value.
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
        // Not `then_some`: it would build, and drop, a handle around NULL.
        if handle.is_null() {
            return None;
        }
        Some(Self(LimitedHandle(handle)))
    }

    /// The process's environment block as `ProcessParameters` in its PEB describes it, at most
    /// [`MAX_ENVIRONMENT_BYTES`]. `None` for a 32-bit (WOW64) process, whose PEB has another
    /// layout, and wherever a read fails.
    #[cfg(target_pointer_width = "64")]
    fn environment_block(&self) -> Option<Vec<u16>> {
        use windows_sys::Win32::System::Threading::IsWow64Process;

        // Offsets in the 64-bit PEB and `RTL_USER_PROCESS_PARAMETERS`, stable since Windows 7.
        const PEB_PROCESS_PARAMETERS: usize = 0x20;
        const PARAMETERS_ENVIRONMENT: usize = 0x80;
        const PARAMETERS_ENVIRONMENT_SIZE: usize = 0x3F0;
        const PROCESS_BASIC_INFORMATION: i32 = 0;

        #[repr(C)]
        #[derive(Default)]
        struct BasicInformation {
            exit_status: i32,
            peb_base_address: usize,
            affinity_mask: usize,
            base_priority: i32,
            unique_process_id: usize,
            inherited_from_unique_process_id: usize,
        }

        let handle = (self.0).0;
        let mut wow64 = 0;
        // SAFETY: the handle is open and `wow64` is a live `BOOL`.
        if unsafe { IsWow64Process(handle, &mut wow64) } == 0 || wow64 != 0 {
            return None;
        }
        let query = nt_query_information_process()?;
        let mut basic = BasicInformation::default();
        let mut written = 0_u32;
        // SAFETY: `basic` is writable for its own size and `written` is a live `u32`.
        let status = unsafe {
            query(
                handle,
                PROCESS_BASIC_INFORMATION,
                (&raw mut basic).cast(),
                size_of::<BasicInformation>() as u32,
                &mut written,
            )
        };
        if status < 0 || basic.peb_base_address == 0 {
            return None;
        }
        let parameters = self.read_usize(basic.peb_base_address + PEB_PROCESS_PARAMETERS)?;
        if parameters == 0 {
            return None;
        }
        let environment = self.read_usize(parameters + PARAMETERS_ENVIRONMENT)?;
        let size = self
            .read_usize(parameters + PARAMETERS_ENVIRONMENT_SIZE)?
            .min(MAX_ENVIRONMENT_BYTES)
            & !1;
        if environment == 0 || size == 0 {
            return None;
        }
        let mut block = vec![0_u16; size / 2];
        self.read(environment, block.as_mut_ptr().cast(), size)
            .then_some(block)
    }

    #[cfg(not(target_pointer_width = "64"))]
    fn environment_block(&self) -> Option<Vec<u16>> {
        None
    }

    #[cfg(target_pointer_width = "64")]
    fn read_usize(&self, address: usize) -> Option<usize> {
        let mut value = 0_usize;
        self.read(address, (&raw mut value).cast(), size_of::<usize>())
            .then_some(value)
    }

    /// Copy exactly `len` bytes at `address` in the process into `into`.
    #[cfg(target_pointer_width = "64")]
    fn read(&self, address: usize, into: *mut c_void, len: usize) -> bool {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let mut read = 0_usize;
        // SAFETY: callers pass `into` writable for `len` bytes; the source is in the other process
        // and a bad address only fails the call.
        let ok = unsafe {
            ReadProcessMemory((self.0).0, address as *const c_void, into, len, &mut read)
        };
        ok != 0 && read == len
    }
}

/// What can be read about one snapshot entry. Whatever cannot be read is left empty; the process
/// is kept either way.
fn observe(entry: &TableEntry) -> ForegroundProcess {
    let mut process = ForegroundProcess {
        pid: entry.pid,
        started: None,
        name: entry.name.clone(),
        executable: None,
        argv: Vec::new(),
        agent_hint: None,
    };
    if let Ok(handle) = LimitedHandle::open(entry.pid) {
        process.started = handle.creation_time();
        process.executable = handle.image_path();
        process.argv = handle.argv();
    }
    process
}

/// A process handle opened with `PROCESS_QUERY_LIMITED_INFORMATION` and nothing else.
struct LimitedHandle(HANDLE);

impl LimitedHandle {
    /// The handle, or the error `OpenProcess` failed with. The error is read before anything
    /// else can run on this thread, since any later call, a `CloseHandle` included, may replace
    /// it. PID 0 is the idle process, which `OpenProcess` refuses with a misleading error, so it
    /// fails as `ERROR_ACCESS_DENIED`.
    fn open(pid: u32) -> Result<Self, u32> {
        if pid == 0 {
            return Err(ERROR_ACCESS_DENIED);
        }
        // SAFETY: plain FFI call; a null result is the failure value, with the reason in this
        // thread's last error, read at once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return Err(GetLastError());
            }
            Ok(Self(handle))
        }
    }

    /// The process's creation time as a FILETIME (100 ns intervals since 1601).
    fn creation_time(&self) -> Option<u64> {
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: the handle is open and every out-pointer is a live `FILETIME`.
        let ok = unsafe { GetProcessTimes(self.0, created, exited, kernel, user) } != 0;
        ok.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    fn image_path(&self) -> Option<String> {
        for capacity in [1024_usize, 32 * 1024] {
            let mut buffer = vec![0_u16; capacity];
            let mut len = capacity as u32;
            // SAFETY: `buffer` holds `len` UTF-16 units; on success `len` is the count written.
            let ok =
                unsafe { QueryFullProcessImageNameW(self.0, 0, buffer.as_mut_ptr(), &mut len) };
            if ok != 0 {
                return String::from_utf16(&buffer[..len as usize]).ok();
            }
            // SAFETY: plain FFI call reading this thread's last error.
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
        }
        None
    }

    /// The process's arguments, split the way the C runtime splits them. Empty when the command
    /// line cannot be read or is not valid UTF-16.
    fn argv(&self) -> Vec<String> {
        self.command_line()
            .filter(|line| !line.is_empty() && String::from_utf16(line).is_ok())
            .map(|line| split_command_line(&line))
            .unwrap_or_default()
    }

    /// The raw command line through `ProcessCommandLineInformation`, which a limited-information
    /// handle may query.
    fn command_line(&self) -> Option<Vec<u16>> {
        const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;
        const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
        const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;
        const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;
        const HEADER: usize = size_of::<UnicodeString>();

        let query = nt_query_information_process()?;
        let mut size: usize = 2048;
        for _ in 0..2 {
            // `u64` elements keep the `UNICODE_STRING` header at the start aligned.
            let mut buffer = vec![0_u64; size.div_ceil(8)];
            let bytes = buffer.len() * 8;
            let mut needed = 0_u32;
            // SAFETY: `buffer` is writable for `bytes` bytes and `needed` is a live `u32`.
            let status = unsafe {
                query(
                    self.0,
                    PROCESS_COMMAND_LINE_INFORMATION,
                    buffer.as_mut_ptr().cast(),
                    bytes as u32,
                    &mut needed,
                )
            };
            if status >= 0 {
                return unicode_string_in(&buffer);
            }
            if !matches!(
                status,
                STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL | STATUS_BUFFER_OVERFLOW
            ) {
                return None;
            }
            size = needed as usize;
            if size <= bytes || size > HEADER + MAX_COMMAND_LINE_BYTES {
                return None;
            }
        }
        None
    }
}

impl Drop for LimitedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed only here.
        unsafe { CloseHandle(self.0) };
    }
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// The string a `UNICODE_STRING` at the start of `buffer` describes, provided its characters lie
/// inside `buffer` too, as `ProcessCommandLineInformation` places them.
fn unicode_string_in(buffer: &[u64]) -> Option<Vec<u16>> {
    if buffer.len() * 8 < size_of::<UnicodeString>() {
        return None;
    }
    // SAFETY: `buffer` is 8-byte aligned and long enough for the header, checked above.
    let header = unsafe { &*buffer.as_ptr().cast::<UnicodeString>() };
    let len = usize::from(header.length);
    if len == 0 {
        return Some(Vec::new());
    }
    let start = buffer.as_ptr() as usize;
    let end = start + buffer.len() * 8;
    let chars = header.buffer as usize;
    if !len.is_multiple_of(2) || !chars.is_multiple_of(2) || chars < start || chars + len > end {
        return None;
    }
    // SAFETY: the characters were checked to lie inside `buffer` and to be `u16`-aligned.
    Some(unsafe { std::slice::from_raw_parts(header.buffer, len / 2) }.to_vec())
}

/// Split a command line with `CommandLineToArgvW`, bounded in count and length.
fn split_command_line(line: &[u16]) -> Vec<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    let mut terminated = Vec::with_capacity(line.len() + 1);
    terminated.extend_from_slice(line);
    terminated.push(0);
    let mut count = 0_i32;
    // SAFETY: `terminated` is NUL-terminated and non-empty: an empty line would make
    // `CommandLineToArgvW` describe this process instead, and callers never pass one.
    let args = unsafe { CommandLineToArgvW(terminated.as_ptr(), &mut count) };
    if args.is_null() {
        return Vec::new();
    }
    let count = usize::try_from(count).unwrap_or(0).min(MAX_ARG_COUNT);
    let argv = (0..count)
        .map(|index| {
            // SAFETY: `args` holds `count` NUL-terminated strings, each no longer than `line`.
            let arg = unsafe { *args.add(index) };
            let len = (0..line.len())
                // SAFETY: `len` stops at the NUL that ends each string.
                .find(|&offset| unsafe { *arg.add(offset) } == 0)
                .unwrap_or(line.len());
            // SAFETY: the first `len` units of `arg` were just read.
            let units = unsafe { std::slice::from_raw_parts(arg, len) };
            char::decode_utf16(units.iter().copied())
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .take(MAX_ARG_CHARS)
                .collect()
        })
        .collect();
    // SAFETY: `args` came from `CommandLineToArgvW`, which allocates with `LocalAlloc`.
    unsafe { LocalFree(args.cast()) };
    argv
}

type NtQueryInformationProcess =
    unsafe extern "system" fn(HANDLE, i32, *mut c_void, u32, *mut u32) -> i32;

/// `ntdll!NtQueryInformationProcess`, resolved once. ntdll is mapped into every process, so this
/// adds no import and no load.
fn nt_query_information_process() -> Option<NtQueryInformationProcess> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::w;

    static QUERY: OnceLock<Option<NtQueryInformationProcess>> = OnceLock::new();
    *QUERY.get_or_init(|| {
        // SAFETY: ntdll stays loaded for the life of the process, and the symbol has the
        // signature `NtQueryInformationProcess` documents.
        unsafe {
            let ntdll = GetModuleHandleW(w!("ntdll.dll"));
            if ntdll.is_null() {
                return None;
            }
            let symbol = GetProcAddress(ntdll, c"NtQueryInformationProcess".as_ptr().cast())?;
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                NtQueryInformationProcess,
            >(symbol))
        }
    })
}

fn wide_until_nul(units: &[u16]) -> String {
    let len = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

/// Whether `pid` has certainly exited: no such process, one that has finished but whose handle is
/// still held, or one whose creation time is not `started`, so the PID was reused. A process this
/// user may not open is not known to be gone.
pub(super) fn process_gone(pid: u32, started: Option<u64>) -> bool {
    if pid == 0 {
        return false;
    }
    let handle = match LimitedHandle::open(pid) {
        Ok(handle) => handle,
        Err(error) => return error == ERROR_INVALID_PARAMETER,
    };
    let mut code = 0;
    // SAFETY: the handle is open and `code` is a live `u32`.
    if unsafe { GetExitCodeProcess(handle.0, &mut code) } != 0 && code != STILL_ACTIVE as u32 {
        return true;
    }
    started.is_some_and(|started| handle.creation_time().is_some_and(|now| now != started))
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};

    use super::*;

    struct KillOnDrop(Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A child's arguments and creation time are read through a handle that cannot read its
    /// memory, and it is found in this process's descendant tree.
    #[test]
    fn a_child_is_read_through_a_limited_handle_and_found_in_the_tree() {
        let child = KillOnDrop(
            Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "Start-Sleep 30 # rozi tree probe"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("powershell starts"),
        );
        let pid = child.0.id();
        let handle = LimitedHandle::open(pid).expect("child opens");
        let argv = handle.argv();
        let started = handle.creation_time().expect("creation time reads");
        assert_eq!(
            argv.get(1..),
            Some(
                ["-NoProfile", "-Command", "Start-Sleep 30 # rozi tree probe"]
                    .map(String::from)
                    .as_slice()
            ),
            "argv {argv:?}"
        );
        assert!(!process_gone(pid, Some(started)));
        assert!(process_gone(pid, Some(started + 1)));

        let root = std::process::id();
        let set = ProcessSnapshot::capture()
            .descendant_set(root)
            .expect("this process is in the snapshot");
        assert_eq!(set.processes[0].process.pid, root);
        let found = set
            .processes
            .iter()
            .find(|observed| observed.process.pid == pid)
            .unwrap_or_else(|| panic!("child {pid} missing from {set:#?}"));
        assert_eq!(found.parent_pid, Some(root));
        assert!(found.process.name.eq_ignore_ascii_case("powershell.exe"));
        assert_eq!(found.process.started, Some(started));
        assert_eq!(found.process.argv, argv);
        assert!(
            found
                .process
                .executable
                .as_deref()
                .is_some_and(|path| path.to_ascii_lowercase().ends_with("\\powershell.exe")),
            "{found:#?}"
        );

        // Killed, reaped, and with every handle to it closed, the process no longer exists.
        drop(handle);
        drop(child);
        assert!(process_gone(pid, None), "an exited child is gone");
        assert!(process_gone(pid, Some(started)));
    }

    /// The pane marker is read out of another process's environment, and only that process's own
    /// marker is found there. A 32-bit process is skipped rather than read with the wrong layout.
    #[test]
    fn the_marker_is_read_from_a_child_environment() {
        let spawn = |program: &str, marker: &str| {
            let child = KillOnDrop(
                Command::new(program)
                    .args(["-n", "30", "127.0.0.1"])
                    .env(super::super::PANE_RUNTIME_ID_ENV, marker)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("ping starts"),
            );
            let pid = child.0.id();
            let started = LimitedHandle::open(pid)
                .ok()
                .and_then(|handle| handle.creation_time())
                .expect("creation time reads");
            (child, pid, started)
        };
        let (_a, a, a_started) = spawn("ping.exe", "marker-a");
        let (_b, b, b_started) = spawn("ping.exe", "marker-b");
        assert_eq!(read_marker(a, a_started).as_deref(), Some("marker-a"));
        assert!(carries_marker(a, a_started, "marker-a"));
        assert!(!carries_marker(b, b_started, "marker-a"));
        assert!(carries_marker(b, b_started, "marker-b"));
        assert_eq!(read_marker(a, a_started + 1), None, "another process");

        let windows = std::env::var("SystemRoot").expect("SystemRoot is set");
        let wow64 = format!(r"{windows}\SysWOW64\PING.EXE");
        if std::path::Path::new(&wow64).exists() {
            let (_c, c, c_started) = spawn(&wow64, "marker-c");
            assert_eq!(read_marker(c, c_started), None);
        }
    }
}
