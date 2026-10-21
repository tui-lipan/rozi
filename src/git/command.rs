//! Bounded, argv-only Git execution on the current host.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::platform::command::CommandOutput;

pub(crate) const BROWSE_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const WORKTREE_LIST_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const WORKTREE_MUTATION_TIMEOUT: Duration = Duration::from_secs(120);
const CAPTURE_LIMIT: usize = 4 * 1024 * 1024;

pub(crate) const REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn run(
    dir: &Path,
    args: &[OsString],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    run_with_env(dir, args, &[], timeout)
}

fn run_with_env(
    dir: &Path,
    args: &[OsString],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    if !crate::platform::command::program_exists("git") {
        return Err("git was not found on the session server's PATH".to_string());
    }
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push(OsString::from("-C"));
    argv.push(dir.as_os_str().to_os_string());
    argv.extend_from_slice(args);
    let output = crate::platform::command::run_bounded_argv_command_with_env(
        "git",
        &argv,
        env,
        None,
        timeout,
        CAPTURE_LIMIT,
    )
    .map_err(|err| format!("git failed: {err}"))?;
    if output.timed_out {
        return Err("git timed out".to_string());
    }
    Ok(output)
}

pub(crate) fn checked(dir: &Path, args: &[OsString], timeout: Duration) -> Result<Vec<u8>, String> {
    finish(run(dir, args, timeout)?)
}

/// [`checked`] for a command that talks to a remote: Git must fail rather than ask for credentials
/// the session server has nowhere to show.
pub(crate) fn checked_remote(dir: &Path, args: &[OsString]) -> Result<Vec<u8>, String> {
    let env = [("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())];
    finish(run_with_env(dir, args, &env, REMOTE_TIMEOUT)?)
}

fn finish(output: CommandOutput) -> Result<Vec<u8>, String> {
    if output.status != Some(0) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git exited with status {:?}", output.status)
        } else {
            stderr
        });
    }
    Ok(output.stdout)
}
