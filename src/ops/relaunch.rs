//! Restart rozi: replace this client with the installed build and come back to the same sessions.
//!
//! The outgoing half runs as a transaction. The dialog finds the build to run and plans what
//! happens to each session. Restarting then asks every server that promised to hold this client's
//! slot for a reservation, and fails closed: a server that does not answer, or refuses, stops the
//! relaunch and the client stays as it was. With every reservation in hand the plan is made again
//! from the client as it is now, and a plan that changed goes back to the dialog. Only then is the
//! handoff written and each reservation committed, and the client lets go of its sessions once every
//! commit is answered. A failure at any point before that aborts what was reserved.
//!
//! The incoming half runs here too, in the new process: it first attaches to every session it can
//! carry over as it is, claiming each slot, then upgrades the sessions that need it and attaches to
//! those, and finally puts the session that was on screen back on screen.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use semver::Version;
use tui_lipan::prelude::*;

use crate::relaunch::document::{ClientHandoff, HandoffSession};
use crate::relaunch::plan::{ServerFacts, SessionFacts, Transition};
use crate::state::{
    AttachmentId, Handover, PlannedSession, RelaunchDialog, RelaunchPlan, RelaunchStage, Restore,
    RestoreActivity, RestoreInFlight, RestoreStep, State, Transaction,
};
use crate::{AppRoot, Msg};

/// How long the outgoing client waits for its servers to answer a reservation or a commit. A
/// server answers within a frame; one that has not by now is wedged or unreachable, and the
/// relaunch stops rather than leave without the slot it promised to keep.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Close the dialog. A relaunch in progress is abandoned: whatever it reserved is discarded, and the
/// client stays as it was.
pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(dialog) = ctx.state.relaunch.dialog.take()
        && let RelaunchStage::Preparing(transaction) | RelaunchStage::Committing(transaction) =
            dialog.stage
    {
        abandon(&ctx.state, &transaction);
    }
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
    show(ctx, stage)
}

/// Put `stage` in the dialog, with focus on the answer it leads to.
fn show(ctx: &mut Context<AppRoot>, stage: RelaunchStage) -> Update {
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
        let transition =
            crate::relaunch::plan::transition(&facts, &candidate.release, &candidate.protocol);
        Some(PlannedSession {
            epoch,
            facts,
            instance: attachment
                .session_instance
                .as_ref()
                .map(|instance| instance.as_str().to_string()),
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
        notice: None,
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
        server: client.map(|client| ServerFacts {
            protocol: client.server_protocol(),
            release: client.server_version().map(str::to_string),
        }),
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

/// Restart: a plan that ends programs arms on the first press and starts on the second; one that
/// is blocked does nothing.
pub(crate) fn confirm(ctx: &mut Context<AppRoot>) -> Update {
    let Some(dialog) = ctx.state.relaunch.dialog.as_mut() else {
        return Update::none();
    };
    let RelaunchStage::Ready(shown) = &mut dialog.stage else {
        return Update::none();
    };
    if shown.blocked() {
        return Update::none();
    }
    if shown.destructive() && !shown.armed {
        shown.armed = true;
        return Update::full();
    }
    let RelaunchStage::Ready(shown) = std::mem::replace(&mut dialog.stage, RelaunchStage::Checking)
    else {
        unreachable!("matched Ready above");
    };
    // Act only on what the user saw: a plan that would now be different is shown again first.
    let current = plan(&ctx.state, shown.candidate.clone());
    if !shown.same_as(&current) {
        return show(ctx, replanned(current));
    }
    ctx.state.relaunch.attempt += 1;
    let attempt = ctx.state.relaunch.attempt;
    let mut transaction = Transaction {
        plan: shown,
        attempt,
        pending: HashMap::new(),
        tokens: HashMap::new(),
        handoff_id: None,
    };
    let claims: Vec<(AttachmentId, String)> = transaction
        .plan
        .sessions
        .iter()
        .filter(|session| session.transition == (Transition::Carry { claim: true }))
        .map(|session| (session.epoch, session.facts.label()))
        .collect();
    for (epoch, label) in claims {
        let client = ctx
            .state
            .attachment_for_epoch(epoch)
            .and_then(|attachment| attachment.session_client.clone());
        ctx.state.relaunch.next_request += 1;
        let request = ctx.state.relaunch.next_request;
        if !client.is_some_and(|client| client.prepare_handoff(request)) {
            let reason = format!("`{label}` could not be asked to hold its place");
            return fail(ctx, transaction, reason);
        }
        transaction.pending.insert(request, epoch);
    }
    if transaction.pending.is_empty() {
        return commit(ctx, transaction);
    }
    awaiting(ctx, RelaunchStage::Preparing(transaction), attempt)
}

fn replanned(mut plan: RelaunchPlan) -> RelaunchStage {
    plan.notice = Some("Sessions changed · check them and restart again".to_string());
    RelaunchStage::Ready(plan)
}

/// Wait for the servers to answer `stage`'s requests, for no longer than [`ANSWER_TIMEOUT`].
fn awaiting(ctx: &mut Context<AppRoot>, stage: RelaunchStage, attempt: u64) -> Update {
    if let Some(dialog) = ctx.state.relaunch.dialog.as_mut() {
        dialog.stage = stage;
    }
    Update::with_command(Command::after(
        ANSWER_TIMEOUT,
        move |link: CommandLink<Msg>| link.send(Msg::RelaunchAnswerTimeout { attempt }),
    ))
}

/// The transaction in progress, taken out of the dialog, when `pick` accepts its stage.
fn take_transaction(
    ctx: &mut Context<AppRoot>,
    pick: impl Fn(&RelaunchStage) -> bool,
) -> Option<(Transaction, bool)> {
    let dialog = ctx.state.relaunch.dialog.as_mut()?;
    if !pick(&dialog.stage) {
        return None;
    }
    match std::mem::replace(&mut dialog.stage, RelaunchStage::Checking) {
        RelaunchStage::Preparing(transaction) => Some((transaction, false)),
        RelaunchStage::Committing(transaction) => Some((transaction, true)),
        other => {
            dialog.stage = other;
            None
        }
    }
}

fn put_back(ctx: &mut Context<AppRoot>, transaction: Transaction, committing: bool) {
    if let Some(dialog) = ctx.state.relaunch.dialog.as_mut() {
        dialog.stage = if committing {
            RelaunchStage::Committing(transaction)
        } else {
            RelaunchStage::Preparing(transaction)
        };
    }
}

/// A server answered a reservation request.
pub(crate) fn handoff_prepared(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    token: String,
    error: Option<String>,
) -> Update {
    let Some((mut transaction, _)) = take_transaction(ctx, |stage| {
        matches!(stage, RelaunchStage::Preparing(transaction)
            if transaction.pending.get(&request_id) == Some(&epoch))
    }) else {
        return Update::none();
    };
    transaction.pending.remove(&request_id);
    if error.is_some() || token.is_empty() {
        let reason = format!(
            "`{}` could not hold its place: {}",
            label_of(&transaction, epoch),
            error.as_deref().unwrap_or("no token")
        );
        return fail(ctx, transaction, reason);
    }
    transaction.tokens.insert(epoch, token);
    if transaction.pending.is_empty() {
        return commit(ctx, transaction);
    }
    put_back(ctx, transaction, false);
    Update::none()
}

/// A server answered a commit.
pub(crate) fn handoff_committed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    error: Option<String>,
) -> Update {
    let Some((mut transaction, _)) = take_transaction(ctx, |stage| {
        matches!(stage, RelaunchStage::Committing(transaction)
            if transaction.pending.get(&request_id) == Some(&epoch))
    }) else {
        return Update::none();
    };
    transaction.pending.remove(&request_id);
    if let Some(error) = error {
        let reason = format!(
            "`{}` could not hold its place: {error}",
            label_of(&transaction, epoch)
        );
        return fail(ctx, transaction, reason);
    }
    if transaction.pending.is_empty() {
        return hand_over(ctx, transaction);
    }
    put_back(ctx, transaction, true);
    Update::none()
}

/// A server did not answer in time: the relaunch stops, failing closed.
pub(crate) fn answer_timeout(ctx: &mut Context<AppRoot>, attempt: u64) -> Update {
    let Some((transaction, _)) = take_transaction(ctx, |stage| {
        matches!(stage, RelaunchStage::Preparing(transaction) | RelaunchStage::Committing(transaction)
            if transaction.attempt == attempt)
    }) else {
        return Update::none();
    };
    let silent: Vec<String> = transaction
        .pending
        .values()
        .map(|epoch| format!("`{}`", label_of(&transaction, *epoch)))
        .collect();
    let reason = format!("{} did not answer", silent.join(", "));
    fail(ctx, transaction, reason)
}

fn label_of(transaction: &Transaction, epoch: AttachmentId) -> String {
    transaction
        .plan
        .sessions
        .iter()
        .find(|session| session.epoch == epoch)
        .map(|session| session.facts.label())
        .unwrap_or_else(|| "a session".to_string())
}

/// Every reservation is in: check the plan still holds, write the handoff, and commit.
fn commit(ctx: &mut Context<AppRoot>, mut transaction: Transaction) -> Update {
    let current = plan(&ctx.state, transaction.plan.candidate.clone());
    if !transaction.plan.same_as(&current) {
        abandon(&ctx.state, &transaction);
        return show(ctx, replanned(current));
    }
    if !transaction.plan.candidate.unchanged() {
        let reason = format!(
            "{} changed after it was checked",
            transaction.plan.candidate.path.display()
        );
        abandon(&ctx.state, &transaction);
        return show(ctx, RelaunchStage::Unavailable(reason));
    }
    let handoff = handoff_document(&ctx.state, &transaction.plan, &transaction.tokens);
    match crate::relaunch::document::store(&handoff) {
        Ok(id) => transaction.handoff_id = Some(id),
        Err(error) => {
            let reason = format!("the handoff could not be written: {error}");
            return fail(ctx, transaction, reason);
        }
    }
    for (epoch, token) in &transaction.tokens {
        let Some(client) = ctx
            .state
            .attachment_for_epoch(*epoch)
            .and_then(|attachment| attachment.session_client.clone())
        else {
            let reason = format!("`{}` went away", label_of(&transaction, *epoch));
            return fail(ctx, transaction, reason);
        };
        ctx.state.relaunch.next_request += 1;
        let request = ctx.state.relaunch.next_request;
        if !client.commit_handoff(request, token.clone()) {
            let reason = format!(
                "`{}` could not hold its place",
                label_of(&transaction, *epoch)
            );
            return fail(ctx, transaction, reason);
        }
        transaction.pending.insert(request, *epoch);
    }
    if transaction.pending.is_empty() {
        return hand_over(ctx, transaction);
    }
    let attempt = transaction.attempt;
    awaiting(ctx, RelaunchStage::Committing(transaction), attempt)
}

/// Every commit is answered: hand the runner the new build and let go.
fn hand_over(ctx: &mut Context<AppRoot>, transaction: Transaction) -> Update {
    let Some(handoff_id) = transaction.handoff_id else {
        unreachable!("a transaction is committed only once its handoff is written");
    };
    ctx.state.relaunch.dialog = None;
    ctx.state.relaunch.handover.set(Handover {
        candidate: transaction.plan.candidate,
        handoff_id,
    });
    crate::ops::exit::leave_for_relaunch(ctx)
}

/// Stop the relaunch: take back what was reserved, and show why, ready to try again.
fn fail(ctx: &mut Context<AppRoot>, transaction: Transaction, reason: String) -> Update {
    abandon(&ctx.state, &transaction);
    let mut plan = transaction.plan;
    plan.armed = false;
    plan.notice = Some(reason);
    show(ctx, RelaunchStage::Ready(plan))
}

/// Discard every reservation `transaction` holds, and its handoff document.
fn abandon(state: &State, transaction: &Transaction) {
    for (epoch, token) in &transaction.tokens {
        if let Some(client) = state
            .attachment_for_epoch(*epoch)
            .and_then(|attachment| attachment.session_client.as_ref())
        {
            client.abort_handoff(token.clone());
        }
    }
    if let Some(id) = &transaction.handoff_id {
        crate::relaunch::document::discard(id);
    }
}

fn handoff_document(
    state: &State,
    plan: &RelaunchPlan,
    tokens: &HashMap<AttachmentId, String>,
) -> ClientHandoff {
    let foreground = plan
        .sessions
        .iter()
        .position(|planned| planned.epoch == state.runtime_epoch);
    let sessions = plan
        .sessions
        .iter()
        .map(|planned| {
            let attachment = state.attachment_for_epoch(planned.epoch);
            HandoffSession {
                name: planned.facts.name.clone(),
                remote: planned
                    .facts
                    .remote
                    .as_ref()
                    .map(crate::session::remote::RemoteTarget::to_spec),
                instance: planned.instance.clone(),
                transition: planned.transition.clone(),
                token: tokens.get(&planned.epoch).cloned(),
                read_only: planned.facts.read_only,
                auto_created: attachment.is_some_and(|attachment| attachment.auto_created),
                engaged: attachment.is_some_and(|attachment| attachment.engaged),
                active_workspace: attachment.map_or(0, |attachment| attachment.active_workspace),
                focused_pane: attachment.and_then(|attachment| attachment.focused_pane),
                parked_seq: attachment.map_or(0, |attachment| attachment.parked_seq),
            }
        })
        .collect();
    ClientHandoff::new(foreground, sessions)
}

/// Start bringing back the sessions `handoff` names, in a relaunched client.
///
/// Every session the new build can attach to as it is comes first, oldest parked first and the one
/// on screen last of them; then every session to upgrade, in the same order; then the landing.
pub(crate) fn begin_restore(ctx: &mut Context<AppRoot>, handoff: ClientHandoff) {
    ctx.state.relaunch.restore = Some(restore_for(&handoff));
}

/// The restore that brings back what `handoff` names, not yet started.
fn restore_for(handoff: &ClientHandoff) -> Restore {
    let order = handoff.restore_order();
    let ordered = || order.iter().map(|index| &handoff.sessions[*index]);
    let mut steps: VecDeque<RestoreStep> = ordered()
        .filter(|session| session.transition != Transition::Upgrade)
        .map(|session| RestoreStep::Attach(session.clone()))
        .collect();
    steps.extend(
        ordered()
            .filter(|session| session.transition == Transition::Upgrade)
            .map(|session| RestoreStep::Upgrade(session.clone())),
    );
    steps.push_back(RestoreStep::Land);
    Restore {
        steps,
        current: None,
        total: handoff.sessions.len(),
        done: 0,
        foreground: handoff
            .foreground
            .map(|index| identity(&handoff.sessions[index])),
        foreground_epoch: None,
        recency: handoff
            .sessions
            .iter()
            .map(|session| (identity(session), session.parked_seq))
            .collect(),
        problems: Vec::new(),
    }
}

fn identity(session: &HandoffSession) -> (String, Option<String>) {
    (session.name.clone(), session.remote.clone())
}

/// Take the next restore step.
pub(crate) fn restore_next(ctx: &mut Context<AppRoot>) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.as_mut() else {
        return Update::none();
    };
    if restore.current.is_some() {
        return Update::none();
    }
    match restore.steps.pop_front() {
        Some(RestoreStep::Attach(session)) => attach_step(ctx, session),
        Some(RestoreStep::Upgrade(session)) => upgrade_step(ctx, session),
        Some(RestoreStep::Land) | None => land(ctx),
    }
}

fn attach_step(ctx: &mut Context<AppRoot>, session: HandoffSession) -> Update {
    let remote = match session.remote.as_deref() {
        Some(spec) => match crate::session::remote::parse_remote_target(spec) {
            Ok(target) => Some(target),
            Err(error) => {
                return skip(ctx, format!("`{}`: {error}", session.name));
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
    // The session that was on screen keeps its layout control while the rest attach after it.
    let lease = if ctx
        .state
        .relaunch
        .restore
        .as_ref()
        .and_then(|r| r.foreground_epoch)
        == Some(ctx.state.runtime_epoch)
    {
        crate::ops::session::ParkLease::Keep
    } else {
        crate::ops::session::ParkLease::Release
    };
    let update = crate::ops::session::begin_named_attach_parking(ctx, request, None, lease);
    let epoch = ctx
        .state
        .current()
        .pending_session_attach
        .as_ref()
        .map(|pending| pending.epoch);
    let Some(restore) = ctx.state.relaunch.restore.as_mut() else {
        return update;
    };
    match epoch {
        Some(epoch) => {
            restore.current = Some(RestoreInFlight::Attach { epoch, session });
            update
        }
        None => update.merge(skip(
            ctx,
            format!("`{}` could not be attached", session.name),
        )),
    }
}

fn upgrade_step(ctx: &mut Context<AppRoot>, session: HandoffSession) -> Update {
    ctx.state.relaunch.attempt += 1;
    let attempt = ctx.state.relaunch.attempt;
    let name = session.name.clone();
    let remote = session.remote.clone();
    let config = ctx.state.config.remote.clone();
    if let Some(restore) = ctx.state.relaunch.restore.as_mut() {
        restore.current = Some(RestoreInFlight::Upgrade { attempt, session });
    }
    Update::full().merge(Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            let result = upgrade_session(&name, remote.as_deref(), &config);
            link.send(Msg::RelaunchUpgraded { attempt, result });
        });
    })))
}

/// Restart session `name` on this build from its snapshot. A remote one first needs a rozi on its
/// host that this build can attach to, installed as an interactive `--remote` connection would.
fn upgrade_session(
    name: &str,
    remote: Option<&str>,
    config: &crate::config::RemoteConfig,
) -> std::result::Result<(), String> {
    let Some(spec) = remote else {
        return crate::session::server::upgrade_named_session(name).map_err(|e| e.to_string());
    };
    let target = crate::session::remote::parse_remote_target(spec)?;
    crate::session::remote::ensure_remote_binary_in_ui(&target, config, None)?;
    crate::session::remote::upgrade_remote_session(&target, name, config).map(|_| ())
}

/// An upgrade finished. The session is attached next, ahead of every step still waiting.
pub(crate) fn upgraded(
    ctx: &mut Context<AppRoot>,
    attempt: u64,
    result: std::result::Result<(), String>,
) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.as_mut() else {
        return Update::none();
    };
    let Some(RestoreInFlight::Upgrade { session, .. }) = restore.current.take_if(
        |current| matches!(current, RestoreInFlight::Upgrade { attempt: a, .. } if *a == attempt),
    ) else {
        return Update::none();
    };
    match result {
        Ok(()) => {
            restore
                .steps
                .push_front(RestoreStep::Attach(HandoffSession {
                    token: None,
                    ..session
                }));
            next_step()
        }
        Err(error) => skip(
            ctx,
            format!(
                "`{}` was not upgraded: {error}",
                crate::view::vocabulary::session_at(&session.name, session.remote.as_deref())
            ),
        ),
    }
}

/// Note `problem`, count the session as dealt with, and move on.
fn skip(ctx: &mut Context<AppRoot>, problem: String) -> Update {
    if let Some(restore) = ctx.state.relaunch.restore.as_mut() {
        restore.problems.push(problem);
        restore.done += 1;
    }
    next_step()
}

/// An attach the restore started has landed or failed. A landed one gets back the view state the
/// previous process had; either way the next step follows.
pub(crate) fn attach_settled(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    failure: Option<&str>,
) -> Update {
    let Some(RestoreInFlight::Attach { session, .. }) =
        ctx.state.relaunch.restore.as_mut().and_then(|restore| {
            restore.current.take_if(|current| {
                matches!(current, RestoreInFlight::Attach { epoch: e, .. } if *e == epoch)
            })
        })
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
    let landed = failure.is_none();
    let runtime_epoch = ctx.state.runtime_epoch;
    if let Some(restore) = ctx.state.relaunch.restore.as_mut() {
        restore.done += 1;
        if landed && restore.foreground.as_ref() == Some(&identity(&session)) {
            restore.foreground_epoch = Some(runtime_epoch);
        }
        restore.problems.extend(problem);
    }
    next_step()
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
    let name = match &restore.current {
        Some(RestoreInFlight::Attach { epoch: e, session }) if *e == epoch => session.name.clone(),
        _ => return Update::none(),
    };
    restore
        .problems
        .push(format!("`{name}` was rejoined without its held place"));
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

/// The next step waits for this message, so the attach that just landed finishes installing before
/// it is parked.
fn next_step() -> Update {
    Update::with_command(Command::spawn(|link| link.send(Msg::RelaunchRestoreNext)))
}

/// Everything is back: put the session that was on screen on screen again, or the launcher, give the
/// background its recency back, and report what did not come back as it was.
fn land(ctx: &mut Context<AppRoot>) -> Update {
    let Some(restore) = ctx.state.relaunch.restore.take() else {
        return Update::none();
    };
    let mut update = Update::full();
    match (restore.foreground.is_some(), restore.foreground_epoch) {
        (_, Some(epoch)) if epoch != ctx.state.runtime_epoch => {
            update = update.merge(crate::ops::session::switch_to_parked(ctx, epoch));
        }
        (false, _) if ctx.state.current().session_attached => {
            crate::ops::session::park_current_session(ctx);
            crate::ops::session::enter_sessionless(ctx);
        }
        _ => {}
    }
    let mut background: Vec<(u64, AttachmentId)> = ctx
        .state
        .background
        .iter()
        .filter_map(|(epoch, attachment)| {
            let identity = (
                attachment.session_name.clone()?,
                attachment
                    .remote_target
                    .as_ref()
                    .map(crate::session::remote::RemoteTarget::to_spec),
            );
            let seq = restore
                .recency
                .iter()
                .find(|(known, _)| *known == identity)
                .map_or(0, |(_, seq)| *seq);
            Some((seq, *epoch))
        })
        .collect();
    background.sort();
    let order: Vec<AttachmentId> = background.into_iter().map(|(_, epoch)| epoch).collect();
    ctx.state.restamp_parked(&order);
    crate::ops::focus::request_current_pane_focus(ctx);
    if !restore.problems.is_empty() {
        let mut body = crate::pane::pty_events::ToastBody::new(restore.problems.join("\n"));
        body = body.hint("Sessions lists every session that is still running");
        crate::pane::pty_events::notify_warning(ctx, "Restarted rozi", body);
    }
    update
}

/// While a relaunched client is bringing its sessions back, the view is covered, so no session
/// flashes by on its way into the background; what it shows instead.
pub(crate) fn covering_activity(state: &State) -> Option<RestoreActivity> {
    state.relaunch.restore.as_ref().map(Restore::activity)
}

#[cfg(test)]
mod tests;
