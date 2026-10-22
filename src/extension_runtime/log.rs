//! What an extension's processes print, kept where they ran.
//!
//! A command's output used to be discarded and a service's too, so the only trace of a process that
//! died at startup was a toast. Each unit of an extension - the client-run commands, one service, one
//! placed command - now has a log on the host that ran it, bounded to [`CAP`] bytes plus one rotated
//! generation, with a line for every start and exit.
//!
//! Several clients on one host share these files, so every write takes an advisory lock on the
//! unit's lock file, then checks the size, rotates, and appends against the file as it is on disk
//! at that moment. Nothing about a log is remembered between writes.
//!
//! Logging never decides whether a process runs: a log that cannot be opened is no log, and the
//! process starts with its output discarded as before.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::platform::paths::{self, PlatformEnv};

/// Bytes a unit's live log may hold before it becomes the rotated one.
pub const CAP: u64 = 64 * 1024;

/// Longest a unit name stays in a file name.
const UNIT_NAME_MAX: usize = 64;

/// Hex digits of the hash that keeps a shortened or altered unit name distinct.
const HASH_DIGITS: usize = 16;

const ROTATED_SUFFIX: &str = "log.1";

/// How long an exit line waits for the process's last output to reach the log. A process that
/// left a descendant holding its pipe open would otherwise hold the line back for as long as that
/// descendant lives.
const DRAIN_WAIT: Duration = Duration::from_millis(250);

/// The longest a write or a read waits for another client to finish with a unit's files.
const LOCK_WAIT: Duration = Duration::from_millis(250);

/// How often a waiting write or read tries the lock again.
const LOCK_POLL: Duration = Duration::from_millis(2);

/// Every writer to one unit's file in this process.
static SINKS: Mutex<Option<HashMap<PathBuf, Weak<Mutex<Sink>>>>> = Mutex::new(None);

/// The way to one unit's log: its path and nothing else. The lock file, the log, and the directory
/// around them are all found by path on each write, never held, so a directory deleted and
/// recreated by hand cannot leave a writer locking a file that is no longer the one beside the log.
/// Removing an extension takes only the logs and leaves the lock files, so writers still share one.
#[derive(Debug)]
struct Sink {
    path: PathBuf,
}

impl Sink {
    fn open(path: PathBuf) -> io::Result<Self> {
        // Fail here, where the caller can decide to go without a log, if there is no lock to take.
        open_lock(&path)?;
        Ok(Self { path })
    }

    fn write(&mut self, bytes: &[u8]) {
        self.append(bytes, false);
    }

    fn note(&mut self, text: &str) {
        let line = format!(
            "[{}] {text}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        self.append(line.as_bytes(), true);
    }

    /// Add `bytes` to the unit's file. A failure - a full disk, a vanished directory - costs the
    /// log, never the process.
    ///
    /// With the lock, this measures, rotates, and appends. A lock that cannot be taken in time is
    /// not a reason to lose the output, but nor is it a licence to move files: without it the write
    /// only appends and never renames or truncates, so no generation can be lost to a client that
    /// still holds the lock mid-rotation. The file may then run past [`CAP`] until the next write
    /// that does hold the lock, which rotates it as it finds it.
    fn append(&mut self, bytes: &[u8], on_fresh_line: bool) {
        // Opened for this write, so it is the lock file beside the log as it is now, and closing
        // it releases the lock.
        let lock = open_lock(&self.path).ok();
        let locked = lock.as_ref().is_some_and(lock_for_a_while);
        let _ = append_to(&self.path, bytes, on_fresh_line, locked);
        if let (true, Some(lock)) = (locked, &lock) {
            let _ = lock.unlock();
        }
    }
}

/// The lock file of the log at `path`, created, and the directory with it if it is gone.
///
/// Read and write rather than append: Windows only locks a handle that can read or write.
fn open_lock(path: &Path) -> io::Result<File> {
    let lock_path = path.with_extension("lock");
    let open = || {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
    };
    match open() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                crate::platform::fs_security::ensure_private_dir(dir)?;
            }
            open()
        }
        other => other,
    }
}

/// Take the unit's lock, waiting at most [`LOCK_WAIT`]. A client that holds it suspended must not
/// hold up a caller that may be the UI, so a lock that is not free in time is given up on and the
/// caller goes ahead without it.
fn lock_for_a_while(lock: &File) -> bool {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return true,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_POLL);
            }
            Err(_) => return false,
        }
    }
}

/// The write itself. `locked` says the caller holds the unit's lock; only then may it rotate.
fn append_to(path: &Path, bytes: &[u8], on_fresh_line: bool, locked: bool) -> io::Result<()> {
    // A chunk larger than the cap keeps only its end, which is the part worth reading.
    let keep = usize::try_from(CAP).unwrap_or(usize::MAX);
    let bytes = &bytes[bytes.len().saturating_sub(keep)..];
    if bytes.is_empty() {
        return Ok(());
    }
    // Everything below about the file - its length, its last byte, where the write lands - goes
    // through this one handle, so it describes the same file the write targets even if another
    // client renames the path in the meantime. Reopening the path could find another file, or none.
    let mut file = open_for_append(path)?;
    let len = file.metadata()?.len();
    // An earlier process may have been cut off mid-line.
    let mut needs_newline = on_fresh_line && len > 0 && !ends_with_newline(&mut file, len)?;
    let size = len + u64::from(needs_newline) + bytes.len() as u64;
    if locked && size > CAP {
        // Another client may have rotated since this one last wrote, so the file is measured and
        // moved here, under the lock, never from a size remembered earlier.
        if fs::rename(path, path.with_extension(ROTATED_SUFFIX)).is_ok() {
            drop(file);
            file = open_for_append(path)?;
        } else {
            // Truncating needs write access, which an append handle does not have on Windows.
            OpenOptions::new().write(true).open(path)?.set_len(0)?;
        }
        // The new file starts clean; what the old one lacked is made up when the two are read.
        needs_newline = false;
    }
    let mut data = Vec::with_capacity(bytes.len() + 1);
    if needs_newline {
        data.push(b'\n');
    }
    data.extend_from_slice(bytes);
    file.write_all(&data)
}

/// Readable as well as appendable, so the same handle can say how the file ends.
fn open_for_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
}

/// Whether the file behind `file`, `len` bytes long, ends a line. Appending ignores where the
/// handle was left, so seeking to look does not move the write.
fn ends_with_newline(file: &mut File, len: u64) -> io::Result<bool> {
    file.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// One unit's log.
#[derive(Clone, Debug)]
pub struct Log {
    sink: Arc<Mutex<Sink>>,
}

/// Open the log of `unit` of `extension` on this host, or `None` when it cannot be.
pub fn open(extension: &str, unit: &str) -> Option<Log> {
    open_in(&PlatformEnv::from_process(), extension, unit)
}

fn open_in(env: &PlatformEnv, extension: &str, unit: &str) -> Option<Log> {
    let dir = paths::ensure_extension_log_dir(env, extension).ok()?;
    let path = dir.join(format!("{}.log", stem(unit)));
    let mut registry = SINKS.lock().unwrap_or_else(|error| error.into_inner());
    let registry = registry.get_or_insert_with(HashMap::new);
    registry.retain(|_, sink| sink.strong_count() > 0);
    if let Some(sink) = registry.get(&path).and_then(Weak::upgrade) {
        return Some(Log { sink });
    }
    let sink = Arc::new(Mutex::new(Sink::open(path.clone()).ok()?));
    registry.insert(path, Arc::downgrade(&sink));
    Some(Log { sink })
}

impl Log {
    /// Route both of `command`'s output streams to the log it is about to be given, or discard
    /// them when there is none.
    pub fn route(command: &mut Command, log: Option<&Self>) {
        command.stdout(pipe_or_null(log));
        command.stderr(pipe_or_null(log));
    }

    /// Route only standard error: the caller reads standard output itself.
    pub fn route_stderr(command: &mut Command, log: Option<&Self>) {
        command.stderr(pipe_or_null(log));
    }

    /// Start copying whichever of the child's output pipes exist into the log.
    pub fn attach(&self, child: &mut Child) -> Attached {
        let attached = self.attached();
        if let Some(stdout) = child.stdout.take() {
            self.pump(stdout, &attached.drain);
        }
        if let Some(stderr) = child.stderr.take() {
            self.pump(stderr, &attached.drain);
        }
        attached
    }

    /// Start copying the child's standard error into the log.
    pub fn attach_stderr(&self, child: &mut Child) -> Attached {
        let attached = self.attached();
        if let Some(stderr) = child.stderr.take() {
            self.pump(stderr, &attached.drain);
        }
        attached
    }

    /// Add a timestamped line of the supervisor's own: a start.
    pub fn note(&self, text: &str) {
        self.sink
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .note(text);
    }

    fn attached(&self) -> Attached {
        Attached {
            log: self.clone(),
            drain: Drain::default(),
        }
    }

    fn pump(&self, mut reader: impl Read + Send + 'static, drain: &Drain) {
        let sink = Arc::clone(&self.sink);
        let pending = drain.begin();
        std::thread::spawn(move || {
            let _pending = pending;
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .write(&chunk[..n]),
                }
            }
        });
    }
}

/// A running process's output pipes being copied into its log.
#[derive(Debug)]
pub struct Attached {
    log: Log,
    drain: Drain,
}

impl Attached {
    /// Record how the process ended, after its last output: the line is written once the pipes it
    /// was reading have closed, or after a short wait when something it started keeps them open.
    pub fn finish(&self, outcome: &str) {
        self.drain.wait(DRAIN_WAIT);
        self.log.note(outcome);
    }
}

/// Counts the pipe readers of one process that have not reached end of file.
#[derive(Clone, Debug, Default)]
struct Drain(Arc<(Mutex<usize>, Condvar)>);

impl Drain {
    fn begin(&self) -> Pending {
        let (count, _) = &*self.0;
        *count.lock().unwrap_or_else(|error| error.into_inner()) += 1;
        Pending(self.clone())
    }

    fn wait(&self, limit: Duration) {
        let (count, idle) = &*self.0;
        let guard = count.lock().unwrap_or_else(|error| error.into_inner());
        let _ = idle.wait_timeout_while(guard, limit, |pending| *pending > 0);
    }
}

/// One reader still copying; dropping it, however its thread ends, counts it as done.
struct Pending(Drain);

impl Drop for Pending {
    fn drop(&mut self) {
        let (count, idle) = &*(self.0).0;
        *count.lock().unwrap_or_else(|error| error.into_inner()) -= 1;
        idle.notify_all();
    }
}

fn pipe_or_null(log: Option<&Log>) -> Stdio {
    if log.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    }
}

/// The name of a unit: its kind and the public ID it has inside `extension`, without the
/// extension's own ID, which already names the log's directory. `service` and `git-tools.watch`
/// of `git-tools` make `service.watch`.
pub fn unit_name(kind: &str, extension: &str, public_id: &str) -> String {
    let local = public_id
        .strip_prefix(extension)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(public_id);
    format!("{kind}.{local}")
}

/// A unit's name as it appears in a file name. A name made only of ASCII letters, digits, `.`, `_`
/// and `-`, short enough, is used as it is. Any other is cut or altered to fit and given a hash
/// of the whole, so two different units never share a file.
pub fn stem(unit: &str) -> String {
    let safe: String = unit
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A leading dot would hide the file or, doubled, climb out of the directory.
    let safe = safe.trim_start_matches('.');
    if !safe.is_empty() && safe == unit && safe.len() <= UNIT_NAME_MAX {
        return safe.to_string();
    }
    let head: String = safe.chars().take(UNIT_NAME_MAX - HASH_DIGITS - 1).collect();
    let head = if head.is_empty() { "unit" } else { &head };
    format!("{head}-{:0width$x}", fnv1a(unit), width = HASH_DIGITS)
}

/// FNV-1a: fixed by definition, so a unit's file name never changes with the Rust release.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The units of `extension` that have a log on this host, sorted, as the names their files carry.
pub fn units(extension: &str) -> io::Result<Vec<String>> {
    units_in(&PlatformEnv::from_process(), extension)
}

fn units_in(env: &PlatformEnv, extension: &str) -> io::Result<Vec<String>> {
    let dir = log_dir(env, extension)?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    // Either generation names a unit. Between a rotation's rename and its new live file, and after
    // a crash in that gap, only the rotated one exists, and the unit must not vanish for it.
    let mut units: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name
                .strip_suffix(".log")
                .or_else(|| name.strip_suffix(".log.1"))?;
            Some(stem.to_string())
        })
        .collect();
    units.sort();
    units.dedup();
    Ok(units)
}

/// The last `lines` lines `unit` printed, oldest first, rotated generation included. `unit` is
/// the unit's name or the file name [`units`] gave for it.
pub fn tail(extension: &str, unit: &str, lines: usize) -> io::Result<Vec<String>> {
    tail_in(&PlatformEnv::from_process(), extension, unit, lines)
}

fn tail_in(
    env: &PlatformEnv,
    extension: &str,
    unit: &str,
    lines: usize,
) -> io::Result<Vec<String>> {
    let live = log_dir(env, extension)?.join(format!("{}.log", stem(unit)));
    let text = snapshot(&live)?;
    let all: Vec<&str> = text.lines().collect();
    Ok(all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|line| (*line).to_string())
        .collect())
}

/// Both generations of a unit's log, oldest first, read as one moment. A writer that rotates
/// between the two reads would otherwise take the file the first read missed out of both.
///
/// Held under the same lock writers take, so a rotation is wholly before or wholly after. A lock
/// that is not free in time is an error, [`io::ErrorKind::WouldBlock`], not a reason to read
/// anyway: unlocked, the two reads can straddle a rotation and return a tail that skips a
/// generation without saying so. Where there is no lock file, nothing ever wrote, and the files
/// are read as they are.
fn snapshot(live: &Path) -> io::Result<String> {
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(live.with_extension("lock"))
    {
        Ok(lock) => Some(lock),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(lock) = &lock
        && !lock_for_a_while(lock)
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "another rozi is writing this log right now; try again in a moment",
        ));
    }
    let mut text = String::new();
    let mut result = Ok(());
    for path in [live.with_extension(ROTATED_SUFFIX), live.to_path_buf()] {
        match read_lossy(&path) {
            Ok(part) => {
                // A generation can end mid-line, and the next must not continue it.
                if !text.is_empty() && !text.ends_with('\n') && !part.is_empty() {
                    text.push('\n');
                }
                text.push_str(&part);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    if let Some(lock) = &lock {
        let _ = lock.unlock();
    }
    result.map(|()| text)
}

fn read_lossy(path: &Path) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

fn log_dir(env: &PlatformEnv, extension: &str) -> io::Result<PathBuf> {
    if !paths::valid_extension_id(extension) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{extension}` is not an extension id"),
        ));
    }
    Ok(paths::extension_log_dir(env, extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, PlatformEnv) {
        let scratch = tempfile::tempdir().unwrap();
        let env = PlatformEnv {
            home: Some(scratch.path().to_path_buf()),
            xdg_data_home: Some(scratch.path().join("data")),
            local_appdata: Some(scratch.path().to_path_buf()),
            ..PlatformEnv::default()
        };
        (scratch, env)
    }

    #[test]
    fn a_plain_unit_name_is_its_own_file_name() {
        assert_eq!(stem("service.watch"), "service.watch");
        assert_eq!(stem("command.open-pr"), "command.open-pr");
        assert_eq!(stem("commands"), "commands");
    }

    #[test]
    fn a_unit_name_that_is_cut_or_altered_keeps_a_hash_of_the_whole() {
        let long_a = format!("command.{}a", "x".repeat(80));
        let long_b = format!("command.{}b", "x".repeat(80));
        let (a, b) = (stem(&long_a), stem(&long_b));
        assert_ne!(a, b, "two long names that share a prefix");
        assert!(a.len() <= UNIT_NAME_MAX && b.len() <= UNIT_NAME_MAX);
        assert_ne!(
            stem("a b"),
            stem("a_b"),
            "an altered name never meets a plain one"
        );
        for odd in ["command.a b/c", "../../etc", "", ".hidden", "ünï"] {
            let name = stem(odd);
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                    && !name.starts_with('.')
                    && !name.is_empty(),
                "{odd:?} -> {name:?}"
            );
            // A file name read back from `units` finds the same file.
            assert_eq!(stem(&name), name, "{odd:?}");
        }
        // Fixed by definition, so a log stays reachable across releases.
        assert_eq!(stem("a b"), format!("a_b-{:016x}", fnv1a("a b")));
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn a_unit_is_named_without_the_extension_that_already_names_its_directory() {
        assert_eq!(
            unit_name("service", "git-tools", "git-tools.watch"),
            "service.watch"
        );
        assert_eq!(
            unit_name("command", "git-tools", "git-tools.open"),
            "command.open"
        );
        // An ID that only starts with the extension's name is not inside it.
        assert_eq!(
            unit_name("tab", "git", "git-tools.board"),
            "tab.git-tools.board"
        );
        assert_eq!(unit_name("tab", "git", "board"), "tab.board");
    }

    #[test]
    fn a_log_keeps_the_end_and_never_outgrows_two_generations() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "service.noisy").unwrap();
        let line = format!("{}\n", "x".repeat(99));
        for index in 0..2000 {
            log.sink
                .lock()
                .unwrap()
                .write(format!("{index:04} {line}").as_bytes());
        }
        let dir = paths::extension_log_dir(&env, "tool");
        let live = fs::metadata(dir.join("service.noisy.log")).unwrap().len();
        let rotated = fs::metadata(dir.join("service.noisy.log.1")).unwrap().len();
        assert!(live <= CAP && rotated <= CAP, "{live} {rotated}");
        let tail = tail_in(&env, "tool", "service.noisy", 3).unwrap();
        assert!(tail[2].starts_with("1999 "), "{tail:?}");
    }

    /// Two clients on one host share the files and know nothing of each other's writes. Each
    /// stands here for one: a sink of its own, writing without regard to the other.
    #[test]
    fn writers_that_know_nothing_of_each_other_still_keep_the_bound() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let writers: Vec<_> = (0..4)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut sink = Sink::open(path).unwrap();
                    for line in 0..400 {
                        sink.write(format!("w{writer} {line:04} {}\n", "x".repeat(90)).as_bytes());
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let live = fs::metadata(&path).unwrap().len();
        let rotated = fs::metadata(path.with_extension(ROTATED_SUFFIX))
            .unwrap()
            .len();
        assert!(live <= CAP && rotated <= CAP, "{live} {rotated}");
        // Whole lines only: no write was torn by another's.
        for line in fs::read_to_string(&path).unwrap().lines() {
            assert!(
                line.len() == 8 + 90 && line.ends_with(&"x".repeat(90)),
                "{line:?}"
            );
        }
    }

    /// Windows refuses to lock a handle that can neither read nor write, which an append-only
    /// handle is, and a refused lock used to cost every write. The lock must work as opened.
    #[test]
    fn the_unit_lock_can_be_taken_and_released_as_soon_as_it_is_opened() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let lock = open_lock(&dir.join("commands.log")).unwrap();
        lock.lock().expect("the lock can be taken");
        lock.unlock().expect("and released");
        assert!(lock_for_a_while(&lock));
        lock.unlock().unwrap();
    }

    /// Removing an extension must not pull the lock out from under a writer that holds it: the next
    /// writer would then create and take a new lock file while the first still believes it owns the
    /// log. Removal takes the logs and leaves the lock files, so they all keep one lock.
    #[test]
    fn removing_an_extensions_logs_leaves_the_lock_every_writer_shares() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let mid_write = open_lock(&path).unwrap();
        mid_write.lock().expect("the lock is free");
        fs::write(&path, "output\n").unwrap();

        // `extensions remove` runs while that writer is between taking the lock and writing.
        assert!(paths::remove_extension_logs(&env, "tool").unwrap());
        assert!(!path.exists());

        // The next writer opens the lock file by path. It must be the same file, still held.
        let next = open_lock(&path).unwrap();
        match next.try_lock() {
            Err(TryLockError::WouldBlock) => {}
            other => panic!("the next writer took a lock the first one holds: {other:?}"),
        }
        mid_write.unlock().unwrap();
        next.lock().expect("free once the first writer is done");
    }

    /// A directory deleted by hand, rather than through `extensions remove`, is not a reason for a
    /// writer that outlives it to lock a stale file. The lock must be the one beside the log now.
    #[test]
    fn a_sink_outliving_its_directory_still_contends_on_the_current_lock_file() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let mut survivor = Sink::open(path.clone()).unwrap();
        survivor.write(b"before\n");

        // `extensions remove`, then the directory coming back for the same extension.
        fs::remove_dir_all(&dir).unwrap();
        paths::ensure_extension_log_dir(&env, "tool").unwrap();
        // Another client opens the unit afresh and takes its lock.
        let newcomer = open_lock(&path).unwrap();
        newcomer.lock().expect("the fresh lock is free");

        // The survivor must find that same lock file and be held off by it, not lock a deleted
        // one of its own: it gives up after the wait and writes append-only.
        let started = Instant::now();
        survivor.write(b"after\n");
        assert!(
            started.elapsed() >= LOCK_WAIT / 2,
            "{:?}",
            started.elapsed()
        );
        newcomer.unlock().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");
        // Free again, the survivor locks as usual.
        let again = Instant::now();
        survivor.write(b"and again\n");
        assert!(again.elapsed() < LOCK_WAIT, "{:?}", again.elapsed());
    }

    /// A write to a unit whose directory was removed brings the directory back.
    #[test]
    fn a_write_after_its_directory_was_removed_recreates_it_privately() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let mut sink = Sink::open(path.clone()).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        sink.write(b"back\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "back\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    /// Another client that holds a unit's lock and stops must not hold up this one for good.
    #[test]
    fn a_write_gives_up_on_a_lock_held_elsewhere_after_a_short_wait_and_still_lands() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let (held, release) = std::sync::mpsc::channel::<()>();
        let (taken, locked) = std::sync::mpsc::channel::<()>();
        let holder = {
            let path = path.clone();
            std::thread::spawn(move || {
                let other = open_lock(&path).unwrap();
                other.lock().unwrap();
                taken.send(()).unwrap();
                let _ = release.recv();
                other.unlock().unwrap();
            })
        };
        locked.recv().unwrap();
        let mut sink = Sink::open(path.clone()).unwrap();
        let started = Instant::now();
        sink.write(b"still lands\n");
        let waited = started.elapsed();
        held.send(()).unwrap();
        holder.join().unwrap();
        assert!(
            waited >= LOCK_WAIT / 2 && waited < Duration::from_secs(5),
            "{waited:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "still lands\n");
    }

    /// A write that gave up on the lock must not move files: the client holding it may be about to
    /// rotate on a decision it already made, and two rotations lose a generation.
    #[test]
    fn a_write_that_timed_out_on_the_lock_only_appends_and_loses_no_generation() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        let rotated = path.with_extension(ROTATED_SUFFIX);
        // Five bytes short of full, so the ten-byte write below would rotate if it were allowed to.
        let preexisting = format!("pre-{}\n", "x".repeat(usize::try_from(CAP).unwrap() - 10));
        fs::write(&path, &preexisting).unwrap();
        let (held, release) = std::sync::mpsc::channel::<()>();
        let (taken, locked) = std::sync::mpsc::channel::<()>();
        let holder = {
            let path = path.clone();
            std::thread::spawn(move || {
                let other = open_lock(&path).unwrap();
                other.lock().unwrap();
                taken.send(()).unwrap();
                let _ = release.recv();
                other.unlock().unwrap();
            })
        };
        locked.recv().unwrap();

        let mut sink = Sink::open(path.clone()).unwrap();
        sink.write(b"timed-out\n");
        assert!(!rotated.exists(), "a write without the lock moved the log");
        assert!(
            fs::metadata(&path).unwrap().len() > CAP,
            "the fixture should leave the file past the cap"
        );

        held.send(()).unwrap();
        holder.join().unwrap();
        // The next write that holds the lock rotates the file as it finds it.
        sink.write(b"after\n");
        assert!(rotated.exists());
        let kept = tail_in(&env, "tool", "commands", 10).unwrap();
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert!(
            kept[0].starts_with("pre-"),
            "the earlier generation was lost"
        );
        assert_eq!(kept[1..], ["timed-out", "after"]);
    }

    /// A reader that arrives while another client is rotating must see the rotation finished,
    /// not the moment between moving the file and starting the new one.
    #[test]
    fn a_read_taken_mid_rotation_waits_for_it_and_sees_both_generations() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        fs::write(&path, "old\n").unwrap();
        let (moved, mid_rotation) = std::sync::mpsc::channel::<()>();
        let rotator = {
            let path = path.clone();
            std::thread::spawn(move || {
                let other = open_lock(&path).unwrap();
                other.lock().unwrap();
                fs::rename(&path, path.with_extension(ROTATED_SUFFIX)).unwrap();
                moved.send(()).unwrap();
                // Within the wait a reader gives a lock, so it sees the rotation finished.
                std::thread::sleep(LOCK_WAIT / 5);
                fs::write(&path, "new\n").unwrap();
                other.unlock().unwrap();
            })
        };
        mid_rotation.recv().unwrap();
        let lines = tail_in(&env, "tool", "commands", 10).unwrap();
        rotator.join().unwrap();
        assert_eq!(lines, ["old", "new"]);
    }

    /// Read unlocked, the two generations can straddle a rotation and the tail skips one without
    /// saying so. A reader that cannot get the lock says so instead.
    #[test]
    fn a_read_that_cannot_get_the_lock_reports_it_busy_not_a_tail_that_may_skip_a_generation() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        fs::write(&path, "line\n").unwrap();
        let (held, release) = std::sync::mpsc::channel::<()>();
        let (taken, locked) = std::sync::mpsc::channel::<()>();
        let holder = {
            let path = path.clone();
            std::thread::spawn(move || {
                let other = open_lock(&path).unwrap();
                other.lock().unwrap();
                taken.send(()).unwrap();
                let _ = release.recv();
                other.unlock().unwrap();
            })
        };
        locked.recv().unwrap();
        let started = Instant::now();
        let busy = tail_in(&env, "tool", "commands", 10).unwrap_err();
        let waited = started.elapsed();
        held.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(busy.kind(), io::ErrorKind::WouldBlock, "{busy}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
        // Once the lock is free it reads as before.
        assert_eq!(tail_in(&env, "tool", "commands", 10).unwrap(), ["line"]);
    }

    /// The write targets the file it opened, so what it learns about that file must come from the
    /// same handle: another client can rename the path between the open and the check.
    #[test]
    fn what_a_write_learns_about_the_file_comes_from_its_own_handle_not_the_path() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        fs::write(&path, "cut off mid-li").unwrap();
        let mut handle = open_for_append(&path).unwrap();
        let len = handle.metadata().unwrap().len();
        // Another client rotates the file under it.
        fs::rename(&path, path.with_extension(ROTATED_SUFFIX)).unwrap();
        assert!(!path.exists());
        assert!(!ends_with_newline(&mut handle, len).unwrap());
        // And the write still lands, in the file the handle was opened on.
        handle.write_all(b"ne\n").unwrap();
        assert_eq!(
            fs::read_to_string(path.with_extension(ROTATED_SUFFIX)).unwrap(),
            "cut off mid-line\n"
        );
    }

    /// Between a rotation's rename and its new live file only the rotated generation exists. The
    /// unit is still there, and `logs` must neither miss it nor list it twice.
    #[test]
    fn a_unit_whose_live_file_does_not_exist_yet_is_still_listed_once() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        fs::write(dir.join("commands.log.1"), "kept\n").unwrap();
        assert_eq!(units_in(&env, "tool").unwrap(), ["commands"]);
        assert_eq!(tail_in(&env, "tool", "commands", 5).unwrap(), ["kept"]);

        // With both generations, and a lock file beside them, it is still one unit.
        fs::write(dir.join("commands.log"), "live\n").unwrap();
        fs::write(dir.join("commands.lock"), "").unwrap();
        fs::write(dir.join("service.watch.log"), "other\n").unwrap();
        assert_eq!(
            units_in(&env, "tool").unwrap(),
            ["commands", "service.watch"].map(String::from)
        );
    }

    /// A generation that ends mid-line must not be continued by the next.
    #[test]
    fn generations_are_joined_on_a_line_boundary() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        let path = dir.join("commands.log");
        fs::write(path.with_extension(ROTATED_SUFFIX), "ends mid-line").unwrap();
        fs::write(&path, "starts fresh\n").unwrap();
        assert_eq!(
            tail_in(&env, "tool", "commands", 10).unwrap(),
            ["ends mid-line", "starts fresh"]
        );
    }

    /// Rotating a file that was cut off mid-line must not leave a blank first line in the new one.
    #[test]
    fn a_rotation_after_an_unterminated_line_starts_the_new_file_clean() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "commands").unwrap();
        let almost_full = "x".repeat(usize::try_from(CAP).unwrap() - 5);
        log.sink.lock().unwrap().write(almost_full.as_bytes());
        log.note("exited with status 1");
        let dir = paths::extension_log_dir(&env, "tool");
        let live = fs::read_to_string(dir.join("commands.log")).unwrap();
        assert!(live.starts_with('['), "{live:?}");
        assert!(fs::metadata(dir.join("commands.log.1")).is_ok());
    }

    /// With no client writing, a reader finds no lock to take and reads what is there.
    #[test]
    fn a_read_of_a_log_with_no_lock_file_still_reads_it() {
        let (_scratch, env) = env();
        let dir = paths::ensure_extension_log_dir(&env, "tool").unwrap();
        fs::write(dir.join("commands.log"), "only\n").unwrap();
        assert_eq!(tail_in(&env, "tool", "commands", 10).unwrap(), ["only"]);
    }

    #[test]
    fn notes_start_on_a_fresh_line_after_output_that_did_not_end_one() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "commands").unwrap();
        log.sink.lock().unwrap().write(b"partial");
        log.note("exited with status 1");
        let lines = tail_in(&env, "tool", "commands", 10).unwrap();
        assert_eq!(lines[0], "partial");
        assert!(lines[1].ends_with("] exited with status 1"), "{lines:?}");
    }

    #[test]
    fn writers_to_one_unit_share_a_sink() {
        let (_scratch, env) = env();
        let first = open_in(&env, "tool", "commands").unwrap();
        let second = open_in(&env, "tool", "commands").unwrap();
        assert!(Arc::ptr_eq(&first.sink, &second.sink));
        let other = open_in(&env, "tool", "service.x").unwrap();
        assert!(!Arc::ptr_eq(&first.sink, &other.sink));
    }

    #[test]
    fn units_and_tails_read_back_what_was_written() {
        let (_scratch, env) = env();
        assert!(units_in(&env, "tool").unwrap().is_empty());
        open_in(&env, "tool", "service.b")
            .unwrap()
            .note("started pid 1");
        open_in(&env, "tool", "commands").unwrap().note("started");
        assert_eq!(
            units_in(&env, "tool").unwrap(),
            ["commands", "service.b"].map(String::from)
        );
        assert!(units_in(&env, "../tool").is_err());
        assert!(tail_in(&env, "tool", "missing", 5).unwrap().is_empty());
    }

    #[test]
    fn a_long_unit_is_found_by_its_name_and_by_the_file_name_listed_for_it() {
        let (_scratch, env) = env();
        let long = format!("command.{}", "y".repeat(120));
        open_in(&env, "tool", &long).unwrap().note("started");
        let listed = units_in(&env, "tool").unwrap();
        assert_eq!(listed, [stem(&long)]);
        assert_eq!(tail_in(&env, "tool", &long, 5).unwrap().len(), 1);
        assert_eq!(tail_in(&env, "tool", &listed[0], 5).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_process_output_lands_in_its_log_before_the_line_saying_it_ended() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "commands").unwrap();
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "echo to-out; echo to-err >&2; sleep 0.2; echo last-words",
        ]);
        Log::route(&mut command, Some(&log));
        let mut child = command.spawn().unwrap();
        let attached = log.attach(&mut child);
        child.wait().unwrap();
        attached.finish("exited with status 0");
        let lines = tail_in(&env, "tool", "commands", 10).unwrap();
        let position = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} in {lines:?}"))
        };
        assert!(position("to-out") < position("exited"));
        assert!(position("to-err") < position("exited"));
        assert!(position("last-words") < position("exited"));
    }

    /// A descendant that outlives the process and keeps its output open must not hold the exit line
    /// back indefinitely.
    #[cfg(unix)]
    #[test]
    fn a_descendant_holding_the_pipe_open_delays_the_exit_line_only_briefly() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "commands").unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5 & echo parent-done"]);
        Log::route(&mut command, Some(&log));
        let mut child = command.spawn().unwrap();
        let attached = log.attach(&mut child);
        child.wait().unwrap();
        let started = std::time::Instant::now();
        attached.finish("exited with status 0");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let lines = tail_in(&env, "tool", "commands", 10).unwrap();
        assert!(
            lines.last().unwrap().ends_with("exited with status 0"),
            "{lines:?}"
        );
    }
}
