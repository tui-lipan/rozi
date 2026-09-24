//! Pane waits on the UI control endpoint: `capture-pane`, `send-text`, and `send-keys` holding
//! their reply until a pane's screen shows some text or settles.
//!
//! The UI answers from its own copy of each pane's screen, with the same evaluator the session
//! server uses ([`crate::pane::capture_wait`]). What differs is how a send finds its baseline. The
//! UI's copy lags the server's: output the program produced before the input may still be on its
//! way. So a send writes its input marked ([`ClientMessage::MarkedInput`]); the server answers the
//! mark in the same step as writing the input, and the wait takes its baseline when the answer
//! comes back. Output ahead of the answer is older than the input, and output behind it is not.
//!
//! Replies go out through the control connection's channel, which the listener holds open for the
//! wait's own timeout plus a margin (see [`crate::control`]). A tick runs while any wait is
//! pending, so a deadline or a settle period passes even on a quiet screen.
//!
//! [`ClientMessage::MarkedInput`]: crate::session::protocol::ClientMessage::MarkedInput

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control::{
    ControlEnvelope, ControlErrorCode, ControlRequest, ControlResponse, PaneCapture,
};
use crate::pane::capture_wait::{ScreenWait, WaitEnd, WaitPlan, WaitReply, WaitStatus};
use crate::pane::lifecycle::{find_pane_in_namespace_mut, find_pane_mut, pane_is_local};
use crate::state::PaneId;
use crate::{AppRoot, Msg};

/// Longest gap between two looks at a pending wait. Output is looked at as it lands; this bounds
/// how late a settle period or a deadline can be noticed on a screen that has gone quiet.
const TICK: Duration = Duration::from_millis(50);

/// Every pane wait this client is holding a reply for.
#[derive(Default)]
pub(crate) struct UiCaptureWaits {
    waits: Vec<UiCaptureWait>,
    next_token: u64,
    ticking: bool,
}

struct UiCaptureWait {
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    plan: WaitPlan,
    /// When the request came in; its deadline runs from here, even while the baseline is pending.
    started: Instant,
    stage: Stage,
    reply: Sender<ControlResponse>,
}

enum Stage {
    /// Input is queued behind a pane that is still starting; it is marked when the queue is
    /// written.
    Queued,
    /// Input is on its way; the wait starts when the server's mark comes back.
    Marking {
        token: u64,
    },
    Watching(ScreenWait),
}

/// Start the wait `envelope` asks for, answering at once if it is refused or already satisfied.
pub(crate) fn start(ctx: &mut Context<AppRoot>, envelope: ControlEnvelope) -> Update {
    let ControlEnvelope { request, reply } = envelope;
    match register(ctx, &request, reply.clone()) {
        Ok(()) => {}
        Err(response) => {
            let _ = reply.send(response);
            return Update::none();
        }
    }
    evaluate(ctx, None);
    if ctx.state.capture_waits.waits.is_empty() || ctx.state.capture_waits.ticking {
        return Update::none();
    }
    ctx.state.capture_waits.ticking = true;
    Update::command_only(schedule_tick())
}

fn register(
    ctx: &mut Context<AppRoot>,
    request: &ControlRequest,
    reply: Sender<ControlResponse>,
) -> std::result::Result<(), ControlResponse> {
    let plan = WaitPlan::for_command(&request.command)?
        .ok_or_else(|| ControlResponse::error("the request holds no pane wait"))?;
    let target = plan.target.or(request.source_pane);
    let started = Instant::now();
    let epoch = ctx.state.runtime_epoch;
    let (pane_id, local, generation, stage) = if plan.after_input {
        let token = ctx.state.capture_waits.next_token();
        let input = crate::ops::control::deliver_send(ctx, target, &request.command, token)?;
        let stage = if input.starting {
            Stage::Queued
        } else {
            Stage::Marking { token }
        };
        (input.id, input.local, input.generation, stage)
    } else {
        let Some(id) = target.or(ctx.state.focused_pane()) else {
            return Err(ControlResponse::error_with(
                ControlErrorCode::TargetRequired,
                "no target pane and no focused pane",
            ));
        };
        let local = pane_is_local(&ctx.state, id);
        let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
            return Err(ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            ));
        };
        let screen = pane.terminal.with_screen_mut(|screen| {
            ScreenWait::start(&plan.wait, screen, false, started, started)
        });
        (id, local, pane.pty_generation, Stage::Watching(screen))
    };
    ctx.state.capture_waits.waits.push(UiCaptureWait {
        epoch,
        pane_id,
        local,
        generation,
        plan,
        started,
        stage,
        reply,
    });
    Ok(())
}

/// A pane's screen took output: look at the waits on it.
pub(crate) fn pane_output(ctx: &mut Context<AppRoot>, pane_id: PaneId, local: bool) {
    if ctx.state.capture_waits.waits.is_empty() {
        return;
    }
    evaluate(ctx, Some((pane_id, local)));
}

/// A pane's program exited. Its last screen can still satisfy a wait, so that is checked before
/// anything closes the pane; a wait it does not satisfy fails with that screen attached.
pub(crate) fn pane_exited(ctx: &mut Context<AppRoot>, pane_id: PaneId, local: bool) {
    pane_output(ctx, pane_id, local);
}

impl UiCaptureWaits {
    fn next_token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }
}

/// A starting pane's queued input is about to be written: the mark it should carry, when a send
/// queued behind that pane is waiting on it. The queue goes out as one write, so every such wait
/// shares the one mark.
pub(crate) fn mark_queued_input(
    state: &mut crate::state::State,
    pane_id: PaneId,
    generation: u64,
    local: bool,
) -> Option<u64> {
    let waits = &mut state.capture_waits;
    let queued = |wait: &UiCaptureWait| {
        matches!(wait.stage, Stage::Queued)
            && (wait.pane_id, wait.generation, wait.local) == (pane_id, generation, local)
    };
    if !waits.waits.iter().any(queued) {
        return None;
    }
    let token = waits.next_token();
    for wait in waits.waits.iter_mut().filter(|wait| queued(wait)) {
        wait.stage = Stage::Marking { token };
    }
    Some(token)
}

/// The server answered a mark: that send's input has reached the pane, and this client's screen
/// holds all output from before it and none from after.
pub(crate) fn input_marked(ctx: &mut Context<AppRoot>, token: u64) -> Update {
    let now = Instant::now();
    let state = &mut ctx.state;
    let mut waits = std::mem::take(&mut state.capture_waits.waits);
    let mut marked = false;
    for wait in &mut waits {
        if !matches!(wait.stage, Stage::Marking { token: t } if t == token) {
            continue;
        }
        marked = true;
        if let Some(pane) = find_pane_in_namespace_mut(state, wait.pane_id, wait.local)
            .filter(|pane| pane.pty_generation == wait.generation)
        {
            let screen = pane.terminal.with_screen_mut(|screen| {
                ScreenWait::start(&wait.plan.wait, screen, true, wait.started, now)
            });
            wait.stage = Stage::Watching(screen);
        }
    }
    waits.append(&mut state.capture_waits.waits);
    state.capture_waits.waits = waits;
    if marked {
        evaluate(ctx, None);
    }
    Update::none()
}

/// The periodic look at every pending wait.
pub(crate) fn tick(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.capture_waits.ticking = false;
    evaluate(ctx, None);
    if ctx.state.capture_waits.waits.is_empty() {
        return Update::none();
    }
    ctx.state.capture_waits.ticking = true;
    Update::command_only(schedule_tick())
}

fn schedule_tick() -> Command {
    Command::after(TICK, move |link: CommandLink<Msg>| {
        link.send(Msg::CaptureWaitTick);
    })
}

/// Look again at the waits on `only` (a pane that just changed), or at all of them, and answer
/// every one that has ended.
fn evaluate(ctx: &mut Context<AppRoot>, only: Option<(PaneId, bool)>) {
    let now = Instant::now();
    let state = &mut ctx.state;
    let mut waits = std::mem::take(&mut state.capture_waits.waits);
    waits.retain_mut(|wait| {
        if only.is_some_and(|only| only != (wait.pane_id, wait.local)) {
            return true;
        }
        if !wait.local && wait.epoch != state.runtime_epoch {
            let _ = wait.reply.send(ControlResponse::error_with(
                ControlErrorCode::SessionNotAttached,
                "the session this pane belongs to was detached while waiting",
            ));
            return false;
        }
        let pane = find_pane_in_namespace_mut(state, wait.pane_id, wait.local)
            .filter(|pane| pane.pty_generation == wait.generation && !pane.closing);
        let end = match pane {
            None => Some(WaitEnd::Closed),
            Some(pane) => {
                let status = match &mut wait.stage {
                    Stage::Watching(screen) => {
                        pane.terminal
                            .with_screen_mut(|terminal| screen.observe(terminal, now));
                        screen.status(now)
                    }
                    Stage::Queued | Stage::Marking { .. } => {
                        let deadline =
                            wait.started + Duration::from_millis(wait.plan.wait.timeout_ms);
                        if now >= deadline {
                            WaitStatus::TimedOut
                        } else {
                            WaitStatus::Pending
                        }
                    }
                };
                match status {
                    WaitStatus::Ready => Some(WaitEnd::Ready),
                    WaitStatus::TimedOut => Some(WaitEnd::TimedOut),
                    WaitStatus::Pending if !pane.terminal.is_running() => Some(WaitEnd::Exited),
                    WaitStatus::Pending => None,
                }
            }
        };
        let Some(end) = end else {
            return true;
        };
        let response = wait
            .plan
            .reply
            .respond(wait.pane_id, &wait.plan.wait, end, |reply| {
                capture(state, wait.pane_id, wait.local, reply)
            });
        let _ = wait.reply.send(response);
        false
    });
    // A wait registered while these were being looked at stays, behind the older ones.
    waits.append(&mut state.capture_waits.waits);
    state.capture_waits.waits = waits;
}

fn capture(
    state: &mut crate::state::State,
    pane_id: PaneId,
    local: bool,
    reply: &WaitReply,
) -> std::result::Result<PaneCapture, ControlResponse> {
    let pane = find_pane_in_namespace_mut(state, pane_id, local).ok_or_else(|| {
        ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {pane_id} not found"),
        )
    })?;
    let content = pane.terminal.with_screen_mut(|screen| {
        crate::pane::capture_screen(
            screen,
            reply.scrollback.clone(),
            reply.render,
            reply.scale,
            reply.image_pixels,
        )
    })?;
    Ok(PaneCapture {
        id: pane_id,
        title: pane.terminal.title(),
        content,
    })
}

#[cfg(test)]
impl crate::state::State {
    pub(crate) fn capture_waits_are_empty(&self) -> bool {
        self.capture_waits.waits.is_empty()
    }
}
