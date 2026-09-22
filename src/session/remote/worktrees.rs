//! `rozi --remote <HOST> worktrees ...`: one worktree call, forwarded over SSH.
//!
//! Like [`super::control`], what crosses the connection is the parsed call as JSON on stdin, never
//! argv. Branch names and paths are arbitrary text, and OpenSSH rebuilds its arguments into a remote
//! shell command, so passing them as arguments would hand them to that shell to reinterpret. Paths
//! also stay opaque here: the far side resolves `~` and relative paths with its own rules.

use std::io::{Read, Write};
use std::process::Stdio;

use crate::config::RemoteConfig;
use crate::session::worktrees::{HostCall, HostReply};

use super::target::RemoteTarget;
use super::{
    ResolvedRemote, append_ssh_destination, ssh_base_command, validate_remote_executable_token,
    validate_remote_target,
};

/// The hidden flag the far side runs. It takes no arguments; the call arrives on stdin.
pub(crate) const RUNNER_FLAG: &str = "--remote-worktrees";

/// Run one worktree call on `target`. `Err` means the call never ran there; a call the host ran
/// and refused comes back as [`HostReply::Failed`].
pub fn forward(
    target: &RemoteTarget,
    call: &HostCall,
    config: &RemoteConfig,
) -> Result<HostReply, String> {
    validate_remote_target(target)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let remote_bin = super::binary::resolve(target, config)?;
    validate_remote_executable_token(&remote_bin)?;
    let payload = serde_json::to_vec(call)
        .map_err(|err| format!("could not encode the worktree request: {err}"))?;

    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    command.arg(&remote_bin);
    command.arg(RUNNER_FLAG);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("remote worktrees ssh failed: {err}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "remote worktrees ssh gave no stdin".to_string())?;
        stdin
            .write_all(&payload)
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|err| format!("could not send the worktree request: {err}"))?;
    }
    // Unbounded like control forwarding: the host bounds each Git command itself, and a large
    // checkout can legitimately take a while to create.
    let output = child
        .wait_with_output()
        .map_err(|err| format!("remote worktrees ssh failed: {err}"))?;
    if !output.status.success() {
        super::binary::invalidate(target, config);
        return Err(forward_failure(
            &String::from_utf8_lossy(&output.stderr),
            &target.display_label(),
        ));
    }
    parse_reply(&output.stdout)
}

/// The answer is the last parseable line: a login banner may share stdout with it.
fn parse_reply(stdout: &[u8]) -> Result<HostReply, String> {
    stdout
        .split(|byte| *byte == b'\n')
        .rev()
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .find_map(|line| serde_json::from_slice(line).ok())
        .ok_or_else(|| "could not read the remote rozi's worktree answer".to_string())
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

    #[test]
    fn the_reply_is_found_past_a_login_banner() {
        let reply = HostReply::Removed {
            path: "/src/repo-worktrees/feat".into(),
        };
        let mut stdout = b"Welcome to workbox!\n".to_vec();
        stdout.extend_from_slice(&serde_json::to_vec(&reply).expect("encodes"));
        stdout.extend_from_slice(b"\nlogout\n");
        assert_eq!(parse_reply(&stdout), Ok(reply));
        assert!(parse_reply(b"\n  \n").is_err());
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
