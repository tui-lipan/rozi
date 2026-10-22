//! What an extension's processes print, kept where they ran.
//!
//! A command's output used to be discarded and a service's too, so the only trace of a process that
//! died at startup was a toast. Each unit of an extension - the client-run commands, one service, one
//! placed command - now has a log on the host that ran it, bounded to [`CAP`] bytes plus one rotated
//! generation, with a line for every start and exit.
//!
//! Logging never decides whether a process runs: a log that cannot be opened is no log, and the
//! process starts with its output discarded as before.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, Weak};

use crate::platform::paths::{self, PlatformEnv};

/// Bytes a unit's live log may hold before it becomes the rotated one.
pub const CAP: u64 = 64 * 1024;

/// Longest a unit name stays in a file name.
const UNIT_NAME_MAX: usize = 64;

const ROTATED_SUFFIX: &str = "log.1";

/// Every writer to one unit's file in this process, so rotation sees all of them.
static SINKS: Mutex<Option<HashMap<PathBuf, Weak<Mutex<Sink>>>>> = Mutex::new(None);

struct Sink {
    path: PathBuf,
    file: File,
    len: u64,
    at_line_start: bool,
}

impl Sink {
    fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            len,
            // A previous run may have been cut off mid-line.
            at_line_start: len == 0,
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        // A chunk larger than the cap keeps only its end, which is the part worth reading.
        let keep = usize::try_from(CAP).unwrap_or(usize::MAX);
        let bytes = &bytes[bytes.len().saturating_sub(keep)..];
        if bytes.is_empty() {
            return;
        }
        if self.len + bytes.len() as u64 > CAP {
            self.rotate();
        }
        // A full disk or a vanished directory costs the log, never the process.
        if self.file.write_all(bytes).is_ok() {
            self.len += bytes.len() as u64;
            self.at_line_start = bytes.ends_with(b"\n");
        }
    }

    fn rotate(&mut self) {
        let rotated = self.path.with_extension(ROTATED_SUFFIX);
        let _ = fs::rename(&self.path, &rotated);
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.file = file;
            self.len = 0;
            self.at_line_start = true;
        }
    }

    fn note(&mut self, text: &str) {
        let mut line = String::new();
        if !self.at_line_start {
            line.push('\n');
        }
        line.push_str(&format!(
            "[{}] {text}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        self.write(line.as_bytes());
    }
}

/// One unit's log.
#[derive(Clone)]
pub struct Log {
    sink: Arc<Mutex<Sink>>,
}

/// Open the log of `unit` of `extension` on this host, or `None` when it cannot be.
pub fn open(extension: &str, unit: &str) -> Option<Log> {
    open_in(&PlatformEnv::from_process(), extension, unit)
}

fn open_in(env: &PlatformEnv, extension: &str, unit: &str) -> Option<Log> {
    let dir = paths::ensure_extension_log_dir(env, extension).ok()?;
    let path = dir.join(format!("{}.log", file_stem(unit)));
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
    pub fn attach(&self, child: &mut Child) {
        if let Some(stdout) = child.stdout.take() {
            self.pump(stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            self.pump(stderr);
        }
    }

    /// Start copying the child's standard error into the log.
    pub fn attach_stderr(&self, child: &mut Child) {
        if let Some(stderr) = child.stderr.take() {
            self.pump(stderr);
        }
    }

    /// Add a timestamped line of the supervisor's own: a start, an exit.
    pub fn note(&self, text: &str) {
        self.sink
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .note(text);
    }

    fn pump(&self, mut reader: impl Read + Send + 'static) {
        let sink = Arc::clone(&self.sink);
        std::thread::spawn(move || {
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

fn pipe_or_null(log: Option<&Log>) -> Stdio {
    if log.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    }
}

/// Record how a unit's process ended. Opens the log itself so a supervisor that kept no handle can
/// still say so.
pub fn note_exit(extension: &str, unit: &str, outcome: &str) {
    if let Some(log) = open(extension, unit) {
        log.note(outcome);
    }
}

/// A unit's name as it appears in a file name: ASCII letters, digits, `.`, `_` and `-` only.
fn file_stem(unit: &str) -> String {
    let stem: String = unit
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(UNIT_NAME_MAX)
        .collect();
    match stem.trim_start_matches('.') {
        "" => "unit".to_string(),
        trimmed => trimmed.to_string(),
    }
}

/// The units of `extension` that have a log on this host, sorted.
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

/// The last `lines` lines `unit` printed, oldest first, rotated generation included.
pub fn tail(extension: &str, unit: &str, lines: usize) -> io::Result<Vec<String>> {
    tail_in(&PlatformEnv::from_process(), extension, unit, lines)
}

fn tail_in(
    env: &PlatformEnv,
    extension: &str,
    unit: &str,
    lines: usize,
) -> io::Result<Vec<String>> {
    let live = log_dir(env, extension)?.join(format!("{}.log", file_stem(unit)));
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
    fn a_unit_name_becomes_one_safe_file_name() {
        assert_eq!(file_stem("service.watcher"), "service.watcher");
        assert_eq!(file_stem("command.a b/c"), "command.a_b_c");
        assert_eq!(file_stem("../../etc"), "_.._etc");
        assert_eq!(file_stem(""), "unit");
        assert_eq!(file_stem(&"x".repeat(200)).len(), UNIT_NAME_MAX);
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

    #[cfg(unix)]
    #[test]
    fn a_process_output_lands_in_its_log_from_both_streams() {
        let (_scratch, env) = env();
        let log = open_in(&env, "tool", "commands").unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "echo to-out; echo to-err >&2"]);
        Log::route(&mut command, Some(&log));
        let mut child = command.spawn().unwrap();
        log.attach(&mut child);
        child.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let lines = tail_in(&env, "tool", "commands", 10).unwrap();
            if lines.iter().any(|l| l == "to-out") && lines.iter().any(|l| l == "to-err") {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{lines:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
