//! The welcome card: a first install's short introduction to sessions, panes, workspaces, and the
//! keys that move between them, shown once in the first session.

use std::fs;
use std::path::Path;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::focus::{request_current_pane_focus, request_welcome_focus};
use crate::state::Mode;

/// Present in the state directory from a first install until one client has shown the card. Its
/// removal is the claim, so several clients starting together show it once, and a client that quit
/// before its first session leaves it for the next launch.
const PENDING_FILE: &str = "welcome-pending";

/// Where the card's "open guide" action leads.
pub(crate) const GUIDE_URL: &str = concat!(env!("CARGO_PKG_HOMEPAGE"), "/getting-started");

/// Decide at startup whether this launch should welcome the user once a session is on screen.
///
/// `first_install` comes from [`crate::ops::release_notes::record_startup`]. A config file that is
/// already there marks someone who used rozi elsewhere and brought their settings along, so they
/// are not introduced to it again.
pub(crate) fn due_at_startup(first_install: bool) -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    let Some(state_dir) = crate::platform::paths::state_dir_if_available(&env) else {
        return false;
    };
    let configured = crate::config::config_path().exists();
    due_in(&state_dir, first_install && !configured)
}

fn due_in(state_dir: &Path, welcome_now: bool) -> bool {
    let pending = state_dir.join(PENDING_FILE);
    if welcome_now {
        let _ = crate::platform::persist::create_private_file(&pending, false);
    }
    pending.exists()
}

/// Take the pending welcome for this client alone.
fn claim_in(state_dir: &Path) -> bool {
    fs::remove_file(state_dir.join(PENDING_FILE)).is_ok()
}

fn claim() -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    crate::platform::paths::state_dir_if_available(&env).is_some_and(|dir| claim_in(&dir))
}

/// Open the card the startup decision held back, once a session is on screen and no other dialog
/// is: until then the user is still choosing where to start, and the card would be in the way.
///
/// Checked after every update, so it costs one flag test until it fires, and fires once.
pub(crate) fn show_if_due(ctx: &mut Context<AppRoot>) -> bool {
    if !ctx.state.welcome_due
        || ctx.state.has_modal_overlay()
        || !ctx.state.current().session_attached
    {
        return false;
    }
    ctx.state.welcome_due = false;
    if !claim() {
        return false;
    }
    ctx.state.show_welcome = true;
    ctx.state.keybindings = None;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_welcome_focus(ctx);
    true
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    if !ctx.state.show_welcome {
        return Update::none();
    }
    ctx.state.show_welcome = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn open_guide(ctx: &mut Context<AppRoot>) -> Update {
    match tui_lipan::utils::open_url(GUIDE_URL) {
        Ok(()) => Update::none(),
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Could not open link", error.to_string());
            Update::full()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_welcome_waits_for_one_client_to_claim_it() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path()).unwrap();

        assert!(due_in(root.path(), true));
        // A client that quit before showing it leaves it for the next launch.
        assert!(due_in(root.path(), false));
        assert!(claim_in(root.path()));
        assert!(!claim_in(root.path()));
        assert!(!due_in(root.path(), false));
    }

    #[test]
    fn nothing_is_due_without_a_first_install() {
        let root = tempfile::tempdir().unwrap();

        assert!(!due_in(root.path(), false));
    }

    #[test]
    fn the_guide_link_points_at_the_documentation() {
        assert_eq!(GUIDE_URL, "https://rozi.tui-lipan.dev/getting-started");
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::Msg;
    use std::str::FromStr;
    use std::sync::{Mutex, MutexGuard};
    use tui_lipan::TestBackend;

    /// Every test here shares this process's scratch state directory, and with it the one pending
    /// file.
    static PENDING: Mutex<()> = Mutex::new(());

    fn pending_welcome() -> MutexGuard<'static, ()> {
        let guard = PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let env = crate::platform::paths::PlatformEnv::from_process();
        let state_dir = crate::platform::paths::state_dir_if_available(&env).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        assert!(due_in(&state_dir, true));
        guard
    }

    fn backend() -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 36,
        });
        let state = backend.state_mut();
        state.config.animations.picker = crate::layout::anim::PickerAnimationStyle::Off;
        state.welcome_due = true;
        backend
    }

    fn frame(backend: &mut TestBackend<AppRoot>) -> String {
        backend.render();
        backend.capture_frame().to_fixed_grid_lines().join("\n")
    }

    #[test]
    fn the_welcome_waits_for_a_session_and_a_free_screen_then_shows_once() {
        crate::test_support::on_large_stack(|| {
            let _pending = pending_welcome();
            let mut backend = backend();
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(
                !backend.state().show_welcome,
                "nothing shows while the user is still choosing a session"
            );

            backend.state_mut().current_mut().session_attached = true;
            backend.state_mut().show_palette = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(
                !backend.state().show_welcome,
                "an open dialog keeps the screen"
            );

            backend.dispatch(Msg::ClosePalette).unwrap();
            assert!(backend.state().show_welcome);
            assert_eq!(backend.state().modal_overlay(), Some("welcome"));

            backend
                .send_key(KeyEvent {
                    code: KeyCode::Enter,
                    mods: KeyMods::NONE,
                })
                .unwrap();
            assert!(!backend.state().show_welcome);

            // Another client starting alongside finds the welcome already taken.
            let mut other = self::backend();
            other.state_mut().current_mut().session_attached = true;
            other.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(!other.state().show_welcome);
            assert!(!other.state().welcome_due);
        });
    }

    #[test]
    fn the_welcome_explains_the_words_and_shows_the_keys_as_bound() {
        crate::test_support::on_large_stack(|| {
            let _pending = pending_welcome();
            let mut backend = backend();
            // An unbound command takes no row rather than naming a key that does nothing.
            backend
                .state_mut()
                .config
                .key_overrides
                .insert("close".to_string(), Vec::new());
            // A key moved off the prefix keeps its whole chord; one moved within it shows its key.
            backend.state_mut().config.key_overrides.insert(
                "command-palette".to_string(),
                vec![tui_lipan::prelude::KeyBinding::from_str("alt-o").unwrap()],
            );
            backend.state_mut().config.key_overrides.insert(
                "help".to_string(),
                vec![tui_lipan::prelude::KeyBinding::from_str("ctrl-a i").unwrap()],
            );
            backend.state_mut().commands_dirty = true;
            backend.state_mut().current_mut().session_attached = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();

            // The full card with its logo, and the compact one a small terminal gets.
            for (width, height) in [(140, 42), (100, 30)] {
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: width,
                    h: height,
                });
                let shown = frame(&mut backend);
                let row = |key: &str, does: &str| {
                    shown
                        .lines()
                        .any(|line| line.contains(&format!(" {key} ")) && line.contains(does))
                };
                assert!(shown.contains("Welcome to rozi"), "{shown}");
                for word in ["session", "pane", "workspace"] {
                    assert!(shown.contains(&format!(" {word} ")), "{word}:\n{shown}");
                }
                assert!(shown.contains("prefix, Ctrl+A: press it"), "{shown}");
                assert!(row("Enter", "open a new pane"), "{shown}");
                assert!(row("h j k l", "move between panes"), "{shown}");
                assert!(row("1-9", "switch workspace"), "{shown}");
                assert!(row("Alt+O", "find any command"), "{shown}");
                assert!(row("i", "see every key"), "{shown}");
                assert!(!shown.contains("close the pane"), "{shown}");
                assert!(shown.contains("open guide"), "{shown}");
            }
            // Only the full card has room for the mark and the version under it.
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 140,
                h: 42,
            });
            let wide = frame(&mut backend);
            assert!(
                wide.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))),
                "{wide}"
            );
        });
    }
}
