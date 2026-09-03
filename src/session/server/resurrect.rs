use super::*;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Version 3 stores the command a pane was observed running alongside the command it was launched
/// with. Version 2 stored pane launch intent as shell-or-direct rather than a shell-only command
/// string.
const SNAPSHOT_VERSION: u32 = 3;

/// How long shutdown waits for an in-flight durable write before abandoning it.
const SNAPSHOT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long a restored pane's captured command waits for that pane's shell to report a prompt
/// before being typed anyway. Only reached by a pane with no shell integration to report one, so
/// the value is a bound on the wait rather than the wait itself: long enough that a slow `rc` file
/// has finished, short enough that a restored session is not visibly inert.
const FOREGROUND_PROMPT_WAIT: Duration = Duration::from_secs(3);

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
    crate::platform::fs_security::ensure_private_dir(parent)?;
    let suffix = format!(
        "{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp = parent.join(format!(".{session_name}.tmp-{suffix}"));
    let backup = parent.join(format!(".{session_name}.old-{suffix}"));
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_from_profile: Option<String>,
    panes: Vec<SnapshotPane>,
}

#[derive(Serialize, Deserialize)]
struct SnapshotPane {
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
    cwd: Option<String>,
    keep_open: bool,
    title: Option<String>,
    palette: WirePalette,
    cols: u16,
    rows: u16,
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
            || crate::state::is_ephemeral_session_name(&self.session_name)
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
            || crate::state::is_ephemeral_session_name(&self.session_name)
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

    /// Program names that mean "this pane is sitting at a prompt" rather than "this pane is
    /// running something": the common set, plus every interactive shell this server actually
    /// launches panes with. A pane whose foreground program is its own shell has nothing to replay.
    fn shell_basenames(&self) -> std::collections::HashSet<String> {
        let mut shells = crate::pane::launch::common_shell_basenames();
        for program in self
            .settings
            .shell
            .first()
            .into_iter()
            .chain(self.panes.values().filter_map(|pane| pane.shell.first()))
        {
            shells.insert(crate::platform::command::normalized_program_name(program));
        }
        shells
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
        let shells = self.shell_basenames();
        for (&pane_id, pane) in &mut self.panes {
            // The popup slot is a transient client-local overlay; resurrecting it would
            // revive an invisible orphan pane no client adopts.
            if pane.exited.is_some() || pane_id == crate::state::POPUP_PANE_ID {
                continue;
            }
            // A pane running its own launch command has nothing to add: restoring `launch` already
            // starts it. Only a shell that someone typed into is telling the snapshot something it
            // does not otherwise know.
            let foreground = (record_foreground && pane.launch.is_none())
                .then(|| observed_foreground_command(pane, &shells))
                .flatten();
            panes.push(SnapshotPane {
                pane_id,
                generation: pane.generation,
                launch: pane.launch.clone(),
                foreground,
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
                created_from_profile: self.created_from_profile.clone(),
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

    pub(super) fn restore(&mut self) -> io::Result<usize> {
        let path = self.snapshot_path()?;
        let meta: SnapshotMeta =
            serde_json::from_slice(&fs::read(path.join("meta.json"))?).map_err(io::Error::other)?;
        if meta.version != SNAPSHOT_VERSION || meta.session != self.session_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported or mismatched session snapshot",
            ));
        }
        self.created_from_profile = meta.created_from_profile.clone();
        let mut layout: Option<SharedLayout> = fs::read(path.join("layout.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let mut restored = 0;
        for saved in meta.panes {
            let generation = self.next_generation;
            self.next_generation += 1;
            let replay = fs::read(path.join("panes").join(format!("{}.replay", saved.pane_id)))
                .unwrap_or_default();
            let result = self.spawn_pane_inner(
                SpawnRequest {
                    owner: None,
                    pane_id: saved.pane_id,
                    generation,
                    launch: saved.launch,
                    cwd: saved.cwd,
                    title: saved.title,
                    cols: saved.cols,
                    rows: saved.rows,
                    keep_open: saved.keep_open,
                    env: Vec::new(),
                    palette: saved.palette,
                    shell: self.settings.shell.clone(),
                    command_shell: self.settings.command_shell.clone(),
                    // Resurrect predates any client attaching; the first controller resize
                    // reports the real cell size.
                    cell: None,
                },
                Some(&replay),
                true,
            );
            if matches!(result, ServerMessage::SpawnResult { ok: true, .. }) {
                restored += 1;
                if let Some(command) = saved.foreground.as_deref() {
                    self.replay_foreground_command(saved.pane_id, generation, command);
                }
            }
            if let Some(layout) = &mut layout {
                for workspace in &mut layout.workspaces {
                    for pane in &mut workspace.panes {
                        if pane.pane_id == saved.pane_id {
                            pane.generation = generation;
                        }
                    }
                }
            }
        }
        if let Some(layout_ref) = &layout
            && (layout_ref.validate().is_err()
                || (!layout_ref.workspaces.is_empty()
                    && !self.validate_shared_layout_against_panes(layout_ref)))
        {
            layout = None;
        }
        self.layout = layout;
        self.layout_rev = u64::from(self.layout.is_some());
        self.snapshot_generation = self.dirty_generation;
        self.forget_persisted_replays();
        self.last_snapshot = Instant::now();
        Ok(restored)
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
    /// "Ready" is the shell's own prompt report (`OSC 133 A`, surfaced as
    /// [`PaneCommandPhase::Prompt`]): past that point the shell is reading the terminal and
    /// nothing else is. Shell integration is not universal - `cmd.exe` has no `133;B`, and a user
    /// may have turned injection off - so a pane that never reports one falls back to writing
    /// after [`FOREGROUND_PROMPT_WAIT`], by which point any ordinary shell is long up. The bytes
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
            let prompted = matches!(
                pane.runtime.command_phase,
                protocol::PaneCommandPhase::Prompt | protocol::PaneCommandPhase::Input
            );
            if prompted || pending.waiting_since.elapsed() >= FOREGROUND_PROMPT_WAIT {
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

/// Best-effort sweep of abandoned staging/backup directories for one session. A leftover that
/// resists removal is not worth failing a delete over: it is already invisible to discovery.
fn remove_staging_dirs(parent: &Path, session_name: &str) {
    let prefixes = [
        format!(".{session_name}.tmp-"),
        format!(".{session_name}.old-"),
    ];
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // A session name cannot contain `.`, so the separator keeps `foo` from matching `foo-bar`.
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
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
    server.delete_snapshot()
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
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The command this pane's shell is running right now, if it is worth writing down. Shares its
/// judgment with profile capture - see
/// [`replayable_foreground_command`](crate::pane::launch::replayable_foreground_command), which
/// explains why a pane at a prompt reports nothing here even though it still names a program.
fn observed_foreground_command(
    pane: &ServerPane,
    shells: &std::collections::HashSet<String>,
) -> Option<String> {
    crate::pane::launch::replayable_foreground_command(
        crate::pane::launch::ForegroundSnapshot {
            command_phase: pane.runtime.command_phase,
            program: pane.runtime.foreground_program.as_deref(),
            executable: pane.runtime.foreground_executable.as_deref(),
            arguments: &pane.runtime.foreground_arguments,
            remote: pane.runtime.cwd_host.is_some(),
        },
        shells,
    )
}

fn default_snapshot_dir() -> Option<PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    if env.home.is_none() && env.xdg_state_home.is_none() {
        return None;
    }
    Some(crate::platform::paths::state_dir(&env).join("sessions"))
}

pub(crate) fn list_snapshot_names_by_recency() -> Vec<String> {
    let Some(root) = default_snapshot_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut snapshots = entries
        .flatten()
        .filter_map(|entry| {
            // Identity is the directory the snapshot is *published* under, not the name recorded
            // inside it. A staging directory abandoned by a killed writer still holds a complete
            // `meta.json`; trusting that would list a restorable session that forget can never
            // remove, because forget deletes `<root>/<session>` and the debris is not there.
            let dir_name = entry.file_name().into_string().ok()?;
            let meta: SnapshotMeta =
                serde_json::from_slice(&fs::read(entry.path().join("meta.json")).ok()?).ok()?;
            (meta.version == SNAPSHOT_VERSION
                && meta.session == dir_name
                && crate::session::discovery::valid_session_name(&meta.session))
            .then_some((meta.saved_at, meta.session))
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    snapshots.into_iter().map(|(_, name)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert_eq!(meta.created_from_profile, None);
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
                    created_from_profile: None,
                    panes: Vec::new(),
                })
                .unwrap(),
            )
            .expect("write snapshot meta");
        }

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

    fn shells() -> std::collections::HashSet<String> {
        crate::pane::launch::common_shell_basenames()
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
            observed_foreground_command(&pane, &shells()).as_deref(),
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
                observed_foreground_command(&pane, &shells()),
                None,
                "{phase:?} means nothing is running, whatever the pane last named"
            );
        }
    }

    /// A pane sitting in its own shell is a pane sitting in its own shell. There is nothing to
    /// replay, and typing the shell's name back would nest a second one inside the first.
    #[test]
    fn a_pane_running_only_its_shell_captures_nothing() {
        let pane = running("zsh", &[], protocol::PaneCommandPhase::Executing);

        assert_eq!(observed_foreground_command(&pane, &shells()), None);
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
        assert!(server.pending_foreground.is_empty());
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
            generation: 1,
            title: None,
            cwd: None,
            launch: None,
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
            terminal: TerminalScreen::new(5, 40, 100),
            content_generation: 0,
            cols: 40,
            rows: 5,
            exited: None,
            log: None,
            runtime: protocol::PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
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
