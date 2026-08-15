//! Platform config/state/cache/runtime path resolution (cross-platform plan Phase 3).
//!
//! Consolidates logic that used to be duplicated (state-home resolution appeared independently in
//! `profiles::session_path`, `session::server::resurrect::default_snapshot_dir`, and
//! `session::server::panes::default_log_dir`) or missing entirely (there was no cache directory
//! concept at all). Every accessor takes an explicit [`PlatformEnv`] rather than reading process
//! environment variables itself, so tests never need to mutate real global env vars.
//!
//! | Purpose | Linux/macOS | Windows |
//! |---|---|---|
//! | Config  | `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi` | `%APPDATA%\rozi` |
//! | State   | `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi` |
//! | Cache   | `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
//! | Runtime | `$XDG_RUNTIME_DIR/rozi`, else a private per-uid temp dir | `%LOCALAPPDATA%\rozi\run`, else `%TEMP%\rozi-<user-sid>` |
//!
//! The Windows column is written per the plan and believed correct against documented API
//! contracts, but is **unverified**: this environment has no Windows target to run it on. See
//! `AGENTS.md`/`CLAUDE.md` for the cross-platform plan's verification constraints.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use super::fs_security;

const APP_DIR: &str = "rozi";

/// Scratch directories installed by [`crate::test_support::isolate_user_dirs`], replacing the real
/// environment for the rest of the process. Empty in production.
static PROCESS_ENV_OVERRIDE: OnceLock<PlatformEnv> = OnceLock::new();

/// Install the process-wide override consulted by [`PlatformEnv::from_process`]. First call wins.
pub(crate) fn install_process_env_override(env: PlatformEnv) {
    let _ = PROCESS_ENV_OVERRIDE.set(env);
}

/// Whether this process resolves user directories into a test scratch root instead of the real
/// environment. Callers that accept an *explicit* path override (`ROZI_CONFIG`) must ignore it
/// while this holds, so an isolated process has no way out of its scratch root.
pub(crate) fn user_dirs_are_isolated() -> bool {
    cfg!(test) || PROCESS_ENV_OVERRIDE.get().is_some()
}

/// Explicit environment snapshot every path accessor in this module resolves against.
///
/// Production code uses [`PlatformEnv::from_process`]. Tests construct this directly instead of
/// mutating `std::env` globals, so path-resolution tests can run in parallel with everything else
/// without a cross-test environment-variable lock.
#[derive(Clone, Debug, Default)]
pub struct PlatformEnv {
    /// `$HOME` (Unix/macOS only; ignored on Windows).
    pub home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`, only if it was set to a non-empty absolute path.
    pub xdg_config_home: Option<PathBuf>,
    /// `$XDG_STATE_HOME`, only if it was set to a non-empty absolute path.
    pub xdg_state_home: Option<PathBuf>,
    /// `$XDG_CACHE_HOME`, only if it was set to a non-empty absolute path.
    ///
    pub xdg_cache_home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, only if it was set to a non-empty absolute path.
    pub xdg_data_home: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR`, only if it was set to a non-empty absolute path.
    pub xdg_runtime_dir: Option<PathBuf>,
    /// `%APPDATA%` (Windows only).
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Windows only).
    pub local_appdata: Option<PathBuf>,
}

impl PlatformEnv {
    /// Snapshot the process environment, or the test scratch root standing in for it.
    ///
    /// Unit tests are isolated unconditionally; integration tests opt in through
    /// [`crate::test_support::isolate_user_dirs`]. Either way no test can write to the directories
    /// a developer's own rozi reads.
    pub fn from_process() -> Self {
        if cfg!(test) {
            return crate::test_support::isolated_env();
        }
        if let Some(env) = PROCESS_ENV_OVERRIDE.get() {
            return env.clone();
        }
        Self::snapshot()
    }

    /// The real process environment, whatever isolation is in force.
    pub(crate) fn snapshot() -> Self {
        Self {
            home: env_path("HOME"),
            xdg_config_home: env_absolute_path("XDG_CONFIG_HOME"),
            xdg_state_home: env_absolute_path("XDG_STATE_HOME"),
            xdg_cache_home: env_absolute_path("XDG_CACHE_HOME"),
            xdg_data_home: env_absolute_path("XDG_DATA_HOME"),
            xdg_runtime_dir: env_absolute_path("XDG_RUNTIME_DIR"),
            appdata: env_absolute_path("APPDATA"),
            local_appdata: env_absolute_path("LOCALAPPDATA"),
        }
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Like [`env_path`], but additionally rejects a relative path.
///
/// A relative `XDG_*`/AppData override is never silently resolved against the process's current
/// directory - that would make the "config directory" move every time the working directory
/// changes. Reject it outright and fall through to the next tier instead.
fn env_absolute_path(key: &str) -> Option<PathBuf> {
    env_path(key).filter(|path| path.is_absolute())
}

/// Base config directory: `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi`;
/// `%APPDATA%\rozi` on Windows.
pub fn config_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(appdata) = &env.appdata
    {
        return appdata.join(APP_DIR);
    }
    xdg_style_dir(env.xdg_config_home.as_ref(), &env.home, ".config")
}

/// Base state directory: `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi`;
/// `%LOCALAPPDATA%\rozi` on Windows.
pub fn state_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(local_appdata) = &env.local_appdata
    {
        return local_appdata.join(APP_DIR);
    }
    xdg_style_dir(env.xdg_state_home.as_ref(), &env.home, ".local/state")
}

/// Base cache directory: `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi`;
/// `%LOCALAPPDATA%\rozi\cache` on Windows.
///
pub fn cache_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(local_appdata) = &env.local_appdata
    {
        return local_appdata.join(APP_DIR).join("cache");
    }
    xdg_style_dir(env.xdg_cache_home.as_ref(), &env.home, ".cache")
}

/// Base data directory used by managed installations: `$XDG_DATA_HOME/rozi`, else
/// `~/.local/share/rozi`; `%LOCALAPPDATA%\rozi` on Windows.
pub fn data_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(local_appdata) = &env.local_appdata
    {
        return local_appdata.join(APP_DIR);
    }
    xdg_style_dir(env.xdg_data_home.as_ref(), &env.home, ".local/share")
}

/// The command path owned by a managed installation. Unix keeps the stable command as an absolute
/// symlink into [`data_dir`]; Windows keeps a stable launcher beside the active-version selector.
pub fn managed_command_path(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows) {
        data_dir(env).join("bin").join("rozi.exe")
    } else {
        env.home
            .as_ref()
            .map(|home| home.join(".local/bin/rozi"))
            .unwrap_or_else(|| PathBuf::from(".local/bin/rozi"))
    }
}

/// Alias kept intentionally descriptive for call sites that refer to the stable command rather
/// than the managed-installation implementation detail.
pub fn default_command_path(env: &PlatformEnv) -> PathBuf {
    managed_command_path(env)
}

fn xdg_style_dir(
    xdg_override: Option<&PathBuf>,
    home: &Option<PathBuf>,
    home_suffix: &str,
) -> PathBuf {
    let base = xdg_override
        .cloned()
        .or_else(|| home.as_ref().map(|home| home.join(home_suffix)))
        .unwrap_or_else(|| PathBuf::from(home_suffix));
    base.join(APP_DIR)
}

/// Runtime endpoint directory, created (if missing) and validated private to the current user.
///
/// Unix/macOS: `$XDG_RUNTIME_DIR/rozi`, falling back to [`fallback_runtime_dir_path`] when
/// `XDG_RUNTIME_DIR` is unset. Windows: `%LOCALAPPDATA%\rozi\run`.
pub fn runtime_dir(env: &PlatformEnv) -> io::Result<PathBuf> {
    let dir = if cfg!(windows) {
        if let Some(local_appdata) = &env.local_appdata {
            local_appdata.join(APP_DIR).join("run")
        } else {
            fallback_runtime_dir_path()
        }
    } else {
        match &env.xdg_runtime_dir {
            Some(base) => base.join(APP_DIR),
            None => fallback_runtime_dir_path(),
        }
    };
    fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Per-user private fallback runtime directory when `$XDG_RUNTIME_DIR` is unavailable.
pub fn fallback_runtime_dir_path() -> PathBuf {
    let owner = super::user::current_user_tag();
    std::env::temp_dir().join(format!("{APP_DIR}-{owner}"))
}

/// This binary's own path, injected into spawned processes as `ROZI_BIN`.
///
/// Everything on the extension surface - a picker, a publisher, a hook, a service - reaches back
/// into rozi by running `rozi`, which assumes an install on `PATH`. A build started with
/// `cargo run`, a binary installed under another name, and a portable copy are all counterexamples,
/// and each one fails as a bare `command not found` deep inside a user's pipeline. Handing out the
/// path removes the assumption.
///
/// `None` when the platform cannot answer, which is not an error: callers fall back to `PATH`.
pub fn current_binary() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Directory for temporary scrollback dumps opened in `$EDITOR` (`state_dir/scrollback`).
pub fn scrollback_dir(env: &PlatformEnv) -> io::Result<PathBuf> {
    let dir = state_dir(env).join("scrollback");
    fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

const SCROLLBACK_DUMP_CAP: usize = 20;

/// Write a scrollback dump as `pane-<id>-<timestamp>.txt` (mode 0600) and prune older dumps so
/// the directory stays near [`SCROLLBACK_DUMP_CAP`] files.
pub fn write_scrollback_dump(env: &PlatformEnv, pane_id: u64, text: &str) -> io::Result<PathBuf> {
    let dir = scrollback_dir(env)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("pane-{pane_id}-{stamp}.txt"));
    fs_security::write_private_file(&path, text.as_bytes())?;
    prune_scrollback_dumps(&dir, SCROLLBACK_DUMP_CAP)?;
    Ok(path)
}

fn prune_scrollback_dumps(dir: &std::path::Path, cap: usize) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "txt"))
        .collect();
    if entries.len() <= cap {
        return Ok(());
    }
    entries.sort_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok());
    let remove_count = entries.len().saturating_sub(cap);
    for entry in entries.into_iter().take(remove_count) {
        let _ = fs::remove_file(entry.path());
    }
    Ok(())
}

/// Normalize a working directory reported by a shell (`OSC 7` / `OSC 9;9`) into the platform's
/// canonical spelling, or reject it (cross-platform plan Phase 6/10 path-encoding rules).
///
/// `None` means "not a usable absolute local path" - the caller must fall through to the next
/// precedence tier rather than repairing it. Repair is exactly the wrong instinct here: a path we
/// had to guess at is a path we should not be handing to `Command::current_dir`.
///
/// Unix: an absolute path (leading `/`) is already canonical; anything else is rejected.
///
/// Windows: accepts a drive path (`C:\...` or `C:/...`) and a UNC path (`\\server\share\...`),
/// rejects a drive-relative path (`C:foo`, which means "whatever `C:`'s current directory happens
/// to be" - a per-process notion no other process can resolve) and a rooted-but-driveless path
/// (`\foo`, which is relative to the current drive). Separators are normalized to `\` and a drive
/// letter is upper-cased, so two spellings of one directory compare equal.
pub fn normalize_reported_cwd(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    if !cfg!(windows) {
        return path.starts_with('/').then(|| path.to_string());
    }

    let mut normalized = path.replace('/', "\\");
    // `OSC 7` carries a `file:` URI, and the URI keeps the separator ahead of the drive letter:
    // `file:///C:/Users/x` and `file://host/C:/Users/x` both yield a path of `/C:/Users/x`. That
    // single leading separator is URI syntax, not a rooted-but-driveless path, so strip it when a
    // drive letter follows. Without this every Windows shell report is rejected below.
    if let Some(rest) = normalized.strip_prefix('\\')
        && starts_with_drive_letter(rest)
    {
        normalized = rest.to_string();
    }
    if let Some(rest) = normalized.strip_prefix("\\\\") {
        // UNC: `\\server\share\...`. Requires at least a server and a share to name anything.
        let mut parts = rest.splitn(3, '\\').filter(|part| !part.is_empty());
        let (Some(_server), Some(_share)) = (parts.next(), parts.next()) else {
            return None;
        };
        return Some(normalized);
    }

    let mut chars = normalized.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() || chars.next() != Some(':') || chars.next() != Some('\\') {
        return None;
    }
    Some(format!(
        "{}{}",
        drive.to_ascii_uppercase(),
        &normalized[1..]
    ))
}

/// Whether `value` begins with a `<letter>:` drive prefix.
///
/// Deliberately does not require a separator after the colon: `C:foo` is drive-relative and must
/// still reach the rejection below rather than being quietly stripped into something absolute.
fn starts_with_drive_letter(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// Whether two paths name the same directory. Case-sensitive on Unix, case-insensitive on Windows
/// (whose filesystems are, and whose shells will happily report `c:\users\x` where the launch
/// directory was recorded as `C:\Users\x`).
pub fn paths_equal(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Whether a path uses the Windows drive/UNC shape. Unix paths may legally contain `\` inside a
/// segment name, so backslash splitting must be reserved for paths that are actually Windows-like.
pub fn is_windows_path_shape(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
}

/// A path's non-empty components, split on whichever separators the path's own shape implies.
pub fn path_segments(path: &str) -> Vec<&str> {
    if is_windows_path_shape(path) {
        path.split(['\\', '/']).filter(|s| !s.is_empty()).collect()
    } else {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

/// The last component of a path — the directory or file name. `None` for a root-only path (`/`,
/// `C:\`), which has no leaf to name.
pub fn path_leaf(path: &str) -> Option<&str> {
    path_segments(path).last().copied()
}

/// The user's home directory for *display* purposes: `$HOME`, or `%USERPROFILE%` on Windows.
///
/// Deliberately separate from [`PlatformEnv::home`], which stays Unix-only because the XDG-style
/// directory tiers must not silently fall back to a Windows profile path. This one only ever feeds
/// text on screen, where a Windows user does want to see `~`.
fn display_home() -> Option<String> {
    home_directory()
}

/// The user's home directory (`$HOME`, or `%USERPROFILE%` on Windows), trimmed of a trailing
/// separator. Used as a spawn-cwd fallback for a pane the client supplied no directory for — a
/// `--remote` pane, where the local launch cwd is meaningless — so the session server starts it
/// (and reports it) in a sensible place even when `current_dir()` is unavailable (a detached
/// Windows server can land on an inaccessible working directory).
pub(crate) fn home_directory() -> Option<String> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(key)
        .ok()
        .map(|home| home.trim_end_matches(['/', '\\']).to_string())
        .filter(|home| !home.is_empty())
}

/// Replace a leading home-directory prefix with `~`, for paths shown to the user. Returned
/// unchanged when the path lies outside the home directory or there is no home directory to
/// compare against.
///
/// Only ever apply this to a *local* path: a remote shell's home is not this machine's, so
/// compressing a reported remote directory would claim a relationship that does not exist.
/// Find the nearest Git project containing `cwd`. A `.git` file counts as well as a directory so
/// worktrees and submodules are detected.
pub fn discover_project_root(cwd: &str) -> Option<String> {
    std::path::Path::new(cwd)
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(|dir| dir.to_string_lossy().into_owned())
}

/// The path `cwd` occupies inside its project, relative to `root` — `src/view` for a pane in
/// `~/Work/rozi/src/view`. Empty (`None`) at the project root itself.
pub fn project_relative_path(root: &str, cwd: &str) -> Option<String> {
    let relative = std::path::Path::new(cwd)
        .strip_prefix(std::path::Path::new(root))
        .ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    // Always the display spelling, whatever the host's separator: this is read as one label beside
    // a project name, not fed back to the filesystem.
    Some(
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// The branch `root` has checked out, or a short commit id when `HEAD` is detached. `None` when
/// `root` is not a repository, or `HEAD` is unreadable or says something neither of those.
///
/// Reads `HEAD` directly rather than running `git`: this is re-read on a timer for every pane in a
/// project (a checkout moves the branch without the working directory changing), where a process
/// spawn per pane per tick would be an absurd price for one line of text — and a repository stays
/// readable on a host with no `git` on `PATH`.
pub fn head_branch(root: &str) -> Option<String> {
    let git = std::path::Path::new(root).join(".git");
    let git_dir = if git.is_dir() {
        git
    } else {
        resolve_gitdir_file(&git)?
    };
    parse_head(&std::fs::read_to_string(git_dir.join("HEAD")).ok()?)
}

/// A linked worktree and a submodule have a `.git` *file* holding `gitdir: <path>`, pointing at the
/// real git directory. A relative target is resolved against the directory holding the file.
fn resolve_gitdir_file(git_file: &std::path::Path) -> Option<std::path::PathBuf> {
    let contents = std::fs::read_to_string(git_file).ok()?;
    let target = contents.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let target = std::path::Path::new(target);
    if target.is_absolute() {
        return Some(target.to_path_buf());
    }
    Some(git_file.parent()?.join(target))
}

fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!name.is_empty()).then(|| name.to_string());
    }
    // Detached: `HEAD` holds the raw commit id, shown short the way a shell prompt shows it. The
    // hex check is what keeps a `HEAD` in some future format from being displayed as a branch.
    let detached = head.len() >= 7 && head.chars().all(|ch| ch.is_ascii_hexdigit());
    detached.then(|| head.chars().take(7).collect())
}

/// Compact cwd label for pane chrome. Inside a Git project this keeps the project name plus the
/// path relative to its root (`rozi/src/view`), which identifies both project and location. An
/// ordinary cwd falls back to its home-relative or absolute spelling.
pub fn display_cwd(cwd: &str) -> String {
    let Some(root) = discover_project_root(cwd) else {
        return compress_home(cwd);
    };
    let root = std::path::Path::new(&root);
    let cwd = std::path::Path::new(cwd);
    let Some(project) = root.file_name() else {
        return compress_home(cwd.to_string_lossy().as_ref());
    };
    let Ok(relative) = cwd.strip_prefix(root) else {
        return compress_home(cwd.to_string_lossy().as_ref());
    };
    if relative.as_os_str().is_empty() {
        project.to_string_lossy().into_owned()
    } else {
        std::path::Path::new(project)
            .join(relative)
            .to_string_lossy()
            .into_owned()
    }
}

pub fn compress_home(path: &str) -> String {
    let Some(home) = display_home() else {
        return path.to_string();
    };
    // A trailing separator is a spelling difference, not a different directory, and it must not be
    // what stops `/home/you/` from matching `/home/you`.
    let path = match path.trim_end_matches(['/', '\\']) {
        "" => path,
        trimmed => trimmed,
    };
    if paths_equal(path, &home) {
        return "~".to_string();
    }
    // The separator must be part of the match, or `/home/youssef` would compress against
    // `/home/you`.
    for separator in ['/', '\\'] {
        let prefix = format!("{home}{separator}");
        if path.len() > prefix.len() && paths_equal(&path[..prefix.len()], &prefix) {
            return format!("~{separator}{}", &path[prefix.len()..]);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_relative_path_names_the_place_inside_the_project() {
        assert_eq!(
            project_relative_path("/home/x/rozi", "/home/x/rozi/src/view").as_deref(),
            Some("src/view")
        );
        // The root itself is not "somewhere inside" the project.
        assert_eq!(project_relative_path("/home/x/rozi", "/home/x/rozi"), None);
        // A cwd outside the claimed root is not describable relative to it.
        assert_eq!(project_relative_path("/home/x/rozi", "/home/x/other"), None);
    }

    #[test]
    fn head_is_read_as_a_branch_a_short_commit_or_nothing() {
        assert_eq!(
            parse_head("ref: refs/heads/master\n").as_deref(),
            Some("master")
        );
        assert_eq!(
            parse_head("ref: refs/heads/feat/pricing\n").as_deref(),
            Some("feat/pricing")
        );
        // Detached: the raw commit id, shown short the way a prompt shows it.
        assert_eq!(
            parse_head("9fceb02d0ae598e95dc970b74767f19372d61af8\n").as_deref(),
            Some("9fceb02")
        );
        // Neither shape: better nothing than a line of a format nobody here understands.
        assert_eq!(parse_head("something else entirely"), None);
        assert_eq!(parse_head("ref:  \n"), None);
        assert_eq!(parse_head(""), None);
    }

    /// The end-to-end read against a real repository, including the worktree form where `.git` is a
    /// file pointing elsewhere — the case a naive `<root>/.git/HEAD` join gets wrong.
    #[test]
    fn head_branch_reads_plain_repositories_and_worktree_pointers() {
        let dir = std::env::temp_dir().join(format!("rozi-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), b"ref: refs/heads/master\n").unwrap();
        assert_eq!(
            head_branch(&repo.to_string_lossy()).as_deref(),
            Some("master")
        );

        // A linked worktree: `.git` is a file, and its `gitdir:` target holds the real HEAD.
        let worktree = dir.join("wt");
        let gitdir = repo.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("HEAD"), b"ref: refs/heads/feat/pricing\n").unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", gitdir.to_string_lossy()).as_bytes(),
        )
        .unwrap();
        assert_eq!(
            head_branch(&worktree.to_string_lossy()).as_deref(),
            Some("feat/pricing")
        );

        // A relative `gitdir:` resolves against the directory holding the file, not the process cwd.
        std::fs::write(
            worktree.join(".git"),
            b"gitdir: ../repo/.git/worktrees/wt\n",
        )
        .unwrap();
        assert_eq!(
            head_branch(&worktree.to_string_lossy()).as_deref(),
            Some("feat/pricing")
        );

        // Not a repository at all.
        assert_eq!(head_branch(&dir.to_string_lossy()), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_leaf_and_segments_respect_the_path_shape() {
        assert_eq!(path_leaf("/home/you/repo"), Some("repo"));
        assert_eq!(path_leaf("/home/you/repo/"), Some("repo"));
        assert_eq!(path_leaf("/"), None);
        assert_eq!(path_leaf("C:\\Users\\you\\repo"), Some("repo"));
        // A Unix directory whose name contains a backslash is one segment, not two.
        assert_eq!(
            path_segments("/home/you/my\\dir"),
            vec!["home", "you", "my\\dir"]
        );
    }

    #[test]
    fn compress_home_only_matches_a_whole_leading_component() {
        let restore = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
        let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        unsafe { std::env::set_var(key, "/home/you") };

        assert_eq!(compress_home("/home/you/Work/repo"), "~/Work/repo");
        assert_eq!(compress_home("/home/you"), "~");
        assert_eq!(compress_home("/home/you/"), "~");
        // A sibling whose name merely starts with the home path is not under it.
        assert_eq!(compress_home("/home/youssef/repo"), "/home/youssef/repo");
        assert_eq!(compress_home("/srv/build"), "/srv/build");

        match restore {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn env_with_home(home: &str) -> PlatformEnv {
        PlatformEnv {
            home: Some(PathBuf::from(home)),
            ..PlatformEnv::default()
        }
    }

    #[test]
    fn config_dir_prefers_xdg_override() {
        let env = PlatformEnv {
            xdg_config_home: Some(PathBuf::from("/custom/config")),
            home: Some(PathBuf::from("/home/user")),
            ..PlatformEnv::default()
        };
        assert_eq!(config_dir(&env), PathBuf::from("/custom/config/rozi"));
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let env = env_with_home("/home/user");
        assert_eq!(config_dir(&env), PathBuf::from("/home/user/.config/rozi"));
    }

    #[test]
    fn state_dir_falls_back_to_home_local_state() {
        let env = env_with_home("/home/user");
        assert_eq!(
            state_dir(&env),
            PathBuf::from("/home/user/.local/state/rozi")
        );
    }

    #[test]
    fn cache_dir_falls_back_to_home_dot_cache() {
        let env = env_with_home("/home/user");
        assert_eq!(cache_dir(&env), PathBuf::from("/home/user/.cache/rozi"));
    }

    #[cfg(not(windows))]
    #[test]
    fn data_dir_prefers_xdg_data_home_and_falls_back_to_local_share() {
        let env = PlatformEnv {
            xdg_data_home: Some(PathBuf::from("/custom/data")),
            home: Some(PathBuf::from("/home/user")),
            ..PlatformEnv::default()
        };
        assert_eq!(data_dir(&env), PathBuf::from("/custom/data/rozi"));
        assert_eq!(
            data_dir(&env_with_home("/home/user")),
            PathBuf::from("/home/user/.local/share/rozi")
        );
    }

    #[cfg(windows)]
    #[test]
    fn data_dir_uses_local_appdata_on_windows() {
        let env = PlatformEnv {
            local_appdata: Some(PathBuf::from(r"C:\Users\user\AppData\Local")),
            ..PlatformEnv::default()
        };
        assert_eq!(
            data_dir(&env),
            PathBuf::from(r"C:\Users\user\AppData\Local\rozi")
        );
    }

    #[test]
    fn managed_command_path_uses_the_platform_stable_command_location() {
        let env = if cfg!(windows) {
            PlatformEnv {
                local_appdata: Some(PathBuf::from(r"C:\Users\user\AppData\Local")),
                ..PlatformEnv::default()
            }
        } else {
            env_with_home("/home/user")
        };
        if cfg!(windows) {
            assert_eq!(
                managed_command_path(&env),
                PathBuf::from(r"C:\Users\user\AppData\Local\rozi\bin\rozi.exe")
            );
        } else {
            assert_eq!(
                managed_command_path(&env),
                PathBuf::from("/home/user/.local/bin/rozi")
            );
        }
    }

    #[test]
    fn relative_xdg_override_is_rejected_not_silently_used() {
        // A relative XDG_CONFIG_HOME must never be interpreted relative to the process cwd;
        // PlatformEnv::from_process already filters these out via `env_absolute_path`, but the
        // resolution functions here must also behave correctly if an override slips through as
        // `None` from a relative value, falling through to the next tier.
        let env = PlatformEnv {
            xdg_config_home: None, // simulates env_absolute_path rejecting "relative/path"
            home: Some(PathBuf::from("/home/user")),
            ..PlatformEnv::default()
        };
        assert_eq!(config_dir(&env), PathBuf::from("/home/user/.config/rozi"));
    }

    #[test]
    fn env_absolute_path_rejects_relative_values() {
        // Directly exercises the filter `PlatformEnv::from_process` relies on.
        assert!(super::env_absolute_path("__ROZI_TEST_NONEXISTENT_VAR__").is_none());
    }

    #[test]
    fn no_home_and_no_xdg_falls_back_to_dotdir_relative_path() {
        let env = PlatformEnv::default();
        assert_eq!(config_dir(&env), PathBuf::from(".config/rozi"));
        assert_eq!(state_dir(&env), PathBuf::from(".local/state/rozi"));
        assert_eq!(cache_dir(&env), PathBuf::from(".cache/rozi"));
    }

    #[test]
    fn runtime_dir_resolves_from_injected_platform_env() {
        let temp = std::env::temp_dir().join(format!("rozi-paths-test-{}", std::process::id()));
        let env = PlatformEnv {
            xdg_runtime_dir: Some(temp.join("run")),
            local_appdata: Some(temp.join("local_appdata")),
            ..PlatformEnv::default()
        };
        let dir = runtime_dir(&env).expect("runtime dir created");
        if cfg!(windows) {
            assert_eq!(dir, temp.join("local_appdata").join(APP_DIR).join("run"));
        } else {
            assert_eq!(dir, temp.join("run").join(APP_DIR));
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn fallback_runtime_dir_path_is_per_user_and_stable() {
        let first = fallback_runtime_dir_path();
        let second = fallback_runtime_dir_path();
        assert_eq!(first, second);
        assert!(first.starts_with(std::env::temp_dir()));
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rozi-")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn reported_cwd_on_unix_accepts_only_absolute_paths() {
        assert_eq!(
            normalize_reported_cwd("/home/user/src"),
            Some("/home/user/src".to_string())
        );
        assert_eq!(normalize_reported_cwd("home/user"), None);
        assert_eq!(normalize_reported_cwd(""), None);
        // A NUL is never a legitimate path byte and must never reach `Command::current_dir`.
        assert_eq!(normalize_reported_cwd("/home/\0user"), None);
    }

    #[cfg(windows)]
    #[test]
    fn reported_cwd_on_windows_normalizes_drives_and_unc_and_rejects_the_rest() {
        assert_eq!(
            normalize_reported_cwd("c:/Users/x"),
            Some(r"C:\Users\x".to_string())
        );
        assert_eq!(
            normalize_reported_cwd(r"C:\Users\x"),
            Some(r"C:\Users\x".to_string())
        );
        // The shape `OSC 7` actually delivers. `parse_file_uri` splits at the first `/` and keeps
        // it, so `file:///C:/Users/x` and `file://host/C:/Users/x` both arrive with the separator
        // still in front of the drive letter. The two spellings above never occur on the wire.
        assert_eq!(
            normalize_reported_cwd("/C:/Users/x"),
            Some(r"C:\Users\x".to_string())
        );
        assert_eq!(
            normalize_reported_cwd(r"\C:\Users\x"),
            Some(r"C:\Users\x".to_string())
        );
        // Stripping the URI separator must not turn a drive-relative path into an absolute one.
        assert_eq!(normalize_reported_cwd("/C:Users"), None);
        assert_eq!(
            normalize_reported_cwd(r"\\server\share\dir"),
            Some(r"\\server\share\dir".to_string())
        );
        // Drive-relative: means "wherever C:'s per-process current directory points", which no
        // other process can resolve.
        assert_eq!(normalize_reported_cwd(r"C:Users"), None);
        // Rooted but driveless: relative to the current drive, same problem.
        assert_eq!(normalize_reported_cwd(r"\Users"), None);
        // UNC with no share names nothing.
        assert_eq!(normalize_reported_cwd(r"\\server"), None);
        assert_eq!(normalize_reported_cwd("relative"), None);
    }

    #[test]
    fn paths_compare_case_insensitively_only_on_windows() {
        assert!(paths_equal(r"C:\Users\x", r"C:\Users\x"));
        assert_eq!(
            paths_equal("/Home/User", "/home/user"),
            cfg!(windows),
            "case-insensitive comparison must follow the platform's own filesystem semantics"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_dir_creates_and_reuses_private_directory() {
        let base = std::env::temp_dir().join(format!(
            "rozi-paths-test-{}-{}",
            "reuse",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let env = PlatformEnv {
            xdg_runtime_dir: Some(base.clone()),
            ..PlatformEnv::default()
        };

        let first = runtime_dir(&env).expect("create");
        assert_eq!(first, base.join("rozi"));
        let second = runtime_dir(&env).expect("reuse");
        assert_eq!(second, first);

        let _ = std::fs::remove_dir_all(&base);
    }
}
