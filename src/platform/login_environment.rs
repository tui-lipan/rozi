//! The user's login environment, for processes rozi starts from a non-interactive SSH command.
//!
//! `ssh host rozi …` runs rozi through the user's shell without reading its login or interactive
//! files, so the process starts with the system's bare `PATH`: no Homebrew on macOS, no
//! `~/.local/bin` added by `~/.profile`. A session server or extension runtime started that way
//! hands the same bare environment to every pane, hook, and extension process it runs.
//!
//! [`apply_to`] fixes that at the two places such long-lived processes are started: it runs the
//! user's shell once as an interactive login shell (`$SHELL -l -i -c`), has it start this binary
//! with `--print-environment`, and applies what the user's files changed to the new process. The
//! shell inherits this process's environment, so only those changes are kept, and the rest of the
//! environment stays live. Anything else the shell prints is ignored: the environment is read only
//! between markers carrying a random nonce. A shell that fails, prints no environment, or takes
//! longer than [`CAPTURE_TIMEOUT`] leaves the environment as it was. `ROZI_RESOLVING_ENVIRONMENT=1`
//! is set while the shell runs, so a user can skip slow or interactive parts of their files.
//!
//! A process with a terminal (`SSH_TTY`) came from an interactive SSH login, whose shell already
//! read the login files and may have changed the environment since, as activating a virtualenv
//! does; it is left alone. So is a login shell outside the sh family and fish, which take
//! `-l -i -c` and expand the probe the same way.
//!
//! Unix only: Windows has no login shell, and its SSH server already gives a full user
//! environment.

use std::ffi::OsString;
use std::time::Duration;

/// Set for the shell [`apply_to`] runs, so the user's startup files can tell.
pub const RESOLVING_ENV: &str = "ROZI_RESOLVING_ENVIRONMENT";

/// Set for a process [`apply_to`] prepared, a pane included, so it never captures again: `1` when
/// it has the login environment, `0` when reading it failed.
pub const RESOLVED_ENV: &str = "ROZI_LOGIN_ENVIRONMENT";

/// How long the user's startup files may take before rozi gives up and keeps its environment.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Variables that describe the capturing shell rather than the user's environment. Each keeps the
/// value it has in this process, or stays unset.
const SHELL_LOCAL: &[&str] = &["_", "SHLVL", "PWD", "OLDPWD", RESOLVING_ENV, PROBE_ENV];

/// Names this binary for the capturing shell.
const PROBE_ENV: &str = "ROZI_ENVIRONMENT_PROBE";

/// Login shells that take `-l -i -c` and expand `"$VAR"` in the probe command.
const CAPTURABLE_SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "ksh", "mksh", "dash", "ash", "yash", "fish",
];

enum Outcome {
    /// This process did not start from a non-interactive SSH command.
    NotNeeded,
    /// What the user's login files set and unset, relative to this process's environment.
    Captured {
        set: Vec<(OsString, OsString)>,
        removed: Vec<OsString>,
    },
    Failed,
}

/// Read once per process, the first time a server or extension process is started.
fn outcome() -> &'static Outcome {
    static OUTCOME: std::sync::OnceLock<Outcome> = std::sync::OnceLock::new();
    OUTCOME.get_or_init(capture_for_this_process)
}

/// Give `command` the user's login environment when this process came from a non-interactive SSH
/// command, or mark that reading it failed, so the process does not try again.
pub fn apply_to(command: &mut std::process::Command) {
    match outcome() {
        Outcome::NotNeeded => {}
        Outcome::Captured { set, removed } => {
            for key in removed {
                command.env_remove(key);
            }
            command.envs(set.iter().map(|(key, value)| (key, value)));
        }
        Outcome::Failed => {
            command.env(RESOLVED_ENV, "0");
        }
    }
}

/// The `PATH` [`apply_to`] gives a process, when the user's login files changed it.
pub fn path_for_children() -> Option<&'static std::ffi::OsStr> {
    match outcome() {
        Outcome::Captured { set, .. } => set
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.as_os_str()),
        Outcome::NotNeeded | Outcome::Failed => None,
    }
}

fn capture_for_this_process() -> Outcome {
    if !cfg!(unix) || !started_by_ssh_command(|key| std::env::var_os(key)) {
        return Outcome::NotNeeded;
    }
    let Some(shell) = std::env::var_os("SHELL").filter(|shell| {
        let name = crate::platform::command::shell_name(&shell.to_string_lossy());
        CAPTURABLE_SHELLS.contains(&name.as_str())
    }) else {
        return Outcome::NotNeeded;
    };
    let Ok(exe) = std::env::current_exe() else {
        return Outcome::Failed;
    };
    let base: Vec<_> = std::env::vars_os().collect();
    match capture(&shell, &exe, &base, CAPTURE_TIMEOUT) {
        Ok(env) => {
            let removed = base
                .iter()
                .map(|(key, _)| key)
                .filter(|key| !env.iter().any(|(captured, _)| captured == *key))
                .cloned()
                .collect();
            let set = env
                .into_iter()
                .filter(|entry| !base.contains(entry))
                .collect();
            Outcome::Captured { set, removed }
        }
        Err(err) => {
            eprintln!(
                "rozi: kept the SSH environment; reading the login environment failed: {err}"
            );
            Outcome::Failed
        }
    }
}

/// Whether the environment `var` reads came from a non-interactive SSH command and no login
/// environment has been read for it yet.
fn started_by_ssh_command(var: impl Fn(&str) -> Option<OsString>) -> bool {
    let set = |key: &str| var(key).is_some_and(|value| !value.is_empty());
    set("SSH_CONNECTION") && !set("SSH_TTY") && var(RESOLVED_ENV).is_none()
}

/// Run `shell` as an interactive login shell in the environment `base`, have it start `exe
/// --print-environment`, and return the environment that printed, with [`RESOLVED_ENV`] set.
pub fn capture(
    shell: &std::ffi::OsStr,
    exe: &std::path::Path,
    base: &[(OsString, OsString)],
    timeout: Duration,
) -> std::io::Result<Vec<(OsString, OsString)>> {
    use std::io::Read;

    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| std::io::Error::other(err.to_string()))?;
    let nonce: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut command = std::process::Command::new(shell);
    command
        .args(["-l", "-i", "-c"])
        // Quoted for every shell family this supports; a shell that reads it differently prints
        // no nonce and the capture fails safe.
        .arg(format!("\"${PROBE_ENV}\" --print-environment {nonce}"))
        .env_clear()
        .envs(base.iter().map(|(key, value)| (key, value)))
        .env(PROBE_ENV, exe)
        .env(RESOLVING_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Its own session, without a controlling terminal: an interactive shell started from a rozi
    // that has one would otherwise take over its foreground and could write to the screen.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and touches no memory of this process.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() >= 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    let end = format!("\0{nonce}-end\0").into_bytes();
    // Read until the closing nonce rather than to end of file: a daemon the user's files start,
    // such as an SSH agent, can hold the pipe open long after the shell exits.
    std::thread::Builder::new()
        .name("rozi-login-env".to_string())
        .spawn(move || {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => output.extend_from_slice(&chunk[..read]),
                }
                if output
                    .windows(end.len())
                    .any(|window| window == end.as_slice())
                {
                    break;
                }
            }
            let _ = sender.send(output);
        })?;
    let output = receiver.recv_timeout(timeout);
    if output.is_err() {
        // Stuck, perhaps on a prompt: end everything the user's files started too, which frees the
        // pipe the reader waits on. `setsid` made the shell's id its group's.
        #[cfg(unix)]
        if let Ok(group) = libc::pid_t::try_from(child.id()) {
            // SAFETY: plain syscall on a process group this function created.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
    // The environment is in hand, or never will be; either way the shell has nothing left to do.
    let _ = child.kill();
    let _ = child.wait();
    let output = output.map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("the login shell took longer than {}s", timeout.as_secs()),
        )
    })?;
    let captured = parse(&output, &nonce)
        .ok_or_else(|| std::io::Error::other("the login shell did not print an environment"))?;
    Ok(settle(captured, base))
}

/// The `KEY=VALUE` entries between the two nonces in `output`, which also holds whatever the
/// user's startup files printed around them.
fn parse(output: &[u8], nonce: &str) -> Option<Vec<(OsString, OsString)>> {
    let start = format!("\0{nonce}-begin\0").into_bytes();
    let end = format!("{nonce}-end");
    let begin = output
        .windows(start.len())
        .position(|window| window == start.as_slice())?
        + start.len();
    let mut entries = Vec::new();
    for field in output[begin..].split(|byte| *byte == 0) {
        if field == end.as_bytes() {
            return Some(entries);
        }
        let split = field.iter().position(|byte| *byte == b'=')?;
        if split == 0 {
            return None;
        }
        entries.push((os_string(&field[..split]), os_string(&field[split + 1..])));
    }
    None
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

/// `captured` with the capturing shell's own variables put back as `base` has them, and
/// [`RESOLVED_ENV`] set.
fn settle(
    captured: Vec<(OsString, OsString)>,
    base: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let shell_local = |key: &OsString| SHELL_LOCAL.iter().any(|name| key == name);
    captured
        .into_iter()
        .filter(|(key, _)| !shell_local(key) && key != RESOLVED_ENV)
        .chain(base.iter().filter(|(key, _)| shell_local(key)).cloned())
        .chain(std::iter::once((RESOLVED_ENV.into(), "1".into())))
        .collect()
}

/// `--print-environment <nonce>`: write this process's environment, NUL-separated, between
/// `<nonce>-begin` and `<nonce>-end` fields, for [`capture`] to read.
pub fn print_environment(nonce: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut out = Vec::new();
    out.extend_from_slice(format!("\0{nonce}-begin\0").as_bytes());
    for (key, value) in std::env::vars_os() {
        out.extend_from_slice(key.as_encoded_bytes());
        out.push(b'=');
        out.extend_from_slice(value.as_encoded_bytes());
        out.push(0);
    }
    out.extend_from_slice(format!("{nonce}-end\0").as_bytes());
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&out)?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_non_interactive_ssh_command_that_has_not_captured_yet_captures() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        const SSH: (&str, &str) = ("SSH_CONNECTION", "10.0.0.2 5000 10.0.0.1 22");
        assert!(started_by_ssh_command(env(&[SSH])));
        // An interactive login already read the login files.
        assert!(!started_by_ssh_command(env(&[
            SSH,
            ("SSH_TTY", "/dev/ttys003")
        ])));
        assert!(!started_by_ssh_command(env(&[])));
        // A process whose parent already tried, even when that failed.
        assert!(!started_by_ssh_command(env(&[SSH, (RESOLVED_ENV, "0")])));
    }
}
