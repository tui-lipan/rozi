//! Scratch user directories for test processes.
//!
//! Test binaries run inside a developer's real environment, and hyprmux persists preferences as a
//! side effect of ordinary actions: toggling the sidebar writes `[sidebar]` through
//! [`crate::config::persist`], and leaving a session writes `session.toml` through
//! [`crate::profiles`]. Without isolation those writes land on the developer's own
//! `~/.config/hyprmux/hyprmux.toml` - and a running hyprmux watches that file, so the test's state
//! is live-reloaded into the UI the developer is working in.
//!
//! Isolation redirects every directory [`crate::platform::paths`] resolves - config, state, cache,
//! data, runtime - into a per-process scratch root, and makes that root inescapable: while it is
//! installed, an ambient `HYPRMUX_CONFIG` is ignored too (see [`crate::config::config_path`]).
//!
//! Unit tests get this for free - [`PlatformEnv::from_process`] isolates itself under `cfg(test)`.
//! Integration tests link the non-test build of the library, so a test binary that builds a
//! `HyprmuxApp` and dispatches actions must call [`isolate_user_dirs`] first.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::platform::paths::PlatformEnv;

/// This process's scratch root, created empty on first use.
///
/// Per-pid, like the rest of this repository's temp-file naming, so concurrent test binaries never
/// share one. Removed first so a recycled pid starts from a clean directory rather than inheriting
/// a previous run's persisted preferences.
fn scratch_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("hyprmux-test-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(&root);
        root
    })
}

/// A [`PlatformEnv`] whose persisted-data directories - on either platform - sit under
/// [`scratch_root`].
///
/// The runtime directory is deliberately left alone. It holds per-run socket endpoints rather than
/// anything a developer keeps, and a Unix socket path is capped at `SUN_LEN` (~108 bytes): a
/// session endpoint that fits under `/run/user/<uid>/hyprmux` does not fit under a temp root.
pub(crate) fn isolated_env() -> PlatformEnv {
    let root = scratch_root();
    PlatformEnv {
        home: Some(root.to_path_buf()),
        xdg_config_home: Some(root.join("config")),
        xdg_state_home: Some(root.join("state")),
        xdg_cache_home: Some(root.join("cache")),
        xdg_data_home: Some(root.join("data")),
        appdata: Some(root.join("AppData").join("Roaming")),
        local_appdata: Some(root.join("AppData").join("Local")),
        ..PlatformEnv::snapshot()
    }
}

/// Redirect this process's user directories into a scratch root and return it.
///
/// Idempotent and permanent: the first call wins and there is no way to restore the real
/// directories, because a test that restored them would hand the next test a live write path.
/// Call it before building anything that can persist - see the module docs.
pub fn isolate_user_dirs() -> &'static Path {
    crate::platform::paths::install_process_env_override(isolated_env());
    scratch_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guarantee the whole module exists for: the two paths tests were found writing to both
    /// resolve inside the scratch root, so neither can reach a developer's live config again.
    ///
    /// Also pins the precedence in [`crate::config::config_path`]: were an ambient `HYPRMUX_CONFIG`
    /// still able to outrank isolation, a developer with one exported would fail this.
    #[test]
    fn persistence_paths_resolve_inside_the_scratch_root() {
        let root = scratch_root();
        assert!(
            crate::config::config_path().starts_with(root),
            "config writes escaped the scratch root: {}",
            crate::config::config_path().display()
        );
        let state = crate::platform::paths::state_dir(&PlatformEnv::from_process());
        assert!(
            state.starts_with(root),
            "session autosave escaped the scratch root: {}",
            state.display()
        );
    }
}
