use super::*;
use crate::pane::launch::REPLAY_PROMPT_WAIT;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
#[cfg(all(test, unix))]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Version 3 stores the command a pane was observed running alongside the command it was launched
/// with. Version 2 stored pane launch intent as shell-or-direct rather than a shell-only command
/// string.
const SNAPSHOT_VERSION: u32 = 3;

/// Every format in this inclusive window has a migration to the current format.
const OLDEST_READABLE_SNAPSHOT_VERSION: u32 = 2;

mod migrate;

fn readable_snapshot_version(version: u32) -> bool {
    (OLDEST_READABLE_SNAPSHOT_VERSION..=SNAPSHOT_VERSION).contains(&version)
}

/// How long shutdown waits for an in-flight durable write before abandoning it.
pub(super) const SNAPSHOT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Longest opaque native agent session reference a snapshot is allowed to hand back to a process.
///
/// Rozi never interprets the value, but it did arrive from outside, so it is bounded rather than
/// trusted to be sane.
const MAX_NATIVE_SESSION: usize = 256;

/// A command a restored pane was observed running, waiting for that pane's new shell to be ready
/// to receive it. See [`SessionServer::flush_pending_foreground`].
pub(super) struct PendingForeground {
    pane_id: PaneId,
    /// The generation the pane was restored at. A pane that has since been respawned is a
    /// different pane, and typing a command from the old one into it would be a surprise.
    generation: u64,
    command: String,
    waiting_since: Instant,
}

/// Where a pane's replay bytes for this snapshot come from.
enum ReplaySource {
    /// Freshly exported from the live screen, because the pane changed.
    Exported(Vec<u8>),
    /// Unchanged since the last successful snapshot, so the file already in the live snapshot
    /// directory is still correct and is linked into the new one instead of being rebuilt.
    Reuse,
}

/// One durable snapshot, owned outright so the write never touches live server state.
struct SnapshotJob {
    /// The `dirty_generation` this capture describes.
    generation: u64,
    /// When the attempt began on the server thread, so the reported duration spans capture and
    /// write rather than only the worker's share.
    started: Instant,
    final_path: PathBuf,
    session_name: String,
    meta: SnapshotMeta,
    layout: Option<SharedLayout>,
    replays: Vec<(PaneId, ReplaySource)>,
    /// Per-pane `content_generation` this snapshot persists, adopted once the write succeeds.
    captured: Vec<(PaneId, u64)>,
    /// How the capture split between re-export and reuse, so the shape of a slow snapshot is
    /// visible without reproducing it.
    exported: u32,
    reused: u32,
    exported_bytes: u64,
}

struct SnapshotOutcome {
    generation: u64,
    result: io::Result<()>,
    total: Duration,
    captured: Vec<(PaneId, u64)>,
}

/// A single background writer for durable snapshots.
///
/// Exports stay on the server thread because they need the live `TerminalScreen`; only the write,
/// sync, and rename move here. One job runs at a time: a snapshot deferred because the previous
/// one is still writing simply stays dirty and is retried, which keeps at most one snapshot's
/// worth of replay bytes alive off-thread.
pub(super) struct SnapshotWorker {
    jobs: Option<mpsc::Sender<SnapshotJob>>,
    done: mpsc::Receiver<SnapshotOutcome>,
    handle: Option<std::thread::JoinHandle<()>>,
    in_flight: usize,
}

impl SnapshotWorker {
    fn new() -> Self {
        let (jobs, job_rx) = mpsc::channel::<SnapshotJob>();
        let (done_tx, done) = mpsc::channel::<SnapshotOutcome>();
        let handle = std::thread::Builder::new()
            .name("rozi-snapshot".to_string())
            .spawn(move || {
                for job in job_rx {
                    let generation = job.generation;
                    let started = job.started;
                    let captured = job.captured.clone();
                    let result = write_snapshot_job(job);
                    if done_tx
                        .send(SnapshotOutcome {
                            generation,
                            result,
                            total: started.elapsed(),
                            captured,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .ok();
        Self {
            jobs: Some(jobs),
            done,
            handle,
            in_flight: 0,
        }
    }

    fn busy(&self) -> bool {
        self.in_flight > 0
    }

    fn dispatch(&mut self, job: SnapshotJob) {
        if let Some(jobs) = self.jobs.as_ref()
            && jobs.send(job).is_ok()
        {
            self.in_flight += 1;
        }
    }

    fn drain(&mut self) -> Vec<SnapshotOutcome> {
        let outcomes: Vec<_> = self.done.try_iter().collect();
        self.in_flight = self.in_flight.saturating_sub(outcomes.len());
        outcomes
    }

    fn finish(mut self, grace: Duration) -> Vec<SnapshotOutcome> {
        // Dropping the sender ends the worker loop once the queue empties.
        self.jobs = None;
        let deadline = Instant::now() + grace;
        let mut outcomes = Vec::new();
        while self.in_flight > 0 {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.done.recv_timeout(remaining) {
                Ok(outcome) => {
                    self.in_flight -= 1;
                    outcomes.push(outcome);
                }
                Err(_) => break,
            }
        }
        // Only join once the writer can no longer be mid-rename; otherwise leave it detached and
        // let process exit reap it rather than blocking shutdown on stuck storage.
        if self.in_flight == 0
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
        outcomes
    }
}

/// A pane's replay file inside a snapshot directory.
///
/// Replay files are immutable once published: a snapshot only ever creates them in its own
/// temporary directory, so a reused file is the same bytes under a new link rather than a file
/// anything writes through.
fn replay_file(snapshot_dir: &Path, pane_id: PaneId) -> PathBuf {
    snapshot_dir.join("panes").join(format!("{pane_id}.replay"))
}

fn write_snapshot_job(job: SnapshotJob) -> io::Result<()> {
    let SnapshotJob {
        final_path,
        session_name,
        meta,
        layout,
        replays,
        ..
    } = job;
    let parent = final_path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "snapshot path has no parent")
    })?;
    refuse_newer_snapshot(&final_path)?;
    crate::platform::fs_security::ensure_private_dir(parent)?;
    let suffix = format!(
        "{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp = parent.join(staging_dir_name(&session_name, STAGING_TMP, &suffix));
    let backup = parent.join(staging_dir_name(&session_name, STAGING_OLD, &suffix));
    crate::platform::fs_security::ensure_private_dir(&temp)?;
    let panes_dir = temp.join("panes");
    crate::platform::fs_security::ensure_private_dir(&panes_dir)?;

    for (pane_id, source) in &replays {
        let target = panes_dir.join(format!("{pane_id}.replay"));
        match source {
            ReplaySource::Exported(replay) => write_secure(&target, replay)?,
            ReplaySource::Reuse => {
                let existing = replay_file(&final_path, *pane_id);
                // A hard link keeps the bytes in place instead of copying them, and the old
                // directory is only unlinked after the rename, so the inode outlives it. Copying
                // is the fallback where linking is unavailable (a filesystem without it, or the
                // snapshot root spanning a mount point).
                if fs::hard_link(&existing, &target).is_err() {
                    fs::copy(&existing, &target)?;
                }
            }
        }
    }
    write_secure(
        &temp.join("meta.json"),
        &serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?,
    )?;
    if let Some(layout) = &layout {
        write_secure(
            &temp.join("layout.json"),
            &serde_json::to_vec_pretty(layout).map_err(io::Error::other)?,
        )?;
    }
    sync_directory(&temp)?;
    // Between this rename and the next, the published directory is missing and only the backup
    // holds the session. A crash inside that window is repaired by the next server for this
    // session, in [`SessionServer::recover_interrupted_snapshot`]; discovery lists the backup
    // meanwhile so the session stays visible.
    if final_path.exists() {
        fs::rename(&final_path, &backup)?;
    }
    if let Err(err) = fs::rename(&temp, &final_path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &final_path);
        }
        let _ = fs::remove_dir_all(&temp);
        return Err(err);
    }
    sync_directory(parent)?;
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct SnapshotMeta {
    version: u32,
    session: String,
    saved_at: u64,
    layout_rev: u64,
    #[serde(
        default,
        skip_serializing_if = "crate::session::origin::SessionOrigin::is_empty"
    )]
    origin: crate::session::origin::SessionOrigin,
    /// Read older snapshots written before origins were structured. New writes omit this field.
    #[serde(default, skip_serializing)]
    created_from_profile: Option<String>,
    panes: Vec<SnapshotPane>,
}

impl SnapshotMeta {
    fn effective_origin(&self) -> crate::session::origin::SessionOrigin {
        if !self.origin.is_empty() {
            self.origin.clone()
        } else {
            crate::session::origin::SessionOrigin {
                profile: self.created_from_profile.clone(),
                ..Default::default()
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SnapshotPane {
    #[serde(default)]
    scrollback: Option<usize>,
    pane_id: PaneId,
    generation: u64,
    /// How the pane was *created*: the launch intent a client asked for, or `None` for a plain
    /// interactive shell.
    launch: Option<crate::pane::launch::PaneLaunch>,
    /// What was *running* in the pane when the snapshot was taken, as a line an interactive shell
    /// would accept.
    ///
    /// Deliberately not folded into `launch`. The two answer different questions, and the
    /// difference outlives the restore: when a replayed foreground command exits, a pane whose
    /// `launch` is `None` must fall back to its shell rather than behave like a pane that was
    /// created to run one command. Absent when the pane sat at a prompt, when it was already
    /// running its own launch command, or when
    /// [`ForegroundRestore::Never`](crate::config::ForegroundRestore::Never) declined to record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    foreground: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_resume: Option<SnapshotAgentResume>,
    cwd: Option<String>,
    keep_open: bool,
    title: Option<String>,
    palette: WirePalette,
    cols: u16,
    rows: u16,
}

#[derive(Serialize, Deserialize)]
struct SnapshotAgentResume {
    agent: String,
    session: String,
    reported_at: u64,
    /// See [`RetainedResume::reported`]. A snapshot from before this was recorded reads as not
    /// reported, which reopens the conversation the way it always did.
    #[serde(default)]
    reported: bool,
}

/// The conversation a restored pane resumes.
///
/// What the pane's live state names, from [`live_agent_resume`], ahead of a conversation retained
/// from agent rows whose window has since left - unless no agent rows are listed and the live
/// answer is a hook report no newer than those rows, which they had already outranked.
fn agent_resume(pane: &ServerPane) -> Option<SnapshotAgentResume> {
    let live = live_agent_resume(pane);
    let retained = pane.agent.retained_resume.as_ref();
    let resume = match (live, retained) {
        (Some(live), Some(retained))
            if !pane
                .runtime
                .rows
                .iter()
                .any(protocol::PublishedRow::is_agent)
                && live.reported_at <= retained.reported_at =>
        {
            retained.clone()
        }
        (Some(live), _) => live,
        (None, retained) => retained?.clone(),
    };
    Some(SnapshotAgentResume {
        agent: resume.agent,
        session: resume.session,
        reported_at: resume.reported_at,
        reported: resume.reported,
    })
}

/// Which conversation a snapshot taken now would resume, and how: the agent, its session, and
/// whether its hooks reported it, without the report time, which moves on its own. A change in this
/// is a change the snapshot must catch.
pub(super) fn resume_target(pane: &ServerPane) -> Option<(String, String, bool)> {
    agent_resume(pane).map(|resume| (resume.agent, resume.session, resume.reported))
}

/// The conversation the pane's live state names.
///
/// The pane's active agent row wins when it names one: a client running several conversations
/// shows that one, and its hooks may still be reporting on a conversation it has since switched
/// away from. Otherwise the pane's integration report decides, as it always has. An activity row
/// is never a resume candidate, whatever it names; see [`protocol::active_agent_row`].
pub(super) fn live_agent_resume(pane: &ServerPane) -> Option<RetainedResume> {
    let report = pane.runtime.integration.as_deref();
    let active =
        protocol::active_agent_row(&pane.runtime.rows).and_then(|row| row.native_session.clone());
    if let Some(session) = active {
        let agent = report
            .map(|report| report.identity.id.clone())
            .or_else(|| {
                pane.runtime
                    .detected_agent
                    .as_ref()
                    .map(|detected| detected.agent.id.clone())
            })?;
        let reported_at = report
            .filter(|report| report.native_session.as_deref() == Some(session.as_str()))
            .map_or_else(crate::runtime_metrics::unix_time_millis, |report| {
                report.reported_at_unix_ms
            });
        return Some(RetainedResume {
            agent,
            session,
            reported_at,
            reported: false,
        });
    }
    report.and_then(|report| {
        Some(RetainedResume {
            agent: report.identity.id.clone(),
            session: report.native_session.clone()?,
            reported_at: report.reported_at_unix_ms,
            reported: true,
        })
    })
}

/// What a restored pane does beyond coming back with its scrollback, in the one precedence order:
/// a native agent conversation outranks a replayed foreground command, which outranks leaving the
/// pane to whatever its own launch intent starts.
enum RestoreAction {
    /// Exec the agent's resume command as this pane's first process.
    ResumeAgent(AgentResumeLaunch),
    /// Type a line at the restored shell's prompt: a foreground command the pane was observed
    /// running, or a resume command `hold` declined to run unasked.
    Type(String),
    /// Nothing to add.
    Nothing,
}

impl SessionServer {
    pub(super) fn snapshot_path(&self) -> io::Result<PathBuf> {
        if !crate::session::discovery::valid_attach_target(&self.session_name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        let root = self
            .settings
            .snapshot_dir
            .clone()
            .or_else(default_snapshot_dir)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "state directory unavailable")
            })?;
        Ok(root.join(&self.session_name))
    }

    /// Record that a snapshot no longer describes the session.
    pub(super) fn mark_dirty(&mut self) {
        self.dirty_generation = self.dirty_generation.saturating_add(1);
    }

    /// Whether live state has moved past the last successfully persisted snapshot.
    pub(super) fn snapshot_dirty(&self) -> bool {
        self.dirty_generation != self.snapshot_generation
    }

    pub(super) fn maybe_snapshot(&mut self) -> io::Result<()> {
        if !self.settings.resurrect
            || !self.snapshot_dirty()
            || crate::session::naming::is_ephemeral_session_name(&self.session_name)
        {
            return Ok(());
        }
        // Checked before the detach edge is consumed below, so a snapshot deferred because the
        // worker is busy still sees `last_detached` on a later call.
        if self
            .snapshot_worker
            .as_ref()
            .is_some_and(SnapshotWorker::busy)
        {
            return Ok(());
        }
        let attached = self.attached_count();
        let last_detached = self.last_attached_count > 0 && attached == 0;
        self.last_attached_count = attached;
        if !last_detached && self.last_snapshot.elapsed() < self.settings.snapshot_interval {
            return Ok(());
        }
        self.dispatch_snapshot()
    }

    fn dispatch_snapshot(&mut self) -> io::Result<()> {
        // Advance the deadline before doing any work. A transient filesystem error keeps the
        // snapshot dirty for a later retry, but must not turn the server loop into a 1 ms
        // export/write/sync retry storm.
        self.last_snapshot = Instant::now();
        let started = Instant::now();
        self.resurrection_metrics.attempts = self.resurrection_metrics.attempts.saturating_add(1);
        let job = self.capture_snapshot(started);
        let blocking = crate::runtime_metrics::duration_micros(started.elapsed());
        self.resurrection_metrics.last_blocking_us = blocking;
        self.resurrection_metrics.max_blocking_us =
            self.resurrection_metrics.max_blocking_us.max(blocking);
        let job = match job {
            Ok(job) => job,
            Err(err) => {
                // Capture failed, so nothing reaches the worker and no completion will arrive.
                self.record_snapshot_outcome(blocking, false);
                self.forget_persisted_replays();
                return Err(err);
            }
        };
        self.resurrection_metrics.last_exported_panes = job.exported;
        self.resurrection_metrics.last_reused_panes = job.reused;
        self.resurrection_metrics.last_exported_bytes = job.exported_bytes;
        self.snapshot_worker
            .get_or_insert_with(SnapshotWorker::new)
            .dispatch(job);
        Ok(())
    }

    /// Finish any in-flight write and persist the newest generation before a signal-driven stop.
    ///
    /// The periodic interval is irrelevant at shutdown: there will be no later retry, and killing
    /// the PTYs before capturing their final screens would turn a clean stop into avoidable
    /// resurrection loss.
    pub(super) fn snapshot_before_shutdown(&mut self) -> io::Result<()> {
        if !self.finish_snapshots() {
            // The old writer exceeded the bounded shutdown grace and may still be inside its atomic
            // replacement. Starting a second writer would race it; preserve the previous behavior
            // and let process exit end the attempt instead.
            return Ok(());
        }
        if !self.settings.resurrect
            || !self.snapshot_dirty()
            || crate::session::naming::is_ephemeral_session_name(&self.session_name)
        {
            return Ok(());
        }
        self.dispatch_snapshot()?;
        self.finish_snapshots();
        Ok(())
    }

    /// Apply any finished snapshot writes. Called once per server-loop iteration.
    pub(super) fn drain_snapshot_results(&mut self) -> io::Result<()> {
        let Some(worker) = self.snapshot_worker.as_mut() else {
            return Ok(());
        };
        let mut failure = None;
        for outcome in worker.drain() {
            let total = crate::runtime_metrics::duration_micros(outcome.total);
            let ok = outcome.result.is_ok();
            self.record_snapshot_outcome(total, ok);
            if ok {
                // Only the generation this job captured is persisted. Changes that arrived while
                // it was being written keep the session dirty for the next snapshot.
                self.snapshot_generation = outcome.generation;
                self.adopt_persisted_replays(outcome.captured);
            } else {
                self.forget_persisted_replays();
                if let Err(err) = outcome.result {
                    failure = Some(err);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Record which replay files the snapshot directory now holds, or forget them all.
    ///
    /// Reuse is only ever as safe as this map, so a failed write drops every entry: whatever went
    /// wrong may have left the directory without the files a later reuse would link against, and a
    /// full re-export is the self-healing answer.
    fn adopt_persisted_replays(&mut self, captured: Vec<(PaneId, u64)>) {
        self.persisted_replays = captured.into_iter().collect();
    }

    fn forget_persisted_replays(&mut self) {
        self.persisted_replays.clear();
    }

    /// Drive completion draining until every dispatched attempt is accounted for, returning the
    /// result the completing drain reported.
    ///
    /// Snapshots are written off the server loop, so a test that asserts on snapshot *files* has
    /// to wait for the worker the way `run_listener` does, rather than assuming `maybe_snapshot`
    /// finished the write.
    #[cfg(test)]
    pub(super) fn wait_for_snapshots(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let result = self.drain_snapshot_results();
            let metrics = &self.resurrection_metrics;
            if metrics.successes + metrics.failures == metrics.attempts {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot worker did not report a completion"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn record_snapshot_outcome(&mut self, duration_us: u64, ok: bool) {
        self.resurrection_metrics.last_duration_us = duration_us;
        self.resurrection_metrics.max_duration_us =
            self.resurrection_metrics.max_duration_us.max(duration_us);
        if ok {
            self.resurrection_metrics.successes =
                self.resurrection_metrics.successes.saturating_add(1);
        } else {
            self.resurrection_metrics.failures =
                self.resurrection_metrics.failures.saturating_add(1);
        }
    }

    /// Let an in-flight snapshot finish before the process exits.
    ///
    /// A snapshot is most often triggered by the last client detaching, which is exactly when
    /// losing it matters, so shutdown waits. The wait is bounded: a hung filesystem must not hold
    /// the session open forever, and abandoning a partial write is safe because the previous
    /// snapshot is only replaced by an atomic rename.
    pub(super) fn finish_snapshots(&mut self) -> bool {
        if let Some(worker) = self.snapshot_worker.take() {
            for outcome in worker.finish(SNAPSHOT_SHUTDOWN_GRACE) {
                let total = crate::runtime_metrics::duration_micros(outcome.total);
                self.record_snapshot_outcome(total, outcome.result.is_ok());
                if outcome.result.is_ok() {
                    self.snapshot_generation = outcome.generation;
                    self.adopt_persisted_replays(outcome.captured);
                } else {
                    self.forget_persisted_replays();
                }
            }
        }
        let metrics = &self.resurrection_metrics;
        metrics.successes + metrics.failures == metrics.attempts
    }

    /// Capture everything the durable write needs, so the write itself needs no access back to
    /// live server state. This is the only part the server loop is blocked for.
    fn capture_snapshot(&mut self, started: Instant) -> io::Result<SnapshotJob> {
        let final_path = self.snapshot_path()?;
        let mut panes = Vec::new();
        let mut replays = Vec::new();
        let mut captured = Vec::new();
        let (mut exported, mut reused, mut exported_bytes) = (0_u32, 0_u32, 0_u64);
        let record_foreground =
            self.settings.resurrect_foreground != crate::config::ForegroundRestore::Never;
        // `never` is an instruction not to keep what a pane was doing, and a reported conversation
        // reference is exactly that - so it also keeps opaque agent session ids out of the state
        // directory, which is the privacy half of the setting.
        let record_agents = self.settings.resurrect_agents && record_foreground;
        for (&pane_id, pane) in &mut self.panes {
            // The popup slot is a transient client-local overlay; resurrecting it would
            // revive an invisible orphan pane no client adopts.
            if pane.exited.is_some() || pane_id == crate::pane::id::POPUP_PANE_ID {
                continue;
            }
            // A pane running its own launch command has nothing to add: restoring `launch` already
            // starts it. Only a shell that someone typed into is telling the snapshot something it
            // does not otherwise know - and a pane still running the resume command a restore
            // started for it is in the same position, with no shell in it at all.
            let foreground =
                (record_foreground && pane.launch.is_none() && pane.agent_resume.is_none())
                    .then(|| observed_foreground_command(pane))
                    .flatten();
            panes.push(SnapshotPane {
                scrollback: Some(pane.scrollback),
                pane_id,
                generation: pane.generation,
                launch: pane.launch.clone(),
                foreground,
                agent_resume: record_agents.then(|| agent_resume(pane)).flatten(),
                cwd: pane.spawnable_cwd(),
                keep_open: pane.keep_open,
                title: pane.effective_title(),
                palette: pane.palette,
                cols: pane.cols,
                rows: pane.rows,
            });
            let content_generation = pane.content_generation;
            // Exporting is the expensive half of a snapshot and it scales with retained history,
            // so an idle pane reuses the file the last snapshot already wrote for this exact
            // generation. `persisted_replays` only records generations a snapshot *succeeded* on,
            // so a reuse can never point at a file that was never written.
            //
            // The source is still confirmed present here, on the loop, where re-exporting is
            // possible: a snapshot directory emptied behind the server's back must degrade into a
            // full export rather than failing the attempt, since the worker has no screen to fall
            // back to.
            let source = if self.persisted_replays.get(&pane_id) == Some(&content_generation)
                && replay_file(&final_path, pane_id).is_file()
            {
                reused += 1;
                ReplaySource::Reuse
            } else {
                let replay = pane.screen_without_change().export_replay_bytes();
                exported += 1;
                exported_bytes += replay.len() as u64;
                ReplaySource::Exported(replay)
            };
            replays.push((pane_id, source));
            captured.push((pane_id, content_generation));
        }
        panes.sort_by_key(|pane| pane.pane_id);
        let layout = self.layout.clone().map(|mut layout| {
            for workspace in &mut layout.workspaces {
                workspace.panes.retain(|saved| {
                    self.panes
                        .get(&saved.pane_id)
                        .is_some_and(|pane| pane.exited.is_none())
                });
            }
            layout
        });
        Ok(SnapshotJob {
            generation: self.dirty_generation,
            started,
            final_path,
            session_name: self.session_name.clone(),
            meta: SnapshotMeta {
                version: SNAPSHOT_VERSION,
                session: self.session_name.clone(),
                saved_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                layout_rev: self.layout_rev,
                origin: self.origin.clone(),
                created_from_profile: None,
                panes,
            },
            layout,
            replays,
            captured,
            exported,
            reused,
            exported_bytes,
        })
    }

    /// Only the owning server repairs snapshots; client discovery remains read-only.
    /// Backups survive until the published metadata and every replay are readable.
    pub(super) fn recover_interrupted_snapshot(&self) -> io::Result<bool> {
        let path = self.snapshot_path()?;
        let candidate = snapshot_recovery_candidate(&path, &self.session_name)?;
        refuse_newer_snapshot(&candidate)?;
        let Some(parent) = path.parent() else {
            return Ok(false);
        };
        let recovered = candidate != path && path.exists();
        if candidate != path {
            if recovered {
                preserve_damaged_snapshot(&path, &self.session_name)?;
            }
            fs::rename(&candidate, &path)?;
            sync_directory(parent)?;
        }
        if complete_snapshot(&path, &self.session_name).is_ok() {
            cleanup_redundant_backups(parent, &self.session_name);
        }
        remove_staging_dirs_of_kind(parent, &self.session_name, &[STAGING_TMP]);
        Ok(recovered)
    }

    fn snapshot_notice(&mut self, message: String) {
        eprintln!("rozi: {message}");
        self.session_notify(
            message,
            Some("Session recovery".into()),
            crate::control::NotifyLevel::Error,
        );
    }

    fn restore_layout(&mut self, path: &Path, layout_rev: u64) -> (Option<SharedLayout>, bool) {
        let result = fs::read(path.join("layout.json"));
        if matches!(&result, Err(err) if err.kind() == io::ErrorKind::NotFound && layout_rev == 0) {
            return (None, false);
        }
        if let Some(layout) = result
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            return (Some(layout), false);
        }
        self.snapshot_notice(
            "Saved layout could not be read; restoring the panes without their saved placement."
                .into(),
        );
        (None, true)
    }

    fn restore_replay(&mut self, path: &Path, saved: &SnapshotPane) -> (Vec<u8>, bool) {
        match fs::read(replay_file(path, saved.pane_id)) {
            Ok(replay) => (replay, false),
            Err(_) => {
                self.snapshot_notice(format!(
                    "History for pane {}{} could not be read; restoring it with empty history.",
                    saved.pane_id,
                    saved
                        .title
                        .as_ref()
                        .map_or(String::new(), |title| format!(" (`{title}`)")),
                ));
                (Vec::new(), true)
            }
        }
    }

    fn validate_restored_layout(
        &mut self,
        layout: Option<SharedLayout>,
    ) -> (Option<SharedLayout>, bool) {
        let Some(layout_ref) = &layout else {
            return (None, false);
        };
        if layout_ref.validate().is_ok()
            && (layout_ref.workspaces.is_empty()
                || self.validate_shared_layout_against_panes(layout_ref))
        {
            return (layout, false);
        }
        self.snapshot_notice(
            "Saved layout is invalid; restoring the panes without their saved placement.".into(),
        );
        (None, true)
    }

    pub(super) fn restore(&mut self) -> io::Result<usize> {
        if self.recover_interrupted_snapshot()? {
            self.snapshot_notice(
                "Restored the session from a backup; kept the damaged snapshot.".into(),
            );
        }
        let path = self.snapshot_path()?;
        let meta = read_restorable_meta(&path, &self.session_name)?;
        self.origin = meta.effective_origin();
        let (mut layout, mut damaged) = self.restore_layout(&path, meta.layout_rev);
        let mut restored = 0;
        let mut resumed = std::collections::HashSet::new();
        // Restore has no client to inject integration into the shell's argv and environment.
        // Without it, prompt readiness never arrives and every command waits for the fallback.
        let (shell, integration_env) =
            crate::platform::shell_integration::resolve_interactive_shell(
                Some(&self.settings.shell),
                &crate::platform::command::ShellEnv::from_process(),
                self.settings.shell_integration,
                &crate::platform::shell_integration::InjectionEnv::from_process(),
            );
        for saved in meta.panes {
            let generation = self.next_generation;
            self.next_generation += 1;
            let (replay, missing) = self.restore_replay(&path, &saved);
            damaged |= missing;
            let (agent_resume, typed) = match self.restore_action(&saved, &mut resumed) {
                RestoreAction::ResumeAgent(resume) => (Some(resume), None),
                RestoreAction::Type(line) => (None, Some(line)),
                RestoreAction::Nothing => (None, None),
            };
            // A typed line needs a shell to read it. A pane launched as the agent itself is the one
            // that could have both: starting that launch would run a new conversation unasked and
            // put the line in its prompt.
            let launch = if typed.is_some() { None } else { saved.launch };
            let result = self.spawn_pane_inner(
                SpawnRequest {
                    owner: None,
                    pane_id: saved.pane_id,
                    generation,
                    launch,
                    agent_resume,
                    cwd: saved.cwd,
                    title: saved.title,
                    cols: saved.cols,
                    rows: saved.rows,
                    keep_open: saved.keep_open,
                    // A restored pane is a pane like any other: a program in it still needs
                    // `ROZI` and `ROZI_PANE` to know where it runs. There is no UI yet to name.
                    env: crate::pane::spawn_policy::spawn_environment(
                        crate::pane::spawn_policy::SpawnOrigin::Headless,
                        saved.pane_id,
                        &integration_env,
                    ),
                    palette: saved.palette,
                    shell: shell.as_argv(),
                    command_shell: self.settings.command_shell.clone(),
                    // Resurrect predates any client attaching; the first controller resize
                    // reports the real cell size.
                    cell: None,
                },
                Some(&replay),
                true,
                saved.scrollback,
            );
            if matches!(result, ServerMessage::SpawnResult { ok: true, .. }) {
                restored += 1;
                if let Some(line) = typed.as_deref() {
                    self.replay_foreground_command(saved.pane_id, generation, line);
                }
            }
            update_restored_layout_generation(&mut layout, saved.pane_id, generation);
        }
        let (layout, invalid_layout) = self.validate_restored_layout(layout);
        damaged |= invalid_layout;
        if damaged && copy_damaged_snapshot(&path, &self.session_name).is_err() {
            self.snapshot_notice("Could not keep a separate copy of the damaged snapshot.".into());
        }
        self.layout = layout;
        self.layout_rev = u64::from(self.layout.is_some());
        self.snapshot_generation = self.dirty_generation;
        if damaged {
            // Publish the recovered state even if no shell emits output before detaching.
            self.mark_dirty();
        }
        self.forget_persisted_replays();
        self.last_snapshot = Instant::now();
        Ok(restored)
    }

    /// Decide what a restored pane does beyond coming back, in the fixed precedence
    /// [`RestoreAction`] documents.
    ///
    /// `resumed` accumulates the conversations already claimed by an earlier pane in this restore.
    fn restore_action(
        &self,
        saved: &SnapshotPane,
        resumed: &mut std::collections::HashSet<(String, String)>,
    ) -> RestoreAction {
        if let Some(resume) = self.resolve_agent_resume(saved.agent_resume.as_ref(), resumed) {
            // `hold` means nothing runs that the user did not ask for, and reopening a conversation
            // is no exception: the command is left at the prompt for them to run or delete.
            return if self.settings.resurrect_foreground == crate::config::ForegroundRestore::Hold {
                // Rendered for the shell this session actually restores panes with, not for a
                // POSIX one: `auto` execs direct argv and never involves a shell, so `hold` is the
                // only path where quoting exists at all - and the only one that can be wrong.
                let quoting = crate::pane::launch::PromptQuoting::of(
                    self.settings.shell.first().map(String::as_str),
                );
                // Always `Some`: a resume launch is direct argv by construction.
                let mut line = crate::pane::launch::prompt_line(
                    resume.launch.argv().unwrap_or_default(),
                    quoting,
                );
                // A conversation still running as a background session cannot be resumed, only
                // attached to. The shell decides which, so the line works either way.
                if let Some(attach) = resume.attach.as_deref().filter(|_| {
                    crate::pane::launch::PromptQuoting::chains_with_or(
                        self.settings.shell.first().map(String::as_str),
                    )
                }) {
                    line = format!(
                        "{line} || {}",
                        crate::pane::launch::prompt_line(attach, quoting)
                    );
                }
                RestoreAction::Type(line)
            } else {
                RestoreAction::ResumeAgent(resume)
            };
        }
        match saved.foreground.clone() {
            Some(command) => RestoreAction::Type(command),
            None => RestoreAction::Nothing,
        }
    }

    /// Turn a snapshot's native agent session fact into a way to reopen that conversation, or
    /// `None` when anything it depended on no longer holds.
    ///
    /// The snapshot recorded a fact - this agent reported this conversation - and deliberately no
    /// recipe for reopening it. The recipe comes from the agent definition *this* Rozi has loaded,
    /// so an agent whose resume flags have changed since resumes the current way, and an agent that
    /// no longer declares a resume capability at all restores as an ordinary pane rather than with
    /// a command nobody can run any more.
    fn resolve_agent_resume(
        &self,
        saved: Option<&SnapshotAgentResume>,
        resumed: &mut std::collections::HashSet<(String, String)>,
    ) -> Option<AgentResumeLaunch> {
        if !self.settings.resurrect_agents
            // Read as the setting stands *now*, so starting a server with `never` also defuses a
            // snapshot taken while it was on.
            || self.settings.resurrect_foreground == crate::config::ForegroundRestore::Never
        {
            return None;
        }
        let saved = saved?;
        if saved.session.is_empty()
            || saved.session.len() > MAX_NATIVE_SESSION
            // A reference is echoed by whatever agent receives it. Control characters are not part
            // of any conversation id and a terminal would act on them.
            || saved.session.chars().any(char::is_control)
        {
            return None;
        }
        let definition = self.settings.agents.by_id(&saved.agent)?;
        let argv = if saved.reported {
            definition.reported_resume_command(&saved.session)?
        } else {
            definition.resume_command(&saved.session)?
        };
        let launch = crate::pane::launch::PaneLaunch::direct(argv).ok()?;
        // One conversation, one pane. Two panes reopening the same agent session would leave both
        // writing into one history, and neither of them is the one the user was talking to.
        if !resumed.insert((saved.agent.clone(), saved.session.clone())) {
            return None;
        }
        Some(AgentResumeLaunch {
            label: definition.label().to_string(),
            launch,
            attach: definition.attach_command(&saved.session),
        })
    }

    /// Queue a captured foreground command to be typed into a freshly restored pane.
    ///
    /// Typing is the whole mechanism, and it is the reason the pane's `launch` stays `None`: this
    /// is a shell that has a command in it, not a pane created to run one. When the command exits,
    /// the shell is still there, exactly as it would be had the user typed it.
    ///
    /// Nothing is written yet. A shell that has not finished starting has not necessarily taken
    /// the terminal's input queue over from its own startup scripts, and a command line half
    /// eaten by an `rc` file that reads stdin would submit *something other than what was
    /// captured* - which is the one failure mode `auto` cannot be allowed to have. See
    /// [`SessionServer::flush_pending_foreground`] for what it waits for.
    fn replay_foreground_command(&mut self, pane_id: PaneId, generation: u64, command: &str) {
        if self.settings.resurrect_foreground == crate::config::ForegroundRestore::Never {
            // Capture already declined to record anything under `Never`, but a snapshot taken
            // under a different setting must still be honored as the setting reads *now*.
            return;
        }
        self.pending_foreground.push(PendingForeground {
            pane_id,
            generation,
            command: command.to_string(),
            waiting_since: Instant::now(),
        });
    }

    /// Type each queued command into its pane once that pane's shell is ready for it.
    ///
    /// "Ready" is the shell's end-of-prompt report (`OSC 133 B`, surfaced as
    /// [`PaneCommandPhase::Input`]). The earlier `OSC 133 A` only starts prompt rendering;
    /// sending input then can echo it before the prompt and display it twice. A pane with no
    /// input-ready marker falls back to writing after [`REPLAY_PROMPT_WAIT`]. The bytes
    /// then take the same path a client's keystrokes do; to the shell this is indistinguishable
    /// from someone typing fast.
    ///
    /// Under [`Auto`](crate::config::ForegroundRestore::Auto) the line is submitted. Under
    /// [`Hold`](crate::config::ForegroundRestore::Hold) it is left at the prompt unsubmitted, so
    /// running it is a keystroke the user chose - and so is deleting it.
    pub(super) fn flush_pending_foreground(&mut self) {
        if self.pending_foreground.is_empty() {
            return;
        }
        let submit = self.settings.resurrect_foreground == crate::config::ForegroundRestore::Auto;
        let mut ready = Vec::new();
        let mut waiting = Vec::new();
        for pending in std::mem::take(&mut self.pending_foreground) {
            // A pane that went away, or was respawned under a new generation, before its shell was
            // ever ready has nothing left to type into.
            let Some(pane) = self.panes.get(&pending.pane_id) else {
                continue;
            };
            if pane.generation != pending.generation {
                continue;
            }
            let prompted = pane.runtime.command_phase == protocol::PaneCommandPhase::Input;
            let fallback = pending.waiting_since.elapsed() >= REPLAY_PROMPT_WAIT;
            if prompted || fallback {
                ready.push(pending);
            } else {
                waiting.push(pending);
            }
        }
        self.pending_foreground = waiting;
        for pending in ready {
            let mut bytes = pending.command.into_bytes();
            if submit {
                bytes.push(b'\r');
            }
            self.handle_pane_input(None, pending.pane_id, pending.generation, &bytes);
        }
    }

    /// Remove the published snapshot and any staging or backup directory left over for this
    /// session. A writer killed between creating `.{session}.tmp-…` and the rename that publishes
    /// it leaves that directory behind holding a complete `meta.json`; forgetting the session has
    /// to take the debris with it, or bytes describing a deleted session outlive it on disk.
    ///
    /// Only a server owning this session name ever writes those directories, and the two callers
    /// (a snapshot the user forgot, and shutdown after `finish_snapshots`) both run with no write
    /// in flight, so nothing live is swept out from under.
    pub(super) fn delete_snapshot(&self) -> io::Result<()> {
        let path = self.snapshot_path()?;
        let removed = match fs::remove_dir_all(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
        if let Some(parent) = path.parent() {
            remove_staging_dirs(parent, &self.session_name);
        }
        removed
    }
}

/// A snapshot being written, before it is published.
pub(super) const STAGING_TMP: &str = "tmp";
/// The published snapshot a write is replacing, kept until the replacement is in place.
pub(super) const STAGING_OLD: &str = "old";
const STAGING_DAMAGED: &str = "damaged";

/// `.{session}.{kind}-{suffix}`: a staging or backup directory next to the published snapshot.
pub(super) fn staging_dir_name(session_name: &str, kind: &str, suffix: &str) -> String {
    format!(".{session_name}.{kind}-{suffix}")
}

/// The session and kind a staging or backup directory name belongs to, with its suffix.
///
/// A session name cannot contain `.`, so the first separator ends it and `foo` never claims a
/// directory of `foo-bar`.
fn parse_staging_dir_name(name: &str) -> Option<(&str, &str, &str)> {
    let (session, rest) = name.strip_prefix('.')?.split_once('.')?;
    let (kind, suffix) = rest.split_once('-')?;
    (!session.is_empty() && matches!(kind, STAGING_TMP | STAGING_OLD | STAGING_DAMAGED))
        .then_some((session, kind, suffix))
}

/// A backup directory left by an interrupted replacement, with the snapshot it holds.
struct Backup {
    path: PathBuf,
    meta: SnapshotMeta,
    /// When the write that moved it aside started, from its suffix; orders backups that share a
    /// `saved_at` second.
    written_nanos: u128,
}

/// Every backup directory in `parent` whose snapshot this build can restore, by session.
///
/// A backup counts only when its `meta.json` names the session its directory name does and has
/// a format in this build's support window, the same test a published snapshot passes.
fn valid_backups(parent: &Path) -> Vec<(String, Backup)> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let dir_name = entry.file_name().into_string().ok()?;
            let (session, kind, suffix) = parse_staging_dir_name(&dir_name)?;
            if kind != STAGING_OLD {
                return None;
            }
            let meta = complete_snapshot(&entry.path(), session).ok()?;
            let written_nanos = suffix
                .split_once('.')
                .and_then(|(_, nanos)| nanos.parse().ok())
                .unwrap_or(0);
            (readable_snapshot_version(meta.version) && meta.session == session).then(|| {
                (
                    session.to_string(),
                    Backup {
                        path: entry.path(),
                        meta,
                        written_nanos,
                    },
                )
            })
        })
        .collect()
}

fn backup_order(backup: &Backup) -> (u64, u128) {
    (backup.meta.saved_at, backup.written_nanos)
}

/// The newest restorable backup of `session_name`, if a replacement was interrupted.
fn newest_valid_backup(parent: &Path, session_name: &str) -> Option<PathBuf> {
    valid_backups(parent)
        .into_iter()
        .filter(|(session, _)| session == session_name)
        .map(|(_, backup)| backup)
        .max_by_key(backup_order)
        .map(|backup| backup.path)
}

/// The newest backup of `session_name` this build cannot read, such as one a build with another
/// snapshot version moved aside. Publishing it again keeps the session for a build that can read
/// it; `restore` then refuses it rather than this build losing it.
fn newest_unreadable_backup(parent: &Path, session_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(parent).ok()?;
    entries
        .flatten()
        .filter_map(|entry| {
            let dir_name = entry.file_name().into_string().ok()?;
            let (session, kind, suffix) = parse_staging_dir_name(&dir_name)?;
            let written_nanos: u128 = suffix
                .split_once('.')
                .and_then(|(_, nanos)| nanos.parse().ok())
                .unwrap_or(0);
            // The directory alone is the evidence: every backup was once a published snapshot, so
            // one without its metadata is damaged, not absent, as a published one would be.
            (session == session_name && kind == STAGING_OLD && entry.path().is_dir())
                .then(|| (written_nanos, entry.path()))
        })
        .max_by_key(|(written_nanos, _)| *written_nanos)
        .map(|(_, path)| path)
}

/// Best-effort sweep of abandoned staging/backup directories for one session. A leftover that
/// resists removal is not worth failing a delete over: it is already invisible to discovery.
fn remove_staging_dirs(parent: &Path, session_name: &str) {
    remove_staging_dirs_of_kind(
        parent,
        session_name,
        &[STAGING_TMP, STAGING_OLD, STAGING_DAMAGED],
    );
}

fn remove_staging_dirs_of_kind(parent: &Path, session_name: &str, kinds: &[&str]) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .and_then(parse_staging_dir_name)
            .is_some_and(|(session, kind, _)| session == session_name && kinds.contains(&kind))
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub fn delete_snapshot_for(name: &str) -> io::Result<()> {
    let server = SessionServer::new_named_with_settings(
        name,
        ServerSettings {
            resurrect: true,
            ..ServerSettings::default()
        },
    );
    // A deployment belongs to the session as completely as its snapshot does. Forced cleanup can
    // reach here without the server's own shutdown path, so both go together.
    let deployments = server.deployments.remove_all();
    let snapshot = server.delete_snapshot();
    deployments.and(snapshot)
}

/// Why this build will not restore a snapshot that is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unrestorable {
    /// Saved in another snapshot format, which the rozi version that saved it can read.
    Format(u32),
    /// Missing or malformed metadata, which no rozi version can read.
    Damaged,
    /// Its metadata names another session than the one it is saved as.
    OtherSession(String),
}

impl Unrestorable {
    /// Whether another rozi version could restore it.
    pub(crate) fn another_version_reads_it(&self) -> bool {
        matches!(self, Self::Format(_))
    }

    /// The reason a refusal carries, if `err` is one.
    fn of(err: &io::Error) -> Option<&Self> {
        err.get_ref()?.downcast_ref()
    }
}

impl std::fmt::Display for Unrestorable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(found) => write!(
                f,
                "its snapshot is in format {found}, and this rozi reads formats {OLDEST_READABLE_SNAPSHOT_VERSION} through {SNAPSHOT_VERSION}"
            ),
            Self::Damaged => f.write_str("its snapshot is damaged"),
            Self::OtherSession(session) => {
                write!(f, "its snapshot belongs to session `{session}`")
            }
        }
    }
}

impl std::error::Error for Unrestorable {}

/// The snapshot in `dir`, if this build can restore it. Only a missing `dir` is `NotFound`: there is
/// no snapshot. Any other failure, a missing `meta.json` included, is `InvalidData` carrying the
/// [`Unrestorable`] reason, so nothing starts empty over what is left of the snapshot.
fn read_restorable_meta(dir: &Path, session_name: &str) -> io::Result<SnapshotMeta> {
    let refuse = |reason| io::Error::new(io::ErrorKind::InvalidData, reason);
    match fs::metadata(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no snapshot"));
        }
        Ok(meta) if meta.is_dir() => {}
        _ => return Err(refuse(Unrestorable::Damaged)),
    }
    let bytes = fs::read(dir.join("meta.json")).map_err(|_| refuse(Unrestorable::Damaged))?;
    let header: SnapshotHeader =
        serde_json::from_slice(&bytes).map_err(|_| refuse(Unrestorable::Damaged))?;
    if !readable_snapshot_version(header.version) {
        return Err(refuse(Unrestorable::Format(header.version)));
    }
    if header.session != session_name {
        return Err(refuse(Unrestorable::OtherSession(header.session)));
    }
    let mut document = serde_json::from_slice(&bytes).map_err(|_| refuse(Unrestorable::Damaged))?;
    migrate::to_current(&mut document, header.version)
        .map_err(|_| refuse(Unrestorable::Damaged))?;
    serde_json::from_value(document).map_err(|_| refuse(Unrestorable::Damaged))
}

fn update_restored_layout_generation(
    layout: &mut Option<SharedLayout>,
    pane_id: PaneId,
    generation: u64,
) {
    let Some(layout) = layout else { return };
    for workspace in &mut layout.workspaces {
        for pane in &mut workspace.panes {
            if pane.pane_id == pane_id {
                pane.generation = generation;
            }
        }
    }
}

/// Read-only selection shared by discovery, refusal diagnostics, and the owning server.
fn snapshot_recovery_candidate(path: &Path, session: &str) -> io::Result<PathBuf> {
    refuse_newer_snapshot(path)?;
    if complete_snapshot(path, session).is_ok() {
        return Ok(path.to_path_buf());
    }
    let Some(parent) = path.parent() else {
        return Ok(path.to_path_buf());
    };
    if let Some(backup) = newest_valid_backup(parent, session) {
        return Ok(backup);
    }
    if !path.exists()
        && let Some(backup) = newest_unreadable_backup(parent, session)
    {
        return Ok(backup);
    }
    Ok(path.to_path_buf())
}

/// Only complete, compatible backups are redundant. Preserve damage and leave newer formats
/// exactly where they are, including when an older compatible backup won selection.
fn cleanup_redundant_backups(parent: &Path, session: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(parse_staging_dir_name)
            .is_some_and(|(owner, kind, _)| owner == session && kind == STAGING_OLD)
        {
            continue;
        }
        let path = entry.path();
        if refuse_newer_snapshot(&path).is_err() {
            continue;
        }
        if complete_snapshot(&path, session).is_ok() {
            let _ = fs::remove_dir_all(path);
        } else {
            // A failed rename leaves the original backup intact for another attempt.
            let _ = preserve_damaged_snapshot(&path, session);
        }
    }
}

/// A complete snapshot is safe to publish and to discard older backups for.
fn complete_snapshot(dir: &Path, session: &str) -> io::Result<SnapshotMeta> {
    let meta = read_restorable_meta(dir, session)?;
    for pane in &meta.panes {
        let file = fs::File::open(replay_file(dir, pane.pane_id))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                Unrestorable::Damaged,
            ));
        }
    }
    Ok(meta)
}

fn refuse_newer_snapshot(dir: &Path) -> io::Result<()> {
    if let Some(header) = fs::read(dir.join("meta.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SnapshotHeader>(&bytes).ok())
        && header.version > SNAPSHOT_VERSION
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            Unrestorable::Format(header.version),
        ));
    }
    Ok(())
}

/// Keep the published copy usable if this server dies before its next save. Snapshot files are
/// immutable, so hard links preserve evidence without duplicating all of the retained history.
fn copy_damaged_snapshot(path: &Path, session: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("snapshot has no parent"))?;
    let suffix = format!(
        "{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    copy_snapshot_files(
        path,
        &parent.join(staging_dir_name(session, STAGING_DAMAGED, &suffix)),
    )?;
    sync_directory(parent)
}

fn copy_snapshot_files(source: &Path, target: &Path) -> io::Result<()> {
    crate::platform::fs_security::ensure_private_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_snapshot_files(&entry.path(), &destination)?;
        } else if fs::hard_link(entry.path(), &destination).is_err() {
            fs::copy(entry.path(), &destination)?;
        }
    }
    sync_directory(target)
}

fn preserve_damaged_snapshot(path: &Path, session: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("snapshot has no parent"))?;
    let suffix = format!(
        "{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    fs::rename(
        path,
        parent.join(staging_dir_name(session, STAGING_DAMAGED, &suffix)),
    )?;
    sync_directory(parent)
}

/// The part of `meta.json` every snapshot format shares.
#[derive(Deserialize)]
struct SnapshotHeader {
    version: u32,
    session: String,
    #[serde(default)]
    saved_at: u64,
}

/// Why `name` has a snapshot its server would refuse to restore, if it has one. Read-only, so a
/// client can explain a server that would not start; see [`saved_snapshot_summaries`] for why a
/// client never moves a backup.
pub(crate) fn unrestorable_snapshot(name: &str) -> Option<Unrestorable> {
    let server = SessionServer::new_named_with_settings(
        name,
        ServerSettings {
            resurrect: true,
            ..ServerSettings::default()
        },
    );
    let path = server.snapshot_path().ok()?;
    snapshot_recovery_candidate(&path, name)
        .and_then(|candidate| read_restorable_meta(&candidate, name))
        .err()
        .and_then(|err| Unrestorable::of(&err).cloned())
}

/// What to do about session `name`, whose snapshot this build will not restore for `reason`.
pub(crate) fn unrestorable_snapshot_error(name: &str, reason: &Unrestorable) -> io::Error {
    let other_version = if reason.another_version_reads_it() {
        "start it with the rozi version that saved it, or "
    } else {
        ""
    };
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "session `{name}` was not started because {reason}; {other_version}run \
             `rozi sessions new {name} --detach` to start it empty and discard the snapshot"
        ),
    )
}

/// Why a server did not start over the snapshot its restore failed on with `err`.
pub(super) fn restore_refusal(name: &str, err: &io::Error) -> io::Error {
    match Unrestorable::of(err) {
        Some(reason) => unrestorable_snapshot_error(name, reason),
        None => io::Error::new(
            err.kind(),
            format!(
                "session `{name}` was not started because its snapshot could not be read: {err}"
            ),
        ),
    }
}

/// Discard `name`'s snapshot, and only that, so the server started next under the name begins with
/// fresh panes and keeps its deployments. Only for a session whose server has already stopped.
pub(crate) fn discard_snapshot_for_restart(name: &str) -> io::Result<()> {
    SessionServer::new_named_with_settings(
        name,
        ServerSettings {
            resurrect: true,
            ..ServerSettings::default()
        },
    )
    .delete_snapshot()
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // Opening a directory for FlushFileBuffers requires Windows-only handle flags and the flush is
    // not supported consistently across filesystems. Snapshot files themselves are synced before
    // the atomic rename; do not fail an otherwise valid snapshot on this durability enhancement.
    Ok(())
}

fn write_secure(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = crate::platform::persist::create_private_file(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The command this pane's shell is running right now, if it is worth writing down. Shares its
/// judgment with profile capture - see
/// [`replayable_foreground_command`](crate::pane::launch::replayable_foreground_command), which
/// explains why a pane at a prompt reports nothing here even though it still names a program.
fn observed_foreground_command(pane: &ServerPane) -> Option<String> {
    crate::pane::launch::replayable_foreground_command(crate::pane::launch::ForegroundSnapshot {
        command_phase: pane.runtime.command_phase,
        program: pane.runtime.foreground_program.as_deref(),
        executable: pane.runtime.foreground_executable.as_deref(),
        arguments: &pane.runtime.foreground_arguments,
        remote: pane.runtime.cwd_host.is_some(),
        shell: pane.runtime.foreground_shell,
    })
}

fn default_snapshot_dir() -> Option<PathBuf> {
    crate::platform::paths::state_dir_if_available(
        &crate::platform::paths::PlatformEnv::from_process(),
    )
    .map(|dir| dir.join("sessions"))
}

/// Discovery metadata for the snapshot the owning server would select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub session: String,
    pub saved_at: u64,
    pub origin: crate::session::origin::SessionOrigin,
    pub panes: usize,
    /// Whether this build can restore the snapshot. One it cannot, such as one saved by a build
    /// with another snapshot format, is still listed: it is kept for the build that can read it.
    pub readable: bool,
}

/// Every session that can come back, most recent first.
///
/// A server can crash before it has saved anything, and a session with nothing in it may never have
/// anything to save. What is deployed to one is still on disk, and still trusted code, so it is
/// listed with no panes until someone reopens or deletes it.
pub fn list_snapshot_summaries_by_recency() -> Vec<SnapshotSummary> {
    let mut summaries = saved_snapshot_summaries();
    for (session, deployed_at_unix_ms) in super::deployments::sessions_with_deployments() {
        if summaries.iter().any(|summary| summary.session == session) {
            continue;
        }
        summaries.push(SnapshotSummary {
            session,
            saved_at: deployed_at_unix_ms / 1000,
            origin: crate::session::origin::SessionOrigin::default(),
            panes: 0,
            readable: true,
        });
    }
    summaries.sort_by(|a, b| {
        b.saved_at
            .cmp(&a.saved_at)
            .then_with(|| a.session.cmp(&b.session))
    });
    summaries
}

fn saved_snapshot_summaries() -> Vec<SnapshotSummary> {
    let Some(root) = default_snapshot_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut sessions: std::collections::BTreeSet<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (crate::session::discovery::valid_session_name(&name) && entry.path().is_dir())
                .then_some(name)
        })
        .collect();
    sessions.extend(backup_sessions(&root));
    sessions
        .into_iter()
        .filter_map(|session| snapshot_summary(&root.join(&session), session))
        .collect()
}

/// The summary of the snapshot in `dir`, published or backed up as `session`. A snapshot this
/// build cannot read is summarized from what every format shares, as unreadable; a directory
/// without `meta.json` is no snapshot.
fn snapshot_summary(dir: &Path, session: String) -> Option<SnapshotSummary> {
    let candidate =
        snapshot_recovery_candidate(dir, &session).unwrap_or_else(|_| dir.to_path_buf());
    let meta = read_restorable_meta(&candidate, &session);
    match meta {
        Ok(meta) => Some(SnapshotSummary {
            session,
            saved_at: meta.saved_at,
            origin: meta.effective_origin(),
            panes: meta.panes.len(),
            readable: true,
        }),
        Err(err) if err.kind() == io::ErrorKind::InvalidData => Some(SnapshotSummary {
            saved_at: fs::read(candidate.join("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SnapshotHeader>(&bytes).ok())
                .map_or(0, |header| header.saved_at),
            session,
            origin: crate::session::origin::SessionOrigin::default(),
            panes: 0,
            readable: false,
        }),
        Err(_) => None,
    }
}

/// Every session with a backup directory in `parent`, readable or not.
fn backup_sessions(parent: &Path) -> std::collections::BTreeSet<String> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Default::default();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let dir_name = entry.file_name().into_string().ok()?;
            let (session, kind, _) = parse_staging_dir_name(&dir_name)?;
            (kind == STAGING_OLD && crate::session::discovery::valid_session_name(session))
                .then(|| session.to_string())
        })
        .collect()
}

/// Whether `name` has a snapshot this build can restore.
pub(crate) fn has_snapshot(name: &str) -> bool {
    list_snapshot_summaries_by_recency()
        .iter()
        .any(|snapshot| snapshot.session == name && snapshot.readable)
}

/// Publish an empty snapshot for `name` that discovery lists as restorable, for tests elsewhere in
/// the crate that need one without running a server. Remove it with [`delete_snapshot_for`].
#[cfg(test)]
pub(crate) fn write_empty_snapshot_for_test(name: &str) {
    // Each level is created the way rozi creates it, private. Every test in the binary shares one
    // state directory, and on Windows one made with a plain `create_dir_all` lacks the protected
    // DACL private state requires, so whichever test created it first would break every later test
    // that persists anything.
    let state = crate::platform::paths::private_state_dir(
        &crate::platform::paths::PlatformEnv::from_process(),
    )
    .expect("isolated state directory");
    let sessions = default_snapshot_dir().expect("isolated state directory");
    debug_assert!(sessions.starts_with(&state));
    crate::platform::fs_security::ensure_private_dir(&sessions).expect("create sessions directory");
    let dir = sessions.join(name);
    crate::platform::fs_security::ensure_private_dir(&dir).expect("create snapshot directory");
    let meta = SnapshotMeta {
        version: SNAPSHOT_VERSION,
        session: name.to_string(),
        saved_at: 0,
        layout_rev: 0,
        origin: Default::default(),
        created_from_profile: None,
        panes: Vec::new(),
    };
    fs::write(
        dir.join("meta.json"),
        serde_json::to_vec(&meta).expect("serialize snapshot"),
    )
    .expect("write snapshot");
}

pub(crate) fn list_snapshot_names_by_recency() -> Vec<String> {
    list_snapshot_summaries_by_recency()
        .into_iter()
        .map(|snapshot| snapshot.session)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_compatibility_snapshot(root: &Path) {
        let mut server = integrity_server(root);
        for (id, launch) in [
            (1, None),
            (
                2,
                Some(crate::pane::launch::PaneLaunch::shell(
                    "printf '%s' 'hello world'",
                )),
            ),
            (
                3,
                Some(
                    crate::pane::launch::PaneLaunch::direct(vec![
                        "example".into(),
                        "two words".into(),
                        "$literal".into(),
                    ])
                    .unwrap(),
                ),
            ),
        ] {
            let mut pane = running("example", &[], protocol::PaneCommandPhase::Executing);
            pane.launch = launch;
            server.panes.insert(id, pane);
        }
        let mut job = server.capture_snapshot(Instant::now()).unwrap();
        job.meta.saved_at = 1_700_000_000;
        write_snapshot_job(job).unwrap();
    }

    /// Run explicitly to refresh the scrubbed corpus through the production capture and writer.
    #[test]
    #[ignore = "writes the checked-in snapshot fixture"]
    fn generate_snapshot_v3_fixture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snapshots/v3");
        write_compatibility_snapshot(&root);
    }

    #[test]
    fn current_writer_matches_snapshot_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshots");
        write_compatibility_snapshot(&root);
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snapshots/v3/integrity");
        let actual = root.join("integrity");
        for relative in [
            "meta.json",
            "panes/1.replay",
            "panes/2.replay",
            "panes/3.replay",
        ] {
            assert_eq!(
                fs::read(actual.join(relative)).unwrap(),
                fs::read(fixture.join(relative)).unwrap(),
                "{relative}"
            );
        }
    }

    #[test]
    fn snapshot_fixtures_read_migrate_and_list_without_changing_files() {
        for version in OLDEST_READABLE_SNAPSHOT_VERSION..=SNAPSHOT_VERSION {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/snapshots/v{version}/integrity"));
            let before = snapshot_tree(&fixture);
            let meta = complete_snapshot(&fixture, "integrity").unwrap();
            assert_eq!(meta.version, SNAPSHOT_VERSION);
            assert_eq!(meta.panes.len(), 3);
            assert_eq!(
                meta.panes[0].foreground.as_deref(),
                (version == 3).then_some("example")
            );
            assert!(meta.panes[1..].iter().all(|pane| pane.foreground.is_none()));
            assert_eq!(meta.panes[0].launch, None);
            assert_eq!(
                meta.panes[1].launch,
                Some(crate::pane::launch::PaneLaunch::shell(
                    "printf '%s' 'hello world'"
                ))
            );
            assert_eq!(
                meta.panes[2].launch,
                Some(
                    crate::pane::launch::PaneLaunch::direct(vec![
                        "example".into(),
                        "two words".into(),
                        "$literal".into()
                    ])
                    .unwrap()
                )
            );
            let summary = snapshot_summary(&fixture, "integrity".into()).unwrap();
            assert!(summary.readable);
            assert_eq!(summary.panes, 3);
            assert_eq!(snapshot_tree(&fixture), before);

            // A backup in the oldest format must also be offered and recovered by the owner.
            let temp = tempfile::tempdir().unwrap();
            let backup = temp.path().join(".integrity.old-1.1");
            copy_snapshot_files(&fixture, &backup).unwrap();
            assert_eq!(valid_backups(temp.path()).len(), 1);
            let server = integrity_server(temp.path());
            server.recover_interrupted_snapshot().unwrap();
            assert!(!backup.exists());
            let recovered = temp.path().join("integrity");
            assert_eq!(
                fs::read(recovered.join("meta.json")).unwrap(),
                fs::read(fixture.join("meta.json")).unwrap()
            );
        }
    }

    #[test]
    fn snapshot_window_refusals_preserve_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("integrity");
        for version in [OLDEST_READABLE_SNAPSHOT_VERSION - 1, SNAPSHOT_VERSION + 1] {
            write_meta(&dir, "integrity", version, 1);
            let before = snapshot_tree(&dir);
            let err = read_restorable_meta(&dir, "integrity").err().unwrap();
            assert_eq!(Unrestorable::of(&err), Some(&Unrestorable::Format(version)));
            assert!(err.to_string().contains("reads formats 2 through 3"));
            assert_eq!(snapshot_tree(&dir), before);
        }
        for document in [
            serde_json::json!({"version": 2, "session": "integrity", "panes": [42]}),
            serde_json::json!({"version": 2, "session": "integrity", "panes": {}}),
        ] {
            fs::write(
                dir.join("meta.json"),
                serde_json::to_vec(&document).unwrap(),
            )
            .unwrap();
            let before = snapshot_tree(&dir);
            let err = read_restorable_meta(&dir, "integrity").err().unwrap();
            assert_eq!(Unrestorable::of(&err), Some(&Unrestorable::Damaged));
            assert_eq!(snapshot_tree(&dir), before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_writes_are_private_and_do_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("replay");
        write_secure(&path, b"private replay").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(write_secure(&link, b"overwrite").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"private replay");
    }

    #[test]
    fn snapshot_without_profile_origin_remains_readable() {
        let meta: SnapshotMeta = serde_json::from_value(serde_json::json!({
            "version": SNAPSHOT_VERSION,
            "session": "dev",
            "saved_at": 0,
            "layout_rev": 0,
            "panes": []
        }))
        .unwrap();

        assert!(meta.effective_origin().is_empty());
    }

    #[test]
    fn legacy_profile_origin_is_read_from_snapshot_metadata() {
        let meta: SnapshotMeta = serde_json::from_value(serde_json::json!({
            "version": SNAPSHOT_VERSION,
            "session": "dev",
            "saved_at": 0,
            "layout_rev": 0,
            "created_from_profile": "work",
            "panes": []
        }))
        .unwrap();
        assert_eq!(meta.effective_origin().profile.as_deref(), Some("work"));
    }

    /// A snapshot write killed before its rename leaves `.{session}.tmp-…` behind, complete with
    /// metadata. Listing that as restorable produced a row nothing could clear: `restore` found no
    /// snapshot and `forget` deleted `<root>/<session>`, which never existed.
    #[test]
    fn an_abandoned_staging_directory_is_neither_restorable_nor_left_behind_by_forget() {
        let root = default_snapshot_dir().expect("test scratch state dir");
        let published = "staging-published";
        let abandoned = "staging-abandoned";
        let staging = root.join(format!(".{abandoned}.tmp-4242.99"));
        for (dir, session) in [
            (root.join(published), published),
            (staging.clone(), abandoned),
        ] {
            fs::create_dir_all(&dir).expect("create snapshot dir");
            fs::write(
                dir.join("meta.json"),
                serde_json::to_vec(&SnapshotMeta {
                    version: SNAPSHOT_VERSION,
                    session: session.to_string(),
                    saved_at: 1,
                    layout_rev: 0,
                    origin: crate::session::origin::SessionOrigin {
                        profile: Some("work".to_string()),
                        ..Default::default()
                    },
                    created_from_profile: None,
                    panes: Vec::new(),
                })
                .unwrap(),
            )
            .expect("write snapshot meta");
        }

        let summaries = list_snapshot_summaries_by_recency();
        let summary = summaries
            .iter()
            .find(|summary| summary.session == published)
            .expect("published snapshot summary");
        assert_eq!(summary.origin.profile.as_deref(), Some("work"));
        assert_eq!(summary.panes, 0);
        assert_eq!(summary.saved_at, 1);
        let listed = list_snapshot_names_by_recency();
        assert!(
            listed.iter().any(|name| name == published),
            "a published snapshot is restorable\n{listed:?}"
        );
        assert!(
            !listed.iter().any(|name| name == abandoned),
            "a staging leftover must not be offered as restorable\n{listed:?}"
        );

        delete_snapshot_for(abandoned).expect("forget the abandoned session");
        assert!(!staging.exists(), "forget sweeps the staging leftover");

        delete_snapshot_for(published).expect("forget the published session");
        assert!(!root.join(published).exists());
    }

    fn write_meta(dir: &Path, session: &str, version: u32, saved_at: u64) {
        fs::create_dir_all(dir).expect("create snapshot dir");
        fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(&SnapshotMeta {
                version,
                session: session.to_string(),
                saved_at,
                layout_rev: 0,
                origin: Default::default(),
                created_from_profile: None,
                panes: Vec::new(),
            })
            .unwrap(),
        )
        .expect("write snapshot meta");
    }

    /// A session caught between a snapshot's two renames lives only in its backup. Discovery must
    /// still offer it, without touching the directory a live writer may be about to rename, and
    /// forget must clear it.
    #[test]
    fn a_session_left_only_in_its_backup_is_listed_read_only_and_forgettable() {
        let root = default_snapshot_dir().expect("test scratch state dir");
        let session = "backup-only";
        let backup = root.join(staging_dir_name(session, STAGING_OLD, "4242.200"));
        let older = root.join(staging_dir_name(session, STAGING_OLD, "4242.100"));
        write_meta(&backup, session, SNAPSHOT_VERSION, 7);
        write_meta(&older, session, SNAPSHOT_VERSION, 3);
        // Neither of these makes a session restorable: a backup of another build's snapshot
        // format, and one whose metadata names a different session than its directory.
        let foreign = "backup-foreign";
        write_meta(
            &root.join(staging_dir_name(foreign, STAGING_OLD, "1.1")),
            foreign,
            SNAPSHOT_VERSION + 1,
            9,
        );
        let mismatched = "backup-mismatch";
        write_meta(
            &root.join(staging_dir_name(mismatched, STAGING_OLD, "1.1")),
            session,
            SNAPSHOT_VERSION,
            9,
        );

        let summaries = list_snapshot_summaries_by_recency();
        let listed: Vec<_> = summaries
            .iter()
            .filter(|summary| summary.session == session)
            .collect();
        assert_eq!(listed.len(), 1, "one row per session\n{summaries:?}");
        assert_eq!(listed[0].saved_at, 7, "the newest backup describes it");
        assert!(has_snapshot(session));
        assert!(!has_snapshot(foreign));
        assert!(!has_snapshot(mismatched));
        // Neither can be restored here, but both are still there, kept for a build that can read
        // them, so both are listed as unreadable rather than hidden.
        for name in [foreign, mismatched] {
            let row = summaries
                .iter()
                .find(|summary| summary.session == name)
                .unwrap_or_else(|| panic!("{name} is listed\n{summaries:?}"));
            assert!(!row.readable, "{name} is listed as unreadable");
        }
        assert!(listed[0].readable);
        // A client can say why a server would refuse the foreign one, and has no reason to give
        // for one it can restore.
        assert_eq!(
            unrestorable_snapshot(foreign),
            Some(Unrestorable::Format(SNAPSHOT_VERSION + 1))
        );
        assert_eq!(unrestorable_snapshot(session), None);
        assert!(
            backup.exists() && older.exists() && !root.join(session).exists(),
            "discovery never moves or removes a backup"
        );

        // With the published snapshot back, it is the one listed and the backup adds nothing.
        write_meta(&root.join(session), session, SNAPSHOT_VERSION, 11);
        let summaries = list_snapshot_summaries_by_recency();
        let listed: Vec<_> = summaries
            .iter()
            .filter(|summary| summary.session == session)
            .collect();
        assert_eq!(listed.len(), 1, "{summaries:?}");
        assert_eq!(listed[0].saved_at, 11);

        for name in [session, foreign, mismatched] {
            delete_snapshot_for(name).expect("forget");
            assert!(!has_snapshot(name));
        }
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(".backup-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// The writer can also die after publishing the replacement but before removing the backup.
    /// The published snapshot is the one restored, and the backup is swept rather than kept.
    #[test]
    fn recovery_keeps_a_published_snapshot_and_sweeps_the_backup_left_beside_it() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-leftover-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let server = SessionServer::new_named_with_settings(
            "leftover",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                ..ServerSettings::default()
            },
        );
        write_meta(&root.join("leftover"), "leftover", SNAPSHOT_VERSION, 5);
        let backup = root.join(staging_dir_name("leftover", STAGING_OLD, "1.2"));
        write_meta(&backup, "leftover", SNAPSHOT_VERSION, 9);
        let stale_version = root.join(staging_dir_name("other", STAGING_OLD, "1.2"));
        write_meta(&stale_version, "other", SNAPSHOT_VERSION - 1, 9);

        server.recover_interrupted_snapshot().unwrap();

        let meta: SnapshotMeta =
            serde_json::from_slice(&fs::read(root.join("leftover/meta.json")).unwrap()).unwrap();
        assert_eq!(meta.saved_at, 5, "a published snapshot is never replaced");
        assert!(!backup.exists());
        assert!(stale_version.exists(), "only this session's debris goes");
        let _ = fs::remove_dir_all(root);
    }

    /// A published snapshot that lost its `meta.json` is damaged, not absent: it is listed as
    /// unreadable, restore refuses it, and what is left of it stays where it is.
    #[test]
    fn a_snapshot_missing_its_metadata_is_unreadable_rather_than_absent() {
        let root = default_snapshot_dir().expect("test scratch state dir");
        let session = "meta-missing";
        let replay = root.join(session).join("panes").join("1.replay");
        fs::create_dir_all(replay.parent().unwrap()).unwrap();
        fs::write(&replay, b"kept").unwrap();

        let row = list_snapshot_summaries_by_recency()
            .into_iter()
            .find(|summary| summary.session == session)
            .expect("listed");
        assert!(!row.readable);
        assert!(!has_snapshot(session));
        assert_eq!(unrestorable_snapshot(session), Some(Unrestorable::Damaged));
        let mut server = SessionServer::new_named_with_settings(
            session,
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let err = server.restore().expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&replay).unwrap(), b"kept");

        delete_snapshot_for(session).expect("forget");
        assert!(unrestorable_snapshot(session).is_none());
        assert!(
            server.restore().expect_err("nothing left").kind() == io::ErrorKind::NotFound,
            "with the directory gone there is no snapshot"
        );
    }

    /// The same holds for a backup that lost its `meta.json`: a backup was once a published
    /// snapshot, so it is damaged rather than absent, and recovery publishes it for restore to
    /// refuse instead of a server starting empty beside it.
    #[test]
    fn a_backup_missing_its_metadata_is_unreadable_rather_than_absent() {
        let root = default_snapshot_dir().expect("test scratch state dir");
        let session = "backup-meta-missing";
        let backup = root.join(staging_dir_name(session, STAGING_OLD, "4242.1"));
        fs::create_dir_all(backup.join("panes")).unwrap();
        fs::write(backup.join("panes").join("1.replay"), b"kept").unwrap();

        let row = list_snapshot_summaries_by_recency()
            .into_iter()
            .find(|summary| summary.session == session)
            .expect("listed");
        assert!(!row.readable);
        assert_eq!(unrestorable_snapshot(session), Some(Unrestorable::Damaged));
        assert!(backup.exists(), "discovery never moves a backup");

        let mut server = SessionServer::new_named_with_settings(
            session,
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let err = server.restore().expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(root.join(session).join("panes").join("1.replay")).unwrap(),
            b"kept",
            "recovery published what was left"
        );

        delete_snapshot_for(session).expect("forget");
        assert!(unrestorable_snapshot(session).is_none());
    }

    /// A newer-format backup stays untouched when this build cannot restore it.
    #[test]
    fn recovery_keeps_a_backup_of_another_snapshot_version() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-version-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "versioned",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                ..ServerSettings::default()
            },
        );
        write_meta(
            &root.join(staging_dir_name("versioned", STAGING_OLD, "1.2")),
            "versioned",
            SNAPSHOT_VERSION + 1,
            9,
        );
        let abandoned = root.join(staging_dir_name("versioned", STAGING_TMP, "1.3"));
        fs::create_dir_all(&abandoned).unwrap();
        let before = snapshot_tree(&root);
        assert!(server.restore().is_err());
        assert_eq!(
            snapshot_tree(&root),
            before,
            "newer backup and staging stay untouched"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn integrity_server(root: &Path) -> SessionServer {
        SessionServer::new_named_with_settings(
            "integrity",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.to_path_buf()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        )
    }

    fn snapshot_tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut tree = std::collections::BTreeMap::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                tree.insert(path.clone(), Vec::new());
                tree.extend(snapshot_tree(&path));
            } else {
                tree.insert(path.clone(), fs::read(path).unwrap());
            }
        }
        tree
    }

    #[test]
    fn newer_canonical_is_untouched_by_restore_and_later_save() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshots");
        crate::platform::fs_security::ensure_private_dir(&root).unwrap();
        let mut server = integrity_server(root.as_path());
        write_meta(
            &root.as_path().join("integrity"),
            "integrity",
            SNAPSHOT_VERSION + 1,
            8,
        );
        write_meta(
            &root.as_path().join(".integrity.old-1.2"),
            "integrity",
            SNAPSHOT_VERSION,
            7,
        );
        fs::create_dir(root.as_path().join(".integrity.tmp-1.3")).unwrap();
        let before = snapshot_tree(root.as_path());
        let err = server.restore().unwrap_err();
        assert_eq!(
            Unrestorable::of(&err),
            Some(&Unrestorable::Format(SNAPSHOT_VERSION + 1))
        );
        assert!(
            restore_refusal("integrity", &err)
                .to_string()
                .contains("format")
        );
        server.mark_dirty();
        server.maybe_snapshot().unwrap();
        assert!(server.wait_for_snapshots().is_err());
        assert_eq!(snapshot_tree(root.as_path()), before);
    }

    #[test]
    fn corrupt_canonical_recovers_newest_complete_backup_and_keeps_damage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshots");
        crate::platform::fs_security::ensure_private_dir(&root).unwrap();
        let mut server = integrity_server(root.as_path());
        let canonical = root.as_path().join("integrity");
        fs::create_dir(&canonical).unwrap();
        fs::write(canonical.join("meta.json"), b"corrupt").unwrap();
        write_meta(
            &root.as_path().join(".integrity.old-1.1"),
            "integrity",
            SNAPSHOT_VERSION,
            4,
        );
        write_meta(
            &root.as_path().join(".integrity.old-1.2"),
            "integrity",
            SNAPSHOT_VERSION,
            7,
        );
        // A newer backup missing a replay must not displace the complete one.
        let mut job = server.capture_snapshot(Instant::now()).unwrap();
        job.meta.saved_at = 9;
        job.meta.panes.push(SnapshotPane {
            pane_id: 1,
            generation: 1,
            scrollback: None,
            launch: None,
            foreground: None,
            agent_resume: None,
            cwd: None,
            keep_open: false,
            title: None,
            palette: test_pane().palette,
            cols: 40,
            rows: 5,
        });
        let mut remaining_pane: SnapshotPane =
            serde_json::from_value(serde_json::to_value(&job.meta.panes[0]).unwrap()).unwrap();
        remaining_pane.pane_id = 2;
        job.meta.panes.push(remaining_pane);
        let incomplete = root.as_path().join(".integrity.old-1.3");
        fs::create_dir_all(incomplete.join("panes")).unwrap();
        fs::write(replay_file(&incomplete, 2), b"newer surviving history").unwrap();
        fs::write(
            incomplete.join("meta.json"),
            serde_json::to_vec(&job.meta).unwrap(),
        )
        .unwrap();
        let future = root.join(".integrity.old-1.4");
        write_meta(&future, "integrity", SNAPSHOT_VERSION + 1, 10);
        let future_before = snapshot_tree(&future);
        let summary = snapshot_summary(&canonical, "integrity".into()).unwrap();
        assert!(summary.readable);
        assert_eq!(summary.saved_at, 7);
        assert_eq!(
            fs::read(canonical.join("meta.json")).unwrap(),
            b"corrupt",
            "discovery is read-only"
        );
        assert_eq!(server.restore().unwrap(), 0);
        assert_eq!(
            read_restorable_meta(&canonical, "integrity")
                .unwrap()
                .saved_at,
            7
        );
        assert_eq!(snapshot_tree(&future), future_before);
        assert!(
            future.exists(),
            "a newer-format backup must remain untouched"
        );
        let damaged_incomplete = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".integrity.damaged-")
                    && read_restorable_meta(&entry.path(), "integrity")
                        .is_ok_and(|meta| meta.saved_at == 9)
            })
            .expect("newer incomplete backup was preserved")
            .path();
        assert_eq!(
            fs::read(damaged_incomplete.join("meta.json")).unwrap(),
            serde_json::to_vec(&job.meta).unwrap()
        );
        assert_eq!(
            fs::read(replay_file(&damaged_incomplete, 2)).unwrap(),
            b"newer surviving history"
        );
        assert!(
            server
                .notifications
                .back()
                .unwrap()
                .message
                .contains("backup")
        );
        let damaged = fs::read_dir(root.as_path())
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".integrity.damaged-")
                    && fs::read(entry.path().join("meta.json"))
                        .is_ok_and(|bytes| bytes == b"corrupt")
            })
            .unwrap()
            .path();
        assert_eq!(fs::read(damaged.join("meta.json")).unwrap(), b"corrupt");
        server.recover_interrupted_snapshot().unwrap();
        assert!(damaged.exists());
        assert!(damaged_incomplete.exists());
        assert_eq!(
            snapshot_tree(&future),
            future_before,
            "cleanup beside a complete canonical also preserves newer backups"
        );
        server.delete_snapshot().unwrap();
        assert!(fs::read_dir(root.as_path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_selects_the_same_backup_as_restore_for_damaged_replays() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshots");
        let mut server = integrity_server(&root);
        server.settings.shell = vec!["/bin/sh".into()];
        server.panes.insert(2, test_pane());
        server.origin.profile = Some("backup-profile".into());
        let mut backup_job = server.capture_snapshot(Instant::now()).unwrap();
        backup_job.meta.saved_at = 9;
        write_snapshot_job(backup_job).unwrap();
        let canonical = root.join("integrity");
        let backup = root.join(".integrity.old-1.1");
        fs::rename(&canonical, &backup).unwrap();
        server.panes.insert(1, test_pane());
        server.origin.profile = Some("canonical-profile".into());
        let mut job = server.capture_snapshot(Instant::now()).unwrap();
        job.meta.saved_at = 10;
        write_snapshot_job(job).unwrap();
        fs::remove_file(replay_file(&canonical, 1)).unwrap();
        let before = snapshot_tree(&root);
        let summary = snapshot_summary(&canonical, "integrity".into()).unwrap();
        assert!(summary.readable);
        assert_eq!(summary.saved_at, 9);
        assert_eq!(summary.panes, 1);
        assert_eq!(summary.origin.profile.as_deref(), Some("backup-profile"));
        assert_eq!(
            snapshot_tree(&root),
            before,
            "candidate selection is read-only"
        );
        server.panes.clear();
        assert_eq!(server.restore().unwrap(), summary.panes);
        assert_eq!(server.origin, summary.origin);
        assert_eq!(
            read_restorable_meta(&canonical, "integrity")
                .unwrap()
                .saved_at,
            summary.saved_at
        );
        server.panes.clear();
    }

    #[cfg(unix)]
    #[test]
    fn damaged_history_and_layout_restore_remaining_content_with_notices() {
        for (broken_layout, unreadable_replay) in [(false, false), (true, false), (false, true)] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("snapshots");
            crate::platform::fs_security::ensure_private_dir(&root).unwrap();
            let mut server = integrity_server(root.as_path());
            server.settings.shell = vec!["/bin/sh".into()];
            for id in [1, 2] {
                let mut pane = test_pane();
                pane.title = Some(format!("editor-{id}"));
                pane.screen_mut()
                    .process_bytes(format!("history-{id}").as_bytes());
                server.panes.insert(id, pane);
            }
            server.layout = Some(super::super::tests::shared_layout_for_panes(&[
                (1, 1),
                (2, 1),
            ]));
            server.layout_rev = 1;
            server.mark_dirty();
            server.maybe_snapshot().unwrap();
            server.wait_for_snapshots().unwrap();
            let canonical = root.as_path().join("integrity");
            fs::remove_file(replay_file(&canonical, 1)).unwrap();
            if unreadable_replay {
                fs::create_dir(replay_file(&canonical, 1)).unwrap();
            }
            if broken_layout {
                fs::write(canonical.join("layout.json"), b"broken").unwrap();
            }
            server.panes.clear();
            assert_eq!(server.restore().unwrap(), 2);
            assert!(
                server.snapshot_dirty(),
                "partial recovery schedules a healing snapshot"
            );
            assert!(
                canonical.exists(),
                "a crash before the next save can still restore the published snapshot"
            );
            assert_eq!(server.layout.is_some(), !broken_layout);
            assert!(
                !server
                    .panes
                    .get_mut(&1)
                    .unwrap()
                    .screen_without_change()
                    .render_snapshot()
                    .text
                    .contains("history-1")
            );
            assert!(
                server
                    .panes
                    .get_mut(&2)
                    .unwrap()
                    .screen_without_change()
                    .render_snapshot()
                    .text
                    .contains("history-2")
            );
            assert!(
                server
                    .notifications
                    .iter()
                    .any(|notice| notice.message.contains("editor-1")
                        && notice.message.contains("empty history"))
            );
            assert_eq!(
                server.notifications.len(),
                if broken_layout { 2 } else { 1 }
            );
            let (client, _stream) = super::super::tests::add_client(&mut server);
            server.handle_message(
                client,
                protocol::ClientMessage::Attach {
                    capabilities: None,
                    session: "integrity".into(),
                    protocol_version: PROTOCOL_VERSION,
                    min_protocol_version: PROTOCOL_VERSION,
                    label: "restore-test".into(),
                    read_only: false,
                    shares_filesystem: true,
                    expected_server_nonce: None,
                    handoff_token: None,
                },
            );
            let messages =
                super::super::tests::decode_outbox_controls(server.client(client).unwrap());
            assert!(messages.iter().any(|message| matches!(message,
                ServerMessage::SessionNotifications { notifications, .. }
                    if notifications.iter().any(|notice| notice.message.contains("editor-1"))
            )));
            let damaged = fs::read_dir(root.as_path())
                .unwrap()
                .flatten()
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".integrity.damaged-")
                })
                .unwrap()
                .path();
            assert!(replay_file(&damaged, 2).exists());
            server.snapshot_before_shutdown().unwrap();
            assert!(!server.snapshot_dirty());
            assert!(complete_snapshot(&canonical, "integrity").is_ok());
            server.recover_interrupted_snapshot().unwrap();
            assert!(
                damaged.exists(),
                "periodic writes and startup sweeps keep evidence"
            );
            server.delete_snapshot().unwrap();
            assert!(fs::read_dir(root.as_path()).unwrap().next().is_none());
        }
    }

    #[test]
    fn shutdown_forces_a_dirty_snapshot_before_the_interval() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-shutdown-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "shutdown",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::from_secs(60 * 60),
                ..ServerSettings::default()
            },
        );
        server.mark_dirty();

        server
            .maybe_snapshot()
            .expect("ordinary interval check should be harmless");
        assert!(!root.join("shutdown/meta.json").exists());

        server
            .snapshot_before_shutdown()
            .expect("shutdown snapshot");
        assert!(root.join("shutdown/meta.json").is_file());
        assert!(!server.snapshot_dirty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_metrics_record_complete_success_and_failure_attempts() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-metrics-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "metrics",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        server.mark_dirty();
        server
            .maybe_snapshot()
            .expect("dispatch successful snapshot");
        server.wait_for_snapshots().expect("successful snapshot");
        assert_eq!(server.resurrection_metrics.attempts, 1);
        assert_eq!(server.resurrection_metrics.successes, 1);
        assert_eq!(server.resurrection_metrics.failures, 0);
        assert_eq!(
            server.resurrection_metrics.max_duration_us,
            server.resurrection_metrics.last_duration_us
        );
        // The whole point of the split: the loop is blocked for less than the full attempt.
        assert!(
            server.resurrection_metrics.last_blocking_us
                <= server.resurrection_metrics.last_duration_us
        );
        assert!(!server.snapshot_dirty(), "a clean snapshot clears the need");

        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"x").unwrap();
        server.settings.snapshot_dir = Some(blocked);
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch failing snapshot");
        assert!(server.wait_for_snapshots().is_err());
        assert_eq!(server.resurrection_metrics.attempts, 2);
        assert_eq!(server.resurrection_metrics.successes, 1);
        assert_eq!(server.resurrection_metrics.failures, 1);
        assert!(
            server.resurrection_metrics.max_duration_us
                >= server.resurrection_metrics.last_duration_us
        );
        assert!(
            server.snapshot_dirty(),
            "a failed snapshot stays dirty for a later retry"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// An idle pane reuses its replay file; a pane that changed is re-exported.
    ///
    /// The reuse path is the one that can silently lose scrollback, so this asserts both halves:
    /// that an untouched pane keeps the bytes it already had, and that a touched pane's new
    /// output actually reaches the file.
    #[test]
    fn unchanged_panes_reuse_their_replay_and_changed_panes_do_not() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-reuse-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "reuse",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        for pane_id in [1, 2] {
            let mut pane = test_pane();
            pane.screen_mut()
                .process_bytes(format!("pane-{pane_id}-original\r\n").as_bytes());
            server.panes.insert(pane_id, pane);
        }
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch first snapshot");
        server.wait_for_snapshots().expect("first snapshot");

        let replay = |pane_id: PaneId| {
            fs::read(root.join(format!("reuse/panes/{pane_id}.replay"))).expect("replay file")
        };
        let idle_before = replay(2);
        assert!(contains(&idle_before, b"pane-2-original"));

        // Only pane 1 changes.
        server
            .panes
            .get_mut(&1)
            .expect("pane 1")
            .screen_mut()
            .process_bytes(b"pane-1-appended\r\n");
        server.mark_dirty();
        assert!(
            matches!(
                server
                    .capture_snapshot(Instant::now())
                    .expect("capture")
                    .replays
                    .iter()
                    .find(|(id, _)| *id == 2)
                    .map(|(_, source)| source),
                Some(ReplaySource::Reuse)
            ),
            "an untouched pane must not be re-exported"
        );

        server.maybe_snapshot().expect("dispatch second snapshot");
        server.wait_for_snapshots().expect("second snapshot");

        assert_eq!(
            replay(2),
            idle_before,
            "reused replay must be byte-identical"
        );
        let changed = replay(1);
        assert!(
            contains(&changed, b"pane-1-appended"),
            "a changed pane must be re-exported with its new output"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Reuse must survive successive generations, and must not depend on the *oldest* snapshot
    /// directory surviving.
    ///
    /// Each snapshot publishes a new directory and unlinks the one it replaced. A reused file is
    /// carried forward by a hard link, so the bytes outlive every pathname they were ever
    /// published under - this test is what documents that as intentional rather than luck.
    #[test]
    fn reuse_chains_across_generations_and_outlives_the_directory_it_came_from() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-chain-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "chain",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        for pane_id in 1..=3 {
            let mut pane = test_pane();
            pane.screen_mut()
                .process_bytes(format!("pane-{pane_id}-a\r\n").as_bytes());
            server.panes.insert(pane_id, pane);
        }

        // A: everything exported.
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch A");
        server.wait_for_snapshots().expect("snapshot A");
        assert_eq!(server.resurrection_metrics.last_exported_panes, 3);
        assert_eq!(server.resurrection_metrics.last_reused_panes, 0);

        // B: pane 1 dirty, 2 and 3 reused.
        server
            .panes
            .get_mut(&1)
            .expect("pane 1")
            .screen_mut()
            .process_bytes(b"pane-1-b\r\n");
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch B");
        server.wait_for_snapshots().expect("snapshot B");
        assert_eq!(server.resurrection_metrics.last_exported_panes, 1);
        assert_eq!(server.resurrection_metrics.last_reused_panes, 2);

        // C: pane 2 dirty, 1 and 3 reused. Pane 3's bytes have now been carried through two
        // directories that no longer exist.
        server
            .panes
            .get_mut(&2)
            .expect("pane 2")
            .screen_mut()
            .process_bytes(b"pane-2-c\r\n");
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch C");
        server.wait_for_snapshots().expect("snapshot C");
        assert_eq!(server.resurrection_metrics.last_exported_panes, 1);
        assert_eq!(server.resurrection_metrics.last_reused_panes, 2);

        let replay = |pane_id: PaneId| {
            fs::read(root.join(format!("chain/panes/{pane_id}.replay"))).expect("replay file")
        };
        assert!(contains(&replay(1), b"pane-1-b"));
        assert!(contains(&replay(2), b"pane-2-c"));
        assert!(
            contains(&replay(3), b"pane-3-a"),
            "a pane untouched across every generation keeps its original bytes"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// A snapshot directory emptied behind the server's back re-exports rather than failing.
    #[test]
    fn a_missing_reuse_source_falls_back_to_export_in_the_same_attempt() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "missing",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        let mut pane = test_pane();
        pane.screen_mut().process_bytes(b"only-copy\r\n");
        server.panes.insert(1, pane);
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch first snapshot");
        server.wait_for_snapshots().expect("first snapshot");
        assert_eq!(server.persisted_replays.get(&1), Some(&1));

        // The reuse claim now points at a file that is gone.
        fs::remove_file(root.join("missing/panes/1.replay")).expect("remove replay");
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch second snapshot");
        server
            .wait_for_snapshots()
            .expect("a missing source must not fail the snapshot");
        assert_eq!(
            server.resurrection_metrics.last_exported_panes, 1,
            "the pane must be re-exported rather than linked from nothing"
        );
        assert_eq!(server.resurrection_metrics.failures, 0);
        assert!(contains(
            &fs::read(root.join("missing/panes/1.replay")).expect("rewritten replay"),
            b"only-copy"
        ));

        let _ = fs::remove_dir_all(root);
    }

    /// A failed snapshot must not leave a reuse pointing at a directory that may not hold the file.
    #[test]
    fn a_failed_snapshot_forces_a_full_export_next_time() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-reuse-heal-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "heal",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(1, test_pane());
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch first snapshot");
        server.wait_for_snapshots().expect("first snapshot");
        assert_eq!(server.persisted_replays.len(), 1);

        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"x").unwrap();
        server.settings.snapshot_dir = Some(blocked);
        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch failing snapshot");
        assert!(server.wait_for_snapshots().is_err());
        assert!(
            server.persisted_replays.is_empty(),
            "a failure must drop every reuse claim"
        );

        server.settings.snapshot_dir = Some(root.clone());
        assert!(
            server
                .capture_snapshot(Instant::now())
                .expect("capture")
                .replays
                .iter()
                .all(|(_, source)| matches!(source, ReplaySource::Exported(_))),
            "the attempt after a failure must export everything"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// A pane whose runtime state says it is running `program args` right now.
    fn running(program: &str, arguments: &[&str], phase: protocol::PaneCommandPhase) -> ServerPane {
        let mut pane = test_pane();
        pane.runtime.command_phase = phase;
        pane.runtime.foreground_program = Some(program.to_string());
        pane.runtime.foreground_arguments = arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect();
        pane
    }

    /// The whole point of capturing a foreground command: a shell someone typed an agent into is
    /// not a shell, and restoring it as one loses the session's actual contents.
    #[test]
    fn a_shell_running_a_command_captures_the_whole_invocation() {
        let pane = running(
            "cursor-agent",
            &["--force", "keep going"],
            protocol::PaneCommandPhase::Executing,
        );

        assert_eq!(
            observed_foreground_command(&pane).as_deref(),
            Some("cursor-agent --force 'keep going'"),
            "arguments distinguish one run of an agent from another, and must survive being \
             typed back at a prompt"
        );
    }

    /// `foreground_program` keeps naming the last command run while the shell idles at its prompt,
    /// so an unfiltered capture would resurrect whatever hook last ran - `cd` machinery included.
    #[test]
    fn a_pane_at_its_prompt_captures_nothing() {
        for phase in [
            protocol::PaneCommandPhase::Prompt,
            protocol::PaneCommandPhase::Input,
            protocol::PaneCommandPhase::Completed { exit_status: None },
        ] {
            let pane = running("__zoxide_hook", &[], phase);
            assert_eq!(
                observed_foreground_command(&pane),
                None,
                "{phase:?} means nothing is running, whatever the pane last named"
            );
        }
    }

    /// A pane sitting in its own shell is a pane sitting in its own shell. There is nothing to
    /// replay, and typing the shell's name back would nest a second one inside the first.
    #[test]
    fn a_pane_running_only_its_shell_captures_nothing() {
        let mut pane = running("zsh", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.foreground_shell = true;

        assert_eq!(observed_foreground_command(&pane), None);
    }

    /// `launch` and `foreground` answer different questions, and only the second is new
    /// information: a pane created to run a command already restores it by respawning that command.
    #[test]
    fn only_shell_panes_record_what_they_are_running() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-foreground-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "foreground",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(
            1,
            running(
                "nvim",
                &["src/main.rs"],
                protocol::PaneCommandPhase::Executing,
            ),
        );
        let mut launched = running("btop", &[], protocol::PaneCommandPhase::Executing);
        launched.launch = Some(crate::pane::launch::PaneLaunch::shell("btop"));
        server.panes.insert(2, launched);

        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let captured = |pane_id: PaneId| {
            job.meta
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .expect("pane in snapshot")
                .foreground
                .clone()
        };

        assert_eq!(captured(1).as_deref(), Some("nvim src/main.rs"));
        assert_eq!(
            captured(2),
            None,
            "a pane launched with a command restores it through `launch`; recording it twice \
             would run it twice"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// `never` is an instruction not to keep the command, not merely not to run it. A user who set
    /// it should not find their command lines sitting in the state directory.
    #[test]
    fn never_writes_no_command_to_the_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-never-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "never",
            ServerSettings {
                resurrect: true,
                resurrect_foreground: crate::config::ForegroundRestore::Never,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(
            1,
            running("cursor-agent", &[], protocol::PaneCommandPhase::Executing),
        );

        let job = server.capture_snapshot(Instant::now()).expect("capture");

        assert_eq!(job.meta.panes[0].foreground, None);

        let _ = fs::remove_dir_all(root);
    }

    /// The last window detaching withdraws its publishers' rows, then snapshots the session. The
    /// agent and its conversation are still running, so the snapshot must still name the
    /// conversation the active agent row did - until the pane shows the conversation is over.
    #[test]
    fn detaching_the_publishing_window_keeps_the_conversation_to_resume() {
        let mut server = SessionServer::new_named_with_settings(
            "native-agent-detach",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        let row = |publisher: &str, kind, id: &str, active: bool| protocol::PublishedRow {
            live_session: None,
            reported: false,
            publisher: publisher.into(),
            kind,
            id: id.into(),
            title: id.into(),
            status: "idle".into(),
            reason: None,
            active,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: Some(format!("native-{id}")),
        };
        pane.runtime.rows = vec![
            row("3/sessions", protocol::PublisherKind::Agent, "left", false),
            row("3/sessions", protocol::PublisherKind::Agent, "shown", true),
            // Another window's activity publisher stays; it is never a resume candidate.
            row(
                "4/publish:prs",
                protocol::PublisherKind::Activity,
                "pr",
                true,
            ),
        ];
        server.panes.insert(1, pane);

        let messages = server.withdraw_client_rows(3);
        assert_eq!(messages.len(), 1, "the departing window's rows went");
        assert_eq!(server.panes[&1].runtime.rows.len(), 1);
        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let resume = job.meta.panes[0]
            .agent_resume
            .as_ref()
            .expect("the conversation outlives its window's rows");
        assert_eq!(resume.agent, "claude");
        assert_eq!(resume.session, "native-shown");

        // Live agent rows arriving again speak for the pane instead.
        let pane = server.panes.get_mut(&1).unwrap();
        super::super::runtime::replace_pane_rows(
            pane,
            vec![row(
                "5/sessions",
                protocol::PublisherKind::Agent,
                "new",
                true,
            )],
        );
        assert!(pane.agent.retained_resume.is_none());
        let job = server.capture_snapshot(Instant::now()).expect("capture");
        assert_eq!(
            job.meta.panes[0].agent_resume.as_ref().unwrap().session,
            "native-new"
        );
    }

    fn agent_row(publisher: &str, id: &str, active: bool) -> protocol::PublishedRow {
        protocol::PublishedRow {
            publisher: publisher.into(),
            kind: protocol::PublisherKind::Agent,
            id: id.into(),
            title: id.into(),
            status: "idle".into(),
            reason: None,
            active,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: Some(format!("native-{id}")),
            live_session: None,
            reported: false,
        }
    }

    #[cfg(unix)]
    fn persisted_resume(root: &std::path::Path, session: &str) -> Option<String> {
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(session).join("meta.json")).unwrap())
                .unwrap();
        meta["panes"][0]["agent_resume"]["session"]
            .as_str()
            .map(str::to_string)
    }

    /// How Rozi learned of a conversation decides how a restore reopens it, so a change in that
    /// alone is a change the snapshot has to catch, in either direction, even with the same
    /// conversation and nothing printed.
    #[cfg(unix)]
    #[test]
    fn a_change_in_how_a_conversation_was_learned_reaches_the_persisted_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("snapshots");
        let name = "native-agent-provenance";
        let mut server = SessionServer::new_named_with_settings(
            name,
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::from_secs(60 * 60),
                ..ServerSettings::default()
            },
        );
        let (client, _stream) = super::super::tests::attach_client(&mut server);
        let mut pane = running("codex", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("codex", "Codex").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        let generation = pane.generation;
        pane.runtime.integration = Some(Box::new(protocol::AgentIntegrationReport {
            unless_quiet: None,
            integration: "hook".into(),
            identity: protocol::AgentIdentity::new("codex", "Codex"),
            reference: protocol::AgentRef {
                pane: protocol::PaneRef {
                    session_instance: server.instance_id.clone(),
                    pane_id: 1,
                    generation,
                },
                slot: None,
                incarnation: 1,
            },
            state: protocol::AgentState::Idle,
            reason: None,
            native_session: Some("native-a".into()),
            seq: 1,
            reported_at_unix_ms: 42,
        }));
        server.panes.insert(1, pane);
        let publish = |server: &mut SessionServer, rows| {
            server
                .report_pane_rows(
                    client,
                    1,
                    generation,
                    false,
                    super::super::runtime::PanePublisher {
                        epoch: 0,
                        responses: Vec::new(),
                        id: "sessions".into(),
                        kind: protocol::PublisherKind::Agent,
                    },
                    rows,
                )
                .expect("rows accepted")
        };
        let persisted = |root: &std::path::Path| {
            let meta: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(name).join("meta.json")).unwrap())
                    .unwrap();
            let resume = &meta["panes"][0]["agent_resume"];
            (
                resume["session"].as_str().map(str::to_string),
                resume["reported"].as_bool(),
            )
        };
        let session = || Some("native-a".to_string());

        server.mark_dirty();
        server.snapshot_before_shutdown().expect("first snapshot");
        assert_eq!(persisted(&root), (session(), Some(true)));

        // A publisher's active row now names the same conversation.
        publish(&mut server, vec![agent_row("", "a", true)]);
        assert!(server.snapshot_dirty(), "only how it was learned changed");
        server.snapshot_before_shutdown().expect("second snapshot");
        assert_eq!(persisted(&root), (session(), Some(false)));

        // That row goes, and the hooks' report speaks for the pane again.
        publish(&mut server, Vec::new());
        assert!(server.snapshot_dirty(), "only how it was learned changed");
        server.snapshot_before_shutdown().expect("third snapshot");
        assert_eq!(persisted(&root), (session(), Some(true)));
    }

    /// The directory a pane restores into is saved for every pane, so a change to it alone dirties
    /// the snapshot: even with nothing printed, for a pane launched with its own command, and with
    /// foreground commands not recorded at all.
    #[test]
    fn a_change_to_the_saved_directory_alone_dirties_the_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-cwd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let name = "saved-cwd";
        let mut server = SessionServer::new_named_with_settings(
            name,
            ServerSettings {
                resurrect: true,
                resurrect_foreground: crate::config::ForegroundRestore::Never,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::from_secs(60 * 60),
                ..ServerSettings::default()
            },
        );
        let mut pane = test_pane();
        pane.launch = Some(crate::pane::launch::PaneLaunch::shell("make watch"));
        let generation = pane.generation;
        server.panes.insert(1, pane);
        server.sync_pane_runtime(None, 1, generation);
        server.mark_dirty();
        server.snapshot_before_shutdown().expect("first snapshot");
        assert!(!server.snapshot_dirty());

        let (uri_path, moved) = if cfg!(windows) {
            ("/C:/moved/here", r"C:\moved\here")
        } else {
            ("/moved/here", "/moved/here")
        };
        // Written straight to the screen: the directory changes and the pane prints nothing.
        server
            .panes
            .get_mut(&1)
            .unwrap()
            .screen_mut()
            .process_bytes(format!("\x1b]7;file://localhost{uri_path}\x1b\\").as_bytes());
        server.sync_pane_runtime(None, 1, generation);
        assert!(server.snapshot_dirty(), "only the saved directory changed");

        server.snapshot_before_shutdown().expect("second snapshot");
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(name).join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["panes"][0]["cwd"], moved);
        let _ = fs::remove_dir_all(&root);
    }

    /// A publisher switching conversation dirties the snapshot even when the pane prints nothing
    /// afterwards, and so does its window leaving: the snapshot the normal scheduling path then
    /// writes names the conversation the publisher switched to, not the one persisted before.
    #[cfg(unix)]
    #[test]
    fn a_conversation_switch_and_its_windows_departure_reach_the_persisted_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-switch-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let name = "native-agent-switch";
        let mut server = SessionServer::new_named_with_settings(
            name,
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::from_secs(60 * 60),
                ..ServerSettings::default()
            },
        );
        let (client, _stream) = super::super::tests::attach_client(&mut server);
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        let generation = pane.generation;
        server.panes.insert(1, pane);
        let publish = |server: &mut SessionServer, rows| {
            server
                .report_pane_rows(
                    client,
                    1,
                    generation,
                    false,
                    super::super::runtime::PanePublisher {
                        epoch: 0,
                        responses: Vec::new(),
                        id: "sessions".into(),
                        kind: protocol::PublisherKind::Agent,
                    },
                    rows,
                )
                .expect("rows accepted")
        };

        publish(&mut server, vec![agent_row("", "a", true)]);
        assert!(server.snapshot_dirty());
        server.snapshot_before_shutdown().expect("first snapshot");
        assert!(!server.snapshot_dirty());
        assert_eq!(persisted_resume(&root, name).as_deref(), Some("native-a"));

        // An activity-like change that keeps the conversation does not dirty the snapshot.
        let mut retitled = agent_row("", "a", true);
        retitled.title = "renamed".into();
        publish(&mut server, vec![retitled]);
        assert!(!server.snapshot_dirty(), "the conversation did not change");

        // The publisher switches conversation; nothing else happens in the pane.
        publish(
            &mut server,
            vec![agent_row("", "a", false), agent_row("", "b", true)],
        );
        assert!(server.snapshot_dirty(), "the switch dirties the snapshot");
        server
            .snapshot_before_shutdown()
            .expect("snapshot after the switch");
        assert_eq!(persisted_resume(&root, name).as_deref(), Some("native-b"));

        // The window leaves: its rows go, the conversation is retained, and the retained fact
        // is what the next scheduled snapshot persists.
        server.snapshot_generation = server.dirty_generation;
        server.withdraw_client_rows(client);
        assert!(server.panes[&1].agent.retained_resume.is_some());
        assert_eq!(
            server
                .panes
                .get_mut(&1)
                .and_then(|pane| resume_target(pane))
                .map(|(_, session, _)| session)
                .as_deref(),
            Some("native-b")
        );
        server
            .snapshot_before_shutdown()
            .expect("snapshot after the departure");
        assert_eq!(persisted_resume(&root, name).as_deref(), Some("native-b"));

        let _ = fs::remove_dir_all(root);
    }

    /// A departed window's conversation never outlives an agent publisher that superseded it:
    /// when window A leaves while window B still publishes agent rows, nothing of A is retained,
    /// so B withdrawing later cannot bring A's conversation back.
    #[test]
    fn a_surviving_agent_publisher_supersedes_a_departed_windows_conversation() {
        let mut server = SessionServer::new_named_with_settings(
            "native-agent-two-windows",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        // A published first, so its active row is the pane's; B's row names no conversation.
        let mut b = agent_row("4/sessions", "b", true);
        b.native_session = None;
        pane.runtime.rows = vec![agent_row("3/sessions", "a", true), b];
        server.panes.insert(1, pane);
        let resumed = |server: &SessionServer| {
            resume_target(&server.panes[&1]).map(|(_, session, _)| session)
        };
        assert_eq!(resumed(&server).as_deref(), Some("native-a"));

        server.withdraw_client_rows(3);
        assert!(server.panes[&1].agent.retained_resume.is_none());
        assert_eq!(resumed(&server), None, "B speaks for the pane now");

        server.withdraw_client_rows(4);
        assert_eq!(resumed(&server), None, "A never comes back");
    }

    #[test]
    fn snapshot_records_only_explicit_native_agent_session_facts() {
        let mut server = SessionServer::new_named_with_settings(
            "native-agent",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        pane.runtime.integration = Some(Box::new(protocol::AgentIntegrationReport {
            unless_quiet: None,
            integration: "hook-abc".into(),
            identity: protocol::AgentIdentity::new("claude", "Claude Code"),
            reference: protocol::AgentRef {
                pane: protocol::PaneRef {
                    session_instance: server.instance_id.clone(),
                    pane_id: 1,
                    generation: pane.generation,
                },
                slot: None,
                incarnation: 1,
            },
            state: protocol::AgentState::Idle,
            reason: None,
            native_session: Some("opaque-session-123".into()),
            seq: 7,
            reported_at_unix_ms: 42,
        }));
        server.panes.insert(1, pane);

        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let resume = job.meta.panes[0]
            .agent_resume
            .as_ref()
            .expect("native session fact");
        assert_eq!(resume.agent, "claude");
        assert_eq!(resume.session, "opaque-session-123");
        assert_eq!(resume.reported_at, 42);

        // A client that switched conversations shows its active row's; the hooks' report still
        // names the one it left, so it must not decide what comes back.
        let pane = server.panes.get_mut(&1).unwrap();
        pane.runtime.rows = ["left-behind", "on-screen"]
            .into_iter()
            .map(|id| protocol::PublishedRow {
                publisher: "sessions".into(),
                kind: crate::session::protocol::PublisherKind::Agent,
                live_session: None,
                reported: false,
                id: id.into(),
                title: id.into(),
                status: "idle".into(),
                reason: None,
                active: id == "on-screen",
                work_started_at: None,
                cwd: None,
                project: None,
                native_session: Some(format!("native-{id}")),
            })
            .collect();
        // An activity publisher that published first, with an active row naming a "session" of
        // its own, is never what a restore resumes.
        let mut activity = pane.runtime.rows[1].clone();
        activity.publisher = "publish:prs".into();
        activity.kind = crate::session::protocol::PublisherKind::Activity;
        activity.native_session = Some("not-a-conversation".into());
        pane.runtime.rows.insert(0, activity.clone());
        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let resume = job.meta.panes[0].agent_resume.as_ref().expect("row fact");
        assert_eq!(resume.agent, "claude");
        assert_eq!(resume.session, "native-on-screen");

        // With only the activity row left, the hooks' report decides again.
        server.panes.get_mut(&1).unwrap().runtime.rows = vec![activity];
        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let resume = job.meta.panes[0]
            .agent_resume
            .as_ref()
            .expect("report fact");
        assert_eq!(resume.session, "opaque-session-123");

        server.settings.resurrect_agents = false;
        let private_job = server.capture_snapshot(Instant::now()).expect("capture");
        assert!(private_job.meta.panes[0].agent_resume.is_none());
    }

    /// A snapshot pane carrying a native agent session fact, and whatever else the test needs.
    fn saved_pane(agent: &str, session: &str, foreground: Option<&str>) -> SnapshotPane {
        SnapshotPane {
            scrollback: None,
            pane_id: 1,
            generation: 1,
            launch: None,
            foreground: foreground.map(str::to_string),
            agent_resume: Some(SnapshotAgentResume {
                agent: agent.to_string(),
                session: session.to_string(),
                reported_at: 42,
                reported: false,
            }),
            cwd: None,
            keep_open: false,
            title: None,
            palette: WirePalette {
                foreground: None,
                background: None,
                ansi: [tui_lipan::prelude::Color::Black; 16],
            },
            cols: 80,
            rows: 24,
        }
    }

    fn restore_server(mode: crate::config::ForegroundRestore, agents: bool) -> SessionServer {
        SessionServer::new_named_with_settings(
            "restore-action",
            ServerSettings {
                resurrect: true,
                resurrect_foreground: mode,
                resurrect_agents: agents,
                ..ServerSettings::default()
            },
        )
    }

    fn action(
        server: &SessionServer,
        saved: &SnapshotPane,
        resumed: &mut std::collections::HashSet<(String, String)>,
    ) -> RestoreAction {
        server.restore_action(saved, resumed)
    }

    /// The whole point of item 13: the stored fact plus the *current* definition's recipe, run as
    /// the pane's own process. The conversation reference is one argv element, so nothing parses it.
    #[test]
    fn a_reported_conversation_is_relaunched_with_the_current_resume_recipe() {
        let server = restore_server(crate::config::ForegroundRestore::Auto, true);
        let saved = saved_pane("claude", "opaque-session-123", Some("claude"));

        match action(&server, &saved, &mut std::collections::HashSet::new()) {
            RestoreAction::ResumeAgent(resume) => {
                assert_eq!(resume.label, "Claude Code");
                assert_eq!(
                    resume.launch.argv(),
                    Some(
                        ["claude", "--resume", "opaque-session-123"]
                            .map(String::from)
                            .as_slice()
                    ),
                    "the reference must arrive as one whole process argument"
                );
            }
            _ => panic!("a resumable conversation must outrank the observed foreground command"),
        }
    }

    /// How Rozi learned of a conversation decides who owns it. Codex's hooks only run in a client
    /// that owns its thread, and it has to keep doing so for them to go on reporting. A thread some
    /// other program published lives on the shared background server, which is what to reconnect to.
    #[test]
    fn a_conversation_resumes_the_way_rozi_learned_of_it() {
        let server = restore_server(crate::config::ForegroundRestore::Auto, true);
        let argv = |reported: bool| {
            let mut saved = saved_pane("codex", "thread", None);
            saved.agent_resume.as_mut().unwrap().reported = reported;
            match action(&server, &saved, &mut std::collections::HashSet::new()) {
                RestoreAction::ResumeAgent(resume) => {
                    resume.launch.argv().unwrap_or_default().to_vec()
                }
                _ => panic!("a resumable conversation must resume"),
            }
        };
        assert_eq!(
            argv(true),
            ["codex", "resume", "--no-daemon", "thread"].map(String::from)
        );
        assert_eq!(argv(false), ["codex", "resume", "thread"].map(String::from));
    }

    /// The fact the restore needs has to survive being saved, from both ways of learning it.
    #[test]
    fn a_snapshot_records_how_a_conversation_was_learned() {
        let mut server = restore_server(crate::config::ForegroundRestore::Auto, true);
        let mut pane = running("codex", &[], protocol::PaneCommandPhase::Executing);
        pane.agent.retained_resume = Some(RetainedResume {
            agent: "codex".to_string(),
            session: "thread".to_string(),
            reported_at: 1,
            reported: true,
        });
        server.panes.insert(1, pane);
        let job = server.capture_snapshot(Instant::now()).expect("capture");
        assert!(job.meta.panes[0].agent_resume.as_ref().unwrap().reported);

        let old: SnapshotAgentResume =
            serde_json::from_str(r#"{"agent":"codex","session":"thread","reported_at":1}"#)
                .unwrap();
        assert!(
            !old.reported,
            "a snapshot from before this reads as not reported"
        );
    }

    /// `hold` is an instruction that nothing runs unasked. Reopening a conversation is something
    /// running, so it waits at the prompt like any other held command.
    #[test]
    fn hold_leaves_the_resume_command_at_the_prompt() {
        let mut server = restore_server(crate::config::ForegroundRestore::Hold, true);
        server.settings.shell = vec!["/bin/bash".to_string()];
        let saved = saved_pane("claude", "needs quoting", None);

        match action(&server, &saved, &mut std::collections::HashSet::new()) {
            RestoreAction::Type(line) => {
                // Claude cannot resume a conversation that still runs in the background, only
                // attach to it, so the line tries both.
                assert_eq!(
                    line,
                    "claude --resume 'needs quoting' || claude attach 'needs qu'"
                );
            }
            _ => panic!("`hold` must not start the agent itself"),
        }
    }

    /// The held line is typed into whatever shell the session restores panes with, and the quoting
    /// families disagree about what a quote character even is. Rendering every one of them the
    /// POSIX way would hand `cmd.exe` a command with two stray apostrophes in it.
    #[test]
    fn a_held_resume_line_is_quoted_for_the_shell_that_will_read_it() {
        let saved = saved_pane("claude", "it's here", None);
        let line = |shell: &str| {
            let mut server = restore_server(crate::config::ForegroundRestore::Hold, true);
            server.settings.shell = vec![shell.to_string()];
            match action(&server, &saved, &mut std::collections::HashSet::new()) {
                RestoreAction::Type(line) => line,
                _ => panic!("`hold` must not start the agent itself"),
            }
        };

        assert_eq!(
            line("/bin/zsh"),
            r"claude --resume 'it'\''s here' || claude attach 'it'\''s her'"
        );
        assert_eq!(
            line("fish"),
            r"claude --resume 'it\'s here' || claude attach 'it\'s her'"
        );
        // PowerShell needs the call operator, or the line is an expression rather than a command.
        assert_eq!(
            line("pwsh"),
            "& claude --resume 'it''s here' || & claude attach 'it''s her'"
        );
        assert_eq!(
            line("cmd.exe"),
            "claude --resume \"it's here\" || claude attach \"it's her\""
        );
    }

    /// `a || b` is not syntax in every shell. A shell this cannot vouch for gets the plain resume
    /// line it always did, rather than one that fails to parse.
    #[test]
    fn a_shell_that_may_not_chain_gets_the_plain_resume_line() {
        let mut server = restore_server(crate::config::ForegroundRestore::Hold, true);
        server.settings.shell = vec!["nu".to_string()];
        let saved = saved_pane("claude", "needs quoting", None);

        match action(&server, &saved, &mut std::collections::HashSet::new()) {
            RestoreAction::Type(line) => {
                let quoting = crate::pane::launch::PromptQuoting::of(Some("nu"));
                assert!(!crate::pane::launch::PromptQuoting::chains_with_or(Some(
                    "nu"
                )));
                assert_eq!(
                    line,
                    format!(
                        "claude --resume {}",
                        crate::pane::launch::quote_for_prompt("needs quoting", quoting)
                    )
                );
            }
            _ => panic!("`hold` must not start the agent itself"),
        }
    }

    /// Windows PowerShell 5.1 cannot parse `||`, and Rozi's own Windows shell resolution falls
    /// back to it when PowerShell 7 is missing.
    #[test]
    fn windows_powershell_gets_the_plain_resume_line() {
        let mut server = restore_server(crate::config::ForegroundRestore::Hold, true);
        server.settings.shell = vec!["powershell.exe".to_string()];
        let saved = saved_pane("claude", "abc", None);

        match action(&server, &saved, &mut std::collections::HashSet::new()) {
            RestoreAction::Type(line) => assert!(!line.contains("||"), "{line}"),
            _ => panic!("`hold` must not start the agent itself"),
        }
    }

    /// The snapshot stored a fact, not a recipe. An agent that no longer declares how to resume -
    /// or is no longer declared at all - restores as the ordinary pane it otherwise is.
    #[test]
    fn an_agent_without_a_resume_capability_restores_normally() {
        let server = restore_server(crate::config::ForegroundRestore::Auto, true);

        for saved in [
            saved_pane("gone-from-the-catalog", "abc", Some("nvim")),
            // Built in, detectable, and declares no `[agents.resume]`.
            saved_pane("aider", "abc", Some("nvim")),
        ] {
            match action(&server, &saved, &mut std::collections::HashSet::new()) {
                RestoreAction::Type(line) => assert_eq!(line, "nvim"),
                _ => panic!("nothing can resume `{}`", saved.agent_resume.unwrap().agent),
            }
        }
    }

    /// Two panes cannot both be the conversation. Resuming it twice would leave two agents writing
    /// into one history, and neither of them the one the user was talking to.
    #[test]
    fn one_conversation_is_resumed_once() {
        let server = restore_server(crate::config::ForegroundRestore::Auto, true);
        let mut resumed = std::collections::HashSet::new();
        let first = saved_pane("claude", "shared-session", None);
        let mut second = saved_pane("claude", "shared-session", None);
        second.pane_id = 2;

        assert!(matches!(
            action(&server, &first, &mut resumed),
            RestoreAction::ResumeAgent(_)
        ));
        assert!(matches!(
            action(&server, &second, &mut resumed),
            RestoreAction::Nothing
        ));
    }

    /// Both halves of the policy are read as they stand *now*, so turning either off also defuses a
    /// snapshot written while it was on.
    #[test]
    fn policy_declines_to_reopen_a_stored_conversation() {
        let mut resumed = std::collections::HashSet::new();
        let saved = saved_pane("claude", "opaque-session-123", None);

        for server in [
            restore_server(crate::config::ForegroundRestore::Never, true),
            restore_server(crate::config::ForegroundRestore::Auto, false),
        ] {
            assert!(matches!(
                action(&server, &saved, &mut resumed),
                RestoreAction::Nothing
            ));
        }
    }

    /// Rozi never reads the reference, but it does hand it to a process that will echo it. A value
    /// carrying terminal control bytes is not a conversation id.
    #[test]
    fn an_implausible_conversation_reference_is_refused() {
        let server = restore_server(crate::config::ForegroundRestore::Auto, true);

        for session in ["", "abc\u{1b}[2J", &"x".repeat(MAX_NATIVE_SESSION + 1)] {
            let saved = saved_pane("claude", session, None);
            assert!(
                matches!(
                    action(&server, &saved, &mut std::collections::HashSet::new()),
                    RestoreAction::Nothing
                ),
                "accepted {session:?}"
            );
        }
    }

    /// `never` is a privacy setting as much as a restore setting: an opaque conversation id is
    /// exactly the kind of thing a user who set it does not want left in the state directory.
    #[test]
    fn never_writes_no_conversation_reference_to_the_snapshot() {
        let mut server = SessionServer::new_named_with_settings(
            "native-never",
            ServerSettings {
                resurrect: true,
                resurrect_foreground: crate::config::ForegroundRestore::Never,
                ..ServerSettings::default()
            },
        );
        server
            .panes
            .insert(1, reporting_agent_pane(&server.instance_id));

        let job = server.capture_snapshot(Instant::now()).expect("capture");

        assert!(job.meta.panes[0].agent_resume.is_none());
    }

    /// A pane still running the resume command a restore started for it has no shell in it, so
    /// there is no typed command to record - and recording one would run the conversation twice.
    #[test]
    fn a_resumed_pane_records_no_foreground_command() {
        let mut server = SessionServer::new_named_with_settings(
            "resumed",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = reporting_agent_pane(&server.instance_id);
        pane.agent_resume = Some(PaneResume {
            label: "Claude Code".to_string(),
            attach: None,
            attached: false,
            started: Instant::now(),
        });
        server.panes.insert(1, pane);

        let job = server.capture_snapshot(Instant::now()).expect("capture");

        assert_eq!(job.meta.panes[0].foreground, None);
        assert!(
            job.meta.panes[0].agent_resume.is_some(),
            "the conversation reference is still a fact worth keeping"
        );
    }

    /// A pane running Claude with a live integration reporting a native conversation.
    fn reporting_agent_pane(instance: &protocol::SessionInstanceId) -> ServerPane {
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.runtime.integration = Some(Box::new(protocol::AgentIntegrationReport {
            unless_quiet: None,
            integration: "hook-abc".into(),
            identity: protocol::AgentIdentity::new("claude", "Claude Code"),
            reference: protocol::AgentRef {
                pane: protocol::PaneRef {
                    session_instance: instance.clone(),
                    pane_id: 1,
                    generation: pane.generation,
                },
                slot: None,
                incarnation: 1,
            },
            state: protocol::AgentState::Idle,
            reason: None,
            native_session: Some("opaque-session-123".into()),
            seq: 7,
            reported_at_unix_ms: 42,
        }));
        pane
    }

    /// The cycle after a failed resume, which is where the guarantee was quietly breaking.
    ///
    /// A pane created to run an agent keeps that launch intent, so once the resume failed and the
    /// pane fell back to a shell, the next snapshot recorded `claude` with no conversation beside
    /// it - and the restore after *that* fell through restore precedence to the launch intent and
    /// opened a fresh conversation. One server lifetime later than the failure, but the same
    /// substitution the failure path exists to prevent.
    ///
    /// This covers the second half of that chain - a pane in the post-fallback state snapshots as
    /// a shell and restores as one. That the fallback actually puts a pane into this state is
    /// pinned next door, where a real PTY can fail to spawn
    /// (`a_resume_command_that_cannot_start_still_leaves_a_working_shell`).
    #[test]
    fn a_pane_that_fell_back_to_a_shell_snapshots_as_a_shell() {
        let mut server = SessionServer::new_named_with_settings(
            "fell-back",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        // What the pane looks like once the fallback has run: a shell, with the agent's launch
        // intent cleared and no integration left to report a conversation.
        let mut pane = running("bash", &[], protocol::PaneCommandPhase::Prompt);
        pane.launch = None;
        pane.agent_resume = None;
        server.panes.insert(1, pane);

        let job = server.capture_snapshot(Instant::now()).expect("capture");
        let saved = &job.meta.panes[0];

        assert_eq!(
            saved.launch, None,
            "a fresh conversation is one restore away"
        );
        assert!(saved.agent_resume.is_none());
        assert!(matches!(
            server.restore_action(saved, &mut std::collections::HashSet::new()),
            RestoreAction::Nothing
        ));
    }

    /// Resurrection must inject integration itself because no client prepares the shell launch.
    #[cfg(unix)]
    #[test]
    fn restored_shell_reports_readiness_without_the_foreground_fallback() {
        if std::process::Command::new("bash")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping Bash restore check: bash is unavailable");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".bashrc"),
            "PS1='$(while [ ! -f \"$HOME/prompt-ready\" ]; do sleep 0.01; done)$ '\n",
        )
        .unwrap();
        // Keep the real Bash parser and PTY, but isolate its startup files in a minimal HOME.
        // The executable basename also exercises Bash integration detection during restore.
        let bash = root.path().join("bash");
        fs::write(
            &bash,
            format!(
                "#!/bin/sh\nexport HOME={}\nexec bash \"$@\"\n",
                crate::platform::command::quote_posix_arg(&root.path().to_string_lossy())
            ),
        )
        .unwrap();
        fs::set_permissions(&bash, fs::Permissions::from_mode(0o700)).unwrap();

        let mut server = SessionServer::new_named_with_settings(
            "prompt-ready",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.path().join("snapshots")),
                shell: vec![bash.to_string_lossy().into_owned(), "--noprofile".into()],
                resurrect_foreground: crate::config::ForegroundRestore::Hold,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(
            1,
            running(
                "printf",
                &["restored-command"],
                protocol::PaneCommandPhase::Executing,
            ),
        );
        server.mark_dirty();
        server.snapshot_before_shutdown().unwrap();
        server.panes.clear();
        assert_eq!(server.restore().unwrap(), 1);
        assert_eq!(server.pending_foreground.len(), 1);

        // Hold Bash between the start and end of prompt rendering. No replay bytes may be sent
        // while prompt expansion still owns the terminal.
        let deadline = Instant::now() + Duration::from_secs(10);
        while server.panes[&1].runtime.command_phase != protocol::PaneCommandPhase::Prompt {
            server.drain_pty_events();
            assert!(
                Instant::now() < deadline,
                "restored Bash never started its prompt"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        server.flush_pending_foreground();
        assert_eq!(server.pending_foreground.len(), 1);
        fs::write(root.path().join("prompt-ready"), "").unwrap();

        while server.panes[&1].runtime.command_phase != protocol::PaneCommandPhase::Input {
            server.drain_pty_events();
            assert!(
                Instant::now() < deadline,
                "restored Bash never reported input readiness"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        server.flush_pending_foreground();
        assert!(server.pending_foreground.is_empty());
        while !server
            .panes
            .get_mut(&1)
            .unwrap()
            .screen_without_change()
            .render_snapshot()
            .text
            .contains("restored-command")
        {
            server.drain_pty_events();
            assert!(Instant::now() < deadline, "restored command was not echoed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            server.panes[&1].runtime.command_phase,
            protocol::PaneCommandPhase::Input
        );
        let text = server
            .panes
            .get_mut(&1)
            .unwrap()
            .screen_without_change()
            .render_snapshot()
            .text;
        assert!(text.contains("$ printf restored-command"), "{text:?}");
        assert_eq!(text.matches("restored-command").count(), 1, "{text:?}");
        assert!(server.panes[&1].launch.is_none());
    }

    /// A pane created by running the agent directly has no shell to hold a line. Starting its launch
    /// would run a new conversation nobody asked for, and type the resume line into that agent's
    /// prompt, where pressing Enter sends it to the model.
    #[cfg(unix)]
    #[test]
    fn hold_gives_a_pane_launched_as_the_agent_a_shell_for_its_resume_line() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("launched");
        let mut server = SessionServer::new_named_with_settings(
            "held-launch",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.path().join("snapshots")),
                shell: vec!["/bin/sh".into()],
                resurrect_foreground: crate::config::ForegroundRestore::Hold,
                ..ServerSettings::default()
            },
        );
        let mut pane = running("claude", &[], protocol::PaneCommandPhase::Executing);
        pane.launch = Some(
            crate::pane::launch::PaneLaunch::direct(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("touch '{}'; sleep 30", marker.display()),
            ])
            .unwrap(),
        );
        pane.agent.retained_resume = Some(super::super::RetainedResume {
            agent: "claude".to_string(),
            session: "held-conversation".to_string(),
            reported_at: 1,
            reported: false,
        });
        server.panes.insert(1, pane);
        server.mark_dirty();
        server.snapshot_before_shutdown().unwrap();
        server.panes.clear();

        assert_eq!(server.restore().unwrap(), 1);
        assert!(
            server.panes[&1].launch.is_none(),
            "this pane is a shell now"
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            server.drain_pty_events();
            server.flush_pending_foreground();
            let text = server
                .panes
                .get_mut(&1)
                .unwrap()
                .screen_without_change()
                .render_snapshot()
                .text;
            // The line wraps at the pane's width.
            if text
                .replace('\n', "")
                .contains("claude --resume held-conversation")
            {
                break;
            }
            assert!(Instant::now() < deadline, "no resume line: {text:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !marker.exists(),
            "`hold` ran what the pane was launched with"
        );
        server.panes.clear();
    }

    /// A command is typed only once the restored shell says it is reading the terminal. Writing it
    /// at spawn races the shell's own startup, and under `auto` a half-consumed line would submit
    /// something other than what was captured.
    #[test]
    fn a_queued_command_waits_for_the_restored_shell_to_reach_its_prompt() {
        let mut server = SessionServer::new_named_with_settings(
            "pending",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = test_pane();
        pane.generation = 7;
        server.panes.insert(1, pane);
        server.replay_foreground_command(1, 7, "cursor-agent");

        server.flush_pending_foreground();
        assert_eq!(
            server.pending_foreground.len(),
            1,
            "a shell that has not reported a prompt is not ready to be typed into"
        );

        server.panes.get_mut(&1).unwrap().runtime.command_phase =
            protocol::PaneCommandPhase::Prompt;
        server.flush_pending_foreground();
        assert_eq!(server.pending_foreground.len(), 1);

        server.panes.get_mut(&1).unwrap().runtime.command_phase = protocol::PaneCommandPhase::Input;
        server.flush_pending_foreground();
        assert!(server.pending_foreground.is_empty());
    }

    #[test]
    fn a_queued_command_without_an_input_marker_uses_the_fallback() {
        let mut server =
            SessionServer::new_named_with_settings("fallback", ServerSettings::default());
        for phase in [
            protocol::PaneCommandPhase::Unknown,
            protocol::PaneCommandPhase::Prompt,
        ] {
            let mut pane = test_pane();
            pane.runtime.command_phase = phase;
            server.panes.insert(1, pane);
            server.replay_foreground_command(1, 1, "cursor-agent");
            server.pending_foreground[0].waiting_since = Instant::now() - REPLAY_PROMPT_WAIT;

            server.flush_pending_foreground();
            assert!(server.pending_foreground.is_empty());
        }
    }

    /// The pane the command belonged to is gone the moment its generation moves; typing an old
    /// pane's command into its replacement would be someone else's command appearing from nowhere.
    #[test]
    fn a_queued_command_is_dropped_when_its_pane_is_replaced() {
        let mut server = SessionServer::new_named_with_settings(
            "respawned",
            ServerSettings {
                resurrect: true,
                ..ServerSettings::default()
            },
        );
        let mut pane = test_pane();
        pane.generation = 7;
        pane.runtime.command_phase = protocol::PaneCommandPhase::Prompt;
        server.panes.insert(1, pane);
        server.replay_foreground_command(1, 7, "cursor-agent");

        server.panes.get_mut(&1).unwrap().generation = 8;
        server.flush_pending_foreground();

        assert!(server.pending_foreground.is_empty());
    }

    /// `never` is read when the session is restored, not when it was snapshotted, so turning it on
    /// also defuses a snapshot taken while it was off.
    #[test]
    fn never_replays_nothing_from_an_existing_snapshot() {
        let mut server = SessionServer::new_named_with_settings(
            "defused",
            ServerSettings {
                resurrect: true,
                resurrect_foreground: crate::config::ForegroundRestore::Never,
                ..ServerSettings::default()
            },
        );
        server.panes.insert(1, test_pane());
        server.replay_foreground_command(1, 1, "terraform apply");

        assert!(server.pending_foreground.is_empty());
    }

    fn test_pane() -> ServerPane {
        ServerPane {
            scrollback: 100,
            generation: 1,
            title: None,
            event_watch: Default::default(),
            cwd: None,
            launch: None,
            agent_resume: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: WirePalette {
                foreground: None,
                background: None,
                ansi: [tui_lipan::prelude::Color::Black; 16],
            },
            pty: None,
            pty_resize_pending: false,
            terminal: crate::pane::engine::screen(5, 40, 100),
            content_generation: 0,
            output_sequence: 0,
            output_coalesce_epoch: 0,
            output_times: VecDeque::new(),
            output_ledger: VecDeque::new(),
            output_bytes_sent: 0,
            graphics_pacer: Default::default(),
            output_seen: false,
            cols: 40,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            program_status_sync: Default::default(),
            drag_and_drop: Default::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            foreground_launch_cwd: None,
            borrowed_program_name: None,
            last_git_read: None,
            initial_cursor_report_primed: false,
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    /// Output arriving while a snapshot is being written must not be treated as already saved.
    #[test]
    fn changes_during_a_write_are_not_marked_snapshotted() {
        let root = std::env::temp_dir().join(format!(
            "rozi-snapshot-generation-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut server = SessionServer::new_named_with_settings(
            "generation",
            ServerSettings {
                resurrect: true,
                snapshot_dir: Some(root.clone()),
                snapshot_interval: Duration::ZERO,
                ..ServerSettings::default()
            },
        );

        server.mark_dirty();
        server.maybe_snapshot().expect("dispatch snapshot");
        // Stands in for a pane emitting output between the capture and the durable write landing.
        server.mark_dirty();
        server.wait_for_snapshots().expect("successful snapshot");

        assert_eq!(server.resurrection_metrics.successes, 1);
        assert!(
            server.snapshot_dirty(),
            "the change that arrived mid-write must survive as unsaved"
        );

        // The next snapshot captures it, and only then is the session clean.
        server
            .maybe_snapshot()
            .expect("dispatch follow-up snapshot");
        server
            .wait_for_snapshots()
            .expect("successful follow-up snapshot");
        assert_eq!(server.resurrection_metrics.successes, 2);
        assert!(!server.snapshot_dirty());

        let _ = fs::remove_dir_all(root);
    }
}
