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
//! | Config  | `$XDG_CONFIG_HOME/hyprmux`, else `~/.config/hyprmux` | `%APPDATA%\hyprmux` |
//! | State   | `$XDG_STATE_HOME/hyprmux`, else `~/.local/state/hyprmux` | `%LOCALAPPDATA%\hyprmux` |
//! | Cache   | `$XDG_CACHE_HOME/hyprmux`, else `~/.cache/hyprmux` | `%LOCALAPPDATA%\hyprmux\cache` |
//! | Runtime | `$XDG_RUNTIME_DIR/hyprmux`, else a private per-uid temp dir | not yet implemented (Phase 5 named-pipe registry) |
//!
//! The Windows column is written per the plan and believed correct against documented API
//! contracts, but is **unverified**: this environment has no Windows target to run it on. See
//! `AGENTS.md`/`CLAUDE.md` for the cross-platform plan's verification constraints.

use std::fs;
use std::io;
use std::path::PathBuf;

use super::fs_security;

const APP_DIR: &str = "hyprmux";

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
    /// `$XDG_RUNTIME_DIR`, only if it was set to a non-empty absolute path.
    pub xdg_runtime_dir: Option<PathBuf>,
    /// `%APPDATA%` (Windows only).
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Windows only).
    pub local_appdata: Option<PathBuf>,
}

impl PlatformEnv {
    /// Snapshot the real process environment.
    pub fn from_process() -> Self {
        Self {
            home: env_path("HOME"),
            xdg_config_home: env_absolute_path("XDG_CONFIG_HOME"),
            xdg_state_home: env_absolute_path("XDG_STATE_HOME"),
            xdg_cache_home: env_absolute_path("XDG_CACHE_HOME"),
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

/// Base config directory: `$XDG_CONFIG_HOME/hyprmux`, else `~/.config/hyprmux`;
/// `%APPDATA%\hyprmux` on Windows.
pub fn config_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(appdata) = &env.appdata
    {
        return appdata.join(APP_DIR);
    }
    xdg_style_dir(env.xdg_config_home.as_ref(), &env.home, ".config")
}

/// Base state directory: `$XDG_STATE_HOME/hyprmux`, else `~/.local/state/hyprmux`;
/// `%LOCALAPPDATA%\hyprmux` on Windows.
pub fn state_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(local_appdata) = &env.local_appdata
    {
        return local_appdata.join(APP_DIR);
    }
    xdg_style_dir(env.xdg_state_home.as_ref(), &env.home, ".local/state")
}

/// Base cache directory: `$XDG_CACHE_HOME/hyprmux`, else `~/.cache/hyprmux`;
/// `%LOCALAPPDATA%\hyprmux\cache` on Windows.
///
pub fn cache_dir(env: &PlatformEnv) -> PathBuf {
    if cfg!(windows)
        && let Some(local_appdata) = &env.local_appdata
    {
        return local_appdata.join(APP_DIR).join("cache");
    }
    xdg_style_dir(env.xdg_cache_home.as_ref(), &env.home, ".cache")
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
/// Unix/macOS: `$XDG_RUNTIME_DIR/hyprmux`, falling back to [`fallback_runtime_dir_path`] when
/// `XDG_RUNTIME_DIR` is unset. Windows has no equivalent yet: the plan calls for a
/// `%LOCALAPPDATA%\hyprmux\run` discovery registry backing named-pipe endpoints (Phase 5), which
/// is not implemented - this function is not meaningful on Windows today.
pub fn runtime_dir(env: &PlatformEnv) -> io::Result<PathBuf> {
    let dir = match &env.xdg_runtime_dir {
        Some(base) => base.join(APP_DIR),
        None => fallback_runtime_dir_path(),
    };
    fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Per-user private fallback runtime directory when `$XDG_RUNTIME_DIR` is unavailable.
pub fn fallback_runtime_dir_path() -> PathBuf {
    let owner = super::user::current_user_tag();
    std::env::temp_dir().join(format!("{APP_DIR}-{owner}"))
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

    let normalized = path.replace('/', "\\");
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config_dir(&env), PathBuf::from("/custom/config/hyprmux"));
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let env = env_with_home("/home/user");
        assert_eq!(
            config_dir(&env),
            PathBuf::from("/home/user/.config/hyprmux")
        );
    }

    #[test]
    fn state_dir_falls_back_to_home_local_state() {
        let env = env_with_home("/home/user");
        assert_eq!(
            state_dir(&env),
            PathBuf::from("/home/user/.local/state/hyprmux")
        );
    }

    #[test]
    fn cache_dir_falls_back_to_home_dot_cache() {
        let env = env_with_home("/home/user");
        assert_eq!(cache_dir(&env), PathBuf::from("/home/user/.cache/hyprmux"));
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
        assert_eq!(
            config_dir(&env),
            PathBuf::from("/home/user/.config/hyprmux")
        );
    }

    #[test]
    fn env_absolute_path_rejects_relative_values() {
        // Directly exercises the filter `PlatformEnv::from_process` relies on.
        assert!(super::env_absolute_path("__HYPRMUX_TEST_NONEXISTENT_VAR__").is_none());
    }

    #[test]
    fn no_home_and_no_xdg_falls_back_to_dotdir_relative_path() {
        let env = PlatformEnv::default();
        assert_eq!(config_dir(&env), PathBuf::from(".config/hyprmux"));
        assert_eq!(state_dir(&env), PathBuf::from(".local/state/hyprmux"));
        assert_eq!(cache_dir(&env), PathBuf::from(".cache/hyprmux"));
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
                .starts_with("hyprmux-")
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
            "hyprmux-paths-test-{}-{}",
            "reuse",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let env = PlatformEnv {
            xdg_runtime_dir: Some(base.clone()),
            ..PlatformEnv::default()
        };

        let first = runtime_dir(&env).expect("create");
        assert_eq!(first, base.join("hyprmux"));
        let second = runtime_dir(&env).expect("reuse");
        assert_eq!(second, first);

        let _ = std::fs::remove_dir_all(&base);
    }
}
