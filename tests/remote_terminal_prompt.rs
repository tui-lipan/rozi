//! An ssh that asks its question on the terminal, as OpenSSH does for an unknown host key, must be
//! able to read the answer when rozi runs it from a terminal with no UI around it.
//!
//! rozi runs each ssh in a process group of its own, and the kernel stops any group that is not the
//! terminal's foreground group when it reads from the terminal. Without lending the terminal, the
//! prompt's ssh sits stopped and the command looks frozen on a blank screen until its deadline.
//!
//! Each ssh stand-in stops itself before it reads, so the tests do not depend on which process
//! the scheduler runs first.
//!
//! The terminal comes from `script`, so this runs on Linux only (util-linux `script`).
#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs `shell_command` under `bash` in a terminal from `script`, with the `ssh` on `PATH` stopping
/// itself with `stop` before it asks, and returns whether the run finished, the answers ssh read,
/// and what the shell logged.
fn run_in_terminal(stop: &str, shell_command: &str) -> (bool, String, String) {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("transport");
    let answers = root.path().join("answers.log");
    let shell_log = root.path().join("shell.log");
    std::fs::create_dir_all(&bin).unwrap();
    // Asks once, on the terminal, the way OpenSSH asks about an unknown host key, then refuses the
    // connection whatever the answer. Later probes fail at once, so only the first one waits.
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            r#"#!/bin/sh
[ -s "$ROZI_TEST_ANSWERS" ] && exit 255
kill -{stop} $$
printf 'Are you sure you want to continue connecting (yes/no)? ' > /dev/tty
IFS= read -r answer < /dev/tty
printf '%s\n' "$answer" >> "$ROZI_TEST_ANSWERS"
echo 'Host key verification failed.' >&2
exit 255
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut child = Command::new("script")
        .args(["--quiet", "--return", "--command"])
        .arg(format!("bash --norc --noprofile -c '{shell_command}'"))
        .arg("/dev/null")
        .env("ROZI", env!("CARGO_BIN_EXE_rozi"))
        .env("ROZI_TEST_ANSWERS", &answers)
        .env("SHELL_LOG", &shell_log)
        .env("HOME", root.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI_BIN")
        .env_remove("ROZI_PANE")
        .env_remove("SSH_ASKPASS")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The line discipline keeps the answer until the prompt reads it. The input stays open until
    // the run ends, as a person's terminal does: `script` turns its end into an end-of-file on the
    // terminal, which a shell bringing a job back with `fg` would see.
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"yes\n").unwrap();

    let started = Instant::now();
    let finished = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(input);
    (
        finished,
        std::fs::read_to_string(&answers).unwrap_or_default(),
        std::fs::read_to_string(&shell_log).unwrap_or_default(),
    )
}

fn have_script() -> bool {
    let found = Command::new("script").arg("--version").output().is_ok()
        && Command::new("bash").arg("--version").output().is_ok();
    if !found {
        eprintln!("skipping: util-linux `script` or bash is not installed");
    }
    found
}

/// The stand-in stops itself with `SIGTTIN`, as the kernel stops an ssh that read the terminal
/// before it was lent. rozi has to continue it, whichever process ran first.
#[test]
fn a_cli_ssh_reads_its_host_key_answer_from_the_terminal() {
    if !have_script() {
        return;
    }
    let (finished, answers, _) =
        run_in_terminal("TTIN", r#""$ROZI" sessions list --remote fixture"#);
    assert!(
        finished && answers.lines().next() == Some("yes"),
        "the prompt never got its answer (finished: {finished}, answers: {answers:?})"
    );
}

/// `Ctrl+Z` at the prompt stops ssh with `SIGTSTP`. rozi must not undo that: it suspends with it,
/// so the shell sees the job stopped, and on `fg` lends the terminal back and lets ssh carry on.
#[test]
fn ctrl_z_at_an_ssh_prompt_suspends_rozi_until_fg() {
    if !have_script() {
        return;
    }
    // `set -m` gives the shell job control: rozi runs as a job in its own group, and a stop of
    // that job returns 128 + SIGTSTP from the foreground wait.
    let (finished, answers, shell) = run_in_terminal(
        "TSTP",
        r#"set -m; "$ROZI" sessions list --remote fixture; echo "returned=$?" >> "$SHELL_LOG"; fg; echo "fg=done" >> "$SHELL_LOG""#,
    );
    assert!(
        finished && shell.contains("returned=148") && answers.lines().next() == Some("yes"),
        "rozi did not suspend and resume with its ssh (finished: {finished}, shell: {shell:?}, answers: {answers:?})"
    );
}
