//! Pane recordings on a session server: `rozi record`.
//!
//! A recording watches one shared pane's server-side screen, the canonical one `capture-pane
//! --session` reads, so it needs no UI. Like a pane wait it is looked at from the pump, never by
//! blocking it: each iteration captures the panes whose screen changed, at most `max_fps` times a
//! second, and hands the frame to the recording's writer thread. Everything slow - spans, diffs,
//! image hashing, the disk - happens there.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::headless::session_control_reply;
use super::*;
use crate::control::{
    ControlCommand, ControlErrorCode, ControlResponse, RecordingInfo, RecordingListPayload,
    RecordingMarked, RecordingStopped,
};
use crate::recording::{
    EndReason, RECORDING_FORMAT, RECORDING_VERSION, Recorder, RecorderOptions, RecordingHeader,
    RecordingMeta, RecordingTarget,
};

/// Recordings one session runs at once.
pub(super) const MAX_RECORDINGS: usize = 16;
/// How long a server that is shutting down waits for each recording's file to be finished.
const SHUTDOWN_FINISH: Duration = Duration::from_secs(5);

/// The shortest gap between two frames at `max_fps`, rounded up so the ceiling is never exceeded.
fn frame_interval(max_fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(max_fps.max(1))))
}

/// A reply held until a recording's file is complete.
pub(super) struct RecordingReply {
    client_id: ClientId,
    capabilities: protocol::Capabilities,
    effective_protocol: u32,
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
    /// `record stop` callers waiting for the file to be complete.
    stoppers: Vec<RecordingReply>,
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
    fn capture(&mut self, pane: &mut ServerPane, now: Instant, due: bool) {
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
        if self.recorder.push_frame(
            now.duration_since(self.started).as_millis() as u64,
            frame,
            palette,
        ) {
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

/// What a `record-start` asked for, checked.
struct RecordingPlan {
    pane_id: PaneId,
    path: PathBuf,
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
        capabilities: protocol::Capabilities,
        effective_protocol: u32,
    ) -> Option<ControlResponse> {
        let reply = RecordingReply {
            client_id,
            capabilities,
            effective_protocol,
        };
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
                    &output,
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
            ControlCommand::RecordStop { id } => {
                let id = match self.recording_id(id) {
                    Ok(id) => id,
                    Err(response) => return Some(response),
                };
                let recording = self.recordings.get_mut(&id)?;
                recording.end(EndReason::Stopped);
                recording.stoppers.push(reply);
                None
            }
            ControlCommand::RecordList => Some(ControlResponse::ok(RecordingListPayload(
                self.recordings
                    .values()
                    .map(|recording| recording.info(&self.session_name))
                    .collect(),
            ))),
            ControlCommand::RecordMark { label, id } => Some(self.mark_recordings(&label, id)),
            _ => Some(ControlResponse::error("not a record command")),
        }
    }

    fn plan_recording(
        &self,
        target: Option<PaneId>,
        output: &str,
        max_fps: Option<u32>,
        duration_ms: Option<u64>,
        max_bytes: Option<u64>,
        force: bool,
    ) -> std::result::Result<RecordingPlan, ControlResponse> {
        let invalid = |message: String| {
            ControlResponse::error_with(ControlErrorCode::InvalidArgument, message)
        };
        let path = PathBuf::from(output);
        if !path.is_absolute() {
            return Err(invalid(format!(
                "the recording path must be absolute on the session's host, not {output:?}"
            )));
        }
        let max_fps = max_fps.unwrap_or(control::DEFAULT_RECORDING_MAX_FPS);
        if !(1..=control::MAX_RECORDING_MAX_FPS).contains(&max_fps) {
            return Err(invalid(format!(
                "max fps must be from 1 to {}",
                control::MAX_RECORDING_MAX_FPS
            )));
        }
        let duration_ms = duration_ms.unwrap_or(control::DEFAULT_RECORDING_DURATION_MS);
        if !(1..=control::MAX_RECORDING_DURATION_MS).contains(&duration_ms) {
            return Err(invalid("a recording lasts from 1ms to 7 days".to_string()));
        }
        let max_bytes = max_bytes.unwrap_or(control::DEFAULT_RECORDING_MAX_BYTES);
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
        // Compared as the file each path names, not as spelled: `--force` on another spelling of a
        // file being recorded would unlink it from under its writer.
        let resolved = crate::platform::persist::resolved_file_path(&path)
            .map_err(|error| invalid(format!("cannot record to {output}: {error}")))?;
        if self.recordings.values().any(|recording| {
            recording.resolved == resolved
                || crate::platform::persist::same_file(&path, &recording.path)
        }) {
            return Err(ControlResponse::error_with(
                ControlErrorCode::Conflict,
                format!("{output} is already being recorded to"),
            ));
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
        Ok(RecordingPlan {
            pane_id,
            path,
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
        let recorder = Recorder::start(RecorderOptions {
            path: plan.path.clone(),
            overwrite: plan.force,
            header,
            max_bytes: plan.max_bytes,
        })
        .map_err(|error| {
            let path = plan.path.display();
            if error.kind() == io::ErrorKind::AlreadyExists {
                ControlResponse::error_with(
                    ControlErrorCode::Conflict,
                    format!("{path} already exists; pass --force to replace it"),
                )
            } else {
                ControlResponse::error_with(
                    ControlErrorCode::RequestFailed,
                    format!("cannot create {path}: {error}"),
                )
            }
        })?;
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
                resolved: crate::platform::persist::resolved_file_path(&plan.path)
                    .unwrap_or_else(|_| plan.path.clone()),
                path: plan.path,
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
                stoppers: Vec::new(),
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

    fn mark_recordings(&mut self, label: &str, id: Option<u64>) -> ControlResponse {
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
        let candidates: Vec<u64> = match id {
            Some(id) => match self.recording_id(Some(id)) {
                Ok(id) => vec![id],
                Err(response) => return response,
            },
            None => self
                .recordings
                .iter()
                .filter(|(_, recording)| !recording.recorder.closed())
                .map(|(&id, _)| id)
                .collect(),
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
            let due = over || exited.is_some() || recording.due(now);
            recording.capture(pane, now, due);
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
            for reply in recording.follower.into_iter().chain(recording.stoppers) {
                let response = ControlResponse::ok(stopped.clone());
                self.enqueue(
                    reply.client_id,
                    Target::Sender,
                    session_control_reply(reply.capabilities, reply.effective_protocol, response),
                );
                self.set_close_after_flush(reply.client_id);
            }
            self.sync_recording_flag(recording.pane_id);
        }
    }

    /// Whether `client_id` is waiting on a recording, so its connection must stay open.
    pub(super) fn holds_recording_reply(&self, client_id: ClientId) -> bool {
        self.recordings.values().any(|recording| {
            recording
                .follower
                .iter()
                .chain(&recording.stoppers)
                .any(|reply| reply.client_id == client_id)
        })
    }

    /// A departed client stops the recording it followed, and waits for nothing.
    pub(super) fn release_recording_replies(&mut self, client_id: ClientId) {
        for recording in self.recordings.values_mut() {
            recording
                .stoppers
                .retain(|reply| reply.client_id != client_id);
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
            output: output.display().to_string(),
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
                    id: None
                }
            )[0]
            .ok
        );

        let (stopper, _stop_stream) = add_client(&mut server);
        assert!(
            ask(
                &mut server,
                stopper,
                ControlCommand::RecordStop { id: None }
            )
            .is_empty(),
            "held"
        );
        assert!(server.holds_recording_reply(stopper));
        pump_until_finished(&mut server);

        let stopped: RecordingStopped =
            serde_json::from_value(answered(&server, stopper)[0].data.clone().unwrap()).unwrap();
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
            ControlCommand::RecordStop { id: None },
        );
        pump_until_finished(&mut server);

        let (frames, ..) = replay(&path);
        assert_eq!(
            frames,
            ["", "9"],
            "the start, then only the newest state of the burst"
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
            ControlCommand::RecordStop { id: None },
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
                ControlCommand::RecordStop { id: None }
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
            refused(&mut server, ControlCommand::RecordStop { id: None }),
            Some(ControlErrorCode::InvalidArgument)
        );
        assert_eq!(
            refused(
                &mut server,
                ControlCommand::RecordMark {
                    label: "x".into(),
                    id: None
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
