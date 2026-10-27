use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::lifecycle::{find_pane_in_namespace, find_pane_in_namespace_mut};
use crate::pane::pty_events::{
    failed_rows_notice, finished_rows_notice, maybe_notify_pane_failure, maybe_notify_pane_status,
    maybe_notify_rows_done, maybe_notify_rows_failed,
};
use crate::session::protocol::PaneRuntimeState;
use crate::state::PaneId;
use crate::update::session::control_replies::{flush_attachment_replay_input, flush_replay_input};

pub(crate) fn status_is_active_run(value: Option<&str>) -> bool {
    value.is_some() && !crate::session::protocol::status_ends_run(value)
}

/// The alertable transitions a pane or a summary can report: its agent started waiting on the
/// user, a run ended, or something failed that had not failed before.
pub(crate) struct AgentEdges {
    pub(crate) became_blocked: bool,
    pub(crate) finished: bool,
    pub(crate) failed: bool,
}

/// The alertable transitions between two summaries of one agent, for a monitor that only ever
/// sees snapshots.
///
/// A finished run is read from the server's completion count rather than from a working -> idle
/// pair, so a run that started and ended between two polls still announces itself. An agent with
/// no earlier summary is a first sighting: it reports a standing prompt by this rule, and the
/// caller keeps a freshly connected source quiet by seeding its baseline without alerting.
pub(crate) fn summary_edges(
    previous: Option<&crate::session::protocol::AgentSummary>,
    current: &crate::session::protocol::AgentSummary,
) -> AgentEdges {
    use crate::session::protocol::AgentState;
    AgentEdges {
        became_blocked: current.state == AgentState::Blocked
            && previous.is_none_or(|previous| previous.state != AgentState::Blocked),
        finished: previous.is_some_and(|previous| current.completions > previous.completions),
        failed: current.state == AgentState::Error
            && previous.is_none_or(|previous| previous.state != AgentState::Error),
    }
}

/// React to a pane's status transition. `previous_age` is how long the outgoing status had held,
/// sampled before it was overwritten. `agent_finished` says whether the pane's own agent finished a
/// run in this update, by the server's count (see [`finished_records`]).
///
/// Stamps the local fallback and banks the length of an active run as it ends, so a finished run
/// can report what it cost. Whether an agent's finish has been seen is not kept here: that is
/// [`crate::state::AgentAttention`]'s, for every agent alike. A pane whose status names no agent - a
/// `rozi status` activity - keeps its own "unseen finish" flag: armed when an active status ends
/// successfully, disarmed when it becomes active or fails again, and cleared when the pane is
/// looked at.
pub(crate) fn update_agent_status_edge(
    pane: &mut crate::pane::TerminalPane,
    previous: Option<&str>,
    previous_age: Option<std::time::Duration>,
    previous_blocked: bool,
    agent_finished: bool,
) -> AgentEdges {
    let current = pane.agent_status();
    let current = current.as_deref();
    use crate::session::protocol::ActivityState;
    let generic = !pane.has_agent();
    let previous_activity = previous.map(ActivityState::from_status);
    let current_activity = current.map(ActivityState::from_status);
    if current != previous {
        let ended = if generic {
            previous_activity.is_some_and(|state| !state.is_terminal())
                && current_activity.is_some_and(ActivityState::is_terminal)
        } else {
            status_is_active_run(previous) && crate::session::protocol::status_ends_run(current)
        };
        if ended {
            pane.last_run = previous_age;
        }
        pane.status_since = Some(std::time::Instant::now());
    }
    let finished = if generic {
        let finished = previous_activity.is_some_and(|state| !state.is_terminal())
            && current_activity.is_some_and(ActivityState::is_successful_terminal);
        if current_activity.is_some_and(|state| !state.is_successful_terminal()) {
            pane.finished_unseen = false;
        } else if finished {
            pane.finished_unseen = true;
        }
        finished
    } else {
        pane.finished_unseen = false;
        agent_finished
    };
    AgentEdges {
        became_blocked: !previous_blocked && pane.is_blocked(),
        finished,
        // Read across the whole pane by `apply_pane_runtime_state`, which sees every row.
        failed: false,
    }
}

/// Which of a pane's agents finished a run between two sets of its records: the pane's own agent
/// (`None`) and each published row by key. Read from the server's completion counts, so a finish
/// is a finish whether or not this client saw the agent working first.
pub(crate) fn finished_records(
    previous: &[crate::session::protocol::AgentRecord],
    current: &[crate::session::protocol::AgentRecord],
) -> Vec<Option<crate::session::protocol::RowKey>> {
    current
        .iter()
        .filter(|record| {
            previous.iter().any(|earlier| {
                earlier.reference == record.reference && record.completions > earlier.completions
            })
        })
        .map(|record| record.reference.slot.clone())
        .collect()
}

pub(crate) struct AppliedPaneRuntime {
    pub(crate) finished_rows: Vec<crate::session::protocol::RowKey>,
    /// Rows the publisher is not showing that failed in this update. A failure on screen, or of
    /// the pane's own agent, is [`AgentEdges::failed`] instead.
    pub(crate) failed_rows: Vec<crate::session::protocol::RowKey>,
    pub(crate) edges: AgentEdges,
    pub(crate) previous_status: Option<crate::session::protocol::PaneStatus>,
    pub(crate) current_status: Option<crate::session::protocol::PaneStatus>,
}

pub(crate) fn apply_pane_runtime_state(
    pane: &mut crate::state::Pane,
    agents: Vec<crate::session::protocol::AgentRecord>,
    state: PaneRuntimeState,
) -> AppliedPaneRuntime {
    let finished = finished_records(&pane.agents, &agents);
    // Read under the old records, which name the rows the old runtime published.
    let previous_blocked = pane.agent_blocked();
    pane.agents = agents;
    let previous_status = pane.terminal.reported_status.clone();
    let previous_agent_status = pane.terminal.agent_status();
    let previous_failures = pane.failures();
    // Sampled before the incoming runtime state overwrites the status it dates.
    let previous_age = pane.terminal.status_age();
    // Replacing a detected agent establishes a new lifecycle baseline: its loading screen is
    // not the previous run finishing. A first detection still alerts if it needs attention.
    let relaunched =
        state.agent_launch != pane.terminal.agent_launch && pane.terminal.detected_agent.is_some();
    let (previous_agent_status, previous_age, previous_blocked) = if relaunched {
        (None, None, false)
    } else {
        (previous_agent_status, previous_age, previous_blocked)
    };
    pane.terminal.agent_launch = state.agent_launch;
    pane.terminal.runtime_sequence = state.sequence;
    pane.terminal.cwd = state.cwd;
    pane.terminal.cwd_host = state.cwd_host;
    pane.terminal.display_path = state.display_path;
    pane.terminal.project_root = state.project_root;
    pane.terminal.git_branch = state.git_branch;
    pane.terminal.repository = state.repository;
    pane.terminal.foreground_program = state.foreground_program;
    pane.terminal.foreground_programs = state.foreground_programs.into_vec();
    pane.terminal.foreground_executable = state.foreground_executable;
    pane.terminal.foreground_arguments = state.foreground_arguments;
    pane.terminal.foreground_pid = state.foreground_pid;
    pane.terminal.command_phase = state.command_phase;
    pane.terminal.last_exit_status = state.last_exit_status;
    pane.terminal.reported_status = state.status;
    pane.terminal.recording = state.recording;
    pane.terminal.detected_agent = state.detected_agent;
    pane.terminal.work_started_at = state.work_started_at;
    let mut finished_rows = pane.terminal.apply_rows(state.rows, state.integration);
    let (failed_rows, failed_here): (Vec<_>, Vec<_>) = pane
        .note_failures(&previous_failures)
        .into_iter()
        .partition(|failure| {
            failure.slot.as_ref().is_some_and(|key| {
                pane.terminal
                    .published_rows
                    .iter()
                    .any(|row| row.has_key(key) && !row.active)
            })
        });
    let mut failed_rows: Vec<_> = failed_rows
        .into_iter()
        .filter_map(|failure| failure.slot)
        .collect();
    finished_rows.extend(finished.iter().flatten().cloned());
    let mut edges = update_agent_status_edge(
        &mut pane.terminal,
        previous_agent_status.as_deref(),
        previous_age,
        previous_blocked,
        finished.contains(&None),
    );
    // A conversation the hooks do not follow can block too; it is news all the same.
    edges.became_blocked = !previous_blocked && pane.agent_blocked();
    edges.failed = !failed_here.is_empty();
    if relaunched {
        edges.finished = false;
        edges.became_blocked = false;
        edges.failed = false;
        failed_rows.clear();
    }
    AppliedPaneRuntime {
        finished_rows,
        failed_rows,
        edges,
        previous_status,
        current_status: pane.terminal.reported_status.clone(),
    }
}

pub(crate) fn pane_runtime_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    agents: Vec<crate::session::protocol::AgentRecord>,
    state: PaneRuntimeState,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if local {
            return Update::none();
        }
        let input_ready = state.command_phase == crate::session::protocol::PaneCommandPhase::Input;
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            if let Some(pane) = attachment.find_pane_mut(pane_id)
                && pane.pty_generation == generation
                && state.sequence > pane.terminal.runtime_sequence
            {
                apply_pane_runtime_state(pane, agents.clone(), state);
            }
            if input_ready {
                flush_attachment_replay_input(attachment, pane_id, generation);
            }
        }
        ctx.state
            .agent_attention
            .observe_records(&agents, crate::state::FirstSighting::New);
        return Update::none();
    }
    let input_ready = state.command_phase == crate::session::protocol::PaneCommandPhase::Input;
    let mut transition = None;
    let mut edges = None;
    let mut title = None;
    let mut reported_status = None;
    let mut finished_rows = Vec::new();
    let mut failed_rows = Vec::new();
    let mut recording_changed = None;
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
        && pane.pty_generation == generation
        && state.sequence > pane.terminal.runtime_sequence
    {
        title = Some(pane.display_title(None));
        if pane.terminal.recording != state.recording {
            recording_changed = Some(state.recording);
        }
        let applied = apply_pane_runtime_state(pane, agents.clone(), state);
        reported_status = applied.current_status.clone();
        finished_rows = applied.finished_rows;
        failed_rows = applied.failed_rows;
        edges = Some(applied.edges);
        if applied.previous_status != applied.current_status {
            transition = Some((
                applied.previous_status,
                applied.current_status,
                title.clone().expect("matched pane has a title"),
            ));
        }
    }
    ctx.state
        .agent_attention
        .observe_records(&agents, crate::state::FirstSighting::New);
    if let Some(recording) = recording_changed {
        crate::ops::recording::runtime_recording_changed(&mut ctx.state, epoch, pane_id, recording);
        // Start/Stop pane recording reads as the focused pane's flag, and Mark needs it set.
        ctx.state.commands_dirty = true;
    }
    if let Some((previous, current, _title)) = transition {
        crate::events::emit_with_controller_hooks(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::PaneStatusChanged,
                vec![
                    ("pane", pane_id.to_string()),
                    (
                        "status",
                        current
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "focused",
                        (ctx.state.current().focused_pane == Some(pane_id)).to_string(),
                    ),
                    (
                        "reason",
                        current
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_status",
                        previous
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_reason",
                        previous
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                ],
            ),
        );
    }
    // A row the publisher is not showing finished. Attending the pane cannot have acknowledged
    // it - the user was looking at a different tab of the same program - so it alerts regardless,
    // naming the rows that finished rather than the pane, whose title is the one on screen.
    if !finished_rows.is_empty() && title.is_some() {
        let notice = find_pane_in_namespace(&ctx.state, pane_id, local)
            .and_then(|pane| finished_rows_notice(pane, &finished_rows));
        if let Some(notice) = notice {
            if !ctx.state.do_not_disturb {
                maybe_notify_rows_done(&ctx.state.config, ctx.state.is_controller(), &notice);
            }
            if ctx.state.is_controller() {
                crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Done);
            }
        }
    }
    // Like a finished row, a failed row the publisher is not showing alerts whether or not the
    // pane is attended, and names the row rather than the pane.
    if !failed_rows.is_empty() && title.is_some() {
        let notice = find_pane_in_namespace(&ctx.state, pane_id, local)
            .and_then(|pane| failed_rows_notice(pane, &failed_rows));
        if let Some(notice) = notice {
            if !ctx.state.do_not_disturb {
                maybe_notify_rows_failed(&ctx.state.config, ctx.state.is_controller(), &notice);
            }
            if ctx.state.is_controller() {
                crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Error);
            }
        }
    }
    if let (Some(edges), Some(title)) = (&edges, &title)
        && edges.failed
    {
        let attended = ctx.state.is_pane_attended(pane_id);
        if !ctx.state.do_not_disturb {
            maybe_notify_pane_failure(
                &ctx.state.config,
                ctx.state.is_controller(),
                attended,
                pane_id,
                title,
                reported_status.as_ref(),
            );
        }
        if ctx.state.is_controller() && !attended {
            crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Error);
        }
    }
    if let (Some(edges), Some(title)) = (edges, title)
        && (edges.became_blocked || edges.finished)
    {
        let attended = ctx.state.is_pane_attended(pane_id);
        if !ctx.state.do_not_disturb {
            maybe_notify_pane_status(
                &ctx.state.config,
                ctx.state.is_controller(),
                attended,
                pane_id,
                &title,
                crate::pane::pty_events::PaneStatusNotification {
                    blocked: edges.became_blocked,
                    done: edges.finished,
                    reported_status: reported_status.as_ref(),
                },
            );
        }
        if ctx.state.is_controller() && !attended {
            if edges.became_blocked {
                crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Blocked);
            }
            if edges.finished {
                crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Done);
            }
        }
    }
    // The shell finished rendering its prompt: deliver queued replay input now, so readline
    // echoes it exactly once at the prompt (see `flush_replay_input`).
    if input_ready {
        flush_replay_input(ctx, pane_id, generation);
    }
    // An agent that just started working is the moment a row first gains an elapsed time, and the
    // Agents tab may already be open with nothing ticking.
    crate::update::sidebar::arm_agent_tick(ctx);
    Update::full()
}

pub(crate) fn program_status_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    initial: bool,
    snapshot: tui_lipan::ProgramStatusSnapshot,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        apply_background_program_status(
            &mut ctx.state,
            epoch,
            pane_id,
            local,
            generation,
            snapshot,
        );
        return Update::none();
    }
    let attended = ctx.state.is_pane_attended(pane_id);
    let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local).filter(|pane| {
        pane.pty_generation == generation
            && snapshot.revision > pane.terminal.program_status.revision
    }) else {
        return Update::none();
    };
    if initial {
        pane.terminal.program_status = snapshot;
        pane.terminal.program_status_attention = false;
        return Update::full();
    }
    let alert = apply_program_status(pane, snapshot, attended);
    if let Some(alert) = alert {
        notify_program_status(ctx, pane_id, local, attended, alert);
    }
    Update::full()
}

fn apply_background_program_status(
    state: &mut crate::state::State,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    snapshot: tui_lipan::ProgramStatusSnapshot,
) {
    if !local
        && let Some(attachment) = state.background.get_mut(&epoch)
        && let Some(pane) = attachment.find_pane_mut(pane_id)
        && pane.pty_generation == generation
        && snapshot.revision > pane.terminal.program_status.revision
    {
        pane.terminal.program_status = snapshot;
    }
}

fn notify_program_status(
    ctx: &mut Context<AppRoot>,
    pane_id: PaneId,
    local: bool,
    attended: bool,
    alert: ProgramStatusAlert,
) {
    let (title, state, message) = alert;
    let Some(cue) = program_status_alert_cue(
        &ctx.state.config,
        ctx.state.is_controller(),
        attended,
        ctx.state.do_not_disturb,
        state,
    ) else {
        return;
    };
    let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local) else {
        return;
    };
    if pane
        .terminal
        .program_status_notification_at
        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(5))
    {
        return;
    }
    let notified = crate::pane::pty_events::maybe_notify_program_status(
        &ctx.state.config,
        ctx.state.is_controller(),
        attended,
        pane_id,
        &title,
        state,
        message.as_deref(),
    );
    let sounded = crate::ops::sound::cue(ctx, cue);
    if (notified || sounded)
        && let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
    {
        pane.terminal.program_status_notification_at = Some(std::time::Instant::now());
    }
}

/// Eligibility precedes the cooldown, so an observed but suppressed alert cannot consume it.
fn program_status_alert_cue(
    config: &crate::config::Config,
    controller: bool,
    attended: bool,
    dnd: bool,
    state: tui_lipan::ProgramStatusState,
) -> Option<crate::platform::sound::Cue> {
    if !controller || attended || dnd {
        return None;
    }
    let cue = if state == tui_lipan::ProgramStatusState::Blocked {
        crate::platform::sound::Cue::Blocked
    } else {
        crate::platform::sound::Cue::Error
    };
    let desktop = if state == tui_lipan::ProgramStatusState::Error {
        crate::pane::pty_events::notifications::should_notify_pane_failure(
            config, controller, attended,
        )
    } else {
        crate::pane::pty_events::notifications::should_notify_pane_status(
            config, controller, attended, true, false,
        )
    };
    let sound = config.sounds.enabled && config.sounds.enabled_for(cue);
    (desktop || sound).then_some(cue)
}

fn actionable_program_status(state: tui_lipan::ProgramStatusState) -> bool {
    matches!(
        state,
        tui_lipan::ProgramStatusState::Blocked | tui_lipan::ProgramStatusState::Error
    )
}

type ProgramStatusAlert = (
    String,
    tui_lipan::ProgramStatusState,
    Option<std::sync::Arc<str>>,
);

fn apply_program_status(
    pane: &mut crate::state::Pane,
    snapshot: tui_lipan::ProgramStatusSnapshot,
    attended: bool,
) -> Option<ProgramStatusAlert> {
    let previous = crate::session::protocol::program_status_summary(&pane.terminal.program_status)
        .map(|r| (r.id.clone(), r.state));
    let current = crate::session::protocol::program_status_summary(&snapshot);
    let mut alert = None;
    if let Some(record) = current.filter(|r| actionable_program_status(r.state)) {
        if previous.as_ref() != Some(&(record.id.clone(), record.state)) {
            pane.terminal.program_status_attention = !attended;
            alert = Some((pane.display_title(None), record.state, record.msg.clone()));
        }
    } else {
        pane.terminal.program_status_attention = false;
    }
    pane.terminal.program_status = snapshot;
    alert
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{AgentIdentity, DetectedAgent, DetectedAgentState};

    #[test]
    fn suppressed_osc_alerts_leave_cooldown_available_for_unattended_error() {
        use crate::Msg;
        use crate::state::Pane;
        use tui_lipan::TestBackend;
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                for suppressed in [
                    "attended",
                    "dnd",
                    "follower",
                    "disabled",
                    "initial",
                    "sound-throttled",
                ] {
                    let mut backend = TestBackend::new(AppRoot::default());
                    let mut pane = Pane::new(7, 100, FloatRect::default());
                    pane.pty_generation = 4;
                    {
                        let state = backend.state_mut();
                        state.current_mut().workspaces[0].panes = vec![pane];
                        state.current_mut().focused_pane = (suppressed == "attended").then_some(7);
                        state.window_focused = true;
                        state.do_not_disturb = suppressed == "dnd";
                        // Exercise the real sound dispatch without desktop popups or audible playback.
                        state.config.notifications.enabled = false;
                        state.config.sounds.enabled = suppressed != "disabled";
                        state.config.sounds.player =
                            Some("/nonexistent-rozi-test-sound-player".into());
                        let mut shared = crate::state::SharedSessionState::new(1);
                        shared.controller = Some(if suppressed == "follower" { 2 } else { 1 });
                        state.current_mut().shared = Some(shared);
                        if suppressed == "sound-throttled" {
                            state.sound_cues.insert(
                                crate::platform::sound::Cue::Blocked,
                                std::time::Instant::now(),
                            );
                        }
                    }
                    let epoch = backend.state().runtime_epoch;
                    let mut screen = tui_lipan::prelude::TerminalScreen::new(24, 80, 100);
                    screen.set_program_status_enabled(true);
                    screen.process_bytes(b"\x1b]7501;state=blocked\x07");
                    let update = |initial, snapshot| Msg::SessionProgramStatusChanged {
                        epoch,
                        pane_id: 7,
                        local: false,
                        generation: 4,
                        initial,
                        snapshot,
                    };
                    backend
                        .dispatch(update(
                            suppressed == "initial",
                            screen.program_status_snapshot(),
                        ))
                        .unwrap();
                    assert!(
                        backend.state().current().workspaces[0].panes[0]
                            .terminal
                            .program_status_notification_at
                            .is_none(),
                        "{suppressed}"
                    );
                    assert!(
                        !backend
                            .state()
                            .sound_cues
                            .contains_key(&crate::platform::sound::Cue::Error),
                        "{suppressed}"
                    );
                    assert_eq!(
                        backend
                            .state()
                            .sound_cues
                            .contains_key(&crate::platform::sound::Cue::Blocked),
                        suppressed == "sound-throttled"
                    );
                    if suppressed == "initial" {
                        assert!(
                            !backend.state().current().workspaces[0].panes[0]
                                .terminal
                                .program_status_attention
                        );
                    }
                    {
                        let state = backend.state_mut();
                        state.current_mut().focused_pane = None;
                        state.do_not_disturb = false;
                        state.config.sounds.enabled = true;
                        state.sound_cues.clear();
                        state.current_mut().shared.as_mut().unwrap().controller = Some(1);
                    }
                    // No sleep: the failure is well within the five-second OSC cooldown.
                    screen.process_bytes(b"\x1b]7501;state=error\x07");
                    backend
                        .dispatch(update(false, screen.program_status_snapshot()))
                        .unwrap();
                    assert!(
                        backend.state().current().workspaces[0].panes[0]
                            .terminal
                            .program_status_notification_at
                            .is_some(),
                        "{suppressed}"
                    );
                    assert!(
                        backend
                            .state()
                            .sound_cues
                            .contains_key(&crate::platform::sound::Cue::Error),
                        "{suppressed}"
                    );
                    assert!(
                        !backend
                            .state()
                            .sound_cues
                            .contains_key(&crate::platform::sound::Cue::Blocked)
                    );
                    // A genuinely delivered error consumes the cooldown for subsequent transitions.
                    screen.process_bytes(b"\x1b]7501;state=blocked\x07");
                    backend
                        .dispatch(update(false, screen.program_status_snapshot()))
                        .unwrap();
                    assert!(
                        !backend
                            .state()
                            .sound_cues
                            .contains_key(&crate::platform::sound::Cue::Blocked)
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn osc_desktop_eligibility_includes_policy_before_consuming_cooldown() {
        use crate::platform::sound::Cue;
        use tui_lipan::ProgramStatusState;
        let mut config = crate::config::Config::default();
        config.notifications.enabled = true;
        config.notifications.pane_blocked = true;
        config.notifications.pane_done = true;
        config.sounds.enabled = false;
        assert_eq!(
            program_status_alert_cue(&config, true, false, false, ProgramStatusState::Error),
            Some(Cue::Error)
        );
        for (controller, attended, dnd) in [
            (true, true, false),
            (true, false, true),
            (false, false, false),
        ] {
            assert_eq!(
                program_status_alert_cue(
                    &config,
                    controller,
                    attended,
                    dnd,
                    ProgramStatusState::Blocked
                ),
                None
            );
        }
        config.notifications.enabled = false;
        assert_eq!(
            program_status_alert_cue(&config, true, false, false, ProgramStatusState::Error),
            None
        );
    }

    #[test]
    fn program_status_attention_is_per_client_and_does_not_touch_agents() {
        let mut screen = TerminalScreen::new(24, 80, 100);
        screen.set_program_status_enabled(true);
        screen.process_bytes(b"\x1b]7501;state=blocked:app=codex\x07");
        let blocked = screen.program_status_snapshot();
        let mut a = crate::state::Pane::new(1, 100, FloatRect::default());
        let mut b = crate::state::Pane::new(1, 100, FloatRect::default());
        a.terminal.finished_unseen = true;
        assert!(apply_program_status(&mut a, blocked.clone(), false).is_some());
        assert!(apply_program_status(&mut b, blocked.clone(), false).is_some());
        assert!(a.terminal.program_status_attention && b.terminal.program_status_attention);
        a.terminal.program_status_attention = false;
        assert!(apply_program_status(&mut a, blocked.clone(), false).is_none());
        assert!(!a.terminal.program_status_attention);
        assert!(b.terminal.program_status_attention);
        assert!(a.terminal.finished_unseen);
        assert!(a.agent_runtimes().is_empty());
        screen.process_bytes(b"\x1b]7501;state=working\x07");
        apply_program_status(&mut b, screen.program_status_snapshot(), false);
        assert!(!b.terminal.program_status_attention);
        // Attach initializes the snapshot directly, so subsequent identical reports do not alert.
        let mut attached = crate::state::Pane::new(1, 100, FloatRect::default());
        attached.terminal.program_status = blocked.clone();
        assert!(apply_program_status(&mut attached, blocked, false).is_none());
        assert!(!attached.terminal.program_status_attention);
    }

    /// An agent's own `blocked` still alerts: while detection reads the agent as working, nothing
    /// else would say it needs the user.
    #[test]
    fn an_agents_own_blocked_report_alerts_while_detection_says_working() {
        let mut pane = crate::state::Pane::new(1, 100, FloatRect::default());
        pane.terminal.detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("claude", "Claude Code").into(),
            state: DetectedAgentState::Working,
        });
        let mut screen = TerminalScreen::new(24, 80, 100);
        screen.set_program_status_enabled(true);
        screen.process_bytes(b"\x1b]7501;state=blocked:app=claude-code\x07");
        assert!(apply_program_status(&mut pane, screen.program_status_snapshot(), false).is_some());
        assert!(pane.terminal.program_status_attention);
    }

    #[test]
    fn generic_terminal_edges_distinguish_success_from_failure() {
        use crate::session::protocol::PaneStatus;
        for previous in ["working", "compacting", "blocked"] {
            for current in ["done", "idle", "error"] {
                let mut pane = crate::pane::TerminalPane::new(100);
                pane.reported_status = Some(PaneStatus {
                    value: current.into(),
                    reason: None,
                    set_at: 1,
                });
                // An old completion must not survive a new failure.
                pane.finished_unseen = true;
                let age = Some(std::time::Duration::from_secs(12));
                let edges = update_agent_status_edge(&mut pane, Some(previous), age, false, false);
                assert_eq!(pane.last_run, age, "{previous} -> {current}");
                assert_eq!(
                    edges.finished,
                    current != "error",
                    "{previous} -> {current}"
                );
                assert_eq!(pane.finished_unseen, current != "error");
                let repeated =
                    update_agent_status_edge(&mut pane, Some(current), age, false, false);
                assert!(!repeated.finished);
            }
        }
    }

    #[test]
    fn a_first_detected_agent_alerts_when_it_is_blocked() {
        let mut pane = crate::state::Pane::new(1, 100, FloatRect::default());
        assert!(pane.terminal.detected_agent.is_none());
        assert_eq!(pane.terminal.agent_launch, 0);
        let runtime = PaneRuntimeState {
            agent_launch: 1,
            detected_agent: Some(DetectedAgent {
                agent: AgentIdentity::new("pi", "Pi").into(),
                state: DetectedAgentState::Blocked,
            }),
            ..PaneRuntimeState::default()
        };
        let applied = apply_pane_runtime_state(&mut pane, Vec::new(), runtime.clone());
        assert!(applied.edges.became_blocked);
        assert!(!applied.edges.finished);
        assert!(!pane.terminal.finished_unseen);
        let applied = apply_pane_runtime_state(&mut pane, Vec::new(), runtime);
        assert!(!applied.edges.became_blocked);
        assert!(!applied.edges.finished);
    }

    #[test]
    fn a_blocked_relaunch_establishes_a_baseline_without_alerts() {
        for previous in [
            DetectedAgentState::Idle,
            DetectedAgentState::Working,
            DetectedAgentState::Blocked,
        ] {
            let mut pane = crate::state::Pane::new(1, 100, FloatRect::default());
            pane.terminal.agent_launch = 1;
            pane.terminal.detected_agent = Some(DetectedAgent {
                agent: AgentIdentity::new("pi", "Pi").into(),
                state: previous,
            });
            let mut runtime = PaneRuntimeState {
                agent_launch: 2,
                detected_agent: Some(DetectedAgent {
                    agent: AgentIdentity::new("pi", "Pi").into(),
                    state: DetectedAgentState::Blocked,
                }),
                ..PaneRuntimeState::default()
            };
            let applied = apply_pane_runtime_state(&mut pane, Vec::new(), runtime.clone());
            assert!(!applied.edges.finished);
            assert!(!applied.edges.became_blocked);
            assert!(!pane.terminal.finished_unseen);
            runtime.detected_agent.as_mut().unwrap().state = DetectedAgentState::Idle;
            apply_pane_runtime_state(&mut pane, Vec::new(), runtime.clone());
            runtime.detected_agent.as_mut().unwrap().state = DetectedAgentState::Blocked;
            let applied = apply_pane_runtime_state(&mut pane, Vec::new(), runtime);
            assert!(applied.edges.became_blocked);
            assert!(!applied.edges.finished);
        }
    }
}
