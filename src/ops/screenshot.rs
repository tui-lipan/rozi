//! The Screenshot pane and Screenshot UI actions.
//!
//! A screenshot is of what this client shows and is written on this client's machine, even when
//! the session it shows is remote. The PNGs come from the same encoder `capture-pane --render png`
//! and `capture-ui --render png` use; only where the bytes go differs.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use tui_lipan::prelude::*;

use crate::pane::lifecycle::find_pane_in_namespace_mut;
use crate::pane::pty_events::{notify_error, notify_info};
use crate::state::{ScreenshotFlash, ScreenshotTarget};
use crate::{AppRoot, Msg};

/// How many numbered names a screenshot tries before giving up on a directory. Two screenshots
/// in one second take `-2`; a thousand means something other than a fast finger is at work.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// What the thread that writes one screenshot needs, and the way back to the UI.
#[derive(Clone)]
pub(crate) struct ScreenshotJob {
    target: ScreenshotTarget,
    dir: Option<PathBuf>,
    scale: u8,
    link: CommandLink<Msg>,
}

impl ScreenshotJob {
    /// Encode `frame`, write it, and report the result. Runs off the UI thread.
    pub(crate) fn write(self, frame: &tui_lipan::CapturedFrame, palette: TerminalColorPalette) {
        let png = self.encode(frame, palette);
        self.save(&png);
    }

    /// The PNG scale this screenshot is written at; jobs at the same scale share one encode.
    pub(crate) fn scale(&self) -> u8 {
        self.scale
    }

    pub(crate) fn encode(
        &self,
        frame: &tui_lipan::CapturedFrame,
        palette: TerminalColorPalette,
    ) -> std::result::Result<Vec<u8>, String> {
        crate::pane::png_bytes(frame, palette, self.scale)
            .map_err(|error| format!("could not encode the PNG: {error}"))
    }

    /// Write an already encoded PNG, and report the result. Runs off the UI thread.
    pub(crate) fn save(self, png: &std::result::Result<Vec<u8>, String>) {
        let result = png
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|png| save_png(self.dir.as_deref(), self.target, png));
        match result {
            Ok(path) => self.link.send(Msg::ScreenshotSaved {
                target: self.target,
                path,
            }),
            Err(error) => self.fail(error),
        }
    }

    pub(crate) fn fail(self, error: String) {
        self.link.send(Msg::ScreenshotFailed { error });
    }
}

fn job(ctx: &mut Context<AppRoot>, target: ScreenshotTarget) -> Option<ScreenshotJob> {
    let Some(link) = ctx.state.command_link.clone() else {
        notify_error(ctx, "Screenshot failed", "rozi is still starting");
        return None;
    };
    let capture = &ctx.state.config.capture;
    Some(ScreenshotJob {
        target,
        dir: capture.dir.clone(),
        scale: capture.scale,
        link,
    })
}

/// Save the focused pane's visible screen, from this client's own copy of it.
pub(crate) fn screenshot_pane(ctx: &mut Context<AppRoot>) -> Update {
    let Some((id, local)) = ctx.state.focused_pane_target() else {
        notify_error(ctx, "Screenshot failed", "No focused pane");
        return Update::full();
    };
    let attachment = (!local).then_some(ctx.state.runtime_epoch);
    let Some(job) = job(ctx, ScreenshotTarget::Pane { id, attachment }) else {
        return Update::full();
    };
    let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, id, local) else {
        notify_error(ctx, "Screenshot failed", format!("Pane {id} not found"));
        return Update::full();
    };
    let (frame, palette) = pane
        .terminal
        .with_screen_mut(|screen| (screen.capture_frame(), screen.palette()));
    if !crate::jobs::try_spawn(move || job.write(&frame, palette)) {
        notify_error(
            ctx,
            "Screenshot failed",
            "Too many background jobs are running",
        );
        return Update::full();
    }
    Update::none()
}

/// Save the whole client as the next frame paints it.
///
/// Running this from the palette has already closed it, so that frame no longer draws the palette.
/// It would still draw the dim the palette left on everything else, fading out, so until that frame
/// is taken [`crate::ops::control::ui_screenshot_waiting`] has the view settle it at once.
pub(crate) fn screenshot_ui(ctx: &mut Context<AppRoot>) -> Update {
    let Some(job) = job(ctx, ScreenshotTarget::Ui) else {
        return Update::full();
    };
    crate::ops::control::capture_ui_for_screenshot(ctx, job);
    Update::full()
}

pub(crate) fn screenshot_saved(
    ctx: &mut Context<AppRoot>,
    target: ScreenshotTarget,
    path: PathBuf,
) -> Update {
    if crate::layout::anim::screenshot_flash_enabled(ctx.state.config.animations) {
        let revision = ctx.state.screenshot.next_revision;
        ctx.state.screenshot.next_revision = revision.wrapping_add(1);
        ctx.state.screenshot.flash = Some(ScreenshotFlash { target, revision });
    }
    let shown = crate::platform::paths::compress_home(&path.to_string_lossy());
    notify_info(ctx, format!("Screenshot saved\n{shown}"));
    Update::full()
}

pub(crate) fn screenshot_failed(ctx: &mut Context<AppRoot>, error: String) -> Update {
    notify_error(ctx, "Screenshot failed", error);
    Update::full()
}

fn save_png(
    configured: Option<&Path>,
    target: ScreenshotTarget,
    png: &[u8],
) -> std::result::Result<PathBuf, String> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    let dir =
        crate::platform::paths::capture_dir(&env, configured).map_err(
            |error| match configured {
                Some(dir) => format!(
                    "cannot use {}: {error}",
                    crate::platform::paths::compress_home(&dir.to_string_lossy())
                ),
                None => format!("cannot create the captures directory: {error}"),
            },
        )?;
    write_unique(&dir, &file_stem(target, chrono::Local::now()), png).map_err(|error| {
        format!(
            "cannot write to {}: {error}",
            crate::platform::paths::compress_home(&dir.to_string_lossy())
        )
    })
}

/// `rozi-pane-<id>-<stamp>` or `rozi-ui-<stamp>`, stamped in local time to the second.
fn file_stem(target: ScreenshotTarget, now: chrono::DateTime<chrono::Local>) -> String {
    let stamp = now.format("%Y%m%d-%H%M%S");
    match target {
        ScreenshotTarget::Pane { id, .. } => format!("rozi-pane-{id}-{stamp}"),
        ScreenshotTarget::Ui => format!("rozi-ui-{stamp}"),
    }
}

/// Write `bytes` to `<stem>.png` in `dir`, or to `<stem>-2.png`, `<stem>-3.png`, … when the name is
/// taken. An existing file is never replaced.
fn write_unique(dir: &Path, stem: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let path = if attempt == 1 {
            dir.join(format!("{stem}.png"))
        } else {
            dir.join(format!("{stem}-{attempt}.png"))
        };
        let mut file = match crate::platform::persist::create_private_file(&path, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free file name for {stem}.png in {}", dir.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        CaptureContent, CaptureRender, ControlCommand, ControlEnvelope, ControlRequest,
        ControlResponse,
    };
    use crate::input::Action;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tui_lipan::TestBackend;

    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .unwrap()
            .join()
            .unwrap();
    }

    /// A client that writes screenshots into `dir`, with the flash on, whose mount has delivered
    /// the command link a screenshot reports back through.
    fn backend_writing_to(dir: &Path) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        let config = &mut backend.state_mut().config;
        config.capture.dir = Some(dir.to_path_buf());
        config.animations.enabled = true;
        config.animations.focus_chrome = true;
        let deadline = Instant::now() + Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(
                Instant::now() < deadline,
                "the mount never delivered the link"
            );
            backend.pump().unwrap();
            std::thread::yield_now();
        }
        backend
    }

    /// Pump until a screenshot reports back, and return its toast: title and body, space-joined.
    fn screenshot_toast(backend: &mut TestBackend<AppRoot>) -> String {
        // The PNG encoder may load a system font on its first use.
        let deadline = Instant::now() + Duration::from_secs(if cfg!(windows) { 30 } else { 10 });
        loop {
            backend.pump().unwrap();
            if let Some(toast) = backend
                .state()
                .replaceable_toasts
                .values()
                .map(|tracked| tracked.content().replace(['\u{0}', '\n'], " "))
                .find(|content| content.starts_with("Screenshot"))
            {
                return toast;
            }
            assert!(
                Instant::now() < deadline,
                "the screenshot never reported back"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn pngs_in(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
            .collect();
        files.sort();
        files
    }

    fn capture_ui_request(render: CaptureRender) -> (Msg, mpsc::Receiver<ControlResponse>) {
        let (reply, response) = mpsc::channel();
        let message = Msg::ControlRequest(ControlEnvelope {
            request: ControlRequest {
                command: ControlCommand::CaptureUi {
                    render,
                    scale: None,
                    image_pixels: false,
                },
                source_pane: None,
                source_session: None,
                extension: None,
            },
            reply,
        });
        (message, response)
    }

    fn capture_ui_content(response: &mpsc::Receiver<ControlResponse>) -> CaptureContent {
        let response = response.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(response.ok, "capture-ui failed: {:?}", response.error);
        serde_json::from_value::<crate::control::UiCapture>(response.data.unwrap())
            .unwrap()
            .content
    }

    #[test]
    fn screenshot_pane_saves_the_focused_pane_at_the_configured_scale() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend_writing_to(dir.path());
            backend.state_mut().config.capture.scale = 2;
            let id = backend.state().focused_pane().expect("a focused pane");
            // A hidden scratch pane may share the focused shared pane's id; it is not the one
            // in focus.
            let decoy = crate::state::Pane::new(id, 100, Default::default());
            decoy
                .terminal
                .with_screen_mut(|screen| screen.process_bytes(b"decoy"));
            backend.state_mut().scratch.panes.push(decoy);
            assert!(!backend.state().scratch_visible);
            let pane =
                crate::pane::lifecycle::find_pane_in_namespace_mut(backend.state_mut(), id, false)
                    .unwrap();
            let (frame, palette) = pane.terminal.with_screen_mut(|screen| {
                screen.process_bytes(b"photographed");
                (screen.capture_frame(), screen.palette())
            });

            backend
                .dispatch(Msg::RunAction(Action::ScreenshotPane))
                .unwrap();
            let toast = screenshot_toast(&mut backend);

            let files = pngs_in(dir.path());
            assert_eq!(files.len(), 1, "{files:?}");
            let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with(&format!("rozi-pane-{id}-")), "{name}");
            assert!(toast.starts_with("Screenshot saved"), "{toast}");
            assert!(toast.contains(&name), "the toast names the file: {toast}");
            let saved = std::fs::read(&files[0]).unwrap();
            assert!(saved.starts_with(PNG_SIGNATURE));
            assert_eq!(
                saved,
                crate::pane::png_bytes(&frame, palette, 2).unwrap(),
                "the pane's own screen, at the configured scale"
            );
            assert_eq!(
                backend.state().screenshot.flash.map(|flash| flash.target),
                Some(ScreenshotTarget::Pane {
                    id,
                    attachment: Some(backend.state().runtime_epoch),
                })
            );
        });
    }

    /// The palette that ran the action is gone from the frame, and so is the dim it cast, which
    /// would otherwise still be fading out when the next frame paints.
    #[test]
    fn screenshot_ui_from_the_palette_saves_the_ui_as_it_was_before_the_palette_opened() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend_writing_to(dir.path());
            backend.render();
            let at_rest = backend.capture_frame().to_ansi_text();

            // A script's capture-ui is not the action: it takes the frame without a flash.
            let (request, reply) = capture_ui_request(CaptureRender::Text);
            backend.dispatch(request).unwrap();
            capture_ui_content(&reply);
            assert_eq!(backend.state().screenshot.flash, None);

            backend
                .dispatch(Msg::RunAction(Action::TogglePalette))
                .unwrap();
            // Long enough for the dim behind the palette to finish deepening.
            backend.advance(Duration::from_secs(1));
            assert!(backend.state().show_palette);
            assert!(
                backend
                    .capture_frame()
                    .plain_text()
                    .contains("Search commands")
            );

            backend
                .update_level(Msg::RunAction(Action::ScreenshotUi))
                .unwrap();
            assert!(!backend.state().show_palette);
            // Waiting for the same frame as the screenshot, so they show what it shows.
            let (ansi, ansi_reply) = capture_ui_request(CaptureRender::Ansi);
            let (png, png_reply) = capture_ui_request(CaptureRender::Png);
            backend.update_level(ansi).unwrap();
            backend.update_level(png).unwrap();
            let toast = screenshot_toast(&mut backend);

            let CaptureContent::Ansi { text } = capture_ui_content(&ansi_reply) else {
                panic!("expected ansi");
            };
            assert!(
                !text.contains("Search commands"),
                "the palette was captured"
            );
            assert_eq!(text, at_rest, "the frame still carries the palette's dim");

            let files = pngs_in(dir.path());
            assert_eq!(files.len(), 1, "{files:?}");
            let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("rozi-ui-"), "{name}");
            assert!(toast.contains(&name), "{toast}");
            let CaptureContent::Png { png_base64 } = capture_ui_content(&png_reply) else {
                panic!("expected png");
            };
            use base64::Engine as _;
            assert_eq!(
                std::fs::read(&files[0]).unwrap(),
                base64::engine::general_purpose::STANDARD
                    .decode(png_base64)
                    .unwrap(),
                "the file is capture-ui's PNG of that frame"
            );
            assert!(!crate::ops::control::ui_screenshot_waiting(backend.state()));
            assert_eq!(
                backend.state().screenshot.flash.map(|flash| flash.target),
                Some(ScreenshotTarget::Ui)
            );

            // The flash is drawn after the frame was taken, never before.
            backend.render();
            let (target, strength) = backend
                .state()
                .screenshot
                .flash_frame
                .get()
                .expect("the flash is running");
            assert_eq!(target, ScreenshotTarget::Ui);
            assert!(strength > 0.0);
            let workbar = |ansi: &str| ansi.lines().next().unwrap_or_default().to_owned();
            let rest_bar = workbar(&at_rest);
            assert_ne!(
                workbar(&backend.capture_frame().to_ansi_text()),
                rest_bar,
                "the flash is not drawn over the UI"
            );
            backend.advance(Duration::from_secs(1));
            assert_eq!(backend.state().screenshot.flash_frame.get(), None);
            assert_eq!(workbar(&backend.capture_frame().to_ansi_text()), rest_bar);
        });
    }

    /// The dim settles for the frame a screenshot waits for, and only that long: an earlier
    /// screenshot's file landing does not end a later one's wait.
    #[test]
    fn a_ui_screenshot_waits_until_its_own_frame_is_taken() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend_writing_to(dir.path());
            let waiting = |backend: &TestBackend<AppRoot>| {
                crate::ops::control::ui_screenshot_waiting(backend.state())
            };

            backend
                .update_level(Msg::RunAction(Action::ScreenshotUi))
                .unwrap();
            assert!(waiting(&backend));
            backend.render();
            assert!(!waiting(&backend), "the first frame has been taken");

            backend
                .update_level(Msg::RunAction(Action::ScreenshotUi))
                .unwrap();
            assert!(waiting(&backend));
            // The first screenshot's write reports back before the second frame is painted.
            backend
                .update_level(Msg::ScreenshotSaved {
                    target: ScreenshotTarget::Ui,
                    path: dir.path().join("first.png"),
                })
                .unwrap();
            assert!(waiting(&backend), "the second frame has not been taken");
            backend.render();
            assert!(!waiting(&backend));

            // A script's capture-ui leaves the dim to fade as usual.
            let (request, _reply) = capture_ui_request(CaptureRender::Text);
            backend.update_level(request).unwrap();
            assert!(!waiting(&backend));
        });
    }

    #[test]
    fn with_animations_off_a_screenshot_is_confirmed_without_a_flash() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend_writing_to(dir.path());
            backend.state_mut().config.animations.enabled = false;

            backend
                .dispatch(Msg::RunAction(Action::ScreenshotPane))
                .unwrap();
            let toast = screenshot_toast(&mut backend);

            assert!(toast.starts_with("Screenshot saved"), "{toast}");
            assert_eq!(backend.state().screenshot.flash, None);
            backend.render();
            assert_eq!(backend.state().screenshot.flash_frame.get(), None);
        });
    }

    #[test]
    fn a_directory_that_cannot_be_used_is_an_error_toast() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let not_a_directory = dir.path().join("taken");
            std::fs::write(&not_a_directory, b"keep").unwrap();
            let mut backend = backend_writing_to(&not_a_directory);

            backend
                .dispatch(Msg::RunAction(Action::ScreenshotUi))
                .unwrap();
            let toast = screenshot_toast(&mut backend);

            assert!(toast.starts_with("Screenshot failed"), "{toast}");
            assert!(toast.contains("not a directory"), "{toast}");
            assert_eq!(std::fs::read(&not_a_directory).unwrap(), b"keep");
            assert!(!crate::ops::control::ui_screenshot_waiting(backend.state()));
            assert_eq!(backend.state().screenshot.flash, None);
        });
    }

    #[test]
    fn screenshot_pane_needs_a_focused_pane() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        state.set_focused_pane(Some(1));
        assert!(crate::commands::command_available(
            Action::ScreenshotPane,
            &state
        ));
        assert!(crate::commands::command_available(
            Action::ScreenshotUi,
            &state
        ));
        state.set_focused_pane(None);
        assert!(!crate::commands::command_available(
            Action::ScreenshotPane,
            &state
        ));
        assert!(crate::commands::command_available(
            Action::ScreenshotUi,
            &state
        ));
    }

    #[test]
    fn screenshots_in_one_second_take_numbered_names_and_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let stem = "rozi-ui-20260925-132300";
        std::fs::write(dir.path().join(format!("{stem}.png")), b"keep").unwrap();

        let second = write_unique(dir.path(), stem, b"two").unwrap();
        let third = write_unique(dir.path(), stem, b"three").unwrap();

        assert_eq!(second, dir.path().join(format!("{stem}-2.png")));
        assert_eq!(third, dir.path().join(format!("{stem}-3.png")));
        assert_eq!(
            std::fs::read(dir.path().join(format!("{stem}.png"))).unwrap(),
            b"keep"
        );
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        assert_eq!(std::fs::read(&third).unwrap(), b"three");
    }

    #[test]
    fn file_names_carry_the_target_and_local_time() {
        use chrono::TimeZone as _;
        let at = chrono::Local
            .with_ymd_and_hms(2026, 9, 25, 13, 23, 7)
            .single()
            .unwrap();
        assert_eq!(
            file_stem(
                ScreenshotTarget::Pane {
                    id: 12,
                    attachment: None,
                },
                at
            ),
            "rozi-pane-12-20260925-132307"
        );
        assert_eq!(
            file_stem(ScreenshotTarget::Ui, at),
            "rozi-ui-20260925-132307"
        );
    }
}
