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
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

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

/// Every writer to one unit's file in this process, so they share one lock handle.
static SINKS: Mutex<Option<HashMap<PathBuf, Weak<Mutex<Sink>>>>> = Mutex::new(None);

#[derive(Debug)]
struct Sink {
    path: PathBuf,
    lock: File,
}

impl Sink {
    fn open(path: PathBuf) -> io::Result<Self> {
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.with_extension("lock"))?;
        Ok(Self { path, lock })
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

    /// Add `bytes` to the unit's file. A failure - a full disk, a vanished directory, a lock that
    /// cannot be taken - costs the log, never the process.
    fn append(&mut self, bytes: &[u8], on_fresh_line: bool) {
        if self.lock.lock().is_err() {
            return;
        }
        let _ = append_locked(&self.path, bytes, on_fresh_line);
        let _ = self.lock.unlock();
    }
}

/// The write itself, with the unit's lock held by the caller.
fn append_locked(path: &Path, bytes: &[u8], on_fresh_line: bool) -> io::Result<()> {
    // A chunk larger than the cap keeps only its end, which is the part worth reading.
    let keep = usize::try_from(CAP).unwrap_or(usize::MAX);
    let bytes = &bytes[bytes.len().saturating_sub(keep)..];
    if bytes.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let len = file.metadata()?.len();
    let mut data = Vec::with_capacity(bytes.len() + 1);
    // An earlier process may have been cut off mid-line.
    if on_fresh_line && len > 0 && !ends_with_newline(path, len)? {
        data.push(b'\n');
    }
    data.extend_from_slice(bytes);
    if len + data.len() as u64 > CAP {
        // Another client may have rotated since this one last wrote, so the file is measured and
        // moved here, under the lock, never from a size remembered earlier.
        if fs::rename(path, path.with_extension(ROTATED_SUFFIX)).is_ok() {
            drop(file);
            file = OpenOptions::new().create(true).append(true).open(path)?;
        } else {
            file.set_len(0)?;
        }
    }
    file.write_all(&data)
}

fn ends_with_newline(path: &Path, len: u64) -> io::Result<bool> {
    let mut file = File::open(path)?;
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
    let mut units: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".log")?;
            Some(stem.to_string())
        })
        .collect();
    units.sort();
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
    let mut text = String::new();
    for path in [live.with_extension(ROTATED_SUFFIX), live] {
        match read_lossy(&path) {
            Ok(part) => text.push_str(&part),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let all: Vec<&str> = text.lines().collect();
    Ok(all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|line| (*line).to_string())
        .collect())
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
