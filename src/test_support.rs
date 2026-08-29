//! Scratch user directories for test processes.
//!
//! Test binaries run inside a developer's real environment, and rozi persists preferences as a
//! side effect of ordinary actions: toggling the sidebar writes `[sidebar]` through
//! [`crate::config::persist`], and leaving a session writes `session.toml` through
//! [`crate::profiles`]. Without isolation those writes land on the developer's own
//! `~/.config/rozi/config.toml` - and a running rozi watches that file, so the test's state
//! is live-reloaded into the UI the developer is working in.
//!
//! Isolation redirects every directory [`crate::platform::paths`] resolves - config, state, cache,
//! data, runtime - into a per-process scratch root, and makes that root inescapable: while it is
//! installed, an ambient `ROZI_CONFIG` is ignored too (see [`crate::config::config_path`]).
//!
//! Unit tests get this for free - [`PlatformEnv::from_process`] isolates itself under `cfg(test)`.
//! Integration tests link the non-test build of the library, so a test binary that builds a
//! `AppRoot` and dispatches actions must call [`isolate_user_dirs`] first.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tui_lipan::CommandLink;

use crate::platform::paths::PlatformEnv;

/// A real session inbound mailbox seeded for dispatcher benchmarks.
///
/// The mailbox stays inactive so it does not enqueue drain messages behind the benchmark's back;
/// callers explicitly dispatch [`drain_message`](Self::drain_message) until
/// [`is_empty`](Self::is_empty).
#[doc(hidden)]
pub struct InboundMailboxFixture {
    epoch: u64,
    mailbox: Arc<crate::session::client::InboundMailbox>,
}

impl InboundMailboxFixture {
    pub fn drain_message(&self) -> crate::Msg {
        crate::Msg::DrainSessionFrames {
            epoch: self.epoch,
            mailbox: Arc::clone(&self.mailbox),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.mailbox.is_empty()
    }
}

/// Seed the production mailbox without activating its automatic drain scheduling.
#[doc(hidden)]
pub fn inbound_mailbox_fixture(
    link: CommandLink<crate::Msg>,
    epoch: u64,
    frames: impl IntoIterator<
        Item = crate::session::protocol::Frame<crate::session::protocol::ServerMessage>,
    >,
) -> InboundMailboxFixture {
    let mailbox =
        crate::session::client::InboundMailbox::new(epoch, "benchmark-session".to_string(), link);
    for frame in frames {
        mailbox
            .push(frame)
            .expect("benchmark frame must fit in the inbound mailbox");
    }
    InboundMailboxFixture { epoch, mailbox }
}

/// This process's scratch root, created empty on first use.
///
/// Per-pid, like the rest of this repository's temp-file naming, so concurrent test binaries never
/// share one. Removed first so a recycled pid starts from a clean directory rather than inheriting
/// a previous run's persisted preferences.
fn scratch_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("rozi-test-home-{}", std::process::id()));
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
/// session endpoint that fits under `/run/user/<uid>/rozi` does not fit under a temp root.
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

/// A per-test directory carrying the private permissions an endpoint parent must have.
///
/// Endpoints live in a directory the platform layer requires to be private to the current user -
/// mode `0700` on Unix, a protected DACL on Windows. `std::env::temp_dir()` is none of those: it is
/// shared, and on Windows it carries the inheritable ACEs of its container rather than a protected
/// DACL of its own. So neither an endpoint placed directly in the temp directory nor one under a
/// plain `create_dir_all` child of it can pass that check - the child inherits the same permissive
/// ACEs. Creating the directory through the same helper the runtime uses is what makes a test
/// endpoint resemble a real one.
///
/// `label` and the pid keep parallel tests and repeated runs from colliding on the derived name.
/// The label must be unique per *test*, not per module: `ensure_private_dir` checks for the
/// directory and then creates it, so two tests that pass the same label can both see it missing and
/// both try to create it, and the loser fails with `ERROR_ALREADY_EXISTS` instead of validating the
/// directory that now exists. Naming the socket inside a shared parent is not enough - the parent is
/// what races.
///
/// `cfg(test)` because every caller is a unit test in this crate. The rest of this module is
/// compiled into normal builds for integration tests to link against; this has no such caller.
#[cfg(test)]
pub(crate) fn private_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rozi-{label}-{}", std::process::id()));
    crate::platform::fs_security::ensure_private_dir(&dir)
        .expect("private scratch directory for a test endpoint");
    dir
}

/// Build the real configured root with a live control endpoint inside the isolated test
/// environment. Integration tests use this when `AppRoot::default()` would intentionally be too
/// inert: the default root has no filesystem config, startup tasks, or listener.
pub fn configured_app() -> crate::AppRoot {
    let config = crate::config::load_config().config;
    let (listener, guard) =
        crate::control::bind_control_socket().expect("bind isolated test control endpoint");
    crate::AppRoot::configured_for_test(config, listener, guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guarantee the whole module exists for: the two paths tests were found writing to both
    /// resolve inside the scratch root, so neither can reach a developer's live config again.
    ///
    /// Also pins the precedence in [`crate::config::config_path`]: were an ambient `ROZI_CONFIG`
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
