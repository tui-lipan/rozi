//! Filesystem browsing served on the session server's host.
//!
//! The sidebar file tree normally enumerates the local filesystem itself. Under `--remote` the
//! files live on the server's machine, so the client asks the server to list them instead
//! ([`ClientMessage::ListDirectory`](crate::session::protocol::ClientMessage::ListDirectory)) and
//! feeds the result to the widget as a provided entry source.
//!
//! Git state is read by shelling out to `git` on this host — the server cannot borrow the client's
//! repository, and the framework's own git discovery only reads local paths.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::session::protocol::{WireChange, WireChangeState, WireDirEntry};

/// How many entries a single listing may return. Matches the widget's own per-directory cap so a
/// pathological directory cannot turn into a multi-megabyte frame.
const MAX_ENTRIES: usize = 10_000;
/// Git can report arbitrarily many paths; keep the result well below the wire frame limit. The byte
/// budget is conservative for JSON escaping, so a result admitted here remains protocol-safe.
const MAX_CHANGE_ENTRIES: usize = 4096;
const MAX_CHANGE_RESULT_BYTES: usize = 2 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
const GIT_CAPTURE_LIMIT: usize = 4 * 1024 * 1024;

/// List `path` on this host. Errors are returned as a message rather than failing the connection —
/// an unreadable directory is a normal thing to browse into.
pub(crate) fn list_directory(path: &str, show_hidden: bool) -> (Vec<WireDirEntry>, Option<String>) {
    let dir = Path::new(path);
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) => return (Vec::new(), Some(err.to_string())),
    };

    let status = git_status_for_dir(dir);
    let mut entries = Vec::new();
    for item in read.flatten() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        // `file_type` does not follow symlinks, so a link to a directory is reported as a link.
        // Resolve separately so a symlinked directory still expands.
        let file_type = item.file_type().ok();
        let is_symlink = file_type.is_some_and(|kind| kind.is_symlink());
        let is_dir = match file_type {
            Some(kind) if kind.is_dir() => true,
            Some(kind) if kind.is_symlink() => item.path().is_dir(),
            Some(_) => false,
            None => false,
        };
        // The link's own text, not a resolved path: it is what the tree shows after the name, and
        // resolving it here would spell a target that means nothing on the client's host.
        let symlink_target = is_symlink
            .then(|| std::fs::read_link(item.path()).ok())
            .flatten()
            .map(|target| target.to_string_lossy().into_owned());
        let entry_status = status.get(name.as_str()).copied();
        entries.push(WireDirEntry {
            name,
            is_dir,
            is_symlink,
            symlink_target,
            ignored: entry_status.is_some_and(|(_, _, ignored)| ignored),
            git_staged: entry_status.and_then(|(staged, _, _)| staged),
            git_unstaged: entry_status.and_then(|(_, unstaged, _)| unstaged),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    (entries, None)
}

/// Scan `root` for changed paths, for the tree's `Changes` projection.
pub(crate) fn list_changes(root: &str) -> (Vec<WireChange>, Option<String>) {
    let dir = Path::new(root);
    let output = match run_git(dir, &["status", "--porcelain=v1", "-z", "--no-renames"]) {
        Ok(output) => output,
        Err(err) => return (Vec::new(), Some(err)),
    };
    let (changes, truncated) = changes_from_porcelain(root, &output);
    if truncated {
        return (
            Vec::new(),
            Some("change list exceeds the server limit".to_string()),
        );
    }
    (changes, None)
}

fn changes_from_porcelain(root: &str, output: &[u8]) -> (Vec<WireChange>, bool) {
    let mut changes = Vec::new();
    let mut estimated_bytes = change_listing_base_bytes(root);
    for record in parse_porcelain_z(output) {
        // Staged wins the marker when a path is both, matching the local widget's precedence.
        let (state, is_staged) = match (record.staged, record.unstaged) {
            (Some(state), _) => (state, true),
            (None, Some(state)) => (state, false),
            (None, None) => continue,
        };
        let change = WireChange {
            path: record.path.to_string(),
            state,
            staged: is_staged,
        };
        let entry_bytes = change_entry_budget(&change.path);
        if changes.len() >= MAX_CHANGE_ENTRIES
            || estimated_bytes.saturating_add(entry_bytes) > MAX_CHANGE_RESULT_BYTES
        {
            return (changes, true);
        }
        estimated_bytes = estimated_bytes.saturating_add(entry_bytes);
        changes.push(change);
    }
    (changes, false)
}

fn change_listing_base_bytes(root: &str) -> usize {
    root.len().saturating_mul(6).saturating_add(128)
}

fn change_entry_budget(path: &str) -> usize {
    path.len().saturating_mul(6).saturating_add(64)
}

#[cfg(test)]
fn estimated_change_listing_bytes(root: &str, changes: &[WireChange]) -> usize {
    changes
        .iter()
        .fold(change_listing_base_bytes(root), |total, change| {
            total.saturating_add(change_entry_budget(&change.path))
        })
}

/// Per-direct-child git state for one directory: `(staged, unstaged, ignored)`.
///
/// `git status` reports repo-root-relative paths, so a nested change is attributed to the child
/// directory that contains it — that is what gives a collapsed directory row its marker.
type DirStatus = HashMap<String, (Option<WireChangeState>, Option<WireChangeState>, bool)>;

fn git_status_for_dir(dir: &Path) -> DirStatus {
    let mut map: DirStatus = HashMap::new();
    let Ok(output) = run_git(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--ignored=matching",
            "--",
            ".",
        ],
    ) else {
        return map;
    };
    for record in parse_porcelain_z(&output) {
        // Paths are relative to the repo root; `git -C dir` with a `.` pathspec keeps them under
        // this directory, so the first component is the direct child to attribute the state to.
        let Some(child) = record
            .path
            .split('/')
            .next()
            .filter(|part| !part.is_empty())
        else {
            continue;
        };
        let slot = map.entry(child.to_string()).or_insert((None, None, false));
        if slot.0.is_none() {
            slot.0 = record.staged;
        }
        if slot.1.is_none() {
            slot.1 = record.unstaged;
        }
        slot.2 |= record.ignored;
    }
    map
}

/// Run `git -C <dir> <args>`, returning stdout. Errors when git is missing or the path is not a
/// repository — both mean "no change decorations", never a hard failure.
fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    if !crate::platform::command::program_exists("git") {
        return Err("git was not found on the session server's PATH".to_string());
    }
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push(OsString::from("-C"));
    argv.push(dir.as_os_str().to_os_string());
    argv.extend(args.iter().map(OsString::from));
    let output = crate::platform::command::run_bounded_argv_command(
        "git",
        &argv,
        GIT_TIMEOUT,
        GIT_CAPTURE_LIMIT,
    )
    .map_err(|err| format!("git failed: {err}"))?;
    if output.timed_out {
        return Err("git timed out".to_string());
    }
    if output.status != Some(0) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

/// One parsed `git status --porcelain=v1 -z` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PorcelainRecord<'a> {
    staged: Option<WireChangeState>,
    unstaged: Option<WireChangeState>,
    /// `!!` — matched an ignore rule. Distinct from `??` (untracked), which shares the same state.
    ignored: bool,
    path: &'a str,
}

/// Parse `git status --porcelain=v1 -z` records.
///
/// `-z` is NUL-separated with no quoting, so a path containing a newline or a quote survives
/// intact — the reason this does not parse the default newline format.
fn parse_porcelain_z(bytes: &[u8]) -> Vec<PorcelainRecord<'_>> {
    let mut out = Vec::new();
    for record in bytes.split(|byte| *byte == 0) {
        if record.len() < 4 {
            continue;
        }
        let Ok(text) = std::str::from_utf8(record) else {
            continue;
        };
        let mut chars = text.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        // Format is exactly `XY<space>PATH`, so the path starts at 3 — not trimmed, because a
        // filename may legitimately begin with a space.
        let path = &text[3..];
        if path.is_empty() {
            continue;
        }
        let record = if x == '!' && y == '!' {
            PorcelainRecord {
                staged: None,
                unstaged: None,
                ignored: true,
                path,
            }
        } else if x == '?' && y == '?' {
            PorcelainRecord {
                staged: Some(WireChangeState::Untracked),
                unstaged: None,
                ignored: false,
                path,
            }
        } else if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            PorcelainRecord {
                staged: Some(WireChangeState::Conflicted),
                unstaged: None,
                ignored: false,
                path,
            }
        } else {
            PorcelainRecord {
                staged: state_from_code(x),
                unstaged: state_from_code(y),
                ignored: false,
                path,
            }
        };
        out.push(record);
    }
    out
}

fn state_from_code(code: char) -> Option<WireChangeState> {
    match code {
        'A' => Some(WireChangeState::Added),
        'M' => Some(WireChangeState::Modified),
        'D' => Some(WireChangeState::Deleted),
        'R' | 'C' => Some(WireChangeState::Renamed),
        '?' => Some(WireChangeState::Untracked),
        'U' => Some(WireChangeState::Conflicted),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_states() {
        let raw = b" M src/lib.rs\0A  src/new.rs\0?? scratch.txt\0UU merge.rs\0";
        let parsed = parse_porcelain_z(raw);
        assert_eq!(parsed[0].unstaged, Some(WireChangeState::Modified));
        assert_eq!(parsed[0].path, "src/lib.rs");
        assert_eq!(parsed[1].staged, Some(WireChangeState::Added));
        assert_eq!(parsed[2].staged, Some(WireChangeState::Untracked));
        assert_eq!(parsed[3].staged, Some(WireChangeState::Conflicted));
        assert!(parsed.iter().all(|record| !record.ignored));
    }

    /// `??` and `!!` both mean "not tracked", but only `!!` is ignored. An untracked *directory* is
    /// reported as `?? dir/`, so a trailing slash must never be read as the ignore signal.
    #[test]
    fn untracked_directory_is_not_mistaken_for_ignored() {
        let parsed = parse_porcelain_z(b"?? newdir/\0!! target/\0");
        assert_eq!(parsed[0].path, "newdir/");
        assert!(
            !parsed[0].ignored,
            "untracked directory must not be ignored"
        );
        assert_eq!(parsed[0].staged, Some(WireChangeState::Untracked));

        assert_eq!(parsed[1].path, "target/");
        assert!(parsed[1].ignored, "`!!` must be reported as ignored");
        assert_eq!(parsed[1].staged, None, "ignored is not a change");
    }

    #[test]
    fn porcelain_z_keeps_paths_with_newlines_intact() {
        let raw = b" M we\nird.txt\0";
        let parsed = parse_porcelain_z(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "we\nird.txt");
    }

    #[test]
    fn listing_reports_error_for_missing_directory() {
        let (entries, error) = list_directory("/definitely/not/here/rozi", false);
        assert!(entries.is_empty());
        assert!(error.is_some(), "missing directory must report an error");
    }

    #[test]
    fn listing_sorts_directories_first_and_honors_show_hidden() {
        let dir = std::env::temp_dir().join(format!("rozi-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), b"x").unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();

        let path = dir.to_string_lossy().into_owned();
        let (visible, error) = list_directory(&path, false);
        assert!(error.is_none());
        let names: Vec<&str> = visible.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha.txt"], "dirs first, then by name");

        let (all, _) = list_directory(&path, true);
        assert!(all.iter().any(|e| e.name == ".hidden"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The client cannot read a link that lives on this host, so the listing carries the link's own
    /// text for the tree to show after the name.
    #[cfg(unix)]
    #[test]
    fn listing_carries_a_symlinks_target() {
        let dir = std::env::temp_dir().join(format!("rozi-browse-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), b"guide").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.join("CLAUDE.md")).unwrap();

        let (entries, error) = list_directory(&dir.to_string_lossy(), false);
        assert!(error.is_none());
        let entry_named = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("{name} listed"))
        };

        let link = entry_named("CLAUDE.md");
        assert!(link.is_symlink);
        assert_eq!(link.symlink_target.as_deref(), Some("AGENTS.md"));
        assert_eq!(
            entry_named("AGENTS.md").symlink_target,
            None,
            "an ordinary file carries no target"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn change_listing_caps_count_and_wire_budget() {
        let mut count_raw = Vec::new();
        for index in 0..(MAX_CHANGE_ENTRIES + 100) {
            count_raw.extend_from_slice(format!("?? file-{index}\0").as_bytes());
        }
        let (count_limited, count_truncated) = changes_from_porcelain("/tmp", &count_raw);
        assert!(count_truncated);
        assert_eq!(count_limited.len(), MAX_CHANGE_ENTRIES);
        assert!(estimated_change_listing_bytes("/tmp", &count_limited) <= MAX_CHANGE_RESULT_BYTES);

        let long_path = "x".repeat(1_000);
        let mut size_raw = Vec::new();
        for _ in 0..MAX_CHANGE_ENTRIES {
            size_raw.extend_from_slice(b"?? ");
            size_raw.extend_from_slice(long_path.as_bytes());
            size_raw.push(0);
        }
        let (size_limited, size_truncated) = changes_from_porcelain("/tmp", &size_raw);
        assert!(size_truncated);
        assert!(size_limited.len() < MAX_CHANGE_ENTRIES);
        assert!(estimated_change_listing_bytes("/tmp", &size_limited) <= MAX_CHANGE_RESULT_BYTES);
    }
}
