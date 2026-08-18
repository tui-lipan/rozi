//! Pane runtime-state computation (cross-platform plan Phase 6/7).
//!
//! [`SessionServer::sync_pane_runtime`] is the single call site the rest of `session::server`
//! should use: it re-derives a pane's [`protocol::PaneRuntimeState`] from its current
//! `TerminalScreen` semantic state plus the [`ProcessInspector`] fallback, and - only when
//! something actually changed - stores the new value and broadcasts
//! [`ServerMessage::PaneRuntimeChanged`].

use super::*;
use crate::platform::process::{LazyProcessScan, PlatformProcessInspector, ProcessInspector};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::protocol::{
    DetectedAgent, PANE_STATUS_MAX_LEN, PANE_STATUS_REASON_MAX_LEN, PaneCommandPhase,
    PaneCwdSource, PaneRuntimeState, PaneStatus,
};

const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How often agent detection re-sweeps even when the foreground program and command phase are
/// unchanged, so a wrapped process that appears inside an unchanged foreground (a package runner
/// launching an agent, say) is still noticed. Detection is far too expensive to run on every
/// [`RUNTIME_POLL_INTERVAL`]; see `AgentScratch::probe`.
pub(super) const AGENT_DETECT_REFRESH: Duration = Duration::from_secs(2);

/// How often a pane re-reads its project root and branch from disk. Both change without the cwd
/// moving — `git checkout` swaps the branch under a stationary shell, `git init` turns a plain
/// directory into a project — so neither can hang off the cwd-changed guard the way `display_path`
/// does. A read is an ancestor walk of `.git` probes plus one small file, cheap enough at this rate
/// that it is not worth a finer trigger.
pub(super) const GIT_REFRESH: Duration = Duration::from_secs(2);

/// How long a held agent state survives with no confirming evidence.
///
/// Generous on purpose: this is a backstop for a pane whose agent the scraper permanently lost
/// track of, not a limit on how long a run may take. A run that actually ends is caught as soon as
/// the agent draws a prompt again, because that is positive idle evidence rather than silence.
pub(super) const AGENT_HOLD_MAX: Duration = Duration::from_secs(15 * 60);

impl SessionServer {
    pub(super) fn set_pane_status(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        status: Option<String>,
        reason: Option<String>,
    ) -> std::result::Result<Option<PaneRuntimeState>, (&'static str, String)> {
        if !self.client_attached(client_id) {
            return Err(("attach-required", "client is not attached".to_string()));
        }
        if self.client_read_only(client_id) {
            return Err(("read-only", "read-only client".to_string()));
        }

        let owner = local.then_some(client_id);
        let Some(pane) = self.pane_mut(owner, pane_id) else {
            return Err(("pane-not-found", format!("pane {pane_id} not found")));
        };
        if pane.generation != generation {
            return Err((
                "stale-generation",
                format!("pane {pane_id} generation does not match"),
            ));
        }
        if pane.exited.is_some() {
            return Err(("pane-exited", format!("pane {pane_id} has exited")));
        }

        let value = status
            .map(|value| {
                tui_lipan::utils::sanitize_display_text(&value)
                    .trim()
                    .to_string()
            })
            .map(|value| value.chars().take(PANE_STATUS_MAX_LEN).collect::<String>())
            .filter(|value| !value.is_empty());
        let reason = value.as_ref().and_then(|_| {
            reason
                .map(|reason| {
                    tui_lipan::utils::sanitize_display_text(&reason)
                        .trim()
                        .to_string()
                })
                .map(|reason| {
                    reason
                        .chars()
                        .take(PANE_STATUS_REASON_MAX_LEN)
                        .collect::<String>()
                })
                .filter(|reason| !reason.is_empty())
        });

        let unchanged = match (&pane.runtime.status, &value) {
            (None, None) => true,
            (Some(current), Some(value)) => current.value == *value && current.reason == reason,
            _ => false,
        };
        if unchanged {
            return Ok(None);
        }

        let previous_runtime = pane.runtime.clone();
        let set_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        pane.runtime.status = value.map(|value| PaneStatus {
            value,
            reason,
            set_at,
        });
        pane.runtime.work_started_at = next_work_started_at(
            &previous_runtime,
            pane.runtime.status.as_ref(),
            pane.runtime.detected_agent.as_ref(),
        );
        pane.runtime.sequence = pane.runtime.sequence.wrapping_add(1);
        Ok(Some(pane.runtime.clone()))
    }

    /// Replace a pane's published rows. An empty list withdraws them and lets screen
    /// detection speak for the pane again.
    ///
    /// Guards match [`Self::set_pane_status`] exactly: this is the same authority - a program
    /// speaking for its own pane - reporting more than one thing at a time.
    pub(super) fn report_pane_rows(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        rows: Vec<protocol::PublishedRow>,
    ) -> std::result::Result<Option<PaneRuntimeState>, (&'static str, String)> {
        if !self.client_attached(client_id) {
            return Err(("attach-required", "client is not attached".to_string()));
        }
        if self.client_read_only(client_id) {
            return Err(("read-only", "read-only client".to_string()));
        }
        let owner = local.then_some(client_id);
        let Some(pane) = self.pane_mut(owner, pane_id) else {
            return Err(("pane-not-found", format!("pane {pane_id} not found")));
        };
        if pane.generation != generation {
            return Err((
                "stale-generation",
                format!("pane {pane_id} generation does not match"),
            ));
        }
        if pane.exited.is_some() {
            return Err(("pane-exited", format!("pane {pane_id} has exited")));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let rows = sanitize_rows(rows, &pane.runtime.rows, now);
        if rows == pane.runtime.rows {
            return Ok(None);
        }
        pane.runtime.rows = rows;
        // A publisher that enumerates its own sessions is better informed than the scraper, which
        // can only see the one it draws. Drop anything the scraper was holding for this pane.
        pane.agent.hold = None;
        pane.runtime.sequence = pane.runtime.sequence.wrapping_add(1);
        Ok(Some(pane.runtime.clone()))
    }

    pub(super) fn poll_pane_runtime(&mut self) {
        if self.last_runtime_poll.elapsed() < RUNTIME_POLL_INTERVAL {
            return;
        }
        self.last_runtime_poll = Instant::now();
        let panes: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.pty.is_some())
            .map(|(&id, pane)| (id, pane.generation))
            .collect();
        // One process-table walk serves every pane in this cycle, and is captured only if some
        // pane's detection is actually stale (see `LazyProcessScan`). Without this each pane
        // walked the whole host separately, so idle cost scaled with pane count.
        let mut scan = LazyProcessScan::default();
        for (id, generation) in panes {
            self.sync_pane_runtime_inner(None, id, generation, true, &mut scan);
        }
    }

    /// Recompute `pane_id`'s runtime state and broadcast a
    /// [`ServerMessage::PaneRuntimeChanged`] if it changed. `generation` must match the pane's
    /// current generation - a stale caller (e.g. a queued event racing a respawn) is a silent
    /// no-op, matching every other per-pane event handler in this module.
    pub(super) fn sync_pane_runtime(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
    ) {
        self.sync_pane_runtime_inner(
            owner,
            pane_id,
            generation,
            false,
            &mut LazyProcessScan::default(),
        );
    }

    fn sync_pane_runtime_inner(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
        detect_agent: bool,
        scan: &mut LazyProcessScan,
    ) {
        let Some(pane) = self.pane_mut(owner, pane_id) else {
            return;
        };
        if pane.generation != generation {
            return;
        }
        let inspector = PlatformProcessInspector::default();
        let next = compute_runtime_state(pane, &inspector, detect_agent, scan);
        if next == pane.runtime {
            return;
        }
        pane.runtime = next.clone();
        let message = ServerMessage::PaneRuntimeChanged {
            pane_id,
            local: wire_local(owner),
            generation,
            state: next,
        };
        if let Some(owner) = owner {
            self.enqueue(owner, Target::Client(owner), message);
        } else {
            self.broadcast_control(&message);
        }
    }
}

/// Build the candidate [`PaneRuntimeState`] for `pane`, bumping `sequence` past its previous value
/// only when some other field actually differs (a no-op recompute must not burn a sequence number,
/// or every idle heartbeat tick would look like a change to clients).
fn compute_runtime_state(
    pane: &mut ServerPane,
    inspector: &impl ProcessInspector,
    detect_agent: bool,
    scan: &mut LazyProcessScan,
) -> PaneRuntimeState {
    let semantic = pane.screen().semantic_state();
    let command_phase = PaneCommandPhase::from(semantic.command_phase);
    let last_exit_status = match command_phase {
        PaneCommandPhase::Completed { exit_status } => {
            exit_status.or(pane.runtime.last_exit_status)
        }
        _ => pane.runtime.last_exit_status,
    };
    // A shell without OSC 133 integration never reports a per-command exit status; once the whole
    // pane exits, its own exit code is at least as informative a fallback as leaving this `None`.
    let last_exit_status = last_exit_status.or(pane.exited);
    let (cwd, cwd_host, cwd_source) = resolve_cwd(pane, semantic.cwd.as_ref(), inspector);
    let cwd_changed = !cwd_unchanged(&cwd, &pane.runtime.cwd) || cwd_host != pane.runtime.cwd_host;
    let display_path_missing = cwd.is_some() && pane.runtime.display_path.is_none();
    let display_path = if cwd_changed || display_path_missing {
        match (cwd.as_deref(), cwd_host.as_deref()) {
            (Some(cwd), None) => Some(crate::platform::paths::display_cwd(cwd)),
            // A nested remote cwd belongs to a host whose filesystem and home directory the session
            // server cannot inspect, so preserve the reported absolute spelling.
            (Some(cwd), Some(_)) => Some(cwd.to_string()),
            (None, _) => None,
        }
    } else {
        pane.runtime.display_path.clone()
    };
    // Local paths only: a `cwd_host` directory lives on a machine whose `.git` this server cannot
    // stat, and reporting this host's answer for it would attribute one repository's branch to
    // another machine's directory.
    let git_stale = pane
        .last_git_read
        .is_none_or(|at| at.elapsed() >= GIT_REFRESH);
    let (project_root, git_branch) = if cwd_changed || git_stale {
        pane.last_git_read = Some(Instant::now());
        match (cwd.as_deref(), cwd_host.as_deref()) {
            (Some(cwd), None) => {
                let root = crate::platform::paths::discover_project_root(cwd);
                let branch = root
                    .as_deref()
                    .and_then(crate::platform::paths::head_branch);
                (root, branch)
            }
            _ => (None, None),
        }
    } else {
        (
            pane.runtime.project_root.clone(),
            pane.runtime.git_branch.clone(),
        )
    };
    let foreground_program = semantic
        .executable
        .as_deref()
        .map(str::to_string)
        .or_else(|| {
            pane.pty
                .as_ref()
                .and_then(|pty| inspector.foreground_program(pty))
        });
    // How the foreground program was invoked - where it lives when its name cannot find it, and
    // the arguments it was given. Read afresh every poll while something is running, because the
    // program name is not enough to tell two runs apart: `sleep 1 && sleep 444` never changes it,
    // and a cached answer would describe the run that already finished. A pane sitting at its
    // prompt is not running anything worth replaying, so it reads nothing at all.
    let (foreground_executable, foreground_arguments) = match command_phase {
        PaneCommandPhase::Executing | PaneCommandPhase::Unknown => {
            foreground_launch(pane, inspector, foreground_program.as_deref())
        }
        PaneCommandPhase::Prompt | PaneCommandPhase::Input | PaneCommandPhase::Completed { .. } => {
            (None, Vec::new())
        }
    };
    // Agent detection sweeps every process on the host (it has to, to find this pane's
    // process-group members), so running it on every poll cost ~2% of a core per idle pane. The
    // foreground program and command phase above are already known and change whenever the pane
    // starts running something new, so an unchanged pair means the sweep would rediscover the
    // cached answer. AGENT_DETECT_REFRESH still re-sweeps periodically, catching a wrapped process
    // that appears inside an unchanged foreground program.
    // A pane whose program enumerates its own sessions is authoritative. Screen detection can only
    // ever see the session in view, so scraping such a pane would answer a question the publisher
    // has already answered better - and would answer it about the wrong session.
    let detected_agent = if !pane.runtime.rows.is_empty() {
        pane.agent.hold = None;
        let aggregate = crate::session::protocol::aggregate_row_state(&pane.runtime.rows);
        // If an agent kind was previously detected, update its aggregate state.
        // If nothing was detected, detected_agent remains None.
        pane.runtime
            .detected_agent
            .as_ref()
            .zip(aggregate)
            .map(|(previous, state)| DetectedAgent {
                kind: previous.kind,
                state,
            })
    } else if detect_agent {
        let probe = AgentProbe {
            foreground_program: foreground_program.clone(),
            command_phase,
        };
        // A different foreground program is a different program: whatever was held describes
        // something that is no longer running behind this pane.
        if pane.agent.probe.as_ref().is_some_and(|last| *last != probe) {
            pane.agent.hold = None;
        }
        let stale = pane.agent.probe.as_ref().is_none_or(|last| *last != probe)
            || pane
                .agent
                .detected_at
                .is_none_or(|at| at.elapsed() >= AGENT_DETECT_REFRESH);
        if stale {
            pane.agent.probe = Some(probe);
            pane.agent.detected_at = Some(Instant::now());
            let observed = detect_pane_agent(pane, inspector, foreground_program.as_deref(), scan);
            resolve_detected_agent(observed, &mut pane.agent.hold, Instant::now())
        } else {
            // The cached value is already resolved, and a skipped sweep observed nothing, so it
            // must not refresh the hold's age - the cap measures from real evidence only.
            pane.runtime.detected_agent.clone()
        }
    } else {
        pane.runtime.detected_agent.clone()
    };

    let work_started_at = next_work_started_at(
        &pane.runtime,
        pane.runtime.status.as_ref(),
        detected_agent.as_ref(),
    );
    let candidate = PaneRuntimeState {
        cwd,
        cwd_host,
        display_path,
        project_root,
        git_branch,
        cwd_source,
        command_phase,
        foreground_program,
        foreground_executable,
        foreground_arguments,
        last_exit_status,
        status: pane.runtime.status.clone(),
        detected_agent,
        work_started_at,
        // Owned by `report_pane_rows`, which is the only writer; a recompute carries them.
        rows: pane.runtime.rows.clone(),
        sequence: pane.runtime.sequence,
    };
    let changed = !cwd_unchanged(&candidate.cwd, &pane.runtime.cwd)
        || candidate.cwd_host != pane.runtime.cwd_host
        || candidate.display_path != pane.runtime.display_path
        || candidate.project_root != pane.runtime.project_root
        || candidate.git_branch != pane.runtime.git_branch
        || candidate.cwd_source != pane.runtime.cwd_source
        || candidate.command_phase != pane.runtime.command_phase
        || candidate.foreground_program != pane.runtime.foreground_program
        || candidate.foreground_executable != pane.runtime.foreground_executable
        || candidate.foreground_arguments != pane.runtime.foreground_arguments
        || candidate.last_exit_status != pane.runtime.last_exit_status
        || candidate.status != pane.runtime.status
        || candidate.detected_agent != pane.runtime.detected_agent
        || candidate.work_started_at != pane.runtime.work_started_at;
    PaneRuntimeState {
        sequence: if changed {
            pane.runtime.sequence.wrapping_add(1)
        } else {
            pane.runtime.sequence
        },
        ..candidate
    }
}

/// How to launch the foreground program again: where it lives, and what it was given.
///
/// The path is empty for the ordinary pane, because a program on `PATH` is reachable by name and a
/// name is what profiles should store - pinning `/usr/bin/nvim` into every capture would make
/// profiles machine-specific for no gain. Arguments are reported whenever they can be read, since
/// a name never carries them: `claude` and `claude --dangerously-skip-permissions` are the same
/// executable and very different panes.
///
/// Both are trusted only when the inspected process is the program the pane reports running. Shell
/// integration reports the command word the shell started while the inspector reports whatever
/// holds the terminal now, and for wrappers, `npx`-style runners, and shell functions those are
/// different programs whose arguments belong to neither.
fn foreground_launch(
    pane: &mut ServerPane,
    inspector: &impl ProcessInspector,
    foreground_program: Option<&str>,
) -> (Option<String>, Vec<String>) {
    let empty = (None, Vec::new());
    let (Some(program), Some(pty)) = (foreground_program, pane.pty.as_ref()) else {
        return empty;
    };
    let Some(launch) = inspector.foreground_launch(pty) else {
        return empty;
    };
    let identified = launch
        .executable
        .as_deref()
        .is_some_and(|path| same_program(program, path))
        || launch
            .argv
            .first()
            .is_some_and(|argv0| same_program(program, std::path::Path::new(argv0)));
    if !identified {
        return empty;
    }
    let executable = launch
        .executable
        .filter(|_| !program_is_on_path(&mut pane.program_on_path, program))
        .map(|path| path.to_string_lossy().into_owned());
    (executable, replayable_arguments(&launch.argv))
}

/// Whether `program` resolves on this server's `PATH`, remembering the last answer.
///
/// The lookup stats every `PATH` entry - and stats all of them for the miss, which is the case
/// that matters here - so at the poll rate it is the one part of reading a pane's invocation worth
/// not repeating. Unlike the invocation itself the answer cannot go stale between two runs of the
/// same name: it is a property of the name.
fn program_is_on_path(cached: &mut Option<(String, bool)>, program: &str) -> bool {
    if let Some((_, resolves)) = cached.as_ref().filter(|(name, _)| name == program) {
        return *resolves;
    }
    let resolves = crate::platform::command::program_exists(program);
    *cached = Some((program.to_string(), resolves));
    resolves
}

/// A process's arguments, minus `argv[0]`, when every one of them can survive being typed back at
/// a shell prompt.
///
/// A restored command is replayed by typing it, so an argument containing a control character -
/// a newline above all - would not come back as one argument but as a truncated command followed
/// by whatever the rest of the bytes happen to mean. There is no partial answer worth giving: a
/// program restored with some of its flags is a different program, so an unquotable argument
/// discards the whole vector and the pane restores as the bare program it already was.
fn replayable_arguments(argv: &[String]) -> Vec<String> {
    let arguments = argv.get(1..).unwrap_or_default();
    if arguments
        .iter()
        .any(|argument| argument.chars().any(char::is_control))
    {
        return Vec::new();
    }
    arguments.to_vec()
}

/// Whether an inspected executable path is the program the pane reports running.
fn same_program(program: &str, path: &std::path::Path) -> bool {
    use crate::platform::command::normalized_program_name;

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| normalized_program_name(name) == normalized_program_name(program))
}

/// Keep one run's wall-clock start across blocked and resumed states. The server owns this value so
/// a client can detach and reattach without turning a still-live run into a new one.
fn next_work_started_at(
    previous: &PaneRuntimeState,
    next_status: Option<&PaneStatus>,
    next_detected: Option<&DetectedAgent>,
) -> Option<u64> {
    let previous_status = crate::session::protocol::effective_agent_status(
        previous.status.as_ref(),
        previous.detected_agent.as_ref(),
    );
    let next_status = crate::session::protocol::effective_agent_status(next_status, next_detected);
    next_run_start(
        previous_status,
        previous.work_started_at,
        next_status,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

/// The run start for one subject - a pane or one of its slots - given where it was and where it is.
///
/// Quiescent ends the run; anything else continues an existing one rather than restarting it, so a
/// block and later resume read as one run. Pure in `now` so per-slot behavior is testable.
fn next_run_start(
    previous_status: Option<&str>,
    previous_started_at: Option<u64>,
    next_status: Option<&str>,
    now: u64,
) -> Option<u64> {
    if next_status.is_none() || crate::session::protocol::status_is_quiescent(next_status) {
        return None;
    }
    if previous_started_at.is_some()
        && !crate::session::protocol::status_is_quiescent(previous_status)
    {
        return previous_started_at;
    }
    Some(now)
}

/// How many rows one pane may publish. A tab bar is a human-sized list; a publisher sending
/// thousands is malfunctioning, and every row costs a sidebar row.
const MAX_PANE_ROWS: usize = 64;

/// Clean a publisher's row list and give each row the run clock the server owns.
///
/// Text is sanitized and truncated exactly as [`SessionServer::set_pane_status`] does its own -
/// this arrives from the same place and is rendered in the same rows. Rows are matched to their
/// previous selves by `id`, so a run keeps one start across reorders, title changes, and
/// block-then-resume; an id appearing for the first time starts a run, and one that disappears
/// takes its clock with it.
fn sanitize_rows(
    rows: Vec<protocol::PublishedRow>,
    previous: &[protocol::PublishedRow],
    now: u64,
) -> Vec<protocol::PublishedRow> {
    let mut active_seen = false;
    rows.into_iter()
        .filter_map(|row| {
            let id = clean_text(&row.id, PANE_STATUS_MAX_LEN)?;
            let status = clean_text(&row.status, PANE_STATUS_MAX_LEN)?;
            let previous = previous.iter().find(|candidate| candidate.id == id);
            let work_started_at = next_run_start(
                previous.map(|previous| previous.status.as_str()),
                previous.and_then(|previous| previous.work_started_at),
                Some(status.as_str()),
                now,
            );
            // At most one row is the one on screen; a publisher that marks several keeps the
            // first, since the rest cannot also be in view.
            let active = row.active && !std::mem::replace(&mut active_seen, row.active);
            Some(protocol::PublishedRow {
                // Left empty when the publisher has none yet - a session is often created, and
                // can even ask its first question, before anything has titled it. The id is not a
                // stand-in: it is an opaque handle, and rendering it would put `ses_9f2c` on
                // screen where a description belongs.
                title: clean_text(&row.title, PANE_STATUS_MAX_LEN).unwrap_or_default(),
                id,
                status,
                reason: clean_text(&row.reason.unwrap_or_default(), PANE_STATUS_REASON_MAX_LEN),
                active,
                work_started_at,
            })
        })
        .take(MAX_PANE_ROWS)
        .collect()
}

fn clean_text(value: &str, limit: usize) -> Option<String> {
    let value = tui_lipan::utils::sanitize_display_text(value)
        .trim()
        .chars()
        .take(limit)
        .collect::<String>();
    (!value.is_empty()).then_some(value)
}

/// Resolve one detection sweep into the state that leaves this process, maintaining `hold`.
///
/// Pure in `now` so the cap is testable without sleeping. The three outcomes are deliberately
/// distinct: no agent at all drops the hold, a positive observation replaces it, and an
/// observation that saw nothing reuses it until [`AGENT_HOLD_MAX`] runs out.
fn resolve_detected_agent(
    observed: Option<crate::agent_detection::AgentObservation>,
    hold: &mut Option<AgentHold>,
    now: Instant,
) -> Option<DetectedAgent> {
    let Some(observed) = observed else {
        *hold = None;
        return None;
    };
    match observed.state {
        Some(state) => {
            *hold = Some(AgentHold {
                state,
                observed_at: now,
            });
            Some(DetectedAgent {
                kind: observed.kind,
                state,
            })
        }
        None => match hold
            .filter(|held| now.duration_since(held.observed_at) < AGENT_HOLD_MAX)
            .map(|held| held.state)
        {
            Some(state) => Some(DetectedAgent {
                kind: observed.kind,
                state,
            }),
            None => {
                *hold = None;
                Some(DetectedAgent {
                    kind: observed.kind,
                    state: crate::session::protocol::DetectedAgentState::Idle,
                })
            }
        },
    }
}

fn detect_pane_agent(
    pane: &mut ServerPane,
    inspector: &impl ProcessInspector,
    foreground_program: Option<&str>,
    scan: &mut LazyProcessScan,
) -> Option<crate::agent_detection::AgentObservation> {
    let mut foreground_job = pane
        .pty
        .as_ref()
        .and_then(|pty| inspector.foreground_job_in(pty, scan.get()));
    let configured_hint = ["ROZI_AGENT", "HERDR_AGENT"].into_iter().find_map(|key| {
        pane.env
            .iter()
            .rev()
            .find_map(|(candidate, value)| (candidate == key).then(|| value.clone()))
    });
    if let Some(hint) = configured_hint {
        if let Some(process) = foreground_job
            .as_mut()
            .and_then(|job| job.processes.first_mut())
        {
            process.agent_hint.get_or_insert(hint);
        } else {
            foreground_job = Some(crate::platform::process::ForegroundJob {
                process_group_id: 0,
                processes: vec![crate::platform::process::ForegroundProcess {
                    pid: 0,
                    name: foreground_program.unwrap_or_default().to_string(),
                    executable: None,
                    argv: Vec::new(),
                    agent_hint: Some(hint),
                }],
            });
        }
    } else if foreground_job.is_none()
        && let Some(program) = foreground_program
    {
        foreground_job = Some(crate::platform::process::ForegroundJob {
            process_group_id: 0,
            processes: vec![crate::platform::process::ForegroundProcess {
                pid: 0,
                name: program.to_string(),
                executable: None,
                argv: vec![program.to_string()],
                agent_hint: None,
            }],
        });
    }
    let screen = pane.screen_without_change().snapshot();
    let title = pane.effective_title().unwrap_or_default();
    crate::agent_detection::detect(foreground_job.as_ref(), screen.as_ref(), &title)
}

/// Resolve the pane's displayable cwd per [`PaneCwdSource`]'s precedence order: a valid local or
/// remote shell report first, then the process-inspector fallback (always local by construction -
/// it reads the actual child process, never a remote one), then the pane's original launch
/// directory. An invalid report (non-UTF-8 path bytes given the JSON wire encoding, or a
/// non-absolute path) falls through to the next tier rather than being repaired, per the plan.
fn resolve_cwd(
    pane: &ServerPane,
    reported: Option<&TerminalWorkingDirectory>,
    inspector: &impl ProcessInspector,
) -> (Option<String>, Option<String>, PaneCwdSource) {
    if let Some(reported) = reported
        && let Some(path) = decode_reported_path(reported)
    {
        let host = (!is_local_host(reported.host.as_deref()))
            .then(|| reported.host.as_deref().unwrap_or_default().to_string());
        return (Some(path), host, PaneCwdSource::ShellReport);
    }
    if let Some(pty) = &pane.pty
        && let Some(path) = inspector.cwd(pty)
    {
        return (
            Some(path.to_string_lossy().to_string()),
            None,
            PaneCwdSource::ProcessInspector,
        );
    }
    (pane.cwd.clone(), None, PaneCwdSource::LaunchDirectory)
}

/// Whether two resolved cwds name the same directory, under the platform's own comparison rules
/// (case-insensitive on Windows). A pane whose shell reports `c:\users\x` where its launch
/// directory was recorded as `C:\Users\x` has not changed directory, and must not burn a sequence
/// number - and therefore a `PaneRuntimeChanged` broadcast to every client - saying it has.
fn cwd_unchanged(candidate: &Option<String>, current: &Option<String>) -> bool {
    match (candidate, current) {
        (Some(candidate), Some(current)) => crate::platform::paths::paths_equal(candidate, current),
        (None, None) => true,
        _ => false,
    }
}

/// Percent-decoded `OSC 7`/`OSC 9;9` path bytes are exposed as raw bytes by the framework (never
/// lossy-converted) so a *local* filesystem operation could handle non-UTF-8 bytes exactly; this
/// wire protocol's control channel is JSON text, though, so a report that is not valid UTF-8
/// cannot be represented here without exactly the silent lossy conversion the plan forbids -
/// treated as an invalid report and left to fall through to the next precedence tier instead.
///
/// A decodable report is then normalized (and validated) by
/// [`crate::platform::paths::normalize_reported_cwd`], which is what applies the Unix leading-`/`
/// rule and the Windows drive-letter/UNC rules.
fn decode_reported_path(reported: &TerminalWorkingDirectory) -> Option<String> {
    crate::platform::paths::normalize_reported_cwd(reported.path_str()?)
}

/// Whether an `OSC 7` `host` component names this machine (`None`/empty/`localhost` always count
/// as local). Falls back to treating an unresolvable local hostname as "not local" - the safer
/// direction, since it only means a genuinely local report loses its `ShellReport` tier rather
/// than a remote one being mistaken for local.
fn is_local_host(host: Option<&str>) -> bool {
    match host {
        None => true,
        Some(host) if host.is_empty() || host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => {
            crate::platform::user::hostname().is_some_and(|local| local.eq_ignore_ascii_case(host))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pane() -> ServerPane {
        ServerPane {
            generation: 1,
            title: None,
            cwd: Some("/launch/dir".to_string()),
            launch: None,
            keep_open: false,
            command_completed: false,
            cell: tui_lipan::TerminalCellSize::default(),
            shell: Vec::new(),
            env: Vec::new(),
            palette: WirePalette::from(TerminalColorPalette::default()),
            pty: None,
            terminal: TerminalScreen::new(24, 80, 100),
            content_generation: 0,
            cols: 80,
            rows: 24,
            exited: None,
            log: None,
            runtime: PaneRuntimeState::default(),
            agent: AgentScratch::default(),
            program_on_path: None,
            last_git_read: None,
            initial_cursor_report_primed: false,
        }
    }

    struct StubInspector;
    impl ProcessInspector for StubInspector {
        fn cwd(&self, _pty: &TerminalPty) -> Option<std::path::PathBuf> {
            None
        }
        fn foreground_program(&self, _pty: &TerminalPty) -> Option<String> {
            None
        }
    }

    /// A command is replayed by typing it back at a prompt, so an argument carrying a control
    /// character would not return as one argument at all. Half an invocation is worse than none:
    /// a program restored with some of its flags is a different program.
    #[test]
    fn arguments_that_could_not_be_typed_back_discard_the_whole_invocation() {
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| word.to_string())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            replayable_arguments(&argv(&["claude", "--model", "opus"])),
            argv(&["--model", "opus"]),
            "argv[0] is the program, not one of its arguments"
        );
        assert!(replayable_arguments(&argv(&["claude"])).is_empty());
        assert!(replayable_arguments(&[]).is_empty());
        assert_eq!(
            replayable_arguments(&argv(&["grep", "-r", ""])),
            argv(&["-r", ""]),
            "an empty argument is quotable, so it is not a reason to drop the rest"
        );
        assert!(
            replayable_arguments(&argv(&["sh", "-c", "one\ntwo"])).is_empty(),
            "a newline would end the replayed command line early"
        );
    }

    /// The path only travels when it is the same program the pane reports; a wrapper or a shell
    /// function leaves the inspector pointing at something else entirely, and replaying *that*
    /// would start a program the user never ran.
    #[test]
    fn an_inspected_path_is_only_trusted_for_the_reported_program() {
        assert!(same_program(
            "opencode-tui",
            std::path::Path::new("/build/target/release/opencode-tui")
        ));
        assert!(
            same_program("Opencode-TUI", std::path::Path::new("/opt/opencode-tui")),
            "spelling differences must not make one program look like two"
        );
        assert!(!same_program(
            "opencode-tui",
            std::path::Path::new("/usr/bin/bash")
        ));
    }

    /// Detection walks every process on the host, so a poll where nothing changed must skip it —
    /// that walk, repeated per pane at the poll rate, was ~2% of a core per idle pane.
    ///
    /// `AgentScratch::detected_at` only advances when detection actually runs, so it is the observable
    /// for the gate. (Asserting on `LazyProcessScan::captured` would pass vacuously here: the test
    /// pane has no PTY, so the walk is unreachable either way.)
    #[test]
    fn an_unchanged_pane_skips_detection() {
        let mut pane = make_pane();
        let mut scan = LazyProcessScan::default();

        // First pass has no cached probe, so detection must run.
        pane.runtime = compute_runtime_state(&mut pane, &StubInspector, true, &mut scan);
        let first = pane.agent.detected_at.expect("first poll must detect");

        // Foreground program and command phase unchanged: detection must be skipped.
        pane.runtime = compute_runtime_state(&mut pane, &StubInspector, true, &mut scan);
        assert_eq!(
            pane.agent.detected_at,
            Some(first),
            "an unchanged pane must not re-run detection"
        );

        // A new foreground program is a real change and must detect again.
        pane.runtime.foreground_program = Some("claude".to_string());
        pane.agent.probe = Some(AgentProbe {
            foreground_program: Some("claude".to_string()),
            command_phase: pane.runtime.command_phase,
        });
        pane.agent.probe = None;
        pane.runtime = compute_runtime_state(&mut pane, &StubInspector, true, &mut scan);
        assert_ne!(
            pane.agent.detected_at,
            Some(first),
            "a changed foreground must re-run detection"
        );
    }

    /// The walk is captured once per cycle and reused, so cost stops scaling with pane count.
    #[test]
    fn panes_in_one_cycle_share_a_single_process_table_walk() {
        let mut scan = LazyProcessScan::default();
        assert!(!scan.captured(), "capture must be lazy");

        let first = scan.get() as *const _;
        let second = scan.get() as *const _;
        assert!(scan.captured());
        assert_eq!(
            first, second,
            "every pane in a cycle must read the same captured walk"
        );
    }

    #[test]
    fn falls_back_to_launch_cwd_with_no_pty_and_no_shell_report() {
        let mut pane = make_pane();
        let state = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(state.cwd, Some("/launch/dir".to_string()));
        assert_eq!(state.cwd_source, PaneCwdSource::LaunchDirectory);
        assert_eq!(state.cwd_host, None);
        assert_eq!(state.sequence, 1);
    }

    /// The `OSC 7` URI path for a directory, and the cwd it must normalize to on this platform.
    ///
    /// A POSIX absolute path is not a usable Windows directory - it is relative to the current
    /// drive - so the two platforms need different spellings rather than a shared one. The URI
    /// keeps its leading separator in both cases, which is what a shell actually emits.
    #[cfg(windows)]
    fn osc7_dir(name: &str) -> (String, String) {
        (
            format!("/C:/{name}"),
            format!(r"C:\{}", name.replace('/', r"\")),
        )
    }
    #[cfg(not(windows))]
    fn osc7_dir(name: &str) -> (String, String) {
        (format!("/{name}"), format!("/{name}"))
    }

    #[test]
    fn a_local_osc7_report_wins_over_launch_cwd() {
        let (uri_path, expected) = osc7_dir("reported/dir");
        let mut pane = make_pane();
        pane.screen_mut()
            .process_bytes(format!("\x1b]7;file://localhost{uri_path}\x1b\\").as_bytes());
        let state = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(state.cwd, Some(expected));
        assert_eq!(state.cwd_source, PaneCwdSource::ShellReport);
        assert_eq!(state.cwd_host, None);
    }

    #[test]
    fn a_remote_osc7_report_is_displayable_but_flagged_with_a_host() {
        let (uri_path, expected) = osc7_dir("remote/dir");
        let mut pane = make_pane();
        pane.screen_mut()
            .process_bytes(format!("\x1b]7;file://otherhost.example{uri_path}\x1b\\").as_bytes());
        let state = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(state.cwd, Some(expected));
        assert_eq!(state.cwd_host, Some("otherhost.example".to_string()));
        assert_eq!(state.cwd_source, PaneCwdSource::ShellReport);
    }

    #[test]
    fn a_non_absolute_osc7_report_falls_through_to_launch_cwd() {
        let mut pane = make_pane();
        pane.screen_mut()
            .process_bytes(b"\x1b]7;file://localhost/relative%2fpath\x1b\\");
        // Force the report path itself to be non-absolute by using a bare relative form: OSC 7
        // URIs are always absolute per spec, so exercise the fallthrough via a directly
        // constructed report instead of a crafted escape sequence.
        let (cwd, host, source) = resolve_cwd(
            &pane,
            Some(&TerminalWorkingDirectory {
                host: None,
                path: std::sync::Arc::from(b"relative/path".as_slice()),
                source: TerminalWorkingDirectorySource::Osc7,
            }),
            &StubInspector,
        );
        assert_eq!(cwd, Some("/launch/dir".to_string()));
        assert_eq!(host, None);
        assert_eq!(source, PaneCwdSource::LaunchDirectory);
    }

    #[test]
    fn recompute_is_a_no_op_when_nothing_changed() {
        let mut pane = make_pane();
        let first = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        pane.runtime = first.clone();
        let second = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(second, first);
        assert_eq!(second.sequence, first.sequence);
    }

    #[test]
    fn recompute_preserves_status_and_only_bumps_for_other_runtime_changes() {
        let mut pane = make_pane();
        pane.runtime.status = Some(PaneStatus {
            value: "working".into(),
            reason: Some("tests".into()),
            set_at: 123,
        });
        pane.runtime.sequence = 7;

        let changed = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(changed.status, pane.runtime.status);
        assert_eq!(changed.sequence, 8);

        pane.runtime = changed.clone();
        let unchanged = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(unchanged, changed);
        assert_eq!(unchanged.sequence, 8);
    }

    #[test]
    fn completed_command_phase_sticks_the_exit_status() {
        let mut pane = make_pane();
        pane.screen_mut().process_bytes(b"\x1b]133;D;7\x1b\\");
        let state = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(
            state.command_phase,
            PaneCommandPhase::Completed {
                exit_status: Some(7)
            }
        );
        assert_eq!(state.last_exit_status, Some(7));

        pane.runtime = state;
        pane.screen_mut().process_bytes(b"\x1b]133;A\x1b\\");
        let next = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(next.command_phase, PaneCommandPhase::Prompt);
        assert_eq!(next.last_exit_status, Some(7));
    }

    #[test]
    fn agent_detection_uses_configured_hint_and_is_preserved_between_polls() {
        let mut pane = make_pane();
        pane.env.push(("ROZI_AGENT".into(), "opencode".into()));
        pane.screen_mut().process_bytes(b"esc to interrupt");

        let detected = compute_runtime_state(
            &mut pane,
            &StubInspector,
            true,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(
            detected.detected_agent,
            Some(crate::session::protocol::DetectedAgent {
                kind: crate::session::protocol::AgentKind::OpenCode,
                state: crate::session::protocol::DetectedAgentState::Working,
            })
        );

        pane.runtime = detected.clone();
        pane.env.clear();
        let event_update = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(event_update.detected_agent, detected.detected_agent);
        assert_eq!(event_update.sequence, detected.sequence);
    }

    #[test]
    fn inferred_agent_run_start_survives_block_and_resume() {
        let mut pane = make_pane();
        pane.runtime.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Working,
        });

        let working = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        let started = working
            .work_started_at
            .expect("inferred working state starts a run");
        pane.runtime = working;
        pane.runtime.detected_agent.as_mut().unwrap().state =
            crate::session::protocol::DetectedAgentState::Blocked;

        let blocked = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(blocked.work_started_at, Some(started));
        pane.runtime = blocked;
        pane.runtime.detected_agent.as_mut().unwrap().state =
            crate::session::protocol::DetectedAgentState::Working;

        let resumed = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(resumed.work_started_at, Some(started));
    }

    /// The reported bug: OpenCode draws its progress bar and interrupt hint only for the session
    /// in view, so opening a subagent erases every signal while the parent run continues. Silence
    /// must reuse the held state rather than reading as a finished run.
    #[test]
    fn an_observation_with_no_evidence_holds_the_previous_state() {
        use crate::agent_detection::AgentObservation;
        use crate::session::protocol::{AgentKind, DetectedAgentState};

        let start = Instant::now();
        let mut hold = None;
        let working = AgentObservation {
            kind: AgentKind::OpenCode,
            state: Some(DetectedAgentState::Working),
        };
        let silent = AgentObservation {
            kind: AgentKind::OpenCode,
            state: None,
        };

        let resolved = resolve_detected_agent(Some(working), &mut hold, start)
            .expect("a recognized agent always resolves to some state");
        assert_eq!(resolved.state, DetectedAgentState::Working);

        // Inside the cap, and far enough in that a naive per-poll refresh would be visible.
        let held = resolve_detected_agent(
            Some(silent),
            &mut hold,
            start + AGENT_HOLD_MAX - Duration::from_secs(1),
        )
        .expect("the agent is still recognized");
        assert_eq!(
            held.state,
            DetectedAgentState::Working,
            "a view that cannot report on the run must not end it"
        );

        // Past the cap the hold is abandoned rather than pinning `working` forever.
        let expired = resolve_detected_agent(Some(silent), &mut hold, start + AGENT_HOLD_MAX)
            .expect("the agent is still recognized");
        assert_eq!(expired.state, DetectedAgentState::Idle);
        assert!(hold.is_none(), "an expired hold is dropped, not renewed");
    }

    #[test]
    fn a_positive_idle_observation_ends_the_run_immediately() {
        use crate::agent_detection::AgentObservation;
        use crate::session::protocol::{AgentKind, DetectedAgentState};

        let start = Instant::now();
        let mut hold = None;
        resolve_detected_agent(
            Some(AgentObservation {
                kind: AgentKind::OpenCode,
                state: Some(DetectedAgentState::Working),
            }),
            &mut hold,
            start,
        );
        // Returning to the composer is evidence, not silence, so the hold must not delay the
        // finish by anything like `AGENT_HOLD_MAX`.
        let idle = resolve_detected_agent(
            Some(AgentObservation {
                kind: AgentKind::OpenCode,
                state: Some(DetectedAgentState::Idle),
            }),
            &mut hold,
            start + Duration::from_secs(1),
        )
        .expect("the agent is still recognized");
        assert_eq!(idle.state, DetectedAgentState::Idle);
    }

    #[test]
    fn losing_the_agent_drops_the_hold() {
        use crate::agent_detection::AgentObservation;
        use crate::session::protocol::{AgentKind, DetectedAgentState};

        let mut hold = None;
        resolve_detected_agent(
            Some(AgentObservation {
                kind: AgentKind::OpenCode,
                state: Some(DetectedAgentState::Working),
            }),
            &mut hold,
            Instant::now(),
        );
        assert!(resolve_detected_agent(None, &mut hold, Instant::now()).is_none());
        assert!(
            hold.is_none(),
            "a pane with no agent must not keep one held"
        );
    }

    /// A `keep_open` pane swaps its PTY in place, keeping its id and generation, so nothing else
    /// clears what was learned about the command that just exited.
    #[test]
    fn the_keep_open_shell_swap_forgets_the_previous_program() {
        let mut pane = make_pane();
        pane.runtime.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Working,
        });
        pane.agent.hold = Some(AgentHold {
            state: crate::session::protocol::DetectedAgentState::Working,
            observed_at: Instant::now(),
        });
        pane.agent.probe = Some(AgentProbe {
            foreground_program: Some("opencode".into()),
            command_phase: pane.runtime.command_phase,
        });

        // What `replace_with_keep_open_shell` does to the pane once the new PTY is live.
        pane.agent = AgentScratch::default();
        pane.runtime.detected_agent = None;
        pane.runtime.work_started_at = None;

        let next = compute_runtime_state(
            &mut pane,
            &StubInspector,
            false,
            &mut LazyProcessScan::default(),
        );
        assert_eq!(next.detected_agent, None);
        assert_eq!(next.work_started_at, None);
    }

    #[test]
    fn detected_blocked_over_reported_idle_keeps_the_server_run_timestamp() {
        let previous = PaneRuntimeState {
            status: Some(PaneStatus {
                value: "working".into(),
                reason: None,
                set_at: 1,
            }),
            work_started_at: Some(42),
            ..PaneRuntimeState::default()
        };
        let idle = PaneStatus {
            value: "idle".into(),
            reason: None,
            set_at: 2,
        };
        let blocked = DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Blocked,
        };

        assert_eq!(
            next_work_started_at(&previous, Some(&idle), Some(&blocked)),
            Some(42)
        );
    }
}
