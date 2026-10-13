use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::lifecycle::{find_pane_in_namespace, find_pane_in_namespace_mut};
use crate::pane::pty_events::{
    finished_rows_notice, maybe_notify_pane_status, maybe_notify_rows_done,
};
use crate::session::protocol::PaneRuntimeState;
use crate::state::PaneId;
use crate::update::session::control_replies::{flush_attachment_replay_input, flush_replay_input};

pub(crate) fn status_is(value: Option<&str>, needle: &str) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case(needle))
}

pub(crate) fn status_is_quiescent(value: Option<&str>) -> bool {
    status_is(value, crate::session::protocol::pane_status::IDLE)
        || status_is(value, crate::session::protocol::pane_status::DONE)
}

pub(crate) fn status_is_active_run(value: Option<&str>) -> bool {
    value.is_some() && !status_is_quiescent(value)
}

/// React to an agent-status transition. `previous_age` is how long the outgoing status had held,
/// sampled before it was overwritten.
///
/// Stamps the local fallback, banks the length of an active run as it ends so a finished run can
/// report what it cost, and updates the "unseen finish" pulse: armed on an active -> quiescent edge
/// (the run finished while you were looking elsewhere), disarmed the moment the agent resumes
/// working, so a spinning agent never wears a completed-dot. A `blocked` outcome is deliberately
/// left un-armed: it already has its own loud glyph, and the server-owned run timestamp keeps the
/// same timer ready for a later resume. A separate focus chokepoint clears the flag once the pane is
/// actually looked at.
pub(crate) struct AgentEdges {
    pub(crate) became_blocked: bool,
    pub(crate) finished: bool,
}

/// The two alertable transitions between one agent status and the next, computed from the status
/// strings alone.
///
/// This is the rule for any agent whose state arrives as a value rather than as a pane: a host
/// monitor's summary of a session nothing here is attached to, for instance. A pane knows more
/// about itself than its reported status says — a detected blocking prompt can outrank a stale
/// `idle` — so [`update_agent_status_edge`] keeps its own richer blocked test and takes only
/// `finished` from here.
///
/// `finished` is an *ended run*, not merely a quiescent agent: it fires on `working -> anything
/// but working or blocked`, so an agent that was already idle when it was first seen never
/// reports having just finished.
pub(crate) fn agent_status_edges(previous: Option<&str>, current: Option<&str>) -> AgentEdges {
    use crate::session::protocol::pane_status;
    AgentEdges {
        became_blocked: !status_is(previous, pane_status::BLOCKED)
            && status_is(current, pane_status::BLOCKED),
        finished: status_is(previous, pane_status::WORKING)
            && current.is_some()
            && !status_is(current, pane_status::WORKING)
            && !status_is(current, pane_status::BLOCKED),
    }
}

pub(crate) fn update_agent_status_edge(
    pane: &mut crate::pane::TerminalPane,
    previous: Option<&str>,
    previous_age: Option<std::time::Duration>,
    previous_blocked: bool,
) -> AgentEdges {
    let current = pane.agent_status();
    let current = current.as_deref();
    if current != previous {
        let generic = pane.detected_agent.is_none() && pane.agent_integration.is_none();
        let ended = if generic {
            previous.is_some_and(|s| {
                !crate::session::protocol::ActivityState::from_status(s).is_terminal()
            }) && current.is_some_and(|s| {
                crate::session::protocol::ActivityState::from_status(s).is_terminal()
            })
        } else {
            status_is_active_run(previous) && status_is_quiescent(current)
        };
        if ended {
            pane.last_run = previous_age;
        }
        pane.status_since = Some(std::time::Instant::now());
    }
    let finished = agent_status_edges(previous, current).finished;
    if status_is(current, crate::session::protocol::pane_status::WORKING) {
        pane.finished_unseen = false;
    } else if finished {
        pane.finished_unseen = true;
    }
    AgentEdges {
        became_blocked: !previous_blocked && pane.is_blocked(),
        finished,
    }
}

pub(crate) struct AppliedPaneRuntime {
    pub(crate) finished_rows: Vec<crate::session::protocol::RowKey>,
    pub(crate) edges: AgentEdges,
    pub(crate) previous_status: Option<crate::session::protocol::PaneStatus>,
    pub(crate) current_status: Option<crate::session::protocol::PaneStatus>,
}

pub(crate) fn apply_pane_runtime_state(
    pane: &mut crate::state::Pane,
    state: PaneRuntimeState,
) -> AppliedPaneRuntime {
    let previous_status = pane.terminal.reported_status.clone();
    let previous_agent_status = pane.terminal.agent_status();
    // Sampled before the incoming runtime state overwrites the status it dates.
    let previous_age = pane.terminal.status_age();
    let previous_blocked = pane.terminal.is_blocked();
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
    let finished_rows = pane.terminal.apply_rows(state.rows, state.integration);
    let mut edges = update_agent_status_edge(
        &mut pane.terminal,
        previous_agent_status.as_deref(),
        previous_age,
        previous_blocked,
    );
    if relaunched {
        edges.finished = false;
        edges.became_blocked = false;
    }
    AppliedPaneRuntime {
        finished_rows,
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
    agent_refs: Vec<crate::session::protocol::AgentRef>,
    state: PaneRuntimeState,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if local {
            return Update::none();
        }
        let at_prompt = matches!(
            state.command_phase,
            crate::session::protocol::PaneCommandPhase::Prompt
                | crate::session::protocol::PaneCommandPhase::Input
        );
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            if let Some(pane) = attachment.find_pane_mut(pane_id)
                && pane.pty_generation == generation
                && state.sequence > pane.terminal.runtime_sequence
            {
                pane.agent_refs.clone_from(&agent_refs);
                apply_pane_runtime_state(pane, state);
            }
            if at_prompt {
                flush_attachment_replay_input(attachment, pane_id, generation);
            }
        }
        return Update::none();
    }
    let at_prompt = matches!(
        state.command_phase,
        crate::session::protocol::PaneCommandPhase::Prompt
            | crate::session::protocol::PaneCommandPhase::Input
    );
    let mut transition = None;
    let mut edges = None;
    let mut title = None;
    let mut reported_status = None;
    let mut finished_rows = Vec::new();
    let mut recording_changed = None;
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
        && pane.pty_generation == generation
        && state.sequence > pane.terminal.runtime_sequence
    {
        title = Some(pane.display_title(None));
        pane.agent_refs = agent_refs;
        if pane.terminal.recording != state.recording {
            recording_changed = Some(state.recording);
        }
        let applied = apply_pane_runtime_state(pane, state);
        reported_status = applied.current_status.clone();
        finished_rows = applied.finished_rows;
        edges = Some(applied.edges);
        if applied.previous_status != applied.current_status {
            transition = Some((
                applied.previous_status,
                applied.current_status,
                title.clone().expect("matched pane has a title"),
            ));
        }
    }
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
    // The shell reached its first prompt: deliver any queued replay input now, so readline
    // echoes it exactly once at the prompt (see `flush_replay_input`).
    if at_prompt {
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
    let alert = apply_program_status(pane, snapshot, attended);
    if let Some(alert) = alert {
        notify_program_status(ctx, pane_id, attended, alert);
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
    attended: bool,
    alert: ProgramStatusAlert,
) {
    let (title, state, message) = alert;
    if ctx.state.do_not_disturb {
        return;
    }
    crate::pane::pty_events::maybe_notify_program_status(
        &ctx.state.config,
        ctx.state.is_controller(),
        attended,
        pane_id,
        &title,
        state,
        message.as_deref(),
    );
    if ctx.state.is_controller() && !attended {
        let cue = if state == tui_lipan::ProgramStatusState::Blocked {
            crate::platform::sound::Cue::Blocked
        } else {
            crate::platform::sound::Cue::Error
        };
        crate::ops::sound::cue(ctx, cue);
    }
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
            if pane
                .terminal
                .program_status_notification_at
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(5))
            {
                alert = Some((pane.display_title(None), record.state, record.msg.clone()));
                pane.terminal.program_status_notification_at = Some(std::time::Instant::now());
            }
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
        let applied = apply_pane_runtime_state(&mut pane, runtime.clone());
        assert!(applied.edges.became_blocked);
        assert!(!applied.edges.finished);
        assert!(!pane.terminal.finished_unseen);
        let applied = apply_pane_runtime_state(&mut pane, runtime);
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
            let applied = apply_pane_runtime_state(&mut pane, runtime.clone());
            assert!(!applied.edges.finished);
            assert!(!applied.edges.became_blocked);
            assert!(!pane.terminal.finished_unseen);
            runtime.detected_agent.as_mut().unwrap().state = DetectedAgentState::Idle;
            apply_pane_runtime_state(&mut pane, runtime.clone());
            runtime.detected_agent.as_mut().unwrap().state = DetectedAgentState::Blocked;
            let applied = apply_pane_runtime_state(&mut pane, runtime);
            assert!(applied.edges.became_blocked);
            assert!(!applied.edges.finished);
        }
    }
}
