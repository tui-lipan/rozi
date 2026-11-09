//! The runner's half of a relaunch: handing the terminal to the new build once the old UI is gone,
//! and, in the new build, getting ready to restore before its own UI starts.
//!
//! Both halves run with the terminal in its ordinary state - after the old UI tore down, before the
//! new one sets up - so what they print stays on the screen the shell gets back.

use std::ffi::OsString;

use crate::config::Config;
use crate::relaunch::document::ClientHandoff;
use crate::relaunch::plan::Transition;
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

/// What a relaunched client starts with: the sessions to restore, once the ones the previous build
/// could not carry over are upgraded onto this one. A handoff that cannot be read opens Sessions
/// instead, where every session the previous process held is still listed.
pub(super) fn resume_session(
    id: &str,
    config: &Config,
    messages: &mut Vec<String>,
) -> StartupSession {
    let mut handoff = match crate::relaunch::document::take(id) {
        Ok(handoff) => handoff,
        Err(error) => {
            messages.push(format!(
                "Restart could not bring your sessions back: {error}"
            ));
            return StartupSession::Picker {
                remote: None,
                last_session: None,
            };
        }
    };
    let problems = upgrade_sessions(&mut handoff, config);
    StartupSession::Resume { handoff, problems }
}

/// Restart every session the previous build could not carry over, on this build, from its snapshot.
/// A session that fails to upgrade is left out of the restore and reported.
fn upgrade_sessions(handoff: &mut ClientHandoff, config: &Config) -> Vec<String> {
    let mut problems = Vec::new();
    let mut index = 0;
    while index < handoff.sessions.len() {
        let session = &handoff.sessions[index];
        if session.transition != Transition::Upgrade {
            index += 1;
            continue;
        }
        let name = session.name.clone();
        let remote = session.remote.clone();
        let label = match &remote {
            Some(host) => format!("{name}@{host}"),
            None => name.clone(),
        };
        eprintln!(
            "rozi: upgrading session `{label}` to rozi {}…",
            env!("CARGO_PKG_VERSION")
        );
        match upgrade(&name, remote.as_deref(), config) {
            Ok(()) => index += 1,
            Err(error) => {
                eprintln!("rozi: `{label}` was not upgraded: {error}");
                problems.push(format!("`{label}` was not upgraded: {error}"));
                handoff.remove(index);
            }
        }
    }
    problems
}

fn upgrade(name: &str, remote: Option<&str>, config: &Config) -> Result<(), String> {
    let Some(spec) = remote else {
        return crate::session::server::upgrade_named_session(name).map_err(|e| e.to_string());
    };
    let target = crate::session::remote::parse_remote_target(spec)?;
    // The host needs a rozi that speaks this build's generation before the session can move to it.
    // Installing one asks first, on this terminal, unless `[remote] install` already decided.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    crate::session::remote::ensure_remote_binary(&target, &config.remote, interactive)?;
    let stderr = crate::session::remote::upgrade_remote_session(&target, name, &config.remote)?;
    let _ = std::io::Write::write_all(&mut std::io::stderr(), &stderr);
    Ok(())
}
