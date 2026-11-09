//! Restart rozi: replace this client with the installed build and come back to the same sessions.
//!
//! The outgoing half runs here: the dialog finds the build to run and plans what happens to each
//! session, then the client asks every server that can to hold its slot, writes the handoff
//! document, lets go of its sessions without closing any, and quits. The runner hands the terminal
//! to the new build once the UI is gone (see [`crate::relaunch`]).
//!
//! The incoming half runs here too, in the new process: it attaches to each session the document
//! names, in an order that rebuilds the background set and its recency, and lands on the session
//! that was on screen.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use semver::Version;
use tui_lipan::prelude::*;

use crate::relaunch::document::{ClientHandoff, HandoffSession};
use crate::relaunch::plan::{SessionFacts, Transition};
use crate::state::{
    AttachmentId, Handover, PlannedSession, Preparation, RelaunchDialog, RelaunchPlan,
    RelaunchStage, Restore, State,
};
use crate::{AppRoot, Msg};

/// How long the outgoing client waits for its servers to reserve its slot. A server answers within
/// a frame; one that has not by now is wedged or unreachable, and the relaunch goes on without its
/// claim rather than leaving the user waiting on it.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(3);

/// Open the Restart rozi dialog. `expected` is the release an update just installed, which the
/// binary found must be; without one, whatever rozi is installed qualifies.
pub(crate) fn open(ctx: &mut Context<AppRoot>, expected: Option<Version>) -> Update {
    open_with(ctx, expected, None)
}

/// Open Restart rozi right after an update installed `latest`, carrying the release-wide `note`
/// the update's metadata raised, if any.
pub(crate) fn open_after_update(
    ctx: &mut Context<AppRoot>,
    latest: Version,
    note: Option<String>,
) -> Update {
    open_with(ctx, Some(latest), note)
}

fn open_with(
    ctx: &mut Context<AppRoot>,
    expected: Option<Version>,
    note: Option<String>,
) -> Update {
    if ctx.state.relaunch.dialog.is_some() || ctx.state.relaunch.restore.is_some() {
        return Update::none();
    }
    ctx.state.relaunch.generation += 1;
    let generation = ctx.state.relaunch.generation;
    ctx.state.relaunch.dialog = Some(RelaunchDialog {
        generation,
        expected: expected.clone(),
        note,
        stage: RelaunchStage::Checking,
    });
    crate::ops::focus::request_dialog_answer_focus(ctx, crate::view::DIALOG_REFUSE);
    if !ctx.state.relaunch.finds_binaries {
        return Update::full();
    }
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            let result = crate::relaunch::candidate::find(expected.as_ref())
                .map_err(|missing| missing.to_string());
            link.send(Msg::RelaunchCandidate { generation, result });
        });
    }))
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    // A preparation in flight is not interrupted: the reservations it asked for expire unclaimed.
    ctx.state.relaunch.dialog = None;
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn focus_answer(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if ctx.state.relaunch.dialog.is_some() {
        crate::ops::focus::request_dialog_answer_focus(ctx, index);
    }
    Update::full()
}

/// The binary was found, or not. A found one is planned against every session right away.
pub(crate) fn candidate_found(
    ctx: &mut Context<AppRoot>,
    generation: u64,
    result: std::result::Result<crate::relaunch::candidate::Candidate, String>,
) -> Update {
    let Some(dialog) = ctx.state.relaunch.dialog.as_ref() else {
        return Update::none();
    };
    if dialog.generation != generation || !matches!(dialog.stage, RelaunchStage::Checking) {
        return Update::none();
    }
    let stage = match result {
        Ok(candidate) => RelaunchStage::Ready(plan(&ctx.state, candidate)),
        Err(reason) => RelaunchStage::Unavailable(reason),
    };
    let ready = matches!(&stage, RelaunchStage::Ready(plan) if !plan.blocked());
    if let Some(dialog) = ctx.state.relaunch.dialog.as_mut() {
        dialog.stage = stage;
    }
    let answer = if ready {
        crate::view::DIALOG_AFFIRM
    } else {
        crate::view::DIALOG_REFUSE
    };
    crate::ops::focus::request_dialog_answer_focus(ctx, answer);
    Update::full()
}

/// What relaunching onto `candidate` does to every session this client holds.
pub(crate) fn plan(
    state: &State,
    candidate: crate::relaunch::candidate::Candidate,
) -> RelaunchPlan {
    let install_allowed = state.config.remote.install != crate::config::RemoteInstallPolicy::Never;
    let planned = |(epoch, attachment): (AttachmentId, &crate::state::Attachment)| {
        let facts = session_facts(attachment, install_allowed)?;
        let transition = crate::relaunch::plan::transition(&facts, &candidate.protocol);
        Some(PlannedSession {
            epoch,
            facts,
            transition,
        })
    };
    // The session on screen first, then the background by name: a stable order to read.
    let mut background: Vec<PlannedSession> = state
        .background
        .iter()
        .filter_map(|(epoch, attachment)| planned((*epoch, attachment)))
        .collect();
    background.sort_by_key(|session| session.facts.label());
    let mut sessions: Vec<PlannedSession> = planned((state.runtime_epoch, state.current()))
        .into_iter()
        .collect();
    sessions.extend(background);
    RelaunchPlan {
        candidate,
        sessions,
        ending: ending_processes(state),
        armed: false,
    }
}

fn session_facts(
    attachment: &crate::state::Attachment,
    remote_install_allowed: bool,
) -> Option<SessionFacts> {
    let name = attachment.session_name.clone()?;
    let connected = attachment.connection == crate::state::ConnectionState::Connected
        && attachment.pending_session_attach.is_none();
    let client = attachment
        .session_client
        .as_ref()
        .filter(|_| connected && attachment.shared.is_some());
    Some(SessionFacts {
        name,
        remote: attachment.remote_target.clone(),
        generation: client.map(crate::session::client::SessionClient::effective_protocol),
        holds_slot: client.is_some_and(|client| {
            client.supports(crate::session::protocol::Capability::ClientHandoff)
        }),
        read_only: attachment
            .shared
            .as_ref()
            .map_or(attachment.reconnect_read_only, |shared| shared.read_only),
        remote_install_allowed,
    })
}

/// The processes this client runs itself, which a relaunch ends: a scratchpad shell and a popup
/// live in this process, not in a session.
fn ending_processes(state: &State) -> Vec<String> {
    let mut ending = Vec::new();
    let scratch = state
        .scratch
        .panes
        .iter()
        .filter(|pane| pane.terminal.is_running())
        .count();
    if scratch > 0 {
        ending.push(crate::view::vocabulary::count(scratch, "scratchpad shell"));
    }
    if state
        .popup
        .as_ref()
        .is_some_and(|popup| popup.terminal.is_running())
    {
        ending.push("popup".to_string());
    }
    ending
}

/// Restart: a plan that ends programs arms on the first press and commits on the second; one that
/// is blocked does nothing.
pub(crate) fn confirm(ctx: &mut Context<AppRoot>) -> Update {
    let Some(dialog) = ctx.state.relaunch.dialog.as_mut() else {
        return Update::none();
    };
    let RelaunchStage::Ready(plan) = &mut dialog.stage else {
        return Update::none();
    };
    if plan.blocked() {
        return Update::none();
    }
    if plan.destructive() && !plan.armed {
        plan.armed = true;
        return Update::full();
    }
    let generation = dialog.generation;
    let RelaunchStage::Ready(plan) = std::mem::replace(&mut dialog.stage, RelaunchStage::Checking)
    else {
        unreachable!("matched Ready above");
    };
    let mut pending = HashMap::new();
    for session in &plan.sessions {
        if session.transition != (Transition::Carry { claim: true }) {
            continue;
        }
        let Some(client) = ctx
            .state
            .attachment_for_epoch(session.epoch)
            .and_then(|attachment| attachment.session_client.clone())
        else {
            continue;
        };
        ctx.state.relaunch.next_request += 1;
        let request = ctx.state.relaunch.next_request;
        if client.prepare_handoff(request) {
            pending.insert(request, session.epoch);
        }
    }
    let waiting = !pending.is_empty();
    if let Some(dialog) = ctx.state.relaunch.dialog.as_mut() {
        dialog.stage = RelaunchStage::Preparing(Preparation {
            plan,
            pending,
            tokens: HashMap::new(),
        });
    }
    if !waiting {
        return finish(ctx);
    }
    Update::with_command(Command::after(
        PREPARE_TIMEOUT,
        move |link: CommandLink<Msg>| link.send(Msg::RelaunchPrepareTimeout { generation }),
    ))
}

/// A server answered a reservation request.
pub(crate) fn handoff_prepared(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    token: String,
    error: Option<String>,
) -> Update {
    let Some(RelaunchStage::Preparing(preparation)) = ctx
        .state
        .relaunch
        .dialog
        .as_mut()
        .map(|dialog| &mut dialog.stage)
    else {
        return Update::none();
    };
    if preparation.pending.get(&request_id) != Some(&epoch) {
        return Update::none();
    }
    preparation.pending.remove(&request_id);
    if error.is_none() && !token.is_empty() {
        preparation.tokens.insert(epoch, token);
    }
    if preparation.pending.is_empty() {
        return finish(ctx);
    }
    Update::none()
}

pub(crate) fn prepare_timeout(ctx: &mut Context<AppRoot>, generation: u64) -> Update {
    let preparing = ctx.state.relaunch.dialog.as_ref().is_some_and(|dialog| {
        dialog.generation == generation && matches!(dialog.stage, RelaunchStage::Preparing(_))
    });
    if preparing {
        finish(ctx)
    } else {
        Update::none()
    }
}

/// Every claim is in, or no longer awaited: write the handoff, let go of the sessions, and quit so
/// the runner can hand the terminal over.
fn finish(ctx: &mut Context<AppRoot>) -> Update {
    let Some(dialog) = ctx.state.relaunch.dialog.as_mut() else {
        return Update::none();
    };
    let RelaunchStage::Preparing(preparation) =
        std::mem::replace(&mut dialog.stage, RelaunchStage::Checking)
    else {
        return Update::none();
    };
    let Preparation { plan, tokens, .. } = preparation;
    // The reservations already granted are left to expire: the client stays, so none is used.
    let unavailable = |ctx: &mut Context<AppRoot>, reason: String| {
        if let Some(dialog) = ctx.state.relaunch.dialog.as_mut() {
            dialog.stage = RelaunchStage::Unavailable(reason);
        }
        crate::ops::focus::request_dialog_answer_focus(ctx, crate::view::DIALOG_REFUSE);
        Update::full()
    };
    if !plan.candidate.unchanged() {
        let reason = format!(
            "{} changed after it was checked",
            plan.candidate.path.display()
        );
        return unavailable(ctx, reason);
    }
    let handoff = handoff_document(&ctx.state, &plan, &tokens);
    let handoff_id = match crate::relaunch::document::store(&handoff) {
        Ok(id) => id,
        Err(error) => {
            return unavailable(ctx, format!("the handoff could not be written: {error}"));
        }
    };
    ctx.state.relaunch.dialog = None;
    ctx.state.relaunch.handover.set(Handover {
        candidate: plan.candidate,
        handoff_id,
    });
    crate::ops::exit::leave_for_relaunch(ctx)
}

fn handoff_document(
    state: &State,
    plan: &RelaunchPlan,
    tokens: &HashMap<AttachmentId, String>,
) -> ClientHandoff {
    let mut foreground = None;
    let sessions = plan
        .sessions
        .iter()
        .filter_map(|planned| {
            let attachment = state.attachment_for_epoch(planned.epoch)?;
            if planned.epoch == state.runtime_epoch {
                foreground = Some(foreground_index(plan, planned.epoch));
            }
            Some(HandoffSession {
                name: planned.facts.name.clone(),
                remote: planned
                    .facts
                    .remote
                    .as_ref()
                    .map(crate::session::remote::RemoteTarget::to_spec),
                instance: attachment
                    .session_instance
                    .as_ref()
                    .map(|instance| instance.as_str().to_string()),
                transition: planned.transition.clone(),
                token: tokens.get(&planned.epoch).cloned(),
                read_only: planned.facts.read_only,
                auto_created: attachment.auto_created,
                engaged: attachment.engaged,
                active_workspace: attachment.active_workspace,
                focused_pane: attachment.focused_pane,
                parked_seq: attachment.parked_seq,
            })
        })
        .collect();
    ClientHandoff::new(foreground.flatten(), sessions)
}

fn foreground_index(plan: &RelaunchPlan, epoch: AttachmentId) -> Option<usize> {
    plan.sessions
        .iter()
        .position(|planned| planned.epoch == epoch)
}

/// Start bringing back the sessions `handoff` names, in a relaunched client. `problems` are what
/// already went wrong before the UI started, such as an upgrade that failed.
pub(crate) fn begin_restore(
    ctx: &mut Context<AppRoot>,
    handoff: ClientHandoff,
    problems: Vec<String>,
) {
    let queue: VecDeque<HandoffSession> = handoff
        .restore_order()
        .into_iter()
        .map(|index| handoff.sessions[index].clone())
        .collect();
    ctx.state.relaunch.restore = Some(Restore {
        total: queue.len(),
        queue,
        current: None,
        launcher: handoff.foreground.is_none(),
        problems,
    });
}

/// Attach the next session, or finish.
pub(crate) fn restore_next(ctx: &mut Context<AppRoot>) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.as_mut() else {
        return Update::none();
    };
    let Some(session) = restore.queue.pop_front() else {
        return finish_restore(ctx);
    };
    let remote = match session.remote.as_deref() {
        Some(spec) => match crate::session::remote::parse_remote_target(spec) {
            Ok(target) => Some(target),
            Err(error) => {
                restore
                    .problems
                    .push(format!("`{}`: {error}", session.name));
                return restore_next(ctx);
            }
        },
        None => None,
    };
    let access = if session.read_only {
        crate::state::AttachAccess::ReadOnly
    } else {
        crate::state::AttachAccess::WRITABLE
    };
    let request =
        crate::state::AttachRequest::new(session.name.clone(), crate::state::AttachMode::Resume)
            .on_host(remote)
            .with_access(access)
            .with_handoff_token(session.token.clone());
    let update = crate::ops::session::begin_named_attach(ctx, request, None);
    let epoch = ctx
        .state
        .current()
        .pending_session_attach
        .as_ref()
        .map(|pending| pending.epoch);
    if let Some(restore) = ctx.state.relaunch.restore.as_mut() {
        restore.current = epoch.map(|epoch| (epoch, session));
    }
    update
}

/// An attach the restore started has landed or failed. A landed one gets back the view state the
/// previous process had; either way the next session follows.
pub(crate) fn attach_settled(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    failure: Option<&str>,
) -> Update {
    let Some((_, session)) = ctx
        .state
        .relaunch
        .restore
        .as_mut()
        .and_then(|restore| restore.current.take_if(|(current, _)| *current == epoch))
    else {
        return Update::none();
    };
    let problem = match failure {
        Some(message) => Some(message.to_string()),
        None => {
            let restarted = session.transition != Transition::Upgrade
                && session.instance.as_deref().is_some_and(|instance| {
                    ctx.state
                        .current()
                        .session_instance
                        .as_ref()
                        .is_some_and(|now| now.as_str() != instance)
                });
            restore_view(ctx.state.current_mut(), &session);
            restarted.then(|| format!("`{}` was restarted meanwhile", session.name))
        }
    };
    if let Some(problem) = problem
        && let Some(restore) = ctx.state.relaunch.restore.as_mut()
    {
        restore.problems.push(problem);
    }
    next_step(ctx)
}

/// Whether a claim on a reserved slot succeeded. A claim that did not means the session's layout
/// control may have passed to another client meanwhile.
pub(crate) fn handoff_claimed(ctx: &mut Context<AppRoot>, epoch: u64, claimed: bool) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.as_mut() else {
        return Update::none();
    };
    if claimed {
        return Update::none();
    }
    let name = restore
        .current
        .as_ref()
        .filter(|(current, _)| *current == epoch)
        .map(|(_, session)| session.name.clone());
    if let Some(name) = name {
        restore
            .problems
            .push(format!("`{name}` was rejoined without its held place"));
    }
    Update::none()
}

fn restore_view(attachment: &mut crate::state::Attachment, session: &HandoffSession) {
    attachment.auto_created = session.auto_created;
    attachment.engaged = session.engaged;
    if session.active_workspace < attachment.workspaces.len() {
        attachment.active_workspace = session.active_workspace;
    }
    if let Some(pane) = session.focused_pane
        && attachment.workspaces[attachment.active_workspace]
            .panes
            .iter()
            .any(|candidate| candidate.id == pane)
    {
        attachment.focused_pane = Some(pane);
    }
}

/// The next attach waits for this message, so the one that just landed finishes installing before
/// it is parked.
fn next_step(_ctx: &mut Context<AppRoot>) -> Update {
    Update::with_command(Command::spawn(|link| link.send(Msg::RelaunchRestoreNext)))
}

fn finish_restore(ctx: &mut Context<AppRoot>) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.take() else {
        return Update::none();
    };
    if restore.launcher && ctx.state.current().session_attached {
        crate::ops::session::park_current_session(ctx);
        crate::ops::session::enter_sessionless(ctx);
    }
    crate::ops::focus::request_current_pane_focus(ctx);
    if !restore.problems.is_empty() {
        let mut body = crate::pane::pty_events::ToastBody::new(restore.problems.join("\n"));
        body = body.hint("Sessions lists every session that is still running");
        crate::pane::pty_events::notify_warning(ctx, "Restarted rozi", body);
    }
    Update::full()
}

/// While the background sessions are coming back, the view is covered, so each does not flash by
/// on its way into the background. `(done, total)` while that is the case.
pub(crate) fn covering_progress(state: &State) -> Option<(usize, usize)> {
    state
        .relaunch
        .restore
        .as_ref()
        .filter(|restore| restore.covers_view())
        .map(|restore| (restore.done(), restore.total))
}

#[cfg(test)]
mod tests;
