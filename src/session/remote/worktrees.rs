//! `rozi --remote <HOST> worktrees ...`: one worktree call, forwarded over SSH.
//!
//! Like [`super::control`], what crosses the connection is the parsed call as JSON on stdin, never
//! argv. Branch names and paths are arbitrary text, and OpenSSH rebuilds its arguments into a remote
//! shell command, so passing them as arguments would hand them to that shell to reinterpret. Paths
//! also stay opaque here: the far side resolves `~` and relative paths with its own rules.

use std::io::Read;

use crate::config::RemoteConfig;
use crate::session::worktrees::{HostCall, HostReply};

use super::json_transport::{self, TransportError};
use super::target::RemoteTarget;

/// The hidden flag the far side runs. It takes no arguments; the call arrives on stdin.
pub(crate) const RUNNER_FLAG: &str = "--remote-worktrees";

/// Run one worktree call on `target`. `Err` means the call never ran there; a call the host ran
/// and refused comes back as [`HostReply::Failed`].
pub fn forward(
    target: &RemoteTarget,
    call: &HostCall,
    config: &RemoteConfig,
) -> Result<HostReply, String> {
    json_transport::forward(target, config, &[RUNNER_FLAG], call).map_err(|error| match error {
        TransportError::Preflight(message) => message,
        TransportError::Encode(error) => format!("could not encode the worktree request: {error}"),
        TransportError::Ssh(error) => format!("remote worktrees ssh failed: {error}"),
        TransportError::Send(error) => format!("could not send the worktree request: {error}"),
        TransportError::Failed(output) => forward_failure(
            &String::from_utf8_lossy(&output.stderr),
            &target.display_label(),
        ),
        TransportError::Answer(_) => "could not read the remote rozi's worktree answer".into(),
    })
}

fn forward_failure(stderr: &str, host: &str) -> String {
    let stderr = stderr.trim();
    if stderr.contains(&format!("unknown flag `{RUNNER_FLAG}`")) {
        return format!(
            "remote worktrees failed: the rozi on `{host}` is too old to manage worktrees (update rozi there)"
        );
    }
    if stderr.is_empty() {
        return format!("remote worktrees failed on `{host}`");
    }
    format!("remote worktrees failed: {stderr}")
}

/// The far side of [`forward`]: one JSON [`HostCall`] on stdin, one JSON [`HostReply`] line out.
pub fn run_remote_worktrees() -> Result<HostReply, String> {
    let mut payload = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut payload)
        .map_err(|err| format!("could not read the worktree request: {err}"))?;
    let call: HostCall = serde_json::from_str(payload.trim())
        .map_err(|err| format!("could not parse the worktree request: {err}"))?;
    Ok(crate::session::worktrees::run_host_call(call))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::PROTOCOL_VERSION;
    use crate::session::worktrees::CheckoutSelector;

    /// The protocol version at which [`HostCall`] last changed shape. The far side decodes only its
    /// own build's shape and [`forward`] picks that build by protocol range alone, so a shape change
    /// bumps `PROTOCOL_VERSION` and this together.
    const HOST_CALL_SHAPE_SINCE: u32 = 37;
    const _: () = assert!(PROTOCOL_VERSION >= HOST_CALL_SHAPE_SINCE);

    #[test]
    fn host_call_shapes_are_pinned_to_a_protocol_version() {
        let calls = [
            HostCall::Resolve {
                cwd: None,
                checkout: CheckoutSelector::Auto("feat/x".into()),
            },
            HostCall::Unlock {
                cwd: Some("~/src/repo".into()),
                checkout: CheckoutSelector::Branch("feat/x".into()),
            },
            HostCall::Remove {
                cwd: None,
                checkout: CheckoutSelector::Path("/wt/x".into()),
                force: false,
                delete_branch: None,
            },
        ];
        assert_eq!(
            serde_json::to_value(calls).expect("encodes"),
            serde_json::json!([
                {"op": "resolve", "cwd": null, "checkout": {"auto": "feat/x"}},
                {"op": "unlock", "cwd": "~/src/repo", "checkout": {"branch": "feat/x"}},
                {"op": "remove", "cwd": null, "checkout": {"path": "/wt/x"}, "force": false},
            ]),
            "HostCall changed shape: bump PROTOCOL_VERSION and HOST_CALL_SHAPE_SINCE"
        );
    }

    #[test]
    fn a_remote_rozi_from_before_the_current_shape_is_not_selected() {
        let older = HOST_CALL_SHAPE_SINCE - 1;
        let report = super::super::bootstrap::parse_probe_output(&format!(
            "candidate=/old\nspeaks_remote=1\nprotocol_min={older}\nprotocol_max={older}\n"
        ));
        assert!(matches!(
            super::super::bootstrap::select_compatible(&report),
            super::super::bootstrap::ProbeResult::Missing { .. }
        ));
    }

    #[test]
    fn an_old_remote_rozi_is_named_as_version_skew() {
        assert_eq!(
            forward_failure(
                "unknown flag `--remote-worktrees`\nRun `rozi --help`",
                "workbox"
            ),
            "remote worktrees failed: the rozi on `workbox` is too old to manage worktrees (update rozi there)"
        );
    }
}
