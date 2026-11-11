//! An ssh that asks its question on the terminal, as OpenSSH does for an unknown host key, must be
//! able to read the answer when rozi runs it from a terminal with no UI around it.
//!
//! rozi runs each ssh in a process group of its own, and the kernel stops any group that is not the
//! terminal's foreground group when it reads from the terminal. Without lending the terminal, the
//! prompt's ssh sits stopped and the command looks frozen on a blank screen until its deadline.
//!
//! The terminal comes from `script`, so this runs on Linux only (util-linux `script`).
#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_cli_ssh_reads_its_host_key_answer_from_the_terminal() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipping: util-linux `script` is not installed");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("transport");
    let log = root.path().join("answers.log");
    std::fs::create_dir_all(&bin).unwrap();
    // Asks once, on the terminal, the way OpenSSH asks about an unknown host key, then refuses the
    // connection whatever the answer. Later probes fail at once, so only the first one waits.
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        br#"#!/bin/sh
[ -s "$ROZI_TEST_ANSWERS" ] && exit 255
printf 'Are you sure you want to continue connecting (yes/no)? ' > /dev/tty
IFS= read -r answer < /dev/tty
printf '%s\n' "$answer" >> "$ROZI_TEST_ANSWERS"
echo 'Host key verification failed.' >&2
exit 255
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let rozi = env!("CARGO_BIN_EXE_rozi");
    let mut child = Command::new("script")
        .args(["--quiet", "--return", "--command"])
        .arg(format!("'{rozi}' sessions list --remote fixture"))
        .arg("/dev/null")
        .env("ROZI_TEST_ANSWERS", &log)
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
    // The line discipline keeps the answer until the prompt reads it.
    child.stdin.take().unwrap().write_all(b"yes\n").unwrap();

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
    let answers = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        finished && answers.lines().next() == Some("yes"),
        "the prompt never got its answer (finished: {finished}, answers: {answers:?})"
    );
}
