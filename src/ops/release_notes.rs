//! What's new: the notes of the release this build is, shown once after rozi updates to it and
//! from Commands for as long as the build runs.
//!
//! The notes are compiled in by `build.rs` from the release workflow's generated notes, so showing
//! them needs no network and works with `[updates] check` off. Builds made anywhere else carry
//! none, and then nothing here appears.

use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;
use tui_lipan::prelude::*;
use url::Url;

use crate::AppRoot;
use crate::ops::focus::{request_current_pane_focus, request_release_notes_focus};
use crate::release_app::ROZI;
use crate::state::Mode;

/// What the release workflow handed `build.rs`; empty for every other build.
const EMBEDDED: &str = include_str!(concat!(env!("OUT_DIR"), "/release-notes.md"));

/// One empty marker file per version whose notes were shown or deliberately skipped, under the
/// state directory.
const SEEN_DIR: &str = "release-notes";

/// The notes one release carries, as Markdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseNotes {
    pub(crate) version: Version,
    pub(crate) markdown: &'static str,
}

impl ReleaseNotes {
    /// Notes for `version`, or `None` when there are none to show: no notes, or a version that is
    /// not a plain release.
    pub(crate) fn new(version: &str, markdown: &'static str) -> Option<Self> {
        let version = Version::parse(version).ok()?;
        let markdown = markdown.trim();
        (version.pre.is_empty() && !markdown.is_empty()).then_some(Self { version, markdown })
    }

    /// The release's page on GitHub, where the same notes sit next to its downloads.
    pub(crate) fn page_url(&self) -> Option<Url> {
        Url::parse(ROZI.repository_url)
            .ok()?
            .join(&format!("releases/tag/v{}", self.version))
            .ok()
    }
}

/// This build's notes, if it carries any.
pub(crate) fn embedded() -> Option<ReleaseNotes> {
    ReleaseNotes::new(env!("CARGO_PKG_VERSION"), EMBEDDED)
}

/// Decide at startup whether this launch should show `notes` once the client is idle.
///
/// Call it before the client writes any state of its own, because a state directory that does not
/// exist yet is how a first install is told from an update: a first install has nothing to catch
/// up on, so it only records the version as seen.
pub(crate) fn due_at_startup(notes: &ReleaseNotes, show_after_update: bool) -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    let Some(state_dir) = crate::platform::paths::state_dir_if_available(&env) else {
        return false;
    };
    show_after_update && due_in(&state_dir, &notes.version)
}

fn due_in(state_dir: &Path, version: &Version) -> bool {
    let seen = state_dir.join(SEEN_DIR);
    if !state_dir.exists() {
        claim_in(&seen, version);
        return false;
    }
    // A downgrade does not replay older notes: every version up to the newest one seen is old news.
    seen_versions(&seen).all(|shown| shown < *version)
}

/// Versions with a marker in `seen`. An unreadable directory reads as none seen.
fn seen_versions(seen: &Path) -> impl Iterator<Item = Version> {
    fs::read_dir(seen)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name();
            Version::parse(name.to_str()?.strip_prefix('v')?).ok()
        })
}

/// Atomically let one client show each release's notes.
///
/// Unlike the update notice, a failure to record keeps the dialog closed: a dialog that opens on
/// every launch because its marker cannot be written is worse than one that never opens, and the
/// command still shows the notes.
fn claim_in(seen: &Path, version: &Version) -> bool {
    if crate::platform::fs_security::ensure_private_dir(seen).is_err() {
        return false;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(seen_marker(seen, version))
        .is_ok()
}

fn seen_marker(seen: &Path, version: &Version) -> PathBuf {
    seen.join(format!("v{version}"))
}

fn claim(version: &Version) -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    crate::platform::paths::state_dir_if_available(&env)
        .is_some_and(|state_dir| claim_in(&state_dir.join(SEEN_DIR), version))
}

/// Open the notes the startup decision held back, once the client has a session on screen and no
/// other dialog: whatever the user opened first keeps the keyboard, and the notes wait their turn.
///
/// Checked after every update, so it costs one flag test until it fires, and fires once.
pub(crate) fn show_if_due(ctx: &mut Context<AppRoot>) -> bool {
    if !ctx.state.release_notes_due
        || ctx.state.has_modal_overlay()
        || !ctx.state.current().session_attached
    {
        return false;
    }
    ctx.state.release_notes_due = false;
    let Some(version) = ctx
        .state
        .release_notes
        .as_ref()
        .map(|notes| notes.version.clone())
    else {
        return false;
    };
    // Several clients may start together after an update; the first to claim the version shows it.
    if !claim(&version) {
        return false;
    }
    let _ = open(ctx);
    true
}

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.release_notes.is_none() {
        return Update::none();
    }
    ctx.state.show_release_notes = true;
    ctx.state.keybindings = None;
    ctx.state.show_palette = false;
    ctx.state.show_theme_picker = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_release_notes_focus(ctx);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    if !ctx.state.show_release_notes {
        return Update::none();
    }
    ctx.state.show_release_notes = false;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn open_page(ctx: &mut Context<AppRoot>) -> Update {
    let Some(url) = ctx
        .state
        .release_notes
        .as_ref()
        .and_then(ReleaseNotes::page_url)
    else {
        return Update::none();
    };
    match tui_lipan::utils::open_url(url.as_str()) {
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

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn only_plain_releases_with_notes_have_any() {
        let notes = ReleaseNotes::new("1.2.3", "## Added\n- Something\n").unwrap();
        assert_eq!(notes.version, version("1.2.3"));
        assert_eq!(
            notes.page_url().unwrap().as_str(),
            "https://github.com/tui-lipan/rozi/releases/tag/v1.2.3"
        );
        assert_eq!(ReleaseNotes::new("1.2.3", " \n"), None);
        assert_eq!(ReleaseNotes::new("1.2.3-rc.1", "## Added\n- x\n"), None);
        assert_eq!(ReleaseNotes::new("local", "## Added\n- x\n"), None);
    }

    #[test]
    fn a_first_install_records_its_version_without_showing_it() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("rozi");

        assert!(!due_in(&state_dir, &version("1.0.0")));
        assert!(seen_marker(&state_dir.join(SEEN_DIR), &version("1.0.0")).exists());
        // The next start of the same version has nothing new; the next release does.
        assert!(!due_in(&state_dir, &version("1.0.0")));
        assert!(due_in(&state_dir, &version("1.1.0")));
    }

    #[test]
    fn an_update_from_a_build_that_never_recorded_notes_shows_them() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("rozi")).unwrap();

        assert!(due_in(&root.path().join("rozi"), &version("1.0.0")));
    }

    #[test]
    fn notes_show_once_across_clients_and_never_after_a_downgrade() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("rozi");
        fs::create_dir_all(&state_dir).unwrap();
        let seen = state_dir.join(SEEN_DIR);

        assert!(due_in(&state_dir, &version("1.1.0")));
        assert!(claim_in(&seen, &version("1.1.0")));
        assert!(!claim_in(&seen, &version("1.1.0")));
        assert!(!due_in(&state_dir, &version("1.1.0")));
        assert!(!due_in(&state_dir, &version("1.0.0")));
        assert!(due_in(&state_dir, &version("1.2.0")));
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::Msg;
    use crate::input::Action;
    use tui_lipan::TestBackend;

    const NOTES: &str = "## Added\n- Pane groups keep their layout. ([#901](https://github.com/tui-lipan/rozi/pull/901))\n\n## Fixed\n- `rozi attach` waits for the server.\n";

    fn backend_with(version: &str) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 36,
        });
        let state = backend.state_mut();
        state.config.animations.picker = crate::layout::anim::PickerAnimationStyle::Off;
        state.release_notes = ReleaseNotes::new(version, NOTES);
        state.commands_dirty = true;
        backend
    }

    fn frame(backend: &mut TestBackend<AppRoot>) -> String {
        backend.render();
        backend.capture_frame().to_fixed_grid_lines().join("\n")
    }

    fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
        backend
            .send_key(KeyEvent {
                code,
                mods: KeyMods::NONE,
            })
            .expect("send key");
    }

    #[test]
    fn a_build_without_notes_offers_no_whats_new() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            assert!(!crate::commands::command_available(
                Action::ShowReleaseNotes,
                backend.state()
            ));
            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            assert!(!backend.state().show_release_notes);
        });
    }

    #[test]
    fn whats_new_renders_the_notes_as_markdown_and_closes() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with("9.9.1");
            assert!(crate::commands::command_available(
                Action::ShowReleaseNotes,
                backend.state()
            ));
            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            let shown = frame(&mut backend);
            assert!(shown.contains("What's new · rozi v9.9.1"), "{shown}");
            assert!(shown.contains("Pane groups keep their layout."), "{shown}");
            assert!(shown.contains("rozi attach"), "{shown}");
            // Markup and link targets are rendered, not printed.
            assert!(!shown.contains("## "), "{shown}");
            assert!(!shown.contains("](https://"), "{shown}");
            assert!(shown.contains("open release page"), "{shown}");
            assert_eq!(backend.state().modal_overlay(), Some("release-notes"));

            press(&mut backend, KeyCode::Enter);
            assert!(!backend.state().show_release_notes);

            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            backend.render();
            press(&mut backend, KeyCode::Esc);
            assert!(!backend.state().show_release_notes);
        });
    }

    /// Notes taller than the dialog scroll under a hint row that stays on screen.
    #[test]
    fn long_notes_scroll_and_keep_the_hints() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with("9.9.3");
            let long: String = (1..=60)
                .map(|n| format!("- Change number {n} fixed something users noticed.\n"))
                .collect();
            backend.state_mut().release_notes =
                ReleaseNotes::new("9.9.3", format!("## Fixed\n{long}").leak());
            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            let top = frame(&mut backend);
            assert!(top.contains("Change number 1 "), "{top}");
            assert!(!top.contains("Change number 60 "), "{top}");
            assert!(top.contains("close Enter"), "{top}");

            press(&mut backend, KeyCode::End);
            let bottom = frame(&mut backend);
            assert!(bottom.contains("Change number 60 "), "{bottom}");
            assert!(bottom.contains("close Enter"), "{bottom}");
        });
    }

    #[test]
    fn due_notes_wait_for_a_session_and_a_free_screen_then_show_once() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with("9.9.2");
            backend.state_mut().release_notes_due = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(
                !backend.state().show_release_notes,
                "nothing shows before a session is on screen"
            );

            backend.state_mut().current_mut().session_attached = true;
            backend.state_mut().show_palette = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(
                !backend.state().show_release_notes,
                "an open dialog keeps the screen"
            );
            assert!(backend.state().release_notes_due);

            backend.dispatch(Msg::ClosePalette).unwrap();
            assert!(backend.state().show_release_notes);
            assert!(!backend.state().release_notes_due);

            // A second client starting on the same version finds the notes already claimed.
            let mut other = backend_with("9.9.2");
            other.state_mut().release_notes_due = true;
            other.state_mut().current_mut().session_attached = true;
            other.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(!other.state().show_release_notes);
            assert!(!other.state().release_notes_due);
        });
    }
}
