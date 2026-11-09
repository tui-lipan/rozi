//! The runner's half of a relaunch: handing the terminal to the new build once the old UI is gone,
//! and, in the new build, getting ready to restore before its own UI starts.
//!
//! Both halves run with the terminal in its ordinary state - after the old UI tore down, before the
//! new one sets up - so what they print stays on the screen the shell gets back.

use std::ffi::OsString;

use crate::state::Handover;

use super::startup::StartupSession;

/// The argument that tells a starting rozi to come back as the client that relaunched it.
pub(crate) const RESUME_FLAG: &str = "--resume-handoff";

/// Replace this process with the build `handover` names. Returns only when that could not happen;
/// on Windows, where the new build runs as a child, this process leaves with its exit code.
pub(super) fn hand_over(handover: Handover) -> ! {
    let Handover {
        candidate,
        handoff_id,
    } = handover;
    let path = candidate.path.clone();
    if !candidate.unchanged() {
        eprintln!(
            "rozi: not restarted: {} changed after it was checked. Your sessions are still running; start rozi again.",
            path.display()
        );
        std::process::exit(1);
    }
    let args = [OsString::from(RESUME_FLAG), OsString::from(handoff_id)];
    match crate::platform::relaunch::replace_process(&path, &args) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!(
                "rozi: could not start {}: {error}. Your sessions are still running; start rozi again.",
                path.display()
            );
            std::process::exit(1)
        }
    }
}

/// What a relaunched client starts with: the sessions the handoff names. A handoff that cannot be
/// read opens Sessions instead, where every session the previous process held is still listed.
///
/// Nothing is done to any session here. The new client claims the sessions it carries over first,
/// and only then upgrades the ones that need it, inside its UI, so no upgrade - however long it
/// takes - can outlast the slots the servers hold for it.
pub(super) fn resume_session(id: &str, messages: &mut Vec<String>) -> StartupSession {
    match crate::relaunch::document::take(id) {
        Ok(handoff) => StartupSession::Resume { handoff },
        Err(error) => {
            messages.push(format!(
                "Restart could not bring your sessions back: {error}"
            ));
            StartupSession::Picker {
                remote: None,
                last_session: None,
            }
        }
    }
}
