//! Client-side state of a relaunch: the dialog that plans one, the claims gathered before the
//! client lets go, and the restore a relaunched client works through.

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
    /// Fences replies (a candidate found, a timeout) meant for a dialog that has since closed.
    pub generation: u64,
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
    /// Asking each server to hold this client's slot.
    Preparing(Preparation),
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
}

#[derive(Clone, Debug)]
pub struct PlannedSession {
    pub epoch: AttachmentId,
    pub facts: SessionFacts,
    pub transition: Transition,
}

#[derive(Debug)]
pub struct Preparation {
    pub plan: RelaunchPlan,
    /// Requests still unanswered, by request id, to the attachment that sent each.
    pub pending: HashMap<u64, AttachmentId>,
    /// Claim tokens the servers issued, by attachment.
    pub tokens: HashMap<AttachmentId, String>,
}

/// The sessions a relaunched client is attaching to, one at a time.
#[derive(Debug)]
pub struct Restore {
    pub queue: VecDeque<HandoffSession>,
    pub total: usize,
    /// The attach in flight, by its epoch.
    pub current: Option<(u64, HandoffSession)>,
    /// Whether the client was on the launcher rather than in a session.
    pub launcher: bool,
    /// What did not come back as it was, for the report at the end.
    pub problems: Vec<String>,
}

impl Restore {
    /// Sessions attached so far, counting the one in flight.
    pub fn done(&self) -> usize {
        self.total - self.queue.len()
    }

    /// Whether the session being attached now is a background one, which the view keeps covered
    /// so each does not flash by on its way to the background.
    pub fn covers_view(&self) -> bool {
        self.launcher || !self.queue.is_empty()
    }
}
