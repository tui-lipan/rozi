//! This UI recording itself: `record-ui-start`, `record-ui-stop`, `record-ui-mark`, and the
//! Start/Stop UI recording command.
//!
//! The framework hands over each frame it paints while the recording holds its paint subscription,
//! and draws nothing for the recording's sake: an idle UI writes nothing, and a UI that is not
//! recording pays nothing. The `max_fps` ceiling is applied here. A frame painted sooner than the
//! ceiling allows waits, replaced by any newer one, until a timer writes it at the moment the
//! ceiling allows; that is the screen the terminal showed then. A frame that changes what the meta
//! events report is written at once, so the event lands on the screen it describes. A frame and
//! the events that happened on it reach the writer together or not at all: a mark first writes
//! the frame still waiting, and a frame the writer refuses keeps its meta events for the next try.
//! The last frame is written when it was painted, so the recording ends when it stopped. Turning
//! frames into spans, diffing, and the disk all happen on the writer thread.
//!
//! The file is written on this client's machine, like a screenshot, even when the session it
//! shows is remote, and with this client's `[recording]` settings.
//!
//! The recording indicator is part of what is recorded. A recording shows what the user saw, and
//! its frames are the ones the terminal got; leaving the indicator out would take a second render
//! of every frame, which would then no longer be the frame that was painted.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tui_lipan::PaintedFrame;
use tui_lipan::prelude::*;

use crate::control::{ControlErrorCode, ControlResponse, UiRecordingInfo, UiRecordingStopped};
use crate::pane::pty_events::{notify_error, notify_path_info};
use crate::recording::start::{Limits, Output, frame_interval};
use crate::recording::{EndReason, RecordingEvent, RecordingMeta, RecordingTarget};
use crate::state::{
    State, UiRecording, UiRecordingFile, UiRecordingPending, UiRecordingPhase, UiRecordingSeen,
};
use crate::{AppRoot, Msg};

/// How often a recording looks at its deadline and its writer while nothing paints.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a start waits for the frame its repaint should have produced.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a recording that ended waits for its writer. Inside the control reply's budget, so a
/// `record-ui-stop` hears about a stuck disk rather than timing out.
const FINISH_TIMEOUT: Duration = Duration::from_secs(50);
/// How long leaving rozi waits for recording files to be finished.
const EXIT_FINISH: Duration = Duration::from_secs(5);

/// What a `record-ui-start`, or the Start UI recording command, asked for.
#[derive(Debug, Default)]
pub(crate) struct StartRequest {
    pub output: Option<String>,
    pub max_fps: Option<u32>,
    pub duration_ms: Option<u64>,
    pub max_bytes: Option<u64>,
    pub force: bool,
}

pub(crate) fn is_recording(state: &State) -> bool {
    state.ui_recording.is_some()
}

/// Serve `record-ui-start`. The reply goes out once the first frame is in the file.
pub(crate) fn start_command(
    ctx: &mut Context<AppRoot>,
    request: StartRequest,
    reply: Sender<ControlResponse>,
) -> Update {
    match start(ctx, request, Some(reply.clone()), false) {
        Ok(command) => Update::with_command(command),
        Err(response) => {
            let _ = reply.send(response);
            Update::none()
        }
    }
}

/// Serve `record-ui-stop`. The reply goes out once the file is complete.
pub(crate) fn stop_command(ctx: &mut Context<AppRoot>, reply: Sender<ControlResponse>) -> Update {
    if ctx.state.ui_recording.is_none() {
        let _ = reply.send(not_recording());
        return Update::none();
    }
    end(ctx, EndReason::Stopped, vec![reply], false)
}

pub(crate) fn mark_command(ctx: &mut Context<AppRoot>, label: &str) -> ControlResponse {
    let Some(label) = crate::recording::start::mark_label(label) else {
        return ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            "a mark needs a label",
        );
    };
    let palette = palette(&ctx.state);
    let Some(recording) = ctx.state.ui_recording.as_mut() else {
        return not_recording();
    };
    let UiRecordingPhase::Running(file) = &recording.phase else {
        return ControlResponse::error_with(
            ControlErrorCode::Unavailable,
            "the UI recording is still starting",
        );
    };
    let now = elapsed_ms(file);
    // A mark belongs on the screen shown when it was made, so a frame still waiting goes first.
    let queued = match recording.pending.take() {
        Some(pending) => {
            let at = pending.painted.painted_at;
            let mark = |t: u64| {
                vec![RecordingEvent::Mark {
                    t: now.max(t),
                    label,
                }]
            };
            let queued = commit(recording, &pending, at, palette, mark, false).is_some();
            if !queued {
                recording.pending = Some(pending);
            }
            queued
        }
        None => {
            let shown = recording.last_written.map_or(0, |at| frame_time(file, at));
            file.recorder.mark(now.max(shown), label)
        }
    };
    if queued {
        ControlResponse::empty()
    } else {
        ControlResponse::error_with(
            ControlErrorCode::Unavailable,
            "the UI recording could not take the mark: it is ending, or too far behind",
        )
    }
}

/// The Start/Stop UI recording command.
pub(crate) fn toggle(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.ui_recording.is_some() {
        return end(ctx, EndReason::Stopped, Vec::new(), true);
    }
    match start(ctx, StartRequest::default(), None, true) {
        Ok(command) => Update::with_command(command),
        Err(response) => {
            notify_error(
                ctx,
                "UI recording did not start",
                response.error.unwrap_or_default(),
            );
            Update::full()
        }
    }
}

fn not_recording() -> ControlResponse {
    ControlResponse::error_with(
        ControlErrorCode::InvalidArgument,
        "this UI is not recording",
    )
}

/// Check the request, subscribe to painted frames, and wait for the first one. The caller's update
/// repaints the UI, which now shows the recording indicator, and that paint is the first frame.
fn start(
    ctx: &mut Context<AppRoot>,
    request: StartRequest,
    reply: Option<Sender<ControlResponse>>,
    from_action: bool,
) -> std::result::Result<Command, ControlResponse> {
    let invalid =
        |message: String| ControlResponse::error_with(ControlErrorCode::InvalidArgument, message);
    if let Some(recording) = &ctx.state.ui_recording {
        return Err(ControlResponse::error_with(
            ControlErrorCode::Conflict,
            match &recording.phase {
                UiRecordingPhase::Running(file) => {
                    format!("this UI is already recording to {}", file.path.display())
                }
                UiRecordingPhase::Starting { .. } => {
                    "this UI is already starting a recording".to_string()
                }
            },
        ));
    }
    if request.force && request.output.is_none() {
        return Err(invalid(
            "--force replaces the file --output names; a file the UI names is always new"
                .to_string(),
        ));
    }
    let limits = Limits::resolve(
        request.max_fps,
        request.duration_ms,
        request.max_bytes,
        &ctx.state.config.recording,
    )?;
    let output = match request.output {
        Some(output) => {
            let path = PathBuf::from(&output);
            if !path.is_absolute() {
                return Err(invalid(format!(
                    "the recording path must be absolute, not {output:?}"
                )));
            }
            Output::File(path)
        }
        None => Output::Named {
            dir: crate::recording::start::recording_dir(ctx.state.config.recording.dir.as_deref())?,
            stem: crate::recording::start::stem(
                &match &ctx.state.current().session_name {
                    Some(session) => format!("{session}-ui"),
                    None => "rozi-ui".to_string(),
                },
                chrono::Local::now(),
            ),
        },
    };
    let subscription = ctx.observe_painted_frames(ctx.link().callback(Msg::UiRecordingFrame));
    ctx.state.commands_dirty = true;
    ctx.state.next_ui_recording = ctx.state.next_ui_recording.wrapping_add(1);
    let id = ctx.state.next_ui_recording;
    ctx.state.ui_recording = Some(UiRecording {
        id,
        subscription,
        max_fps: limits.max_fps,
        duration_ms: limits.duration_ms,
        max_bytes: limits.max_bytes,
        phase: UiRecordingPhase::Starting {
            output,
            force: request.force,
            reply,
            requested: Instant::now(),
        },
        last_written: None,
        pending: None,
        armed_flush: None,
        flush_revision: 0,
        seen: UiRecordingSeen::default(),
        from_action,
    });
    Ok(poll_after(id, POLL_INTERVAL))
}

fn poll_after(id: u64, delay: Duration) -> Command {
    Command::after(delay, move |link: CommandLink<Msg>| {
        link.send(Msg::UiRecordingPoll { id });
    })
}

/// A frame the UI painted. Never asks for a repaint: that would paint another frame.
pub(crate) fn frame(ctx: &mut Context<AppRoot>, painted: PaintedFrame) -> Update {
    let Some(recording) = ctx.state.ui_recording.as_ref() else {
        return Update::none();
    };
    if matches!(recording.phase, UiRecordingPhase::Starting { .. }) {
        return begin(ctx, painted);
    }
    let pending = UiRecordingPending {
        painted,
        seen: seen(&ctx.state),
    };
    let palette = palette(&ctx.state);
    let Some(recording) = ctx.state.ui_recording.as_mut() else {
        return Update::none();
    };
    let interval = frame_interval(recording.max_fps);
    let painted_at = pending.painted.painted_at;
    let due = recording.seen != pending.seen
        || recording
            .last_written
            .is_none_or(|last| painted_at.saturating_duration_since(last) >= interval);
    recording.pending = None;
    if !due
        || commit(
            recording,
            &pending,
            painted_at,
            palette,
            |_| Vec::new(),
            false,
        )
        .is_none()
    {
        recording.pending = Some(pending);
    }
    let mut command = None;
    if recording.pending.is_some() && recording.armed_flush.is_none() {
        let wait = recording.last_written.map_or(interval, |last| {
            (last + interval).saturating_duration_since(painted_at)
        });
        command = Some(arm_flush(recording, wait));
    }
    if recorder_closed(recording) {
        return end(ctx, EndReason::Stopped, Vec::new(), true);
    }
    command.map_or_else(Update::none, Update::command_only)
}

/// Hand the writer `pending`'s frame at `at`, with the meta events for what its screen changed
/// and then the events `after` makes from the frame's time, all or nothing. Only a frame the
/// writer took moves what the recording last reported to its state, so a refused one keeps its
/// meta for the next try. Returns the frame's time in the file.
fn commit(
    recording: &mut UiRecording,
    pending: &UiRecordingPending,
    at: Instant,
    palette: TerminalColorPalette,
    after: impl FnOnce(u64) -> Vec<RecordingEvent>,
    last: bool,
) -> Option<u64> {
    let UiRecordingPhase::Running(file) = &recording.phase else {
        return None;
    };
    let t = frame_time(file, at);
    let mut events: Vec<_> = meta_changes(&recording.seen, &pending.seen)
        .into_iter()
        .map(|meta| RecordingEvent::Meta { t, meta })
        .collect();
    events.extend(after(t));
    let frame = pending.painted.frame.clone();
    if !file
        .recorder
        .push_frame_with(t, frame, palette, events, last)
    {
        return None;
    }
    recording.seen = pending.seen.clone();
    recording.last_written = Some(at);
    recording.armed_flush = None;
    Some(t)
}

/// A timer that writes the waiting frame after `wait`, replacing any armed before it.
fn arm_flush(recording: &mut UiRecording, wait: Duration) -> Command {
    recording.flush_revision = recording.flush_revision.wrapping_add(1);
    let revision = recording.flush_revision;
    recording.armed_flush = Some(revision);
    let id = recording.id;
    Command::after(wait, move |link: CommandLink<Msg>| {
        link.send(Msg::UiRecordingFlush { id, revision });
    })
}

fn recorder_closed(recording: &UiRecording) -> bool {
    matches!(&recording.phase, UiRecordingPhase::Running(file) if file.recorder.closed())
}

/// The first frame: create the file from its size and colors, and answer the start.
fn begin(ctx: &mut Context<AppRoot>, painted: PaintedFrame) -> Update {
    let palette = palette(&ctx.state);
    let now_seen = seen(&ctx.state);
    let session = ctx.state.current().session_name.clone();
    let Some(recording) = ctx.state.ui_recording.as_mut() else {
        return Update::none();
    };
    let UiRecordingPhase::Starting {
        output,
        force,
        reply,
        ..
    } = &mut recording.phase
    else {
        return Update::none();
    };
    let reply = reply.take();
    let started_at_unix_ms = crate::runtime_metrics::unix_time_millis();
    let opened = crate::pane::spans::span_frame(&painted.frame, palette, false).and_then(|first| {
        let header = crate::recording::start::header(
            RecordingTarget::Ui { session },
            &first,
            recording.max_fps,
            started_at_unix_ms,
        );
        crate::recording::start::start(output.clone(), *force, &header, recording.max_bytes)
    });
    let (path, recorder) = match opened {
        Ok(opened) => opened,
        Err(response) => {
            let from_action = recording.from_action;
            ctx.state.ui_recording = None;
            ctx.state.commands_dirty = true;
            if from_action {
                notify_error(
                    ctx,
                    "UI recording did not start",
                    response.error.clone().unwrap_or_default(),
                );
            }
            if let Some(reply) = reply {
                let _ = reply.send(response);
            }
            return Update::full();
        }
    };
    let events = initial_meta(&now_seen)
        .into_iter()
        .map(|meta| RecordingEvent::Meta { t: 0, meta })
        .collect();
    let queued = recorder.push_frame_with(0, painted.frame.clone(), palette, events, false);
    debug_assert!(queued, "an empty queue takes the first frame");
    let info = UiRecordingInfo {
        path: path.display().to_string(),
        started_at_unix_ms,
        max_fps: recording.max_fps,
        duration_ms: recording.duration_ms,
        max_bytes: recording.max_bytes,
    };
    recording.phase = UiRecordingPhase::Running(Box::new(UiRecordingFile {
        recorder,
        path,
        first_painted: painted.painted_at,
        started: Instant::now(),
        started_at_unix_ms,
    }));
    recording.last_written = Some(painted.painted_at);
    recording.seen = now_seen;
    let from_action = recording.from_action;
    if let Some(reply) = reply {
        let _ = reply.send(ControlResponse::ok(&info));
    }
    if from_action {
        let shown = crate::platform::paths::compress_home(&info.path);
        notify_path_info(ctx, "UI recording started", shown, info.path);
        return Update::full();
    }
    Update::none()
}

/// The ceiling allows the waiting frame now. It is written at the moment the ceiling allowed it,
/// which is when the terminal was showing it. A frame the writer refused earlier that changed the
/// meta keeps the moment it was painted, since that is when its events happened.
pub(crate) fn flush(ctx: &mut Context<AppRoot>, id: u64, revision: u64) -> Update {
    let palette = palette(&ctx.state);
    let Some(recording) = ctx
        .state
        .ui_recording
        .as_mut()
        .filter(|r| r.id == id && r.armed_flush == Some(revision))
    else {
        return Update::none();
    };
    recording.armed_flush = None;
    if !matches!(recording.phase, UiRecordingPhase::Running(_)) {
        return Update::none();
    }
    let Some(pending) = recording.pending.take() else {
        return Update::none();
    };
    let interval = frame_interval(recording.max_fps);
    let painted_at = pending.painted.painted_at;
    let at = match recording.last_written {
        Some(last) if recording.seen == pending.seen => (last + interval).max(painted_at),
        _ => painted_at,
    };
    if commit(recording, &pending, at, palette, |_| Vec::new(), false).is_none() {
        recording.pending = Some(pending);
        return Update::command_only(arm_flush(recording, interval));
    }
    if recorder_closed(recording) {
        return end(ctx, EndReason::Stopped, Vec::new(), true);
    }
    Update::none()
}

/// Between frames: give up on a start that never painted, and end a recording at its deadline or
/// when its writer stopped on its own.
pub(crate) fn poll(ctx: &mut Context<AppRoot>, id: u64) -> Update {
    let Some(recording) = ctx.state.ui_recording.as_ref().filter(|r| r.id == id) else {
        return Update::none();
    };
    let deadline = Duration::from_millis(recording.duration_ms);
    match &recording.phase {
        UiRecordingPhase::Starting { requested, .. } => {
            if requested.elapsed() >= FIRST_FRAME_TIMEOUT {
                return fail_start(ctx, "the UI painted no frame to record");
            }
            Update::command_only(poll_after(id, POLL_INTERVAL))
        }
        UiRecordingPhase::Running(file) => {
            if file.recorder.closed() {
                return end(ctx, EndReason::Stopped, Vec::new(), true);
            }
            let elapsed = file.started.elapsed();
            if elapsed >= deadline {
                return end(ctx, EndReason::Duration, Vec::new(), true);
            }
            Update::command_only(poll_after(id, POLL_INTERVAL.min(deadline - elapsed)))
        }
    }
}

fn fail_start(ctx: &mut Context<AppRoot>, error: &str) -> Update {
    let Some(recording) = ctx.state.ui_recording.take() else {
        return Update::none();
    };
    ctx.state.commands_dirty = true;
    if let UiRecordingPhase::Starting {
        reply: Some(reply), ..
    } = recording.phase
    {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::Unavailable,
            error,
        ));
    }
    if recording.from_action {
        notify_error(ctx, "UI recording did not start", error);
    }
    Update::full()
}

/// End the recording: write its waiting frame, then have a thread wait for the file and answer
/// `replies`. The indicator goes at once; the file is finished off the UI thread.
fn end(
    ctx: &mut Context<AppRoot>,
    reason: EndReason,
    replies: Vec<Sender<ControlResponse>>,
    notify: bool,
) -> Update {
    let palette = palette(&ctx.state);
    let Some(recording) = ctx.state.ui_recording.take() else {
        return Update::none();
    };
    ctx.state.commands_dirty = true;
    let Some(file) = close(recording, reason, palette) else {
        let response = ControlResponse::error_with(
            ControlErrorCode::Unavailable,
            "the UI recording stopped before its first frame; no file was written",
        );
        for reply in replies {
            let _ = reply.send(response.clone());
        }
        return Update::full();
    };
    let link = ctx.state.command_link.clone();
    let finish = move || {
        let stopped = finished_file(file);
        for reply in replies {
            let _ = reply.send(ControlResponse::ok(&stopped));
        }
        stopped
    };
    ctx.state
        .ui_recording_finishing
        .retain(|handle| !handle.is_finished());
    match link {
        Some(link) => {
            let spawned = std::thread::Builder::new()
                .name("rozi-record-ui-finish".to_string())
                .spawn(move || {
                    let stopped = finish();
                    link.send(Msg::UiRecordingFinished { stopped, notify });
                });
            match spawned {
                Ok(handle) => ctx.state.ui_recording_finishing.push(handle),
                Err(error) => {
                    notify_error(ctx, "UI recording failed", error.to_string());
                }
            }
            Update::full()
        }
        None => {
            let stopped = finish();
            finished(ctx, stopped, notify)
        }
    }
}

/// Write the waiting frame and end the file with `reason`. `None` for a recording that never
/// opened one, whose start is refused.
///
/// The waiting frame is the last screen, so it is written at the moment it was painted rather
/// than when the ceiling would have allowed it, which would stretch the recording past its end.
fn close(
    mut recording: UiRecording,
    reason: EndReason,
    palette: TerminalColorPalette,
) -> Option<(UiRecordingFile, u64)> {
    let last_shown = recording.pending.take().and_then(|pending| {
        let at = pending.painted.painted_at;
        commit(&mut recording, &pending, at, palette, |_| Vec::new(), true)
    });
    let UiRecording {
        subscription,
        phase,
        ..
    } = recording;
    subscription.unsubscribe();
    let file = match phase {
        UiRecordingPhase::Running(file) => *file,
        UiRecordingPhase::Starting { reply, .. } => {
            if let Some(reply) = reply {
                let _ = reply.send(ControlResponse::error_with(
                    ControlErrorCode::Unavailable,
                    "the UI recording stopped before its first frame",
                ));
            }
            return None;
        }
    };
    let elapsed = elapsed_ms(&file).max(last_shown.unwrap_or(0));
    file.recorder.finish(elapsed, reason);
    Some((file, elapsed))
}

/// Wait for a closed recording's writer, and say how the file ended.
fn finished_file((file, elapsed_ms): (UiRecordingFile, u64)) -> UiRecordingStopped {
    let path = file.path.display().to_string();
    match file.recorder.join(FINISH_TIMEOUT) {
        Some(outcome) => UiRecordingStopped {
            path,
            reason: outcome.reason,
            elapsed_ms,
            error: outcome.error,
            totals: outcome.totals,
        },
        None => UiRecordingStopped {
            path,
            reason: EndReason::WriteFailed,
            elapsed_ms,
            error: Some("the recording's writer did not finish in time".to_string()),
            totals: Default::default(),
        },
    }
}

/// A UI recording's file is complete. A recording that ended on its own, or that the palette
/// stopped, says so; a `record-ui-stop` already told its caller.
pub(crate) fn finished(
    ctx: &mut Context<AppRoot>,
    stopped: UiRecordingStopped,
    notify: bool,
) -> Update {
    if !notify {
        return Update::none();
    }
    let shown = crate::platform::paths::compress_home(&stopped.path);
    let what = match stopped.reason {
        EndReason::WriteFailed => {
            let error = stopped.error.unwrap_or_default();
            notify_error(ctx, "UI recording failed", format!("{error}\n{shown}"));
            return Update::full();
        }
        EndReason::Duration => "reached its duration",
        EndReason::MaxBytes => "reached its size limit",
        _ => "stopped",
    };
    notify_path_info(ctx, format!("UI recording {what}"), shown, stopped.path);
    Update::full()
}

/// rozi is leaving: end the recording and wait, briefly, for every file still being finished.
pub(crate) fn finish_for_exit(state: &mut State) {
    let palette = palette(state);
    if let Some(recording) = state.ui_recording.take()
        && let Some((file, _)) = close(recording, EndReason::UiExited, palette)
    {
        let _ = file.recorder.join(EXIT_FINISH);
    }
    let deadline = Instant::now() + EXIT_FINISH;
    for handle in std::mem::take(&mut state.ui_recording_finishing) {
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

fn frame_time(file: &UiRecordingFile, at: Instant) -> u64 {
    at.saturating_duration_since(file.first_painted).as_millis() as u64
}

fn elapsed_ms(file: &UiRecordingFile) -> u64 {
    file.started.elapsed().as_millis() as u64
}

/// The colors a UI capture resolves its frame with, as `capture-ui` does.
fn palette(state: &State) -> TerminalColorPalette {
    TerminalColorPalette::from_theme(&state.theme, state.theme.surface.backdrop)
}

fn seen(state: &State) -> UiRecordingSeen {
    let attachment = state.current();
    UiRecordingSeen {
        focus: state.focused_pane(),
        workspace: attachment.active_workspace,
        workspace_name: attachment
            .workspaces
            .get(attachment.active_workspace)
            .and_then(|workspace| workspace.name.clone()),
        overlay: state.modal_overlay(),
    }
}

/// What a recording notes at its start: where focus is, the workspace, and any open overlay.
fn initial_meta(now: &UiRecordingSeen) -> Vec<RecordingMeta> {
    let mut meta = vec![
        RecordingMeta::Focus { pane: now.focus },
        RecordingMeta::Workspace {
            workspace: now.workspace + 1,
            name: now.workspace_name.clone(),
        },
    ];
    if let Some(overlay) = now.overlay {
        meta.push(RecordingMeta::Overlay {
            overlay: Some(overlay.to_string()),
        });
    }
    meta
}

fn meta_changes(before: &UiRecordingSeen, now: &UiRecordingSeen) -> Vec<RecordingMeta> {
    let mut changes = Vec::new();
    if (now.workspace, &now.workspace_name) != (before.workspace, &before.workspace_name) {
        changes.push(RecordingMeta::Workspace {
            workspace: now.workspace + 1,
            name: now.workspace_name.clone(),
        });
    }
    if now.focus != before.focus {
        changes.push(RecordingMeta::Focus { pane: now.focus });
    }
    if now.overlay != before.overlay {
        changes.push(RecordingMeta::Overlay {
            overlay: now.overlay.map(str::to_string),
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::path::Path;
    use std::sync::{Arc, mpsc};

    use tui_lipan::TestBackend;

    use super::*;
    use crate::control::{ControlCommand, ControlEnvelope, ControlRequest};
    use crate::input::Action;
    use crate::recording::{RecorderOptions, RecordingEnd, Replay, ReplayStep};

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .unwrap()
            .join()
            .unwrap();
    }

    /// A client with motion off, so nothing paints unless a test makes it, whose mount has
    /// delivered the command link a finished recording reports back through.
    fn backend() -> TestBackend<AppRoot> {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().config.animations.enabled = false;
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

    fn ask(backend: &mut TestBackend<AppRoot>, command: ControlCommand) -> ControlResponse {
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane: None,
                    source_session: None,
                    extension: None,
                },
                reply,
            }))
            .unwrap();
        response
            .recv_timeout(Duration::from_secs(10))
            .expect("the UI answered")
    }

    fn start_command(output: &Path, max_fps: Option<u32>) -> ControlCommand {
        ControlCommand::RecordUiStart {
            output: Some(output.display().to_string()),
            max_fps,
            duration_ms: None,
            max_bytes: None,
            force: false,
        }
    }

    fn started(backend: &mut TestBackend<AppRoot>, output: &Path, max_fps: Option<u32>) {
        let response = ask(backend, start_command(output, max_fps));
        assert!(response.ok, "{:?}", response.error);
        let info: UiRecordingInfo = serde_json::from_value(response.data.unwrap()).unwrap();
        assert_eq!(info.path, output.display().to_string());
    }

    fn stopped(backend: &mut TestBackend<AppRoot>) -> UiRecordingStopped {
        let response = ask(backend, ControlCommand::RecordUiStop);
        assert!(response.ok, "{:?}", response.error);
        serde_json::from_value(response.data.unwrap()).unwrap()
    }

    fn running(backend: &TestBackend<AppRoot>) -> &UiRecording {
        backend.state().ui_recording.as_ref().expect("recording")
    }

    fn first_painted(backend: &TestBackend<AppRoot>) -> Instant {
        match &running(backend).phase {
            UiRecordingPhase::Running(file) => file.first_painted,
            UiRecordingPhase::Starting { .. } => panic!("still starting"),
        }
    }

    /// A frame showing `text`, painted `ms` after the recording's first.
    fn painted(backend: &TestBackend<AppRoot>, ms: u64, text: &str) -> Msg {
        let mut screen = TerminalScreen::new(4, 20, 0);
        screen.process_bytes(text.as_bytes());
        Msg::UiRecordingFrame(PaintedFrame {
            frame: Arc::new(screen.capture_frame()),
            painted_at: first_painted(backend) + Duration::from_millis(ms),
            sequence: ms,
        })
    }

    #[derive(Debug, Default)]
    struct Replayed {
        target: Option<RecordingTarget>,
        frames: Vec<(u64, String)>,
        meta: Vec<RecordingMeta>,
        marks: Vec<String>,
        end: Option<RecordingEnd>,
        /// Every frame, mark, and meta event, in file order.
        steps: Vec<String>,
    }

    fn replay(path: &Path) -> Replayed {
        let file = std::fs::File::open(path).unwrap();
        let mut replay = Replay::new(BufReader::new(file)).unwrap();
        let mut out = Replayed {
            target: Some(replay.header().target.clone()),
            ..Default::default()
        };
        while let Some(step) = replay.step().unwrap() {
            match step {
                ReplayStep::Frame { t } => {
                    let text = replay
                        .frame()
                        .unwrap()
                        .rows
                        .iter()
                        .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim_end()
                        .to_string();
                    out.steps.push(format!("frame {text}"));
                    out.frames.push((t, text));
                }
                ReplayStep::Mark { label, .. } => {
                    out.steps.push(format!("mark {label}"));
                    out.marks.push(label);
                }
                ReplayStep::Meta { meta, .. } => {
                    out.steps.push(format!("meta {meta:?}"));
                    out.meta.push(meta);
                }
                ReplayStep::End(end) => out.end = Some(end),
            }
        }
        out
    }

    fn toasts(backend: &TestBackend<AppRoot>) -> Vec<String> {
        backend
            .state()
            .replaceable_toasts
            .values()
            .map(|tracked| tracked.content().replace(['\u{0}', '\n'], " "))
            .collect()
    }

    fn wait_for_toast(backend: &mut TestBackend<AppRoot>, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            backend.pump().unwrap();
            if let Some(toast) = toasts(backend)
                .into_iter()
                .find(|toast| toast.starts_with(prefix))
            {
                return toast;
            }
            assert!(Instant::now() < deadline, "no `{prefix}` toast");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_ui_recording_writes_what_the_ui_painted_indicator_and_meta_included() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("ui.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, None);
            assert!(backend.state().ui_recording.is_some());

            backend
                .dispatch(Msg::RunAction(Action::TogglePalette))
                .unwrap();
            let mark = ask(
                &mut backend,
                ControlCommand::RecordUiMark {
                    label: " palette open ".into(),
                },
            );
            assert!(mark.ok, "{:?}", mark.error);
            let stopped = stopped(&mut backend);
            assert!(backend.state().ui_recording.is_none());
            assert_eq!(stopped.reason, EndReason::Stopped);
            assert_eq!(stopped.path, path.display().to_string());

            let replayed = replay(&path);
            assert_eq!(replayed.target, Some(RecordingTarget::Ui { session: None }));
            let (t, first) = &replayed.frames[0];
            assert_eq!(*t, 0);
            assert!(first.contains("REC"), "the indicator is recorded:\n{first}");
            assert!(replayed.frames.len() >= 2, "{:?}", replayed.frames.len());
            assert_eq!(
                replayed.meta[..2],
                [
                    RecordingMeta::Focus {
                        pane: backend.state().focused_pane()
                    },
                    RecordingMeta::Workspace {
                        workspace: 1,
                        name: None
                    },
                ]
            );
            assert!(replayed.meta.contains(&RecordingMeta::Overlay {
                overlay: Some("palette".into())
            }));
            assert_eq!(replayed.marks, ["palette open"]);
            assert_eq!(replayed.end.unwrap().reason, EndReason::Stopped);
        });
    }

    #[test]
    fn frames_faster_than_max_fps_wait_and_the_latest_is_written_when_the_ceiling_allows() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("ceiling.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, Some(10));
            let id = running(&backend).id;

            let too_soon = painted(&backend, 10, "too soon");
            assert_eq!(
                backend.update_level(too_soon).unwrap(),
                UpdateLevel::None,
                "a recorded frame never asks for another paint"
            );
            let latest = painted(&backend, 20, "latest");
            assert_eq!(backend.update_level(latest).unwrap(), UpdateLevel::None);
            let recording = running(&backend);
            let revision = recording.armed_flush.expect("a flush is armed");
            assert_eq!(recording.pending.as_ref().unwrap().painted.sequence, 20);

            assert_eq!(
                backend
                    .update_level(Msg::UiRecordingFlush { id, revision })
                    .unwrap(),
                UpdateLevel::None
            );
            let recording = running(&backend);
            assert!(recording.pending.is_none() && recording.armed_flush.is_none());
            assert_eq!(
                recording.last_written,
                Some(first_painted(&backend) + Duration::from_millis(100))
            );

            stopped(&mut backend);
            let frames = replay(&path).frames;
            assert!(
                frames.contains(&(100, "latest".to_string())),
                "the waiting frame is written when the ceiling allows: {frames:?}"
            );
            assert!(
                !frames.iter().any(|(_, text)| text == "too soon"),
                "a frame replaced while waiting is never written: {frames:?}"
            );
        });
    }

    #[test]
    fn a_frame_that_changes_the_meta_is_written_at_once() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("meta.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, Some(1));

            backend.state_mut().show_settings = true;
            let frame = painted(&backend, 10, "settings");
            backend.update_level(frame).unwrap();
            let recording = running(&backend);
            assert!(recording.pending.is_none());
            assert_eq!(
                recording.last_written,
                Some(first_painted(&backend) + Duration::from_millis(10))
            );
            assert_eq!(recording.seen.overlay, Some("settings"));

            stopped(&mut backend);
            let replayed = replay(&path);
            assert!(replayed.frames.contains(&(10, "settings".to_string())));
            assert_eq!(
                replayed.meta.last(),
                Some(&RecordingMeta::Overlay {
                    overlay: Some("settings".into())
                })
            );
        });
    }

    /// The step right after the frame showing `text`.
    fn after_frame<'a>(replayed: &'a Replayed, text: &str) -> Option<&'a str> {
        let at = replayed
            .steps
            .iter()
            .position(|step| *step == format!("frame {text}"))?;
        replayed.steps.get(at + 1).map(String::as_str)
    }

    #[test]
    fn a_mark_lands_on_the_frame_still_waiting_for_the_ceiling() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("mark.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, Some(1));
            let waiting = painted(&backend, 10, "waiting");
            backend.update_level(waiting).unwrap();
            assert!(running(&backend).pending.is_some());

            let mark = ask(
                &mut backend,
                ControlCommand::RecordUiMark {
                    label: "here".into(),
                },
            );
            assert!(mark.ok, "{:?}", mark.error);
            stopped(&mut backend);

            let replayed = replay(&path);
            assert!(replayed.frames.contains(&(10, "waiting".to_string())));
            assert_eq!(
                after_frame(&replayed, "waiting"),
                Some("mark here"),
                "{:#?}",
                replayed.steps
            );
        });
    }

    #[test]
    fn a_flush_armed_before_a_mark_wrote_its_frame_does_nothing() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("stale.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, Some(1));
            let id = running(&backend).id;
            let held = painted(&backend, 900, "held");
            backend.update_level(held).unwrap();
            let stale = running(&backend).armed_flush.expect("a flush is armed");
            let mark = ask(
                &mut backend,
                ControlCommand::RecordUiMark {
                    label: "here".into(),
                },
            );
            assert!(mark.ok, "{:?}", mark.error);
            let written = Some(first_painted(&backend) + Duration::from_millis(900));
            assert_eq!(running(&backend).last_written, written);
            let next = painted(&backend, 960, "next");
            backend.update_level(next).unwrap();

            backend
                .dispatch(Msg::UiRecordingFlush {
                    id,
                    revision: stale,
                })
                .unwrap();
            let recording = running(&backend);
            assert_eq!(
                recording.pending.as_ref().map(|p| p.painted.sequence),
                Some(960),
                "the stale timer leaves the waiting frame alone"
            );
            assert_eq!(recording.last_written, written);
            assert_ne!(recording.armed_flush, Some(stale));

            stopped(&mut backend);
            let replayed = replay(&path);
            assert!(
                replayed.frames.contains(&(960, "next".to_string())),
                "{:?}",
                replayed.frames
            );
            let end = replayed.end.unwrap().t;
            assert!(end < 1900, "nothing was stamped at the next ceiling: {end}");
        });
    }

    #[test]
    fn a_frame_the_writer_refuses_keeps_its_meta_until_it_is_written() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend();
            started(&mut backend, &dir.path().join("first.rozirec"), Some(1));
            let path = dir.path().join("stalled.rozirec");
            let palette = palette(backend.state());
            let Msg::UiRecordingFrame(filler) = painted(&backend, 0, "filler") else {
                unreachable!()
            };
            let recording = backend.state_mut().ui_recording.as_mut().unwrap();
            let UiRecordingPhase::Running(file) = &mut recording.phase else {
                panic!("still starting");
            };
            let first = crate::pane::spans::span_frame(&filler.frame, palette, false).unwrap();
            let header = crate::recording::start::header(
                RecordingTarget::Ui { session: None },
                &first,
                1,
                0,
            );
            file.recorder = crate::recording::Recorder::start_paused(RecorderOptions {
                path: path.clone(),
                overwrite: false,
                header,
                max_bytes: u64::MAX,
            })
            .unwrap();
            for t in 0..crate::recording::writer::QUEUE_FRAMES as u64 {
                let mark = vec![RecordingEvent::Mark {
                    t,
                    label: format!("filler {t}"),
                }];
                let frame = filler.frame.clone();
                assert!(
                    file.recorder
                        .push_frame_with(t, frame, palette, mark, false)
                );
            }

            backend.state_mut().show_settings = true;
            let settings = painted(&backend, 20, "settings");
            backend.update_level(settings).unwrap();
            let recording = running(&backend);
            assert_eq!(
                recording
                    .pending
                    .as_ref()
                    .map(|pending| pending.seen.overlay),
                Some(Some("settings")),
                "the refused frame waits with the state it showed"
            );
            assert_eq!(recording.seen.overlay, None, "its meta is not reported yet");

            let id = recording.id;
            let recording = backend.state_mut().ui_recording.as_mut().unwrap();
            if let UiRecordingPhase::Running(file) = &mut recording.phase {
                file.recorder.resume();
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while running(&backend).pending.is_some() {
                assert!(Instant::now() < deadline, "the frame was never written");
                std::thread::sleep(Duration::from_millis(5));
                if let Some(revision) = running(&backend).armed_flush {
                    backend
                        .dispatch(Msg::UiRecordingFlush { id, revision })
                        .unwrap();
                }
            }
            assert_eq!(running(&backend).seen.overlay, Some("settings"));
            stopped(&mut backend);

            let replayed = replay(&path);
            let overlay = format!(
                "meta {:?}",
                RecordingMeta::Overlay {
                    overlay: Some("settings".into())
                }
            );
            assert_eq!(
                after_frame(&replayed, "settings"),
                Some(overlay.as_str()),
                "{:#?}",
                replayed.steps
            );
            assert_eq!(
                replayed
                    .steps
                    .iter()
                    .filter(|step| **step == overlay)
                    .count(),
                1,
                "{:#?}",
                replayed.steps
            );
        });
    }

    #[test]
    fn stopping_writes_the_waiting_frame_when_it_was_painted() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("stop.rozirec");
            let mut backend = backend();
            started(&mut backend, &path, Some(1));
            let last = painted(&backend, 10, "last");
            backend.update_level(last).unwrap();
            assert!(running(&backend).pending.is_some());

            let stopped = stopped(&mut backend);
            assert!(stopped.elapsed_ms < 1000, "{}", stopped.elapsed_ms);
            let replayed = replay(&path);
            assert!(
                replayed.frames.contains(&(10, "last".to_string())),
                "the last frame keeps its moment, not the ceiling's: {:?}",
                replayed.frames
            );
            let end = replayed.end.unwrap();
            assert!(
                end.t < 1000,
                "the end is not pushed past the stop: {}",
                end.t
            );
            assert_eq!(end.t, stopped.elapsed_ms);
        });
    }

    #[test]
    fn requests_a_ui_recording_cannot_serve_are_refused() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend();
            for command in [
                ControlCommand::RecordUiStop,
                ControlCommand::RecordUiMark { label: "x".into() },
            ] {
                let response = ask(&mut backend, command);
                assert_eq!(response.error.as_deref(), Some("this UI is not recording"));
            }
            for (command, error) in [
                (
                    ControlCommand::RecordUiStart {
                        output: Some("relative.rozirec".into()),
                        max_fps: None,
                        duration_ms: None,
                        max_bytes: None,
                        force: false,
                    },
                    "must be absolute",
                ),
                (
                    ControlCommand::RecordUiStart {
                        output: None,
                        max_fps: None,
                        duration_ms: None,
                        max_bytes: None,
                        force: true,
                    },
                    "--force",
                ),
                (start_command(&dir.path().join("a"), Some(500)), "max fps"),
            ] {
                let response = ask(&mut backend, command);
                assert!(
                    response.error.as_deref().is_some_and(|e| e.contains(error)),
                    "{:?}",
                    response.error
                );
            }
            assert!(backend.state().ui_recording.is_none());

            let path = dir.path().join("once.rozirec");
            started(&mut backend, &path, None);
            let twice = ask(&mut backend, start_command(&dir.path().join("b"), None));
            assert_eq!(twice.code, Some(ControlErrorCode::Conflict));
            stopped(&mut backend);
            let exists = ask(&mut backend, start_command(&path, None));
            assert_eq!(exists.code, Some(ControlErrorCode::Conflict));

            assert!(
                crate::session::server::session_control_unsupported(&ControlCommand::RecordUiStop)
                    .is_some_and(|reason| reason.contains("draws nothing"))
            );
        });
    }

    #[test]
    fn the_deadline_and_leaving_rozi_each_end_a_recording() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend();
            let timed = dir.path().join("timed.rozirec");
            started(&mut backend, &timed, None);
            let id = running(&backend).id;
            // The deadline comes to the recording rather than the clock going back to it: an
            // `Instant` may not reach a day before now, as on Windows soon after boot.
            backend
                .state_mut()
                .ui_recording
                .as_mut()
                .unwrap()
                .duration_ms = 0;
            backend.dispatch(Msg::UiRecordingPoll { id }).unwrap();
            assert!(backend.state().ui_recording.is_none());
            let toast = wait_for_toast(&mut backend, "UI recording");
            assert!(
                toast.starts_with("UI recording reached its duration"),
                "{toast}"
            );
            assert_eq!(replay(&timed).end.unwrap().reason, EndReason::Duration);

            let left = dir.path().join("left.rozirec");
            started(&mut backend, &left, None);
            finish_for_exit(backend.state_mut());
            assert!(backend.state().ui_recording.is_none());
            assert_eq!(replay(&left).end.unwrap().reason, EndReason::UiExited);
        });
    }

    #[test]
    fn the_palette_command_names_the_file_and_says_where_it_went() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend();
            backend.state_mut().config.recording.dir = Some(dir.path().to_path_buf());

            backend
                .dispatch(Msg::RunAction(Action::ToggleUiRecording))
                .unwrap();
            let toast = wait_for_toast(&mut backend, "UI recording started");
            let files: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            let [name] = &files[..] else {
                panic!("expected one file, got {files:?}");
            };
            assert!(
                name.starts_with("rozi-ui-") && name.ends_with(".rozirec"),
                "{name}"
            );
            assert!(toast.ends_with(name.as_str()), "{toast}");
            let path = dir.path().join(name).display().to_string();
            assert!(
                backend
                    .state()
                    .replaceable_toasts
                    .values()
                    .any(|toast| { toast.content() == format!("UI recording started\u{0}{path}") })
            );

            backend
                .dispatch(Msg::RunAction(Action::ToggleUiRecording))
                .unwrap();
            let toast = wait_for_toast(&mut backend, "UI recording stopped");
            assert!(toast.ends_with(name.as_str()), "{toast}");
            assert!(
                backend
                    .state()
                    .replaceable_toasts
                    .values()
                    .any(|toast| { toast.content() == format!("UI recording stopped\u{0}{path}") })
            );
            assert_eq!(
                replay(&dir.path().join(name)).end.unwrap().reason,
                EndReason::Stopped
            );
        });
    }

    #[test]
    fn the_indicator_says_rec_blinks_its_dot_and_survives_a_hidden_workbar() {
        on_large_stack(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut backend = backend();
            assert_eq!(crate::view::ui_recording_label(backend.state()), None);
            started(&mut backend, &dir.path().join("chip.rozirec"), None);
            let icon = backend.state().config.recording_icon();
            assert_eq!(
                crate::view::ui_recording_label(backend.state()),
                Some(format!(" {icon} REC "))
            );

            let state = backend.state_mut();
            state.config.animations.enabled = true;
            state.config.animations.focus_chrome = true;
            state.alert_pulse_armed = true;
            state.alert_pulse_calm_phase = true;
            let blank = " ".repeat(unicode_width::UnicodeWidthStr::width(icon));
            assert_eq!(
                crate::view::ui_recording_label(backend.state()),
                Some(format!(" {blank} REC ")),
                "the dot gives way to blanks of its width; REC stays"
            );
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            state.alert_pulse_armed = false;
            state.alert_pulse_calm_phase = false;

            backend.state_mut().config.pane.show_workbar = false;
            backend.render();
            let top = backend.capture_frame().to_fixed_grid_lines().remove(0);
            assert!(top.trim_end().ends_with("REC"), "{top:?}");

            // A fullscreen pane covers the workbar, so its title carries `REC` after its badge.
            backend.state_mut().config.pane.show_workbar = true;
            let mut cover = crate::state::Pane::new(9, 100, FloatRect::default());
            cover.opening = false;
            cover.fullscreen = true;
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(cover);
            assert_eq!(
                crate::view::ui_recording_chip(backend.state()),
                Some(crate::view::UiRecordingChip::Title(9))
            );
            backend.render();
            let top = backend.capture_frame().to_fixed_grid_lines().remove(0);
            assert!(
                top.trim_end()
                    .ends_with(&format!("fullscreen · {icon} UI REC")),
                "the title carries UI REC after its badge: {top:?}"
            );

            // A recording the fullscreen pane covers keeps its own, quieter marker after `REC`.
            let mut recorded = crate::state::Pane::new(10, 100, FloatRect::default());
            recorded.opening = false;
            recorded.terminal.recording = true;
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .insert(0, recorded);
            backend.render();
            let top = backend.capture_frame().to_fixed_grid_lines().remove(0);
            assert!(
                top.trim_end()
                    .ends_with(&format!("fullscreen · {icon} UI + 1 PANE REC")),
                "one dot, then what else records: {top:?}"
            );

            // With no title either, the chip falls back to the corner.
            backend.state_mut().config.pane.show_titles = false;
            assert_eq!(
                crate::view::ui_recording_chip(backend.state()),
                Some(crate::view::UiRecordingChip::Overlay)
            );
            backend.render();
            let top = backend.capture_frame().to_fixed_grid_lines().remove(0);
            assert!(top.trim_end().ends_with("REC"), "{top:?}");
            stopped(&mut backend);
        });
    }
}
