//! The sessionless client: what it draws, and that it stays sessionless until asked otherwise.

use hyprmux::HyprmuxApp;
use hyprmux::Msg;
use hyprmux::config::{UserCommand, UserCommandAction};
use hyprmux::input::Action;
use hyprmux::state::{Attachment, PendingSessionAction};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyBinding, Rect};

/// Serializes every test in this binary. `set_var` is only sound while no other thread reads the
/// environment, and building a `HyprmuxApp` reads it (`HYPRMUX_CONFIG`, `HOME`, ...), so the tests
/// that never touch `$EDITOR` still have to hold this.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restores `$EDITOR` when it goes out of scope.
struct EditorEnv(Option<std::ffi::OsString>);

impl EditorEnv {
    /// `open_config_file` refuses before it defers when no editor is installed, so the ambient
    /// `$EDITOR` would otherwise decide the test. Pins it to the test binary itself: an executable
    /// that provably exists wherever the test runs. Quoted because the path may contain spaces.
    fn pinned_to_an_existing_program() -> Self {
        let previous = std::env::var_os("EDITOR");
        let editor = std::env::current_exe().expect("test binary path");
        unsafe { std::env::set_var("EDITOR", format!("\"{}\"", editor.display())) };
        Self(previous)
    }
}

impl Drop for EditorEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => unsafe { std::env::set_var("EDITOR", previous) },
            None => unsafe { std::env::remove_var("EDITOR") },
        }
    }
}

fn launcher_backend() -> TestBackend<HyprmuxApp> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    // What dismissing the startup picker (or killing the last session) leaves behind: no session
    // client, no pending attach, no panes.
    *backend.state_mut().current_mut() = Attachment::new();
    backend
}

/// A client with no session is a normal state, so it must say so — and say how to leave it. The
/// empty-workspace hint would be wrong here: there is no session for a pane to be spawned into.
#[test]
fn a_sessionless_client_renders_the_launcher_panel() {
    let _env = env_lock();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = launcher_backend();
            assert!(backend.state().is_launcher());
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();

            assert!(
                lines.iter().any(|line| line.contains("No session")),
                "the launcher must name its own state, got {lines:#?}"
            );
            // The hint reflows, so each way out is checked on its own rather than as one line.
            assert!(
                lines.iter().any(|line| line.contains("pick a session")),
                "the launcher must say how to reach the picker, got {lines:#?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("start a shell")),
                "the launcher must say how to start a session, got {lines:#?}"
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("Ctrl+A d") && line.contains("detach")),
                "the launcher must say how to leave, got {lines:#?}"
            );
            // The launcher claims a bare Enter (`key_routing::launcher_start_key`); advertising
            // only the spawn binding here is what made the state look like a dead end.
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("Enter / Ctrl+A Enter")
                        && line.contains("start a shell")),
                "the launcher must offer the bare Enter it accepts, got {lines:#?}"
            );
            // Prefix and held-modifier spellings resolve to the same table entry
            // (`config::schema::scheme_shortcuts`), so listing both would be one binding said twice.
            assert!(
                !lines.iter().any(|line| line.contains("Alt+Enter")),
                "the modifier spelling is the same binding as the prefix one, got {lines:#?}"
            );
            assert!(
                !lines.iter().any(|line| line.contains("Empty workspace")),
                "the empty-workspace hint is about panes, not sessions"
            );
        })
        .expect("spawn launcher smoke thread")
        .join()
        .expect("launcher smoke completes");
}

/// Open-config (and other PTY actions) must not invent a blank local pane in the launcher. They
/// stash the action and start an ephemeral attach; the pane is created after the client arrives.
#[test]
fn open_config_in_launcher_defers_until_ephemeral_attaches() {
    let _env = env_lock();
    let _editor = EditorEnv::pinned_to_an_existing_program();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = launcher_backend();
            assert!(backend.state().is_launcher());
            backend
                .dispatch(Msg::RunAction(Action::OpenConfigFile))
                .expect("open-config from launcher");

            assert!(
                matches!(
                    backend.state().pending_session_action,
                    Some(PendingSessionAction::OpenConfigFile)
                ),
                "open-config must be deferred, got {:?}",
                backend.state().pending_session_action
            );
            assert!(
                backend.state().current().pending_session_attach.is_some(),
                "an ephemeral attach must be in flight"
            );
            assert!(
                backend
                    .state()
                    .current()
                    .workspaces
                    .iter()
                    .all(|workspace| workspace.panes.is_empty()),
                "no blank local pane before the session client exists"
            );
            assert!(backend.state().current().pending_spawns.is_empty());
        })
        .expect("spawn open-config launcher test")
        .join()
        .expect("open-config launcher test completes");
}

#[test]
fn user_run_command_in_launcher_defers_until_ephemeral_attaches() {
    let _env = env_lock();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = launcher_backend();
            backend.state_mut().config.user_commands = vec![UserCommand {
                action: UserCommandAction::run("htop"),
                binding: KeyBinding::from_str("ctrl-a h").expect("binding"),
            }];
            backend
                .dispatch(Msg::RunAction(Action::RunUserCommand(0)))
                .expect("user run from launcher");

            assert!(matches!(
                backend.state().pending_session_action,
                Some(PendingSessionAction::UserCommand { .. })
            ));
            assert!(backend.state().current().pending_session_attach.is_some());
            assert!(
                backend
                    .state()
                    .current()
                    .workspaces
                    .iter()
                    .all(|workspace| workspace.panes.is_empty())
            );
        })
        .expect("spawn user-run launcher test")
        .join()
        .expect("user-run launcher test completes");
}
