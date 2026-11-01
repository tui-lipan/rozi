//! What's new: the notes of the release this build is, shown once after rozi updates to it and
//! from Commands for as long as the build runs.
//!
//! The notes are compiled in by `build.rs` from the release workflow's generated notes, so showing
//! them needs no network and works with `[updates] check` off. Builds made anywhere else carry
//! none, and then nothing here appears.

use std::fs;
use std::io::Write;
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

/// Under the state directory: the versions that ran here, and the releases whose notes were shown.
const NOTES_DIR: &str = "release-notes";
/// One empty file per rozi version that started a client here, notes or not. The newest of them is
/// what tells an update from a downgrade.
const RAN_DIR: &str = "ran";
/// One empty file per release whose notes a client showed, or that a first install skipped. Created
/// with `create_new`, so it is also the claim that lets one client of several show them.
const SHOWN_DIR: &str = "shown";

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

/// `rozi --release-notes`: write what this build embeds, byte for byte and without a trailing
/// newline, because the release workflow compares it with the file it handed `build.rs`.
pub(crate) fn print_embedded() {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(EMBEDDED.as_bytes());
    let _ = stdout.flush();
}

/// What [`record_startup`] found about this launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Startup {
    /// No rozi has run here before: the state directory did not exist yet.
    pub(crate) first_install: bool,
    /// Show this build's notes once the client is idle.
    pub(crate) notes_due: bool,
}

/// Record that this version started a client, and decide whether this launch should show its notes
/// once the client is idle. Runs on every launch, with or without notes, because a version that
/// carries none must still count as having run here.
///
/// Call it before the client writes any state of its own, because a state directory that does not
/// exist yet is how a first install is told from an update: a first install has nothing to catch
/// up on, so it only records its version as shown.
pub(crate) fn record_startup(show_notes: bool) -> Startup {
    let env = crate::platform::paths::PlatformEnv::from_process();
    let (Some(state_dir), Ok(running)) = (
        crate::platform::paths::state_dir_if_available(&env),
        Version::parse(env!("CARGO_PKG_VERSION")),
    ) else {
        return Startup::default();
    };
    record_startup_in(&state_dir, &running, show_notes)
}

fn record_startup_in(state_dir: &Path, running: &Version, show_notes: bool) -> Startup {
    let first_install = !state_dir.exists();
    let dir = state_dir.join(NOTES_DIR);
    // A downgrade does not replay older notes: everything up to the newest version run is old news.
    let downgrade = recorded_versions(&dir.join(RAN_DIR)).any(|ran| ran > *running);
    record(&dir.join(RAN_DIR), running);
    if first_install {
        record(&dir.join(SHOWN_DIR), running);
        return Startup {
            first_install,
            notes_due: false,
        };
    }
    Startup {
        first_install,
        notes_due: show_notes && !downgrade && !marker(&dir.join(SHOWN_DIR), running).exists(),
    }
}

/// Versions with a marker in `dir`. An unreadable directory reads as none.
fn recorded_versions(dir: &Path) -> impl Iterator<Item = Version> {
    fs::read_dir(dir).into_iter().flatten().filter_map(|entry| {
        let name = entry.ok()?.file_name();
        Version::parse(name.to_str()?.strip_prefix('v')?).ok()
    })
}

/// Create `version`'s marker in `dir`, and say whether this call created it.
///
/// As the claim to show notes, a failure keeps the dialog closed: a dialog that opens on every
/// launch because its marker cannot be written is worse than one that never opens, and the command
/// still shows the notes.
fn record(dir: &Path, version: &Version) -> bool {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return false;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker(dir, version))
        .is_ok()
}

fn marker(dir: &Path, version: &Version) -> PathBuf {
    dir.join(format!("v{version}"))
}

/// Claim `version`'s notes as shown, for this client alone.
fn claim(version: &Version) -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    crate::platform::paths::state_dir_if_available(&env)
        .is_some_and(|state_dir| record(&state_dir.join(NOTES_DIR).join(SHOWN_DIR), version))
}

/// Open the notes at startup (including over the startup session or remote-host picker) or once an
/// interrupting dialog closes.
///
/// Checked at startup and after every update, so it costs one flag test until it fires, and fires
/// once.
pub(crate) fn show_if_due(ctx: &mut Context<AppRoot>) -> bool {
    if !ctx.state.release_notes_due
        || ctx
            .state
            .modal_overlays()
            .any(|name| !matches!(name, "session-picker" | "remote-picker"))
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
    let Some(version) = ctx
        .state
        .release_notes
        .as_ref()
        .map(|notes| notes.version.clone())
    else {
        return Update::none();
    };
    // Opened by hand before the automatic showing came round: this is that showing, so neither this
    // client nor another one opens the same notes again.
    if std::mem::take(&mut ctx.state.release_notes_due) {
        claim(&version);
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
    if ctx.state.follow_prompt.is_some() {
        ctx.request_focus(crate::view::follow_prompt_key());
    } else if let Some(picker) = ctx.state.remote_picker.as_ref() {
        if picker.host_form.is_some() {
            crate::ops::focus::request_host_form_focus(ctx);
        } else {
            crate::ops::focus::request_remote_picker_focus(ctx);
        }
    } else if ctx.state.show_session_picker {
        crate::ops::focus::request_session_picker_focus(ctx);
    } else {
        request_current_pane_focus(ctx);
    }
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

    /// A build the release workflow did not hand notes to - every CI, nightly, and local build -
    /// embeds nothing, so it never shows the dialog. The workflow checks the opposite case on the
    /// packaged binary through `rozi --release-notes`.
    #[test]
    fn a_build_without_release_notes_embeds_none() {
        if option_env!("ROZI_RELEASE_NOTES").is_some_and(|path| !path.is_empty()) {
            return;
        }
        assert_eq!(EMBEDDED, "");
        assert_eq!(embedded(), None);
    }

    /// Start `running` against `state_dir`, as a client whose build has notes and shows them.
    fn start(state_dir: &Path, running: &str) -> bool {
        record_startup_in(state_dir, &version(running), true).notes_due
    }

    fn shown(state_dir: &Path) -> PathBuf {
        state_dir.join(NOTES_DIR).join(SHOWN_DIR)
    }

    #[test]
    fn a_first_install_records_its_version_without_showing_it() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("rozi");

        assert!(!start(&state_dir, "1.0.0"));
        assert!(marker(&shown(&state_dir), &version("1.0.0")).exists());
        // The next start of the same version has nothing new; the next release does.
        assert!(!start(&state_dir, "1.0.0"));
        assert!(start(&state_dir, "1.1.0"));
    }

    #[test]
    fn an_update_from_a_build_that_never_recorded_anything_shows_them() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("rozi")).unwrap();

        assert!(start(&root.path().join("rozi"), "1.0.0"));
    }

    /// Notes due but not yet shown stay due: a client that quit before an idle moment leaves them
    /// for the next launch. Once one client claims them, no other start shows them.
    #[test]
    fn notes_stay_due_until_one_client_claims_them() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("rozi");
        fs::create_dir_all(&state_dir).unwrap();

        assert!(start(&state_dir, "1.1.0"));
        assert!(start(&state_dir, "1.1.0"));
        assert!(record(&shown(&state_dir), &version("1.1.0")));
        assert!(!record(&shown(&state_dir), &version("1.1.0")));
        assert!(!start(&state_dir, "1.1.0"));
    }

    /// The newest version that ran decides what a downgrade is, whether or not that version had notes
    /// to show or showed them.
    #[test]
    fn a_downgrade_below_any_version_that_ran_shows_nothing() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("rozi");
        fs::create_dir_all(&state_dir).unwrap();

        assert!(start(&state_dir, "1.0.0"));
        record(&shown(&state_dir), &version("1.0.0"));
        // 2.0.0 carries no notes, or has the automatic dialog turned off.
        assert!(!record_startup_in(&state_dir, &version("2.0.0"), false).notes_due);
        assert!(!start(&state_dir, "1.5.0"));
        assert!(start(&state_dir, "2.1.0"));
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
            let doc_rect = backend
                .rect_of_key(&crate::view::release_notes_key().into())
                .expect("release notes document rect");
            assert!(
                doc_rect.h <= 8,
                "short release notes should size to content rather than the 65% cap, got {}",
                doc_rect.h
            );

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
            let doc_rect = backend
                .rect_of_key(&crate::view::release_notes_key().into())
                .expect("release notes document rect");
            assert_eq!(doc_rect.h, 19);

            press(&mut backend, KeyCode::End);
            let bottom = frame(&mut backend);
            assert!(bottom.contains("Change number 60 "), "{bottom}");
            assert!(bottom.contains("close Enter"), "{bottom}");
        });
    }

    /// Opening the notes by hand while their automatic showing is still waiting counts as that
    /// showing, here and for any other client starting on the same version.
    #[test]
    fn notes_opened_by_hand_are_not_shown_again_automatically() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with("9.9.4");
            backend.state_mut().release_notes_due = true;
            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            assert!(backend.state().show_release_notes);
            backend.render();
            press(&mut backend, KeyCode::Enter);
            assert!(!backend.state().show_release_notes);

            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(!backend.state().show_release_notes);

            let mut other = backend_with("9.9.4");
            other.state_mut().release_notes_due = true;
            other.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(!other.state().show_release_notes);
        });
    }

    #[test]
    fn due_notes_show_at_startup_and_over_startup_picker_then_claim_once() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with("9.9.2");
            backend.state_mut().release_notes_due = true;
            backend.state_mut().show_palette = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(
                !backend.state().show_release_notes,
                "an open non-startup dialog keeps the screen"
            );
            assert!(backend.state().release_notes_due);

            // Over the startup session picker, due notes show immediately on top and return focus
            // to the picker when dismissed.
            backend.state_mut().show_palette = false;
            backend.state_mut().session_picker =
                Some(crate::state::SessionPickerState::new(Vec::new()));
            backend.state_mut().show_session_picker = true;
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(backend.state().show_release_notes);
            assert!(!backend.state().release_notes_due);
            assert!(backend.state().release_notes_is_stacked());
            assert_eq!(backend.state().modal_overlay(), Some("release-notes"));
            backend.render();
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::release_notes_key())
            );

            press(&mut backend, KeyCode::Enter);
            assert!(!backend.state().show_release_notes);
            assert!(backend.state().show_session_picker);
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::session_picker_key())
            );

            // A second client starting on the same version finds the notes already claimed.
            let mut other = backend_with("9.9.2");
            other.state_mut().release_notes_due = true;
            other.dispatch(Msg::RefreshPaintLayers).unwrap();
            assert!(!other.state().show_release_notes);
            assert!(!other.state().release_notes_due);
        });
    }

    fn row_of(frame: &str, needle: &str) -> usize {
        frame
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("`{needle}` is on screen:\n{frame}"))
    }

    #[test]
    fn askpass_over_release_notes_and_startup_picker_staggers_modal_tops() {
        crate::test_support::on_large_stack(|| {
            // Case 1: release notes -> askpass (no picker beneath).
            let mut backend = backend_with("9.9.5");
            backend
                .dispatch(Msg::RunAction(Action::ShowReleaseNotes))
                .unwrap();
            let base_top = row_of(&frame(&mut backend), "What's new · rozi v9.9.5");
            assert!(!backend.state().release_notes_is_stacked());

            backend
                .dispatch(Msg::RemoteAskpassPrompt {
                    id: 1,
                    session: "ssh-1".into(),
                    attach_epoch: None,
                    kind: crate::session::remote::AskpassKind::Secret,
                    prompt: "dev@workbox's password: ".into(),
                })
                .unwrap();
            assert!(!backend.state().release_notes_is_stacked());
            assert_eq!(backend.state().askpass_stack_depth(), 1);
            let stacked_two = frame(&mut backend);
            assert_eq!(
                row_of(&stacked_two, "What's new · rozi v9.9.5"),
                base_top,
                "{stacked_two}"
            );
            assert_eq!(
                row_of(&stacked_two, "SSH · dev@workbox's password"),
                base_top + 1,
                "{stacked_two}"
            );

            // Case 2: session picker -> release notes -> askpass.
            backend.state_mut().session_picker =
                Some(crate::state::SessionPickerState::new(Vec::new()));
            backend.state_mut().show_session_picker = true;
            assert!(backend.state().release_notes_is_stacked());
            assert_eq!(backend.state().askpass_stack_depth(), 2);
            let stacked_three = frame(&mut backend);
            assert_eq!(
                row_of(&stacked_three, "Sessions"),
                base_top,
                "{stacked_three}"
            );
            assert_eq!(
                row_of(&stacked_three, "What's new · rozi v9.9.5"),
                base_top + 1,
                "{stacked_three}"
            );
            assert_eq!(
                row_of(&stacked_three, "SSH · dev@workbox's password"),
                base_top + 2,
                "{stacked_three}"
            );
        });
    }
}
