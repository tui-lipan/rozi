//! Client-side state of a relaunch: the dialog that plans one, the transaction that reserves and
//! commits this client's slots before it lets go, and the restore a relaunched client works through.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use semver::Version;

use super::AttachmentId;
use crate::relaunch::candidate::Candidate;
use crate::relaunch::document::HandoffSession;
use crate::relaunch::plan::{SessionFacts, Transition};

/// What the runner runs once the UI has given the terminal back.
#[derive(Clone, Debug)]
pub struct Handover {
    pub candidate: Candidate,
    /// Names the handoff document the new process reads.
    pub handoff_id: String,
}

/// Where the UI leaves a [`Handover`] for the runner that outlives it. Shared between the two:
/// the runner owns one end and reads it after the UI has finished tearing down, which is the only
/// moment the terminal is free to hand over.
#[derive(Clone, Debug, Default)]
pub struct HandoverSlot(Arc<Mutex<Option<Handover>>>);

impl HandoverSlot {
    pub fn set(&self, handover: Handover) {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(handover);
    }

    pub fn take(&self) -> Option<Handover> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
    }
}

#[derive(Debug, Default)]
pub struct RelaunchState {
    pub handover: HandoverSlot,
    /// Whether the installed rozi may be looked for by running it. Off in a test app; there the
    /// dialog waits on a [`crate::Msg::RelaunchCandidate`] the test sends itself.
    pub finds_binaries: bool,
    pub dialog: Option<RelaunchDialog>,
    pub restore: Option<Restore>,
    /// Fences a candidate search meant for a dialog that has since closed.
    pub generation: u64,
    /// Fences replies and timeouts meant for an attempt that has since ended.
    pub attempt: u64,
    pub next_request: u64,
}

#[derive(Debug)]
pub struct RelaunchDialog {
    pub generation: u64,
    /// The release an update just installed; `None` for a plain restart.
    pub expected: Option<Version>,
    /// A release-wide step the update's metadata raised, such as a new extension API.
    pub note: Option<String>,
    pub stage: RelaunchStage,
}

#[derive(Debug)]
pub enum RelaunchStage {
    /// Looking for the binary to run and asking it what it is.
    Checking,
    /// Nothing to relaunch into, and why.
    Unavailable(String),
    Ready(RelaunchPlan),
    /// Asking each server to reserve this client's slot.
    Preparing(Transaction),
    /// The handoff is written; asking each server to commit its reservation.
    Committing(Transaction),
    /// The relaunch stopped; waiting for each server to confirm it took back what it reserved.
    Aborting(Aborting),
}

/// What relaunching would do, as the dialog shows it.
#[derive(Clone, Debug)]
pub struct RelaunchPlan {
    pub candidate: Candidate,
    pub sessions: Vec<PlannedSession>,
    /// Processes this client runs itself, which cannot outlive it: scratchpad shells, a popup.
    pub ending: Vec<String>,
    /// Armed by a first confirmation of a plan that ends programs; the second one commits.
    pub armed: bool,
    /// Why the last attempt did not go ahead, shown until the next one.
    pub notice: Option<String>,
}

impl RelaunchPlan {
    pub fn blocked(&self) -> bool {
        self.sessions
            .iter()
            .any(|session| matches!(session.transition, Transition::Blocked { .. }))
    }

    pub fn destructive(&self) -> bool {
        !self.ending.is_empty()
            || self
                .sessions
                .iter()
                .any(|session| session.transition.is_destructive())
    }

    /// Whether `other`, planned again from the client as it is now, would do the same thing to the
    /// same sessions. Anything else - a session renamed, reconnected to another server, or newly
    /// attached - has to be shown again before it is acted on.
    pub fn same_as(&self, other: &RelaunchPlan) -> bool {
        self.sessions == other.sessions && self.ending == other.ending
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSession {
    pub epoch: AttachmentId,
    pub facts: SessionFacts,
    /// The server instance the plan was made against.
    pub instance: Option<String>,
    pub transition: Transition,
}

/// A relaunch in progress in the outgoing client: reservations asked for or committed, by request.
#[derive(Debug)]
pub struct Transaction {
    pub plan: RelaunchPlan,
    pub attempt: u64,
    /// Requests still unanswered, by request id, to the attachment that sent each.
    pub pending: HashMap<u64, AttachmentId>,
    /// Claim tokens the servers issued, by attachment.
    pub tokens: HashMap<AttachmentId, String>,
    /// The handoff document, once written.
    pub handoff_id: Option<String>,
    /// How long each server must hold this client's slot: until the new process can have reached
    /// that session, counting every session it attaches to first.
    pub required: HashMap<AttachmentId, std::time::Duration>,
}

/// A relaunch being rolled back.
#[derive(Debug)]
pub struct Aborting {
    pub attempt: u64,
    /// Abort requests still unconfirmed, by request id, to the attachment that sent each.
    pub pending: HashMap<u64, AttachmentId>,
    /// How each of those sessions is named, for a server that never confirms.
    pub labels: HashMap<AttachmentId, String>,
    /// What the dialog shows once every abort is confirmed.
    pub after: Box<RelaunchStage>,
}

/// One step of bringing a relaunched client back.
#[derive(Clone, Debug)]
pub enum RestoreStep {
    /// Attach to a running session, claiming this client's slot in it.
    Attach(HandoffSession),
    /// Restart a session on this build from its snapshot, then attach to it.
    Upgrade(HandoffSession),
    /// Put the session that was on screen back on screen, or the launcher, and finish.
    Land,
}

#[derive(Debug)]
pub enum RestoreInFlight {
    Attach {
        epoch: u64,
        session: HandoffSession,
    },
    Upgrade {
        attempt: u64,
        session: HandoffSession,
    },
}

/// The sessions a relaunched client is bringing back.
///
/// Every session the new build can attach to as it is comes first, so each slot its server holds
/// is claimed within moments and stays claimed by a live connection. Only then are the sessions
/// that need an upgrade restarted, which may take long - over SSH, or behind a confirmation to
/// install rozi on a host - and attached.
#[derive(Debug)]
pub struct Restore {
    pub steps: VecDeque<RestoreStep>,
    pub current: Option<RestoreInFlight>,
    /// Sessions to bring back, and how many have been dealt with.
    pub total: usize,
    pub done: usize,
    /// The session that was on screen, by name and host; `None` for the launcher.
    pub foreground: Option<(String, Option<String>)>,
    /// The attachment the session on screen landed in, once it has.
    pub foreground_epoch: Option<AttachmentId>,
    /// When each session was last parked, by name and host, to put the background back in order.
    pub recency: Vec<((String, Option<String>), u64)>,
    /// What did not come back as it was, for the report at the end.
    pub problems: Vec<String>,
}

/// What a relaunched client shows while it is restoring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreActivity {
    Attaching { done: usize, total: usize },
    Upgrading { session: String },
}

impl Restore {
    pub fn activity(&self) -> RestoreActivity {
        match &self.current {
            Some(RestoreInFlight::Upgrade { session, .. }) => RestoreActivity::Upgrading {
                session: match &session.remote {
                    Some(host) => format!("{}@{host}", session.name),
                    None => session.name.clone(),
                },
            },
            _ => RestoreActivity::Attaching {
                done: self.done,
                total: self.total,
            },
        }
    }
}
