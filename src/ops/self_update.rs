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
        UpdateChannel::Command { .. } => UpdatePhase::Running,
    };
    let stop = matches!(channel, UpdateChannel::Command { .. })
        .then(|| Arc::new(crate::platform::command::CommandStop::default()));
    let running = update.running().clone();
    let latest = update.latest.clone();
    ctx.state.self_update.run = Some(SelfUpdateRun {
        generation,
        running: running.clone(),
        latest: update.latest.clone(),
        note: update.extension_note(),
        channel: channel.clone(),
        phase,
        hidden: false,
        output: Default::default(),
        downloaded: None,
        installed: None,
        stop: stop.clone(),
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
        // This thread spawns a package manager's command and waits on it for as long as it runs,
        // which is what lets Linux end the command with this process; the run's `stop` ends it
        // when this client leaves first.
        std::thread::spawn(move || {
            let result = match (channel, stop) {
                (UpdateChannel::Command { command, source }, Some(stop)) => run_command(
                    generation,
                    (command, source),
                    (&running, &latest),
                    &shell,
                    stop,
                    &link,
                ),
                _ => install_managed(generation, &running, &link),
            };
            link.send(Msg::SelfUpdateFinished { generation, result });
        });
    }))
}

/// This client is leaving: end a package manager's command that is still running, rather than
/// leave it working on the install with nobody to report to. rozi's own installer runs in this
/// process and ends with it; its install is journaled, so the next start finishes or undoes it.
pub(crate) fn stop_for_exit(state: &mut crate::state::State) {
    if let Some(stop) = state
        .self_update
        .run
        .as_mut()
        .filter(|run| run.phase.in_progress())
        .and_then(|run| run.stop.take())
    {
        stop.stop_and_wait(Duration::from_secs(1));
    }
}

/// Whether an update is changing the installed rozi right now. Restart rozi waits for it: the
/// build it would start is the one being replaced.
pub(crate) fn in_progress(state: &crate::state::State) -> bool {
    state
        .self_update
        .run
        .as_ref()
        .is_some_and(|run| run.phase.in_progress())
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
    result: std::result::Result<semver::Version, String>,
) -> Update {
    let Some(run) = current_run(ctx, generation) else {
        return Update::none();
    };
    run.stop = None;
    match result {
        Ok(installed) => {
            // A download of unknown length never says it is done; what arrived is its size.
            if let UpdatePhase::Downloading { downloaded, .. } = run.phase
                && downloaded > 0
            {
                run.downloaded.get_or_insert(downloaded);
            }
            run.installed = Some(installed.clone());
            run.phase = UpdatePhase::Installed;
            settle_offer(ctx, &installed);
            let Some(run) = current_run(ctx, generation) else {
                return Update::none();
            };
            if run.hidden {
                return installed_in_background(ctx);
            }
            if !ctx.state.config.animations.enabled {
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
                        .hint("use `Update rozi` to try again"),
                )
                .update();
            }
            crate::ops::focus::request_dialog_answer_focus(ctx, crate::view::DIALOG_AFFIRM);
            Update::full()
        }
    }
}

/// Square the release this client offers with the one just installed. A check that ran during the
/// update may have announced a newer release, which the updater, asking for the latest itself,
/// may have installed. An offer for that release or an older one is settled - no more **Update
/// rozi**, no notice - and when it is exactly the installed release its metadata is that release's,
/// so its note now counts. A strictly newer offer stands.
fn settle_offer(ctx: &mut Context<AppRoot>, installed: &semver::Version) {
    let Some(offered) = ctx.state.available_update.as_ref() else {
        return;
    };
    if offered.latest > *installed {
        return;
    }
    if offered.latest == *installed
        && let Some(run) = ctx.state.self_update.run.as_mut()
    {
        run.latest = installed.clone();
        run.note = offered.extension_note();
    }
    ctx.state.update_started = true;
    ctx.state.commands_dirty = true;
    crate::pane::pty_events::dismiss_update_notice(ctx);
}

/// An update in the background is done. The user chose to keep working, so nothing takes the
/// keyboard: a toast says the release is installed, and a click on it, or **Restart rozi** from
/// Commands whenever suits, takes the restart.
fn installed_in_background(ctx: &mut Context<AppRoot>) -> Update {
    let Some(run) = ctx.state.self_update.run.take() else {
        return Update::none();
    };
    // Its card was what brought Update rozi back, and it is gone: the command goes too.
    ctx.state.commands_dirty = true;
    let version = run.release().clone();
    let note = run.note_for(&version);
    crate::pane::pty_events::notify_update_installed(
        ctx,
        format!("rozi v{version} installed"),
        crate::pane::pty_events::ToastBody::new(format!("v{} → v{version}", run.running))
            .hint("click to restart now, or use `Restart rozi` later"),
        Msg::SelfUpdateRestart { version, note },
    )
    .update()
}

/// The installed-release toast was clicked: open Restart rozi for exactly that release, unless a
/// restart dialog is already up.
pub(crate) fn restart_installed(
    ctx: &mut Context<AppRoot>,
    version: semver::Version,
    note: Option<String>,
) -> Update {
    if ctx.state.relaunch.dialog.is_some() {
        return Update::none();
    }
    crate::ops::relaunch::open_after_update(ctx, version, note).merge(Update::full())
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
    ctx.state.commands_dirty = true;
    let version = run.release().clone();
    let note = run.note_for(&version);
    crate::ops::relaunch::open_after_update(ctx, version, note).merge(Update::full())
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

/// The answer the card opens on: **Run in background** while working, **Try again** once failed.
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

/// rozi's own installer, in this process: download with progress, then verify and switch. Returns
/// the release it activated, which is the newest one published when it ran - not necessarily the
/// one the check announced earlier.
fn install_managed(
    generation: u64,
    running: &semver::Version,
    link: &CommandLink<Msg>,
) -> std::result::Result<semver::Version, String> {
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
    newer_than_running(result.version, running, "rozi's installer")
}

/// `installed`, when it is a release this client can move to; otherwise why the update left
/// nothing to restart onto.
fn newer_than_running(
    installed: semver::Version,
    running: &semver::Version,
    by: &str,
) -> std::result::Result<semver::Version, String> {
    if installed > *running {
        Ok(installed)
    } else {
        Err(format!(
            "{by} finished, but the installed rozi is v{installed}, not newer than this one"
        ))
    }
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
///
/// Its exit status only says the command is content. Success is the rozi it left installed: found
/// where this channel installs - and only there - asked what it is, and newer than this client.
fn run_command(
    generation: u64,
    (command, source): (&'static str, crate::platform::install_source::InstallSource),
    (running, latest): (&semver::Version, &semver::Version),
    shell: &crate::platform::command::ShellCommand,
    stop: Arc<crate::platform::command::CommandStop>,
    link: &CommandLink<Msg>,
) -> std::result::Result<semver::Version, String> {
    use crate::platform::command::OutputStream;
    // One buffer per stream: a line half-written to stdout must not be finished by stderr.
    let lines = Arc::new(Mutex::new([LineBuffer::default(), LineBuffer::default()]));
    let sink = lines.clone();
    let output_link = Mutex::new(link.clone());
    let observer: crate::platform::command::OutputObserver =
        Arc::new(move |stream: OutputStream, chunk: &[u8]| {
            let index = usize::from(stream == OutputStream::Stderr);
            let complete =
                sink.lock().unwrap_or_else(|poison| poison.into_inner())[index].push(chunk);
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
        stop,
    )
    .map_err(|error| format!("could not run `{command}`: {error}"))?;
    let rest: Vec<String> = lines
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter_mut()
        .flat_map(LineBuffer::finish)
        .collect();
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
        Some(0) => {
            let installed = crate::relaunch::candidate::newest_in_channel(source, latest)
                .map_err(|missing| format!("`{command}` finished, but {missing}"))?;
            newer_than_running(installed.release, running, &format!("`{command}`"))
        }
        Some(code) => Err(format!("`{command}` exited with {code}")),
        None => Err(format!("`{command}` was stopped")),
    }
}

/// Splits one stream of a command's output into the lines a person would see: a carriage return
/// alone starts the line over, as a progress meter means it to, while `\r\n` simply ends it; and
/// terminal escape sequences and other control characters go.
#[derive(Default)]
struct LineBuffer {
    pending: Vec<u8>,
    /// A carriage return was the last byte: what follows decides whether it ended the line
    /// (`\r\n`) or started it over.
    after_return: bool,
}

impl LineBuffer {
    /// Take `chunk`, returning the lines it completed.
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut complete = Vec::new();
        for &byte in chunk {
            if std::mem::take(&mut self.after_return) && byte != b'\n' {
                self.pending.clear();
            }
            match byte {
                b'\n' => {
                    let line = clean(&self.pending);
                    self.pending.clear();
                    if !line.is_empty() {
                        complete.push(line);
                    }
                }
                b'\r' => self.after_return = true,
                _ => self.pending.push(byte),
            }
        }
        complete
    }

    /// The last line, if the command ended without a newline.
    fn finish(&mut self) -> Vec<String> {
        self.after_return = false;
        let line = clean(&self.pending);
        self.pending.clear();
        if line.is_empty() { vec![] } else { vec![line] }
    }
}

/// `bytes` as the text a terminal would show, trimmed: every escape sequence goes, whether 7-bit
/// (`ESC [`, `ESC ]`, …) or its 8-bit C1 form, and so does every other control character.
fn clean(bytes: &[u8]) -> String {
    const ESC: char = '\u{1b}';
    const BEL: char = '\u{7}';
    /// The 8-bit String Terminator, and the 7-bit one's second character after `ESC`.
    const ST: char = '\u{9c}';

    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Skip a control string to its terminator: ST, `ESC \`, or BEL, which xterm also accepts.
    let skip_string = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        while let Some(next) = chars.next() {
            match next {
                BEL | ST => return,
                ESC if chars.peek() == Some(&'\\') => {
                    chars.next();
                    return;
                }
                _ => {}
            }
        }
    };
    // Skip a control sequence's parameters and intermediates to its final byte.
    let skip_csi = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        for next in chars.by_ref() {
            if ('@'..='~').contains(&next) {
                return;
            }
        }
    };
    while let Some(ch) = chars.next() {
        match ch {
            ESC => match chars.next() {
                Some('[') => skip_csi(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string(&mut chars),
                // `ESC`, any intermediates, then one final character.
                Some(' '..='/') => {
                    for next in chars.by_ref() {
                        if !(' '..='/').contains(&next) {
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\t' => out.push(' '),
            ch if ch.is_control() => {}
            ch => out.push(ch),
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

    fn finish(
        backend: &mut TestBackend<AppRoot>,
        result: std::result::Result<semver::Version, String>,
    ) {
        let generation = generation(backend);
        backend
            .dispatch(Msg::SelfUpdateFinished { generation, result })
            .unwrap();
    }

    fn screen(backend: &mut TestBackend<AppRoot>) -> String {
        backend.render();
        backend.capture_frame().plain_text()
    }

    /// What the toast for a release installed in the background says, if one was raised.
    fn installed_toast(backend: &TestBackend<AppRoot>) -> Option<String> {
        backend
            .state()
            .replaceable_toasts
            .values()
            .map(|tracked| tracked.content().replace('\u{0}', " "))
            .find(|content| content.contains(" installed"))
    }

    /// A release whose metadata raises the extension API, so it carries a release-wide note.
    fn raising_extension_api(version: &str) -> Msg {
        Msg::UpdateAvailable {
            update: crate::ops::update_check::AvailableUpdate::for_test(
                version,
                InstallSource::Managed,
                Some((
                    crate::config::EXTENSION_API_VERSION + 1,
                    crate::session::protocol::PROTOCOL_VERSION,
                )),
            ),
            announce: true,
        }
    }

    /// A client told of `version`, with `note` raised by its metadata when `warned`, that has just
    /// run **Update rozi**.
    fn updating_to(version: &str, warned: bool) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().config.animations.enabled = false;
        let update = if warned {
            raising_extension_api(version)
        } else {
            Msg::UpdateAvailable {
                update: crate::ops::update_check::AvailableUpdate::for_test(
                    version,
                    InstallSource::Managed,
                    None,
                ),
                announce: true,
            }
        };
        backend.dispatch(update).unwrap();
        backend
            .dispatch(Msg::RunAction(crate::input::Action::UpdateRozi))
            .unwrap();
        backend
    }

    fn restart_note(backend: &TestBackend<AppRoot>) -> Option<String> {
        let dialog = backend
            .state()
            .relaunch
            .dialog
            .as_ref()
            .expect("Restart rozi");
        dialog.note.clone()
    }

    /// What follows an update names the release it proved it installed, with that release's own
    /// metadata: the announced one keeps its note, a newer one published in between does not
    /// inherit it, and a warning announced while the update runs is not lent to it.
    #[test]
    fn the_release_named_after_an_update_is_the_one_it_installed() {
        crate::test_support::on_large_stack(|| {
            let mut announced = updating_to("9.9.9", true);
            finish(&mut announced, Ok(semver::Version::new(9, 9, 9)));
            assert!(
                restart_note(&announced).is_some(),
                "its own note travels with it"
            );

            let mut newer = updating_to("9.9.9", true);
            finish(&mut newer, Ok(semver::Version::new(9, 9, 10)));
            let dialog = newer.state().relaunch.dialog.as_ref().unwrap();
            assert_eq!(dialog.expected, Some(semver::Version::new(9, 9, 10)));
            assert_eq!(dialog.note, None, "9.9.9's note is not 9.9.10's");

            let mut overtaken = updating_to("9.9.9", false);
            overtaken.dispatch(raising_extension_api("9.9.10")).unwrap();
            finish(&mut overtaken, Ok(semver::Version::new(9, 9, 9)));
            assert_eq!(
                restart_note(&overtaken),
                None,
                "9.9.10's warning, announced meanwhile, is not 9.9.9's"
            );

            let mut background = updating_to("9.9.9", false);
            background.dispatch(Msg::SelfUpdateHide).unwrap();
            background
                .dispatch(raising_extension_api("9.9.10"))
                .unwrap();
            finish(&mut background, Ok(semver::Version::new(9, 9, 9)));
            let toast = installed_toast(&background).expect("the installed toast");
            assert!(toast.contains("rozi v9.9.9 installed"), "{toast}");
            let notice = background
                .state()
                .replaceable_toasts
                .get(&crate::pane::pty_events::ToastKey::Channel(
                    crate::state::ToastChannel::UpdateNotice,
                ))
                .expect("the newer release's notice is not replaced by the older news")
                .content()
                .to_string();
            assert!(notice.contains("v9.9.10"), "{notice}");
        });
    }

    /// Restart rozi would start the very build an update is replacing, so it waits for the update,
    /// whether its card is on screen or in the background.
    #[test]
    fn restart_rozi_waits_while_an_update_changes_the_install() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Homebrew);
            backend.dispatch(Msg::SelfUpdateHide).unwrap();
            assert!(!crate::commands::command_available(
                crate::input::Action::RestartRozi,
                backend.state()
            ));
            backend
                .dispatch(Msg::RunAction(crate::input::Action::RestartRozi))
                .unwrap();
            assert!(backend.state().relaunch.dialog.is_none());

            finish(
                &mut backend,
                Err("`brew upgrade rozi` exited with 1".into()),
            );
            assert!(crate::commands::command_available(
                crate::input::Action::RestartRozi,
                backend.state()
            ));
        });
    }

    /// The registry, not just the availability check, drops Update rozi once a background update
    /// finishes: with nothing modal before or after, only an explicit rebuild notices. The
    /// which-key strip lists exactly the registry's enabled entries, so it shows what a stale one
    /// would leave behind.
    #[test]
    fn a_background_update_that_finishes_takes_update_rozi_off_the_keys() {
        crate::test_support::on_large_stack(|| {
            let app = App::new()
                .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
                .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
                .command_chord_reveal_delay(Duration::ZERO);
            let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 120,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.config.animations.enabled = false;
                let binding = crate::config::scheme_shortcuts(&state.config.input, "y");
                state
                    .config
                    .key_overrides
                    .insert("update-rozi".to_string(), binding);
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
            }
            backend
                .dispatch(Msg::UpdateAvailable {
                    update: crate::ops::update_check::AvailableUpdate::for_test(
                        "9.9.9",
                        InstallSource::Managed,
                        None,
                    ),
                    announce: false,
                })
                .unwrap();
            backend
                .dispatch(Msg::RunAction(crate::input::Action::UpdateRozi))
                .unwrap();
            backend.dispatch(Msg::SelfUpdateHide).unwrap();

            let prefix = KeyEvent {
                code: KeyCode::Char('a'),
                mods: KeyMods::CTRL,
            };
            let strip = |backend: &mut TestBackend<AppRoot>| {
                backend.send_key(prefix).unwrap();
                let shown = screen(backend);
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Esc,
                        mods: KeyMods::NONE,
                    })
                    .unwrap();
                shown
            };
            let running = strip(&mut backend);
            assert!(
                running.contains("Update rozi"),
                "a background update is brought back from the keys:\n{running}"
            );

            finish(&mut backend, Ok(semver::Version::new(9, 9, 9)));
            let done = strip(&mut backend);
            assert!(
                !done.contains("Update rozi"),
                "nothing is left to update:\n{done}"
            );
        });
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
                "Run in background",
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
            finish(&mut backend, Ok(semver::Version::new(9, 9, 9)));
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
                    result: Ok(semver::Version::new(9, 9, 9)),
                })
                .unwrap();
            assert!(backend.state().relaunch.dialog.is_none());
            assert!(screen(&mut backend).contains("Updating rozi"));
        });
    }

    #[test]
    fn an_update_in_the_background_keeps_going_and_finishes_with_a_toast() {
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
            finish(&mut backend, Ok(semver::Version::new(9, 9, 9)));
            assert!(
                backend.state().relaunch.dialog.is_none(),
                "nothing takes the keyboard from a user who chose to keep working"
            );
            assert!(backend.state().self_update.run.is_none());
            let toast = installed_toast(&backend).expect("a toast says the release is installed");
            for expected in [
                "rozi v9.9.9 installed",
                "click to restart now",
                "Restart rozi",
            ] {
                assert!(
                    toast.contains(expected),
                    "missing {expected:?} in {toast:?}"
                );
            }

            // The toast's click.
            backend
                .dispatch(Msg::SelfUpdateRestart {
                    version: semver::Version::new(9, 9, 9),
                    note: None,
                })
                .unwrap();
            let dialog = backend
                .state()
                .relaunch
                .dialog
                .as_ref()
                .expect("the click offers the restart");
            assert_eq!(dialog.expected, Some(semver::Version::new(9, 9, 9)));
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

    /// Windows tools end lines with `\r\n`, which ends the line rather than starting it over -
    /// including when the two bytes arrive in separate chunks.
    #[test]
    fn a_crlf_line_ending_keeps_its_line() {
        let mut buffer = LineBuffer::default();
        assert_eq!(
            buffer.push(b"Updating 'rozi'\r\nInstalled\r"),
            vec!["Updating 'rozi'"]
        );
        assert_eq!(
            buffer.push(b"\nScanning 1%\rScanning 99%\r\n"),
            vec!["Installed", "Scanning 99%"]
        );
    }

    /// Every escape-sequence form a tool may print goes, whatever terminates it, and the text
    /// around it stays.
    #[test]
    fn every_kind_of_escape_sequence_is_stripped() {
        for (raw, shown) in [
            ("\x1b[1;32m==>\x1b[0m Pouring", "==> Pouring"),
            ("\x1b]0;brew upgrade\x07Upgrading", "Upgrading"),
            (
                "\x1b]8;;https://brew.sh\x1b\\link\x1b]8;;\x1b\\ text",
                "link text",
            ),
            ("\x1b(Bplain\x1b=", "plain"),
            ("\x1bP1$r0m\x1b\\after DCS", "after DCS"),
            ("\u{9b}31mC1 colour\u{9b}0m", "C1 colour"),
            ("\u{9d}2;title\u{9c}C1 title", "C1 title"),
            ("\ttabbed\u{7}", "tabbed"),
        ] {
            assert_eq!(clean(raw.as_bytes()), shown, "{raw:?}");
        }
    }

    /// The updater asked for the latest release itself and caught up to one a check announced
    /// while it ran. That offer is settled: its note is the installed release's, its notice goes,
    /// and choosing Later does not bring Update rozi back for a release already installed.
    #[test]
    fn an_update_that_catches_up_to_a_newer_offer_settles_it() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating_to("9.9.9", false);
            backend.dispatch(raising_extension_api("9.9.10")).unwrap();
            assert!(notice_raised(&backend));
            finish(&mut backend, Ok(semver::Version::new(9, 9, 10)));

            assert!(
                restart_note(&backend).is_some(),
                "9.9.10's own note belongs to the 9.9.10 just installed"
            );
            assert!(!notice_raised(&backend), "no notice for what is installed");
            backend.dispatch(Msg::RelaunchCancel).unwrap();
            assert!(!crate::commands::command_available(
                crate::input::Action::UpdateRozi,
                backend.state()
            ));
        });
    }

    /// A download whose length the server never said does not report that it finished, so the
    /// card learns it from the update succeeding: the size that arrived, and both steps done.
    #[test]
    fn a_download_of_unknown_length_ends_with_its_size() {
        crate::test_support::on_large_stack(|| {
            let mut backend = updating(InstallSource::Managed);
            // Animations on, so the installed card stays up until it is told to settle.
            backend.state_mut().config.animations.enabled = true;
            report(
                &mut backend,
                SelfUpdateEvent::Downloaded {
                    downloaded: 5 * MB,
                    total: None,
                },
            );
            finish(&mut backend, Ok(semver::Version::new(9, 9, 9)));
            // The card is still opening at this frame, so read what it draws from: the row for a
            // finished download shows `downloaded`, as the first test sees on screen.
            let run = backend.state().self_update.run.as_ref().unwrap();
            assert_eq!(run.phase, UpdatePhase::Installed);
            assert_eq!(run.downloaded, Some(5 * MB));
        });
    }
}
