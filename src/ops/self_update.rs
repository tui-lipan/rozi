//! **Update rozi**: install the newer release from a card of rozi's own, then turn the card into
//! Restart rozi.
//!
//! A managed install runs rozi's installer in this process, so the card shows the real download
//! and then the switch. An install a package manager owns runs that manager's command, without a
//! terminal, and the card shows the lines it prints. Either way the update belongs to this client:
//! it installs the rozi this client was started from, whichever session is in front.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::state::{SelfUpdateEvent, SelfUpdateRun, UpdateChannel, UpdatePhase};
use crate::{AppRoot, Msg};

/// How long the card shows the installed release before it becomes the restart dialog: long
/// enough to read that it worked, short enough not to be waited on.
const SETTLE: Duration = Duration::from_millis(700);
/// A package manager gets this long. A source build (`cargo install`) is the slow one.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How much of a package manager's output is kept, beyond the lines the card shows.
const COMMAND_CAPTURE: usize = 64 * 1024;
/// Download progress is drawn at most this often: relswap reports every 64 KiB.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(33);

/// **Update rozi**: show the card of an update already running, or start one.
pub(crate) fn start(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(run) = ctx.state.self_update.run.as_mut() {
        run.hidden = false;
        focus_default(ctx);
        return Update::full();
    }
    let Some(update) = ctx.state.available_update.clone() else {
        return Update::none();
    };
    let Some(channel) = update.channel() else {
        return Update::none();
    };
    ctx.state.self_update.generation += 1;
    let generation = ctx.state.self_update.generation;
    let phase = match channel {
        UpdateChannel::Managed => UpdatePhase::Downloading {
            downloaded: 0,
            total: None,
        },
        UpdateChannel::Command(_) => UpdatePhase::Running,
    };
    ctx.state.self_update.run = Some(SelfUpdateRun {
        generation,
        running: update.running().clone(),
        latest: update.latest.clone(),
        channel: channel.clone(),
        phase,
        hidden: false,
        output: Default::default(),
        downloaded: None,
    });
    ctx.state.update_started = true;
    ctx.state.commands_dirty = true;
    crate::pane::pty_events::dismiss_update_notice(ctx);
    focus_default(ctx);
    // A client that may not reach the release host never installs from it either; tests drive the
    // card through its messages instead.
    if !ctx.state.update_checks_allowed {
        return Update::full();
    }
    let shell = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    Update::with_command(Command::spawn(move |link: CommandLink<Msg>| {
        std::thread::spawn(move || {
            let result = match channel {
                UpdateChannel::Managed => install_managed(generation, &link),
                UpdateChannel::Command(command) => run_command(generation, command, &shell, &link),
            };
            link.send(Msg::SelfUpdateFinished { generation, result });
        });
    }))
}

pub(crate) fn progress(
    ctx: &mut Context<AppRoot>,
    generation: u64,
    event: SelfUpdateEvent,
) -> Update {
    let Some(run) = current_run(ctx, generation) else {
        return Update::none();
    };
    run.advance(event);
    if run.hidden {
        Update::none()
    } else {
        Update::full()
    }
}

/// The worker is done. Success holds the installed release on the card for a moment and then
/// turns the card into Restart rozi; failure stays on the card with the reason, or becomes a toast
/// if the card was put away.
pub(crate) fn finished(
    ctx: &mut Context<AppRoot>,
    generation: u64,
    result: std::result::Result<(), String>,
) -> Update {
    let Some(run) = current_run(ctx, generation) else {
        return Update::none();
    };
    match result {
        Ok(()) => {
            run.phase = UpdatePhase::Installed;
            if run.hidden || !ctx.state.config.animations.enabled {
                return settled(ctx, generation);
            }
            Update::with_command(Command::after(SETTLE, move |link: CommandLink<Msg>| {
                link.send(Msg::SelfUpdateSettled { generation })
            }))
        }
        Err(reason) => {
            let hidden = run.hidden;
            run.phase.fail(reason.clone());
            // The same update can be tried again, from the card or from Commands.
            ctx.state.update_started = false;
            ctx.state.commands_dirty = true;
            if hidden {
                ctx.state.self_update.run = None;
                return crate::pane::pty_events::notify_error(
                    ctx,
                    "rozi update failed",
                    crate::pane::pty_events::ToastBody::new(reason)
                        .hint("run Update rozi to try again"),
                )
                .update();
            }
            crate::ops::focus::request_dialog_answer_focus(ctx, crate::view::DIALOG_AFFIRM);
            Update::full()
        }
    }
}

/// The installed release has had its moment: the card becomes Restart rozi for it.
pub(crate) fn settled(ctx: &mut Context<AppRoot>, generation: u64) -> Update {
    let installed = ctx
        .state
        .self_update
        .run
        .as_ref()
        .is_some_and(|run| run.generation == generation && run.phase == UpdatePhase::Installed);
    if !installed {
        return Update::none();
    }
    let Some(run) = ctx.state.self_update.run.take() else {
        return Update::none();
    };
    let note = ctx
        .state
        .available_update
        .as_ref()
        .and_then(crate::ops::update_check::AvailableUpdate::extension_note);
    crate::ops::relaunch::open_after_update(ctx, run.latest, note).merge(Update::full())
}

/// Put the card away. An update in progress keeps going; a failed one is dismissed.
pub(crate) fn hide(ctx: &mut Context<AppRoot>) -> Update {
    let Some(run) = ctx.state.self_update.run.as_mut() else {
        return Update::none();
    };
    match run.phase {
        UpdatePhase::Installed => return Update::none(),
        UpdatePhase::Failed { .. } => ctx.state.self_update.run = None,
        _ => run.hidden = true,
    }
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

/// Run a failed update again, on the same card.
pub(crate) fn retry(ctx: &mut Context<AppRoot>) -> Update {
    let failed = ctx
        .state
        .self_update
        .run
        .as_ref()
        .is_some_and(|run| matches!(run.phase, UpdatePhase::Failed { .. }));
    if !failed {
        return Update::none();
    }
    ctx.state.self_update.run = None;
    start(ctx)
}

pub(crate) fn focus_answer(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if ctx.state.self_update.visible() {
        crate::ops::focus::request_dialog_answer_focus(ctx, index);
    }
    Update::full()
}

/// The answer the card opens on: **Hide** while working, **Try again** once failed.
fn focus_default(ctx: &mut Context<AppRoot>) {
    let failed = ctx
        .state
        .self_update
        .run
        .as_ref()
        .is_some_and(|run| matches!(run.phase, UpdatePhase::Failed { .. }));
    let index = if failed {
        crate::view::DIALOG_AFFIRM
    } else {
        crate::view::DIALOG_REFUSE
    };
    crate::ops::focus::request_dialog_answer_focus(ctx, index);
}

fn current_run(ctx: &mut Context<AppRoot>, generation: u64) -> Option<&mut SelfUpdateRun> {
    ctx.state
        .self_update
        .run
        .as_mut()
        .filter(|run| run.generation == generation)
}

/// rozi's own installer, in this process: download with progress, then verify and switch.
fn install_managed(generation: u64, link: &CommandLink<Msg>) -> std::result::Result<(), String> {
    use crate::platform::install::InstallError;
    let reporter = Arc::new(DownloadReporter {
        generation,
        link: Mutex::new(link.clone()),
        last: Mutex::new(None),
    });
    let installation = crate::platform::install::from_process_with_progress(reporter.clone());
    let result = installation.update().map_err(|error| match error {
        InstallError::Unmanaged => "this rozi is not managed by its installer".to_string(),
        error => crate::platform::install::describe_error(&error),
    })?;
    // The installed binary carries the new skill document; registered copies follow it. A copy
    // that cannot be refreshed keeps working and is reported by `rozi skill status`.
    let _ = crate::skill::refresh_after_update(
        installation.command_path(),
        &result.version.to_string(),
    );
    Ok(())
}

/// Turns relswap's byte counts into card progress: only the archive, at a steady rate, and the
/// moment it is complete the card moves on to installing.
struct DownloadReporter {
    generation: u64,
    link: Mutex<CommandLink<Msg>>,
    last: Mutex<Option<Instant>>,
}

impl relswap::ProgressObserver for DownloadReporter {
    fn advance(&self, downloaded: u64, total: Option<u64>) {
        if !crate::platform::progress::worth_drawing(total) {
            return;
        }
        let complete = total.is_some_and(|total| downloaded >= total);
        {
            let mut last = self
                .last
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !complete && last.is_some_and(|last| last.elapsed() < PROGRESS_INTERVAL) {
                return;
            }
            *last = Some(Instant::now());
        }
        let link = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let generation = self.generation;
        link.send(Msg::SelfUpdateProgress {
            generation,
            event: SelfUpdateEvent::Downloaded { downloaded, total },
        });
        if complete {
            link.send(Msg::SelfUpdateProgress {
                generation,
                event: SelfUpdateEvent::Installing,
            });
        }
    }
}

/// The package manager that owns this install, with no terminal: its input is closed, so a
/// command that would stop to ask fails instead of waiting for an answer that cannot come.
fn run_command(
    generation: u64,
    command: &'static str,
    shell: &crate::platform::command::ShellCommand,
    link: &CommandLink<Msg>,
) -> std::result::Result<(), String> {
    let lines = Arc::new(Mutex::new(LineBuffer::default()));
    let sink = lines.clone();
    let output_link = Mutex::new(link.clone());
    let observer: crate::platform::command::OutputObserver = Arc::new(move |chunk: &[u8]| {
        let complete = sink
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(chunk);
        if !complete.is_empty() {
            output_link
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .send(Msg::SelfUpdateProgress {
                    generation,
                    event: SelfUpdateEvent::Output(complete),
                });
        }
    });
    let output = crate::platform::command::run_observed_shell_command(
        shell,
        command,
        COMMAND_TIMEOUT,
        COMMAND_CAPTURE,
        observer,
    )
    .map_err(|error| format!("could not run `{command}`: {error}"))?;
    let rest = lines
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .finish();
    if !rest.is_empty() {
        link.send(Msg::SelfUpdateProgress {
            generation,
            event: SelfUpdateEvent::Output(rest),
        });
    }
    match output.status {
        _ if output.timed_out => Err(format!(
            "`{command}` took longer than {} minutes",
            COMMAND_TIMEOUT.as_secs() / 60
        )),
        Some(0) => Ok(()),
        Some(code) => Err(format!("`{command}` exited with {code}")),
        None => Err(format!("`{command}` was stopped")),
    }
}

/// Splits a command's output into the lines a person would see: a carriage return starts the line
/// over, as a progress meter means it to, and escape sequences and other control characters go.
#[derive(Default)]
struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    /// Take `chunk`, returning the lines it completed.
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut complete = Vec::new();
        for &byte in chunk {
            match byte {
                b'\n' => {
                    let line = clean(&self.pending);
                    self.pending.clear();
                    if !line.is_empty() {
                        complete.push(line);
                    }
                }
                b'\r' => self.pending.clear(),
                _ => self.pending.push(byte),
            }
        }
        complete
    }

    /// The last line, if the command ended without a newline.
    fn finish(&mut self) -> Vec<String> {
        let line = clean(&self.pending);
        self.pending.clear();
        if line.is_empty() { vec![] } else { vec![line] }
    }
}

/// `bytes` as text, without escape sequences or control characters, trimmed.
fn clean(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // CSI: `ESC [` parameters, then one final byte in `@`..`~`. Anything else after `ESC`
            // is a two-character sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if ch == '\t' {
            out.push(' ');
        } else if !ch.is_control() {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::install_source::InstallSource;
    use tui_lipan::TestBackend;

    const MB: u64 = 1024 * 1024;

    /// A client that was told of 9.9.9, raised its toast, and has just run **Update rozi**. Its
    /// worker never starts: `update_checks_allowed` is off in a test app, so the test plays the
    /// worker's part.
    fn updating(source: InstallSource) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 120,
            h: 40,
        });
        backend.state_mut().config.animations.enabled = false;
        backend
            .dispatch(Msg::UpdateAvailable {
                update: crate::ops::update_check::AvailableUpdate::for_test("9.9.9", source, None),
                announce: true,
            })
            .expect("the release is announced");
        assert!(notice_raised(&backend), "the release has its toast");
        backend
            .dispatch(Msg::RunAction(crate::input::Action::UpdateRozi))
            .expect("Update rozi runs");
        backend
    }

    fn notice_raised(backend: &TestBackend<AppRoot>) -> bool {
        backend.state().replaceable_toasts.contains_key(
            &crate::pane::pty_events::ToastKey::Channel(crate::state::ToastChannel::UpdateNotice),
        )
    }

    fn generation(backend: &TestBackend<AppRoot>) -> u64 {
        backend
            .state()
            .self_update
            .run
            .as_ref()
            .expect("an update is running")
            .generation
    }

    fn report(backend: &mut TestBackend<AppRoot>, event: SelfUpdateEvent) {
        let generation = generation(backend);
        backend
            .dispatch(Msg::SelfUpdateProgress { generation, event })
            .unwrap();
    }

    fn finish(backend: &mut TestBackend<AppRoot>, result: std::result::Result<(), String>) {
        let generation = generation(backend);
        backend
            .dispatch(Msg::SelfUpdateFinished { generation, result })
            .unwrap();
    }

    fn screen(backend: &mut TestBackend<AppRoot>) -> String {
        backend.render();
        backend.capture_frame().plain_text()
    }

    #[test]
    fn the_card_follows_the_download_and_becomes_restart_rozi() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Managed);
            let versions = format!("v{} → v9.9.9", env!("CARGO_PKG_VERSION"));
            assert!(
                !notice_raised(&backend),
                "the card replaces the release toast"
            );
            let opening = screen(&mut backend);
            for expected in [
                "Updating rozi",
                versions.as_str(),
                "Download",
                "Install",
                "Hide",
            ] {
                assert!(
                    opening.contains(expected),
                    "missing {expected:?} in\n{opening}"
                );
            }

            report(
                &mut backend,
                SelfUpdateEvent::Downloaded {
                    downloaded: 9 * MB,
                    total: Some(18 * MB),
                },
            );
            let halfway = screen(&mut backend);
            assert!(halfway.contains("50%"), "{halfway}");

            report(
                &mut backend,
                SelfUpdateEvent::Downloaded {
                    downloaded: 18 * MB,
                    total: Some(18 * MB),
                },
            );
            report(&mut backend, SelfUpdateEvent::Installing);
            let installing = screen(&mut backend);
            assert!(installing.contains("Installing…"), "{installing}");
            assert!(
                installing.contains("18.0 MB"),
                "the finished download keeps its size:\n{installing}"
            );

            // With animations off the card does not pause on the installed release.
            finish(&mut backend, Ok(()));
            assert!(backend.state().self_update.run.is_none());
            let dialog = backend
                .state()
                .relaunch
                .dialog
                .as_ref()
                .expect("the card becomes Restart rozi");
            assert_eq!(dialog.expected, Some(semver::Version::new(9, 9, 9)));
            assert!(
                backend.state().update_started,
                "an installed update is not offered again"
            );
        });
    }

    #[test]
    fn a_failed_update_says_why_and_tries_again_on_the_same_card() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Managed);
            let first = generation(&backend);
            finish(
                &mut backend,
                Err("could not reach the release server".into()),
            );
            let failed = screen(&mut backend);
            for expected in [
                "rozi update failed",
                "could not reach the release server",
                "Failed",
                "Try again",
            ] {
                assert!(
                    failed.contains(expected),
                    "missing {expected:?} in\n{failed}"
                );
            }
            assert!(!backend.state().update_started, "{failed}");

            backend.dispatch(Msg::SelfUpdateRetry).unwrap();
            let again = generation(&backend);
            assert!(again > first);
            // What the abandoned worker still sends is ignored.
            backend
                .dispatch(Msg::SelfUpdateFinished {
                    generation: first,
                    result: Ok(()),
                })
                .unwrap();
            assert!(backend.state().relaunch.dialog.is_none());
            assert!(screen(&mut backend).contains("Updating rozi"));
        });
    }

    #[test]
    fn a_hidden_update_keeps_going_and_comes_back_through_update_rozi() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Managed);
            backend.dispatch(Msg::SelfUpdateHide).unwrap();
            assert!(!backend.state().self_update.visible());
            assert!(!screen(&mut backend).contains("Updating rozi"));
            assert!(
                crate::commands::command_available(
                    crate::input::Action::UpdateRozi,
                    backend.state()
                ),
                "Update rozi brings the card back"
            );

            backend
                .dispatch(Msg::RunAction(crate::input::Action::UpdateRozi))
                .unwrap();
            assert!(backend.state().self_update.visible());

            backend.dispatch(Msg::SelfUpdateHide).unwrap();
            finish(&mut backend, Ok(()));
            assert!(
                backend.state().relaunch.dialog.is_some(),
                "a hidden update still offers the restart once it is done"
            );
        });
    }

    #[test]
    fn a_package_manager_update_shows_its_command_and_what_it_prints() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Homebrew);
            report(
                &mut backend,
                SelfUpdateEvent::Output(
                    [
                        "one",
                        "two",
                        "three",
                        "==> Upgrading rozi",
                        "==> Pouring rozi",
                    ]
                    .map(String::from)
                    .to_vec(),
                ),
            );
            let running = screen(&mut backend);
            for expected in [
                "brew upgrade rozi",
                "Running…",
                "==> Upgrading rozi",
                "==> Pouring rozi",
            ] {
                assert!(
                    running.contains(expected),
                    "missing {expected:?} in\n{running}"
                );
            }
            assert!(
                !running.contains("one"),
                "only the newest lines stay:\n{running}"
            );

            finish(
                &mut backend,
                Err("`brew upgrade rozi` exited with 1".into()),
            );
            let failed = screen(&mut backend);
            assert!(failed.contains("exited with 1"), "{failed}");
            assert!(
                failed.contains("==> Pouring rozi"),
                "the output stays to read:\n{failed}"
            );
        });
    }

    #[test]
    fn output_reads_as_the_lines_a_terminal_would_leave() {
        let mut buffer = LineBuffer::default();
        assert_eq!(
            buffer.push(b"\x1b[1;32m==>\x1b[0m Fetching rozi\nDownload  10%\rDownload 100%\n"),
            vec!["==> Fetching rozi", "Download 100%"],
            "colour goes, and a carriage return rewrites the line"
        );
        assert_eq!(buffer.push(b"Pouring rozi"), Vec::<String>::new());
        assert_eq!(buffer.push(b"--0.0.44\n\n"), vec!["Pouring rozi--0.0.44"]);
        assert_eq!(buffer.push(b"done"), Vec::<String>::new());
        assert_eq!(buffer.finish(), vec!["done"]);
    }
}
