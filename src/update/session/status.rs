use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::{find_pane_in_namespace, find_pane_in_namespace_mut};
use crate::pty_events::maybe_notify_pane_status;
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

pub(crate) fn update_agent_status_edge(
    pane: &mut crate::pane::TerminalPane,
    previous: Option<&str>,
    previous_age: Option<std::time::Duration>,
    previous_blocked: bool,
) -> AgentEdges {
    let current = pane.agent_status();
    let current = current.as_deref();
    if current != previous {
        if status_is_active_run(previous) && status_is_quiescent(current) {
            pane.last_run = previous_age;
        }
        pane.status_since = Some(std::time::Instant::now());
    }
    let became_blocked = !previous_blocked && pane.is_blocked();
    let mut finished = false;
    if status_is(current, crate::session::protocol::pane_status::WORKING) {
        pane.finished_unseen = false;
    } else if status_is(previous, crate::session::protocol::pane_status::WORKING)
        && current.is_some()
        && !status_is(current, crate::session::protocol::pane_status::BLOCKED)
    {
        pane.finished_unseen = true;
        finished = true;
    }
    AgentEdges {
        became_blocked,
        finished,
    }
}

pub(crate) struct AppliedPaneRuntime {
    pub(crate) finished_rows: Vec<String>,
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
    pane.terminal.runtime_sequence = state.sequence;
    pane.terminal.cwd = state.cwd;
    pane.terminal.cwd_host = state.cwd_host;
    pane.terminal.display_path = state.display_path;
    pane.terminal.project_root = state.project_root;
    pane.terminal.git_branch = state.git_branch;
    pane.terminal.foreground_program = state.foreground_program;
    pane.terminal.foreground_executable = state.foreground_executable;
    pane.terminal.foreground_arguments = state.foreground_arguments;
    pane.terminal.command_phase = state.command_phase;
    pane.terminal.last_exit_status = state.last_exit_status;
    pane.terminal.reported_status = state.status;
    pane.terminal.detected_agent = state.detected_agent;
    pane.terminal.work_started_at = state.work_started_at;
    let finished_rows = pane.terminal.apply_rows(state.rows);
    let edges = update_agent_status_edge(
        &mut pane.terminal,
        previous_agent_status.as_deref(),
        previous_age,
        previous_blocked,
    );
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
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
        && pane.pty_generation == generation
        && state.sequence > pane.terminal.runtime_sequence
    {
        title = Some(pane.display_title(None));
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
    // it - the user was looking at a different tab of the same program - so it alerts regardless.
    if !finished_rows.is_empty()
        && let Some(title) = title.clone()
    {
        let background = find_pane_in_namespace(&ctx.state, pane_id, local).is_some_and(|pane| {
            finished_rows.iter().any(|id| {
                pane.terminal
                    .published_rows
                    .iter()
                    .any(|row| &row.id == id && !row.active)
            })
        });
        if background {
            if !ctx.state.do_not_disturb {
                maybe_notify_pane_status(
                    &ctx.state.config,
                    ctx.state.is_controller(),
                    false,
                    pane_id,
                    &title,
                    crate::pty_events::PaneStatusNotification {
                        blocked: false,
                        done: true,
                        reported_status: None,
                    },
                );
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
                crate::pty_events::PaneStatusNotification {
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
