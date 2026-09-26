//! Pane recordings on a session server: `rozi record`.
//!
//! A recording watches one shared pane's server-side screen, the canonical one `capture-pane
//! --session` reads, so it needs no UI. Like a pane wait it is looked at from the pump, never by
//! blocking it: each iteration captures the panes whose screen changed, at most `max_fps` times a
//! second, and hands the frame to the recording's writer thread. Everything slow - spans, diffs,
//! image hashing, the disk - happens there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::headless::ReplyTo;
use super::*;
use crate::control::{
    ControlCommand, ControlErrorCode, ControlResponse, RecordingInfo, RecordingListPayload,
    RecordingMarked, RecordingStopList, RecordingStopped,
};
use crate::recording::{
    EndReason, RECORDING_FORMAT, RECORDING_VERSION, Recorder, RecorderOptions, RecordingHeader,
    RecordingMeta, RecordingTarget,
};

/// Recordings one session runs at once.
pub(super) const MAX_RECORDINGS: usize = 16;
/// How long a server that is shutting down waits for each recording's file to be finished.
const SHUTDOWN_FINISH: Duration = Duration::from_secs(5);
/// Names a server tries for a recording it names itself before giving up on the directory.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// The shortest gap between two frames at `max_fps`, rounded up so the ceiling is never exceeded.
fn frame_interval(max_fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(max_fps.max(1))))
}

/// A reply held until a recording's file is complete.
pub(super) struct RecordingReply {
    client_id: ClientId,
    to: ReplyTo,
}

/// A `record-stop` held until every recording it stopped has finished its file.
pub(super) struct PendingStop {
    reply: RecordingReply,
    waiting: Vec<u64>,
    stopped: Vec<RecordingStopped>,
}

pub(super) struct ActiveRecording {
    id: u64,
    pane_id: PaneId,
    generation: u64,
    path: PathBuf,
    /// `path` fully resolved once the file exists, to recognize another spelling of it.
    resolved: PathBuf,
    started: Instant,
    started_unix_ms: u64,
    max_fps: u32,
    min_interval: Duration,
    duration_ms: u64,
    max_bytes: u64,
    recorder: Recorder,
    /// The pane's `content_generation` and palette at the last capture.
    seen: u64,
    palette: TerminalColorPalette,
    last_capture: Instant,
    meta: MetaSeen,
    /// The pane's `content_generation` and runtime `sequence` when `meta` was taken. A title
    /// arrives as output and the rest as runtime changes, so neither moving means nothing to note.
    meta_seen: (u64, u64),
    /// The foreground `record pane` holding this recording, answered when it ends.
    follower: Option<RecordingReply>,
}

impl ActiveRecording {
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn info(&self, session: &str) -> RecordingInfo {
        RecordingInfo {
            id: self.id,
            session: session.to_string(),
            pane: self.pane_id,
            path: self.path.display().to_string(),
            started_at_unix_ms: self.started_unix_ms,
            elapsed_ms: self.elapsed_ms(),
            max_fps: self.max_fps,
            duration_ms: self.duration_ms,
            max_bytes: self.max_bytes,
            follow: self.follower.is_some(),
            totals: self.recorder.totals(),
        }
    }

    /// End the recording at the current time. Its writer finishes the file; the pump notices.
    fn end(&self, reason: EndReason) {
        self.recorder.finish(self.elapsed_ms(), reason);
    }

    /// Hand the writer the pane's screen if it changed since the last capture. `due` says whether
    /// the frame-rate ceiling allows a capture now.
    ///
    /// `last` is the frame before the recording ends, which must not be refused: nothing will
    /// offer it again.
    fn capture(&mut self, pane: &mut ServerPane, now: Instant, due: bool, last: bool) {
        let palette = pane.screen().palette();
        if pane.content_generation == self.seen && palette == self.palette {
            return;
        }
        if !due {
            return;
        }
        let frame = pane.screen_without_change().capture_frame();
        // A refused frame leaves the change pending, so it is taken again at the next interval.
        self.last_capture = now;
        let t = now.duration_since(self.started).as_millis() as u64;
        let queued = if last {
            self.recorder.push_last_frame(t, frame, palette)
        } else {
            self.recorder.push_frame(t, frame, palette)
        };
        if queued {
            self.seen = pane.content_generation;
            self.palette = palette;
        }
    }

    fn due(&self, now: Instant) -> bool {
        now.duration_since(self.last_capture) >= self.min_interval
    }

    /// Write a meta event for whatever the server learned about the pane since the last look.
    fn observe(&mut self, pane: &ServerPane) {
        let seen = (pane.content_generation, pane.runtime.sequence);
        if seen == self.meta_seen {
            return;
        }
        self.meta_seen = seen;
        let now = MetaSeen::of(pane);
        let t = self.elapsed_ms();
        for meta in self.meta.changes(&now) {
            self.recorder.meta(t, meta);
        }
        self.meta = now;
    }
}

/// The semantic state of a pane a recording notes changes in.
#[derive(Clone, Debug, Default, PartialEq)]
struct MetaSeen {
    title: Option<String>,
    phase: protocol::PaneCommandPhase,
    agent: Option<(String, &'static str)>,
    status: Option<String>,
}

impl MetaSeen {
    fn of(pane: &ServerPane) -> Self {
        Self {
            title: pane.effective_title(),
            phase: pane.runtime.command_phase,
            agent: pane.runtime.detected_agent.as_ref().map(|detected| {
                (
                    detected.agent.id.clone(),
                    protocol::detected_agent_status(detected),
                )
            }),
            status: pane
                .runtime
                .status
                .as_ref()
                .map(|status| status.value.clone()),
        }
    }

    fn changes(&self, now: &Self) -> Vec<RecordingMeta> {
        let mut changes = Vec::new();
        if now.title != self.title {
            changes.push(RecordingMeta::Title {
                title: now.title.clone(),
            });
        }
        if now.phase != self.phase {
            match now.phase {
                protocol::PaneCommandPhase::Executing => {
                    changes.push(RecordingMeta::CommandStarted);
                }
                protocol::PaneCommandPhase::Completed { exit_status } => {
                    changes.push(RecordingMeta::CommandFinished {
                        status: exit_status,
                    });
                }
                _ => {}
            }
        }
        if now.agent != self.agent {
            changes.push(RecordingMeta::Agent {
                agent: now.agent.as_ref().map(|(id, _)| id.clone()),
                state: now.agent.as_ref().map(|(_, state)| (*state).to_string()),
            });
        }
        if now.status != self.status {
            changes.push(RecordingMeta::Status {
                status: now.status.clone(),
            });
        }
        changes
    }
}

/// Where a recording is written.
enum PlannedOutput {
    /// The file the request named.
    File(PathBuf),
    /// A new file the server names: `<stem>.rozirec` in `dir`, or `<stem>-2.rozirec`, … when that
    /// is taken.
    Named { dir: PathBuf, stem: String },
}

/// What a `record-start` asked for, checked.
struct RecordingPlan {
    pane_id: PaneId,
    output: PlannedOutput,
    max_fps: u32,
    duration_ms: u64,
    max_bytes: u64,
    force: bool,
}

impl SessionServer {
    /// Serve a `record-*` command. `Some` is the reply now; `None` means it is held until a
    /// recording's file is complete, and answered from [`Self::pump_recordings`].
    pub(super) fn handle_record_command(
        &mut self,
        client_id: ClientId,
        command: ControlCommand,
        to: ReplyTo,
    ) -> Option<ControlResponse> {
        let reply = RecordingReply { client_id, to };
        match command {
            ControlCommand::RecordStart {
                target,
                output,
                max_fps,
                duration_ms,
                max_bytes,
                force,
                follow,
            } => {
                let plan = match self.plan_recording(
                    target,
                    output.as_deref(),
                    max_fps,
                    duration_ms,
                    max_bytes,
                    force,
                ) {
                    Ok(plan) => plan,
                    Err(response) => return Some(response),
                };
                match self.start_recording(plan) {
                    Ok(id) if follow => {
                        if let Some(recording) = self.recordings.get_mut(&id) {
                            recording.follower = Some(reply);
                        }
                        None
                    }
                    Ok(id) => Some(ControlResponse::ok(
                        self.recordings[&id].info(&self.session_name),
                    )),
                    Err(response) => Some(response),
                }
            }
            ControlCommand::RecordStop { id, target } => {
                let ids = match self.selected_recordings(id, target) {
                    Ok(ids) => ids,
                    Err(response) => return Some(response),
                };
                for id in &ids {
                    self.recordings[id].end(EndReason::Stopped);
                }
                self.recording_stops.push(PendingStop {
                    reply,
                    waiting: ids,
                    stopped: Vec::new(),
                });
                None
            }
            ControlCommand::RecordList => Some(ControlResponse::ok(RecordingListPayload(
                self.recordings
                    .values()
                    .map(|recording| recording.info(&self.session_name))
                    .collect(),
            ))),
            ControlCommand::RecordMark { label, id, target } => {
                Some(self.mark_recordings(&label, id, target))
            }
            _ => Some(ControlResponse::error("not a record command")),
        }
    }

    fn plan_recording(
        &self,
        target: Option<PaneId>,
        output: Option<&str>,
        max_fps: Option<u32>,
        duration_ms: Option<u64>,
        max_bytes: Option<u64>,
        force: bool,
    ) -> std::result::Result<RecordingPlan, ControlResponse> {
        let invalid = |message: String| {
            ControlResponse::error_with(ControlErrorCode::InvalidArgument, message)
        };
        let path = output.map(PathBuf::from);
        if let (Some(path), Some(output)) = (&path, output)
            && !path.is_absolute()
        {
            return Err(invalid(format!(
                "the recording path must be absolute on the session's host, not {output:?}"
            )));
        }
        let defaults = &self.settings.recording;
        let max_fps = max_fps.unwrap_or(defaults.max_fps);
        if !(1..=control::MAX_RECORDING_MAX_FPS).contains(&max_fps) {
            return Err(invalid(format!(
                "max fps must be from 1 to {}",
                control::MAX_RECORDING_MAX_FPS
            )));
        }
        let duration_ms = duration_ms.unwrap_or(defaults.duration_ms);
        if !(1..=control::MAX_RECORDING_DURATION_MS).contains(&duration_ms) {
            return Err(invalid("a recording lasts from 1ms to 7 days".to_string()));
        }
        let max_bytes = max_bytes.unwrap_or(defaults.max_bytes);
        if max_bytes < crate::recording::writer::MIN_MAX_BYTES {
            return Err(invalid(format!(
                "max bytes must be at least {} KiB",
                crate::recording::writer::MIN_MAX_BYTES / 1024
            )));
        }
        if self.recordings.len() >= MAX_RECORDINGS {
            return Err(ControlResponse::error_with(
                ControlErrorCode::Conflict,
                format!("this session already runs {MAX_RECORDINGS} recordings"),
            ));
        }
        if let Some(path) = &path {
            // Compared as the file each path names, not as spelled: `--force` on another spelling
            // of a file being recorded would unlink it from under its writer.
            let shown = path.display();
            let resolved = crate::platform::persist::resolved_file_path(path)
                .map_err(|error| invalid(format!("cannot record to {shown}: {error}")))?;
            if self.recordings.values().any(|recording| {
                recording.resolved == resolved
                    || crate::platform::persist::same_file(path, &recording.path)
            }) {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::Conflict,
                    format!("{shown} is already being recorded to"),
                ));
            }
        }
        let pane_id = self.session_target_pane(target)?;
        let pane = self.panes.get(&pane_id).ok_or_else(|| {
            ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {pane_id} not found"),
            )
        })?;
        if pane.exited.is_some() {
            return Err(ControlResponse::error_with(
                ControlErrorCode::PaneNotRunning,
                format!("pane {pane_id} has exited"),
            ));
        }
        let output = match path {
            Some(path) => PlannedOutput::File(path),
            None => PlannedOutput::Named {
                dir: self.recording_dir()?,
                stem: recording_stem(&self.session_name, pane_id, chrono::Local::now()),
            },
        };
        Ok(RecordingPlan {
            pane_id,
            output,
            max_fps,
            duration_ms,
            max_bytes,
            force,
        })
    }

    fn start_recording(
        &mut self,
        plan: RecordingPlan,
    ) -> std::result::Result<u64, ControlResponse> {
        let session = self.session_name.clone();
        let pane = self.panes.get_mut(&plan.pane_id).ok_or_else(|| {
            ControlResponse::error_with(ControlErrorCode::PaneNotFound, "pane not found")
        })?;
        let frame = pane.screen_without_change().capture_frame();
        let palette = pane.screen().palette();
        let first = crate::pane::spans::span_frame(&frame, palette, false)?;
        let started_unix_ms = crate::runtime_metrics::unix_time_millis();
        let header = RecordingHeader {
            format: RECORDING_FORMAT.to_string(),
            version: RECORDING_VERSION,
            rozi: env!("CARGO_PKG_VERSION").to_string(),
            target: RecordingTarget::Pane {
                session,
                pane: plan.pane_id,
            },
            width: first.width,
            height: first.height,
            started_at_unix_ms: started_unix_ms,
            max_fps: plan.max_fps,
            keyframe_interval_ms: crate::recording::format::KEYFRAME_INTERVAL_MS,
            spans_version: crate::control::SPAN_FRAME_VERSION,
            palette: first.palette,
            compression: None,
        };
        let start = |path: &Path, overwrite: bool| {
            Recorder::start(RecorderOptions {
                path: path.to_path_buf(),
                overwrite,
                header: header.clone(),
                max_bytes: plan.max_bytes,
            })
        };
        let cannot_create = |path: &Path, error: io::Error| {
            ControlResponse::error_with(
                ControlErrorCode::RequestFailed,
                format!("cannot create {}: {error}", path.display()),
            )
        };
        let (path, recorder) = match plan.output {
            PlannedOutput::File(path) => match start(&path, plan.force) {
                Ok(recorder) => (path, recorder),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(ControlResponse::error_with(
                        ControlErrorCode::Conflict,
                        format!(
                            "{} already exists; pass --force to replace it",
                            path.display()
                        ),
                    ));
                }
                Err(error) => return Err(cannot_create(&path, error)),
            },
            PlannedOutput::Named { dir, stem } => {
                let mut started = None;
                for attempt in 1..=MAX_NAME_ATTEMPTS {
                    let path = if attempt == 1 {
                        dir.join(format!("{stem}.rozirec"))
                    } else {
                        dir.join(format!("{stem}-{attempt}.rozirec"))
                    };
                    match start(&path, false) {
                        Ok(recorder) => {
                            started = Some((path, recorder));
                            break;
                        }
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(cannot_create(&path, error)),
                    }
                }
                started.ok_or_else(|| {
                    ControlResponse::error_with(
                        ControlErrorCode::Conflict,
                        format!("no free file name for {stem}.rozirec in {}", dir.display()),
                    )
                })?
            }
        };
        let started = Instant::now();
        let queued = recorder.push_frame(0, frame, palette);
        debug_assert!(queued, "an empty queue takes the first frame");
        let meta = MetaSeen::of(pane);
        for change in MetaSeen::default().changes(&meta) {
            recorder.meta(0, change);
        }
        self.next_recording_id += 1;
        let id = self.next_recording_id;
        self.recordings.insert(
            id,
            ActiveRecording {
                id,
                pane_id: plan.pane_id,
                generation: pane.generation,
                resolved: crate::platform::persist::resolved_file_path(&path)
                    .unwrap_or_else(|_| path.clone()),
                path,
                started,
                started_unix_ms,
                max_fps: plan.max_fps,
                min_interval: frame_interval(plan.max_fps),
                duration_ms: plan.duration_ms,
                max_bytes: plan.max_bytes,
                recorder,
                seen: pane.content_generation,
                palette,
                last_capture: started,
                meta,
                meta_seen: (pane.content_generation, pane.runtime.sequence),
                follower: None,
            },
        );
        self.recording_totals.started += 1;
        self.sync_recording_flag(plan.pane_id);
        Ok(id)
    }

    /// The recording `id` names, or the only one running when it names none.
    fn recording_id(&self, id: Option<u64>) -> std::result::Result<u64, ControlResponse> {
        match id {
            Some(id) if self.recordings.contains_key(&id) => Ok(id),
            Some(id) => Err(ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                format!("no recording {id} is running"),
            )),
            None => {
                let mut ids = self.recordings.keys();
                match (ids.next(), ids.next()) {
                    (Some(&id), None) => Ok(id),
                    (None, _) => Err(ControlResponse::error_with(
                        ControlErrorCode::InvalidArgument,
                        "no recording is running",
                    )),
                    (Some(_), Some(_)) => Err(ControlResponse::error_with(
                        ControlErrorCode::TargetRequired,
                        "several recordings are running; name one with --id",
                    )),
                }
            }
        }
    }

    /// The recordings a stop or a mark names: recording `id`, every recording of pane `target`, or
    /// with neither, the only one running.
    fn selected_recordings(
        &self,
        id: Option<u64>,
        target: Option<PaneId>,
    ) -> std::result::Result<Vec<u64>, ControlResponse> {
        match (id, target) {
            (Some(_), Some(_)) => Err(ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "name a recording with --id or a pane with --target, not both",
            )),
            (None, Some(pane)) => {
                let ids: Vec<u64> = self
                    .recordings
                    .iter()
                    .filter(|(_, recording)| recording.pane_id == pane)
                    .map(|(&id, _)| id)
                    .collect();
                if ids.is_empty() {
                    Err(ControlResponse::error_with(
                        ControlErrorCode::InvalidArgument,
                        format!("pane {pane} is not being recorded"),
                    ))
                } else {
                    Ok(ids)
                }
            }
            (id, None) => self.recording_id(id).map(|id| vec![id]),
        }
    }

    /// The directory a recording the server names goes in, created private when rozi makes it.
    fn recording_dir(&self) -> std::result::Result<PathBuf, ControlResponse> {
        let configured = self.settings.recording.dir.as_deref();
        let env = crate::platform::paths::PlatformEnv::from_process();
        crate::platform::paths::recording_dir(&env, configured).map_err(|error| {
            let what = match configured {
                Some(dir) => format!("cannot use {}", dir.display()),
                None => "cannot create the recordings directory".to_string(),
            };
            ControlResponse::error_with(ControlErrorCode::RequestFailed, format!("{what}: {error}"))
        })
    }

    fn mark_recordings(
        &mut self,
        label: &str,
        id: Option<u64>,
        target: Option<PaneId>,
    ) -> ControlResponse {
        let label: String = tui_lipan::utils::sanitize_display_text(label)
            .trim()
            .chars()
            .take(control::MAX_RECORDING_MARK_CHARS)
            .collect();
        if label.is_empty() {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "a mark needs a label",
            );
        }
        // A recording that is ending takes no more marks: its writer is finishing the file.
        let candidates: Vec<u64> = match (id, target) {
            (None, None) => self
                .recordings
                .iter()
                .filter(|(_, recording)| !recording.recorder.closed())
                .map(|(&id, _)| id)
                .collect(),
            (id, target) => match self.selected_recordings(id, target) {
                Ok(ids) if id.is_some() => ids,
                Ok(ids) => ids
                    .into_iter()
                    .filter(|id| !self.recordings[id].recorder.closed())
                    .collect(),
                Err(response) => return response,
            },
        };
        if candidates.is_empty() {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "no recording is running",
            );
        }
        let (ids, refused): (Vec<u64>, Vec<u64>) = candidates.into_iter().partition(|id| {
            let recording = &self.recordings[id];
            recording
                .recorder
                .mark(recording.elapsed_ms(), label.clone())
        });
        if !refused.is_empty() && (id.is_some() || ids.is_empty()) {
            let listed = refused
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return ControlResponse::error_with(
                ControlErrorCode::Unavailable,
                format!(
                    "recording {listed} could not take the mark: it is ending, or too far behind"
                ),
            );
        }
        ControlResponse::ok(RecordingMarked { ids })
    }

    /// Capture what changed in every recorded pane, end the recordings whose pane or limits say
    /// so, and answer for the ones whose files are complete.
    pub(super) fn pump_recordings(&mut self) {
        if self.recordings.is_empty() {
            return;
        }
        let now = Instant::now();
        for recording in self.recordings.values_mut() {
            if recording.recorder.closed() {
                continue;
            }
            let Some(pane) = self
                .panes
                .get_mut(&recording.pane_id)
                .filter(|pane| pane.generation == recording.generation)
            else {
                recording.end(EndReason::PaneClosed);
                continue;
            };
            let over = now >= recording.started + Duration::from_millis(recording.duration_ms);
            // A pane's last output, and the frame at a deadline, are written whatever the ceiling.
            let exited = pane.exited;
            let last = over || exited.is_some();
            let due = last || recording.due(now);
            recording.capture(pane, now, due, last);
            recording.observe(pane);
            if let Some(status) = exited {
                recording
                    .recorder
                    .meta(recording.elapsed_ms(), RecordingMeta::Exited { status });
                recording.end(EndReason::PaneExited);
            } else if over {
                recording.end(EndReason::Duration);
            }
        }
        self.finish_recordings();
    }

    /// Answer for, and forget, every recording whose file is complete.
    fn finish_recordings(&mut self) {
        let finished: Vec<u64> = self
            .recordings
            .iter()
            .filter(|(_, recording)| recording.recorder.outcome().is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in finished {
            let Some(recording) = self.recordings.remove(&id) else {
                continue;
            };
            let outcome = recording.recorder.outcome().expect("finished");
            self.recording_totals.finished += 1;
            self.recording_totals.frames += outcome.totals.frames;
            self.recording_totals.bytes += outcome.totals.bytes;
            self.recording_totals.dropped += outcome.totals.dropped;
            let stopped = RecordingStopped {
                id,
                pane: recording.pane_id,
                path: recording.path.display().to_string(),
                reason: outcome.reason,
                elapsed_ms: recording.elapsed_ms(),
                error: outcome.error.clone(),
                totals: outcome.totals,
            };
            if let Some(reply) = recording.follower {
                self.answer_held(
                    reply.client_id,
                    &reply.to,
                    ControlResponse::ok(stopped.clone()),
                );
            }
            for stop in &mut self.recording_stops {
                if let Some(index) = stop.waiting.iter().position(|&waiting| waiting == id) {
                    stop.waiting.swap_remove(index);
                    stop.stopped.push(stopped.clone());
                }
            }
            self.sync_recording_flag(recording.pane_id);
        }
        let (done, waiting) = std::mem::take(&mut self.recording_stops)
            .into_iter()
            .partition(|stop| stop.waiting.is_empty());
        self.recording_stops = waiting;
        for PendingStop {
            reply, mut stopped, ..
        } in done
        {
            stopped.sort_by_key(|stopped| stopped.id);
            self.answer_held(
                reply.client_id,
                &reply.to,
                ControlResponse::ok(RecordingStopList { stopped }),
            );
        }
    }

    /// Whether `client_id` is waiting on a recording, so its connection must stay open.
    pub(super) fn holds_recording_reply(&self, client_id: ClientId) -> bool {
        self.recording_stops
            .iter()
            .any(|stop| stop.reply.client_id == client_id)
            || self.recordings.values().any(|recording| {
                recording
                    .follower
                    .as_ref()
                    .is_some_and(|reply| reply.client_id == client_id)
            })
    }

    /// A departed client stops the recording it followed, and waits for nothing.
    pub(super) fn release_recording_replies(&mut self, client_id: ClientId) {
        self.recording_stops
            .retain(|stop| stop.reply.client_id != client_id);
        for recording in self.recordings.values_mut() {
            if recording
                .follower
                .as_ref()
                .is_some_and(|reply| reply.client_id == client_id)
            {
                recording.follower = None;
                recording.end(EndReason::Stopped);
            }
        }
    }

    /// End every recording and wait, briefly, for their files: the server is going away.
    pub(super) fn finish_recordings_for_shutdown(&mut self, reason: EndReason) {
        for recording in std::mem::take(&mut self.recordings).into_values() {
            recording.end(reason);
            let _ = recording.recorder.join(SHUTDOWN_FINISH);
        }
    }

    /// Tell every client whether `pane_id` is being recorded, when that changed.
    fn sync_recording_flag(&mut self, pane_id: PaneId) {
        let recording = self
            .recordings
            .values()
            .any(|recording| recording.pane_id == pane_id);
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if pane.runtime.recording == recording {
            return;
        }
        pane.runtime.recording = recording;
        pane.runtime.sequence = pane.runtime.sequence.wrapping_add(1);
        let (generation, state) = (pane.generation, pane.runtime.clone());
        let message = ServerMessage::PaneRuntimeChanged {
            pane_id,
            local: false,
            generation,
            agent_refs: self.agent_references(None, pane_id),
            state,
        };
        self.broadcast_outbound(&ServerOutbound::control(message));
    }

    pub(super) fn recording_metrics(&self) -> crate::runtime_metrics::RecordingMetrics {
        let mut metrics = self.recording_totals;
        metrics.active = self.recordings.len() as u64;
        for recording in self.recordings.values() {
            let totals = recording.recorder.totals();
            metrics.frames += totals.frames;
            metrics.bytes += totals.bytes;
            metrics.dropped += totals.dropped;
        }
        metrics
    }
}

pub(super) type Recordings = BTreeMap<u64, ActiveRecording>;

/// `<session>-pane-<id>-<stamp>`, stamped in the server's local time to the second.
fn recording_stem(session: &str, pane: PaneId, now: chrono::DateTime<chrono::Local>) -> String {
    format!("{session}-pane-{pane}-{}", now.format("%Y%m%d-%H%M%S"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::control::ControlRequest;
    use crate::recording::{RecordingEnd, Replay, ReplayStep};
    use crate::session::server::tests::{add_client, decode_outbox_controls, test_pane};
    use std::path::Path;

    fn server() -> SessionServer {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(3, test_pane(1));
        server
    }

    fn start(output: &Path, max_fps: Option<u32>, follow: bool) -> ControlCommand {
        ControlCommand::RecordStart {
            target: Some(3),
            output: Some(output.display().to_string()),
            max_fps,
            duration_ms: None,
            max_bytes: None,
            force: false,
            follow,
        }
    }

    fn ask(
        server: &mut SessionServer,
        client: ClientId,
        command: ControlCommand,
    ) -> Vec<ControlResponse> {
        server
            .handle_session_control(
                client,
                "dev".to_string(),
                PROTOCOL_VERSION,
                PROTOCOL_VERSION,
                None,
                ControlRequest {
                    command,
                    source_pane: None,
                    source_session: None,
                    extension: None,
                },
            )
            .into_iter()
            .filter_map(|(_, message)| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response),
                _ => None,
            })
            .collect()
    }

    fn outbox(server: &SessionServer, client: ClientId) -> Vec<ServerMessage> {
        let conn = server
            .clients
            .iter()
            .find(|conn| conn.id == client)
            .unwrap();
        decode_outbox_controls(conn)
    }

    fn answered(server: &SessionServer, client: ClientId) -> Vec<ControlResponse> {
        outbox(server, client)
            .into_iter()
            .filter_map(|message| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response),
                _ => None,
            })
            .collect()
    }

    /// The recordings a `record-stop` answer says it stopped.
    fn stopped_by(response: ControlResponse) -> Vec<RecordingStopped> {
        assert!(response.ok, "{:?}", response.error);
        serde_json::from_value::<RecordingStopList>(response.data.unwrap())
            .unwrap()
            .stopped
    }

    fn print(server: &mut SessionServer, bytes: &[u8]) {
        server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_mut()
            .process_bytes(bytes);
    }

    /// Pump until every recording has finished and been answered for.
    fn pump_until_finished(server: &mut SessionServer) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server.recordings.is_empty() {
            assert!(Instant::now() < deadline, "a recording never finished");
            server.pump_recordings();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn replay(path: &Path) -> (Vec<String>, Vec<RecordingMeta>, Vec<String>, RecordingEnd) {
        let bytes = std::fs::read(path).unwrap();
        let mut replay = Replay::new(std::io::BufReader::new(bytes.as_slice())).unwrap();
        let (mut frames, mut meta, mut marks, mut end) = (Vec::new(), Vec::new(), Vec::new(), None);
        while let Some(step) = replay.step().unwrap() {
            match step {
                ReplayStep::Frame { .. } => {
                    let frame = replay.frame().unwrap();
                    frames.push(
                        frame
                            .rows
                            .iter()
                            .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .trim_end()
                            .to_string(),
                    );
                }
                ReplayStep::Meta { meta: m, .. } => meta.push(m),
                ReplayStep::Mark { label, .. } => marks.push(label),
                ReplayStep::End(e) => end = Some(e),
            }
        }
        (frames, meta, marks, end.expect("an end event"))
    }

    fn runtime_flags(server: &SessionServer, client: ClientId) -> Vec<bool> {
        outbox(server, client)
            .into_iter()
            .filter_map(|message| match message {
                ServerMessage::PaneRuntimeChanged {
                    pane_id: 3, state, ..
                } => Some(state.recording),
                _ => None,
            })
            .collect()
    }

    fn attached(server: &mut SessionServer) -> ClientId {
        let (client, stream) = add_client(server);
        std::mem::forget(stream);
        server
            .clients
            .iter_mut()
            .find(|conn| conn.id == client)
            .unwrap()
            .attached = true;
        client
    }

    #[test]
    fn a_recording_writes_each_change_and_answers_stop_once_its_file_is_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pane.rozirec");
        let mut server = server();
        let watcher = attached(&mut server);
        let (client, _stream) = add_client(&mut server);
        print(&mut server, b"$ ");

        let started = ask(&mut server, client, start(&path, Some(120), false));
        assert!(started[0].ok, "{:?}", started[0].error);
        let info: RecordingInfo = serde_json::from_value(started[0].data.clone().unwrap()).unwrap();
        assert_eq!((info.id, info.pane, info.max_fps), (1, 3, 120));
        assert!(server.panes[&3].runtime.recording);
        assert_eq!(
            runtime_flags(&server, watcher),
            [true],
            "every client sees it start"
        );

        // Nothing changes: nothing is captured, however often the pump looks.
        for _ in 0..5 {
            server.pump_recordings();
        }
        std::thread::sleep(Duration::from_millis(10));
        print(&mut server, b"make\r\n");
        server.pump_recordings();
        std::thread::sleep(Duration::from_millis(10));
        print(&mut server, b"done");
        server.pump_recordings();

        let listed = ask(&mut server, client, ControlCommand::RecordList);
        let list: RecordingListPayload =
            serde_json::from_value(listed[0].data.clone().unwrap()).unwrap();
        assert_eq!(list.0.len(), 1);
        let (marker, _marker_stream) = add_client(&mut server);
        assert!(
            ask(
                &mut server,
                marker,
                ControlCommand::RecordMark {
                    label: "built".into(),
                    id: None,
                    target: None,
                }
            )[0]
            .ok
        );

        let (stopper, _stop_stream) = add_client(&mut server);
        assert!(
            ask(
                &mut server,
                stopper,
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                }
            )
            .is_empty(),
            "held"
        );
        assert!(server.holds_recording_reply(stopper));
        pump_until_finished(&mut server);

        let [stopped] = stopped_by(answered(&server, stopper).remove(0))
            .try_into()
            .unwrap();
        assert_eq!(stopped.reason, EndReason::Stopped);
        assert_eq!(stopped.totals.frames, 3);
        assert_eq!(stopped.totals.marks, 1);
        assert!(!server.panes[&3].runtime.recording);
        assert_eq!(runtime_flags(&server, watcher), [true, false]);

        let (frames, _, marks, end) = replay(&path);
        assert_eq!(frames, ["$", "$ make", "$ make\ndone"]);
        assert_eq!(marks, ["built"]);
        assert_eq!(end.reason, EndReason::Stopped);
        assert_eq!(server.recording_metrics().finished, 1);
    }

    fn unnamed(target: PaneId) -> ControlCommand {
        start_with(target, None, None)
    }

    fn start_with(target: PaneId, output: Option<&Path>, max_fps: Option<u32>) -> ControlCommand {
        ControlCommand::RecordStart {
            target: Some(target),
            output: output.map(|path| path.display().to_string()),
            max_fps,
            duration_ms: None,
            max_bytes: None,
            force: false,
            follow: false,
        }
    }

    fn started_info(response: &ControlResponse) -> RecordingInfo {
        assert!(response.ok, "{:?}", response.error);
        serde_json::from_value(response.data.clone().unwrap()).unwrap()
    }

    #[test]
    fn a_recording_without_a_path_is_named_by_the_server_in_its_configured_directory() {
        let dir = tempfile::tempdir().unwrap();
        let configured = dir.path().join("made-by-rozi");
        let mut server = server();
        server.settings.recording = crate::config::RecordingConfig {
            dir: Some(configured.clone()),
            max_fps: 7,
            duration_ms: 90_000,
            max_bytes: 2 * 1024 * 1024,
        };
        let (client, _stream) = add_client(&mut server);

        let first = started_info(&ask(&mut server, client, unnamed(3))[0]);
        let second = started_info(&ask(&mut server, client, unnamed(3))[0]);
        assert_eq!(
            (first.max_fps, first.duration_ms, first.max_bytes),
            (7, 90_000, 2 * 1024 * 1024),
            "the configured defaults fill what the request leaves out"
        );
        let (first, second) = (PathBuf::from(first.path), PathBuf::from(second.path));
        assert_eq!(first.parent(), Some(configured.as_path()));
        let stem = first.file_stem().unwrap().to_str().unwrap().to_string();
        assert!(stem.starts_with("dev-pane-3-"), "{stem}");
        let second_stem = second.file_stem().unwrap().to_str().unwrap();
        assert!(
            second_stem == format!("{stem}-2") || !second_stem.starts_with(&stem),
            "a second recording in the same second takes the next free name, not {second_stem}"
        );
        assert!(second.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "a recording is private");
        }

        let overridden = ask(&mut server, client, start_with(3, None, Some(30)));
        assert_eq!(started_info(&overridden[0]).max_fps, 30);
    }

    #[test]
    fn a_recording_without_a_path_or_a_directory_goes_in_the_state_directory() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);

        let path = PathBuf::from(started_info(&ask(&mut server, client, unnamed(3))[0]).path);
        let env = crate::platform::paths::PlatformEnv::from_process();
        let expected = crate::platform::paths::recording_dir(&env, None).unwrap();
        assert_eq!(path.parent(), Some(expected.as_path()));
        assert!(path.exists());
    }

    #[test]
    fn a_pane_target_stops_and_marks_every_recording_of_that_pane() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = server();
        server.panes.insert(4, test_pane(2));
        let (client, _stream) = add_client(&mut server);
        for (pane, name) in [(3, "a"), (3, "b"), (4, "other")] {
            let command = start_with(pane, Some(&dir.path().join(name)), None);
            started_info(&ask(&mut server, client, command)[0]);
        }

        let both = ask(
            &mut server,
            client,
            ControlCommand::RecordStop {
                id: Some(1),
                target: Some(3),
            },
        );
        assert_eq!(both[0].code, Some(ControlErrorCode::InvalidArgument));
        let idle = ask(
            &mut server,
            client,
            ControlCommand::RecordMark {
                label: "x".into(),
                id: None,
                target: Some(9),
            },
        );
        assert!(
            idle[0].error.as_ref().unwrap().contains("pane 9"),
            "{:?}",
            idle[0].error
        );

        let marked = ask(
            &mut server,
            client,
            ControlCommand::RecordMark {
                label: "here".into(),
                id: None,
                target: Some(3),
            },
        );
        assert!(marked[0].ok, "{:?}", marked[0].error);
        let (stopper, _stop_stream) = add_client(&mut server);
        assert!(
            ask(
                &mut server,
                stopper,
                ControlCommand::RecordStop {
                    id: None,
                    target: Some(3),
                },
            )
            .is_empty()
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while server.recordings.len() > 1 {
            assert!(
                Instant::now() < deadline,
                "the pane's recordings never finished"
            );
            server.pump_recordings();
            std::thread::sleep(Duration::from_millis(2));
        }
        let stopped = stopped_by(answered(&server, stopper).remove(0));
        assert_eq!(
            stopped
                .iter()
                .map(|one| (one.id, one.totals.marks))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 1)],
            "one answer lists both, in order, each carrying the mark"
        );
        assert_eq!(server.recordings.keys().copied().collect::<Vec<_>>(), [3]);
        assert!(!server.panes[&3].runtime.recording);
        assert!(server.panes[&4].runtime.recording);
        for name in ["a", "b"] {
            assert_eq!(replay(&dir.path().join(name)).2, ["here"]);
        }
    }

    fn tunnel(
        server: &mut SessionServer,
        client: ClientId,
        request_id: u64,
        command: ControlCommand,
    ) {
        tunnel_request(
            server,
            client,
            request_id,
            ControlRequest {
                command,
                source_pane: None,
                source_session: None,
                extension: None,
            },
        );
    }

    fn tunnel_request(
        server: &mut SessionServer,
        client: ClientId,
        request_id: u64,
        request: ControlRequest,
    ) {
        server.process_client_frame(
            client,
            protocol::Frame::Control(ClientMessage::AttachedControl {
                request_id,
                request,
            }),
        );
    }

    fn tunnelled(server: &SessionServer, client: ClientId) -> Vec<(u64, ControlResponse)> {
        outbox(server, client)
            .into_iter()
            .filter_map(|message| match message {
                ServerMessage::AttachedControlResult {
                    request_id,
                    response,
                } => Some((request_id, response)),
                _ => None,
            })
            .collect()
    }

    fn closing(server: &SessionServer, client: ClientId) -> bool {
        server
            .clients
            .iter()
            .find(|conn| conn.id == client)
            .is_some_and(|conn| conn.close_after_flush)
    }

    #[test]
    fn an_attached_client_records_over_its_open_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pane.rozirec");
        let mut server = server();
        let client = attached(&mut server);
        print(&mut server, b"$ ");

        tunnel(&mut server, client, 41, start(&path, None, false));
        tunnel(&mut server, client, 42, ControlCommand::RecordList);
        tunnel(
            &mut server,
            client,
            43,
            ControlCommand::RecordMark {
                label: "here".into(),
                id: None,
                target: None,
            },
        );
        let answers = tunnelled(&server, client);
        assert_eq!(
            answers.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [41, 42, 43],
            "each answer carries the id it was asked with"
        );
        assert!(
            answers.iter().all(|(_, response)| response.ok),
            "{answers:?}"
        );
        assert!(server.panes[&3].runtime.recording);

        tunnel(
            &mut server,
            client,
            44,
            ControlCommand::RecordStop {
                id: None,
                target: None,
            },
        );
        assert_eq!(
            tunnelled(&server, client).len(),
            3,
            "stop is held until the file is done"
        );
        pump_until_finished(&mut server);
        let (id, stopped) = tunnelled(&server, client).pop().unwrap();
        assert_eq!(id, 44);
        let [stopped] = stopped_by(stopped).try_into().unwrap();
        assert_eq!(
            (stopped.reason, stopped.totals.marks),
            (EndReason::Stopped, 1)
        );
        assert!(
            answered(&server, client).is_empty(),
            "nothing is answered as a headless reply"
        );
        assert!(
            !closing(&server, client),
            "the attachment stays open throughout"
        );
        assert!(server.client_attached(client));
    }

    #[test]
    fn an_attached_client_is_refused_everything_off_the_recording_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pane.rozirec");
        let mut server = server();
        let client = attached(&mut server);
        let refusal = |server: &SessionServer, id| {
            tunnelled(server, client)
                .into_iter()
                .find(|(request_id, _)| *request_id == id)
                .map(|(_, response)| (response.ok, response.code))
                .unwrap()
        };

        tunnel(&mut server, client, 1, ControlCommand::ListPanes);
        assert_eq!(
            refusal(&server, 1),
            (false, Some(ControlErrorCode::Unsupported))
        );
        tunnel(&mut server, client, 2, start(&path, None, true));
        assert_eq!(
            refusal(&server, 2),
            (false, Some(ControlErrorCode::InvalidArgument))
        );
        tunnel_request(
            &mut server,
            client,
            3,
            ControlRequest {
                command: start(&path, None, false),
                source_pane: None,
                source_session: None,
                extension: Some(crate::config::ExtensionProvenance {
                    id: "ext".into(),
                    generation: "g".into(),
                }),
            },
        );
        assert!(!refusal(&server, 3).0);
        assert!(server.recordings.is_empty(), "nothing was started");
        assert!(!closing(&server, client));

        server
            .clients
            .iter_mut()
            .find(|conn| conn.id == client)
            .unwrap()
            .read_only = true;
        tunnel(&mut server, client, 4, start(&path, None, false));
        assert_eq!(
            refusal(&server, 4),
            (false, Some(ControlErrorCode::ReadOnly))
        );
        tunnel(
            &mut server,
            client,
            5,
            ControlCommand::RecordStop {
                id: None,
                target: None,
            },
        );
        assert_eq!(
            refusal(&server, 5),
            (false, Some(ControlErrorCode::ReadOnly))
        );
        tunnel(&mut server, client, 6, ControlCommand::RecordList);
        assert_eq!(refusal(&server, 6), (true, None), "a viewer may look");

        let (stranger, _stream) = add_client(&mut server);
        tunnel(&mut server, stranger, 7, ControlCommand::RecordList);
        assert!(tunnelled(&server, stranger).is_empty());
        assert!(outbox(&server, stranger).iter().any(|message| matches!(
            message,
            ServerMessage::Error { code, .. } if code == "attach-required"
        )));
        assert!(closing(&server, stranger), "a connection must attach first");
    }

    #[test]
    fn changes_faster_than_max_fps_coalesce_into_the_latest_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pane.rozirec");
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&path, Some(5), false))[0].ok);
        // A burst inside one 200ms interval: none of it is captured yet.
        for n in 0..10 {
            print(&mut server, format!("\r{n}").as_bytes());
            server.pump_recordings();
        }
        std::thread::sleep(Duration::from_millis(210));
        server.pump_recordings();
        let (stopper, _s) = add_client(&mut server);
        ask(
            &mut server,
            stopper,
            ControlCommand::RecordStop {
                id: None,
                target: None,
            },
        );
        pump_until_finished(&mut server);

        let (frames, ..) = replay(&path);
        assert_eq!(
            frames,
            ["", "9"],
            "the start, then only the newest state of the burst"
        );
    }

    /// Start recording pane 3 to `path` with its writer stalled and its queue full of frames that
    /// each precede a mark, which none of the newer frames may displace.
    fn stalled_and_full(server: &mut SessionServer, path: &Path, duration_ms: Option<u64>) {
        let (client, _stream) = add_client(server);
        let mut command = start(path, None, false);
        if let ControlCommand::RecordStart {
            duration_ms: duration,
            ..
        } = &mut command
        {
            *duration = duration_ms;
        }
        assert!(ask(server, client, command)[0].ok);
        // Swap in a stalled writer on the same file, with the header the server wrote.
        let header: crate::recording::RecordingHeader = serde_json::from_str(
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let paused = Recorder::start_paused(RecorderOptions {
            path: path.to_path_buf(),
            overwrite: true,
            header,
            max_bytes: u64::MAX,
        })
        .unwrap();
        let recording = server.recordings.get_mut(&1).unwrap();
        let _ = std::mem::replace(&mut recording.recorder, paused).join(Duration::ZERO);
        for n in 0..crate::recording::writer::QUEUE_FRAMES as u64 {
            let pane = server.panes.get_mut(&3).unwrap();
            pane.screen_mut()
                .process_bytes(format!("\x1b[1;1Hframe {n}").as_bytes());
            let recording = server.recordings.get_mut(&1).unwrap();
            let screen = pane.screen_without_change();
            assert!(
                recording
                    .recorder
                    .push_frame(n, screen.capture_frame(), screen.palette())
            );
            assert!(recording.recorder.mark(n, format!("after {n}")));
        }
    }

    fn resume_and_finish(server: &mut SessionServer) {
        server.recordings.get_mut(&1).unwrap().recorder.resume();
        pump_until_finished(server);
    }

    #[test]
    fn a_pane_exiting_behind_a_stalled_writer_still_ends_on_its_last_screen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stalled-exit.rozirec");
        let mut server = server();
        stalled_and_full(&mut server, &path, None);
        print(&mut server, b"\x1b[2;1Hlast words");
        server.panes.get_mut(&3).unwrap().exited = Some(1);
        server.pump_recordings();
        resume_and_finish(&mut server);

        let (frames, meta, _, end) = replay(&path);
        assert_eq!(end.reason, EndReason::PaneExited);
        assert!(frames.last().unwrap().contains("last words"), "{frames:?}");
        assert!(meta.contains(&RecordingMeta::Exited { status: 1 }));
    }

    #[test]
    fn a_deadline_behind_a_stalled_writer_still_ends_on_the_last_screen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stalled-deadline.rozirec");
        let mut server = server();
        stalled_and_full(&mut server, &path, Some(50));
        print(&mut server, b"\x1b[2;1Hat the deadline");
        std::thread::sleep(Duration::from_millis(60));
        server.pump_recordings();
        resume_and_finish(&mut server);

        let (frames, _, _, end) = replay(&path);
        assert_eq!(end.reason, EndReason::Duration);
        assert!(
            frames.last().unwrap().contains("at the deadline"),
            "{frames:?}"
        );
    }

    #[test]
    fn a_pane_exiting_ends_its_recording_on_its_last_screen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pane.rozirec");
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(
            ask(&mut server, client, start(&path, Some(1), true)).is_empty(),
            "follow holds"
        );
        print(&mut server, b"bye");
        server.panes.get_mut(&3).unwrap().exited = Some(2);
        pump_until_finished(&mut server);

        let stopped: RecordingStopped =
            serde_json::from_value(answered(&server, client)[0].data.clone().unwrap()).unwrap();
        assert_eq!(stopped.reason, EndReason::PaneExited);
        let (frames, meta, _, end) = replay(&path);
        assert_eq!(
            frames.last().unwrap(),
            "bye",
            "the last output is written despite max_fps"
        );
        assert!(meta.contains(&RecordingMeta::Exited { status: 2 }));
        assert_eq!(end.reason, EndReason::PaneExited);
    }

    #[test]
    fn a_closed_pane_a_deadline_and_a_departed_follower_each_end_a_recording() {
        let dir = tempfile::tempdir().unwrap();

        let closed = dir.path().join("closed.rozirec");
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&closed, None, false))[0].ok);
        server.panes.remove(&3);
        pump_until_finished(&mut server);
        assert_eq!(replay(&closed).3.reason, EndReason::PaneClosed);

        let timed = dir.path().join("timed.rozirec");
        let mut server = server_with_pane();
        let (client, _stream) = add_client(&mut server);
        let mut command = start(&timed, None, false);
        if let ControlCommand::RecordStart { duration_ms, .. } = &mut command {
            *duration_ms = Some(20);
        }
        assert!(ask(&mut server, client, command)[0].ok);
        std::thread::sleep(Duration::from_millis(30));
        pump_until_finished(&mut server);
        assert_eq!(replay(&timed).3.reason, EndReason::Duration);

        let followed = dir.path().join("followed.rozirec");
        let mut server = server_with_pane();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&followed, None, true)).is_empty());
        server.remove_client(client);
        pump_until_finished(&mut server);
        assert_eq!(replay(&followed).3.reason, EndReason::Stopped);

        let shutdown = dir.path().join("shutdown.rozirec");
        let mut server = server_with_pane();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&shutdown, None, false))[0].ok);
        server.finish_recordings_for_shutdown(EndReason::ServerShutdown);
        assert_eq!(replay(&shutdown).3.reason, EndReason::ServerShutdown);
    }

    #[test]
    fn a_title_and_a_status_the_server_learns_become_meta_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.rozirec");
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&path, None, false))[0].ok);
        print(&mut server, b"\x1b]2;building\x07");
        server.pump_recordings();
        server
            .apply_pane_status(None, 3, 1, Some("working".into()), None)
            .unwrap();
        server.pump_recordings();
        let (stopper, _s) = add_client(&mut server);
        ask(
            &mut server,
            stopper,
            ControlCommand::RecordStop {
                id: None,
                target: None,
            },
        );
        pump_until_finished(&mut server);

        let (_, meta, ..) = replay(&path);
        assert!(
            meta.contains(&RecordingMeta::Title {
                title: Some("building".into())
            }),
            "{meta:?}"
        );
        assert!(
            meta.contains(&RecordingMeta::Status {
                status: Some("working".into())
            }),
            "{meta:?}"
        );
    }

    #[test]
    fn a_mark_sent_while_a_recording_ends_is_refused_rather_than_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ending.rozirec");
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&path, None, false))[0].ok);
        let (stopper, _s) = add_client(&mut server);
        assert!(
            ask(
                &mut server,
                stopper,
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                }
            )
            .is_empty()
        );
        // The recording is still listed while its writer finishes the file.
        assert_eq!(server.recordings.len(), 1);

        let named = ask(
            &mut server,
            client,
            ControlCommand::RecordMark {
                label: "late".into(),
                id: Some(1),
                target: None,
            },
        );
        assert!(!named[0].ok);
        assert_eq!(named[0].code, Some(ControlErrorCode::Unavailable));
        let any = ask(
            &mut server,
            client,
            ControlCommand::RecordMark {
                label: "late".into(),
                id: None,
                target: None,
            },
        );
        assert!(!any[0].ok, "{any:?}");

        pump_until_finished(&mut server);
        assert!(replay(&path).2.is_empty(), "no mark was written");
    }

    #[test]
    fn another_spelling_of_a_file_being_recorded_is_refused_even_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rozirec");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("link")).unwrap();
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, start(&path, None, false))[0].ok);

        for alias in [
            dir.path().join("sub/../a.rozirec"),
            dir.path().join("link/a.rozirec"),
            dir.path().join("./a.rozirec"),
        ] {
            let mut forced = start(&alias, None, false);
            if let ControlCommand::RecordStart { force, .. } = &mut forced {
                *force = true;
            }
            let refused = ask(&mut server, client, forced);
            assert_eq!(
                refused[0].code,
                Some(ControlErrorCode::Conflict),
                "{alias:?}"
            );
        }
        assert!(path.is_file(), "the recording's file is still there");
        assert_eq!(server.recordings.len(), 1);

        let missing = ask(
            &mut server,
            client,
            start(&dir.path().join("missing/b.rozirec"), None, false),
        );
        assert_eq!(missing[0].code, Some(ControlErrorCode::InvalidArgument));
        server.finish_recordings_for_shutdown(EndReason::ServerShutdown);
    }

    #[test]
    fn max_fps_is_a_ceiling_the_interval_never_undercuts() {
        for fps in 1..=control::MAX_RECORDING_MAX_FPS {
            assert!(frame_interval(fps) * fps >= Duration::from_secs(1), "{fps}");
        }
        assert_eq!(frame_interval(30), Duration::from_nanos(33_333_334));
    }

    fn server_with_pane() -> SessionServer {
        server()
    }

    #[test]
    fn a_bad_request_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.rozirec");
        std::fs::write(&existing, b"keep").unwrap();
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        let refused = |server: &mut SessionServer, command| ask(server, client, command)[0].code;

        assert_eq!(
            refused(
                &mut server,
                start(Path::new("relative.rozirec"), None, false)
            ),
            Some(ControlErrorCode::InvalidArgument)
        );
        assert_eq!(
            refused(&mut server, start(&existing, None, false)),
            Some(ControlErrorCode::Conflict)
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep");
        assert_eq!(
            refused(&mut server, start(&dir.path().join("a"), Some(0), false)),
            Some(ControlErrorCode::InvalidArgument)
        );
        assert_eq!(
            refused(
                &mut server,
                ControlCommand::RecordStop {
                    id: None,
                    target: None,
                }
            ),
            Some(ControlErrorCode::InvalidArgument)
        );
        assert_eq!(
            refused(
                &mut server,
                ControlCommand::RecordMark {
                    label: "x".into(),
                    id: None,
                    target: None,
                }
            ),
            Some(ControlErrorCode::InvalidArgument)
        );
        server.panes.get_mut(&3).unwrap().exited = Some(0);
        assert_eq!(
            refused(&mut server, start(&dir.path().join("b"), None, false)),
            Some(ControlErrorCode::PaneNotRunning)
        );
        assert!(server.recordings.is_empty());

        // `--force` replaces the file.
        server.panes.get_mut(&3).unwrap().exited = None;
        let mut forced = start(&existing, None, false);
        if let ControlCommand::RecordStart { force, .. } = &mut forced {
            *force = true;
        }
        assert!(ask(&mut server, client, forced)[0].ok);
        // The same file cannot be written by two recordings at once.
        let mut again = start(&existing, None, false);
        if let ControlCommand::RecordStart { force, .. } = &mut again {
            *force = true;
        }
        assert_eq!(
            refused(&mut server, again),
            Some(ControlErrorCode::Conflict)
        );
        server.finish_recordings_for_shutdown(EndReason::ServerShutdown);
    }
}
